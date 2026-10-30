use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Stroke, Vec2};
use kurbo::{ParamCurve, ParamCurveNearest, Point, QuadBez, Vec2 as KVec2};

use crate::canvas_tool::{
    begin_move_selection_drag, insert_and_select_shapes_in_scope,
    instantiate_relative_shapes_in_scope, quad_control_bbox_contains, ActiveCanvasDrag,
    CanvasState, HoverTarget, MarqueeDrag, SelectDragState, SelectSubMode,
};
pub(crate) use crate::canvas_tool::{
    apply_marquee_selection, clear_selection, commit_moved_selection, compute_hover_target,
    exit_current_group, finish_move_selection_drag, handle_canvas_double_click,
    select_single_shape_at, toggle_shape_selection,
};
#[cfg(test)]
pub(crate) use crate::canvas_tool::{
    duplicate_selected_shapes_for_alt_drag, enter_group_at,
};
use crate::vector_boolean::{merge_shapes_into_canvas, resolve_deformed_shape};
use crate::vector_engine::{arc_mesh_to_shape, normalize_vec, to_pos2, FillShape};
use crate::Is2dApp;

// ============================================================================
// 選択ツール（Tool::Select / V キー）・輪郭変形・アンカー編集・グループ・クリップボード
// ============================================================================

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

// ------------------------------------------------------------------------
// 2. グループ作成/解除・削除・クリップボード操作
// ------------------------------------------------------------------------

/// 選択中のオブジェクト（生シェイプ・既存グループ問わず）を新しい1つのグループにまとめる（`Cmd+G` / 無限入れ子対応）
pub(crate) fn group_selected_shapes(app: &mut Is2dApp) -> Option<u64> {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    if app.canvas.selected_shapes.is_empty() {
        return None;
    }
    let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
    let new_gid = app.symbols.alloc_group_id();

    let target_lid =
        crate::canvas_tool::topmost_layer_id_for_indices(app, &app.canvas.selected_shapes);

    let depth = app.canvas.editing_group_path.len();
    let mut any_grouped = false;
    for &sel_idx in &app.canvas.selected_shapes {
        if let Some(shape) = app.canvas.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&app.canvas.editing_group_path) {
                shape.layer_id = target_lid;
                shape.group_path.insert(depth, new_gid);
                any_grouped = true;
            }
        }
    }
    if !any_grouped {
        app.canvas.restore_unsliced_snapshot(snapshot);
        return None;
    }

    if let Some(l_idx) = app.timeline.layer_index_by_id(target_lid) {
        app.timeline.active_layer = l_idx;
    }
    commit_moved_selection(app, snapshot);
    app.canvas.selection_modified = false;
    app.canvas.transform_pivot = None;
    Some(new_gid)
}

/// 選択中のグループまたはシンボルインスタンスを1段階だけ解除（分離）する（`Cmd+B`）。
/// 多段入れ子の場合は内側の子グループ構造を維持したまま最外殻のグループのみを外す。
pub(crate) fn ungroup_selected_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    if !app.canvas.selection_has_group() {
        return false;
    }
    let snapshot = app.canvas.take_or_clone_unsliced_snapshot();

    let depth = app.canvas.editing_group_path.len();
    let mut any_became_raw = false;
    let mut detached_inner_gid_map: Vec<((u64, u64), u64)> = Vec::new();
    for &sel_idx in &app.canvas.selected_shapes {
        if let Some(shape) = app.canvas.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&app.canvas.editing_group_path)
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
                            let alloc = app.symbols.alloc_group_id();
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
    let (mut is_sel, _) = app.canvas.selected_shape_mask();
    let mut write_idx = 0;
    for read_idx in 0..app.canvas.shapes.len() {
        let keep = !app.canvas.shapes[read_idx].exterior.segments.is_empty()
            || app.canvas.shapes[read_idx].symbol_instance.is_some();
        if keep {
            if write_idx != read_idx {
                app.canvas.shapes.swap(write_idx, read_idx);
                is_sel[write_idx] = is_sel[read_idx];
            }
            write_idx += 1;
        }
    }
    if write_idx < app.canvas.shapes.len() {
        app.canvas.shapes.truncate(write_idx);
        app.canvas.selected_shapes = (0..write_idx).filter(|&i| is_sel[i]).collect();
    }
    if any_became_raw {
        app.canvas.selection_modified = true;
    }
    app.canvas.transform_pivot = None;
    app.commit_canvas_mutation(snapshot);
    true
}

