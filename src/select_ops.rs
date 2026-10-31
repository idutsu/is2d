use std::collections::HashSet;

use kurbo::{ParamCurve, ParamCurveNearest, Point, QuadBez, Rect as KRect};

use crate::canvas_tool::{
    HoverTarget, SelectSubMode, Session, Tab, cancel_active_drag, clear_selection,
    focus_active_layer_to_shape, prepare_moved_selection, sync_shapes_to_timeline,
    sync_timeline_layer_from_canvas_selection, topmost_layer_id_for_indices,
};
use crate::symbol_ops::{enter_symbol_timeline, exit_symbol_timeline};
use crate::vector_boolean::{
    BoolOp, boolean_operation, merge_shapes_into_canvas, slice_shape_with_marquee_rect,
};
use crate::vector_engine::{FillShape, make_marquee_rect_shape, normalize_vec, rects_overlap};

/// 現在の選択図形のうち `keep_indices` のみを選択に残し、選択から外れた変更済み生シェイプのみをキャンバスへ結合・切り抜き確定する
/// （選択に残った図形は結合せず `selection_modified = true` のまま維持する）
pub(crate) fn retain_selection_and_flush_deselected(tab: &mut Tab, keep_indices: &[usize]) {
    let (is_sel, any_valid) = tab.session.selected_shape_mask();
    if !any_valid {
        clear_selection(tab);
        return;
    }
    let mut is_keep = vec![false; tab.session.shapes.len()];
    let mut any_keep = false;
    for &idx in keep_indices {
        if idx < is_keep.len() && is_sel[idx] {
            is_keep[idx] = true;
            any_keep = true;
        }
    }
    if !any_keep {
        clear_selection(tab);
        return;
    }

    tab.session.set_selected_indices(
        (0..is_keep.len()).filter(|&i| is_keep[i]),
    );
    if tab.session.selection_modified {
        let insert_limit = tab.session.scope_insert_index();
        let mut base_canvas = Vec::with_capacity(tab.session.shapes.len());
        let mut deselected_raw_shapes = Vec::new();
        let mut kept_shapes = Vec::new();
        let mut trailing_outside = Vec::new();
        for (idx, shape) in tab.session.shapes.drain(..).enumerate() {
            if is_keep[idx] {
                kept_shapes.push(shape);
            } else if is_sel[idx] && shape.is_raw_at_scope(&tab.session.editing_group_path) {
                deselected_raw_shapes.push(shape);
            } else if idx < insert_limit {
                base_canvas.push(shape);
            } else {
                trailing_outside.push(shape);
            }
        }
        let merged_any = !deselected_raw_shapes.is_empty();
        if merged_any {
            merge_shapes_into_canvas(&mut base_canvas, deselected_raw_shapes);
        }
        base_canvas.extend(kept_shapes);
        base_canvas.extend(trailing_outside);
        tab.session.shapes = base_canvas;
        tab.session.sort_shapes_by_layer_order(&tab.doc.root);
        tab.session.selection.retain_existing(&tab.session.shapes);
        if merged_any {
            tab.session.clear_unsliced_snapshot();
            sync_shapes_to_timeline(tab);
        }
        tab.session.selection_modified = !tab.session.selection.is_empty();
    } else {
        let any_raw_kept = tab.session.shapes.iter().any(|s| {
            tab.session.selection.contains_shape(s)
                && s.is_raw_at_scope(&tab.session.editing_group_path)
        });
        if !any_raw_kept {
            tab.session.restore_unsliced_preserving_selection();
        }
    }
    tab.session.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(tab);
}

/// ダブルクリック位置 `pt` にシンボルがあればそのシンボルの独自タイムラインへ入り、グループがあればインプレイス編集モードへ移行する
pub(crate) fn enter_group_at(tab: &mut Tab, pt: Point) -> bool {
    let Some(hit_idx) = tab.session.topmost_shape_at(pt) else {
        return false;
    };
    let Some(gid) =
        tab.session.shapes[hit_idx].active_group_id_at_scope(&tab.session.editing_group_path)
    else {
        return false;
    };
    if let Some(binding) = tab.session.shapes[hit_idx]
        .symbol_instance
        .filter(|b| b.instance_gid == gid)
    {
        return enter_symbol_timeline(tab, binding);
    }
    clear_selection(tab);
    tab.session.editing_group_path.push(gid);
    true
}

