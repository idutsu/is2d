use std::sync::Arc;

use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Stroke, Vec2};
use kurbo::{ParamCurve, Point, QuadBez};

use crate::canvas_tool::{
    ActiveCanvasDrag, HoverTarget, LayerId, MapOwner, MarqueeDrag, SelectDragState, SelectSubMode,
    Tab,
};
use crate::clipboard_ops::begin_move_selection_drag;
use crate::doc_actions::{apply, Action, ActionHost};
use crate::planar_map::{reshape_result, scope_maps, LayerBase, MapEdit};
use crate::select_ops::{apply_click_selection, finish_selection_drag, update_selection_drag};
pub(crate) use crate::select_ops::{compute_hover_target, handle_canvas_double_click};
use crate::vector_engine::FillShape;

// ============================================================================
// 選択ツール（Tool::Select / V キー）・輪郭変形・アンカー編集・グループ・クリップボード
// ============================================================================

/// Flash風ドット柄（Polka-Dot）選択オーバーレイを描画
pub(crate) fn draw_flash_selection_overlay(tab: &Tab, painter: &egui::Painter, scene_idx: usize) {
    let zoom = tab.view.zoom;
    let pan = tab.view.pan;
    let Some(shape) = tab.scene().get(scene_idx) else {
        return;
    };
    painter.add(egui::Shape::Mesh(shape.selection_screen_mesh(zoom, pan)));
}

pub(crate) fn draw_shape_contour_lines(
    tab: &Tab,
    painter: &egui::Painter,
    shape: &FillShape,
    stroke: Stroke,
    show_anchors: bool,
) {
    let z = tab.view.zoom as f64;
    let pad = 4.0 / z;
    let view_rect = tab
        .view
        .visible_world_rect(
            painter,
            tab.doc.stage_size,
            !tab.session.edit_path.is_empty(),
        )
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
                        tab.view.world_to_screen(seg.p0),
                        tab.view.world_to_screen(seg.p2),
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
                            tab.view.world_to_screen(prev),
                            tab.view.world_to_screen(curr),
                        ],
                        stroke,
                    );
                    prev = curr;
                }
            }
            if show_anchors {
                let a = tab.view.world_to_screen(seg.p0);
                painter.circle_filled(a, 3.0, Color32::WHITE);
                painter.circle_stroke(
                    a,
                    3.0,
                    Stroke::new(1.2_f32, Color32::from_rgb(30, 120, 230)),
                );
            }
        }
    }
}

pub(crate) fn handle_select_tool_input(
    host: &mut impl ActionHost,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    let sub_mode = host.tool().select_sub_mode;
    match sub_mode {
        SelectSubMode::AddAnchor => {
            handle_anchor_edit_input(host, ui, response, mouse_pt, true);
        }
        SelectSubMode::DeleteAnchor => {
            handle_anchor_edit_input(host, ui, response, mouse_pt, false);
        }
        SelectSubMode::Reshape => {
            let shift_held = ui.input(|i| i.modifiers.shift);
            let alt_held = ui.input(|i| i.modifiers.alt);
            if response.double_clicked_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    handle_canvas_double_click(host, pt);
                }
                return;
            }
            if response.drag_started_by(PointerButton::Primary) {
                if let Some(start_pt) = host.tab().view.press_origin_world(ui.ctx()).or(mouse_pt) {
                    start_reshape_drag(host, start_pt, shift_held, alt_held);
                    if let Some(curr_pt) = mouse_pt {
                        if crate::canvas_tool::View::moved_since_press(start_pt, curr_pt) {
                            update_reshape_drag(host.tab_mut(), curr_pt);
                        }
                    }
                }
            } else if response.dragged_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    update_reshape_drag(host.tab_mut(), pt);
                }
            }
            if response.drag_stopped_by(PointerButton::Primary) {
                finish_reshape_drag(host, mouse_pt);
            } else if response.clicked_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    host.tab_mut().view.inspect_timeline_frame = false;
                    apply_click_selection(host, pt, shift_held, true);
                }
            }
        }
    }
    // アンカーの追加・削除のモードではドラッグを始めないので、どのモードでも同じ条件でよい
    let tab = host.tab_mut();
    if matches!(tab.session.drag, ActiveCanvasDrag::None) {
        let zoom = tab.view.zoom;
        let hover = mouse_pt
            .map(|pt| compute_hover_target(tab.scene(), &tab.session, pt, zoom, sub_mode))
            .unwrap_or(HoverTarget::None);
        tab.session.hover_target = hover;
    }
}

