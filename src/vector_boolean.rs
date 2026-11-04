use eframe::egui::Color32;
use i_overlay::{
    core::{fill_rule::FillRule as OverlayFillRule, overlay_rule::OverlayRule, solver::Solver},
    float::{filter::ContourFilter, overlay::FloatOverlay},
};
use kurbo::{
    ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, Point, QuadBez, Rect as KRect,
};

use crate::{
    vector_engine::{
        dedup_closed_ring, fill_cumulative_arc_lengths, is_quad_finite, next_shape_id,
        normalize_vec, rects_overlap, ring_area_and_perimeter_f64, signed_area_f64,
        union_bounding_boxes, Contour, FillShape,
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
    let mut iter = incoming.into_iter().peekable();
    while let Some(first) = iter.next() {
        let lid = first.layer_id;
        let col = first.color;
        let gp = first.group_path.clone();
        let mut batch = vec![first];
        while iter
            .peek()
            .is_some_and(|next| next.layer_id == lid && next.color == col && next.group_path == gp)
        {
            if let Some(next) = iter.next() {
                batch.push(next);
            }
        }
        merge_homogeneous_batch_into_canvas(canvas, batch, subtract_diff_color);
    }
}

fn raw_scope_insert_index(
    canvas: &[FillShape],
    target_layer_id: u64,
    target_group_path: &[u64],
) -> usize {
    canvas
        .iter()
        .rposition(|s| {
            s.layer_id == target_layer_id
                && s.group_path == target_group_path
                && s.symbol_instance.is_none()
        })
        .map(|pos| pos + 1)
        .or_else(|| {
            canvas.iter().position(|s| {
                s.layer_id == target_layer_id
                    && s.is_in_editing_scope(target_group_path)
                    && !s.is_raw_at_scope(target_group_path)
            })
        })
        .or_else(|| {
            canvas
                .iter()
                .rposition(|s| {
                    s.layer_id == target_layer_id && s.is_in_editing_scope(target_group_path)
                })
                .map(|pos| pos + 1)
        })
        .or_else(|| {
            canvas
                .iter()
                .rposition(|s| s.layer_id == target_layer_id)
                .map(|pos| pos + 1)
        })
        .unwrap_or(canvas.len())
}

fn subtract_clip_in_scope(
    canvas: &mut Vec<FillShape>,
    clip_shapes: &[FillShape],
    target_layer_id: u64,
    editing_scope: &[u64],
    subtract_color: impl Fn(Color32) -> bool,
) {
    let test_bbox = union_bounding_boxes(clip_shapes).inflate(1.5, 1.5);
    let clip_sub_bboxes = collect_sub_bboxes(clip_shapes, 1.5);
    let insert_limit = raw_scope_insert_index(canvas, target_layer_id, editing_scope);
    let trailing_outside = canvas.split_off(insert_limit);

    let mut updated_canvas = Vec::with_capacity(canvas.len() + trailing_outside.len());
    let mut overlapping_by_color: Vec<(Color32, Vec<FillShape>)> = Vec::new();
    for existing in canvas.drain(..) {
        if existing.layer_id == target_layer_id
            && existing.group_path == editing_scope
            && existing.symbol_instance.is_none()
            && subtract_color(existing.color)
            && rects_overlap(existing.bounding_box(), test_bbox)
            && shape_interacts_with_clip(&existing, clip_shapes, &clip_sub_bboxes)
        {
            if let Some((_, group)) = overlapping_by_color
                .iter_mut()
                .find(|(c, _)| *c == existing.color)
            {
                group.push(existing);
            } else {
                overlapping_by_color.push((existing.color, vec![existing]));
            }
        } else {
            updated_canvas.push(existing);
        }
    }
    if !overlapping_by_color.is_empty() {
        let clip = PreparedClip::new(clip_shapes);
        for (color, group) in overlapping_by_color {
            updated_canvas.extend(
                op_with_far_holes_detached(&group, test_bbox, |g| {
                    clip.subtract_from_group(g, color)
                })
                .into_iter()
                .map(|s| s.with_scope(target_layer_id, editing_scope)),
            );
        }
    }
    updated_canvas.extend(trailing_outside);
    *canvas = updated_canvas;
}

fn merge_homogeneous_batch_into_canvas(
    canvas: &mut Vec<FillShape>,
    incoming: Vec<FillShape>,
    subtract_diff_color: bool,
) {
    if incoming.is_empty() {
        return;
    }
    let target_color = incoming[0].color;
    let layer_id = incoming[0].layer_id;
    let scope = incoming[0].group_path.clone();
    if subtract_diff_color {
        subtract_clip_in_scope(canvas, &incoming, layer_id, &scope, |c| c != target_color);
    }

    let test_bbox = union_bounding_boxes(&incoming).inflate(1.5, 1.5);
    let clip_sub_bboxes = collect_sub_bboxes(&incoming, 1.5);
    let insert_limit = raw_scope_insert_index(canvas, layer_id, &scope);
    let trailing_outside = canvas.split_off(insert_limit);

    let mut updated_canvas =
        Vec::with_capacity(canvas.len() + incoming.len() + trailing_outside.len());
    let mut overlapping_same_color = Vec::new();

    for existing in canvas.drain(..) {
        if existing.layer_id == layer_id
            && existing.group_path == scope
            && existing.symbol_instance.is_none()
            && existing.color == target_color
            && rects_overlap(existing.bounding_box(), test_bbox)
            && shape_interacts_with_clip(&existing, &incoming, &clip_sub_bboxes)
        {
            overlapping_same_color.push(existing);
        } else {
            updated_canvas.push(existing);
        }
    }

    let incoming_self_overlaps = incoming.len() > 1
        && incoming.iter().enumerate().any(|(i, a)| {
            let a_box = a.bounding_box().inflate(1.5, 1.5);
            incoming[i + 1..]
                .iter()
                .any(|b| rects_overlap(a_box, b.bounding_box()))
        });

    if overlapping_same_color.is_empty() && !incoming_self_overlaps {
        updated_canvas.extend(incoming);
    } else if overlapping_same_color.is_empty() {
        let mut unioned =
            boolean_operation(&incoming[..1], &incoming[1..], BoolOp::Union, target_color);
        let mut used_ids = Vec::with_capacity(unioned.len());
        for s in &mut unioned {
            let s_bbox = s.bounding_box().inflate(1.5, 1.5);
            if let Some(inc) = incoming.iter().find(|inc| {
                !used_ids.contains(&inc.shape_id) && rects_overlap(s_bbox, inc.bounding_box())
            }) {
                used_ids.push(inc.shape_id);
                s.shape_id = inc.shape_id;
            }
        }
        let mut seen_ids = Vec::with_capacity(unioned.len());
        for mut s in unioned {
            if seen_ids.contains(&s.shape_id) {
                s.shape_id = next_shape_id();
            } else {
                seen_ids.push(s.shape_id);
            }
            updated_canvas.push(s.with_scope(layer_id, &scope));
        }
    } else {
        updated_canvas.extend(
            op_with_far_holes_detached(&overlapping_same_color, test_bbox, |existing| {
                boolean_operation(existing, &incoming, BoolOp::Union, target_color)
            })
            .into_iter()
            .map(|s| s.with_scope(layer_id, &scope)),
        );
    }

    updated_canvas.extend(trailing_outside);
    *canvas = updated_canvas;
}

/// `shapes` の穴のうち `near` に重ならない穴（遠い穴）を外した写しで `op` を行い、
/// 遠い穴を、それを含む結果の図形へ付け直す。変わらなかった図形は元の図形に戻す。
/// 遠い穴の行き先が決まらないときは、穴を全部渡して `op` をやり直す
fn op_with_far_holes_detached(
    shapes: &[FillShape],
    near: KRect,
    op: impl Fn(&[FillShape]) -> Vec<FillShape>,
) -> Vec<FillShape> {
    let mut far: Vec<(usize, Contour, KRect)> = Vec::new();
    let trimmed: Vec<FillShape> = shapes
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut near_holes = Vec::with_capacity(s.holes.len());
            for h in &s.holes {
                let b = h.bounding_box();
                if rects_overlap(b, near) {
                    near_holes.push(h.clone());
                } else {
                    far.push((i, h.clone(), b));
                }
            }
            if near_holes.len() == s.holes.len() {
                return s.clone();
            }
            let mut t = FillShape::new(s.color, s.exterior.clone(), near_holes);
            t.shape_id = s.shape_id;
            t
        })
        .collect();
    if far.is_empty() {
        return op(shapes);
    }
    let mut out = op(&trimmed);
    let mut extra: Vec<Vec<Contour>> = vec![Vec::new(); out.len()];
    for (owner, hole, hole_box) in far {
        if let Some(k) = out
            .iter()
            .position(|p| p.shape_id == shapes[owner].shape_id)
        {
            out[k] = shapes[owner].clone();
            continue;
        }
        let candidates: Vec<usize> = (0..out.len())
            .filter(|&k| {
                !shapes.iter().any(|s| s.shape_id == out[k].shape_id)
                    && out[k]
                        .bounding_box()
                        .inflate(1e-6, 1e-6)
                        .contains_rect(hole_box)
            })
            .collect();
        let pt = hole.segments.first().map(|seg| seg.p0);
        let dest = match candidates.as_slice() {
            [k] => Some(*k),
            _ => candidates
                .iter()
                .copied()
                .find(|&k| pt.is_some_and(|pt| out[k].contains_point(pt))),
        };
        let Some(k) = dest else {
            return op(shapes);
        };
        extra[k].push(hole);
    }
    out.into_iter()
        .zip(extra)
        .map(|(piece, extra)| {
            if extra.is_empty() {
                return piece;
            }
            let mut holes = piece.holes.clone();
            holes.extend(extra);
            FillShape::new(piece.color, piece.exterior.clone(), holes)
        })
        .collect()
}