/// 現在入っているグループまたはシンボルタイムライン編集階層から1段階上の親階層へ戻る
pub(crate) fn exit_current_group(tab: &mut Tab) -> bool {
    if !tab.session.editing_group_path.is_empty() {
        clear_selection(tab);
        let Some(exited_gid) = tab.session.editing_group_path.pop() else {
            return false;
        };
        let indices: Vec<usize> = (0..tab.session.shapes.len())
            .filter(|&i| {
                tab.session.shapes[i].active_group_id_at_scope(&tab.session.editing_group_path)
                    == Some(exited_gid)
            })
            .collect();
        tab.session.set_selected_indices(indices);
        tab.session.transform_pivot = None;
        return true;
    }
    exit_symbol_timeline(tab)
}

/// キャンバス上のダブルクリックによるグループ/シンボルへのインプレイス進入、または空白背景ダブルクリックによる親階層への復帰を処理する
pub(crate) fn handle_canvas_double_click(tab: &mut Tab, pt: Point) {
    if !enter_group_at(tab, pt) && tab.session.topmost_shape_at(pt).is_none() {
        exit_current_group(tab);
    }
}

/// Shift クリック等で指定シェイプ（またはグループ塊全体）の選択状態をトグルし、変形ピボットをリセットする
pub(crate) fn toggle_shape_selection(tab: &mut Tab, shape_idx: usize) {
    tab.history.end_coalesce();
    let unit = tab.session.expand_to_group_unit(shape_idx);
    let (sel_mask, _) = tab.session.selected_shape_mask();
    if sel_mask.get(shape_idx).copied().unwrap_or(false) {
        let remaining: Vec<usize> = tab
            .session
            .selected_indices()
            .into_iter()
            .filter(|i| unit.binary_search(i).is_err())
            .collect();
        retain_selection_and_flush_deselected(tab, &remaining);
    } else {
        tab.session.select_shape_indices(unit);
        tab.session.transform_pivot = None;
        sync_timeline_layer_from_canvas_selection(tab);
    }
}

/// 既存の選択を確定解除した上で、指定座標 `pt` にある最前面シェイプ（塗り・境界線・頂点、またはグループ塊全体）を選択する
pub(crate) fn select_single_shape_at(
    tab: &mut Tab,
    sub_mode: SelectSubMode,
    pt: Point,
) -> bool {
    clear_selection(tab);
    let zoom = tab.view.zoom;
    if let Some(hit_idx) = tab
        .session
        .topmost_shape_at(pt)
        .or_else(|| compute_hover_target(&tab.session, pt, zoom, sub_mode).shape_idx())
    {
        let unit = tab.session.expand_to_group_unit(hit_idx);
        tab.session.set_selected_indices(unit);
        focus_active_layer_to_shape(tab, hit_idx);
        sync_timeline_layer_from_canvas_selection(tab);
        true
    } else {
        false
    }
}

