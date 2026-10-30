use eframe::egui::{self, Color32, CursorIcon, Pos2, Stroke, Vec2};
use kurbo::{Affine, Point, Rect as KRect, Vec2 as KVec2};

use crate::canvas_tool::{
    ActiveCanvasDrag, CanvasState, MarqueeDrag, MoveSelectionDrag, ViewportState,
};
use crate::symbol::{
    resolve_cel_with_library, transform_layers_with_local_cache, SymbolEditSession, SymbolEvalCtx,
    SymbolInstanceBinding, SymbolLoopMode, TweenRotate,
};
use crate::timeline::Cel;
use crate::vector_engine::{
    affine_rotate_around, affine_scale_around, union_bounding_boxes, FillShape,
};
use crate::Is2dApp;

// ============================================================================
// 自由変形ツール（Tool::Transform / Q キー）の型定義・操作・アフィン変換・変形枠描画
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScaleHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl ScaleHandle {
    pub(crate) const ALL: [ScaleHandle; 8] = [
        ScaleHandle::TopLeft,
        ScaleHandle::Top,
        ScaleHandle::TopRight,
        ScaleHandle::Right,
        ScaleHandle::BottomRight,
        ScaleHandle::Bottom,
        ScaleHandle::BottomLeft,
        ScaleHandle::Left,
    ];

    pub(crate) fn position(self, r: KRect) -> Point {
        let cx = 0.5 * (r.x0 + r.x1);
        let cy = 0.5 * (r.y0 + r.y1);
        match self {
            ScaleHandle::TopLeft => Point::new(r.x0, r.y0),
            ScaleHandle::Top => Point::new(cx, r.y0),
            ScaleHandle::TopRight => Point::new(r.x1, r.y0),
            ScaleHandle::Right => Point::new(r.x1, cy),
            ScaleHandle::BottomRight => Point::new(r.x1, r.y1),
            ScaleHandle::Bottom => Point::new(cx, r.y1),
            ScaleHandle::BottomLeft => Point::new(r.x0, r.y1),
            ScaleHandle::Left => Point::new(r.x0, cy),
        }
    }

    pub(crate) fn opposite(self) -> Self {
        match self {
            ScaleHandle::TopLeft => ScaleHandle::BottomRight,
            ScaleHandle::Top => ScaleHandle::Bottom,
            ScaleHandle::TopRight => ScaleHandle::BottomLeft,
            ScaleHandle::Right => ScaleHandle::Left,
            ScaleHandle::BottomRight => ScaleHandle::TopLeft,
            ScaleHandle::Bottom => ScaleHandle::Top,
            ScaleHandle::BottomLeft => ScaleHandle::TopRight,
            ScaleHandle::Left => ScaleHandle::Right,
        }
    }

    pub(crate) fn opposite_position(self, r: KRect) -> Point {
        self.opposite().position(r)
    }

    pub(crate) fn affects_x(self) -> bool {
        !matches!(self, ScaleHandle::Top | ScaleHandle::Bottom)
    }

    pub(crate) fn affects_y(self) -> bool {
        !matches!(self, ScaleHandle::Left | ScaleHandle::Right)
    }

    pub(crate) fn is_corner(self) -> bool {
        matches!(
            self,
            ScaleHandle::TopLeft
                | ScaleHandle::TopRight
                | ScaleHandle::BottomRight
                | ScaleHandle::BottomLeft
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum TransformHoverTarget {
    #[default]
    None,
    Pivot,
    Scale(ScaleHandle),
    Rotate,
    Move,
}

#[derive(Clone, Debug)]
pub(crate) enum TransformDragState {
    MovePivot {
        orig_pivot: Point,
        undo_snapshot: Vec<FillShape>,
        changed: bool,
    },
    Move(MoveSelectionDrag),
    Scale {
        handle: ScaleHandle,
        anchor: Point,
        handle_start: Point,
        orig_pivot: Point,
        orig_shapes: Vec<(usize, FillShape)>,
        undo_snapshot: Vec<FillShape>,
        changed: bool,
    },
    Rotate {
        orig_bbox: KRect,
        pivot: Point,
        start_angle: f64,
        current_angle: f64,
        orig_shapes: Vec<(usize, FillShape)>,
        undo_snapshot: Vec<FillShape>,
    },
    MarqueeSelect(MarqueeDrag),
}

/// 単一のシンボルインスタンスが選択されている場合、ワールド座標 `world_pivot`（`None` なら初期基準点 `(0, 0)`）を
/// シンボルローカル座標に変換してインスタンスの `binding.pivot` に保存する
pub(crate) fn set_selected_symbol_world_pivot(
    app: &mut Is2dApp,
    world_pivot: Option<Point>,
    commit_to_timeline: bool,
) -> bool {
    let Some(binding) = app.canvas.selected_single_symbol_binding() else {
        return false;
    };
    let new_local_pivot = match world_pivot {
        Some(pt) => {
            if binding.xform.determinant().abs() <= 1e-9 {
                return false;
            }
            binding.xform.inverse() * pt
        }
        None => Point::ZERO,
    };
    if (new_local_pivot - binding.pivot).hypot() <= 1e-7 {
        return true;
    }
    let undo_snapshot = commit_to_timeline.then(|| app.canvas.shapes.clone());
    app.canvas
        .update_symbol_instance_binding(binding.instance_gid, |b| b.pivot = new_local_pivot);
    if let Some(snapshot) = undo_snapshot {
        app.commit_canvas_mutation(snapshot);
    }
    true
}

/// 選択オブジェクトの変形中心（ピボット）を初期位置（選択中央、またはシンボルの基準点 `(0, 0)`）へリセットする
pub(crate) fn reset_selected_transform_pivot(app: &mut Is2dApp) -> bool {
    let mut changed = app.canvas.transform_pivot.take().is_some();
    if set_selected_symbol_world_pivot(app, None, true) {
        changed = true;
    }
    changed
}

fn flush_transform_pivot_to_symbol(app: &mut Is2dApp) {
    if let Some(piv) = app.canvas.transform_pivot {
        if set_selected_symbol_world_pivot(app, Some(piv), false) {
            app.canvas.transform_pivot = None;
        }
    }
}

/// 選択中のオブジェクトを現在の基準点（ピボット）中心にアフィン変換する
pub(crate) fn apply_affine_to_selected(app: &mut Is2dApp, affine: Affine) -> bool {
    crate::canvas_tool::cancel_active_drag(app);
    if app.canvas.selected_shapes.is_empty() {
        return false;
    }
    let undo_snapshot = app.canvas.shapes.clone();
    flush_transform_pivot_to_symbol(app);
    let any_transformed = app.canvas.transform_selected(affine);
    if any_transformed {
        crate::tool_select::commit_moved_selection(app, undo_snapshot);
    }
    any_transformed
}

/// 選択中のオブジェクトを現在の基準点（ピボット）中心に `(sx, sy)` 倍に拡大・縮小（または反転）する
pub(crate) fn scale_selected_shapes(app: &mut Is2dApp, sx: f64, sy: f64) -> bool {
    let Some(bbox) = app.canvas.selection_bounding_box(app.viewport.zoom) else {
        return false;
    };
    let pivot = app.canvas.active_transform_pivot(bbox);
    apply_affine_to_selected(app, affine_scale_around(pivot, sx, sy))
}

/// 選択中のオブジェクトを左右反転（`flip_x`）または上下反転（`flip_y`）する
pub(crate) fn flip_selected_shapes(app: &mut Is2dApp, flip_x: bool, flip_y: bool) -> bool {
    scale_selected_shapes(
        app,
        if flip_x { -1.0 } else { 1.0 },
        if flip_y { -1.0 } else { 1.0 },
    )
}

/// 選択中のオブジェクトを現在の基準点（ピボット）中心に `angle_rad` 回転する
pub(crate) fn rotate_selected_shapes(app: &mut Is2dApp, angle_rad: f64) -> bool {
    let Some(bbox) = app.canvas.selection_bounding_box(app.viewport.zoom) else {
        return false;
    };
    let pivot = app.canvas.active_transform_pivot(bbox);
    apply_affine_to_selected(app, affine_rotate_around(pivot, angle_rad))
}

/// 自由変形ツール（`Tool::Transform` / `Q`）のマウス入力処理
pub(crate) fn handle_transform_tool_input(
    app: &mut Is2dApp,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    let shift_held = ui.input(|i| i.modifiers.shift);
    let alt_held = ui.input(|i| i.modifiers.alt);
    let zoom = app.viewport.zoom;

    if response.double_clicked() {
        if let Some(pt) = mouse_pt {
            if compute_transform_hover(&app.canvas, pt, zoom) == TransformHoverTarget::Pivot {
                reset_selected_transform_pivot(app);
            } else {
                crate::tool_select::handle_canvas_double_click(app, pt);
            }
        }
        return;
    }

    if response.drag_started() {
        let press_pt = ui
            .input(|i| i.pointer.press_origin())
            .map(|p| app.viewport.screen_to_world(p));
        if let Some(start_pt) = press_pt.or(mouse_pt) {
            start_transform_drag(app, start_pt, shift_held, alt_held);
            if let Some(curr_pt) = mouse_pt {
                if (curr_pt - start_pt).hypot() > 1e-6 {
                    update_transform_drag(app, curr_pt, shift_held, alt_held);
                }
            }
        }
    } else if response.dragged() {
        if let Some(pt) = mouse_pt {
            update_transform_drag(app, pt, shift_held, alt_held);
        }
    }
    if response.drag_stopped() {
        finish_transform_drag(app, mouse_pt);
    } else if response.clicked() {
        if let Some(pt) = mouse_pt {
            handle_transform_click(app, pt, shift_held);
        }
    }

    if !matches!(app.canvas.drag, ActiveCanvasDrag::Transform(_)) {
        app.canvas.transform_hover = mouse_pt
            .map(|pt| compute_transform_hover(&app.canvas, pt, app.viewport.zoom))
            .unwrap_or(TransformHoverTarget::None);
    }
}

fn start_transform_drag(app: &mut Is2dApp, pt: Point, shift_held: bool, alt_held: bool) {
    app.ui.inspect_timeline_frame = false;
    let zoom = app.viewport.zoom;
    let hover = compute_transform_hover(&app.canvas, pt, zoom);
    if shift_held && hover == TransformHoverTarget::None {
        app.canvas.drag = ActiveCanvasDrag::Transform(TransformDragState::MarqueeSelect(
            MarqueeDrag::new(pt, true),
        ));
        return;
    }

    match hover {
        TransformHoverTarget::Pivot => {
            if let Some(bbox) = app.canvas.selection_bounding_box(zoom) {
                let orig_pivot = app.canvas.active_transform_pivot(bbox);
                app.canvas.drag = ActiveCanvasDrag::Transform(TransformDragState::MovePivot {
                    orig_pivot,
                    undo_snapshot: app.canvas.snapshot_for_drag(),
                    changed: false,
                });
            }
        }
        TransformHoverTarget::Scale(handle) => {
            if let Some(orig_bbox) = app.canvas.selection_bounding_box(zoom) {
                let undo_snapshot = app.canvas.snapshot_for_drag();
                let orig_pivot = app.canvas.active_transform_pivot(orig_bbox);
                flush_transform_pivot_to_symbol(app);
                let anchor = if alt_held {
                    orig_pivot
                } else {
                    handle.opposite_position(orig_bbox)
                };
                app.canvas.drag = ActiveCanvasDrag::Transform(TransformDragState::Scale {
                    handle,
                    anchor,
                    handle_start: handle.position(orig_bbox),
                    orig_pivot,
                    orig_shapes: app.canvas.selected_shapes_snapshot(),
                    undo_snapshot,
                    changed: false,
                });
            }
        }
        TransformHoverTarget::Rotate => {
            if let Some(orig_bbox) = app.canvas.selection_bounding_box(zoom) {
                let undo_snapshot = app.canvas.snapshot_for_drag();
                let pivot = app.canvas.active_transform_pivot(orig_bbox);
                flush_transform_pivot_to_symbol(app);
                app.canvas.drag = ActiveCanvasDrag::Transform(TransformDragState::Rotate {
                    orig_bbox,
                    pivot,
                    start_angle: (pt.y - pivot.y).atan2(pt.x - pivot.x),
                    current_angle: 0.0,
                    orig_shapes: app.canvas.selected_shapes_snapshot(),
                    undo_snapshot,
                });
            }
        }
        TransformHoverTarget::Move => {
            let move_drag = crate::tool_select::begin_move_selection_drag(app, pt, alt_held);
            app.canvas.drag = ActiveCanvasDrag::Transform(TransformDragState::Move(move_drag));
        }
        TransformHoverTarget::None => {
            if let Some(hit_idx) = app.canvas.topmost_shape_at(pt).or_else(|| {
                crate::tool_select::compute_hover_target(
                    &app.canvas,
                    pt,
                    zoom,
                    app.ui.select_sub_mode,
                )
                .shape_idx()
            }) {
                if !app.canvas.selected_shapes.contains(&hit_idx) {
                    crate::tool_select::select_single_shape_at(app, pt);
                }
                let move_drag = crate::tool_select::begin_move_selection_drag(app, pt, alt_held);
                app.canvas.drag = ActiveCanvasDrag::Transform(TransformDragState::Move(move_drag));
            } else {
                app.canvas.drag = ActiveCanvasDrag::Transform(TransformDragState::MarqueeSelect(
                    MarqueeDrag::new(pt, false),
                ));
            }
        }
    }
}

fn update_transform_drag(app: &mut Is2dApp, pt: Point, shift_held: bool, alt_held: bool) {
    let mut move_delta: Option<KVec2> = None;
    let mut new_pivot_pt: Option<Point> = None;
    let mut scale_update: Option<(Affine, Point, bool)> = None;
    let mut rotate_update: Option<Affine> = None;

    if let ActiveCanvasDrag::Transform(transform_drag) = &mut app.canvas.drag {
        match transform_drag {
            TransformDragState::MovePivot {
                orig_pivot,
                changed,
                ..
            } => {
                if (pt - *orig_pivot).hypot() > 0.5 / (app.viewport.zoom as f64) {
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
                scale_update = Some((affine, *orig_pivot, app.canvas.transform_pivot.is_some()));
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
            &app.canvas.drag
        {
            CanvasState::transform_selected_from_origins(
                &mut app.canvas.shapes,
                orig_shapes,
                affine,
            );
        }
        if has_piv {
            app.canvas.transform_pivot = Some(affine * orig_pivot);
        }
    }
    if let Some(affine) = rotate_update {
        if let ActiveCanvasDrag::Transform(TransformDragState::Rotate { orig_shapes, .. }) =
            &app.canvas.drag
        {
            CanvasState::transform_selected_from_origins(
                &mut app.canvas.shapes,
                orig_shapes,
                affine,
            );
        }
    }
    if let Some(piv_pt) = new_pivot_pt {
        if set_selected_symbol_world_pivot(app, Some(piv_pt), false) {
            app.canvas.transform_pivot = None;
        } else {
            app.canvas.transform_pivot = Some(piv_pt);
        }
    }
    if let Some(delta) = move_delta {
        app.canvas.translate_selected(delta);
    }
}

fn finish_transform_drag(app: &mut Is2dApp, mouse_pt: Option<Point>) {
    let ActiveCanvasDrag::Transform(finished) =
        std::mem::replace(&mut app.canvas.drag, ActiveCanvasDrag::None)
    else {
        return;
    };
    match finished {
        TransformDragState::Move(drag) => {
            let min_move = 1.0 / (app.viewport.zoom as f64);
            let rolled_back =
                drag.total_delta.hypot() <= min_move || app.canvas.selected_shapes.is_empty();
            let orig_ids = drag.orig_selected_ids;
            crate::tool_select::finish_move_selection_drag(
                app,
                drag.total_delta,
                drag.undo_snapshot,
            );
            if rolled_back && !orig_ids.is_empty() {
                app.canvas.restore_selection_by_shape_ids(&orig_ids);
            }
        }
        TransformDragState::Scale {
            undo_snapshot,
            changed,
            ..
        } => {
            if changed && !app.canvas.selected_shapes.is_empty() {
                crate::tool_select::commit_moved_selection(app, undo_snapshot);
            } else {
                app.canvas.restore_drag_snapshot(undo_snapshot);
            }
        }
        TransformDragState::Rotate {
            current_angle,
            undo_snapshot,
            ..
        } => {
            if current_angle.abs() > 0.002 && !app.canvas.selected_shapes.is_empty() {
                crate::tool_select::commit_moved_selection(app, undo_snapshot);
            } else {
                app.canvas.restore_drag_snapshot(undo_snapshot);
            }
        }
        TransformDragState::MarqueeSelect(m) => {
            let end_pos = mouse_pt.unwrap_or(m.current_pos);
            crate::tool_select::apply_marquee_selection(app, m.start_pos, end_pos, m.additive);
        }
        TransformDragState::MovePivot {
            undo_snapshot,
            changed,
            ..
        } => {
            if changed {
                if selected_single_symbol_info(app).is_some() {
                    app.commit_canvas_mutation(undo_snapshot);
                }
            } else {
                app.canvas.restore_drag_snapshot(undo_snapshot);
            }
        }
    }
}

fn handle_transform_click(app: &mut Is2dApp, pt: Point, shift_held: bool) {
    app.ui.inspect_timeline_frame = false;
    let zoom = app.viewport.zoom;
    let hit_opt = app.canvas.topmost_shape_at(pt).or_else(|| {
        crate::tool_select::compute_hover_target(&app.canvas, pt, zoom, app.ui.select_sub_mode)
            .shape_idx()
    });
    if shift_held {
        if let Some(hit_idx) = hit_opt {
            crate::tool_select::toggle_shape_selection(app, hit_idx);
        }
    } else if compute_transform_hover(&app.canvas, pt, zoom) != TransformHoverTarget::Pivot {
        if let Some(hit_idx) = hit_opt {
            if !app.canvas.selected_shapes.contains(&hit_idx) {
                crate::tool_select::select_single_shape_at(app, pt);
            } else {
                let unit = app.canvas.expand_to_group_unit(hit_idx);
                if app.canvas.selection_modified && app.canvas.selected_shapes != unit {
                    let unit_ids: Vec<u64> = unit
                        .iter()
                        .filter_map(|&i| app.canvas.shapes.get(i).map(|s| s.shape_id))
                        .collect();
                    if let Some(unsliced) = app.canvas.take_unsliced_snapshot() {
                        let all_sel_ids = app.canvas.selected_shape_ids();
                        app.canvas.shapes = unsliced;
                        app.canvas.restore_selection_by_shape_ids(&all_sel_ids);
                    }
                    crate::tool_select::flush_modified_raw_selection_merge(app);
                    app.canvas.restore_selection_by_shape_ids(&unit_ids);
                } else {
                    app.canvas.selected_shapes = unit;
                }
                app.canvas.transform_pivot = None;
                crate::canvas_tool::sync_timeline_layer_from_canvas_selection(app);
            }
        } else {
            crate::tool_select::clear_selection(app);
        }
    }
}

/// 自由変形ツール（`Q`）でのマウス位置のヒット判定
pub(crate) fn compute_transform_hover(
    canvas: &CanvasState,
    mouse_pt: Point,
    zoom: f32,
) -> TransformHoverTarget {
    let Some(bbox) = canvas.selection_bounding_box(zoom) else {
        return TransformHoverTarget::None;
    };
    let inv_z = 1.0 / (zoom as f64);
    let pivot = canvas.active_transform_pivot(bbox);
    if (mouse_pt - pivot).hypot() <= 8.5 * inv_z {
        return TransformHoverTarget::Pivot;
    }

    let mut best_handle: Option<(f64, ScaleHandle)> = None;
    for handle in ScaleHandle::ALL {
        let d = (mouse_pt - handle.position(bbox)).hypot();
        if d <= 9.0 * inv_z && best_handle.is_none_or(|(bd, _)| d < bd) {
            best_handle = Some((d, handle));
        }
    }
    if let Some((_, h)) = best_handle {
        return TransformHoverTarget::Scale(h);
    }

    let outer_rotate_rect = bbox.inflate(24.0 * inv_z, 24.0 * inv_z);
    let inner_move_rect = bbox.inflate(-2.0 * inv_z, -2.0 * inv_z);
    if outer_rotate_rect.contains(mouse_pt) && !inner_move_rect.contains(mouse_pt) {
        return TransformHoverTarget::Rotate;
    }

    if bbox.contains(mouse_pt)
        || canvas.selected_shapes.iter().any(|&idx| {
            canvas
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
    canvas: &CanvasState,
    painter: &egui::Painter,
    viewport: &ViewportState,
) {
    if canvas.selected_shapes.is_empty() {
        return;
    }

    let (base_bbox, rot_affine, pivot) =
        if let ActiveCanvasDrag::Transform(TransformDragState::Rotate {
            orig_bbox,
            pivot,
            current_angle,
            ..
        }) = &canvas.drag
        {
            (
                *orig_bbox,
                affine_rotate_around(*pivot, *current_angle),
                *pivot,
            )
        } else if let Some(bbox) = canvas.selection_bounding_box(viewport.zoom) {
            let piv = canvas.active_transform_pivot(bbox);
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
                viewport.world_to_screen(corners[i]),
                viewport.world_to_screen(corners[(i + 1) % 4]),
            ],
            box_stroke,
        );
    }

    // 8個の変形ハンドル（四隅＋各辺の中央）を描画
    let is_drag_idle = !matches!(canvas.drag, ActiveCanvasDrag::Transform(_));
    for handle in ScaleHandle::ALL {
        let hp = viewport.world_to_screen(rot_affine * handle.position(base_bbox));
        let is_active = matches!(
            canvas.drag,
            ActiveCanvasDrag::Transform(TransformDragState::Scale { handle: h, .. }) if h == handle
        ) || (is_drag_idle
            && canvas.transform_hover == TransformHoverTarget::Scale(handle));

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
    let piv_pos = viewport.world_to_screen(pivot);
    let piv_active = matches!(
        canvas.drag,
        ActiveCanvasDrag::Transform(TransformDragState::MovePivot { .. })
    ) || (is_drag_idle && canvas.transform_hover == TransformHoverTarget::Pivot);
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
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match &app.canvas.drag {
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
        _ => match app.canvas.transform_hover {
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

// ============================================================================
// シンボル変換・ライブラリ配置/削除・シンボルタイムライン階層遷移
// ============================================================================

/// 選択中のオブジェクト（生シェイプ・グループ問わず）を新しい「シンボル」に変換してライブラリに登録する（`F8`）
pub(crate) fn convert_selection_to_symbol(app: &mut Is2dApp) -> Option<u64> {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    if app.canvas.selected_shapes.is_empty() {
        return None;
    }
    let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
    let valid_indices: Vec<usize> = app
        .canvas
        .selected_shapes
        .iter()
        .copied()
        .filter(|&idx| {
            app.canvas
                .shapes
                .get(idx)
                .is_some_and(|s| s.is_in_editing_scope(&app.canvas.editing_group_path))
        })
        .collect();
    if valid_indices.is_empty() {
        app.canvas.restore_unsliced_snapshot(snapshot);
        return None;
    }

    let mut bbox = app.canvas.shapes[valid_indices[0]].bounding_box();
    for &idx in &valid_indices[1..] {
        bbox = bbox.union(app.canvas.shapes[idx].bounding_box());
    }
    let center = Point::new((bbox.x0 + bbox.x1) * 0.5, (bbox.y0 + bbox.y1) * 0.5);

    let sym_id = app.symbols.next_symbol_id;
    let inst_gid = app.symbols.alloc_group_id();

    let depth = app.canvas.editing_group_path.len();
    let to_local = KVec2::new(-center.x, -center.y);
    let mut local_shapes = Vec::with_capacity(valid_indices.len());
    for &idx in &valid_indices {
        let mut local_s = app.canvas.shapes[idx].clone();
        local_s.layer_id = 1;
        local_s.group_path = local_s.group_path.get(depth..).unwrap_or_default().to_vec();
        if local_s
            .symbol_instance
            .is_some_and(|b| !local_s.group_path.contains(&b.instance_gid))
        {
            local_s.symbol_instance = None;
        }
        local_s.translate(to_local);
        local_shapes.push(local_s);
    }

    let target_lid = crate::canvas_tool::topmost_layer_id_for_indices(app, &valid_indices);
    let binding = SymbolInstanceBinding::new(
        inst_gid,
        sym_id,
        Affine::translate(KVec2::new(center.x, center.y)),
    );
    for &idx in &valid_indices {
        if let Some(shape) = app.canvas.shapes.get_mut(idx) {
            shape.layer_id = target_lid;
            shape.group_path.insert(depth, inst_gid);
            shape.symbol_instance = Some(binding);
        }
    }

    if let Some(l_idx) = app.timeline.layer_index_by_id(target_lid) {
        app.timeline.active_layer = l_idx;
    }
    // 1. Undo スナップショットに変換前のキャンバス＆ライブラリ状態を記録
    let snapshot = app.canvas.take_unsliced_snapshot().unwrap_or(snapshot);
    app.record_undo_snapshot(snapshot);
    // 2. 新規シンボル定義をライブラリへ先に登録（親シンボル内編集中でも sync_active_edit が新シンボルを参照できるようにする）
    app.symbols.register_new_symbol(local_shapes);
    // 3. 選択図形をスコープ最前面へ整列してタイムライン＆親シンボル編集スタックへ同期
    crate::tool_select::reorder_selected_to_scope_front(app);
    crate::canvas_tool::sync_shapes_to_timeline(app);
    app.canvas.selection_modified = false;
    app.canvas.transform_pivot = None;
    app.ui.inspect_timeline_frame = false;
    Some(sym_id)
}

/// ライブラリ内の指定シンボル `symbol_id` の新しいインスタンスを現在のステージ中央へ配置する
/// （直接・間接の循環参照を起こす場合は拒否し、Frame 0 が空白のマルチフレームシンボルや全フレーム白紙の空シンボルでも配置可能）
pub(crate) fn place_symbol_instance_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool {
    crate::canvas_tool::stop_playback(app);
    let target_lid = crate::canvas_tool::active_scope_layer_id(app);
    if !app.symbols.can_place_symbol_in_current_context(symbol_id)
        || !app.timeline.is_layer_editable_by_id(target_lid)
    {
        return false;
    }
    let Some(first_shapes) = app
        .symbols
        .find_symbol(symbol_id)
        .map(|s| s.first_non_empty_local_shapes(&app.symbols.library))
    else {
        return false;
    };
    let first_bbox_center = if first_shapes.is_empty() {
        Point::ZERO
    } else {
        union_bounding_boxes(&first_shapes).center()
    };

    app.mutate_canvas_with_undo(true, |app| {
        let inst_gid = app.symbols.alloc_group_id();
        let stage_center = app.viewport.stage_center_world();
        let offset = stage_center - first_bbox_center.to_vec2();
        let binding = SymbolInstanceBinding::new(
            inst_gid,
            symbol_id,
            Affine::translate(KVec2::new(offset.x, offset.y)),
        );
        let target_lid = crate::canvas_tool::active_scope_layer_id(app);
        let curr_frame = app.timeline.current_frame;
        let kf_start = app
            .timeline
            .layer_index_by_id(target_lid)
            .and_then(|l_idx| {
                app.timeline.layers[l_idx]
                    .keyframe_index_at(curr_frame)
                    .map(|ki| app.timeline.layers[l_idx].keyframes[ki].start_frame)
            })
            .unwrap_or(curr_frame);
        let sym_shapes = app
            .symbols
            .find_symbol(symbol_id)
            .map(|s| {
                s.evaluated_local_shapes_for_instance(
                    binding,
                    kf_start,
                    curr_frame,
                    SymbolEvalCtx::new(&app.symbols.library),
                )
            })
            .unwrap_or_default();

        let placed = if !sym_shapes.is_empty() {
            binding.instantiate_shapes(&sym_shapes, target_lid, &app.canvas.editing_group_path)
        } else {
            vec![FillShape::new_empty_symbol_marker(
                binding,
                target_lid,
                &app.canvas.editing_group_path,
            )]
        };
        crate::tool_select::insert_and_select_shapes_in_scope(app, placed, false);
    });
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の名前を変更し、Undo履歴に記録する
/// （同一シンボルの連続テキスト入力中は初回変更時のみ Undo を積む）
pub(crate) fn rename_symbol_in_library(
    app: &mut Is2dApp,
    symbol_id: u64,
    new_name: impl Into<String>,
) -> bool {
    crate::canvas_tool::stop_playback(app);
    let new_name = new_name.into();
    if app
        .symbols
        .find_symbol(symbol_id)
        .is_none_or(|s| s.name == new_name)
    {
        return false;
    }
    if app.symbols.actively_renaming_symbol != Some(symbol_id) {
        app.push_undo();
    }
    let renamed = app.symbols.rename_symbol(symbol_id, new_name);
    if renamed {
        app.symbols.actively_renaming_symbol = Some(symbol_id);
    }
    renamed
}

/// ライブラリから指定シンボル `symbol_id` を削除し、ステージ上および全フレームに配置されていた該当インスタンスは
/// 消去せず通常のグループ図形へ自動分解（デタッチ）して残す
pub(crate) fn delete_symbol_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool {
    crate::canvas_tool::stop_playback(app);
    if app.symbols.find_symbol(symbol_id).is_none() {
        return false;
    }
    while app
        .symbols
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == symbol_id)
    {
        exit_symbol_timeline(app);
    }
    crate::tool_select::clear_selection(app);
    app.push_undo();

    app.symbols
        .remove_symbol_and_detach(symbol_id, &mut app.timeline, &mut app.canvas.shapes);

    app.symbols.sync_active_edit(&app.timeline);
    crate::canvas_tool::load_shapes_from_timeline(app);
    true
}

/// 現在のキーフレーム（または保持フレームが属する開始キーフレーム）に対してクラシックトゥイーンを ON / OFF トグルする（`T` キー）。
/// トゥイーンを ON にするための厳格条件：開始キーフレームに「シンボルインスタンスが1つだけ」存在すること。
pub(crate) fn timeline_toggle_tween(app: &mut Is2dApp) -> bool {
    let targets = app.timeline.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    let prev_selected_ids = app.canvas.selected_shape_ids();
    crate::canvas_tool::flush_canvas_before_timeline_op(app);

    let (can_toggle, is_currently_tweened) = app.timeline.can_toggle_tween_at_current_frame();
    if !can_toggle {
        app.canvas
            .restore_selection_by_shape_ids(&prev_selected_ids);
        app.notify_warning(
            "トゥイーンを設定するには、キーフレームに単一のシンボルインスタンス（F8）のみを配置してください",
        );
        return false;
    }

    app.push_undo();
    let curr_f = app.timeline.current_frame;
    let next_tween = !is_currently_tweened;
    let mut changed = false;

    for &l_idx in &targets {
        let layer = &mut app.timeline.layers[l_idx];
        if let Some(kf_idx) = layer.keyframe_index_at(curr_f) {
            let kf = &mut layer.keyframes[kf_idx];
            if next_tween {
                if kf.cel.single_symbol_instance().is_some() && !kf.tween {
                    kf.tween = true;
                    changed = true;
                }
            } else if kf.tween {
                kf.tween = false;
                changed = true;
            }
        }
    }

    if changed {
        crate::canvas_tool::load_shapes_from_timeline(app);
        crate::canvas_tool::select_shapes_for_timeline_selection(app);
        app.ui.inspect_timeline_frame = true;
    }
    changed
}

/// 現在のアクティブレイヤー・現在フレームが属するトゥイーンキーフレームの回転方向 `tween_rotate` を設定する
pub(crate) fn set_active_keyframe_tween_rotate(
    app: &mut Is2dApp,
    rotate_mode: TweenRotate,
) -> bool {
    let Some(layer) = app.timeline.layers.get(app.timeline.active_layer) else {
        return false;
    };
    if layer.locked || !layer.visible {
        return false;
    }
    let Some(kf_idx) = layer.keyframe_index_at(app.timeline.current_frame) else {
        return false;
    };
    if !layer.keyframes[kf_idx].tween || layer.keyframes[kf_idx].tween_rotate == rotate_mode {
        return false;
    }

    crate::canvas_tool::mutate_timeline_with_undo(app, |tl| {
        if let Some(layer_mut) = tl.layers.get_mut(tl.active_layer) {
            if let Some(kf) = layer_mut.keyframes.get_mut(kf_idx) {
                kf.tween_rotate = rotate_mode;
            }
        }
    });
    crate::canvas_tool::select_shapes_for_timeline_selection(app);
    app.ui.inspect_timeline_frame = true;
    true
}

/// 現在選択されている図形（またはアクティブセル上の単一シンボル）について、その `(SymbolInstanceBinding, シンボル名, シンボル総フレーム数)` を返す
pub(crate) fn selected_single_symbol_info(
    app: &Is2dApp,
) -> Option<(SymbolInstanceBinding, String, usize)> {
    let binding = app.canvas.selected_single_symbol_binding().or_else(|| {
        let layer = app.timeline.layers.get(app.timeline.active_layer)?;
        if !layer.visible || layer.locked {
            return None;
        }
        layer
            .cel_at(app.timeline.current_frame)?
            .single_symbol_instance()
    })?;
    let sym = app.symbols.find_symbol(binding.symbol_id)?;
    Some((binding, sym.name.clone(), sym.timeline.total_frames.max(1)))
}

/// 現在選択されている単一シンボルインスタンスの再生モード（ループ再生 / 1回再生 / 任意のフレーム）および
/// 開始/指定フレーム番号（0始まり）を更新し、現在フレームのキャンバス表示と選択状態を即時反映する
pub(crate) fn set_selected_symbol_playback(
    app: &mut Is2dApp,
    loop_mode: SymbolLoopMode,
    first_frame: usize,
) -> bool {
    crate::canvas_tool::stop_playback(app);
    let Some((binding, _, total_frames)) = selected_single_symbol_info(app) else {
        return false;
    };
    let clamped_first = first_frame.min(total_frames.saturating_sub(1));
    if binding.loop_mode == loop_mode && binding.first_frame == clamped_first {
        return false;
    }

    let inst_gid = binding.instance_gid;
    let sym_id = binding.symbol_id;
    crate::tool_select::clear_selection(app);
    crate::canvas_tool::sync_shapes_to_timeline(app);
    if !app.ui.editing_symbol_playback {
        app.push_undo();
    }

    if app.symbols.find_symbol(sym_id).is_none() {
        return false;
    }

    let curr_f = app.timeline.current_frame;
    let mut updated = false;
    for layer in &mut app.timeline.layers {
        if !layer.visible || layer.locked {
            continue;
        }
        let Some(kf_idx) = layer.keyframe_index_at(curr_f) else {
            continue;
        };
        let kf = &mut layer.keyframes[kf_idx];
        if kf.cel.update_symbol_instances_with(|mut b| {
            (b.instance_gid == inst_gid).then(|| {
                b.loop_mode = loop_mode;
                b.first_frame = clamped_first;
                b
            })
        }) {
            updated = true;
        }
    }

    if updated {
        app.symbols.sync_active_edit(&app.timeline);
        crate::canvas_tool::load_shapes_from_timeline(app);
        app.canvas.select_symbol_instance_by_gid(inst_gid);
    }
    updated
}

/// シンボルインスタンスをダブルクリックして、そのシンボル専用の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_timeline(app: &mut Is2dApp, binding: SymbolInstanceBinding) -> bool {
    let Some(sym_def) = app.symbols.find_symbol(binding.symbol_id).cloned() else {
        return false;
    };

    crate::canvas_tool::flush_canvas_before_timeline_op(app);

    // 親シーン（およびさらに外側の親シンボル）の現在フレーム図形のうち、今回入るインスタンス以外の図形を背景用に退避
    let mut backdrop_shapes: Vec<FillShape> = app
        .symbols
        .edit_stack
        .last()
        .map(|s| {
            resolve_cel_with_library(
                &s.backdrop_cel,
                s.parent_timeline.current_frame,
                s.parent_timeline.current_frame,
                SymbolEvalCtx::new(&app.symbols.library),
            )
        })
        .unwrap_or_default();
    let curr_f = app.timeline.current_frame;
    for layer_idx in 0..app.timeline.layers.len() {
        let layer = &app.timeline.layers[layer_idx];
        if !layer.visible {
            continue;
        }
        let kf_start = layer
            .keyframe_index_at(curr_f)
            .map(|ki| layer.keyframes[ki].start_frame)
            .unwrap_or(curr_f);
        let resolved =
            app.timeline
                .evaluate_layer_shapes_at(layer_idx, curr_f, &app.symbols.library);
        for mut s in resolved {
            if s.symbol_instance
                .is_some_and(|b| b.instance_gid == binding.instance_gid)
            {
                continue;
            }
            if let Some(mut b) = s.symbol_instance {
                if let Some(sym) = app.symbols.find_symbol(b.symbol_id) {
                    b.first_frame = sym.resolve_internal_frame(b, kf_start, curr_f);
                    b.loop_mode = SymbolLoopMode::SingleFrame;
                    s.symbol_instance = Some(b);
                }
            }
            backdrop_shapes.push(s);
        }
    }

    // シンボル定義のローカルレイヤー群をインスタンスのステージ変換 `binding.xform` で展開して作業タイムラインを構築
    let (sym_layers, stage_to_local_cache) =
        transform_layers_with_local_cache(&sym_def.timeline.layers, binding.xform);
    let sym_timeline = app.timeline.new_for_symbol_edit(&sym_def, sym_layers);
    app.symbols.selected_library_symbol = Some(sym_def.id);

    let parent_timeline = std::mem::replace(&mut app.timeline, sym_timeline);
    let parent_editing_group_path = std::mem::take(&mut app.canvas.editing_group_path);
    app.symbols.edit_stack.push(SymbolEditSession {
        symbol_id: binding.symbol_id,
        instance_gid: binding.instance_gid,
        instance_xform: binding.xform,
        parent_timeline,
        parent_editing_group_path,
        backdrop_cel: Cel::new(backdrop_shapes),
        stage_to_local_cache,
    });
    app.symbols.bump_revision();
    app.timeline_ui
        .reset_for_timeline_switch(app.timeline.current_frame);

    crate::canvas_tool::load_shapes_from_timeline(app);
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool {
    crate::canvas_tool::stop_playback(app);
    if app
        .symbols
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == symbol_id)
    {
        return false;
    }
    let has_instance_in_current_timeline = app
        .canvas
        .shapes
        .iter()
        .any(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == symbol_id))
        || app.timeline.layers.iter().any(|l| {
            l.keyframes.iter().any(|kf| {
                kf.cel
                    .instances
                    .iter()
                    .any(|inst| inst.binding.symbol_id == symbol_id)
            })
        });
    if !app.symbols.edit_stack.is_empty() && !has_instance_in_current_timeline {
        while !app.symbols.edit_stack.is_empty() {
            exit_symbol_timeline(app);
        }
    }
    let curr_f = app.timeline.current_frame;
    let binding =
        app.canvas
            .shapes
            .iter()
            .find_map(|s| s.symbol_instance.filter(|b| b.symbol_id == symbol_id))
            .or_else(|| {
                app.timeline.layers.iter().find_map(|l| {
                    l.cel_at(curr_f)?.instances.iter().find_map(|inst| {
                        (inst.binding.symbol_id == symbol_id).then_some(inst.binding)
                    })
                })
            })
            .unwrap_or_else(|| {
                let stage_center = app.viewport.stage_center_world();
                SymbolInstanceBinding::new(
                    0,
                    symbol_id,
                    Affine::translate(KVec2::new(stage_center.x, stage_center.y)),
                )
            });
    enter_symbol_timeline(app, binding)
}

/// 現在入っているシンボル内部タイムライン編集モードから1段階上の親シーン（または親シンボル）タイムラインへ戻る
pub(crate) fn exit_symbol_timeline(app: &mut Is2dApp) -> bool {
    if app.symbols.edit_stack.is_empty() {
        return false;
    }
    crate::canvas_tool::flush_canvas_before_timeline_op(app);

    let Some(mut session) = app.symbols.edit_stack.pop() else {
        return false;
    };
    session.parent_timeline.fps = app.timeline.fps;
    let sym_timeline = std::mem::replace(&mut app.timeline, session.parent_timeline);
    app.canvas.editing_group_path = session.parent_editing_group_path;
    app.timeline_ui
        .reset_for_timeline_switch(app.timeline.current_frame);

    if session.instance_xform.determinant().abs() > 1e-9 {
        let inv_xform = session.instance_xform.inverse();
        if let Some(sym_def) = app.symbols.find_symbol_mut(session.symbol_id) {
            sym_def.update_from_working_timeline(
                &sym_timeline,
                inv_xform,
                &mut session.stage_to_local_cache,
            );
        }
    }
    app.symbols.bump_revision();

    // 親シンボル編集中であればライブラリへも同期し、親タイムラインの図形を再展開する（Cel正規化により全インスタンスのshapes再構築は不要）
    app.symbols.sync_active_edit(&app.timeline);
    crate::canvas_tool::load_shapes_from_timeline(app);

    // 戻った親階層で元のシンボルインスタンスを選択状態にする
    app.canvas
        .select_symbol_instance_by_gid(session.instance_gid);
    true
}