/// 消しゴムの輪郭で、指定されたアクティブレイヤー（`layer_id`）かつ同じ編集スコープ（`scope`）直下の生シェイプに対してのみ Boolean Difference を適用する
pub(crate) fn erase_from_canvas(
    canvas: &mut Vec<FillShape>,
    eraser_shapes: &[FillShape],
    layer_id: u64,
    scope: &[u64],
) {
    if !eraser_shapes.is_empty() {
        subtract_clip_in_scope(canvas, eraser_shapes, layer_id, scope, |_| true);
    }
}

#[inline]
fn seg_padded_bbox(seg: QuadBez, pad: f64) -> KRect {
    KRect::new(
        seg.p0.x.min(seg.p1.x).min(seg.p2.x) - pad,
        seg.p0.y.min(seg.p1.y).min(seg.p2.y) - pad,
        seg.p0.x.max(seg.p1.x).max(seg.p2.x) + pad,
        seg.p0.y.max(seg.p1.y).max(seg.p2.y) + pad,
    )
}

fn collect_sub_bboxes(shapes: &[FillShape], inflate: f64) -> Vec<KRect> {
    let mut out = Vec::new();
    for s in shapes {
        for c in s.contours() {
            for chunk in c.segments.chunks(8) {
                if let Some(&first) = chunk.first() {
                    let b = chunk[1..]
                        .iter()
                        .fold(seg_padded_bbox(first, 0.0), |acc, &seg| {
                            acc.union(seg_padded_bbox(seg, 0.0))
                        });
                    out.push(b.inflate(inflate, inflate));
                }
            }
        }
    }
    out
}