/// 選択中のオブジェクトを十字キー等で `delta` だけ移動し、Undo 履歴記録・最前面整列・タイムライン同期を行う
pub(crate) fn nudge_selected_shapes(app: &mut Is2dApp, delta: KVec2) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    let (_, any_valid) = app.canvas.selected_shape_mask();
    if !any_valid {
        return false;
    }
    let undo_snapshot = app.canvas.snapshot_for_drag();
    app.canvas.translate_selected(delta);
    commit_moved_selection(app, undo_snapshot);
    true
}

pub(crate) fn delete_selected_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    if app.canvas.selected_shapes.is_empty() {
        return false;
    }
    let (is_sel, any_valid) = app.canvas.selected_shape_mask();
    if !any_valid {
        app.canvas.selected_shapes.clear();
        app.canvas.clear_unsliced_snapshot();
        app.canvas.transform_pivot = None;
        return false;
    }

    let snapshot = app.canvas.take_or_clone_unsliced_snapshot();

    let mut remaining = Vec::with_capacity(app.canvas.shapes.len());
    for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
        if !is_sel[idx] {
            remaining.push(shape);
        }
    }
    app.canvas.shapes = remaining;
    app.canvas.selection_modified = false;
    app.canvas.selected_shapes.clear();
    app.canvas.transform_pivot = None;
    app.commit_canvas_mutation(snapshot);
    true
}

/// 現在の編集スコープ（グループ編集時はそのグループ内、通常時は現在フレームの全編集可能レイヤー）の図形をすべて消去する
pub(crate) fn clear_active_frame_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    let has_any_in_scope = app
        .canvas
        .shapes
        .iter()
        .any(|s| s.is_in_editing_scope(&app.canvas.editing_group_path));
    if !has_any_in_scope {
        return false;
    }
    app.mutate_canvas_with_undo(true, |app| {
        app.canvas
            .shapes
            .retain(|s| !s.is_in_editing_scope(&app.canvas.editing_group_path));
    });
    true
}

/// 選択中のオブジェクト（生シェイプ・部分スライス領域・グループ問わず）を内部クリップボードにコピーする（`Cmd+C`）
pub(crate) fn copy_selected_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    let mut extracted = app.canvas.extract_selected_relative_to_scope();
    if extracted.is_empty() {
        return false;
    }
    for s in &mut extracted {
        s.strip_transient_caches();
    }
    app.canvas.clipboard = extracted;
    app.canvas.paste_count = 0;
    app.canvas.paste_offset = KVec2::ZERO;
    crate::canvas_tool::capture_clipboard_symbols(app);
    true
}

/// 選択中のオブジェクトを内部クリップボードにコピーした上でキャンバスから削除する（`Cmd+X`）
pub(crate) fn cut_selected_shapes(app: &mut Is2dApp) -> bool {
    if !copy_selected_shapes(app) {
        return false;
    }
    delete_selected_shapes(app)
}

/// 内部クリップボードのオブジェクトを現在の編集スコープへ貼り付ける。
pub(crate) fn paste_clipboard(app: &mut Is2dApp, in_place: bool) -> bool {
    crate::canvas_tool::stop_playback(app);
    if app.canvas.clipboard.is_empty() {
        return false;
    }
    let target_lid = crate::canvas_tool::active_scope_layer_id(app);
    if !app.timeline.is_layer_editable_by_id(target_lid) {
        app.notify_warning("現在のレイヤーまたはフレームは編集できないためペーストできません");
        return false;
    }
    if crate::canvas_tool::would_clipboard_paste_all_cycle(app) {
        app.notify_warning("編集中のシンボル自身や親シンボルは循環参照になるため配置できません");
        return false;
    }

    app.mutate_canvas_with_undo(true, |app| {
        let (resolved, had_cycle) = crate::canvas_tool::resolve_clipboard_for_paste(app);
        if had_cycle {
            app.notify_warning(
                "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
            );
        }
        if resolved.is_empty() {
            return;
        }

        let offset = if in_place {
            KVec2::ZERO
        } else {
            if app.canvas.paste_count == 0 {
                app.canvas.paste_offset = KVec2::ZERO;
            }
            app.canvas.paste_count += 1;
            let step = 16.0 / (app.viewport.zoom as f64);
            app.canvas.paste_offset += KVec2::new(step, step);
            app.canvas.paste_offset
        };

        let same_timeline_context = app.tabs.clipboard_source_doc_id == app.tabs.active_doc_id()
            && app.tabs.clipboard_source_symbol_id
                == app.symbols.edit_stack.last().map(|s| s.symbol_id)
            && app.canvas.editing_group_path.is_empty();
        let multi_layer_copy = same_timeline_context
            && resolved
                .first()
                .is_some_and(|first| resolved.iter().any(|s| s.layer_id != first.layer_id));
        let (pasted_shapes, any_raw) =
            instantiate_relative_shapes_in_scope(app, &resolved, offset, multi_layer_copy);

        insert_and_select_shapes_in_scope(app, pasted_shapes, any_raw);
    });
    true
}