fn handle_anchor_edit_input(
    host: &mut impl ActionHost,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
    is_add: bool,
) {
    let just_pressed = response.hovered() && ui.input(|i| i.pointer.primary_pressed());
    if just_pressed {
        if let Some(pt) = mouse_pt {
            let action = if is_add {
                Action::AddAnchor { pt }
            } else {
                Action::DeleteAnchor { pt }
            };
            apply(host, action);
        }
    }
}

/// 地図での変形のドラッグを始める：レイヤー `layer_id` の地図から、ドラッグのあいだ使う元の地図と階層を作り、選択と仮表示を空にする
fn begin_map_drag(tab: &mut Tab, layer_id: LayerId) -> Option<(Arc<LayerBase>, Vec<u64>)> {
    let maps = scope_maps(tab.scene(), &tab.session);
    let lm = maps.maps.iter().find(|m| m.layer_id == layer_id)?;
    let (base, group_path) = (lm.base.clone(), lm.group_path.clone());
    tab.session.selection.clear();
    tab.session
        .set_reshape_overlay(&[], Vec::new(), layer_id, &group_path);
    Some((base, group_path))
}

pub(crate) fn start_reshape_drag(
    host: &mut impl ActionHost,
    pt: Point,
    shift_held: bool,
    alt_held: bool,
) {
    let sub_mode = host.tool().select_sub_mode;
    host.tab_mut().view.inspect_timeline_frame = false;
    if shift_held {
        host.tab_mut().session.drag = ActiveCanvasDrag::Marquee(MarqueeDrag::new(pt, true));
        return;
    }
    let mut target = compute_hover_target(
        host.tab().scene(),
        &host.tab().session,
        pt,
        host.tab().view.zoom,
        sub_mode,
    );
    if alt_held {
        if let Some(shape_id) = target.shape_id() {
            if !host.tab().session.is_shape_id_selected(shape_id) {
                apply(host, Action::SelectSingleAt { pt });
            }
            let tab = host.tab_mut();
            let move_drag = begin_move_selection_drag(tab, pt, true);
            tab.session.drag = ActiveCanvasDrag::MoveSelection(move_drag);
            return;
        }
    }
    if matches!(
        target,
        HoverTarget::Vertex { .. } | HoverTarget::Edge { .. }
    ) && (!host.tab().session.selection.is_empty() || host.tab().session.floating().is_some())
    {
        apply(host, Action::Deselect);
        target = compute_hover_target(
            host.tab().scene(),
            &host.tab().session,
            pt,
            host.tab().view.zoom,
            sub_mode,
        );
    }
    match target {
        HoverTarget::Vertex {
            map_node: Some((MapOwner::Layer(layer_id), node, orig_vertex)),
            ..
        } => {
            let tab = host.tab_mut();
            let Some((base, group_path)) = begin_map_drag(tab, layer_id) else {
                return;
            };
            tab.session.drag = ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
                layer_id,
                group_path,
                base,
                node,
                orig_vertex,
                target: orig_vertex,
                snapped_to: None,
            });
        }
        HoverTarget::Edge {
            t,
            seg,
            map_edge: Some((MapOwner::Layer(layer_id), orig_seg)),
            ..
        } => {
            let tab = host.tab_mut();
            let Some((base, group_path)) = begin_map_drag(tab, layer_id) else {
                return;
            };
            let grab_t = t.clamp(0.18, 0.82);
            // マウス pt から、つかむ所（丸めた grab_t の所）までのずれ（ステージの座標）
            let grab_offset = seg.eval(grab_t) - pt;
            tab.session.drag = ActiveCanvasDrag::Select(SelectDragState::PullEdge {
                layer_id,
                group_path,
                base,
                grab_t,
                grab_offset,
                target: orig_seg.eval(grab_t),
                orig_seg,
            });
        }
        HoverTarget::Vertex { .. } | HoverTarget::Edge { .. } => {}
        HoverTarget::Fill { shape_id } => {
            if !host.tab().session.is_shape_id_selected(shape_id) {
                apply(host, Action::SelectSingleAt { pt });
            }
            let tab = host.tab_mut();
            let move_drag = begin_move_selection_drag(tab, pt, false);
            tab.session.drag = ActiveCanvasDrag::MoveSelection(move_drag);
        }
        HoverTarget::None => {
            host.tab_mut().session.drag = ActiveCanvasDrag::Marquee(MarqueeDrag::new(pt, false));
        }
    }
}