#[inline]
fn overlaps_any_bbox(rect: KRect, bboxes: &[KRect]) -> bool {
    bboxes.iter().any(|&b| rects_overlap(rect, b))
}

fn shape_interacts_with_clip(
    subj: &FillShape,
    clip: &[FillShape],
    clip_sub_bboxes: &[KRect],
) -> bool {
    if !overlaps_any_bbox(subj.bounding_box(), clip_sub_bboxes) {
        return false;
    }
    if subj.contours().any(|c| {
        c.segments
            .iter()
            .any(|&seg| overlaps_any_bbox(seg_padded_bbox(seg, 0.6), clip_sub_bboxes))
    }) {
        return true;
    }
    subj.exterior
        .segments
        .first()
        .is_some_and(|s| clip.iter().any(|cs| cs.contains_point(s.p0)))
        || clip.iter().any(|cs| {
            cs.exterior
                .segments
                .first()
                .is_some_and(|s| subj.contains_point(s.p0))
        })
}

struct InputRing<'a> {
    shape_idx: usize,
    ring: &'a [[f64; 2]],
    area: f64,
    perim: f64,
    contour: &'a Contour,
    used: bool,
}

impl<'a> InputRing<'a> {
    fn new(shape_idx: usize, ring: &'a [[f64; 2]], contour: &'a Contour) -> Self {
        let (area, perim) = ring_area_and_perimeter_f64(ring);
        Self {
            shape_idx,
            ring,
            area,
            perim,
            contour,
            used: false,
        }
    }
}