/// 輪郭エッジつまみ（PullEdge）または頂点移動（MoveVertex）ドラッグ終了時の確定・図形分割・ロールバック共通処理
pub(crate) fn finish_deform_shape_drag(
    app: &mut Is2dApp,
    shape_idx: usize,
    drag_dist: f64,
    undo_snapshot: Vec<FillShape>,
    apply_deform: impl FnOnce(&mut FillShape),
) {
    let min_deform = 0.5 / (app.viewport.zoom as f64);
    if drag_dist > min_deform && shape_idx < app.canvas.shapes.len() {
        crate::canvas_tool::focus_active_layer_to_shape(app, shape_idx);
        let mut deformed = app.canvas.shapes.remove(shape_idx);
        apply_deform(&mut deformed);
        let resolved = resolve_deformed_shape(&deformed);
        merge_shapes_into_canvas(&mut app.canvas.shapes, resolved);
        app.canvas.sort_shapes_by_layer_order(&app.timeline);
        app.commit_canvas_mutation(undo_snapshot);
    } else {
        app.canvas.restore_drag_snapshot(undo_snapshot);
    }
}

/// Flash風ドット柄（Polka-Dot）選択オーバーレイを描画
pub(crate) fn draw_flash_selection_overlay(
    app: &mut Is2dApp,
    painter: &egui::Painter,
    shape_idx: usize,
) {
    let zoom = app.viewport.zoom;
    let pan = app.viewport.pan;
    let Some(shape) = app.canvas.shapes.get_mut(shape_idx) else {
        return;
    };
    let sel_screen_mesh = shape.cached_selection_screen_mesh(zoom, pan);
    painter.add(arc_mesh_to_shape(sel_screen_mesh));
    if let Some(shape_ref) = app.canvas.shapes.get(shape_idx) {
        draw_shape_contour_lines(
            app,
            painter,
            shape_ref,
            Stroke::new(1.5_f32, Color32::from_rgb(0, 175, 255)),
            false,
        );
    }
}

pub(crate) fn draw_shape_contour_lines(
    app: &Is2dApp,
    painter: &egui::Painter,
    shape: &FillShape,
    stroke: Stroke,
    show_anchors: bool,
) {
    let z = app.viewport.zoom as f64;
    let pad = 4.0 / z;
    let view_rect = app
        .viewport
        .visible_world_rect(painter, !app.symbols.edit_stack.is_empty())
        .inflate(pad, pad);
    for contour in shape.contours() {
        for seg in contour.segments.iter() {
            if seg.p0.x.max(seg.p1.x).max(seg.p2.x) < view_rect.x0
                || seg.p0.x.min(seg.p1.x).min(seg.p2.x) > view_rect.x1
                || seg.p0.y.max(seg.p1.y).max(seg.p2.y) < view_rect.y0
                || seg.p0.y.min(seg.p1.y).min(seg.p2.y) > view_rect.y1
            {
                continue;
            }
            let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot() * z;
            if mid_dev <= 0.15 {
                painter.line_segment(
                    [
                        app.viewport.world_to_screen(seg.p0),
                        app.viewport.world_to_screen(seg.p2),
                    ],
                    stroke,
                );
            } else {
                let mut prev = seg.p0;
                let approx_len = ((seg.p1 - seg.p0).hypot() + (seg.p2 - seg.p1).hypot()) * z;
                let steps = ((approx_len / 8.0).ceil() as usize).clamp(3, 18);
                for k in 1..=steps {
                    let curr = seg.eval(k as f64 / steps as f64);
                    painter.line_segment(
                        [
                            app.viewport.world_to_screen(prev),
                            app.viewport.world_to_screen(curr),
                        ],
                        stroke,
                    );
                    prev = curr;
                }
            }
            if show_anchors {
                let a = app.viewport.world_to_screen(seg.p0);
                painter.circle_filled(a, 3.0, Color32::WHITE);
                painter.circle_stroke(a, 3.0, Stroke::new(1.2_f32, Color32::from_rgb(30, 120, 230)));
            }
        }
    }
}

