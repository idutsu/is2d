use eframe::egui::Color32;
use i_overlay::{
    core::{fill_rule::FillRule as OverlayFillRule, overlay_rule::OverlayRule, solver::Solver},
    float::{filter::ContourFilter, overlay::FloatOverlay},
};
use kurbo::{
    ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, Point, QuadBez,
    Rect as KRect,
};

use crate::{
    vector_engine::{
        dedup_closed_ring, fill_cumulative_arc_lengths, normalize_vec, rects_overlap,
        ring_area_and_perimeter_f64, signed_area_f64, Contour, FillShape,
    },
    vector_stroke::{
        estimate_brush_radius_from_ring, fit_quad_control_point_with_arc_lengths, rdp_recursive,
        smooth_ring_to_quad_contour,
    },
};

// ============================================================================
// アルゴリズム 3: 平面ブーリアン演算（同色 Union / 異色・消しゴム Difference）
// & De Casteljau 2次ベジェ曲線ハイブリッド再構築エンジン
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BoolOp {
    Union,
    Difference,
    Intersection,
}

/// `i_overlay` に入出力する1図形分の閉リング配列（先頭が外周、2番目以降が穴）
type OverlayShape = Vec<Vec<[f64; 2]>>;

/// 新しいブラシストロークまたは変形確定シェイプをキャンバスへマージする（Flashのマージ描画モードの核心挙動）。
/// 同一の `group_path` を持つ生シェイプ同士のみが結合（`Union`）・切り抜き（`Difference`）を行い、
/// 異なるグループに属するシェイプは保護される。
pub(crate) fn merge_shapes_into_canvas(canvas: &mut Vec<FillShape>, incoming: Vec<FillShape>) {
    merge_shapes_into_canvas_inner(canvas, incoming, true);
}

/// 他の異色シェイプと重ならない塗りシェイプ（バケツ塗り・リカラー）をキャンバスへマージし、
/// 同じ `group_path` の隣接する同色シェイプがあればシームレスに結合（Union）する。
pub(crate) fn merge_disjoint_fill_into_canvas(canvas: &mut Vec<FillShape>, fill_shape: FillShape) {
    merge_shapes_into_canvas_inner(canvas, vec![fill_shape], false);
}

fn merge_shapes_into_canvas_inner(
    canvas: &mut Vec<FillShape>,
    incoming: Vec<FillShape>,
    subtract_diff_color: bool,
) {
    if incoming.is_empty() {
        return;
    }
    let target_color = incoming[0].color;
    let target_layer_id = incoming[0].layer_id;
    let target_group_path = incoming[0].group_path.clone();
    let test_bbox = union_bounding_boxes(&incoming).inflate(1.5, 1.5);

    let insert_limit = if target_group_path.is_empty() {
        canvas
            .iter()
            .rposition(|s| s.layer_id == target_layer_id)
            .map(|pos| pos + 1)
            .unwrap_or(canvas.len())
    } else {
        canvas
            .iter()
            .rposition(|s| {
                s.layer_id == target_layer_id && s.is_in_editing_scope(&target_group_path)
            })
            .or_else(|| canvas.iter().rposition(|s| s.layer_id == target_layer_id))
            .map(|pos| pos + 1)
            .unwrap_or(canvas.len())
    };
    let trailing_outside = canvas.split_off(insert_limit);

    let mut updated_canvas =
        Vec::with_capacity(canvas.len() + incoming.len() + trailing_outside.len());
    let mut overlapping_same_color = Vec::new();

    for existing in canvas.drain(..) {
        if existing.layer_id != target_layer_id || existing.group_path != target_group_path {
            updated_canvas.push(existing);
            continue;
        }
        let intersects_bbox = rects_overlap(existing.bounding_box(), test_bbox);
        if existing.color == target_color {
            if intersects_bbox {
                overlapping_same_color.push(existing);
            } else {
                updated_canvas.push(existing);
            }
        } else if subtract_diff_color && intersects_bbox {
            let color = existing.color;
            updated_canvas.extend(
                boolean_operation(&[existing], &incoming, BoolOp::Difference, color)
                    .into_iter()
                    .map(|s| s.with_scope(target_layer_id, &target_group_path)),
            );
        } else {
            updated_canvas.push(existing);
        }
    }

    if overlapping_same_color.is_empty() {
        updated_canvas.extend(incoming);
    } else {
        updated_canvas.extend(
            boolean_operation(
                &overlapping_same_color,
                &incoming,
                BoolOp::Union,
                target_color,
            )
            .into_iter()
            .map(|s| s.with_scope(target_layer_id, &target_group_path)),
        );
    }

    updated_canvas.extend(trailing_outside);
    *canvas = updated_canvas;
}