fn ring_matches_input(out_r: &[[f64; 2]], in_r: &[[f64; 2]]) -> bool {
    let eq_pt =
        |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= 1e-3 && (a[1] - b[1]).abs() <= 1e-3;
    let is_subseq = |s: usize, rev: bool| {
        let (n, mut j) = (in_r.len(), 0);
        out_r.iter().all(|&op| {
            while j < n && !eq_pt(op, in_r[if rev { (s + n - j) % n } else { (s + j) % n }]) {
                j += 1;
            }
            (j < n).then(|| j += 1).is_some()
        })
    };
    out_r.len() >= 3
        && out_r.len() <= in_r.len()
        && in_r
            .iter()
            .position(|&p| eq_pt(p, out_r[0]))
            .is_some_and(|s| is_subseq(s, false) || is_subseq(s, true))
}

fn take_matching_input_ring<'a>(
    out_r: &[[f64; 2]],
    out_a: f64,
    out_p: f64,
    pool: &mut [InputRing<'a>],
) -> Option<(usize, &'a Contour)> {
    let item = pool.iter_mut().find(|ir| {
        !ir.used
            && (out_a - ir.area).abs() <= 1e-2
            && (out_p - ir.perim).abs() <= 1e-2
            && ring_matches_input(out_r, ir.ring)
    })?;
    item.used = true;
    Some((item.shape_idx, item.contour))
}

/// 複数の対象シェイプに対して繰り返し Difference（減算）を行う際に、
/// クリップ側シェイプ群のフラットニング結果と `QuadRecord` を1回だけ事前構築して使い回すキャッシュ構造体
struct PreparedClip<'a> {
    clip: &'a [FillShape],
    clip_shapes: Vec<OverlayShape>,
    clip_quads: Vec<QuadRecord>,
}

impl<'a> PreparedClip<'a> {
    fn new(clip: &'a [FillShape]) -> Self {
        let mut clip_quads = Vec::new();
        let clip_shapes = clip
            .iter()
            .map(|s| flatten_shape_for_overlay(s, &mut clip_quads))
            .filter(|r| !r.is_empty())
            .collect();
        Self {
            clip,
            clip_shapes,
            clip_quads,
        }
    }

    fn subtract_from_group(&self, subjs: &[FillShape], color: Color32) -> Vec<FillShape> {
        let mut active_subjs = Vec::new();
        let mut subj_polys = Vec::new();
        let mut quad_table = Vec::with_capacity(16 * subjs.len() + self.clip_quads.len());

        for subj in subjs {
            let poly = flatten_shape_for_overlay(subj, &mut quad_table);
            if !poly.is_empty() {
                subj_polys.push(poly);
                active_subjs.push(subj.clone());
            }
        }
        if subj_polys.is_empty() {
            return Vec::new();
        }
        quad_table.extend_from_slice(&self.clip_quads);
        let min_area = adaptive_overlay_min_area(&active_subjs, self.clip);
        let overlay_result = run_float_overlay(
            &subj_polys,
            &self.clip_shapes,
            OverlayRule::Difference,
            OverlayFillRule::NonZero,
            min_area,
        );
        reconstruct_shapes_from_overlay(
            &overlay_result,
            &[(&active_subjs, &subj_polys)],
            &quad_table,
            color,
            false,
        )
    }
}