/// Adobe Animate 風の矩形選択（Marquee Selection）
pub(crate) fn apply_marquee_selection(tab: &mut Tab, p0: Point, p1: Point, additive: bool) {
    tab.history.end_coalesce();
    let min_x = p0.x.min(p1.x);
    let max_x = p0.x.max(p1.x);
    let min_y = p0.y.min(p1.y);
    let max_y = p0.y.max(p1.y);

    let min_marquee = 2.0 / (tab.view.zoom as f64);
    if (max_x - min_x) < min_marquee || (max_y - min_y) < min_marquee {
        if !additive {
            clear_selection(tab);
        }
        return;
    }

    if !additive {
        clear_selection(tab);
    }

    let rect = KRect::new(min_x, min_y, max_x, max_y);
    let clip_rect_shape = make_marquee_rect_shape(rect);
    let (was_selected, _) = if additive {
        tab.session.selected_shape_mask()
    } else {
        (vec![false; tab.session.shapes.len()], false)
    };
    let hit_group_ids = collect_marquee_hit_groups(&tab.session, rect, &clip_rect_shape);
    let pre_slice_snapshot =
        (!tab.session.has_unsliced_snapshot()).then(|| tab.session.shapes.clone());

    let mut any_sliced = false;
    let mut new_shapes = Vec::with_capacity(tab.session.shapes.len() + 4);
    let mut new_selected = Vec::new();

    for (idx, shape) in tab.session.shapes.drain(..).enumerate() {
        if was_selected[idx] {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
            continue;
        }
        if !shape.is_in_editing_scope(&tab.session.editing_group_path) {
            new_shapes.push(shape);
            continue;
        }
        if let Some(gid) = shape.active_group_id_at_scope(&tab.session.editing_group_path) {
            if hit_group_ids.contains(&gid) {
                new_selected.push(new_shapes.len());
            }
            new_shapes.push(shape);
            continue;
        }
        let bbox = shape.bounding_box();
        if !rects_overlap(bbox, rect) {
            new_shapes.push(shape);
            continue;
        }
        if bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1 {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
            continue;
        }

        let lid = shape.layer_id;
        let gp = shape.group_path.clone();
        let (inside, outside) = slice_shape_with_marquee_rect(&shape, &clip_rect_shape);
        if inside.is_empty() {
            new_shapes.push(shape);
        } else if outside.is_empty() {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
        } else {
            any_sliced = true;
            new_shapes.extend(outside.into_iter().map(|s| s.with_scope(lid, &gp)));
            for piece in inside {
                new_selected.push(new_shapes.len());
                new_shapes.push(piece.with_scope(lid, &gp));
            }
        }
    }

    if any_sliced && !tab.session.has_unsliced_snapshot() {
        if let Some(snap) = pre_slice_snapshot {
            tab.session.restore_unsliced_snapshot(snap);
        }
    }
    tab.session.shapes = new_shapes;
    tab.session.set_selected_indices(new_selected);
    if tab.session.selection.is_empty() && !tab.session.selection_modified {
        if let Some(unsliced) = tab.session.take_unsliced_snapshot() {
            tab.session.shapes = unsliced;
        }
    }
    tab.session.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(tab);
}

/// 2次ベジェ曲線の3制御点 `(p0, p1, p2)` の凸包AABB（余白 `margin` 付き）に点 `pt` が含まれるかを高速判定する
#[inline]
pub(crate) fn quad_control_bbox_contains(seg: &QuadBez, pt: Point, margin: f64) -> bool {
    let min_x = seg.p0.x.min(seg.p1.x).min(seg.p2.x) - margin;
    let max_x = seg.p0.x.max(seg.p1.x).max(seg.p2.x) + margin;
    let min_y = seg.p0.y.min(seg.p1.y).min(seg.p2.y) - margin;
    let max_y = seg.p0.y.max(seg.p1.y).max(seg.p2.y) + margin;
    pt.x >= min_x && pt.x <= max_x && pt.y >= min_y && pt.y <= max_y
}

/// 指定座標 `pt` がシェイプの輪郭（外周・穴）から距離 `radius` 以内にあるかを判定する
fn point_within_shape_contours(shape: &FillShape, pt: Point, radius: f64, inv_z: f64) -> bool {
    let r_sq = radius * radius;
    for contour in shape.contours() {
        for seg in contour.segments.iter() {
            if !quad_control_bbox_contains(seg, pt, radius) {
                continue;
            }
            if seg.nearest(pt, 0.05 * inv_z).distance_sq <= r_sq {
                return true;
            }
        }
    }
    false
}

/// 矩形 `rect` と交差する現在の編集スコープ直下の子グループID一覧を収集する
pub(crate) fn collect_marquee_hit_groups(
    session: &Session,
    rect: KRect,
    clip_rect_shape: &FillShape,
) -> HashSet<u64> {
    let mut hit_group_ids: HashSet<u64> = HashSet::new();
    for shape in &session.shapes {
        let Some(gid) = shape.active_group_id_at_scope(&session.editing_group_path) else {
            continue;
        };
        if hit_group_ids.contains(&gid) {
            continue;
        }
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !rects_overlap(bbox, rect) {
            continue;
        }
        let in_rect =
            bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1;
        if in_rect
            || !boolean_operation(
                std::slice::from_ref(shape),
                std::slice::from_ref(clip_rect_shape),
                BoolOp::Intersection,
                shape.color,
            )
            .is_empty()
        {
            hit_group_ids.insert(gid);
        }
    }
    hit_group_ids
}