/// 消しゴムの輪郭で、同じ編集スコープ（`group_path`）直下の生シェイプに対してのみ Boolean Difference を適用する
pub(crate) fn erase_from_canvas(canvas: &mut Vec<FillShape>, eraser_shapes: &[FillShape]) {
    if eraser_shapes.is_empty() {
        return;
    }
    let target_group_path = &eraser_shapes[0].group_path;
    let test_bbox = union_bounding_boxes(eraser_shapes).inflate(1.5, 1.5);

    let mut updated_canvas = Vec::with_capacity(canvas.len());
    for existing in canvas.drain(..) {
        if existing.group_path == *target_group_path
            && rects_overlap(existing.bounding_box(), test_bbox)
        {
            let color = existing.color;
            let layer_id = existing.layer_id;
            updated_canvas.extend(
                boolean_operation(&[existing], eraser_shapes, BoolOp::Difference, color)
                    .into_iter()
                    .map(|s| s.with_scope(layer_id, target_group_path)),
            );
        } else {
            updated_canvas.push(existing);
        }
    }
    *canvas = updated_canvas;
}

pub(crate) fn union_bounding_boxes(shapes: &[FillShape]) -> KRect {
    let Some(first) = shapes.first() else {
        return KRect::ZERO;
    };
    shapes[1..]
        .iter()
        .fold(first.bounding_box(), |acc, s| acc.union(s.bounding_box()))
}

/// `i_overlay` クレート（`FloatOverlay`）を用いたハイブリッド2次ベジェブーリアン演算＆再フィッティング
pub(crate) fn boolean_operation(
    subj: &[FillShape],
    clip: &[FillShape],
    op: BoolOp,
    out_color: Color32,
) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let subj_shapes: Vec<OverlayShape> = subj
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();
    let clip_shapes: Vec<OverlayShape> = clip
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();

    if (subj_shapes.is_empty() && op == BoolOp::Difference)
        || (op == BoolOp::Intersection && (subj_shapes.is_empty() || clip_shapes.is_empty()))
        || (subj_shapes.is_empty() && clip_shapes.is_empty())
    {
        return Vec::new();
    }

    let overlay_rule = match op {
        BoolOp::Union => OverlayRule::Union,
        BoolOp::Difference => OverlayRule::Difference,
        BoolOp::Intersection => OverlayRule::Intersect,
    };

    let overlay_result = run_float_overlay(
        &subj_shapes,
        &clip_shapes,
        overlay_rule,
        OverlayFillRule::NonZero,
        0.04,
    );
    reconstruct_shapes_from_overlay(&overlay_result, &quad_table, out_color, op == BoolOp::Union)
}

/// マーキー矩形 `clip_rect_shape` で単一シェイプ `shape` をスライスし、
/// 1回のフラット化＆`quad_table` 構築で `(矩形内 inside, 矩形外 outside)` を同時に返す
pub(crate) fn slice_shape_with_marquee_rect(
    shape: &FillShape,
    clip_rect_shape: &FillShape,
) -> (Vec<FillShape>, Vec<FillShape>) {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let subj = flatten_shape_for_overlay(shape, &mut quad_table);
    let clip = flatten_shape_for_overlay(clip_rect_shape, &mut quad_table);
    if subj.is_empty() || clip.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let subj_slice = std::slice::from_ref(&subj);
    let clip_slice = std::slice::from_ref(&clip);
    let inside_rings = run_float_overlay(
        subj_slice,
        clip_slice,
        OverlayRule::Intersect,
        OverlayFillRule::NonZero,
        0.04,
    );
    let inside = reconstruct_shapes_from_overlay(&inside_rings, &quad_table, shape.color, false);
    if inside.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let outside_rings = run_float_overlay(
        subj_slice,
        clip_slice,
        OverlayRule::Difference,
        OverlayFillRule::NonZero,
        0.04,
    );
    let outside = reconstruct_shapes_from_overlay(&outside_rings, &quad_table, shape.color, false);
    (inside, outside)
}

