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

fn push_by_color(groups: &mut Vec<(Color32, Vec<FillShape>)>, s: FillShape) {
    if let Some((_, g)) = groups.iter_mut().find(|(c, _)| *c == s.color) {
        g.push(s);
    } else {
        groups.push((s.color, vec![s]));
    }
}

/// 互いに重ならず同色で接する片もない `Floating::pieces`（`pieces_disjoint == true`）をキャンバスへマージする
pub(crate) fn merge_disjoint_pieces_into_canvas(canvas: &mut Vec<FillShape>, pieces: Vec<FillShape>) {
    let mut scope_groups: Vec<(u64, Vec<u64>, Vec<FillShape>)> = Vec::new();
    for p in pieces {
        if let Some((_, _, g)) = scope_groups.iter_mut().find(|(l, gp, _)| *l == p.layer_id && *gp == p.group_path) {
            g.push(p);
        } else {
            scope_groups.push((p.layer_id, p.group_path.clone(), vec![p]));
        }
    }
    for (lid, gp, scope_pieces) in scope_groups {
        let base_bboxes: Vec<KRect> = canvas
            .iter()
            .filter(|s| s.layer_id == lid && s.group_path == gp && s.symbol_instance.is_none())
            .map(|s| s.bounding_box())
            .collect();
        let (overlapping, non_overlapping): (Vec<_>, Vec<_>) = scope_pieces.into_iter().partition(|p| {
            let pb = p.bounding_box().inflate(1.5, 1.5);
            base_bboxes.iter().any(|&bb| rects_overlap(bb, pb))
        });
        if overlapping.is_empty() {
            let pos = raw_scope_insert_index(canvas, lid, &gp);
            canvas.splice(pos..pos, non_overlapping);
            continue;
        }
        let test_bbox = union_bounding_boxes(&overlapping).inflate(1.5, 1.5);
        let clip = PreparedClip::new(&overlapping);
        let trailing_outside = canvas.split_off(raw_scope_insert_index(canvas, lid, &gp));
        let mut updated_canvas = Vec::with_capacity(canvas.len() + overlapping.len() + non_overlapping.len() + trailing_outside.len());
        let mut base_by_color = Vec::new();
        for existing in canvas.drain(..) {
            if existing.layer_id == lid && existing.group_path == gp && existing.symbol_instance.is_none() && rects_overlap(existing.bounding_box(), test_bbox) {
                push_by_color(&mut base_by_color, existing);
            } else {
                updated_canvas.push(existing);
            }
        }
        for item in &clip.items {
            if !base_by_color.iter().any(|(col, _)| *col == item.shape.color) {
                base_by_color.push((item.shape.color, Vec::new()));
            }
        }
        clip.apply_to_color_groups(base_by_color, lid, &gp, true, &mut updated_canvas);
        updated_canvas.extend(non_overlapping);
        updated_canvas.extend(trailing_outside);
        *canvas = updated_canvas;
    }
}