pub(crate) fn handle_select_tool_input(
    app: &mut Is2dApp,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    match app.ui.select_sub_mode {
        SelectSubMode::AddAnchor => {
            handle_anchor_edit_input(app, ui, response, mouse_pt, true);
        }
        SelectSubMode::DeleteAnchor => {
            handle_anchor_edit_input(app, ui, response, mouse_pt, false);
        }
        SelectSubMode::Reshape => {
            let shift_held = ui.input(|i| i.modifiers.shift);
            let alt_held = ui.input(|i| i.modifiers.alt);
            if response.double_clicked_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    handle_canvas_double_click(app, pt);
                }
                return;
            }
            if response.drag_started_by(PointerButton::Primary) {
                let press_pt = ui
                    .input(|i| i.pointer.press_origin())
                    .map(|p| app.viewport.screen_to_world(p));
                if let Some(start_pt) = press_pt.or(mouse_pt) {
                    start_reshape_drag(app, start_pt, shift_held, alt_held);
                    if let Some(curr_pt) = mouse_pt {
                        if (curr_pt - start_pt).hypot() > 1e-6 {
                            update_reshape_drag(app, curr_pt);
                        }
                    }
                }
            } else if response.dragged_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    update_reshape_drag(app, pt);
                }
            }
            if response.drag_stopped_by(PointerButton::Primary) {
                finish_reshape_drag(app, mouse_pt);
            } else if response.clicked_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    handle_reshape_click(app, pt, shift_held);
                }
            }

            if !matches!(app.canvas.drag, ActiveCanvasDrag::Select(_)) {
                let zoom = app.viewport.zoom;
                let sub_mode = app.ui.select_sub_mode;
                app.canvas.hover_target = mouse_pt
                    .map(|pt| compute_hover_target(&app.canvas, pt, zoom, sub_mode))
                    .unwrap_or(HoverTarget::None);
            }
        }
    }
}

fn handle_anchor_edit_input(
    app: &mut Is2dApp,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
    is_add: bool,
) {
    let zoom = app.viewport.zoom;
    let sub_mode = app.ui.select_sub_mode;
    let just_pressed = response.hovered() && ui.input(|i| i.pointer.primary_pressed());
    if just_pressed {
        if let Some(pt) = mouse_pt {
            let target = compute_hover_target(&app.canvas, pt, zoom, sub_mode);
            let had_unsliced = app.canvas.has_unsliced_snapshot();
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            let modified_shape = match (is_add, target) {
                (
                    true,
                    HoverTarget::Edge {
                        shape_idx,
                        contour_idx,
                        seg_idx,
                        t,
                        ..
                    },
                ) => app
                    .canvas
                    .shapes
                    .get_mut(shape_idx)
                    .and_then(|s| s.get_contour_mut(contour_idx))
                    .is_some_and(|c| c.insert_anchor_at(seg_idx, t))
                    .then_some(shape_idx),
                (
                    false,
                    HoverTarget::Vertex {
                        shape_idx,
                        contour_idx,
                        seg_idx,
                        ..
                    },
                ) => app
                    .canvas
                    .shapes
                    .get_mut(shape_idx)
                    .and_then(|s| s.get_contour_mut(contour_idx))
                    .is_some_and(|c| c.delete_anchor_at(seg_idx))
                    .then_some(shape_idx),
                _ => None,
            };
            if let Some(s_idx) = modified_shape {
                if let Some(shape) = app.canvas.shapes.get_mut(s_idx) {
                    shape.rebuild_cache();
                }
                app.commit_canvas_mutation(snapshot);
            } else if had_unsliced {
                app.canvas.restore_unsliced_snapshot(snapshot);
            }
        }
    }
    app.canvas.hover_target = mouse_pt
        .map(|pt| compute_hover_target(&app.canvas, pt, zoom, sub_mode))
        .unwrap_or(HoverTarget::None);
}