fn reconstruct_shapes_from_overlay(
    overlay_result: &[OverlayShape],
    quad_table: &[QuadRecord],
    color: Color32,
    filter_sliver_holes: bool,
) -> Vec<FillShape> {
    overlay_result
        .iter()
        .filter_map(|poly_shape| {
            reconstruct_fill_shape_from_rings(poly_shape, quad_table, color, filter_sliver_holes)
        })
        .collect()
}

fn run_float_overlay(
    subj: &[OverlayShape],
    clip: &[OverlayShape],
    overlay_rule: OverlayRule,
    fill_rule: OverlayFillRule,
    min_area: f64,
) -> Vec<OverlayShape> {
    let filter = ContourFilter {
        min_area,
        simplify: false,
    };
    FloatOverlay::with_subj_and_clip(subj, clip).overlay_with_filter_and_solver(
        overlay_rule,
        fill_rule,
        filter,
        Solver::AUTO,
    )
}

fn reconstruct_fill_shape_from_rings(
    poly_shape: &[Vec<[f64; 2]>],
    quad_table: &[QuadRecord],
    color: Color32,
    filter_sliver_holes: bool,
) -> Option<FillShape> {
    if poly_shape.is_empty() {
        return None;
    }
    let exterior = reconstruct_or_refit_quad_contour(&poly_shape[0], quad_table, true)?;
    let holes = poly_shape[1..]
        .iter()
        .filter(|hole_ring| {
            if !filter_sliver_holes {
                return true;
            }
            let (a, p) = ring_area_and_perimeter_f64(hole_ring);
            a >= 1.5 && (a / p.max(1e-3)) >= 0.15
        })
        .filter_map(|hole_ring| reconstruct_or_refit_quad_contour(hole_ring, quad_table, false))
        .collect();
    Some(FillShape::new(color, exterior, holes))
}

