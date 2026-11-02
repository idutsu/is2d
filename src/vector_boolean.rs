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
        dedup_closed_ring, fill_cumulative_arc_lengths, normalize_vec, rects_overlap,
        ring_area_and_perimeter_f64, signed_area_f64, union_bounding_boxes, Contour, FillShape,
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

/// 互いに重ならず同色で接する片もない `Floating::pieces`（`pieces_disjoint == true`）をキャンバスへマージする
pub(crate) fn merge_disjoint_pieces_into_canvas(
    canvas: &mut Vec<FillShape>,
    pieces: Vec<FillShape>,
) {
    if pieces.is_empty() {
        return;
    }
    let base_raw_bboxes: Vec<(u64, Vec<u64>, KRect)> = canvas
        .iter()
        .filter(|s| s.symbol_instance.is_none())
        .map(|s| (s.layer_id, s.group_path.clone(), s.bounding_box()))
        .collect();

    let mut non_overlapping_groups: Vec<(u64, Vec<u64>, Vec<FillShape>)> = Vec::new();
    let mut overlapping_batches: Vec<(u64, Vec<u64>, Color32, Vec<FillShape>)> = Vec::new();

    let piece_meta: Vec<(u64, Vec<u64>, Color32, KRect)> = pieces
        .iter()
        .map(|p| (p.layer_id, p.group_path.clone(), p.color, p.bounding_box()))
        .collect();

    for (i, piece) in pieces.into_iter().enumerate() {
        let p_bbox = piece_meta[i].3.inflate(1.5, 1.5);
        let overlaps_base = base_raw_bboxes.iter().any(|(lid, gp, b_bbox)| {
            *lid == piece.layer_id && *gp == piece.group_path && rects_overlap(*b_bbox, p_bbox)
        }) || piece_meta.iter().enumerate().any(|(j, (lid, gp, col, b_bbox))| {
            i != j
                && *lid == piece.layer_id
                && *gp == piece.group_path
                && *col == piece.color
                && rects_overlap(*b_bbox, p_bbox)
        });
        if overlaps_base {
            if let Some((_, _, _, batch)) = overlapping_batches.iter_mut().find(|(lid, gp, c, _)| {
                *lid == piece.layer_id && *gp == piece.group_path && *c == piece.color
            }) {
                batch.push(piece);
            } else {
                overlapping_batches.push((
                    piece.layer_id,
                    piece.group_path.clone(),
                    piece.color,
                    vec![piece],
                ));
            }
        } else if let Some((_, _, group)) = non_overlapping_groups
            .iter_mut()
            .find(|(lid, gp, _)| *lid == piece.layer_id && *gp == piece.group_path)
        {
            group.push(piece);
        } else {
            non_overlapping_groups.push((
                piece.layer_id,
                piece.group_path.clone(),
                vec![piece],
            ));
        }
    }

    for (_, _, _, batch) in overlapping_batches {
        merge_homogeneous_batch_into_canvas(canvas, batch, true);
    }
    for (lid, gp, group) in non_overlapping_groups {
        let insert_pos = raw_scope_insert_index(canvas, lid, &gp);
        canvas.splice(insert_pos..insert_pos, group);
    }
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
    let target_layer_id = incoming[0].layer_id;
    let target_group_path = incoming[0].group_path.clone();
    let test_bbox = union_bounding_boxes(&incoming).inflate(1.5, 1.5);
    let clip_sub_bboxes = collect_sub_bboxes(&incoming, 1.5);

    let insert_limit = raw_scope_insert_index(canvas, target_layer_id, &target_group_path);
    let trailing_outside = canvas.split_off(insert_limit);

    let mut updated_canvas =
        Vec::with_capacity(canvas.len() + incoming.len() + trailing_outside.len());
    let mut overlapping_same_color = Vec::new();
    let mut overlapping_diff_by_color: Vec<(Color32, Vec<FillShape>)> = Vec::new();

    for existing in canvas.drain(..) {
        if existing.layer_id != target_layer_id
            || existing.group_path != target_group_path
            || existing.symbol_instance.is_some()
        {
            updated_canvas.push(existing);
            continue;
        }
        let interacts = rects_overlap(existing.bounding_box(), test_bbox)
            && shape_interacts_with_clip(&existing, &incoming, &clip_sub_bboxes);
        if existing.color == target_color {
            if interacts {
                overlapping_same_color.push(existing);
            } else {
                updated_canvas.push(existing);
            }
        } else if subtract_diff_color && interacts {
            if let Some((_, group)) = overlapping_diff_by_color
                .iter_mut()
                .find(|(c, _)| *c == existing.color)
            {
                group.push(existing);
            } else {
                overlapping_diff_by_color.push((existing.color, vec![existing]));
            }
        } else {
            updated_canvas.push(existing);
        }
    }

    if !overlapping_diff_by_color.is_empty() {
        let clip = PreparedClip::new(&incoming, &clip_sub_bboxes);
        for (diff_color, diff_group) in overlapping_diff_by_color {
            updated_canvas.extend(
                clip.subtract_from_group(diff_group, diff_color)
                    .into_iter()
                    .map(|s| s.with_scope(target_layer_id, &target_group_path)),
            );
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
        let unioned =
            boolean_operation(&incoming[..1], &incoming[1..], BoolOp::Union, target_color);
        let mut used_ids = Vec::with_capacity(unioned.len());
        updated_canvas.extend(unioned.into_iter().map(|mut s| {
            let s_bbox = s.bounding_box().inflate(1.5, 1.5);
            if let Some(inc) = incoming.iter().find(|inc| {
                !used_ids.contains(&inc.shape_id) && rects_overlap(s_bbox, inc.bounding_box())
            }) {
                used_ids.push(inc.shape_id);
                s.shape_id = inc.shape_id;
            }
            s.with_scope(target_layer_id, &target_group_path)
        }));
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

/// 消しゴムの輪郭で、指定されたアクティブレイヤー（`target_layer_id`）かつ同じ編集スコープ（`editing_scope`）直下の生シェイプに対してのみ Boolean Difference を適用する
pub(crate) fn erase_from_canvas(
    canvas: &mut Vec<FillShape>,
    eraser_shapes: &[FillShape],
    target_layer_id: u64,
    editing_scope: &[u64],
) {
    if eraser_shapes.is_empty() {
        return;
    }
    let test_bbox = union_bounding_boxes(eraser_shapes).inflate(1.5, 1.5);
    let clip_sub_bboxes = collect_sub_bboxes(eraser_shapes, 1.5);
    let insert_limit = raw_scope_insert_index(canvas, target_layer_id, editing_scope);
    let trailing_outside = canvas.split_off(insert_limit);

    let mut updated_canvas = Vec::with_capacity(canvas.len() + trailing_outside.len());
    let mut overlapping_by_color: Vec<(Color32, Vec<FillShape>)> = Vec::new();
    for existing in canvas.drain(..) {
        if existing.layer_id == target_layer_id
            && existing.group_path == editing_scope
            && existing.symbol_instance.is_none()
            && rects_overlap(existing.bounding_box(), test_bbox)
            && shape_interacts_with_clip(&existing, eraser_shapes, &clip_sub_bboxes)
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
        let clip = PreparedClip::new(eraser_shapes, &clip_sub_bboxes);
        for (color, group) in overlapping_by_color {
            updated_canvas.extend(
                clip.subtract_from_group(group, color)
                    .into_iter()
                    .map(|s| s.with_scope(target_layer_id, editing_scope)),
            );
        }
    }
    updated_canvas.extend(trailing_outside);
    *canvas = updated_canvas;
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

fn poly_shape_matches_input(out_poly: &OverlayShape, in_poly: &OverlayShape) -> bool {
    out_poly.len() == in_poly.len()
        && out_poly.iter().zip(in_poly.iter()).all(|(out_r, in_r)| {
            let n = in_r.len();
            if out_r.len() < 3 || out_r.len() > n {
                return false;
            }
            let (out_a, out_p) = ring_area_and_perimeter_f64(out_r);
            let (in_a, in_p) = ring_area_and_perimeter_f64(in_r);
            if (out_a - in_a).abs() > 1e-2 || (out_p - in_p).abs() > 1e-2 {
                return false;
            }
            let eq_pt = |a: [f64; 2], b: [f64; 2]| {
                (a[0] - b[0]).abs() <= 1e-3 && (a[1] - b[1]).abs() <= 1e-3
            };
            let Some(s) = in_r.iter().position(|&p| eq_pt(p, out_r[0])) else {
                return false;
            };
            let mut j = 0usize;
            let fwd = out_r.iter().all(|&op| {
                while j < n && !eq_pt(op, in_r[(s + j) % n]) {
                    j += 1;
                }
                if j < n {
                    j += 1;
                    true
                } else {
                    false
                }
            });
            if fwd {
                return true;
            }
            let mut j = 0usize;
            out_r.iter().all(|&op| {
                while j < n && !eq_pt(op, in_r[(s + n - j) % n]) {
                    j += 1;
                }
                if j < n {
                    j += 1;
                    true
                } else {
                    false
                }
            })
        })
}

/// 複数の対象シェイプに対して繰り返し Difference（減算）を行う際に、
/// クリップ側シェイプ群のフラットニング結果と `QuadRecord` を1回だけ事前構築して使い回すキャッシュ構造体
struct PreparedClip<'a> {
    clip: &'a [FillShape],
    clip_sub_bboxes: &'a [KRect],
}

impl<'a> PreparedClip<'a> {
    fn new(clip: &'a [FillShape], clip_sub_bboxes: &'a [KRect]) -> Self {
        Self {
            clip,
            clip_sub_bboxes,
        }
    }

    fn subtract_from_group(&self, subjs: Vec<FillShape>, color: Color32) -> Vec<FillShape> {
        let mut untouched = Vec::new();
        let mut active_subjs = Vec::new();
        let mut subj_polys = Vec::new();
        let mut quad_table = Vec::new();
        let mut direct_map = std::collections::HashMap::new();

        for subj in subjs {
            let poly = flatten_shape_for_overlay(
                &subj,
                &mut quad_table,
                &mut direct_map,
                Some(self.clip_sub_bboxes),
            );
            if !poly.is_empty() {
                subj_polys.push(poly);
                active_subjs.push(subj);
            }
        }
        if subj_polys.is_empty() {
            return untouched;
        }
        let subj_sub_bboxes = collect_sub_bboxes(&active_subjs, 1.5);
        let clip_filter = if self.clip.len() <= 1 {
            Some(subj_sub_bboxes.as_slice())
        } else {
            None
        };
        let clip_shapes: Vec<OverlayShape> = self
            .clip
            .iter()
            .map(|s| flatten_shape_for_overlay(s, &mut quad_table, &mut direct_map, clip_filter))
            .filter(|r| !r.is_empty())
            .collect();
        let min_area = adaptive_overlay_min_area(&active_subjs, self.clip);
        let overlay_result = run_float_overlay(
            &subj_polys,
            &clip_shapes,
            OverlayRule::Difference,
            OverlayFillRule::NonZero,
            min_area,
        );
        let mut to_reconstruct = Vec::new();
        let mut used_input = vec![false; active_subjs.len()];
        for poly_shape in overlay_result {
            if let Some(idx) = (0..active_subjs.len()).find(|&i| {
                !used_input[i] && poly_shape_matches_input(&poly_shape, &subj_polys[i])
            }) {
                used_input[idx] = true;
                untouched.push(active_subjs[idx].clone());
            } else {
                to_reconstruct.push(poly_shape);
            }
        }
        if !to_reconstruct.is_empty() {
            untouched.extend(reconstruct_shapes_from_overlay(
                &to_reconstruct,
                &quad_table,
                &direct_map,
                color,
                false,
            ));
        }
        untouched
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
    let subj_sub_bboxes = collect_sub_bboxes(subj, 1.5);
    let clip_sub_bboxes = collect_sub_bboxes(clip, 1.5);
    let subj_filter = Some(clip_sub_bboxes.as_slice());
    let clip_filter = if clip.len() <= 1 {
        Some(subj_sub_bboxes.as_slice())
    } else {
        None
    };
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let mut direct_map: DirectQuadMap = std::collections::HashMap::new();
    let subj_shapes: Vec<OverlayShape> = subj
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table, &mut direct_map, subj_filter))
        .filter(|r| !r.is_empty())
        .collect();
    let clip_shapes: Vec<OverlayShape> = clip
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table, &mut direct_map, clip_filter))
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

    let min_area = adaptive_overlay_min_area(subj, clip);
    let overlay_result = run_float_overlay(
        &subj_shapes,
        &clip_shapes,
        overlay_rule,
        OverlayFillRule::NonZero,
        min_area,
    );
    reconstruct_shapes_from_overlay(
        &overlay_result,
        &quad_table,
        &direct_map,
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
    let mut direct_map: DirectQuadMap = std::collections::HashMap::new();
    let subj = flatten_shape_for_overlay(shape, &mut quad_table, &mut direct_map, None);
    let clip = flatten_shape_for_overlay(clip_rect_shape, &mut quad_table, &mut direct_map, None);
    if subj.is_empty() || clip.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let min_area = adaptive_overlay_min_area(
        std::slice::from_ref(shape),
        std::slice::from_ref(clip_rect_shape),
    );
    let subj_slice = std::slice::from_ref(&subj);
    let clip_slice = std::slice::from_ref(&clip);
    let inside_rings = run_float_overlay(
        subj_slice,
        clip_slice,
        OverlayRule::Intersect,
        OverlayFillRule::NonZero,
        min_area,
    );
    let inside = reconstruct_shapes_from_overlay(
        &inside_rings,
        &quad_table,
        &direct_map,
        shape.color,
        false,
    );
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
    let outside = reconstruct_shapes_from_overlay(
        &outside_rings,
        &quad_table,
        &direct_map,
        shape.color,
        false,
    );
    (inside, outside)
}

type DirectQuadMap = std::collections::HashMap<([i64; 2], [i64; 2]), (usize, bool)>;

#[inline]
fn quantize_pt_key(p: Point) -> [i64; 2] {
    [(p.x * 256.0).round() as i64, (p.y * 256.0).round() as i64]
}

#[inline]
fn quantize_coord_candidates(v: f64) -> ([i64; 2], usize) {
    let scaled = v * 256.0;
    let r = scaled.round();
    let r_i = r as i64;
    if (scaled - r).abs() > 0.40 {
        if scaled > r {
            ([r_i, r_i + 1], 2)
        } else {
            ([r_i - 1, r_i], 2)
        }
    } else {
        ([r_i, r_i], 1)
    }
}

fn insert_direct_quad_edge(map: &mut DirectQuadMap, a: Point, b: Point, qid: usize) {
    let (axs, n_ax) = quantize_coord_candidates(a.x);
    let (ays, n_ay) = quantize_coord_candidates(a.y);
    let (bxs, n_bx) = quantize_coord_candidates(b.x);
    let (bys, n_by) = quantize_coord_candidates(b.y);
    for &ax in &axs[..n_ax] {
        for &ay in &ays[..n_ay] {
            for &bx in &bxs[..n_bx] {
                for &by in &bys[..n_by] {
                    map.entry(([ax, ay], [bx, by])).or_insert((qid, false));
                    map.entry(([bx, by], [ax, ay])).or_insert((qid, true));
                }
            }
        }
    }
}

fn reconstruct_shapes_from_overlay(
    overlay_result: &[OverlayShape],
    quad_table: &[QuadRecord],
    direct_map: &DirectQuadMap,
    color: Color32,
    filter_sliver_holes: bool,
) -> Vec<FillShape> {
    let grid = QuadSpatialGrid::new(quad_table);
    overlay_result
        .iter()
        .filter_map(|poly_shape| {
            reconstruct_fill_shape_from_rings(
                poly_shape,
                quad_table,
                &grid,
                direct_map,
                color,
                filter_sliver_holes,
            )
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
    grid: &QuadSpatialGrid,
    direct_map: &DirectQuadMap,
    color: Color32,
    filter_sliver_holes: bool,
) -> Option<FillShape> {
    if poly_shape.is_empty() {
        return None;
    }
    let exterior =
        reconstruct_or_refit_quad_contour(&poly_shape[0], quad_table, grid, direct_map, true)?;
    let (_, ext_perim) = ring_area_and_perimeter_f64(&poly_shape[0]);
    let hole_scale = if ext_perim < 16.0 {
        (ext_perim / 16.0).clamp(0.005, 1.0)
    } else {
        1.0
    };
    let min_hole_area = 1.5 * hole_scale * hole_scale;
    let min_hole_thickness = 0.15 * hole_scale;
    let holes = poly_shape[1..]
        .iter()
        .filter(|hole_ring| {
            if !filter_sliver_holes {
                return true;
            }
            let (a, p) = ring_area_and_perimeter_f64(hole_ring);
            let circularity = (4.0 * std::f64::consts::PI * a) / (p * p).max(1e-9);
            if circularity >= 0.18 {
                a >= 1e-4
            } else {
                a >= min_hole_area && (a / p.max(1e-5)) >= min_hole_thickness
            }
        })
        .filter_map(|hole_ring| {
            reconstruct_or_refit_quad_contour(hole_ring, quad_table, grid, direct_map, false)
        })
        .collect();
    Some(FillShape::new(color, exterior, holes))
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
    if signed_area_f64(&bounds_ring) > 0.0 {
        bounds_ring.reverse();
    }

    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let mut empty_direct_map: DirectQuadMap = std::collections::HashMap::new();
    let clip_shapes: Vec<OverlayShape> = candidates
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table, &mut empty_direct_map, None))
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

    let mut cached_grid: Option<QuadSpatialGrid> = None;
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

        let grid = cached_grid.get_or_insert_with(|| QuadSpatialGrid::new(&quad_table));
        let Some(candidate) = reconstruct_fill_shape_from_rings(
            &poly_shape,
            &quad_table,
            grid,
            &empty_direct_map,
            fill_color,
            false,
        ) else {
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
    end_pt: Point,
    bbox: KRect,
    active: bool,
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
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut active_count = 0usize;
        for rec in quad_table {
            if !rec.active {
                continue;
            }
            active_count += 1;
            min_x = min_x.min(rec.bbox.x0);
            min_y = min_y.min(rec.bbox.y0);
            max_x = max_x.max(rec.bbox.x1);
            max_y = max_y.max(rec.bbox.y1);
        }
        if active_count == 0 {
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
        let side = (active_count as f64).sqrt().ceil() as usize;
        let cols = side.clamp(1, 32);
        let rows = side.clamp(1, 32);
        let span_x = (max_x - min_x).max(1.0);
        let span_y = (max_y - min_y).max(1.0);
        let inv_cell_w = (cols as f64) / span_x;
        let inv_cell_h = (rows as f64) / span_y;
        let mut cells = vec![Vec::new(); cols * rows];

        for (qid, rec) in quad_table.iter().enumerate() {
            if !rec.active {
                continue;
            }
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
enum EdgeQuadMatch {
    Direct(usize, bool),
    Subdivided(usize, f64),
}

#[derive(Clone, Copy, Debug)]
struct TaggedEdge {
    from: Point,
    to: Point,
    quad_match: Option<EdgeQuadMatch>,
}

fn flatten_contour_for_overlay(
    contour: &Contour,
    is_outer: bool,
    quad_table: &mut Vec<QuadRecord>,
    direct_map: &mut DirectQuadMap,
    active_bboxes: Option<&[KRect]>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table, direct_map, active_bboxes)?;
    let area = signed_area_f64(&ring);
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
    direct_map: &mut DirectQuadMap,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table, direct_map, None)?;
    ring.reverse();
    Some(ring)
}

fn flatten_shape_for_overlay(
    shape: &FillShape,
    quad_table: &mut Vec<QuadRecord>,
    direct_map: &mut DirectQuadMap,
    active_bboxes: Option<&[KRect]>,
) -> OverlayShape {
    let Some(ext_ring) = flatten_contour_for_overlay(
        &shape.exterior,
        true,
        quad_table,
        direct_map,
        active_bboxes,
    ) else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) =
            flatten_contour_for_overlay(hole, false, quad_table, direct_map, active_bboxes)
        {
            rings.push(hole_ring);
        }
    }
    rings
}

/// 輪郭変形（エッジつまみ変形・頂点移動）後のシェイプの自己交差・反対側輪郭の突き抜けを解消し、
/// 正のワインディング領域（`OverlayFillRule::Positive`）のみを残して1つまたは複数の `FillShape` に分割・再構築する。
pub(crate) fn resolve_deformed_shape(shape: &FillShape) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let mut direct_map: DirectQuadMap = std::collections::HashMap::new();
    let Some(ext_ring) =
        flatten_contour_preserving_direction(&shape.exterior, &mut quad_table, &mut direct_map)
    else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) =
            flatten_contour_preserving_direction(hole, &mut quad_table, &mut direct_map)
        {
            rings.push(hole_ring);
        }
    }
    let min_area = adaptive_overlay_min_area(std::slice::from_ref(shape), &[]);
    let overlay_result = run_float_overlay(
        &[rings],
        &[],
        OverlayRule::Subject,
        OverlayFillRule::Positive,
        min_area,
    );
    reconstruct_shapes_from_overlay(&overlay_result, &quad_table, &direct_map, shape.color, false)
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
    direct_map: &mut DirectQuadMap,
    active_bboxes: Option<&[KRect]>,
) -> Option<Vec<[f64; 2]>> {
    let len = contour.segments.len();
    if len == 0 {
        return None;
    }

    let mut raw_pts: Vec<(Point, Option<usize>)> = Vec::new();
    for (idx, &seg) in contour.segments.iter().enumerate() {
        if !seg.p0.x.is_finite()
            || !seg.p0.y.is_finite()
            || !seg.p1.x.is_finite()
            || !seg.p1.y.is_finite()
            || !seg.p2.x.is_finite()
            || !seg.p2.y.is_finite()
        {
            return None;
        }
        let next_p0 = contour.segments[(idx + 1) % len].p0;
        let min_x = seg.p0.x.min(seg.p1.x).min(seg.p2.x) - 0.6;
        let max_x = seg.p0.x.max(seg.p1.x).max(seg.p2.x) + 0.6;
        let min_y = seg.p0.y.min(seg.p1.y).min(seg.p2.y) - 0.6;
        let max_y = seg.p0.y.max(seg.p1.y).max(seg.p2.y) + 0.6;
        let bbox = KRect::new(min_x, min_y, max_x, max_y);
        let active = len <= 3
            || (next_p0 - seg.p0).hypot() <= 1e-3
            || active_bboxes.is_none_or(|boxes| overlaps_any_bbox(bbox, boxes));
        let qid = quad_table.len();
        quad_table.push(QuadRecord {
            quad: seg,
            end_pt: next_p0,
            bbox,
            active,
        });

        if active {
            let steps = quad_subdivision_steps(seg);
            for k in 0..steps {
                raw_pts.push((seg.eval(k as f64 / steps as f64), None));
            }
        } else {
            let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot();
            let steps = if mid_dev <= 0.16 {
                1
            } else {
                ((mid_dev / 0.16).sqrt().ceil() as usize).clamp(2, 8)
            };
            for k in 0..steps {
                let pt = if k == 0 {
                    seg.p0
                } else {
                    seg.eval(k as f64 / steps as f64)
                };
                raw_pts.push((pt, Some(qid)));
            }
        }
    }

    let mut deduped: Vec<(Point, Option<usize>)> = Vec::with_capacity(raw_pts.len());
    for (pt, qid_opt) in raw_pts {
        if let Some(last) = deduped.last_mut() {
            if (last.0 - pt).hypot() < 1e-4 {
                last.1 = qid_opt;
                continue;
            }
        }
        deduped.push((pt, qid_opt));
    }
    if deduped.len() >= 2
        && (deduped.first().unwrap().0 - deduped.last().unwrap().0).hypot() < 1e-4
    {
        deduped.pop();
    }
    if deduped.len() < 3 {
        return None;
    }

    let m = deduped.len();
    for i in 0..m {
        if let Some(qid) = deduped[i].1 {
            insert_direct_quad_edge(direct_map, deduped[i].0, deduped[(i + 1) % m].0, qid);
        }
    }

    Some(deduped.into_iter().map(|(p, _)| [p.x, p.y]).collect())
}

