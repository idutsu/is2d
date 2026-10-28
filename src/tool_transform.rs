use eframe::egui::{self, Color32, Stroke, Vec2};
use kurbo::{Affine, Point, Rect as KRect, Vec2 as KVec2};

use crate::canvas_tool::{MarqueeDrag, ScaleHandle, TransformDragState, TransformHoverTarget};
use crate::vector_engine::{affine_rotate_around, affine_scale_around};
use crate::FlashBrushApp;

// ============================================================================
// 自由変形ツール（Tool::Transform / Q キー）の操作・アフィン変換・変形枠描画
// ============================================================================

impl FlashBrushApp {
    // ------------------------------------------------------------------------
    // 3. 自由変形・アフィン変換・ブラシ / 消しゴム / バケツ / スポイト操作
    // ------------------------------------------------------------------------

    /// 自由変形ツールの現在の基準点（ピボット座標）を返す
    /// （単一シンボルインスタンス選択時はインスタンスに保存されたピボット座標 `binding.world_pivot()` を返す）
    pub(crate) fn active_transform_pivot(&self, bbox: KRect) -> Point {
        if let Some(piv) = self.transform_pivot {
            return piv;
        }
        if let Some((binding, _, _)) = self.selected_single_symbol_info() {
            return binding.world_pivot();
        }
        bbox.center()
    }

    /// 現在の選択対象に初期位置から移動されたカスタム変形中心（ピボット）が設定されているかを返す
    pub(crate) fn has_custom_transform_pivot(&self) -> bool {
        if self.transform_pivot.is_some() {
            return true;
        }
        self.selected_single_symbol_info()
            .is_some_and(|(b, _, _)| (b.pivot - Point::ZERO).hypot() > 1e-6)
    }