/// キャンバス上のシェイプ群によって完全に囲まれた空白領域（単一シェイプの穴、または複数シェイプで囲まれた閉領域）のうち、
/// 指定座標 `pt` を含むものを検出し、周囲の2次ベジェ曲線をそのまま引き継いだ `FillShape` を生成する。
pub(crate) fn find_enclosed_region_at(
    canvas: &[FillShape],
    pt: Point,
    fill_color: Color32,
) -> Option<FillShape> {
    if canvas.is_empty() {
        return None;
    }
    let all_bbox = union_bounding_boxes(canvas);
    if !all_bbox.contains(pt) {
        return None;
    }

    let bounds = all_bbox.inflate(32.0, 32.0);
    let mut bounds_ring = vec![
        [bounds.x0, bounds.y0],
        [bounds.x1, bounds.y0],
        [bounds.x1, bounds.y1],
        [bounds.x0, bounds.y1],
    ];
    if signed_area_f64(&bounds_ring) > 0.0 {
        bounds_ring.reverse();
    }

    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let clip_shapes: Vec<OverlayShape> = canvas
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();
    if clip_shapes.is_empty() {
        return None;
    }

    let overlay_result = run_float_overlay(
        &[vec![bounds_ring]],
        &clip_shapes,
        OverlayRule::Difference,
        OverlayFillRule::NonZero,
        0.5,
    );

    for poly_shape in overlay_result {
        if poly_shape.is_empty() {
            continue;
        }
        let ext_ring = &poly_shape[0];
        let touches_bounds = ext_ring.iter().any(|&[x, y]| {
            (x - bounds.x0).abs() < 4.0
                || (x - bounds.x1).abs() < 4.0
                || (y - bounds.y0).abs() < 4.0
                || (y - bounds.y1).abs() < 4.0
        });
        if touches_bounds {
            continue;
        }

        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for &[x, y] in ext_ring {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        if pt.x < min_x - 1.0 || pt.x > max_x + 1.0 || pt.y < min_y - 1.0 || pt.y > max_y + 1.0 {
            continue;
        }
        if !poly_shape_contains_point(&poly_shape, pt) {
            continue;
        }

        let Some(candidate) =
            reconstruct_fill_shape_from_rings(&poly_shape, &quad_table, fill_color, false)
        else {
            continue;
        };
        if candidate.contains_point(pt) {
            return Some(candidate);
        }
    }

    None
}

/// ポリゴンリング群（外周＋穴）が点 `pt` を内包するかを偶奇レイキャスト法で高速判定する（ベジェ再構築・メッシュ化前のフィルタ用）
fn poly_shape_contains_point(poly_shape: &[Vec<[f64; 2]>], pt: Point) -> bool {
    let mut inside = false;
    for ring in poly_shape {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let [xi, yi] = ring[i];
            let [xj, yj] = ring[j];
            if ((yi > pt.y) != (yj > pt.y))
                && (pt.x < (xj - xi) * (pt.y - yi) / (yj - yi) + xi)
            {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

#[derive(Clone, Copy, Debug)]
struct QuadRecord {
    quad: QuadBez,
    bbox: KRect,
}

#[derive(Clone, Copy, Debug)]
struct TaggedEdge {
    from: Point,
    to: Point,
    quad_match: Option<(usize, f64)>,
}

fn flatten_contour_ring(
    contour: &Contour,
    orient_as_outer: Option<bool>,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table)?;
    if let Some(is_outer) = orient_as_outer {
        let area = signed_area_f64(&ring);
        if area.abs() < 0.25 {
            return None;
        }
        if (is_outer && area > 0.0) || (!is_outer && area < 0.0) {
            ring.reverse();
        }
    } else {
        ring.reverse();
    }
    Some(ring)
}

fn flatten_shape_rings(
    shape: &FillShape,
    orient_by_area: bool,
    quad_table: &mut Vec<QuadRecord>,
) -> OverlayShape {
    let Some(ext_ring) =
        flatten_contour_ring(&shape.exterior, orient_by_area.then_some(true), quad_table)
    else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) =
            flatten_contour_ring(hole, orient_by_area.then_some(false), quad_table)
        {
            rings.push(hole_ring);
        }
    }
    rings
}

fn flatten_shape_for_overlay(
    shape: &FillShape,
    quad_table: &mut Vec<QuadRecord>,
) -> OverlayShape {
    flatten_shape_rings(shape, true, quad_table)
}

/// 輪郭変形（エッジつまみ変形・頂点移動）後のシェイプの自己交差・反対側輪郭の突き抜けを解消し、
/// 正のワインディング領域（`OverlayFillRule::Positive`）のみを残して1つまたは複数の `FillShape` に分割・再構築する。
pub(crate) fn resolve_deformed_shape(shape: &FillShape) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let rings = flatten_shape_rings(shape, false, &mut quad_table);
    if rings.is_empty() {
        return Vec::new();
    }
    let overlay_result = run_float_overlay(
        &[rings],
        &[],
        OverlayRule::Subject,
        OverlayFillRule::Positive,
        0.5,
    );
    reconstruct_shapes_from_overlay(&overlay_result, &quad_table, shape.color, false)
        .into_iter()
        .map(|s| s.with_scope(shape.layer_id, &shape.group_path))
        .collect()
}

fn quad_subdivision_steps(seg: QuadBez) -> usize {
    let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot();
    if mid_dev <= 0.04 {
        2
    } else {
        let arc = seg.arclen(0.5);
        ((arc / 1.2).ceil() as usize).clamp(4, 256)
    }
}

