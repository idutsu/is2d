use eframe::egui::{self, Color32, Stroke, Vec2};
use kurbo::{Affine, Point, Rect as KRect, Vec2 as KVec2};

use crate::canvas_tool::{ScaleHandle, TransformDragState, TransformHoverTarget};
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
    pub(crate) fn active_transform_pivot(&self, bbox: KRect) -> Point {
        self.transform_pivot.unwrap_or_else(|| bbox.center())
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

    /// 選択中のオブジェクトを現在の基準点（ピボット）中心にアフィン変換する
    pub(crate) fn apply_affine_to_selected(&mut self, affine: Affine) -> bool {
        if self.selected_shapes.is_empty() {
            return false;
        }
        let undo_snapshot = self.shapes.clone();
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

    /// 選択中のオブジェクトを左右反転（`flip_x`）または上下反転（`flip_y`）する
    pub(crate) fn flip_selected_shapes(&mut self, flip_x: bool, flip_y: bool) -> bool {
        let Some(bbox) = self.selection_bounding_box() else {
            return false;
        };
        let pivot = self.active_transform_pivot(bbox);
        let sx = if flip_x { -1.0 } else { 1.0 };
        let sy = if flip_y { -1.0 } else { 1.0 };
        self.apply_affine_to_selected(affine_scale_around(pivot, sx, sy))
    }

    /// 選択中のオブジェクトを現在の基準点（ピボット）中心に `angle_rad` 回転する
    pub(crate) fn rotate_selected_shapes(&mut self, angle_rad: f64) -> bool {
        let Some(bbox) = self.selection_bounding_box() else {
            return false;
        };
        let pivot = self.active_transform_pivot(bbox);
        self.apply_affine_to_selected(affine_rotate_around(pivot, angle_rad))
    }

    /// 選択中のオブジェクトを現在の基準点（ピボット）中心に `(sx, sy)` 倍に拡大・縮小する
    #[cfg(test)]
    pub(crate) fn scale_selected_shapes(&mut self, sx: f64, sy: f64) -> bool {
        let Some(bbox) = self.selection_bounding_box() else {
            return false;
        };
        let pivot = self.active_transform_pivot(bbox);
        self.apply_affine_to_selected(affine_scale_around(pivot, sx, sy))
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
                    self.transform_pivot = None;
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
        if shift_held && self.compute_transform_hover(pt) == TransformHoverTarget::None {
            self.transform_drag = TransformDragState::MarqueeSelect {
                start_pos: pt,
                current_pos: pt,
                additive: true,
            };
            return;
        }

        match self.compute_transform_hover(pt) {
            TransformHoverTarget::Pivot => {
                self.transform_drag = TransformDragState::MovePivot;
            }
            TransformHoverTarget::Scale(handle) => {
                if let Some(orig_bbox) = self.selection_bounding_box() {
                    let orig_pivot = self.active_transform_pivot(orig_bbox);
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
                        undo_snapshot: self.shapes.clone(),
                        changed: false,
                    };
                }
            }
            TransformHoverTarget::Rotate => {
                if let Some(orig_bbox) = self.selection_bounding_box() {
                    let pivot = self.active_transform_pivot(orig_bbox);
                    self.transform_drag = TransformDragState::Rotate {
                        orig_bbox,
                        pivot,
                        start_angle: (pt.y - pivot.y).atan2(pt.x - pivot.x),
                        current_angle: 0.0,
                        orig_shapes: self.selected_shapes_snapshot(),
                        undo_snapshot: self.shapes.clone(),
                    };
                }
            }
            TransformHoverTarget::Move => {
                self.transform_drag = TransformDragState::Move {
                    last_pos: pt,
                    total_delta: KVec2::ZERO,
                    undo_snapshot: self.shapes.clone(),
                };
            }
            TransformHoverTarget::None => {
                if let Some(hit_idx) = self.topmost_shape_at(pt) {
                    if !self.selected_shapes.contains(&hit_idx) {
                        self.select_single_shape_at(pt);
                    }
                    self.transform_drag = TransformDragState::Move {
                        last_pos: pt,
                        total_delta: KVec2::ZERO,
                        undo_snapshot: self.shapes.clone(),
                    };
                } else {
                    self.transform_drag = TransformDragState::MarqueeSelect {
                        start_pos: pt,
                        current_pos: pt,
                        additive: false,
                    };
                }
            }
        }
    }

    fn update_transform_drag(&mut self, pt: Point, shift_held: bool, alt_held: bool) {
        let mut move_delta: Option<KVec2> = None;
        match &mut self.transform_drag {
            TransformDragState::MovePivot => {
                self.transform_pivot = Some(pt);
            }
            TransformDragState::Move {
                last_pos,
                total_delta,
                ..
            } => {
                let step_delta = pt - *last_pos;
                *last_pos = pt;
                *total_delta += step_delta;
                move_delta = Some(step_delta);
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
                let mut sx = if handle.affects_x() {
                    let dx0 = handle_start.x - eff_anchor.x;
                    if dx0.abs() > 1e-3 {
                        (pt.x - eff_anchor.x) / dx0
                    } else {
                        1.0
                    }
                } else {
                    1.0
                };
                let mut sy = if handle.affects_y() {
                    let dy0 = handle_start.y - eff_anchor.y;
                    if dy0.abs() > 1e-3 {
                        (pt.y - eff_anchor.y) / dy0
                    } else {
                        1.0
                    }
                } else {
                    1.0
                };

                if shift_held && handle.is_corner() {
                    let mag = sx.abs().max(sy.abs());
                    sx = mag.copysign(sx);
                    sy = mag.copysign(sy);
                }
                if sx.abs() < 0.02 {
                    sx = 0.02_f64.copysign(sx);
                }
                if sy.abs() < 0.02 {
                    sy = 0.02_f64.copysign(sy);
                }

                let affine = affine_scale_around(eff_anchor, sx, sy);
                for (sel_idx, orig_shape) in orig_shapes.iter() {
                    if let Some(shape) = self.shapes.get_mut(*sel_idx) {
                        *shape = orig_shape.clone();
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
                            *shape = orig_shape.clone();
                            shape.transform(affine);
                        }
                    }
                }
            }
            TransformDragState::MarqueeSelect { current_pos, .. } => {
                *current_pos = pt;
            }
            TransformDragState::None => {}
        }
        if let Some(delta) = move_delta {
            self.translate_selected_shapes(delta);
        }
    }

    fn finish_transform_drag(&mut self, mouse_pt: Option<Point>) {
        let finished = std::mem::replace(&mut self.transform_drag, TransformDragState::None);
        match finished {
            TransformDragState::Move {
                total_delta,
                undo_snapshot,
                ..
            } => {
                self.finish_move_selection_drag(total_delta, undo_snapshot);
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
            TransformDragState::MarqueeSelect {
                start_pos,
                current_pos,
                additive,
            } => {
                let end_pos = mouse_pt.unwrap_or(current_pos);
                self.apply_marquee_selection(start_pos, end_pos, additive);
            }
            TransformDragState::MovePivot | TransformDragState::None => {}
        }
    }

    fn handle_transform_click(&mut self, pt: Point, shift_held: bool) {
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
        let piv_active = matches!(self.transform_drag, TransformDragState::MovePivot)
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