/// アルゴリズム 4: マウス位置に対する頂点・輪郭エッジ・塗りのヒットテスト
pub(crate) fn compute_hover_target(
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
    sub_mode: SelectSubMode,
) -> HoverTarget {
    let inv_z = 1.0 / (zoom as f64);
    let vertex_hit_radius = 9.0 * inv_z;
    let edge_hit_outer_radius = 9.5 * inv_z;
    let edge_hit_inner_radius = 5.5 * inv_z;
    let anchor_edit_hit_radius = 12.0 * inv_z;
    let selected_grab_radius = 6.0 * inv_z;

    let (is_sel, has_sel) = session.selected_shape_mask();

    if sub_mode == SelectSubMode::Reshape && has_sel {
        for (s_idx, shape) in session.shapes.iter().enumerate().rev() {
            if !is_sel[s_idx] {
                continue;
            }
            if !shape.is_in_editing_scope(&session.editing_group_path)
                || !shape
                    .bounding_box()
                    .inflate(selected_grab_radius, selected_grab_radius)
                    .contains(mouse_pt)
            {
                continue;
            }
            if shape.contains_point(mouse_pt)
                || point_within_shape_contours(shape, mouse_pt, selected_grab_radius, inv_z)
            {
                return HoverTarget::Fill { shape_idx: s_idx };
            }
        }
    }

    let topmost_fill_idx = session.topmost_shape_at(mouse_pt);

    if sub_mode != SelectSubMode::AddAnchor {
        let v_radius = if sub_mode == SelectSubMode::DeleteAnchor {
            anchor_edit_hit_radius
        } else {
            vertex_hit_radius
        };
        let mut best_vertex: Option<(f64, HoverTarget)> = None;
        for (s_idx, shape) in session.shapes.iter().enumerate().rev() {
            if topmost_fill_idx.is_some_and(|top| s_idx < top) {
                continue;
            }
            let Some(bbox) = shape.geometry_bounding_box() else {
                continue;
            };
            if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
                || !shape.is_raw_at_scope(&session.editing_group_path)
                || !bbox.inflate(v_radius, v_radius).contains(mouse_pt)
            {
                continue;
            }
            for (c_idx, contour) in shape.contours().enumerate() {
                if sub_mode == SelectSubMode::DeleteAnchor && contour.segments.len() <= 2 {
                    continue;
                }
                for (seg_idx, seg) in contour.segments.iter().enumerate() {
                    let dist = (mouse_pt - seg.p0).hypot();
                    if dist <= v_radius && best_vertex.as_ref().is_none_or(|(d, _)| dist < *d) {
                        let occluded_by_front = (s_idx + 1..session.shapes.len()).any(|front_idx| {
                            session.shapes[front_idx].is_in_editing_scope(&session.editing_group_path)
                                && session.shapes[front_idx].contains_point(seg.p0)
                        });
                        if occluded_by_front {
                            continue;
                        }
                        best_vertex = Some((
                            dist,
                            HoverTarget::Vertex {
                                shape_idx: s_idx,
                                contour_idx: c_idx,
                                seg_idx,
                                pos: seg.p0,
                            },
                        ));
                    }
                }
            }
        }
        if let Some((_, target)) = best_vertex {
            return target;
        }
        if sub_mode == SelectSubMode::DeleteAnchor {
            return HoverTarget::None;
        }
    }

    let mut best_edge: Option<(f64, HoverTarget)> = None;
    for (s_idx, shape) in session.shapes.iter().enumerate().rev() {
        if topmost_fill_idx.is_some_and(|top| s_idx < top) {
            continue;
        }
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
            || !shape.is_raw_at_scope(&session.editing_group_path)
            || !bbox
                .inflate(anchor_edit_hit_radius, anchor_edit_hit_radius)
                .contains(mouse_pt)
        {
            continue;
        }
        let max_dist = if sub_mode == SelectSubMode::AddAnchor {
            anchor_edit_hit_radius
        } else if shape.contains_point(mouse_pt) {
            edge_hit_inner_radius
        } else {
            edge_hit_outer_radius
        };

        for (c_idx, contour) in shape.contours().enumerate() {
            for (seg_idx, seg) in contour.segments.iter().enumerate() {
                if !quad_control_bbox_contains(seg, mouse_pt, max_dist) {
                    continue;
                }
                let nearest = seg.nearest(mouse_pt, 0.05 * inv_z);
                let t_clamped = nearest.t.clamp(0.08, 0.92);
                let eval_pt = if sub_mode == SelectSubMode::AddAnchor {
                    seg.eval(t_clamped)
                } else {
                    seg.eval(nearest.t)
                };
                let dist = nearest.distance_sq.sqrt();
                if dist <= max_dist && best_edge.as_ref().is_none_or(|(d, _)| dist < *d) {
                    let occluded_by_front = (s_idx + 1..session.shapes.len()).any(|front_idx| {
                        session.shapes[front_idx].is_in_editing_scope(&session.editing_group_path)
                            && session.shapes[front_idx].contains_point(eval_pt)
                    });
                    if occluded_by_front {
                        continue;
                    }
                    best_edge = Some((
                        dist,
                        HoverTarget::Edge {
                            shape_idx: s_idx,
                            contour_idx: c_idx,
                            seg_idx,
                            t: if sub_mode == SelectSubMode::AddAnchor {
                                t_clamped
                            } else {
                                nearest.t
                            },
                            closest_pos: eval_pt,
                        },
                    ));
                }
            }
        }
    }
    if let Some((_, target)) = best_edge {
        return target;
    }
    if sub_mode == SelectSubMode::AddAnchor {
        return HoverTarget::None;
    }

    if let Some(s_idx) = topmost_fill_idx {
        return HoverTarget::Fill { shape_idx: s_idx };
    }

    HoverTarget::None
}