fn sample_contour_ring_for_overlay(
    contour: &Contour,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    if contour.segments.is_empty() {
        return None;
    }

    let mut raw_pts = Vec::new();
    for &seg in contour.segments.iter() {
        if !seg.p0.x.is_finite()
            || !seg.p0.y.is_finite()
            || !seg.p1.x.is_finite()
            || !seg.p1.y.is_finite()
            || !seg.p2.x.is_finite()
            || !seg.p2.y.is_finite()
        {
            return None;
        }
        let min_x = seg.p0.x.min(seg.p1.x).min(seg.p2.x) - 0.6;
        let max_x = seg.p0.x.max(seg.p1.x).max(seg.p2.x) + 0.6;
        let min_y = seg.p0.y.min(seg.p1.y).min(seg.p2.y) - 0.6;
        let max_y = seg.p0.y.max(seg.p1.y).max(seg.p2.y) + 0.6;
        let bbox = KRect::new(min_x, min_y, max_x, max_y);
        quad_table.push(QuadRecord { quad: seg, bbox });

        let steps = quad_subdivision_steps(seg);
        for k in 0..steps {
            raw_pts.push(seg.eval(k as f64 / steps as f64));
        }
    }

    let deduped = dedup_closed_ring(raw_pts, 1e-4);
    if deduped.len() < 3 {
        return None;
    }

    Some(deduped.into_iter().map(|p| [p.x, p.y]).collect())
}

fn reconstruct_or_refit_quad_contour(
    ring_f64: &[[f64; 2]],
    quad_table: &[QuadRecord],
    is_outer: bool,
) -> Option<Contour> {
    let pts = dedup_closed_ring(ring_f64.iter().map(|&[x, y]| Point::new(x, y)), 1e-4);
    let n = pts.len();
    if n < 3 {
        return None;
    }

    let tagged_edges: Vec<TaggedEdge> = (0..n)
        .map(|i| {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            TaggedEdge {
                from: a,
                to: b,
                quad_match: match_edge_to_quad(a, b, quad_table),
            }
        })
        .collect();

    if tagged_edges.iter().all(|e| e.quad_match.is_none()) {
        let r_est = estimate_brush_radius_from_ring(&pts);
        return smooth_ring_to_quad_contour(&pts, r_est, is_outer);
    }

    let shift = (0..n)
        .find(|&i| {
            tagged_edges[i].quad_match.map(|(id, _)| id)
                != tagged_edges[(i + n - 1) % n].quad_match.map(|(id, _)| id)
        })
        .unwrap_or(0);

    let rotated: Vec<TaggedEdge> = (0..n).map(|i| tagged_edges[(i + shift) % n]).collect();
    let mut segments: Vec<QuadBez> = Vec::new();
    let mut i = 0;

    while i < n {
        if let Some((qid, mut prev_t)) = rotated[i].quad_match {
            let orig_quad = quad_table[qid].quad;
            let run_start_pt = rotated[i].from;
            let mut run_end_pt = rotated[i].to;
            let mut run_len = 1usize;

            while i + run_len < n {
                let Some((next_qid, next_t)) = rotated[i + run_len].quad_match else {
                    break;
                };
                if next_qid != qid || (next_t - prev_t).abs() > 0.65 {
                    break;
                }
                run_end_pt = rotated[i + run_len].to;
                prev_t = next_t;
                run_len += 1;
            }

            if shift == 0 && i == 0 && run_len == n {
                let mid_pt = rotated[n / 2].from;
                segments.push(extract_sub_quad(orig_quad, run_start_pt, mid_pt));
                segments.push(extract_sub_quad(orig_quad, mid_pt, run_start_pt));
                break;
            }

            segments.push(extract_sub_quad(orig_quad, run_start_pt, run_end_pt));
            i += run_len;
        } else {
            let mut chain_pts = vec![rotated[i].from, rotated[i].to];
            let mut run_len = 1usize;
            while i + run_len < n && rotated[i + run_len].quad_match.is_none() {
                chain_pts.push(rotated[i + run_len].to);
                run_len += 1;
            }
            segments.extend(refit_open_polyline_to_quads(&chain_pts, 0.35));
            i += run_len;
        }
    }

    if segments.len() == 1 {
        let single = segments[0];
        segments = vec![single.subsegment(0.0..0.5), single.subsegment(0.5..1.0)];
    }

    if segments.len() < 2 {
        let r_est = estimate_brush_radius_from_ring(&pts);
        return smooth_ring_to_quad_contour(&pts, r_est, is_outer);
    }

    Contour::from_segments_oriented(segments, 0.02, 0.04, is_outer)
}