pub(crate) fn start_reshape_drag(app: &mut Is2dApp, pt: Point, shift_held: bool, alt_held: bool) {
    app.ui.inspect_timeline_frame = false;
    if shift_held {
        app.canvas.drag =
            ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect(MarqueeDrag::new(pt, true)));
        return;
    }
    let mut target =
        compute_hover_target(&app.canvas, pt, app.viewport.zoom, app.ui.select_sub_mode);
    if alt_held {
        if let Some(shape_idx) = target.shape_idx() {
            if !app.canvas.selected_shapes.contains(&shape_idx) {
                select_single_shape_at(app, pt);
            }
            let move_drag = begin_move_selection_drag(app, pt, true);
            app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MoveShape(move_drag));
            return;
        }
    }
    if matches!(
        target,
        HoverTarget::Vertex { .. } | HoverTarget::Edge { .. }
    ) && (!app.canvas.selected_shapes.is_empty() || app.canvas.has_unsliced_snapshot())
    {
        clear_selection(app);
        target = compute_hover_target(&app.canvas, pt, app.viewport.zoom, app.ui.select_sub_mode);
    }
    match target {
        HoverTarget::Vertex {
            shape_idx,
            contour_idx,
            seg_idx,
            pos,
        } => {
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            if let Some((orig_prev_ctrl, orig_curr_ctrl)) = app
                .canvas
                .shapes
                .get(shape_idx)
                .and_then(|s| s.get_contour(contour_idx))
                .and_then(|contour| {
                    let n = contour.segments.len();
                    if n == 0 || seg_idx >= n {
                        return None;
                    }
                    let prev_idx = (seg_idx + n - 1) % n;
                    Some((contour.segments[prev_idx].p1, contour.segments[seg_idx].p1))
                })
            {
                app.canvas.selected_shapes.clear();
                app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
                    shape_idx,
                    contour_idx,
                    seg_idx,
                    orig_vertex: pos,
                    last_pos: pos,
                    orig_prev_ctrl,
                    orig_curr_ctrl,
                    snapped_to: None,
                    undo_snapshot: snapshot,
                });
            }
        }
        HoverTarget::Edge {
            shape_idx,
            contour_idx,
            seg_idx,
            t,
            closest_pos,
        } => {
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            if let Some(orig_seg) = app
                .canvas
                .get_contour_segment(shape_idx, contour_idx, seg_idx)
            {
                let grab_t = t.clamp(0.18, 0.82);
                let grab_pos = orig_seg.eval(grab_t);
                // 実カーソル位置 pt と曲線上の最近接点 closest_pos の差、および丸め前 t と grab_t の位置差を合わせた初期オフセット
                let grab_offset = (closest_pos - pt) + (grab_pos - closest_pos);
                app.canvas.selected_shapes.clear();
                app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::PullEdge {
                    shape_idx,
                    contour_idx,
                    seg_idx,
                    grab_t,
                    grab_pos,
                    grab_offset,
                    last_pos: grab_pos,
                    orig_seg,
                    undo_snapshot: snapshot,
                });
            }
        }
        HoverTarget::Fill { shape_idx } => {
            if !app.canvas.selected_shapes.contains(&shape_idx) {
                select_single_shape_at(app, pt);
            }
            let move_drag = begin_move_selection_drag(app, pt, false);
            app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MoveShape(move_drag));
        }
        HoverTarget::None => {
            app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect(
                MarqueeDrag::new(pt, false),
            ));
        }
    }
}

pub(crate) fn update_reshape_drag(app: &mut Is2dApp, pt: Point) {
    let vertex_drag_info = if let ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
        shape_idx,
        contour_idx,
        seg_idx,
        orig_vertex,
        orig_prev_ctrl,
        orig_curr_ctrl,
        ..
    }) = &app.canvas.drag
    {
        Some((
            *shape_idx,
            *contour_idx,
            *seg_idx,
            *orig_vertex,
            *orig_prev_ctrl,
            *orig_curr_ctrl,
        ))
    } else {
        None
    };

    if let Some((s_idx, c_idx, g_idx, orig_v, prev_c, curr_c)) = vertex_drag_info {
        let snap_opt = find_snap_vertex(&app.canvas, pt, app.viewport.zoom, s_idx, c_idx, g_idx);
        let target_pt = snap_opt.unwrap_or(pt);
        if let ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
            snapped_to,
            last_pos,
            ..
        }) = &mut app.canvas.drag
        {
            *snapped_to = snap_opt;
            *last_pos = target_pt;
        }
        if let Some(shape) = app.canvas.shapes.get_mut(s_idx) {
            set_moved_vertex(shape, (c_idx, g_idx), orig_v, (prev_c, curr_c), target_pt);
            shape.rebuild_cache();
        }
        return;
    }

    let mut move_delta: Option<KVec2> = None;
    if let ActiveCanvasDrag::Select(select_drag) = &mut app.canvas.drag {
        match select_drag {
            SelectDragState::MoveShape(drag) => {
                move_delta = Some(drag.step(pt));
            }
            SelectDragState::MarqueeSelect(m) => {
                m.current_pos = pt;
            }
            SelectDragState::PullEdge {
                shape_idx,
                contour_idx,
                seg_idx,
                grab_t,
                grab_offset,
                last_pos,
                orig_seg,
                ..
            } => {
                let target_pt = pt + *grab_offset;
                *last_pos = target_pt;
                if let Some(shape) = app.canvas.shapes.get_mut(*shape_idx) {
                    set_pulled_edge(
                        shape,
                        (*contour_idx, *seg_idx),
                        *grab_t,
                        *orig_seg,
                        target_pt,
                    );
                    shape.rebuild_cache();
                }
            }
            _ => {}
        }
    }
    if let Some(delta) = move_delta {
        app.canvas.translate_selected(delta);
    }
}