fn merge_homogeneous_batch_into_canvas(canvas: &mut Vec<FillShape>, incoming: Vec<FillShape>, subtract_diff_color: bool) {
    if incoming.is_empty() {
        return;
    }
    let (target_color, target_layer_id, target_group_path) = (incoming[0].color, incoming[0].layer_id, incoming[0].group_path.clone());
    let test_bbox = union_bounding_boxes(&incoming).inflate(1.5, 1.5);
    let clip_sub_bboxes = collect_sub_bboxes(&incoming, 1.5);
    let trailing_outside = canvas.split_off(raw_scope_insert_index(canvas, target_layer_id, &target_group_path));
    let mut updated_canvas = Vec::with_capacity(canvas.len() + incoming.len() + trailing_outside.len());
    let (mut overlapping_same_color, mut overlapping_diff_by_color) = (Vec::new(), Vec::new());

    for existing in canvas.drain(..) {
        if existing.layer_id != target_layer_id || existing.group_path != target_group_path || existing.symbol_instance.is_some() {
            updated_canvas.push(existing);
            continue;
        }
        let interacts = rects_overlap(existing.bounding_box(), test_bbox) && shape_interacts_with_clip(&existing, &incoming, &clip_sub_bboxes);
        if existing.color == target_color && interacts {
            overlapping_same_color.push(existing);
        } else if existing.color != target_color && subtract_diff_color && interacts {
            push_by_color(&mut overlapping_diff_by_color, existing);
        } else {
            updated_canvas.push(existing);
        }
    }
    if !overlapping_diff_by_color.is_empty() {
        PreparedClip::new(&incoming).apply_to_color_groups(overlapping_diff_by_color, target_layer_id, &target_group_path, false, &mut updated_canvas);
    }

    let incoming_self_overlaps = incoming.len() > 1 && incoming.iter().enumerate().any(|(i, a)| {
        let a_box = a.bounding_box().inflate(1.5, 1.5);
        incoming[i + 1..].iter().any(|b| rects_overlap(a_box, b.bounding_box()))
    });
    if overlapping_same_color.is_empty() && !incoming_self_overlaps {
        updated_canvas.extend(incoming);
    } else if overlapping_same_color.is_empty() {
        let unioned = boolean_operation(&incoming[..1], &incoming[1..], BoolOp::Union, target_color);
        let mut used_ids = Vec::with_capacity(unioned.len());
        updated_canvas.extend(unioned.into_iter().map(|mut s| {
            let s_bbox = s.bounding_box().inflate(1.5, 1.5);
            if let Some(inc) = incoming.iter().find(|inc| !used_ids.contains(&inc.shape_id) && rects_overlap(s_bbox, inc.bounding_box())) {
                used_ids.push(inc.shape_id);
                s.shape_id = inc.shape_id;
            }
            s.with_scope(target_layer_id, &target_group_path)
        }));
    } else {
        updated_canvas.extend(
            boolean_operation(&overlapping_same_color, &incoming, BoolOp::Union, target_color)
                .into_iter()
                .map(|s| s.with_scope(target_layer_id, &target_group_path)),
        );
    }
    updated_canvas.extend(trailing_outside);
    *canvas = updated_canvas;
}