fn match_edge_to_quad(a: Point, b: Point, quad_table: &[QuadRecord]) -> Option<(usize, f64)> {
    let chord = b - a;
    let chord_len = chord.hypot();
    if chord_len < 1e-5 {
        return None;
    }
    let chord_dir = chord / chord_len;
    let mid = a.lerp(b, 0.5);

    const DIST_TOL_SQ: f64 = 0.45 * 0.45;
    let mut best_match = None;
    let mut best_score = f64::INFINITY;

    for (qid, rec) in quad_table.iter().enumerate() {
        if !rec.bbox.contains(mid) {
            continue;
        }
        let n_mid = rec.quad.nearest(mid, 1e-3);
        if n_mid.distance_sq > DIST_TOL_SQ {
            continue;
        }
        let n_a = rec.quad.nearest(a, 1e-3);
        if n_a.distance_sq > DIST_TOL_SQ {
            continue;
        }
        let n_b = rec.quad.nearest(b, 1e-3);
        if n_b.distance_sq > DIST_TOL_SQ {
            continue;
        }

        let deriv = rec.quad.deriv().eval(n_mid.t).to_vec2();
        let deriv_len = deriv.hypot();
        if deriv_len > 1e-6 && chord_dir.dot(deriv / deriv_len).abs() < 0.90 {
            continue;
        }

        let score = n_mid.distance_sq + 0.5 * (n_a.distance_sq + n_b.distance_sq);
        if score < best_score {
            best_score = score;
            best_match = Some((qid, n_b.t));
        }
    }

    best_match
}

fn refit_open_polyline_to_quads(pts: &[Point], epsilon: f64) -> Vec<QuadBez> {
    let n = pts.len();
    if n < 2 {
        return Vec::new();
    }
    if n == 2 {
        return vec![QuadBez::new(pts[0], pts[0].lerp(pts[1], 0.5), pts[1])];
    }

    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;

    for i in 1..(n - 1) {
        let d_in = normalize_vec(pts[i] - pts[i - 1]);
        let d_out = normalize_vec(pts[i + 1] - pts[i]);
        if d_in.dot(d_out) < 0.57 {
            keep[i] = true;
        }
    }

    let mut seg_start = 0;
    for i in 1..n {
        if keep[i] {
            rdp_recursive(pts, seg_start, i, epsilon, &mut keep);
            seg_start = i;
        }
    }

    let anchors: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
    let mut cum = Vec::new();
    anchors
        .windows(2)
        .map(|w| {
            let span = &pts[w[0]..=w[1]];
            let p0 = pts[w[0]];
            let p2 = pts[w[1]];
            fill_cumulative_arc_lengths(span, &mut cum);
            let p1 = fit_quad_control_point_with_arc_lengths(span, &cum, p0, p2);
            QuadBez::new(p0, p1, p2)
        })
        .collect()
}

fn extract_sub_quad(orig: QuadBez, p_start: Point, p_end: Point) -> QuadBez {
    let t0 = orig.nearest(p_start, 1e-3).t.clamp(0.0, 1.0);
    let t1 = orig.nearest(p_end, 1e-3).t.clamp(0.0, 1.0);

    if (t1 - t0).abs() < 1e-4 {
        return QuadBez::new(p_start, p_start.lerp(p_end, 0.5), p_end);
    }

    let mut sub = orig.subsegment(t0..t1);
    sub.p0 = p_start;
    sub.p2 = p_end;
    sub
}
