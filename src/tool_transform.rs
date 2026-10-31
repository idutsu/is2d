use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Stroke, Vec2};
use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{ActiveCanvasDrag, MarqueeDrag, Session, Tab, View, clear_selection};
pub(crate) use crate::canvas_tool::{ScaleHandle, TransformDragState, TransformHoverTarget};
use crate::clipboard_ops::begin_move_selection_drag;
use crate::doc_actions::{
    Action, ActionHost, apply, flush_transform_pivot_to_symbol,
    update_selected_symbol_world_pivot_live,
};
#[cfg(test)]
pub(crate) use crate::doc_actions::{
    convert_selection_to_symbol, delete_symbol_from_library, flip_selected_shapes,
    place_symbol_instance_from_library, rename_symbol_in_library, rotate_selected_shapes,
    scale_selected_shapes, set_active_keyframe_tween_rotate, set_selected_symbol_playback,
    set_selected_symbol_world_pivot, timeline_toggle_tween,
};
use crate::select_ops::{
    compute_hover_target, handle_canvas_double_click, retain_selection_and_flush_deselected,
    select_single_shape_at, toggle_shape_selection,
};
#[cfg(test)]
pub(crate) use crate::symbol_ops::{
    enter_symbol_from_library, enter_symbol_timeline, exit_symbol_timeline,
    selected_single_symbol_info,
};
use crate::vector_engine::{affine_rotate_around, affine_scale_around};

// ============================================================================
// 自由変形ツール（Tool::Transform / Q キー）の型定義・操作・アフィン変換・変形枠描画
// ============================================================================

/// 自由変形ツール（`Tool::Transform` / `Q`）のマウス入力処理
pub(crate) fn handle_transform_tool_input(
    host: &mut impl ActionHost,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    let shift_held = ui.input(|i| i.modifiers.shift);
    let alt_held = ui.input(|i| i.modifiers.alt);
    let zoom = host.tab().view.zoom;

    if response.double_clicked_by(PointerButton::Primary) {
        if let Some(pt) = mouse_pt {
            if compute_transform_hover(&host.tab().session, pt, zoom)
                == TransformHoverTarget::Pivot
            {
                apply(host, Action::ResetTransformPivot);
            } else {
                handle_canvas_double_click(host.tab_mut(), pt);
            }
        }
        return;
    }

    if response.drag_started_by(PointerButton::Primary) {
        let press_pt = ui
            .input(|i| i.pointer.press_origin())
            .map(|p| host.tab().view.screen_to_world(p));
        if let Some(start_pt) = press_pt.or(mouse_pt) {
            start_transform_drag(host, start_pt, shift_held, alt_held);
            if let Some(curr_pt) = mouse_pt {
                if (curr_pt - start_pt).hypot() > 1e-6 {
                    update_transform_drag(host.tab_mut(), curr_pt, shift_held, alt_held);
                }
            }
        }
    } else if response.dragged_by(PointerButton::Primary) {
        if let Some(pt) = mouse_pt {
            update_transform_drag(host.tab_mut(), pt, shift_held, alt_held);
        }
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        finish_transform_drag(host, mouse_pt);
    } else if response.clicked_by(PointerButton::Primary) {
        if let Some(pt) = mouse_pt {
            handle_transform_click(host, pt, shift_held);
        }
    }

    let tab = host.tab_mut();
    if !matches!(tab.session.drag, ActiveCanvasDrag::Transform(_)) {
        tab.session.transform_hover = mouse_pt
            .map(|pt| compute_transform_hover(&tab.session, pt, tab.view.zoom))
            .unwrap_or(TransformHoverTarget::None);
    }
}