/// 消しゴムの輪郭で、指定されたアクティブレイヤー（`target_layer_id`）かつ同じ編集スコープ（`editing_scope`）直下の生シェイプに対してのみ Boolean Difference を適用する
pub(crate) fn erase_from_canvas(canvas: &mut Vec<FillShape>, eraser_shapes: &[FillShape], target_layer_id: u64, editing_scope: &[u64]) {
    if eraser_shapes.is_empty() {
        return;
    }
    let test_bbox = union_bounding_boxes(eraser_shapes).inflate(1.5, 1.5);
    let clip_sub_bboxes = collect_sub_bboxes(eraser_shapes, 1.5);
    let trailing_outside = canvas.split_off(raw_scope_insert_index(canvas, target_layer_id, editing_scope));
    let mut updated_canvas = Vec::with_capacity(canvas.len() + trailing_outside.len());
    let mut overlapping_by_color = Vec::new();
    for existing in canvas.drain(..) {
        if existing.layer_id == target_layer_id
            && existing.group_path == editing_scope
            && existing.symbol_instance.is_none()
            && rects_overlap(existing.bounding_box(), test_bbox)
            && shape_interacts_with_clip(&existing, eraser_shapes, &clip_sub_bboxes)
        {
            push_by_color(&mut overlapping_by_color, existing);
        } else {
            updated_canvas.push(existing);
        }
    }
    if !overlapping_by_color.is_empty() {
        PreparedClip::new(eraser_shapes).apply_to_color_groups(overlapping_by_color, target_layer_id, editing_scope, false, &mut updated_canvas);
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
            for &seg in c.segments.iter() {
                if (seg.p2 - seg.p0).hypot() > 16.0 {
                    let (a, b) = seg.subdivide();
                    out.extend([seg_padded_bbox(a, inflate), seg_padded_bbox(b, inflate)]);
                } else {
                    out.push(seg_padded_bbox(seg, inflate));
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

fn shape_interacts_with_clip(subj: &FillShape, clip: &[FillShape], clip_sub_bboxes: &[KRect]) -> bool {
    (overlaps_any_bbox(subj.bounding_box(), clip_sub_bboxes)
        && subj.contours().any(|c| {
            c.segments.iter().any(|&seg| {
                overlaps_any_bbox(seg_padded_bbox(seg, 0.1), clip_sub_bboxes)
                    && ((seg.p2 - seg.p0).hypot() <= 16.0 || {
                        let (a, b) = seg.subdivide();
                        overlaps_any_bbox(seg_padded_bbox(a, 0.1), clip_sub_bboxes) || overlaps_any_bbox(seg_padded_bbox(b, 0.1), clip_sub_bboxes)
                    })
            })
        }))
        || subj.contours().any(|c| c.segments.first().is_some_and(|s| clip.iter().any(|cs| cs.contains_point(s.p0))))
        || clip.iter().any(|cs| cs.contours().any(|c| c.segments.first().is_some_and(|s| subj.contains_point(s.p0))))
}

fn poly_shape_matches_input(out_poly: &OverlayShape, in_poly: &OverlayShape) -> bool {
    let eq_pt = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= 1e-3 && (a[1] - b[1]).abs() <= 1e-3;
    let is_subseq = |out_r: &[[f64; 2]], in_r: &[[f64; 2]], s: usize, rev: bool| {
        let (n, mut j) = (in_r.len(), 0);
        out_r.iter().all(|&op| {
            while j < n && !eq_pt(op, in_r[if rev { (s + n - j) % n } else { (s + j) % n }]) {
                j += 1;
            }
            (j < n).then(|| j += 1).is_some()
        })
    };
    out_poly.len() == in_poly.len()
        && out_poly.iter().zip(in_poly.iter()).all(|(out_r, in_r)| {
            let ((out_a, out_p), (in_a, in_p)) = (ring_area_and_perimeter_f64(out_r), ring_area_and_perimeter_f64(in_r));
            out_r.len() >= 3 && out_r.len() <= in_r.len() && (out_a - in_a).abs() <= 1e-2 && (out_p - in_p).abs() <= 1e-2
                && in_r.iter().position(|&p| eq_pt(p, out_r[0])).is_some_and(|s| is_subseq(out_r, in_r, s, false) || is_subseq(out_r, in_r, s, true))
        })
}

struct PreparedClipItem<'a> {
    shape: &'a FillShape,
    padded_bbox: KRect,
    sub_bboxes: Vec<KRect>,
    poly: OverlayShape,
    quads: Vec<QuadRecord>,
}

/// 複数の対象シェイプに対して繰り返しブーリアン演算を行う際に、
/// クリップ側シェイプ群のフラットニング結果・`QuadRecord`・サブAABBを1回だけ事前構築して使い回すキャッシュ構造体
struct PreparedClip<'a> {
    items: Vec<PreparedClipItem<'a>>,
}

impl<'a> PreparedClip<'a> {
    fn new(clip: &'a [FillShape]) -> Self {
        let items = clip
            .iter()
            .filter_map(|s| {
                let mut quads = Vec::new();
                let poly = flatten_shape_for_overlay(s, &mut quads);
                (!poly.is_empty()).then(|| PreparedClipItem {
                    shape: s,
                    padded_bbox: s.bounding_box().inflate(1.5, 1.5),
                    sub_bboxes: collect_sub_bboxes(std::slice::from_ref(s), 0.15),
                    poly,
                    quads,
                })
            })
            .collect();
        Self { items }
    }

    fn apply_to_color_groups(
        &self,
        groups: Vec<(Color32, Vec<FillShape>)>,
        lid: u64,
        gp: &[u64],
        include_same_color: bool,
        out: &mut Vec<FillShape>,
    ) {
        struct ColorJob {
            color: Color32,
            bbox: KRect,
            sub_bboxes: Vec<KRect>,
            active: Vec<FillShape>,
            used_in: Vec<bool>,
            subj_polys: Vec<OverlayShape>,
            quads: Vec<QuadRecord>,
            used_diff: Vec<bool>,
            any_same_merged: bool,
            out: Vec<FillShape>,
        }
        let mut jobs = Vec::with_capacity(groups.len());
        for (color, subjs) in groups {
            let (mut active, mut subj_polys, mut quads, mut job_out, mut sub_bboxes) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let (mut bbox, mut used_diff, mut used_same) =
                (KRect::ZERO, vec![false; self.items.len()], vec![false; self.items.len()]);
            for subj in subjs {
                let mut touches = false;
                for (i, it) in self.items.iter().enumerate() {
                    if rects_overlap(subj.bounding_box(), it.padded_bbox)
                        && shape_interacts_with_clip(&subj, std::slice::from_ref(it.shape), &it.sub_bboxes)
                    {
                        if include_same_color && it.shape.color == color {
                            used_same[i] = true;
                        } else {
                            used_diff[i] = true;
                        }
                        touches = true;
                    }
                }
                if !touches {
                    job_out.push(subj.with_scope(lid, gp));
                } else {
                    let poly = flatten_shape_for_overlay(&subj, &mut quads);
                    if !poly.is_empty() {
                        let sb = subj.bounding_box().inflate(0.15, 0.15);
                        bbox = if subj_polys.is_empty() { sb } else { bbox.union(sb) };
                        sub_bboxes.extend(collect_sub_bboxes(std::slice::from_ref(&subj), 0.05));
                        subj_polys.push(poly);
                        active.push(subj);
                    }
                }
            }
            let mut any_same_merged = false;
            if include_same_color {
                for (i, it) in self.items.iter().enumerate().filter(|(_, it)| it.shape.color == color) {
                    if used_same[i] {
                        any_same_merged = true;
                        let ib = it.shape.bounding_box().inflate(0.15, 0.15);
                        bbox = if subj_polys.is_empty() { ib } else { bbox.union(ib) };
                        sub_bboxes.extend_from_slice(&it.sub_bboxes);
                        subj_polys.push(it.poly.clone());
                        quads.extend_from_slice(&it.quads);
                    } else {
                        job_out.push(it.shape.clone().with_scope(lid, gp));
                    }
                }
            }
            let used_in = vec![false; active.len()];
            jobs.push(ColorJob { color, bbox, sub_bboxes, active, used_in, subj_polys, quads, used_diff, any_same_merged, out: job_out });
        }

        let mut to_reconstruct = vec![Vec::new(); jobs.len()];
        let mut order: Vec<usize> = (0..jobs.len()).filter(|&i| !jobs[i].subj_polys.is_empty()).collect();
        order.sort_unstable_by_key(|&i| std::cmp::Reverse(jobs[i].sub_bboxes.len()));
        let mut done = vec![false; jobs.len()];
        for (pos, &start) in order.iter().enumerate() {
            if done[start] {
                continue;
            }
            let mut batch = vec![start];
            done[start] = true;
            for &cand in &order[(pos + 1)..] {
                if !done[cand]
                    && batch.iter().all(|&b| {
                        (!rects_overlap(jobs[b].bbox, jobs[cand].bbox)
                            || !jobs[b].sub_bboxes.iter().any(|&ra| jobs[cand].sub_bboxes.iter().any(|&rb| rects_overlap(ra, rb))))
                            && !self.items.iter().enumerate().any(|(i, it)| {
                                (jobs[b].used_diff[i] && it.shape.color == jobs[cand].color)
                                    || (jobs[cand].used_diff[i] && it.shape.color == jobs[b].color)
                            })
                    })
                {
                    batch.push(cand);
                    done[cand] = true;
                }
            }
            let (mut batch_subjs, mut batch_clips, mut min_area) = (Vec::new(), Vec::new(), 0.04_f64);
            for &b in &batch {
                min_area = min_area.min(adaptive_overlay_min_area(&jobs[b].active, &[]));
                batch_subjs.extend(jobs[b].subj_polys.iter().cloned());
            }
            for (i, it) in self.items.iter().enumerate() {
                if batch.iter().any(|&b| jobs[b].used_diff[i]) {
                    batch_clips.push(it.poly.clone());
                }
            }
            let rule = if batch_clips.is_empty() { OverlayRule::Subject } else { OverlayRule::Difference };
            for poly_shape in run_float_overlay(&batch_subjs, &batch_clips, rule, OverlayFillRule::NonZero, min_area) {
                let b = if batch.len() == 1 {
                    batch[0]
                } else {
                    poly_shape[0]
                        .iter()
                        .find_map(|&[x, y]| {
                            let pt = Point::new(x, y);
                            batch.iter().copied().find(|&b| jobs[b].bbox.contains(pt) && jobs[b].sub_bboxes.iter().any(|&r| r.contains(pt)))
                        })
                        .unwrap_or(batch[0])
                };
                if let Some(idx) = (!jobs[b].any_same_merged)
                    .then(|| (0..jobs[b].active.len()).find(|&i| !jobs[b].used_in[i] && poly_shape_matches_input(&poly_shape, &jobs[b].subj_polys[i])))
                    .flatten()
                {
                    jobs[b].used_in[idx] = true;
                    let s = jobs[b].active[idx].clone().with_scope(lid, gp);
                    jobs[b].out.push(s);
                } else {
                    to_reconstruct[b].push(poly_shape);
                }
            }
        }
        for (job, polys) in jobs.iter_mut().zip(to_reconstruct) {
            if !polys.is_empty() {
                for (_, it) in self.items.iter().enumerate().filter(|&(i, _)| job.used_diff[i]) {
                    job.quads.extend_from_slice(&it.quads);
                }
                job.out.extend(
                    reconstruct_shapes_from_overlay(&polys, &job.quads, job.color, job.any_same_merged)
                        .into_iter()
                        .map(|s| s.with_scope(lid, gp)),
                );
            }
            out.append(&mut job.out);
        }
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

    let min_area = adaptive_overlay_min_area(subj, clip);
    let overlay_result = run_float_overlay(
        &subj_shapes,
        &clip_shapes,
        overlay_rule,
        OverlayFillRule::NonZero,
        min_area,
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
    let inside = reconstruct_shapes_from_overlay(&inside_rings, &quad_table, shape.color, false);
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
    let outside = reconstruct_shapes_from_overlay(&outside_rings, &quad_table, shape.color, false);
    (inside, outside)
}

fn reconstruct_shapes_from_overlay(
    overlay_result: &[OverlayShape],
    quad_table: &[QuadRecord],
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
    color: Color32,
    filter_sliver_holes: bool,
) -> Option<FillShape> {
    if poly_shape.is_empty() {
        return None;
    }
    let exterior = reconstruct_or_refit_quad_contour(&poly_shape[0], quad_table, grid, true)?;
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
            reconstruct_or_refit_quad_contour(hole_ring, quad_table, grid, false)
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
        let Some(candidate) =
            reconstruct_fill_shape_from_rings(&poly_shape, &quad_table, grid, fill_color, false)
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
    let overlay_result = run_float_overlay(
        &[rings],
        &[],
        OverlayRule::Subject,
        OverlayFillRule::Positive,
        min_area,
    );
    reconstruct_shapes_from_overlay(&overlay_result, &quad_table, shape.color, false)
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
    grid: &QuadSpatialGrid,
    is_outer: bool,
) -> Option<Contour> {
    let pts = dedup_closed_ring(ring_f64.iter().map(|&[x, y]| Point::new(x, y)), 1e-4);
    let n = pts.len();
    if n < 3 {
        return None;
    }

    let mut hint_qid = None;
    let tagged_edges: Vec<TaggedEdge> = (0..n)
        .map(|i| {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let quad_match = match_edge_to_quad(a, b, quad_table, grid, hint_qid);
            hint_qid = quad_match.map(|(qid, _)| qid);
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
    hint_qid: Option<usize>,
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

    for qid in hint_qid.into_iter().chain(grid.candidates_at(mid).iter().copied()) {
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
            if best_score <= 1e-7 {
                break;
            }
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