    /// 単一のシンボルインスタンスが選択されている場合、ワールド座標 `world_pivot`（`None` なら初期基準点 `(0, 0)`）を
    /// シンボルローカル座標に変換してインスタンスの `binding.pivot` に保存する
    pub(crate) fn set_selected_symbol_world_pivot(
        &mut self,
        world_pivot: Option<Point>,
        commit_to_timeline: bool,
    ) -> bool {
        let Some((binding, _, _)) = self.selected_single_symbol_info() else {
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
        if commit_to_timeline {
            let undo_snapshot = self.shapes.clone();
            self.record_undo_snapshot(undo_snapshot);
        }
        let inst_gid = binding.instance_gid;
        for shape in &mut self.shapes {
            if let Some(b) = shape
                .symbol_instance
                .as_mut()
                .filter(|b| b.instance_gid == inst_gid)
            {
                b.pivot = new_local_pivot;
            }
        }
        if commit_to_timeline {
            self.sync_shapes_to_timeline();
        }
        true
    }

    /// 選択オブジェクトの変形中心（ピボット）を初期位置（選択中央、またはシンボルの基準点 `(0, 0)`）へリセットする
    pub(crate) fn reset_selected_transform_pivot(&mut self) -> bool {
        let mut changed = self.transform_pivot.take().is_some();
        if self.set_selected_symbol_world_pivot(None, true) {
            changed = true;
        }
        changed
    }

    /// 自由変形ツール（`Q`）でのマウス位置のヒット判定
    pub(crate) fn compute_transform_hover(&self, mouse_pt: Point) -> TransformHoverTarget {
        let Some(bbox) = self.selection_bounding_box() else {
            return TransformHoverTarget::None;
        };
        let inv_z = 1.0 / (self.view_zoom as f64);
        let pivot = self.active_transform_pivot(bbox);
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
            || self
                .selected_shapes
                .iter()
                .any(|&idx| self.shapes.get(idx).is_some_and(|s| s.contains_point(mouse_pt)))
        {
            return TransformHoverTarget::Move;
        }

        TransformHoverTarget::None
    }

    fn flush_transform_pivot_to_symbol(&mut self) {
        if let Some(piv) = self.transform_pivot {
            if self.set_selected_symbol_world_pivot(Some(piv), false) {
                self.transform_pivot = None;
            }
        }
    }

    /// 選択中のオブジェクトを現在の基準点（ピボット）中心にアフィン変換する
    pub(crate) fn apply_affine_to_selected(&mut self, affine: Affine) -> bool {
        if self.selected_shapes.is_empty() {
            return false;
        }
        let undo_snapshot = self.shapes.clone();
        self.flush_transform_pivot_to_symbol();
        let mut any_transformed = false;
        for &sel_idx in &self.selected_shapes {
            if let Some(shape) = self.shapes.get_mut(sel_idx) {
                shape.transform(affine);
                any_transformed = true;
            }
        }
        if any_transformed {
            self.commit_moved_selection(undo_snapshot);
        }
        any_transformed
    }

    /// 選択中のオブジェクトを現在の基準点（ピボット）中心に `(sx, sy)` 倍に拡大・縮小（または反転）する
    pub(crate) fn scale_selected_shapes(&mut self, sx: f64, sy: f64) -> bool {
        let Some(bbox) = self.selection_bounding_box() else {
            return false;
        };
        let pivot = self.active_transform_pivot(bbox);
        self.apply_affine_to_selected(affine_scale_around(pivot, sx, sy))
    }

    /// 選択中のオブジェクトを左右反転（`flip_x`）または上下反転（`flip_y`）する
    pub(crate) fn flip_selected_shapes(&mut self, flip_x: bool, flip_y: bool) -> bool {
        self.scale_selected_shapes(
            if flip_x { -1.0 } else { 1.0 },
            if flip_y { -1.0 } else { 1.0 },
        )
    }

    /// 選択中のオブジェクトを現在の基準点（ピボット）中心に `angle_rad` 回転する
    pub(crate) fn rotate_selected_shapes(&mut self, angle_rad: f64) -> bool {
        let Some(bbox) = self.selection_bounding_box() else {
            return false;
        };
        let pivot = self.active_transform_pivot(bbox);
        self.apply_affine_to_selected(affine_rotate_around(pivot, angle_rad))
    }


    /// 自由変形ツール（`Tool::Transform` / `Q`）のマウス入力処理
    pub(crate) fn handle_transform_tool_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        mouse_pt: Option<Point>,
    ) {
        let shift_held = ui.input(|i| i.modifiers.shift);
        let alt_held = ui.input(|i| i.modifiers.alt);

        if response.double_clicked() {
            if let Some(pt) = mouse_pt {
                if self.compute_transform_hover(pt) == TransformHoverTarget::Pivot {
                    self.reset_selected_transform_pivot();
                } else if !self.enter_group_at(pt) {
                    self.exit_current_group();
                }
            }
            return;
        }

        if response.drag_started() {
            if let Some(pt) = mouse_pt {
                self.start_transform_drag(pt, shift_held, alt_held);
            }
        } else if response.dragged() {
            if let Some(pt) = mouse_pt {
                self.update_transform_drag(pt, shift_held, alt_held);
            }
        } else if response.drag_stopped() {
            self.finish_transform_drag(mouse_pt);
        } else if response.clicked() {
            if let Some(pt) = mouse_pt {
                self.handle_transform_click(pt, shift_held);
            }
        }

        if matches!(self.transform_drag, TransformDragState::None) {
            self.transform_hover = mouse_pt
                .map(|pt| self.compute_transform_hover(pt))
                .unwrap_or(TransformHoverTarget::None);
        }
    }