fn adaptive_overlay_min_area(shapes_a: &[FillShape], shapes_b: &[FillShape]) -> f64 {
    let min_dim = shapes_a
        .iter()
        .chain(shapes_b.iter())
        .map(|s| {
            let b = s.bounding_box();
            b.width().min(b.height())
        })
        .filter(|&d| d > 1e-6)
        .fold(f64::INFINITY, f64::min);
    if min_dim.is_finite() && min_dim < 2.0 {
        0.04_f64.min((min_dim * min_dim * 0.01).max(1e-5))
    } else {
        0.04
    }
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
        .collect();
    let clip_shapes: Vec<OverlayShape> = clip
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .collect();
    let subj_empty = subj_shapes.iter().all(Vec::is_empty);
    let clip_empty = clip_shapes.iter().all(Vec::is_empty);

    if (subj_empty && op == BoolOp::Difference)
        || (op == BoolOp::Intersection && (subj_empty || clip_empty))
        || (subj_empty && clip_empty)
    {
        return Vec::new();
    }

    let overlay_rule = match op {
        BoolOp::Union => OverlayRule::Union,
        BoolOp::Difference => OverlayRule::Difference,
        BoolOp::Intersection => OverlayRule::Intersect,
    };

    let min_area = adaptive_overlay_min_area(subj, clip);
    let overlay_result = run_float_overlay(
        &subj_shapes,
        &clip_shapes,
        overlay_rule,
        OverlayFillRule::NonZero,
        min_area,
    );
    let in_groups: &[(&[FillShape], &[OverlayShape])] = if op == BoolOp::Union {
        &[(subj, &subj_shapes), (clip, &clip_shapes)]
    } else {
        &[(subj, &subj_shapes)]
    };
    reconstruct_shapes_from_overlay(
        &overlay_result,
        in_groups,
        &quad_table,
        out_color,
        op == BoolOp::Union,
    )
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
    let min_area = adaptive_overlay_min_area(
        std::slice::from_ref(shape),
        std::slice::from_ref(clip_rect_shape),
    );
    let subj_slice = std::slice::from_ref(&subj);
    let clip_slice = std::slice::from_ref(&clip);
    let in_groups = &[(std::slice::from_ref(shape), subj_slice)];
    let inside_rings = run_float_overlay(
        subj_slice,
        clip_slice,
        OverlayRule::Intersect,
        OverlayFillRule::NonZero,
        min_area,
    );
    let inside =
        reconstruct_shapes_from_overlay(&inside_rings, in_groups, &quad_table, shape.color, false);
    if inside.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let outside_rings = run_float_overlay(
        subj_slice,
        clip_slice,
        OverlayRule::Difference,
        OverlayFillRule::NonZero,
        min_area,
    );
    let outside =
        reconstruct_shapes_from_overlay(&outside_rings, in_groups, &quad_table, shape.color, false);
    (inside, outside)
}

