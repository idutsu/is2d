use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Stroke, Vec2};
use kurbo::{ParamCurve, Point, QuadBez, Vec2 as KVec2};

pub(crate) use crate::canvas_tool::clear_selection;
use crate::canvas_tool::{
    ActiveCanvasDrag, CanvasState, EditorDoc, HoverTarget, MarqueeDrag, SelectDragState,
    SelectSubMode,
};
use crate::clipboard_ops::begin_move_selection_drag;
#[cfg(test)]
pub(crate) use crate::clipboard_ops::duplicate_selected_shapes_for_alt_drag;
use crate::doc_actions::{Action, apply};
#[cfg(test)]
pub(crate) use crate::doc_actions::{
    clear_active_frame_shapes, commit_moved_selection, copy_selected_shapes, cut_selected_shapes,
    delete_selected_shapes, finish_move_selection_drag, group_selected_shapes,
    nudge_selected_shapes, paste_clipboard, ungroup_selected_shapes,
};
use crate::select_ops::retain_selection_and_flush_deselected;
#[cfg(not(test))]
use crate::select_ops::{compute_hover_target, handle_canvas_double_click};
pub(crate) use crate::select_ops::{
    select_single_shape_at, set_moved_vertex, set_pulled_edge, toggle_shape_selection,
};
#[cfg(test)]
pub(crate) use crate::select_ops::{
    apply_marquee_selection, apply_pull_edge_to_shape, compute_hover_target, enter_group_at,
    exit_current_group, handle_canvas_double_click,
};
use crate::vector_engine::{FillShape, arc_mesh_to_shape, to_pos2};

// ============================================================================
// 選択ツール（Tool::Select / V キー）・輪郭変形・アンカー編集・グループ・クリップボード
// ============================================================================

/// Flash風ドット柄（Polka-Dot）選択オーバーレイを描画
pub(crate) fn draw_flash_selection_overlay(
    app: &EditorDoc,
    painter: &egui::Painter,
    shape_idx: usize,
) {
    let zoom = app.viewport.zoom;
    let pan = app.viewport.pan;
    let Some(shape) = app.canvas.shapes.get(shape_idx) else {
        return;
    };
    let sel_screen_mesh = shape.cached_selection_screen_mesh(zoom, pan);
    painter.add(arc_mesh_to_shape(sel_screen_mesh));
    draw_shape_contour_lines(
        app,
        painter,
        shape,
        Stroke::new(1.5_f32, Color32::from_rgb(0, 175, 255)),
        false,
    );
}

pub(crate) fn draw_shape_contour_lines(
    app: &EditorDoc,
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
    app: &mut EditorDoc,
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
    app: &mut EditorDoc,
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
            let action = if is_add {
                Action::AddAnchor { pt }
            } else {
                Action::DeleteAnchor { pt }
            };
            apply(app, action);
        }
    }
    app.canvas.hover_target = mouse_pt
        .map(|pt| compute_hover_target(&app.canvas, pt, zoom, sub_mode))
        .unwrap_or(HoverTarget::None);
}

pub(crate) fn start_reshape_drag(app: &mut EditorDoc, pt: Point, shift_held: bool, alt_held: bool) {
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

pub(crate) fn update_reshape_drag(app: &mut EditorDoc, pt: Point) {
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

fn finish_reshape_drag(app: &mut EditorDoc, mouse_pt: Option<Point>) {
    let ActiveCanvasDrag::Select(finished_drag) =
        std::mem::replace(&mut app.canvas.drag, ActiveCanvasDrag::None)
    else {
        return;
    };
    match finished_drag {
        SelectDragState::MoveShape(drag) => {
            apply(
                app,
                Action::FinishMoveSelectionDrag {
                    total_delta: drag.total_delta,
                    undo_snapshot: drag.undo_snapshot,
                    orig_selected_ids: drag.orig_selected_ids,
                },
            );
        }
        SelectDragState::MarqueeSelect(m) => {
            let end_pos = mouse_pt.unwrap_or(m.current_pos);
            apply(
                app,
                Action::SelectMarquee {
                    p0: m.start_pos,
                    p1: end_pos,
                    additive: m.additive,
                },
            );
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
            apply(
                app,
                Action::ReshapeEdge {
                    shape_idx,
                    contour_idx,
                    seg_idx,
                    grab_t,
                    grab_pos,
                    target_pos: last_pos,
                    orig_seg,
                    undo_snapshot,
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
            apply(
                app,
                Action::MoveVertex {
                    shape_idx,
                    contour_idx,
                    seg_idx,
                    orig_vertex,
                    target_pos: last_pos,
                    orig_ctrls: (orig_prev_ctrl, orig_curr_ctrl),
                    undo_snapshot,
                },
            );
        }
    }
}

fn handle_reshape_click(app: &mut EditorDoc, pt: Point, shift_held: bool) {
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
                retain_selection_and_flush_deselected(app, &unit);
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
    app: &EditorDoc,
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
    app: &EditorDoc,
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
    app: &EditorDoc,
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
pub(crate) fn draw_highlighted_quad_segment(app: &EditorDoc, painter: &egui::Painter, seg: QuadBez) {
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