fn reconstruct_or_refit_quad_contour(
    ring_f64: &[[f64; 2]],
    quad_table: &[QuadRecord],
    grid: &QuadSpatialGrid,
    direct_map: &DirectQuadMap,
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
            let direct_hit = if direct_map.is_empty() {
                None
            } else {
                direct_map
                    .get(&(quantize_pt_key(a), quantize_pt_key(b)))
                    .copied()
            };
            let quad_match = if let Some((qid, rev)) = direct_hit {
                Some(EdgeQuadMatch::Direct(qid, rev))
            } else {
                match_edge_to_quad(a, b, quad_table, grid)
                    .map(|(qid, t)| EdgeQuadMatch::Subdivided(qid, t))
            };
            TaggedEdge {
                from: a,
                to: b,
                quad_match,
            }
        })
        .collect();

    if tagged_edges.iter().all(|e| e.quad_match.is_none()) {
        let r_est = estimate_brush_radius_from_ring(&pts);
        return smooth_ring_to_quad_contour(&pts, r_est, is_outer);
    }

    let edge_qid = |m: Option<EdgeQuadMatch>| match m {
        Some(EdgeQuadMatch::Direct(id, _)) | Some(EdgeQuadMatch::Subdivided(id, _)) => Some(id),
        None => None,
    };

    let shift = (0..n)
        .find(|&i| {
            edge_qid(tagged_edges[i].quad_match)
                != edge_qid(tagged_edges[(i + n - 1) % n].quad_match)
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
        match rotated[i].quad_match {
            Some(EdgeQuadMatch::Direct(qid, rev)) => {
                let rec = &quad_table[qid];
                let orig = rec.quad;
                let run_start_pt = rotated[i].from;
                let mut run_end_pt = rotated[i].to;
                let mut run_len = 1usize;

                while i + run_len < n {
                    let Some(EdgeQuadMatch::Direct(next_qid, next_rev)) =
                        rotated[i + run_len].quad_match
                    else {
                        break;
                    };
                    if next_qid != qid || next_rev != rev {
                        break;
                    }
                    run_end_pt = rotated[i + run_len].to;
                    run_len += 1;
                }

                if shift == 0 && i == 0 && run_len == n {
                    let mid_pt = rotated[n / 2].from;
                    segments.push(extract_sub_quad(orig, run_start_pt, mid_pt));
                    segments.push(extract_sub_quad(orig, mid_pt, run_start_pt));
                    break;
                }

                let expected_start = if rev { rec.end_pt } else { orig.p0 };
                let expected_end = if rev { orig.p0 } else { rec.end_pt };
                if (run_start_pt - expected_start).hypot() <= 1e-2
                    && (run_end_pt - expected_end).hypot() <= 1e-2
                {
                    let mut q = if rev {
                        QuadBez::new(orig.p2, orig.p1, orig.p0)
                    } else {
                        orig
                    };
                    q.p0 = run_start_pt;
                    q.p2 = run_end_pt;
                    segments.push(q);
                } else {
                    segments.push(extract_sub_quad(orig, run_start_pt, run_end_pt));
                }
                i += run_len;
            }
            Some(EdgeQuadMatch::Subdivided(qid, mut prev_t)) => {
                let orig_quad = quad_table[qid].quad;
                let run_start_pt = rotated[i].from;
                let mut run_end_pt = rotated[i].to;
                let mut run_len = 1usize;

                while i + run_len < n {
                    let Some(EdgeQuadMatch::Subdivided(next_qid, next_t)) =
                        rotated[i + run_len].quad_match
                    else {
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
            }
            None => {
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