pub(crate) fn start_transform_drag(
    host: &mut impl ActionHost,
    pt: Point,
    shift_held: bool,
    alt_held: bool,
) {
    let sub_mode = host.tool().select_sub_mode;
    let tab = host.tab_mut();
    tab.view.inspect_timeline_frame = false;
    let zoom = tab.view.zoom;
    let hover = compute_transform_hover(&tab.session, pt, zoom);
    if shift_held && hover == TransformHoverTarget::None {
        tab.session.drag = ActiveCanvasDrag::Transform(TransformDragState::MarqueeSelect(
            MarqueeDrag::new(pt, true),
        ));
        return;
    }

    match hover {
        TransformHoverTarget::Pivot => {
            if tab.session.selected_single_symbol_binding().is_some()
                && !tab.session.selection_modified
            {
                tab.session.restore_unsliced_preserving_selection();
            }
            if let Some(bbox) = tab.session.selection_bounding_box(zoom) {
                let orig_pivot = tab.session.active_transform_pivot(bbox);
                tab.session.drag = ActiveCanvasDrag::Transform(TransformDragState::MovePivot {
                    orig_pivot,
                    undo_snapshot: tab.session.snapshot_for_drag(),
                    changed: false,
                });
            }
        }
        TransformHoverTarget::Scale(handle) => {
            if let Some(orig_bbox) = tab.session.selection_bounding_box(zoom) {
                let undo_snapshot = tab.session.snapshot_for_drag();
                let orig_pivot = tab.session.active_transform_pivot(orig_bbox);
                flush_transform_pivot_to_symbol(tab);
                let anchor = if alt_held {
                    orig_pivot
                } else {
                    handle.opposite_position(orig_bbox)
                };
                tab.session.drag = ActiveCanvasDrag::Transform(TransformDragState::Scale {
                    handle,
                    anchor,
                    handle_start: handle.position(orig_bbox),
                    orig_pivot,
                    orig_shapes: tab.session.selected_shapes_snapshot(),
                    undo_snapshot,
                    changed: false,
                });
            }
        }
        TransformHoverTarget::Rotate => {
            if let Some(orig_bbox) = tab.session.selection_bounding_box(zoom) {
                let undo_snapshot = tab.session.snapshot_for_drag();
                let pivot = tab.session.active_transform_pivot(orig_bbox);
                flush_transform_pivot_to_symbol(tab);
                tab.session.drag = ActiveCanvasDrag::Transform(TransformDragState::Rotate {
                    orig_bbox,
                    pivot,
                    start_angle: (pt.y - pivot.y).atan2(pt.x - pivot.x),
                    current_angle: 0.0,
                    orig_shapes: tab.session.selected_shapes_snapshot(),
                    undo_snapshot,
                });
            }
        }
        TransformHoverTarget::Move => {
            let move_drag = begin_move_selection_drag(tab, pt, alt_held);
            tab.session.drag = ActiveCanvasDrag::Transform(TransformDragState::Move(move_drag));
        }
        TransformHoverTarget::None => {
            if let Some(hit_idx) = tab.session.topmost_shape_at(pt).or_else(|| {
                compute_hover_target(&tab.session, pt, zoom, sub_mode).shape_idx()
            }) {
                if !tab.session.selected_shapes.contains(&hit_idx) {
                    select_single_shape_at(tab, sub_mode, pt);
                }
                let move_drag = begin_move_selection_drag(tab, pt, alt_held);
                tab.session.drag =
                    ActiveCanvasDrag::Transform(TransformDragState::Move(move_drag));
            } else {
                tab.session.drag = ActiveCanvasDrag::Transform(
                    TransformDragState::MarqueeSelect(MarqueeDrag::new(pt, false)),
                );
            }
        }
    }
}