fn reconstruct_shapes_from_overlay(
    overlay_result: &[OverlayShape],
    in_groups: &[(&[FillShape], &[OverlayShape])],
    quad_table: &[QuadRecord],
    color: Color32,
    filter_sliver_holes: bool,
) -> Vec<FillShape> {
    let mut flat_shapes: Vec<&FillShape> = Vec::new();
    let mut ext_pool: Vec<InputRing<'_>> = Vec::new();
    let mut hole_pool: Vec<InputRing<'_>> = Vec::new();
    for &(shapes, polys) in in_groups {
        for (s, poly) in shapes.iter().zip(polys.iter()) {
            if poly.len() != 1 + s.holes.len() {
                continue;
            }
            let shape_idx = flat_shapes.len();
            flat_shapes.push(s);
            ext_pool.push(InputRing::new(shape_idx, &poly[0], &s.exterior));
            for (h_ring, h_contour) in poly[1..].iter().zip(s.holes.iter()) {
                hole_pool.push(InputRing::new(shape_idx, h_ring, h_contour));
            }
        }
    }

    let mut grid: Option<QuadSpatialGrid> = None;
    let mut resolve_ring = |ring: &[[f64; 2]],
                            (a, p): (f64, f64),
                            is_outer: bool,
                            pool: &mut [InputRing<'_>]|
     -> Option<(Option<usize>, Contour)> {
        if let Some((s_idx, c)) = take_matching_input_ring(ring, a, p, pool) {
            return Some((Some(s_idx), c.clone()));
        }
        let g = grid.get_or_insert_with(|| QuadSpatialGrid::new(quad_table));
        Some((
            None,
            reconstruct_or_refit_quad_contour(ring, quad_table, g, is_outer)?,
        ))
    };

    let mut out = Vec::with_capacity(overlay_result.len());
    for poly_shape in overlay_result {
        if poly_shape.is_empty() {
            continue;
        }
        let (ext_a, ext_p) = ring_area_and_perimeter_f64(&poly_shape[0]);
        let Some((ext_match, exterior)) =
            resolve_ring(&poly_shape[0], (ext_a, ext_p), true, &mut ext_pool)
        else {
            continue;
        };
        let hole_scale = if ext_p < 16.0 {
            (ext_p / 16.0).clamp(0.005, 1.0)
        } else {
            1.0
        };
        let min_hole_area = 1.5 * hole_scale * hole_scale;
        let min_hole_thickness = 0.15 * hole_scale;
        let mut holes = Vec::with_capacity(poly_shape.len() - 1);
        let mut all_holes_match_ext = ext_match.is_some();
        for hole_ring in &poly_shape[1..] {
            let (a, p) = ring_area_and_perimeter_f64(hole_ring);
            if filter_sliver_holes {
                let circularity = (4.0 * std::f64::consts::PI * a) / (p * p).max(1e-9);
                let keep = if circularity >= 0.18 {
                    a >= 1e-4
                } else {
                    a >= min_hole_area && (a / p.max(1e-5)) >= min_hole_thickness
                };
                if !keep {
                    continue;
                }
            }
            if let Some((h_match, c)) = resolve_ring(hole_ring, (a, p), false, &mut hole_pool) {
                all_holes_match_ext &= h_match.is_some() && h_match == ext_match;
                holes.push(c);
            } else {
                all_holes_match_ext = false;
            }
        }
        if let Some(s_idx) = ext_match.filter(|&i| {
            all_holes_match_ext
                && holes.len() == flat_shapes[i].holes.len()
                && flat_shapes[i].color == color
                && flat_shapes[i].symbol_instance.is_none()
        }) {
            out.push(flat_shapes[s_idx].clone());
        } else {
            out.push(FillShape::new(color, exterior, holes));
        }
    }
    out
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

/// 指定座標 `pt` を囲み得る連結AABBクラスタをスイープライン＋Union-Find（素集合データ構造）で計算し、
/// 無関係な遠方のシェイプを事前にAABBカリングする
fn cull_enclosed_region_candidates(
    canvas: &[FillShape],
    pt: Point,
) -> Option<(KRect, Vec<FillShape>)> {
    let mut comp_rects: Vec<KRect> = canvas
        .iter()
        .filter(|s| !s.exterior.segments.is_empty())
        .map(|s| s.bounding_box().inflate(1.0, 1.0))
        .collect();
    let n = comp_rects.len();
    if n == 0 {
        return None;
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_unstable_by(|&a, &b| comp_rects[a].x0.total_cmp(&comp_rects[b].x0));

    let mut parent: Vec<usize> = (0..n).collect();
    let find_root = |parent: &mut [usize], mut x: usize| -> usize {
        while parent[x] != x {
            let p = parent[x];
            parent[x] = parent[p];
            x = p;
        }
        x
    };

    for a_pos in 0..n {
        let i = order[a_pos];
        let rect_i = comp_rects[i];
        for &j in &order[(a_pos + 1)..] {
            let rect_j = comp_rects[j];
            if rect_j.x0 > rect_i.x1 {
                break;
            }
            if rect_i.y0 <= rect_j.y1 && rect_i.y1 >= rect_j.y0 {
                let ri = find_root(&mut parent, i);
                let rj = find_root(&mut parent, j);
                if ri != rj {
                    parent[rj] = ri;
                }
            }
        }
    }

    for i in 0..n {
        let root = find_root(&mut parent, i);
        if root != i {
            let r = comp_rects[i];
            comp_rects[root] = comp_rects[root].union(r);
        }
    }

    let mut active_bounds: Option<KRect> = None;
    for i in 0..n {
        if parent[i] == i && comp_rects[i].contains(pt) {
            active_bounds = Some(match active_bounds {
                Some(acc) => acc.union(comp_rects[i]),
                None => comp_rects[i],
            });
        }
    }
    let bounds = active_bounds?;
    let candidates: Vec<FillShape> = canvas
        .iter()
        .filter(|s| !s.exterior.segments.is_empty() && rects_overlap(s.bounding_box(), bounds))
        .cloned()
        .collect();
    if candidates.is_empty() {
        None
    } else {
        Some((bounds, candidates))
    }
}

/// キャンバス上のシェイプ群によって完全に囲まれた空白領域（単一シェイプの穴、または複数シェイプで囲まれた閉領域）のうち、
/// 指定座標 `pt` を含むものを検出し、周囲の2次ベジェ曲線をそのまま引き継いだ `FillShape` を生成する。
pub(crate) fn find_enclosed_region_at(
    canvas: &[FillShape],
    pt: Point,
    fill_color: Color32,
) -> Option<FillShape> {
    let (active_bbox, candidates) = cull_enclosed_region_candidates(canvas, pt)?;

    let bounds = active_bbox.inflate(32.0, 32.0);
    let mut bounds_ring = vec![
        [bounds.x0, bounds.y0],
        [bounds.x1, bounds.y0],
        [bounds.x1, bounds.y1],
        [bounds.x0, bounds.y1],
    ];
    if signed_area_f64(bounds_ring.iter().copied()) > 0.0 {
        bounds_ring.reverse();
    }

    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let clip_shapes: Vec<OverlayShape> = candidates
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();
    if clip_shapes.is_empty() {
        return None;
    }

    let min_area = adaptive_overlay_min_area(&candidates, &[]);
    let overlay_result = run_float_overlay(
        &[vec![bounds_ring]],
        &clip_shapes,
        OverlayRule::Difference,
        OverlayFillRule::NonZero,
        min_area,
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

        let Some(candidate) = reconstruct_shapes_from_overlay(
            std::slice::from_ref(&poly_shape),
            &[],
            &quad_table,
            fill_color,
            false,
        )
        .into_iter()
        .next() else {
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
            if ((yi > pt.y) != (yj > pt.y)) && (pt.x < (xj - xi) * (pt.y - yi) / (yj - yi) + xi) {
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

/// `quad_table` 内の2次ベジェ曲線セグメント群を 2D 均一グリッドに空間インデックス化し、
/// ブーリアン演算後のポリゴン辺と元の `QuadBez` の照合を O(1) セル検索に高速化する構造体
struct QuadSpatialGrid {
    min_x: f64,
    min_y: f64,
    inv_cell_w: f64,
    inv_cell_h: f64,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<usize>>,
}

impl QuadSpatialGrid {
    fn new(quad_table: &[QuadRecord]) -> Self {
        if quad_table.is_empty() {
            return Self {
                min_x: 0.0,
                min_y: 0.0,
                inv_cell_w: 1.0,
                inv_cell_h: 1.0,
                cols: 1,
                rows: 1,
                cells: vec![Vec::new()],
            };
        }
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for rec in quad_table {
            min_x = min_x.min(rec.bbox.x0);
            min_y = min_y.min(rec.bbox.y0);
            max_x = max_x.max(rec.bbox.x1);
            max_y = max_y.max(rec.bbox.y1);
        }
        let side = (quad_table.len() as f64).sqrt().ceil() as usize;
        let cols = side.clamp(1, 32);
        let rows = side.clamp(1, 32);
        let span_x = (max_x - min_x).max(1.0);
        let span_y = (max_y - min_y).max(1.0);
        let inv_cell_w = (cols as f64) / span_x;
        let inv_cell_h = (rows as f64) / span_y;
        let mut cells = vec![Vec::new(); cols * rows];

        for (qid, rec) in quad_table.iter().enumerate() {
            let cx0 = (((rec.bbox.x0 - min_x) * inv_cell_w).floor() as isize)
                .clamp(0, cols as isize - 1) as usize;
            let cx1 = (((rec.bbox.x1 - min_x) * inv_cell_w).floor() as isize)
                .clamp(0, cols as isize - 1) as usize;
            let cy0 = (((rec.bbox.y0 - min_y) * inv_cell_h).floor() as isize)
                .clamp(0, rows as isize - 1) as usize;
            let cy1 = (((rec.bbox.y1 - min_y) * inv_cell_h).floor() as isize)
                .clamp(0, rows as isize - 1) as usize;
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    cells[cy * cols + cx].push(qid);
                }
            }
        }

        Self {
            min_x,
            min_y,
            inv_cell_w,
            inv_cell_h,
            cols,
            rows,
            cells,
        }
    }

    #[inline]
    fn candidates_at(&self, pt: Point) -> &[usize] {
        let cx = ((pt.x - self.min_x) * self.inv_cell_w).floor() as isize;
        let cy = ((pt.y - self.min_y) * self.inv_cell_h).floor() as isize;
        if cx < 0 || cx >= self.cols as isize || cy < 0 || cy >= self.rows as isize {
            return &[];
        }
        &self.cells[(cy as usize) * self.cols + (cx as usize)]
    }
}

#[derive(Clone, Copy, Debug)]
struct TaggedEdge {
    from: Point,
    to: Point,
    quad_match: Option<(usize, f64)>,
}

fn flatten_contour_for_overlay(
    contour: &Contour,
    is_outer: bool,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table)?;
    let area = signed_area_f64(ring.iter().copied());
    if area.abs() < 1e-6 {
        return None;
    }
    if (is_outer && area > 0.0) || (!is_outer && area < 0.0) {
        ring.reverse();
    }
    Some(ring)
}

fn flatten_contour_preserving_direction(
    contour: &Contour,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table)?;
    ring.reverse();
    Some(ring)
}

fn flatten_shape_for_overlay(shape: &FillShape, quad_table: &mut Vec<QuadRecord>) -> OverlayShape {
    let Some(ext_ring) = flatten_contour_for_overlay(&shape.exterior, true, quad_table) else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) = flatten_contour_for_overlay(hole, false, quad_table) {
            rings.push(hole_ring);
        }
    }
    rings
}

/// 輪郭変形（エッジつまみ変形・頂点移動）後のシェイプの自己交差・反対側輪郭の突き抜けを解消し、
/// 正のワインディング領域（`OverlayFillRule::Positive`）のみを残して1つまたは複数の `FillShape` に分割・再構築する。
pub(crate) fn resolve_deformed_shape(shape: &FillShape) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let Some(ext_ring) = flatten_contour_preserving_direction(&shape.exterior, &mut quad_table)
    else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) = flatten_contour_preserving_direction(hole, &mut quad_table) {
            rings.push(hole_ring);
        }
    }
    let min_area = adaptive_overlay_min_area(std::slice::from_ref(shape), &[]);
    let subj = [rings];
    let overlay_result = run_float_overlay(
        &subj,
        &[],
        OverlayRule::Subject,
        OverlayFillRule::Positive,
        min_area,
    );
    let in_groups = [(std::slice::from_ref(shape), &subj[..])];
    reconstruct_shapes_from_overlay(&overlay_result, &in_groups, &quad_table, shape.color, false)
        .into_iter()
        .map(|s| s.with_scope(shape.layer_id, &shape.group_path))
        .collect()
}

