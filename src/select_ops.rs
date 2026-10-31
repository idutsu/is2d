use std::collections::HashSet;

use kurbo::{ParamCurve, ParamCurveNearest, Point, QuadBez, Rect as KRect, Vec2 as KVec2};

use crate::canvas_tool::{
    CanvasState, EditorDoc, HoverTarget, SelectSubMode, cancel_active_drag, clear_selection,
    commit_moved_selection, focus_active_layer_to_shape, sync_shapes_to_timeline,
    sync_timeline_layer_from_canvas_selection, topmost_layer_id_for_indices,
};
use crate::symbol_ops::{enter_symbol_timeline, exit_symbol_timeline};
use crate::vector_boolean::{
    BoolOp, boolean_operation, merge_shapes_into_canvas, slice_shape_with_marquee_rect,
};
use crate::vector_engine::{FillShape, make_marquee_rect_shape, rects_overlap};

/// 現在の選択図形のうち `keep_indices` のみを選択に残し、選択から外れた変更済み生シェイプのみをキャンバスへ結合・切り抜き確定する
/// （選択に残った図形は結合せず `selection_modified = true` のまま維持する）
pub(crate) fn retain_selection_and_flush_deselected(doc: &mut EditorDoc, keep_indices: &[usize]) {
    let (is_sel, any_valid) = doc.canvas.selected_shape_mask();
    if !any_valid {
        clear_selection(doc);
        return;
    }
    let mut is_keep = vec![false; doc.canvas.shapes.len()];
    let mut any_keep = false;
    for &idx in keep_indices {
        if idx < is_keep.len() && is_sel[idx] {
            is_keep[idx] = true;
            any_keep = true;
        }
    }
    if !any_keep {
        clear_selection(doc);
        return;
    }

    if doc.canvas.selection_modified {
        let keep_ids: Vec<u64> = doc
            .canvas
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| is_keep[idx].then_some(s.shape_id))
            .collect();
        let insert_limit = doc.canvas.scope_insert_index();
        let mut base_canvas = Vec::with_capacity(doc.canvas.shapes.len());
        let mut deselected_raw_shapes = Vec::new();
        let mut kept_shapes = Vec::with_capacity(keep_ids.len());
        let mut trailing_outside = Vec::new();
        for (idx, shape) in doc.canvas.shapes.drain(..).enumerate() {
            if is_keep[idx] {
                kept_shapes.push(shape);
            } else if is_sel[idx] && shape.is_raw_at_scope(&doc.canvas.editing_group_path) {
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
        doc.canvas.shapes = base_canvas;
        doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
        doc.canvas.restore_selection_by_shape_ids(&keep_ids);
        if merged_any {
            doc.canvas.clear_unsliced_snapshot();
            sync_shapes_to_timeline(doc);
        }
        doc.canvas.selection_modified = !doc.canvas.selected_shapes.is_empty();
    } else {
        doc.canvas.selected_shapes = (0..is_keep.len()).filter(|&i| is_keep[i]).collect();
        let any_raw_kept = doc.canvas.selected_shapes.iter().any(|&i| {
            doc.canvas
                .shapes
                .get(i)
                .is_some_and(|s| s.is_raw_at_scope(&doc.canvas.editing_group_path))
        });
        if !any_raw_kept {
            if let Some(mut unsliced) = doc.canvas.take_unsliced_snapshot() {
                doc.canvas.salvage_caches_into(&mut unsliced);
                let keep_ids = doc.canvas.selected_shape_ids();
                doc.canvas.shapes = unsliced;
                doc.canvas.restore_selection_by_shape_ids(&keep_ids);
            }
        }
    }
    doc.canvas.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(doc);
}

/// ダブルクリック位置 `pt` にシンボルがあればそのシンボルの独自タイムラインへ入り、グループがあればインプレイス編集モードへ移行する
pub(crate) fn enter_group_at(doc: &mut EditorDoc, pt: Point) -> bool {
    let Some(hit_idx) = doc.canvas.topmost_shape_at(pt) else {
        return false;
    };
    let Some(gid) =
        doc.canvas.shapes[hit_idx].active_group_id_at_scope(&doc.canvas.editing_group_path)
    else {
        return false;
    };
    if let Some(binding) = doc.canvas.shapes[hit_idx]
        .symbol_instance
        .filter(|b| b.instance_gid == gid)
    {
        return enter_symbol_timeline(doc, binding);
    }
    clear_selection(doc);
    doc.canvas.editing_group_path.push(gid);
    true
}

/// 現在入っているグループまたはシンボルタイムライン編集階層から1段階上の親階層へ戻る
pub(crate) fn exit_current_group(doc: &mut EditorDoc) -> bool {
    if !doc.canvas.editing_group_path.is_empty() {
        clear_selection(doc);
        let Some(exited_gid) = doc.canvas.editing_group_path.pop() else {
            return false;
        };
        doc.canvas.selected_shapes = (0..doc.canvas.shapes.len())
            .filter(|&i| {
                doc.canvas.shapes[i].active_group_id_at_scope(&doc.canvas.editing_group_path)
                    == Some(exited_gid)
            })
            .collect();
        doc.canvas.transform_pivot = None;
        return true;
    }
    exit_symbol_timeline(doc)
}

/// キャンバス上のダブルクリックによるグループ/シンボルへのインプレイス進入、または空白背景ダブルクリックによる親階層への復帰を処理する
pub(crate) fn handle_canvas_double_click(doc: &mut EditorDoc, pt: Point) {
    if !enter_group_at(doc, pt) && doc.canvas.topmost_shape_at(pt).is_none() {
        exit_current_group(doc);
    }
}

/// Shift クリック等で指定シェイプ（またはグループ塊全体）の選択状態をトグルし、変形ピボットをリセットする
pub(crate) fn toggle_shape_selection(doc: &mut EditorDoc, shape_idx: usize) {
    let unit = doc.canvas.expand_to_group_unit(shape_idx);
    let (sel_mask, _) = doc.canvas.selected_shape_mask();
    if sel_mask.get(shape_idx).copied().unwrap_or(false) {
        let remaining: Vec<usize> = doc
            .canvas
            .selected_shapes
            .iter()
            .copied()
            .filter(|i| unit.binary_search(i).is_err())
            .collect();
        retain_selection_and_flush_deselected(doc, &remaining);
    } else {
        for idx in unit {
            if !sel_mask.get(idx).copied().unwrap_or(false) {
                doc.canvas.selected_shapes.push(idx);
            }
        }
        doc.canvas.transform_pivot = None;
        sync_timeline_layer_from_canvas_selection(doc);
    }
}

/// 既存の選択を確定解除した上で、指定座標 `pt` にある最前面シェイプ（塗り・境界線・頂点、またはグループ塊全体）を選択する
pub(crate) fn select_single_shape_at(doc: &mut EditorDoc, pt: Point) -> bool {
    clear_selection(doc);
    let zoom = doc.viewport.zoom;
    let sub_mode = doc.ui.select_sub_mode;
    if let Some(hit_idx) = doc
        .canvas
        .topmost_shape_at(pt)
        .or_else(|| compute_hover_target(&doc.canvas, pt, zoom, sub_mode).shape_idx())
    {
        doc.canvas.selected_shapes = doc.canvas.expand_to_group_unit(hit_idx);
        focus_active_layer_to_shape(doc, hit_idx);
        sync_timeline_layer_from_canvas_selection(doc);
        true
    } else {
        false
    }
}

/// Adobe Animate 風の矩形選択（Marquee Selection）
pub(crate) fn apply_marquee_selection(doc: &mut EditorDoc, p0: Point, p1: Point, additive: bool) {
    let min_x = p0.x.min(p1.x);
    let max_x = p0.x.max(p1.x);
    let min_y = p0.y.min(p1.y);
    let max_y = p0.y.max(p1.y);

    let min_marquee = 2.0 / (doc.viewport.zoom as f64);
    if (max_x - min_x) < min_marquee || (max_y - min_y) < min_marquee {
        if !additive {
            clear_selection(doc);
        }
        return;
    }

    if !additive {
        clear_selection(doc);
    }

    let rect = KRect::new(min_x, min_y, max_x, max_y);
    let clip_rect_shape = make_marquee_rect_shape(rect);
    let (was_selected, _) = if additive {
        doc.canvas.selected_shape_mask()
    } else {
        (vec![false; doc.canvas.shapes.len()], false)
    };
    let hit_group_ids = collect_marquee_hit_groups(&doc.canvas, rect, &clip_rect_shape);
    let pre_slice_snapshot =
        (!doc.canvas.has_unsliced_snapshot()).then(|| doc.canvas.shapes.clone());

    let mut any_sliced = false;
    let mut new_shapes = Vec::with_capacity(doc.canvas.shapes.len() + 4);
    let mut new_selected = Vec::new();

    for (idx, shape) in doc.canvas.shapes.drain(..).enumerate() {
        if was_selected[idx] {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
            continue;
        }
        if !shape.is_in_editing_scope(&doc.canvas.editing_group_path) {
            new_shapes.push(shape);
            continue;
        }
        if let Some(gid) = shape.active_group_id_at_scope(&doc.canvas.editing_group_path) {
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

    if any_sliced && !doc.canvas.has_unsliced_snapshot() {
        if let Some(snap) = pre_slice_snapshot {
            doc.canvas.restore_unsliced_snapshot(snap);
        }
    }
    doc.canvas.shapes = new_shapes;
    doc.canvas.selected_shapes = new_selected;
    if doc.canvas.selected_shapes.is_empty() && !doc.canvas.selection_modified {
        if let Some(unsliced) = doc.canvas.take_unsliced_snapshot() {
            doc.canvas.shapes = unsliced;
        }
    }
    doc.canvas.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(doc);
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
    canvas: &CanvasState,
    rect: KRect,
    clip_rect_shape: &FillShape,
) -> HashSet<u64> {
    let mut hit_group_ids: HashSet<u64> = HashSet::new();
    for shape in &canvas.shapes {
        let Some(gid) = shape.active_group_id_at_scope(&canvas.editing_group_path) else {
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
    canvas: &CanvasState,
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

    let (is_sel, has_sel) = canvas.selected_shape_mask();

    if sub_mode == SelectSubMode::Reshape && has_sel {
        for &s_idx in canvas.selected_shapes.iter().rev() {
            let Some(shape) = canvas.shapes.get(s_idx) else {
                continue;
            };
            if !shape.is_in_editing_scope(&canvas.editing_group_path)
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

    let topmost_fill_idx = canvas.topmost_shape_at(mouse_pt);

    if sub_mode != SelectSubMode::AddAnchor {
        let v_radius = if sub_mode == SelectSubMode::DeleteAnchor {
            anchor_edit_hit_radius
        } else {
            vertex_hit_radius
        };
        let mut best_vertex: Option<(f64, HoverTarget)> = None;
        for (s_idx, shape) in canvas.shapes.iter().enumerate().rev() {
            if topmost_fill_idx.is_some_and(|top| s_idx < top) {
                continue;
            }
            let Some(bbox) = shape.geometry_bounding_box() else {
                continue;
            };
            if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
                || !shape.is_raw_at_scope(&canvas.editing_group_path)
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
                        let occluded_by_front = (s_idx + 1..canvas.shapes.len()).any(|front_idx| {
                            canvas.shapes[front_idx].is_in_editing_scope(&canvas.editing_group_path)
                                && canvas.shapes[front_idx].contains_point(seg.p0)
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
    for (s_idx, shape) in canvas.shapes.iter().enumerate().rev() {
        if topmost_fill_idx.is_some_and(|top| s_idx < top) {
            continue;
        }
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
            || !shape.is_raw_at_scope(&canvas.editing_group_path)
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
                    let occluded_by_front = (s_idx + 1..canvas.shapes.len()).any(|front_idx| {
                        canvas.shapes[front_idx].is_in_editing_scope(&canvas.editing_group_path)
                            && canvas.shapes[front_idx].contains_point(eval_pt)
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

/// 選択中のオブジェクト（生シェイプ・既存グループ問わず）を新しい1つのグループにまとめる（`Cmd+G` / 無限入れ子対応）
pub(crate) fn group_selected_shapes(doc: &mut EditorDoc) -> Option<u64> {
    cancel_active_drag(doc);
    if doc.canvas.selected_shapes.is_empty() {
        return None;
    }
    let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
    let new_gid = doc.symbols.alloc_group_id();

    let target_lid =
        topmost_layer_id_for_indices(&doc.canvas, &doc.timeline, &doc.canvas.selected_shapes);

    let depth = doc.canvas.editing_group_path.len();
    let mut any_grouped = false;
    for &sel_idx in &doc.canvas.selected_shapes {
        if let Some(shape) = doc.canvas.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&doc.canvas.editing_group_path) {
                shape.layer_id = target_lid;
                shape.group_path.insert(depth, new_gid);
                any_grouped = true;
            }
        }
    }
    if !any_grouped {
        doc.canvas.restore_unsliced_snapshot(snapshot);
        return None;
    }

    if let Some(l_idx) = doc.timeline.layer_index_by_id(target_lid) {
        doc.timeline.active_layer = l_idx;
    }
    commit_moved_selection(doc, snapshot);
    doc.canvas.selection_modified = false;
    doc.canvas.transform_pivot = None;
    Some(new_gid)
}

/// 選択中のグループまたはシンボルインスタンスを1段階だけ解除（分離）する（`Cmd+B`）。
/// 多段入れ子の場合は内側の子グループ構造を維持したまま最外殻のグループのみを外す。
pub(crate) fn ungroup_selected_shapes(doc: &mut EditorDoc) -> bool {
    cancel_active_drag(doc);
    if !doc.canvas.selection_has_group() {
        return false;
    }
    let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();

    let depth = doc.canvas.editing_group_path.len();
    let mut any_became_raw = false;
    let mut detached_inner_gid_map: Vec<((u64, u64), u64)> = Vec::new();
    for &sel_idx in &doc.canvas.selected_shapes {
        if let Some(shape) = doc.canvas.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&doc.canvas.editing_group_path)
                && shape.group_path.len() > depth
            {
                let removed_gid = shape.group_path.remove(depth);
                if shape
                    .symbol_instance
                    .is_some_and(|b| b.instance_gid == removed_gid)
                {
                    shape.symbol_instance = None;
                    for inner_gid in &mut shape.group_path[depth..] {
                        let key = (removed_gid, *inner_gid);
                        let mapped = if let Some(&(_, m)) =
                            detached_inner_gid_map.iter().find(|&&(k, _)| k == key)
                        {
                            m
                        } else {
                            let alloc = doc.symbols.alloc_group_id();
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
    let (mut is_sel, _) = doc.canvas.selected_shape_mask();
    let mut write_idx = 0;
    for read_idx in 0..doc.canvas.shapes.len() {
        let keep = !doc.canvas.shapes[read_idx].exterior.segments.is_empty()
            || doc.canvas.shapes[read_idx].symbol_instance.is_some();
        if keep {
            if write_idx != read_idx {
                doc.canvas.shapes.swap(write_idx, read_idx);
                is_sel[write_idx] = is_sel[read_idx];
            }
            write_idx += 1;
        }
    }
    if write_idx < doc.canvas.shapes.len() {
        doc.canvas.shapes.truncate(write_idx);
        doc.canvas.selected_shapes = (0..write_idx).filter(|&i| is_sel[i]).collect();
    }
    if any_became_raw {
        doc.canvas.selection_modified = true;
    }
    doc.canvas.transform_pivot = None;
    doc.commit_canvas_mutation(snapshot);
    true
}

/// 選択中のオブジェクトを十字キー等で `delta` だけ移動し、Undo 履歴記録・最前面整列・タイムライン同期を行う
pub(crate) fn nudge_selected_shapes(doc: &mut EditorDoc, delta: KVec2) -> bool {
    cancel_active_drag(doc);
    let (_, any_valid) = doc.canvas.selected_shape_mask();
    if !any_valid {
        return false;
    }
    let undo_snapshot = doc.canvas.snapshot_for_drag();
    doc.canvas.translate_selected(delta);
    commit_moved_selection(doc, undo_snapshot);
    true
}

pub(crate) fn delete_selected_shapes(doc: &mut EditorDoc) -> bool {
    cancel_active_drag(doc);
    if doc.canvas.selected_shapes.is_empty() {
        return false;
    }
    let (is_sel, any_valid) = doc.canvas.selected_shape_mask();
    if !any_valid {
        doc.canvas.selected_shapes.clear();
        doc.canvas.clear_unsliced_snapshot();
        doc.canvas.transform_pivot = None;
        return false;
    }

    let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();

    let mut remaining = Vec::with_capacity(doc.canvas.shapes.len());
    for (idx, shape) in doc.canvas.shapes.drain(..).enumerate() {
        if !is_sel[idx] {
            remaining.push(shape);
        }
    }
    doc.canvas.shapes = remaining;
    doc.canvas.selection_modified = false;
    doc.canvas.selected_shapes.clear();
    doc.canvas.transform_pivot = None;
    doc.commit_canvas_mutation(snapshot);
    true
}

/// 現在の編集スコープ（グループ編集時はそのグループ内、通常時は現在フレームの全編集可能レイヤー）の図形をすべて消去する
pub(crate) fn clear_active_frame_shapes(doc: &mut EditorDoc) -> bool {
    let has_any_in_scope = doc
        .canvas
        .shapes
        .iter()
        .any(|s| s.is_in_editing_scope(&doc.canvas.editing_group_path));
    if !has_any_in_scope {
        return false;
    }
    doc.mutate_canvas_with_undo(true, |doc| {
        doc.canvas
            .shapes
            .retain(|s| !s.is_in_editing_scope(&doc.canvas.editing_group_path));
    });
    true
}