pub(crate) fn update_transform_drag(
    tab: &mut Tab,
    pt: Point,
    shift_held: bool,
    alt_held: bool,
) {
    let mut move_delta: Option<KVec2> = None;
    let mut new_pivot_pt: Option<Point> = None;
    let mut scale_update: Option<(Affine, Point, bool)> = None;
    let mut rotate_update: Option<Affine> = None;

    if let ActiveCanvasDrag::Transform(transform_drag) = &mut tab.session.drag {
        match transform_drag {
            TransformDragState::MovePivot {
                orig_pivot,
                changed,
                ..
            } => {
                if (pt - *orig_pivot).hypot() > 0.5 / (tab.view.zoom as f64) {
                    *changed = true;
                }
                new_pivot_pt = Some(pt);
            }
            TransformDragState::Move(drag) => {
                move_delta = Some(drag.step(pt));
            }
            TransformDragState::Scale {
                handle,
                anchor,
                handle_start,
                orig_pivot,
                changed,
                ..
            } => {
                let eff_anchor = if alt_held { *orig_pivot } else { *anchor };
                let axis_scale = |affects: bool, hs: f64, p: f64, a: f64| {
                    if affects && (hs - a).abs() > 1e-3 {
                        (p - a) / (hs - a)
                    } else {
                        1.0
                    }
                };
                let mut sx = axis_scale(handle.affects_x(), handle_start.x, pt.x, eff_anchor.x);
                let mut sy = axis_scale(handle.affects_y(), handle_start.y, pt.y, eff_anchor.y);

                if shift_held && handle.is_corner() {
                    let mag = sx.abs().max(sy.abs());
                    sx = mag.copysign(sx);
                    sy = mag.copysign(sy);
                }
                sx = sx.abs().max(0.02).copysign(sx);
                sy = sy.abs().max(0.02).copysign(sy);

                let affine = affine_scale_around(eff_anchor, sx, sy);
                *changed = (sx - 1.0).abs() > 0.005 || (sy - 1.0).abs() > 0.005;
                scale_update = Some((affine, *orig_pivot, tab.session.transform_pivot.is_some()));
            }
            TransformDragState::Rotate {
                pivot,
                start_angle,
                current_angle,
                ..
            } => {
                if (pt - *pivot).hypot() > 2.0 {
                    let raw = (pt.y - pivot.y).atan2(pt.x - pivot.x) - *start_angle;
                    let mut delta_angle = raw.sin().atan2(raw.cos());
                    if shift_held {
                        let step = std::f64::consts::PI / 12.0; // 15° スナップ
                        delta_angle = (delta_angle / step).round() * step;
                    }
                    *current_angle = delta_angle;
                    rotate_update = Some(affine_rotate_around(*pivot, delta_angle));
                }
            }
            TransformDragState::MarqueeSelect(m) => {
                m.current_pos = pt;
            }
        }
    }

    if let Some((affine, orig_pivot, has_piv)) = scale_update {
        if let ActiveCanvasDrag::Transform(TransformDragState::Scale { orig_shapes, .. }) =
            &tab.session.drag
        {
            Session::transform_selected_from_origins(
                &mut tab.session.shapes,
                orig_shapes,
                affine,
            );
        }
        if has_piv {
            tab.session.transform_pivot = Some(affine * orig_pivot);
        }
    }
    if let Some(affine) = rotate_update {
        if let ActiveCanvasDrag::Transform(TransformDragState::Rotate { orig_shapes, .. }) =
            &tab.session.drag
        {
            Session::transform_selected_from_origins(
                &mut tab.session.shapes,
                orig_shapes,
                affine,
            );
        }
    }
    if let Some(piv_pt) = new_pivot_pt {
        if update_selected_symbol_world_pivot_live(tab, Some(piv_pt)) {
            tab.session.transform_pivot = None;
        } else {
            tab.session.transform_pivot = Some(piv_pt);
        }
    }
    if let Some(delta) = move_delta {
        tab.session.translate_selected(delta);
    }
}

pub(crate) fn finish_transform_drag(host: &mut impl ActionHost, mouse_pt: Option<Point>) {
    let ActiveCanvasDrag::Transform(finished) =
        std::mem::replace(&mut host.tab_mut().session.drag, ActiveCanvasDrag::None)
    else {
        return;
    };
    match finished {
        TransformDragState::Move(drag) => {
            apply(
                host,
                Action::FinishMoveSelectionDrag {
                    total_delta: drag.total_delta,
                    undo_snapshot: drag.undo_snapshot,
                    orig_selected_ids: drag.orig_selected_ids,
                },
            );
        }
        TransformDragState::Scale {
            undo_snapshot,
            changed,
            ..
        } => {
            apply(
                host,
                Action::CommitTransformDrag {
                    changed,
                    undo_snapshot,
                },
            );
        }
        TransformDragState::Rotate {
            current_angle,
            undo_snapshot,
            ..
        } => {
            apply(
                host,
                Action::CommitTransformDrag {
                    changed: current_angle.abs() > 0.002,
                    undo_snapshot,
                },
            );
        }
        TransformDragState::MarqueeSelect(m) => {
            let end_pos = mouse_pt.unwrap_or(m.current_pos);
            apply(
                host,
                Action::SelectMarquee {
                    p0: m.start_pos,
                    p1: end_pos,
                    additive: m.additive,
                },
            );
        }
        TransformDragState::MovePivot {
            undo_snapshot,
            changed,
            ..
        } => {
            apply(
                host,
                Action::CommitPivotDrag {
                    changed,
                    undo_snapshot,
                },
            );
        }
    }
}

fn handle_transform_click(host: &mut impl ActionHost, pt: Point, shift_held: bool) {
    let sub_mode = host.tool().select_sub_mode;
    let tab = host.tab_mut();
    tab.view.inspect_timeline_frame = false;
    let zoom = tab.view.zoom;
    let hit_opt = tab.session.topmost_shape_at(pt).or_else(|| {
        compute_hover_target(&tab.session, pt, zoom, sub_mode).shape_idx()
    });
    if shift_held {
        if let Some(hit_idx) = hit_opt {
            toggle_shape_selection(tab, hit_idx);
        }
    } else if compute_transform_hover(&tab.session, pt, zoom) != TransformHoverTarget::Pivot {
        if let Some(hit_idx) = hit_opt {
            if !tab.session.selected_shapes.contains(&hit_idx) {
                select_single_shape_at(tab, sub_mode, pt);
            } else {
                let unit = tab.session.expand_to_group_unit(hit_idx);
                retain_selection_and_flush_deselected(tab, &unit);
            }
        } else {
            clear_selection(tab);
        }
    }
}