fn quad_subdivision_steps(seg: QuadBez) -> usize {
    let chord_len = (seg.p2 - seg.p0).hypot();
    let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot();
    let flat_tol = 0.04_f64.min((chord_len * 0.015).max(1e-4));
    if mid_dev <= flat_tol {
        2
    } else {
        let arc = seg.arclen(0.5);
        let step_len = 1.2_f64.min((arc * 0.125).max(0.02));
        ((arc / step_len).ceil() as usize).clamp(4, 256)
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
        if !is_quad_finite(&seg) {
            return None;
        }
        quad_table.push(QuadRecord {
            quad: seg,
            bbox: seg_padded_bbox(seg, 0.6),
        });

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
    grid: &QuadSpatialGrid,
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
                quad_match: match_edge_to_quad(a, b, quad_table, grid),
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

    let (_, ring_perim) = ring_area_and_perimeter_f64(ring_f64);
    let scale = if ring_perim < 4.0 {
        (ring_perim / 4.0).clamp(0.005, 1.0)
    } else {
        1.0
    };
    let refit_eps = 0.35 * scale;
    let min_seg_len = 0.02 * scale;
    let min_area = 0.04 * scale * scale;

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
            segments.extend(refit_open_polyline_to_quads(&chain_pts, refit_eps));
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

    Contour::from_segments_oriented(segments, min_seg_len, min_area, is_outer)
}

fn match_edge_to_quad(
    a: Point,
    b: Point,
    quad_table: &[QuadRecord],
    grid: &QuadSpatialGrid,
) -> Option<(usize, f64)> {
    let chord = b - a;
    let chord_len = chord.hypot();
    if chord_len < 1e-5 {
        return None;
    }
    let chord_dir = chord / chord_len;
    let mid = a.lerp(b, 0.5);

    let dist_tol = 0.45_f64.min((chord_len * 0.35).max(1e-4));
    let dist_tol_sq = dist_tol * dist_tol;
    let nearest_acc = (dist_tol_sq * 0.01).clamp(1e-7, 1e-3);
    let mut best_match = None;
    let mut best_score = f64::INFINITY;

    for &qid in grid.candidates_at(mid) {
        let rec = &quad_table[qid];
        if !rec.bbox.contains(mid) {
            continue;
        }
        let n_mid = rec.quad.nearest(mid, nearest_acc);
        if n_mid.distance_sq > dist_tol_sq {
            continue;
        }
        let n_a = rec.quad.nearest(a, nearest_acc);
        if n_a.distance_sq > dist_tol_sq {
            continue;
        }
        let n_b = rec.quad.nearest(b, nearest_acc);
        if n_b.distance_sq > dist_tol_sq {
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
    let t0 = orig.nearest(p_start, 1e-6).t.clamp(0.0, 1.0);
    let t1 = orig.nearest(p_end, 1e-6).t.clamp(0.0, 1.0);

    if (t1 - t0).abs() < 1e-4 {
        return QuadBez::new(p_start, p_start.lerp(p_end, 0.5), p_end);
    }

    let mut sub = orig.subsegment(t0..t1);
    sub.p0 = p_start;
    sub.p2 = p_end;
    sub
}