pub(crate) fn update_reshape_drag(tab: &mut Tab, pt: Point) {
    if update_selection_drag(tab, pt) {
        return;
    }
    let snap_radius = 12.0 / (tab.view.zoom as f64);
    // 地図は文書の座標。マウスの位置は、変換の逆で文書の座標へ写す（吸い付く距離はステージで測る）
    let xform = tab.session.edit_path.world_xform();
    // ドラッグ中も、離したときと同じ計算（交われば辺を切って色を決め直す）をして、その結果を見せる
    let update = match &mut tab.session.drag {
        ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
            layer_id,
            group_path,
            base,
            node,
            orig_vertex,
            target,
            snapped_to,
        }) => {
            let snap = base.map.snap_target(*node, pt, snap_radius, xform);
            *target = snap.unwrap_or_else(|| xform.inverse() * pt);
            *snapped_to = snap.map(|p| xform * p);
            Some((
                base.clone(),
                MapEdit::MoveVertex {
                    orig_vertex: *orig_vertex,
                    target: *target,
                },
                *layer_id,
                group_path.clone(),
            ))
        }
        ActiveCanvasDrag::Select(SelectDragState::PullEdge {
            layer_id,
            group_path,
            base,
            grab_t,
            grab_offset,
            target,
            orig_seg,
        }) => {
            *target = xform.inverse() * (pt + *grab_offset);
            let target = *target;
            Some((
                base.clone(),
                MapEdit::PullEdge {
                    orig_seg: *orig_seg,
                    grab_t: *grab_t,
                    target,
                },
                *layer_id,
                group_path.clone(),
            ))
        }
        _ => None,
    };
    let Some((base, edit, layer_id, group_path)) = update else {
        return;
    };
    let faces = base.faces(&group_path);
    let own: Vec<&FillShape> = faces.iter().map(|s| &**s).collect();
    let (removed, added) =
        reshape_result(&base, edit, &own, layer_id, &group_path, xform).unwrap_or_default();
    tab.session
        .set_reshape_overlay(&removed, added, layer_id, &group_path);
}

pub(crate) fn finish_reshape_drag(host: &mut impl ActionHost, mouse_pt: Option<Point>) {
    if finish_selection_drag(host, mouse_pt) {
        return;
    }
    let ActiveCanvasDrag::Select(finished_drag) =
        std::mem::replace(&mut host.tab_mut().session.drag, ActiveCanvasDrag::None)
    else {
        return;
    };
    match finished_drag {
        SelectDragState::PullEdge {
            layer_id,
            grab_t,
            target,
            orig_seg,
            ..
        } => {
            apply(
                host,
                Action::ReshapeEdge {
                    layer_id,
                    grab_t,
                    target_pos: target,
                    orig_seg,
                },
            );
        }
        SelectDragState::MoveVertex {
            layer_id,
            orig_vertex,
            target,
            ..
        } => {
            apply(
                host,
                Action::MoveVertex {
                    layer_id,
                    orig_vertex,
                    target_pos: target,
                },
            );
        }
    }
}

// ------------------------------------------------------------------------
// 3. 選択ツールカーソル描画
// ------------------------------------------------------------------------

pub(crate) fn draw_select_tool_cursor(
    tab: &Tab,
    select_sub_mode: SelectSubMode,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    ctx.set_cursor_icon(CursorIcon::Default);
    match &tab.session.drag {
        ActiveCanvasDrag::Select(SelectDragState::PullEdge {
            orig_seg,
            grab_t,
            target,
            ..
        }) => {
            if tab.view.show_wireframe {
                // 引っぱっている辺の今の形（`PlanarMap::commit_pull_edge` と同じ式）を、ステージに置いて見せる
                let mut seg = *orig_seg;
                seg.p1 += (*target - orig_seg.eval(*grab_t)) / (2.0 * grab_t * (1.0 - grab_t));
                draw_highlighted_quad_segment(
                    tab,
                    painter,
                    tab.session.edit_path.world_xform() * seg,
                );
            }
        }
        ActiveCanvasDrag::Select(SelectDragState::MoveVertex { snapped_to, .. }) => {
            if let Some(snap_pt) = snapped_to {
                let sp = tab.view.world_to_screen(*snap_pt);
                painter.circle_stroke(
                    sp,
                    9.0,
                    Stroke::new(2.5_f32, Color32::from_rgb(255, 50, 140)),
                );
                painter.circle_filled(sp, 4.0, Color32::from_rgb(255, 50, 140));
            }
        }
        ActiveCanvasDrag::MoveSelection(_) | ActiveCanvasDrag::Marquee(_) => {}
        _ => draw_select_sub_mode_cursor(tab, select_sub_mode, painter, pos),
    }
}