/// 自由変形ツール（`Q`）でのマウス位置のヒット判定
pub(crate) fn compute_transform_hover(
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
) -> TransformHoverTarget {
    let Some(bbox) = session.selection_bounding_box(zoom) else {
        return TransformHoverTarget::None;
    };
    let inv_z = 1.0 / (zoom as f64);
    let mut best_handle: Option<(f64, ScaleHandle)> = None;
    for handle in ScaleHandle::ALL {
        let d = (mouse_pt - handle.position(bbox)).hypot();
        if d <= 9.0 * inv_z && best_handle.is_none_or(|(bd, _)| d < bd) {
            best_handle = Some((d, handle));
        }
    }
    let pivot = session.active_transform_pivot(bbox);
    let pivot_dist = (mouse_pt - pivot).hypot();
    if pivot_dist <= 8.5 * inv_z && best_handle.is_none_or(|(hd, _)| pivot_dist < hd) {
        return TransformHoverTarget::Pivot;
    }
    if let Some((_, h)) = best_handle {
        return TransformHoverTarget::Scale(h);
    }

    let corner_rotate_radius = 24.0 * inv_z;
    let outer_rotate_rect = bbox.inflate(corner_rotate_radius, corner_rotate_radius);
    let inner_move_rect = bbox.inflate(-2.0 * inv_z, -2.0 * inv_z);
    let corners = [
        ScaleHandle::TopLeft.position(bbox),
        ScaleHandle::TopRight.position(bbox),
        ScaleHandle::BottomRight.position(bbox),
        ScaleHandle::BottomLeft.position(bbox),
    ];
    if outer_rotate_rect.contains(mouse_pt)
        && !inner_move_rect.contains(mouse_pt)
        && corners
            .iter()
            .any(|&c| (mouse_pt - c).hypot() <= corner_rotate_radius)
    {
        return TransformHoverTarget::Rotate;
    }

    if bbox.contains(mouse_pt)
        || session.selected_shapes.iter().any(|&idx| {
            session
                .shapes
                .get(idx)
                .is_some_and(|s| s.contains_point(mouse_pt))
        })
    {
        return TransformHoverTarget::Move;
    }

    TransformHoverTarget::None
}

/// 自由変形ツール（`Tool::Transform`）のバウンディングボックス・8ハンドル・基準点（ピボット）を描画
pub(crate) fn draw_free_transform_box(
    session: &Session,
    painter: &egui::Painter,
    view: &View,
) {
    if session.selected_shapes.is_empty() {
        return;
    }

    let (base_bbox, rot_affine, pivot) =
        if let ActiveCanvasDrag::Transform(TransformDragState::Rotate {
            orig_bbox,
            pivot,
            current_angle,
            ..
        }) = &session.drag
        {
            (
                *orig_bbox,
                affine_rotate_around(*pivot, *current_angle),
                *pivot,
            )
        } else if let Some(bbox) = session.selection_bounding_box(view.zoom) {
            let piv = session.active_transform_pivot(bbox);
            (bbox, Affine::IDENTITY, piv)
        } else {
            return;
        };

    let corners = [
        rot_affine * Point::new(base_bbox.x0, base_bbox.y0),
        rot_affine * Point::new(base_bbox.x1, base_bbox.y0),
        rot_affine * Point::new(base_bbox.x1, base_bbox.y1),
        rot_affine * Point::new(base_bbox.x0, base_bbox.y1),
    ];

    let box_stroke = Stroke::new(1.5_f32, Color32::from_rgb(20, 115, 225));
    for i in 0..4 {
        painter.line_segment(
            [
                view.world_to_screen(corners[i]),
                view.world_to_screen(corners[(i + 1) % 4]),
            ],
            box_stroke,
        );
    }

    // 8個の変形ハンドル（四隅＋各辺の中央）を描画
    let is_drag_idle = !matches!(session.drag, ActiveCanvasDrag::Transform(_));
    for handle in ScaleHandle::ALL {
        let hp = view.world_to_screen(rot_affine * handle.position(base_bbox));
        let is_active = matches!(
            session.drag,
            ActiveCanvasDrag::Transform(TransformDragState::Scale { handle: h, .. }) if h == handle
        ) || (is_drag_idle
            && session.transform_hover == TransformHoverTarget::Scale(handle));

        let half = if is_active { 4.5 } else { 3.6 };
        let rect = egui::Rect::from_center_size(hp, Vec2::splat(half * 2.0));
        let fill_col = if is_active {
            Color32::from_rgb(0, 175, 255)
        } else {
            Color32::from_rgb(25, 30, 40)
        };
        painter.rect_filled(rect, 0.0, fill_col);
        painter.rect_stroke(rect, 0.0, Stroke::new(1.5_f32, Color32::WHITE));
    }

    // 基準点（ピボット点：白い丸＋中心ドット）を描画
    let piv_pos = view.world_to_screen(pivot);
    let piv_active = matches!(
        session.drag,
        ActiveCanvasDrag::Transform(TransformDragState::MovePivot { .. })
    ) || (is_drag_idle && session.transform_hover == TransformHoverTarget::Pivot);
    let piv_radius = if piv_active { 5.8 } else { 4.8 };
    painter.circle_filled(piv_pos, piv_radius, Color32::WHITE);
    painter.circle_stroke(
        piv_pos,
        piv_radius,
        Stroke::new(
            1.8_f32,
            if piv_active {
                Color32::from_rgb(255, 110, 30)
            } else {
                Color32::from_rgb(20, 25, 35)
            },
        ),
    );
    painter.circle_filled(piv_pos, 1.5, Color32::from_rgb(20, 25, 35));
}