/// 向こう側の輪郭エッジ付近でドロップされた際に確実に貫通して分割されるよう僅かに押し出す。
pub(crate) fn punch_through_opposite_edge(
    shape: &FillShape,
    contour_idx: usize,
    ignore_segs: &[usize],
    (anchor_a, anchor_b): (Point, Point),
    (start_pt, drag_pt): (Point, Point),
) -> Point {
    let bbox = shape.bounding_box();
    let min_dim = bbox.width().min(bbox.height());
    let chord_len = (anchor_b - anchor_a).hypot();
    let scale = (min_dim / 16.0).min(chord_len / 24.0).clamp(0.04, 1.0);
    let min_drag = 6.0 * scale;
    let min_anchor_dist = 10.0 * scale;
    let near_thresh = 6.0 * scale;
    let min_near_anchor = 8.0 * scale;
    let punch_push = 4.5 * scale;

    let drag_vec = drag_pt - start_pt;
    if drag_vec.hypot() <= min_drag
        || (drag_pt - anchor_a).hypot() <= min_anchor_dist
        || (drag_pt - anchor_b).hypot() <= min_anchor_dist
    {
        return drag_pt;
    }

    let drag_dir = normalize_vec(drag_vec);
    let mut best_near: Option<(f64, Point)> = None;
    for (c_idx, c) in shape.contours().enumerate() {
        for (s_idx, seg) in c.segments.iter().enumerate() {
            if (c_idx == contour_idx && ignore_segs.contains(&s_idx))
                || !quad_control_bbox_contains(seg, drag_pt, near_thresh)
            {
                continue;
            }
            let near = seg.nearest(drag_pt, 0.05 * scale);
            let d = near.distance_sq.sqrt();
            if d <= near_thresh {
                let near_pt = seg.eval(near.t);
                if (near_pt - anchor_a).hypot() > min_near_anchor
                    && (near_pt - anchor_b).hypot() > min_near_anchor
                    && best_near.is_none_or(|(bd, _)| d < bd)
                {
                    best_near = Some((d, near_pt));
                }
            }
        }
    }
    if let Some((_, near_pt)) = best_near {
        let punched = near_pt + drag_dir * punch_push;
        if (punched - start_pt).dot(drag_dir) > (drag_pt - start_pt).dot(drag_dir) {
            return punched;
        }
    }
    drag_pt
}