    fn start_transform_drag(&mut self, pt: Point, shift_held: bool, alt_held: bool) {
        self.inspect_timeline_frame = false;
        if shift_held && self.compute_transform_hover(pt) == TransformHoverTarget::None {
            self.transform_drag = TransformDragState::MarqueeSelect(MarqueeDrag::new(pt, true));
            return;
        }

        match self.compute_transform_hover(pt) {
            TransformHoverTarget::Pivot => {
                if let Some(bbox) = self.selection_bounding_box() {
                    let orig_pivot = self.active_transform_pivot(bbox);
                    self.transform_drag = TransformDragState::MovePivot {
                        orig_pivot,
                        undo_snapshot: self.shapes.clone(),
                        changed: false,
                    };
                }
            }
            TransformHoverTarget::Scale(handle) => {
                if let Some(orig_bbox) = self.selection_bounding_box() {
                    let undo_snapshot = self.shapes.clone();
                    let orig_pivot = self.active_transform_pivot(orig_bbox);
                    self.flush_transform_pivot_to_symbol();
                    let anchor = if alt_held {
                        orig_pivot
                    } else {
                        handle.opposite_position(orig_bbox)
                    };
                    self.transform_drag = TransformDragState::Scale {
                        handle,
                        anchor,
                        handle_start: handle.position(orig_bbox),
                        orig_pivot,
                        orig_shapes: self.selected_shapes_snapshot(),
                        undo_snapshot,
                        changed: false,
                    };
                }
            }
            TransformHoverTarget::Rotate => {
                if let Some(orig_bbox) = self.selection_bounding_box() {
                    let undo_snapshot = self.shapes.clone();
                    let pivot = self.active_transform_pivot(orig_bbox);
                    self.flush_transform_pivot_to_symbol();
                    self.transform_drag = TransformDragState::Rotate {
                        orig_bbox,
                        pivot,
                        start_angle: (pt.y - pivot.y).atan2(pt.x - pivot.x),
                        current_angle: 0.0,
                        orig_shapes: self.selected_shapes_snapshot(),
                        undo_snapshot,
                    };
                }
            }
            TransformHoverTarget::Move => {
                self.transform_drag =
                    TransformDragState::Move(self.begin_move_selection_drag(pt, alt_held));
            }
            TransformHoverTarget::None => {
                if let Some(hit_idx) = self
                    .topmost_shape_at(pt)
                    .or_else(|| self.compute_hover_target(pt).shape_idx())
                {
                    if !self.selected_shapes.contains(&hit_idx) {
                        self.select_single_shape_at(pt);
                    }
                    self.transform_drag =
                        TransformDragState::Move(self.begin_move_selection_drag(pt, alt_held));
                } else {
                    self.transform_drag =
                        TransformDragState::MarqueeSelect(MarqueeDrag::new(pt, false));
                }
            }
        }
    }

    fn update_transform_drag(&mut self, pt: Point, shift_held: bool, alt_held: bool) {
        let mut move_delta: Option<KVec2> = None;
        let mut new_pivot_pt: Option<Point> = None;
        match &mut self.transform_drag {
            TransformDragState::MovePivot {
                orig_pivot,
                changed,
                ..
            } => {
                if (pt - *orig_pivot).hypot() > 0.5 / (self.view_zoom as f64) {
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
                orig_shapes,
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
                for (sel_idx, orig_shape) in orig_shapes.iter() {
                    if let Some(shape) = self.shapes.get_mut(*sel_idx) {
                        shape.clone_from(orig_shape);
                        shape.transform(affine);
                    }
                }
                if self.transform_pivot.is_some() {
                    self.transform_pivot = Some(affine * *orig_pivot);
                }
                *changed = (sx - 1.0).abs() > 0.005 || (sy - 1.0).abs() > 0.005;
            }
            TransformDragState::Rotate {
                pivot,
                start_angle,
                current_angle,
                orig_shapes,
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
                    let affine = affine_rotate_around(*pivot, delta_angle);
                    for (sel_idx, orig_shape) in orig_shapes.iter() {
                        if let Some(shape) = self.shapes.get_mut(*sel_idx) {
                            shape.clone_from(orig_shape);
                            shape.transform(affine);
                        }
                    }
                }
            }
            TransformDragState::MarqueeSelect(m) => {
                m.current_pos = pt;
            }
            TransformDragState::None => {}
        }
        if let Some(piv_pt) = new_pivot_pt {
            if self.set_selected_symbol_world_pivot(Some(piv_pt), false) {
                self.transform_pivot = None;
            } else {
                self.transform_pivot = Some(piv_pt);
            }
        }
        if let Some(delta) = move_delta {
            self.translate_selected_shapes(delta);
        }
    }