pub(crate) fn draw_transform_tool_cursor(
    tab: &Tab,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match &tab.session.drag {
        ActiveCanvasDrag::Transform(
            TransformDragState::Move { .. } | TransformDragState::MovePivot { .. },
        ) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        }
        ActiveCanvasDrag::Transform(TransformDragState::Scale { handle, .. }) => {
            ctx.set_cursor_icon(scale_handle_cursor_icon(*handle));
        }
        ActiveCanvasDrag::Transform(TransformDragState::Rotate { .. }) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            draw_transform_rotate_cursor_badge(painter, pos);
        }
        ActiveCanvasDrag::Transform(TransformDragState::MarqueeSelect { .. }) => {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
        }
        _ => match tab.session.transform_hover {
            TransformHoverTarget::Pivot => {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
            TransformHoverTarget::Scale(h) => {
                ctx.set_cursor_icon(scale_handle_cursor_icon(h));
            }
            TransformHoverTarget::Rotate => {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                draw_transform_rotate_cursor_badge(painter, pos);
            }
            TransformHoverTarget::Move => {
                ctx.set_cursor_icon(CursorIcon::Move);
            }
            TransformHoverTarget::None => {
                ctx.set_cursor_icon(CursorIcon::Default);
            }
        },
    }
}

/// 自由変形ツールの各スケールハンドルに対応するリサイズカーソルアイコンを返す
fn scale_handle_cursor_icon(handle: ScaleHandle) -> CursorIcon {
    match handle {
        ScaleHandle::TopLeft | ScaleHandle::BottomRight => CursorIcon::ResizeNwSe,
        ScaleHandle::TopRight | ScaleHandle::BottomLeft => CursorIcon::ResizeNeSw,
        ScaleHandle::Top | ScaleHandle::Bottom => CursorIcon::ResizeVertical,
        ScaleHandle::Left | ScaleHandle::Right => CursorIcon::ResizeHorizontal,
    }
}

/// 自由変形ツールの外周で回転モードに入った際の「回転アーク」カーソルバッジを描画
fn draw_transform_rotate_cursor_badge(painter: &egui::Painter, cursor: Pos2) {
    let base = cursor + Vec2::new(15.0, 15.0);
    painter.circle_filled(
        base,
        8.5,
        Color32::from_rgba_unmultiplied(255, 255, 255, 235),
    );
    painter.circle_stroke(base, 8.5, Stroke::new(1.5_f32, Color32::from_rgb(0, 125, 240)));
    let r = 5.0_f32;
    let steps = 10;
    let start_ang = -std::f32::consts::FRAC_PI_4 * 3.0;
    let end_ang = std::f32::consts::FRAC_PI_2;
    let stroke = Stroke::new(1.8_f32, Color32::from_rgb(20, 25, 35));
    let mut prev = Pos2::new(base.x + r * start_ang.cos(), base.y + r * start_ang.sin());
    for i in 1..=steps {
        let a = start_ang + (end_ang - start_ang) * (i as f32 / steps as f32);
        let curr = Pos2::new(base.x + r * a.cos(), base.y + r * a.sin());
        painter.line_segment([prev, curr], stroke);
        prev = curr;
    }
    // 矢印の先端
    let tip = prev;
    painter.line_segment([tip, tip + Vec2::new(-3.2, -2.5)], stroke);
    painter.line_segment([tip, tip + Vec2::new(2.5, -3.2)], stroke);
}