/// エッジつまみ変形（PullEdge）の制御点を計算して反映する（リアルタイムプレビュー用）。
pub(crate) fn set_pulled_edge(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    grab_t: f64,
    orig_seg: QuadBez,
    target_pt: Point,
) {
    let grab_pos = orig_seg.eval(grab_t);
    let delta = target_pt - grab_pos;
    let denom = 2.0 * grab_t * (1.0 - grab_t);
    let new_p1 = orig_seg.p1 + delta / denom;
    if let Some(contour) = shape.get_contour_mut(contour_idx) {
        if seg_idx < contour.segments.len() {
            contour.segments_mut()[seg_idx].p1 = new_p1;
        }
    }
}

/// エッジつまみ変形（PullEdge）を対向エッジ貫通補正込みで確定適用する。
pub(crate) fn apply_pull_edge_to_shape(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    grab_t: f64,
    orig_seg: QuadBez,
    drag_pt: Point,
) {
    let grab_pos = orig_seg.eval(grab_t);
    let effective_pt = punch_through_opposite_edge(
        shape,
        contour_idx,
        &[seg_idx],
        (orig_seg.p0, orig_seg.p2),
        (grab_pos, drag_pt),
    );
    set_pulled_edge(
        shape,
        (contour_idx, seg_idx),
        grab_t,
        orig_seg,
        effective_pt,
    );
}

/// 頂点ドラッグ変形（MoveVertex）の頂点と前後制御点を計算して反映する（リアルタイムプレビュー用）。
pub(crate) fn set_moved_vertex(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    orig_vertex: Point,
    orig_ctrls: (Point, Point),
    target_pt: Point,
) {
    let delta = target_pt - orig_vertex;
    if let Some(contour) = shape.get_contour_mut(contour_idx) {
        let n = contour.segments.len();
        if n > 0 && seg_idx < n {
            let prev_idx = (seg_idx + n - 1) % n;
            let segs = contour.segments_mut();
            segs[prev_idx].p2 = target_pt;
            segs[seg_idx].p0 = target_pt;
            segs[prev_idx].p1 = orig_ctrls.0 + delta * 0.5;
            segs[seg_idx].p1 = orig_ctrls.1 + delta * 0.5;
        }
    }
}

/// 頂点ドラッグ変形（MoveVertex）を対向エッジ貫通補正込みで確定適用する。
pub(crate) fn apply_move_vertex_to_shape(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    orig_vertex: Point,
    orig_ctrls: (Point, Point),
    drag_pt: Point,
) {
    let Some((prev_idx, prev_p0, curr_p2)) = shape.get_contour(contour_idx).and_then(|c| {
        let n = c.segments.len();
        if n == 0 || seg_idx >= n {
            return None;
        }
        let prev_idx = (seg_idx + n - 1) % n;
        Some((prev_idx, c.segments[prev_idx].p0, c.segments[seg_idx].p2))
    }) else {
        return;
    };

    let effective_pt = punch_through_opposite_edge(
        shape,
        contour_idx,
        &[prev_idx, seg_idx],
        (prev_p0, curr_p2),
        (orig_vertex, drag_pt),
    );
    set_moved_vertex(
        shape,
        (contour_idx, seg_idx),
        orig_vertex,
        orig_ctrls,
        effective_pt,
    );
}

/// 選択中のオブジェクト（生シェイプ・既存グループ問わず）を新しい1つのグループにまとめ、`(new_gid, undo_snapshot)` を返す
pub(crate) fn prepare_group_selected_shapes(tab: &mut Tab) -> Option<(u64, Vec<FillShape>)> {
    cancel_active_drag(tab);
    let sel_indices = tab.session.selected_indices();
    if sel_indices.is_empty() {
        return None;
    }
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();
    let new_gid = tab.doc.alloc_group_id();

    let target_lid = topmost_layer_id_for_indices(&tab.session, &tab.doc.root, &sel_indices);

    let depth = tab.session.editing_group_path.len();
    let mut any_grouped = false;
    for &sel_idx in &sel_indices {
        if let Some(shape) = tab.session.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&tab.session.editing_group_path) {
                shape.layer_id = target_lid;
                shape.group_path.insert(depth, new_gid);
                any_grouped = true;
            }
        }
    }
    if !any_grouped {
        tab.session.restore_unsliced_snapshot(snapshot);
        return None;
    }

    if let Some(l_idx) = tab.doc.root.layer_index_by_id(target_lid) {
        tab.doc.root.active_layer = l_idx;
    }
    let _ = prepare_moved_selection(tab, &snapshot);
    tab.session.selection_modified = false;
    tab.session.transform_pivot = None;
    Some((new_gid, snapshot))
}