fn draw_select_sub_mode_cursor(
    tab: &Tab,
    select_sub_mode: SelectSubMode,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match select_sub_mode {
        SelectSubMode::AddAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, true);
            if let HoverTarget::Edge {
                seg, closest_pos, ..
            } = tab.session.hover_target
            {
                draw_highlighted_quad_segment(tab, painter, seg);
                let cp = tab.view.world_to_screen(closest_pos);
                painter.circle_filled(cp, 5.5, Color32::from_rgb(30, 200, 95));
                painter.circle_stroke(cp, 5.5, Stroke::new(2.0_f32, Color32::WHITE));
            }
        }
        SelectSubMode::DeleteAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, false);
            if let HoverTarget::Vertex { pos: v_pos, .. } = tab.session.hover_target {
                let vp = tab.view.world_to_screen(v_pos);
                painter.circle_filled(vp, 6.0, Color32::from_rgb(235, 55, 55));
                painter.circle_stroke(vp, 6.0, Stroke::new(2.0_f32, Color32::WHITE));
            }
        }
        SelectSubMode::Reshape => draw_reshape_hover_cursor(tab, painter),
    }
}

fn draw_reshape_hover_cursor(tab: &Tab, painter: &egui::Painter) {
    if !tab.view.show_wireframe {
        return;
    }
    match tab.session.hover_target {
        HoverTarget::Vertex {
            pos: v_pos,
            shape_id,
            contour_idx,
            ..
        } => {
            if let Some(contour) = tab
                .scene()
                .shape_by_id(shape_id)
                .and_then(|s| s.get_contour(contour_idx))
            {
                for seg in contour.segments.iter() {
                    let p = tab.view.world_to_screen(seg.p0);
                    painter.circle_filled(p, 3.2, Color32::WHITE);
                    painter.circle_stroke(
                        p,
                        3.2,
                        Stroke::new(1.2_f32, Color32::from_rgb(30, 130, 240)),
                    );
                }
            }
            let vp = tab.view.world_to_screen(v_pos);
            painter.circle_filled(vp, 5.5, Color32::from_rgb(255, 120, 40));
            painter.circle_stroke(vp, 5.5, Stroke::new(2.0_f32, Color32::WHITE));
        }
        HoverTarget::Edge {
            seg, closest_pos, ..
        } => {
            draw_highlighted_quad_segment(tab, painter, seg);
            painter.circle_filled(
                tab.view.world_to_screen(closest_pos),
                4.0,
                Color32::from_rgb(0, 175, 255),
            );
        }
        HoverTarget::Fill { .. } | HoverTarget::None => {}
    }
}

/// ホバー・ドラッグ中の2次ベジェ曲線セグメントとその両端アンカーを強調描画（ハンドルは表示しない）
pub(crate) fn draw_highlighted_quad_segment(tab: &Tab, painter: &egui::Painter, seg: QuadBez) {
    let steps = 16;
    let mut prev = seg.p0;
    for k in 1..=steps {
        let curr = seg.eval(k as f64 / steps as f64);
        painter.line_segment(
            [
                tab.view.world_to_screen(prev),
                tab.view.world_to_screen(curr),
            ],
            Stroke::new(3.0_f32, Color32::from_rgb(0, 175, 255)),
        );
        prev = curr;
    }
    for end_pt in [seg.p0, seg.p2] {
        let p = tab.view.world_to_screen(end_pt);
        painter.circle_filled(p, 4.0, Color32::WHITE);
        painter.circle_stroke(p, 4.0, Stroke::new(1.8_f32, Color32::from_rgb(0, 140, 230)));
    }
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
