use eframe::egui::{self, Color32};
use kurbo::Point;

use crate::canvas_tool::Tool;
use crate::vector_engine::{
    build_smoothed_stroke_shapes_at_zoom, erase_from_canvas, find_enclosed_region_at,
    generate_brush_outline_at_zoom, merge_disjoint_fill_into_canvas, merge_shapes_into_canvas,
    FillShape,
};
use crate::FlashBrushApp;

// ============================================================================
// ブラシ（B）・消しゴム（E）・バケツ塗り（K）・スポイト（I）の操作ロジック
// ============================================================================

impl FlashBrushApp {
    pub(crate) fn handle_brush_or_eraser_input(
        &mut self,
        response: &egui::Response,
        mouse_pt: Option<Point>,
    ) {
        let base_r = if self.tool == Tool::Brush {
            self.brush_radius as f64
        } else {
            self.eraser_radius as f64
        };
        let z = (self.view_zoom as f64).clamp(0.15, 16.0);
        let screen_r = base_r * z;
        let min_sample_dist = (0.25 * screen_r).clamp(0.4, 1.5) / z;

        if response.drag_started() {
            if let Some(pt) = mouse_pt {
                self.active_stroke.clear();
                self.active_stroke.push(pt);
                self.active_raw_outline =
                    generate_brush_outline_at_zoom(&self.active_stroke, base_r, z);
            }
        } else if response.dragged() {
            if let Some(pt) = mouse_pt {
                if self
                    .active_stroke
                    .last()
                    .is_none_or(|&last| (pt - last).hypot() >= min_sample_dist)
                {
                    self.active_stroke.push(pt);
                    self.active_raw_outline =
                        generate_brush_outline_at_zoom(&self.active_stroke, base_r, z);
                }
            }
        } else if response.drag_stopped() && !self.active_raw_outline.is_empty() {
            self.clear_selection();
            self.push_undo();
            self.active_stroke.clear();
            let raw = std::mem::take(&mut self.active_raw_outline);
            self.commit_raw_outline(&raw);
        } else if response.clicked() {
            if let Some(pt) = mouse_pt {
                self.clear_selection();
                self.push_undo();
                let raw = generate_brush_outline_at_zoom(&[pt], base_r, z);
                self.commit_raw_outline(&raw);
            }
        }
    }


    pub(crate) fn commit_raw_outline(&mut self, raw: &[Point]) {
        let target_lid = self.active_scope_layer_id();
        let stroke_shapes: Vec<FillShape> =
            build_smoothed_stroke_shapes_at_zoom(raw, self.brush_color, self.view_zoom as f64)
                .into_iter()
                .map(|s| s.with_scope(target_lid, &self.editing_group_path))
                .collect();
        if self.tool == Tool::Brush {
            merge_shapes_into_canvas(&mut self.shapes, stroke_shapes);
            self.sort_shapes_by_layer_order();
        } else {
            erase_from_canvas(&mut self.shapes, &stroke_shapes);
        }
    }

    /// バケツツール（色流し込み）
    pub(crate) fn bucket_fill_at(&mut self, pt: Point) -> bool {
        let fill_color = self.brush_color;

        // 0. 選択領域が存在する場合：選択中の「生シェイプ」のみにバケツで色を流し込む（グループは無効）
        if !self.selected_shapes.is_empty() {
            let clicked_selected_raw = self.selected_shapes.iter().copied().any(|idx| {
                self.shapes.get(idx).is_some_and(|s| {
                    s.is_raw_at_scope(&self.editing_group_path) && s.contains_point(pt)
                })
            });
            if clicked_selected_raw {
                let any_color_diff = self.selected_shapes.iter().copied().any(|idx| {
                    self.shapes.get(idx).is_some_and(|s| {
                        s.is_raw_at_scope(&self.editing_group_path) && s.color != fill_color
                    })
                });
                if !any_color_diff {
                    return false;
                }
                let snapshot = self.take_or_clone_unsliced_snapshot();
                for &sel_idx in &self.selected_shapes {
                    if let Some(shape) = self.shapes.get_mut(sel_idx) {
                        if shape.is_raw_at_scope(&self.editing_group_path) {
                            shape.color = fill_color;
                            shape.rebuild_cache();
                        }
                    }
                }
                self.selection_modified = true;
                self.record_undo_snapshot(snapshot);
                return true;
            }
            return false;
        }

        // 1. 既存の生シェイプ上をクリックした場合：色の塗り替え
        if let Some(hit_idx) = self.topmost_shape_at(pt) {
            if !self.shapes[hit_idx].is_raw_at_scope(&self.editing_group_path)
                || self.shapes[hit_idx].color == fill_color
            {
                return false;
            }
            self.focus_active_layer_to_shape(hit_idx);
            let snapshot = self.shapes.clone();
            let mut target = self.shapes.remove(hit_idx);
            target.color = fill_color;
            target.rebuild_cache();
            merge_disjoint_fill_into_canvas(&mut self.shapes, target);
            self.sort_shapes_by_layer_order();
            self.record_undo_snapshot(snapshot);
            return true;
        }

        // 2. 現在のスコープ直下の生シェイプ群に囲まれた空白（閉領域）をクリックした場合：ベクター流し込み
        let target_lid = self.active_scope_layer_id();
        let active_layer_raw: Vec<FillShape> = self
            .shapes
            .iter()
            .filter(|s| s.layer_id == target_lid && s.is_raw_at_scope(&self.editing_group_path))
            .cloned()
            .collect();
        let enclosed_opt = find_enclosed_region_at(&active_layer_raw, pt, fill_color).or_else(|| {
            let all_raw_scope: Vec<FillShape> = self
                .shapes
                .iter()
                .filter(|s| s.is_raw_at_scope(&self.editing_group_path))
                .cloned()
                .collect();
            find_enclosed_region_at(&all_raw_scope, pt, fill_color)
        });
        if let Some(enclosed_fill) = enclosed_opt {
            let snapshot = self.shapes.clone();
            merge_disjoint_fill_into_canvas(
                &mut self.shapes,
                enclosed_fill.with_scope(target_lid, &self.editing_group_path),
            );
            self.sort_shapes_by_layer_order();
            self.record_undo_snapshot(snapshot);
            return true;
        }

        false
    }

    /// スポイトツール（色抽出）：指定位置 `pt` の最前面シェイプの色を抽出し、`self.brush_color` に設定する
    pub(crate) fn pick_color_at(&mut self, pt: Point) -> Option<Color32> {
        let picked = self.topmost_visible_color_at(pt)?;
        self.brush_color = picked;
        Some(picked)
    }

}