fn finish_reshape_drag(app: &mut Is2dApp, mouse_pt: Option<Point>) {
    let ActiveCanvasDrag::Select(finished_drag) =
        std::mem::replace(&mut app.canvas.drag, ActiveCanvasDrag::None)
    else {
        return;
    };
    match finished_drag {
        SelectDragState::MoveShape(drag) => {
            let min_move = 1.0 / (app.viewport.zoom as f64);
            let rolled_back =
                drag.total_delta.hypot() <= min_move || app.canvas.selected_shapes.is_empty();
            let orig_ids = drag.orig_selected_ids;
            finish_move_selection_drag(app, drag.total_delta, drag.undo_snapshot);
            if rolled_back && !orig_ids.is_empty() {
                app.canvas.restore_selection_by_shape_ids(&orig_ids);
            }
        }
        SelectDragState::MarqueeSelect(m) => {
            let end_pos = mouse_pt.unwrap_or(m.current_pos);
            apply_marquee_selection(app, m.start_pos, end_pos, m.additive);
        }
        SelectDragState::PullEdge {
            shape_idx,
            contour_idx,
            seg_idx,
            grab_t,
            grab_pos,
            last_pos,
            orig_seg,
            undo_snapshot,
            ..
        } => {
            finish_deform_shape_drag(
                app,
                shape_idx,
                (last_pos - grab_pos).hypot(),
                undo_snapshot,
                |deformed| {
                    apply_pull_edge_to_shape(
                        deformed,
                        (contour_idx, seg_idx),
                        grab_t,
                        orig_seg,
                        last_pos,
                    );
                },
            );
        }
        SelectDragState::MoveVertex {
            shape_idx,
            contour_idx,
            seg_idx,
            orig_vertex,
            last_pos,
            orig_prev_ctrl,
            orig_curr_ctrl,
            undo_snapshot,
            ..
        } => {
            finish_deform_shape_drag(
                app,
                shape_idx,
                (last_pos - orig_vertex).hypot(),
                undo_snapshot,
                |deformed| {
                    apply_move_vertex_to_shape(
                        deformed,
                        (contour_idx, seg_idx),
                        orig_vertex,
                        (orig_prev_ctrl, orig_curr_ctrl),
                        last_pos,
                    );
                },
            );
        }
    }
}

fn handle_reshape_click(app: &mut Is2dApp, pt: Point, shift_held: bool) {
    app.ui.inspect_timeline_frame = false;
    let zoom = app.viewport.zoom;
    let sub_mode = app.ui.select_sub_mode;
    let hit_shape = app
        .canvas
        .topmost_shape_at(pt)
        .or_else(|| compute_hover_target(&app.canvas, pt, zoom, sub_mode).shape_idx());
    if shift_held {
        if let Some(shape_idx) = hit_shape {
            toggle_shape_selection(app, shape_idx);
        }
    } else {
        match hit_shape {
            Some(shape_idx) if app.canvas.selected_shapes.contains(&shape_idx) => {
                let unit = app.canvas.expand_to_group_unit(shape_idx);
                crate::canvas_tool::retain_selection_and_flush_deselected(app, &unit);
            }
            Some(_) => {
                select_single_shape_at(app, pt);
            }
            None => {
                clear_selection(app);
            }
        }
    }
}

// ------------------------------------------------------------------------
// 3. 頂点スナップ・選択ツールカーソル描画
// ------------------------------------------------------------------------