/// 選択中のグループまたはシンボルインスタンスを1段階だけ解除（分離）し、変更前スナップショットを返す
pub(crate) fn prepare_ungroup_selected_shapes(tab: &mut Tab) -> Option<Vec<FillShape>> {
    cancel_active_drag(tab);
    if !tab.session.selection_has_group() {
        return None;
    }
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();

    let depth = tab.session.editing_group_path.len();
    let mut any_became_raw = false;
    let mut detached_inner_gid_map: Vec<((u64, u64), u64)> = Vec::new();
    let sel_indices = tab.session.selected_indices();
    for sel_idx in sel_indices {
        if let Some(shape) = tab.session.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&tab.session.editing_group_path)
                && shape.group_path.len() > depth
            {
                let removed_gid = shape.group_path.remove(depth);
                if shape
                    .symbol_instance
                    .is_some_and(|b| b.instance_gid == removed_gid)
                {
                    shape.symbol_instance = None;
                    tab.session.selection.instances.remove(&removed_gid);
                    if !shape.exterior.segments.is_empty() {
                        tab.session.selection.shapes.insert(shape.shape_id);
                    }
                    for inner_gid in &mut shape.group_path[depth..] {
                        let key = (removed_gid, *inner_gid);
                        let mapped = if let Some(&(_, m)) =
                            detached_inner_gid_map.iter().find(|&&(k, _)| k == key)
                        {
                            m
                        } else {
                            let alloc = tab.doc.alloc_group_id();
                            detached_inner_gid_map.push((key, alloc));
                            alloc
                        };
                        *inner_gid = mapped;
                    }
                }
                if shape.group_path.len() == depth && !shape.exterior.segments.is_empty() {
                    any_became_raw = true;
                }
            }
        }
    }
    tab.session
        .shapes
        .retain(|s| !s.exterior.segments.is_empty() || s.symbol_instance.is_some());
    tab.session.selection.retain_existing(&tab.session.shapes);
    if any_became_raw {
        tab.session.selection_modified = true;
    }
    tab.session.transform_pivot = None;
    Some(snapshot)
}

/// 選択中のオブジェクトをキャンバスから削除し、変更前スナップショットを返す
pub(crate) fn prepare_delete_selected_shapes(tab: &mut Tab) -> Option<Vec<FillShape>> {
    cancel_active_drag(tab);
    if tab.session.selection.is_empty() {
        return None;
    }
    let (is_sel, any_valid) = tab.session.selected_shape_mask();
    if !any_valid {
        tab.session.selection.clear();
        tab.session.clear_unsliced_snapshot();
        tab.session.transform_pivot = None;
        return None;
    }

    let snapshot = tab.session.take_or_clone_unsliced_snapshot();

    let mut remaining = Vec::with_capacity(tab.session.shapes.len());
    for (idx, shape) in tab.session.shapes.drain(..).enumerate() {
        if !is_sel[idx] {
            remaining.push(shape);
        }
    }
    tab.session.shapes = remaining;
    tab.session.selection_modified = false;
    tab.session.selection.clear();
    tab.session.transform_pivot = None;
    Some(snapshot)
}

/// 現在の編集スコープの図形をすべて消去し、変更前スナップショットを返す
pub(crate) fn prepare_clear_active_frame_shapes(tab: &mut Tab) -> Option<Vec<FillShape>> {
    let has_any_in_scope = tab
        .session
        .shapes
        .iter()
        .any(|s| s.is_in_editing_scope(&tab.session.editing_group_path));
    if !has_any_in_scope {
        return None;
    }
    clear_selection(tab);
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();
    tab.session
        .shapes
        .retain(|s| !s.is_in_editing_scope(&tab.session.editing_group_path));
    Some(snapshot)
}