    fn finish_transform_drag(&mut self, mouse_pt: Option<Point>) {
        let finished = std::mem::replace(&mut self.transform_drag, TransformDragState::None);
        match finished {
            TransformDragState::Move(drag) => {
                self.finish_move_selection_drag(drag.total_delta, drag.undo_snapshot);
            }
            TransformDragState::Scale {
                undo_snapshot,
                changed,
                ..
            } => {
                if changed && !self.selected_shapes.is_empty() {
                    self.commit_moved_selection(undo_snapshot);
                } else {
                    self.shapes = undo_snapshot;
                }
            }
            TransformDragState::Rotate {
                current_angle,
                undo_snapshot,
                ..
            } => {
                if current_angle.abs() > 0.002 && !self.selected_shapes.is_empty() {
                    self.commit_moved_selection(undo_snapshot);
                } else {
                    self.shapes = undo_snapshot;
                }
            }
            TransformDragState::MarqueeSelect(m) => {
                let end_pos = mouse_pt.unwrap_or(m.current_pos);
                self.apply_marquee_selection(m.start_pos, end_pos, m.additive);
            }
            TransformDragState::MovePivot {
                undo_snapshot,
                changed,
                ..
            } => {
                if changed {
                    if self.selected_single_symbol_info().is_some() {
                        self.record_undo_snapshot(undo_snapshot);
                        self.sync_shapes_to_timeline();
                    }
                } else {
                    self.shapes = undo_snapshot;
                }
            }
            TransformDragState::None => {}
        }
    }

    fn handle_transform_click(&mut self, pt: Point, shift_held: bool) {
        self.inspect_timeline_frame = false;
        if shift_held {
            if let Some(hit_idx) = self.topmost_shape_at(pt) {
                self.toggle_shape_selection(hit_idx);
            }
        } else if self.compute_transform_hover(pt) != TransformHoverTarget::Pivot {
            if let Some(hit_idx) = self.topmost_shape_at(pt) {
                if !self.selected_shapes.contains(&hit_idx) {
                    self.select_single_shape_at(pt);
                }
            } else {
                self.clear_selection();
            }
        }
    }

    /// 自由変形ツール（`Tool::Transform`）のバウンディングボックス・8ハンドル・基準点（ピボット）を描画
    pub(crate) fn draw_free_transform_box(&self, painter: &egui::Painter) {
        if self.selected_shapes.is_empty() {
            return;
        }

        let (base_bbox, rot_affine, pivot) = if let TransformDragState::Rotate {
            orig_bbox,
            pivot,
            current_angle,
            ..
        } = &self.transform_drag
        {
            (
                *orig_bbox,
                affine_rotate_around(*pivot, *current_angle),
                *pivot,
            )
        } else if let Some(bbox) = self.selection_bounding_box() {
            let piv = self.active_transform_pivot(bbox);
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

        let box_stroke = Stroke::new(1.5, Color32::from_rgb(20, 115, 225));
        for i in 0..4 {
            painter.line_segment(
                [
                    self.world_to_screen(corners[i]),
                    self.world_to_screen(corners[(i + 1) % 4]),
                ],
                box_stroke,
            );
        }

        // 8個の変形ハンドル（四隅＋各辺の中央）を描画
        for handle in ScaleHandle::ALL {
            let hp = self.world_to_screen(rot_affine * handle.position(base_bbox));
            let is_active = matches!(
                self.transform_drag,
                TransformDragState::Scale { handle: h, .. } if h == handle
            ) || (matches!(self.transform_drag, TransformDragState::None)
                && self.transform_hover == TransformHoverTarget::Scale(handle));

            let half = if is_active { 4.5 } else { 3.6 };
            let rect = egui::Rect::from_center_size(hp, Vec2::splat(half * 2.0));
            let fill_col = if is_active {
                Color32::from_rgb(0, 175, 255)
            } else {
                Color32::from_rgb(25, 30, 40)
            };
            painter.rect_filled(rect, 0.0, fill_col);
            painter.rect_stroke(rect, 0.0, Stroke::new(1.5, Color32::WHITE));
        }

        // 基準点（ピボット点：白い丸＋中心ドット）を描画
        let piv_pos = self.world_to_screen(pivot);
        let piv_active = matches!(self.transform_drag, TransformDragState::MovePivot { .. })
            || (matches!(self.transform_drag, TransformDragState::None)
                && self.transform_hover == TransformHoverTarget::Pivot);
        let piv_radius = if piv_active { 5.8 } else { 4.8 };
        painter.circle_filled(piv_pos, piv_radius, Color32::WHITE);
        painter.circle_stroke(
            piv_pos,
            piv_radius,
            Stroke::new(
                1.8,
                if piv_active {
                    Color32::from_rgb(255, 110, 30)
                } else {
                    Color32::from_rgb(20, 25, 35)
                },
            ),
        );
        painter.circle_filled(piv_pos, 1.5, Color32::from_rgb(20, 25, 35));
    }

}