/// 頂点ドラッグ時に周囲の他の頂点へ吸着（スナップ）する座標を検索
pub(crate) fn find_snap_vertex(
    canvas: &CanvasState,
    drag_pt: Point,
    zoom: f32,
    ignore_shape: usize,
    ignore_contour: usize,
    ignore_seg: usize,
) -> Option<Point> {
    let snap_radius = 12.0 / (zoom as f64);
    let mut best_snap: Option<(f64, Point)> = None;

    for (s_idx, shape) in canvas.shapes.iter().enumerate() {
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !shape.is_raw_at_scope(&canvas.editing_group_path)
            || !bbox.inflate(snap_radius, snap_radius).contains(drag_pt)
        {
            continue;
        }
        for (c_idx, contour) in shape.contours().enumerate() {
            for (seg_idx, seg) in contour.segments.iter().enumerate() {
                if s_idx == ignore_shape && c_idx == ignore_contour && seg_idx == ignore_seg {
                    continue;
                }
                let dist = (drag_pt - seg.p0).hypot();
                if dist <= snap_radius && best_snap.as_ref().is_none_or(|(d, _)| dist < *d) {
                    best_snap = Some((dist, seg.p0));
                }
            }
        }
    }
    best_snap.map(|(_, pt)| pt)
}

pub(crate) fn draw_select_tool_cursor(
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match &app.canvas.drag {
        ActiveCanvasDrag::Select(SelectDragState::PullEdge {
            shape_idx,
            contour_idx,
            seg_idx,
            ..
        }) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            if app.ui.show_wireframe {
                if let Some(seg) =
                    app.canvas
                        .get_contour_segment(*shape_idx, *contour_idx, *seg_idx)
                {
                    draw_highlighted_quad_segment(app, painter, seg);
                }
            }
            draw_flash_curve_cursor_badge(painter, pos);
        }
        ActiveCanvasDrag::Select(SelectDragState::MoveVertex { snapped_to, .. }) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            if let Some(snap_pt) = snapped_to {
                let sp = app.viewport.world_to_screen(*snap_pt);
                painter.circle_stroke(sp, 9.0, Stroke::new(2.5_f32, Color32::from_rgb(255, 50, 140)));
                painter.circle_filled(sp, 4.0, Color32::from_rgb(255, 50, 140));
            }
            draw_flash_corner_cursor_badge(painter, pos);
        }
        ActiveCanvasDrag::Select(SelectDragState::MoveShape { .. }) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        }
        ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect { .. }) => {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
        }
        _ => draw_select_sub_mode_cursor(app, ctx, painter, pos),
    }
}

fn draw_select_sub_mode_cursor(
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match app.ui.select_sub_mode {
        SelectSubMode::AddAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, true);
            if let HoverTarget::Edge {
                shape_idx,
                contour_idx,
                seg_idx,
                closest_pos,
                ..
            } = app.canvas.hover_target
            {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                if let Some(seg) = app
                    .canvas
                    .get_contour_segment(shape_idx, contour_idx, seg_idx)
                {
                    draw_highlighted_quad_segment(app, painter, seg);
                }
                let cp = app.viewport.world_to_screen(closest_pos);
                painter.circle_filled(cp, 5.5, Color32::from_rgb(30, 200, 95));
                painter.circle_stroke(cp, 5.5, Stroke::new(2.0_f32, Color32::WHITE));
            } else {
                ctx.set_cursor_icon(CursorIcon::Default);
            }
        }
        SelectSubMode::DeleteAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, false);
            if let HoverTarget::Vertex { pos: v_pos, .. } = app.canvas.hover_target {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                let vp = app.viewport.world_to_screen(v_pos);
                painter.circle_filled(vp, 6.0, Color32::from_rgb(235, 55, 55));
                painter.circle_stroke(vp, 6.0, Stroke::new(2.0_f32, Color32::WHITE));
            } else {
                ctx.set_cursor_icon(CursorIcon::Default);
            }
        }
        SelectSubMode::Reshape => draw_reshape_hover_cursor(app, ctx, painter, pos),
    }
}

fn draw_reshape_hover_cursor(
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match app.canvas.hover_target {
        HoverTarget::Vertex {
            pos: v_pos,
            shape_idx,
            contour_idx,
            ..
        } => {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            if app.ui.show_wireframe {
                if let Some(contour) = app
                    .canvas
                    .shapes
                    .get(shape_idx)
                    .and_then(|s| s.get_contour(contour_idx))
                {
                    for seg in contour.segments.iter() {
                        let p = app.viewport.world_to_screen(seg.p0);
                        painter.circle_filled(p, 3.2, Color32::WHITE);
                        painter.circle_stroke(
                            p,
                            3.2,
                            Stroke::new(1.2_f32, Color32::from_rgb(30, 130, 240)),
                        );
                    }
                }
                let vp = app.viewport.world_to_screen(v_pos);
                painter.circle_filled(vp, 5.5, Color32::from_rgb(255, 120, 40));
                painter.circle_stroke(vp, 5.5, Stroke::new(2.0_f32, Color32::WHITE));
            }
            draw_flash_corner_cursor_badge(painter, pos);
        }
        HoverTarget::Edge {
            shape_idx,
            contour_idx,
            seg_idx,
            closest_pos,
            ..
        } => {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            if app.ui.show_wireframe {
                if let Some(seg) = app
                    .canvas
                    .get_contour_segment(shape_idx, contour_idx, seg_idx)
                {
                    draw_highlighted_quad_segment(app, painter, seg);
                }
                painter.circle_filled(
                    app.viewport.world_to_screen(closest_pos),
                    4.0,
                    Color32::from_rgb(0, 175, 255),
                );
            }
            draw_flash_curve_cursor_badge(painter, pos);
        }
        HoverTarget::Fill { .. } => {
            ctx.set_cursor_icon(CursorIcon::Move);
        }
        HoverTarget::None => {
            ctx.set_cursor_icon(CursorIcon::Default);
        }
    }
}

/// ホバー・ドラッグ中の2次ベジェ曲線セグメントとその両端アンカーを強調描画（ハンドルは表示しない）
pub(crate) fn draw_highlighted_quad_segment(app: &Is2dApp, painter: &egui::Painter, seg: QuadBez) {
    let steps = 16;
    let mut prev = seg.p0;
    for k in 1..=steps {
        let curr = seg.eval(k as f64 / steps as f64);
        painter.line_segment(
            [
                app.viewport.world_to_screen(prev),
                app.viewport.world_to_screen(curr),
            ],
            Stroke::new(3.0_f32, Color32::from_rgb(0, 175, 255)),
        );
        prev = curr;
    }
    for end_pt in [seg.p0, seg.p2] {
        let p = app.viewport.world_to_screen(end_pt);
        painter.circle_filled(p, 4.0, Color32::WHITE);
        painter.circle_stroke(p, 4.0, Stroke::new(1.8_f32, Color32::from_rgb(0, 140, 230)));
    }
}

/// Flash の選択ツールでエッジに近づいた際の「曲線アーク」カーソルバッジを描画
fn draw_flash_curve_cursor_badge(painter: &egui::Painter, cursor: Pos2) {
    let base = cursor + Vec2::new(14.0, 14.0);
    let arc = QuadBez::new(
        Point::new(base.x as f64 - 6.0, base.y as f64 + 4.0),
        Point::new(base.x as f64, base.y as f64 - 8.0),
        Point::new(base.x as f64 + 8.0, base.y as f64 + 4.0),
    );
    let mut prev = arc.p0;
    for k in 1..=10 {
        let curr = arc.eval(k as f64 / 10.0);
        painter.line_segment(
            [to_pos2(prev), to_pos2(curr)],
            Stroke::new(2.2_f32, Color32::from_rgb(20, 20, 25)),
        );
        prev = curr;
    }
}

/// Flash の選択ツールで頂点（アンカー）に近づいた際の「角（コーナー）」カーソルバッジを描画
fn draw_flash_corner_cursor_badge(painter: &egui::Painter, cursor: Pos2) {
    let base = cursor + Vec2::new(14.0, 14.0);
    let p0 = Pos2::new(base.x - 6.0, base.y + 5.0);
    let p1 = Pos2::new(base.x + 1.0, base.y - 5.0);
    let p2 = Pos2::new(base.x + 8.0, base.y + 5.0);
    let stroke = Stroke::new(2.2_f32, Color32::from_rgb(20, 20, 25));
    painter.line_segment([p0, p1], stroke);
    painter.line_segment([p1, p2], stroke);
}

/// アンカー追加（`is_plus = true`: 緑「＋」）およびアンカー削除（`is_plus = false`: 赤「－」）のカーソルバッジを描画
fn draw_anchor_edit_cursor_badge(painter: &egui::Painter, cursor: Pos2, is_plus: bool) {
    let base = cursor + Vec2::new(15.0, 15.0);
    let fill = if is_plus {
        Color32::from_rgb(30, 190, 90)
    } else {
        Color32::from_rgb(230, 55, 55)
    };
    painter.circle_filled(base, 8.0, fill);
    painter.circle_stroke(base, 8.0, Stroke::new(1.5_f32, Color32::WHITE));
    let stroke = Stroke::new(2.2_f32, Color32::WHITE);
    painter.line_segment(
        [
            Pos2::new(base.x - 4.5, base.y),
            Pos2::new(base.x + 4.5, base.y),
        ],
        stroke,
    );
    if is_plus {
        painter.line_segment(
            [
                Pos2::new(base.x, base.y - 4.5),
                Pos2::new(base.x, base.y + 4.5),
            ],
            stroke,
        );
    }
}
