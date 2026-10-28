use eframe::egui::{self, Color32, Stroke};
use kurbo::{
    ParamCurve, ParamCurveArclen, ParamCurveNearest, Point, QuadBez, Rect as KRect, Vec2 as KVec2,
};

use crate::canvas_tool::{HoverTarget, SelectDragState, SelectSubMode, Tool};
use crate::vector_engine::{
    boolean_operation, make_marquee_rect_shape, merge_shapes_into_canvas, normalize_vec,
    rects_overlap, resolve_deformed_shape, transform_mesh_to_screen, BoolOp, FillShape,
};
use crate::FlashBrushApp;

// ============================================================================
// 選択ツール（Tool::Select / V キー）・輪郭変形・アンカー編集・グループ・クリップボード
// ============================================================================

impl FlashBrushApp {
    /// 矩形スライス前のキャンバス状態があれば取り出し、なければ現在の `self.shapes` をクローンして返す
    #[inline]
    pub(crate) fn take_or_clone_unsliced_snapshot(&mut self) -> Vec<FillShape> {
        self.unsliced_canvas
            .take()
            .unwrap_or_else(|| self.shapes.clone())
    }

    /// 現在の編集スコープ（`editing_group_path`）末尾に対応する挿入インデックスを返す
    pub(crate) fn scope_insert_index(&self) -> usize {
        if self.editing_group_path.is_empty() {
            self.shapes.len()
        } else {
            self.shapes
                .iter()
                .rposition(|s| s.is_in_editing_scope(&self.editing_group_path))
                .map(|pos| pos + 1)
                .unwrap_or(self.shapes.len())
        }
    }

    /// 指定シェイプ・輪郭・セグメント番号に対応する `QuadBez` を取得する
    #[inline]
    pub(crate) fn get_contour_segment(
        &self,
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
    ) -> Option<QuadBez> {
        self.shapes
            .get(shape_idx)
            .and_then(|s| s.get_contour(contour_idx))
            .and_then(|c| c.segments.get(seg_idx))
            .copied()
    }


    // ------------------------------------------------------------------------
    // 2. 選択管理・グループ階層操作・クリップボード
    // ------------------------------------------------------------------------

    pub(crate) fn clear_selection(&mut self) {
        if let Some(unsliced) = self.unsliced_canvas.take() {
            self.shapes = unsliced;
        } else if self.selection_modified && !self.selected_shapes.is_empty() {
            let (is_sel, _) = self.selected_shape_mask();
            let mut base_canvas = Vec::with_capacity(self.shapes.len());
            let mut modified_raw_shapes = Vec::with_capacity(self.selected_shapes.len());
            for (idx, shape) in self.shapes.drain(..).enumerate() {
                // 現在のスコープ直下の生シェイプのみ選択解除時に同一レイヤー内で結合・切り抜き（Merge Drawing）を行い、
                // グループ化されているオブジェクトや別レイヤーの絵は削らずにそのまま残す
                if is_sel[idx] && shape.is_raw_at_scope(&self.editing_group_path) {
                    modified_raw_shapes.push(shape);
                } else {
                    base_canvas.push(shape);
                }
            }
            while !modified_raw_shapes.is_empty() {
                let lid = modified_raw_shapes[0].layer_id;
                let c = modified_raw_shapes[0].color;
                let gp = modified_raw_shapes[0].group_path.clone();
                let mut batch = Vec::new();
                let mut remaining = Vec::new();
                for shape in modified_raw_shapes {
                    if shape.layer_id == lid && shape.color == c && shape.group_path == gp {
                        batch.push(shape);
                    } else {
                        remaining.push(shape);
                    }
                }
                merge_shapes_into_canvas(&mut base_canvas, batch);
                modified_raw_shapes = remaining;
            }
            self.shapes = base_canvas;
            self.sort_shapes_by_layer_order();
        }
        self.selection_modified = false;
        self.selected_shapes.clear();
        self.transform_pivot = None;
    }

    /// 現在の編集スコープ（`editing_group_path`）内で、指定座標 `pt` にヒットする最前面シェイプのインデックスを返す
    pub(crate) fn topmost_shape_at(&self, pt: Point) -> Option<usize> {
        (0..self.shapes.len()).rev().find(|&idx| {
            self.shapes[idx].is_in_editing_scope(&self.editing_group_path)
                && self.shapes[idx].contains_point(pt)
        })
    }

    /// 編集スコープおよび表示中の全レイヤーを対象に、指定座標 `pt` にヒットする最前面シェイプの色を返す（スポイト用）
    pub(crate) fn topmost_visible_color_at(&self, pt: Point) -> Option<Color32> {
        let num_layers = self.timeline.layers.len();
        if num_layers <= 1 {
            if let Some(hit) = self.shapes.iter().rev().find(|s| s.contains_point(pt)) {
                return Some(hit.color);
            }
        }
        for layer_idx in (0..num_layers).rev() {
            let layer = &self.timeline.layers[layer_idx];
            if !layer.visible {
                continue;
            }
            if layer.locked {
                if let Some(cel) = layer.cel_at(self.timeline.current_frame) {
                    if let Some(hit) = cel.shapes.iter().rev().find(|s| s.contains_point(pt)) {
                        return Some(hit.color);
                    }
                }
            } else {
                let lid = layer.id;
                if let Some(hit) = self
                    .shapes
                    .iter()
                    .rev()
                    .find(|s| s.layer_id == lid && s.contains_point(pt))
                {
                    return Some(hit.color);
                }
            }
        }
        None
    }

    /// 指定シェイプが現在の編集スコープで子グループに属していれば、同じ子グループに属する全シェイプのインデックス列を返す。
    /// 生シェイプであれば `vec![shape_idx]` のみを返す。
    pub(crate) fn expand_to_group_unit(&self, shape_idx: usize) -> Vec<usize> {
        let Some(shape) = self.shapes.get(shape_idx) else {
            return Vec::new();
        };
        if let Some(gid) = shape.active_group_id_at_scope(&self.editing_group_path) {
            (0..self.shapes.len())
                .filter(|&i| {
                    self.shapes[i].active_group_id_at_scope(&self.editing_group_path) == Some(gid)
                })
                .collect()
        } else {
            vec![shape_idx]
        }
    }

    /// 現在の選択対象にグループ化されたオブジェクトが含まれているか
    pub(crate) fn selection_has_group(&self) -> bool {
        self.selected_shapes.iter().any(|&idx| {
            self.shapes
                .get(idx)
                .is_some_and(|s| s.active_group_id_at_scope(&self.editing_group_path).is_some())
        })
    }

    /// 選択中のオブジェクト（生シェイプ・既存グループ問わず）を新しい1つのグループにまとめる（`Cmd+G` / 無限入れ子対応）
    pub(crate) fn group_selected_shapes(&mut self) -> Option<u64> {
        if self.selected_shapes.is_empty() {
            return None;
        }
        let snapshot = self.take_or_clone_unsliced_snapshot();
        let new_gid = self.next_group_id;
        self.next_group_id += 1;

        let target_lid = self.topmost_layer_id_for_indices(&self.selected_shapes);

        let depth = self.editing_group_path.len();
        let mut any_grouped = false;
        for &sel_idx in &self.selected_shapes {
            if let Some(shape) = self.shapes.get_mut(sel_idx) {
                if shape.is_in_editing_scope(&self.editing_group_path) {
                    shape.layer_id = target_lid;
                    shape.group_path.insert(depth, new_gid);
                    any_grouped = true;
                }
            }
        }
        if !any_grouped {
            self.unsliced_canvas = Some(snapshot);
            return None;
        }

        if let Some(l_idx) = self.timeline.layer_index_by_id(target_lid) {
            self.timeline.active_layer = l_idx;
        }
        self.commit_moved_selection(snapshot);
        self.selection_modified = false;
        self.transform_pivot = None;
        Some(new_gid)
    }

    /// 選択中のグループまたはシンボルインスタンスを1段階だけ解除（分離）する（`Cmd+B`）。
    /// 多段入れ子の場合は内側の子グループ構造を維持したまま最外殻のグループのみを外す。
    pub(crate) fn ungroup_selected_shapes(&mut self) -> bool {
        if !self.selection_has_group() {
            return false;
        }
        let snapshot = self.take_or_clone_unsliced_snapshot();
        self.record_undo_snapshot(snapshot);

        let depth = self.editing_group_path.len();
        let mut any_became_raw = false;
        for &sel_idx in &self.selected_shapes {
            if let Some(shape) = self.shapes.get_mut(sel_idx) {
                if shape.is_in_editing_scope(&self.editing_group_path)
                    && shape.group_path.len() > depth
                {
                    let removed_gid = shape.group_path.remove(depth);
                    if shape
                        .symbol_instance
                        .is_some_and(|b| b.instance_gid == removed_gid)
                    {
                        shape.symbol_instance = None;
                    }
                    if shape.group_path.len() == depth {
                        any_became_raw = true;
                    }
                }
            }
        }
        if any_became_raw {
            self.selection_modified = true;
        }
        self.transform_pivot = None;
        true
    }

    /// ダブルクリック位置 `pt` にシンボルがあればそのシンボルの独自タイムラインへ入り、グループがあればインプレイス編集モードへ移行する
    pub(crate) fn enter_group_at(&mut self, pt: Point) -> bool {
        let Some(hit_idx) = self.topmost_shape_at(pt) else {
            return false;
        };
        let Some(gid) = self.shapes[hit_idx].active_group_id_at_scope(&self.editing_group_path)
        else {
            return false;
        };
        if let Some(binding) = self.shapes[hit_idx]
            .symbol_instance
            .filter(|b| b.instance_gid == gid)
        {
            return self.enter_symbol_timeline(binding);
        }
        self.clear_selection();
        self.editing_group_path.push(gid);
        true
    }

    /// 現在入っているグループまたはシンボルタイムライン編集階層から1段階上の親階層へ戻る
    pub(crate) fn exit_current_group(&mut self) -> bool {
        if !self.editing_group_path.is_empty() {
            self.clear_selection();
            let exited_gid = self.editing_group_path.pop().unwrap();
            self.selected_shapes = (0..self.shapes.len())
                .filter(|&i| {
                    self.shapes[i].active_group_id_at_scope(&self.editing_group_path)
                        == Some(exited_gid)
                })
                .collect();
            self.transform_pivot = None;
            return true;
        }
        self.exit_symbol_timeline()
    }

    /// Undo / Redo 後に現在潜っている `editing_group_path` のグループが存在しなくなった場合は親階層へ戻す
    pub(crate) fn sanitize_editing_group_path(&mut self) {
        while !self.editing_group_path.is_empty() {
            let exists = self
                .shapes
                .iter()
                .any(|s| s.is_in_editing_scope(&self.editing_group_path));
            if exists {
                break;
            }
            self.editing_group_path.pop();
        }
    }

    /// Shift クリック等で指定シェイプ（またはグループ塊全体）の選択状態をトグルし、変形ピボットをリセットする
    pub(crate) fn toggle_shape_selection(&mut self, shape_idx: usize) {
        let unit = self.expand_to_group_unit(shape_idx);
        if self.selected_shapes.contains(&shape_idx) {
            self.selected_shapes.retain(|i| !unit.contains(i));
        } else {
            for idx in unit {
                if !self.selected_shapes.contains(&idx) {
                    self.selected_shapes.push(idx);
                }
            }
        }
        self.transform_pivot = None;
        self.sync_timeline_layer_from_canvas_selection();
    }

    /// 既存の選択を確定解除した上で、指定座標 `pt` にある最前面シェイプ（塗り・境界線・頂点、またはグループ塊全体）を選択する
    pub(crate) fn select_single_shape_at(&mut self, pt: Point) -> bool {
        self.clear_selection();
        if let Some(hit_idx) = self
            .topmost_shape_at(pt)
            .or_else(|| self.compute_hover_target(pt).shape_idx())
        {
            self.selected_shapes = self.expand_to_group_unit(hit_idx);
            self.focus_active_layer_to_shape(hit_idx);
            self.sync_timeline_layer_from_canvas_selection();
            true
        } else {
            false
        }
    }

    /// 選択中の全シェイプおよびカスタムピボット点を `delta` だけ平行移動する
    pub(crate) fn translate_selected_shapes(&mut self, delta: KVec2) {
        for &sel_idx in &self.selected_shapes {
            if let Some(shape) = self.shapes.get_mut(sel_idx) {
                shape.translate(delta);
            }
        }
        if let Some(piv) = &mut self.transform_pivot {
            *piv += delta;
        }
    }

    /// 選択中のオブジェクトを十字キー等で `delta` だけ移動し、Undo 履歴記録・最前面整列・タイムライン同期を行う
    pub(crate) fn nudge_selected_shapes(&mut self, delta: KVec2) -> bool {
        let (_, any_valid) = self.selected_shape_mask();
        if !any_valid {
            return false;
        }
        let undo_snapshot = self.shapes.clone();
        self.translate_selected_shapes(delta);
        self.commit_moved_selection(undo_snapshot);
        self.sync_shapes_to_timeline();
        true
    }

    /// Adobe Animate 風の矩形選択（Marquee Selection）
    pub(crate) fn apply_marquee_selection(&mut self, p0: Point, p1: Point, additive: bool) {
        let min_x = p0.x.min(p1.x);
        let max_x = p0.x.max(p1.x);
        let min_y = p0.y.min(p1.y);
        let max_y = p0.y.max(p1.y);

        let min_marquee = 2.0 / (self.view_zoom as f64);
        if (max_x - min_x) < min_marquee || (max_y - min_y) < min_marquee {
            if !additive {
                self.clear_selection();
            }
            return;
        }

        if !additive {
            self.clear_selection();
        }

        let rect = KRect::new(min_x, min_y, max_x, max_y);
        let clip_rect_shape = make_marquee_rect_shape(rect);

        let (was_selected, _) = if additive {
            self.selected_shape_mask()
        } else {
            (vec![false; self.shapes.len()], false)
        };

        // 矩形と交差する子グループIDを事前収集（グループはスライスせず塊単位で選択する）
        let mut hit_group_ids: Vec<u64> = Vec::new();
        for shape in &self.shapes {
            if let Some(gid) = shape.active_group_id_at_scope(&self.editing_group_path) {
                if hit_group_ids.contains(&gid) {
                    continue;
                }
                let bbox = shape.bounding_box();
                if !rects_overlap(bbox, rect) {
                    continue;
                }
                let in_rect =
                    bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1;
                if in_rect
                    || !boolean_operation(
                        std::slice::from_ref(shape),
                        std::slice::from_ref(&clip_rect_shape),
                        BoolOp::Intersection,
                        shape.color,
                    )
                    .is_empty()
                {
                    hit_group_ids.push(gid);
                }
            }
        }

        let pre_slice_snapshot = self
            .unsliced_canvas
            .is_none()
            .then(|| self.shapes.clone());
        let mut any_sliced = false;
        let mut new_shapes = Vec::with_capacity(self.shapes.len() + 4);
        let mut new_selected = Vec::new();

        for (idx, shape) in self.shapes.drain(..).enumerate() {
            if was_selected[idx] {
                new_selected.push(new_shapes.len());
                new_shapes.push(shape);
                continue;
            }

            if !shape.is_in_editing_scope(&self.editing_group_path) {
                new_shapes.push(shape);
                continue;
            }

            if let Some(gid) = shape.active_group_id_at_scope(&self.editing_group_path) {
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

            if bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1
            {
                new_selected.push(new_shapes.len());
                new_shapes.push(shape);
                continue;
            }

            let color = shape.color;
            let lid = shape.layer_id;
            let gp = shape.group_path.clone();
            let inside = boolean_operation(
                std::slice::from_ref(&shape),
                std::slice::from_ref(&clip_rect_shape),
                BoolOp::Intersection,
                color,
            );
            if inside.is_empty() {
                new_shapes.push(shape);
                continue;
            }

            let outside = boolean_operation(
                std::slice::from_ref(&shape),
                std::slice::from_ref(&clip_rect_shape),
                BoolOp::Difference,
                color,
            );
            if outside.is_empty() {
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

        if any_sliced && self.unsliced_canvas.is_none() {
            self.unsliced_canvas = pre_slice_snapshot;
        }
        self.shapes = new_shapes;
        self.selected_shapes = new_selected;
        self.transform_pivot = None;
        self.sync_timeline_layer_from_canvas_selection();
    }

    pub(crate) fn delete_selected_shapes(&mut self) -> bool {
        if self.selected_shapes.is_empty() {
            return false;
        }
        let (is_sel, any_valid) = self.selected_shape_mask();
        if !any_valid {
            self.selected_shapes.clear();
            self.unsliced_canvas = None;
            self.transform_pivot = None;
            return false;
        }

        let snapshot = self.take_or_clone_unsliced_snapshot();
        self.record_undo_snapshot(snapshot);

        let mut remaining = Vec::with_capacity(self.shapes.len());
        for (idx, shape) in self.shapes.drain(..).enumerate() {
            if !is_sel[idx] {
                remaining.push(shape);
            }
        }
        self.shapes = remaining;
        self.selection_modified = false;
        self.selected_shapes.clear();
        self.transform_pivot = None;
        true
    }

    /// 選択中のオブジェクト（生シェイプ・部分スライス領域・グループ問わず）を内部クリップボードにコピーする（`Cmd+C`）
    pub(crate) fn copy_selected_shapes(&mut self) -> bool {
        let (is_sel, any_valid) = self.selected_shape_mask();
        if !any_valid {
            return false;
        }
        let depth = self.editing_group_path.len();
        self.clipboard.clear();
        for (idx, shape) in self.shapes.iter().enumerate() {
            if is_sel[idx] {
                let rel_group_path = if shape.group_path.starts_with(&self.editing_group_path) {
                    shape.group_path[depth..].to_vec()
                } else {
                    Vec::new()
                };
                self.clipboard
                    .push(shape.clone().with_group_path(rel_group_path));
            }
        }
        self.paste_count = 0;
        !self.clipboard.is_empty()
    }

    /// 選択中のオブジェクトを内部クリップボードにコピーした上でキャンバスから削除する（`Cmd+X`）
    pub(crate) fn cut_selected_shapes(&mut self) -> bool {
        if !self.copy_selected_shapes() {
            return false;
        }
        self.delete_selected_shapes()
    }

    /// 内部クリップボードのオブジェクトを現在の編集スコープへ貼り付ける。
    pub(crate) fn paste_clipboard(&mut self, in_place: bool) -> bool {
        if self.clipboard.is_empty() {
            return false;
        }

        self.clear_selection();
        self.push_undo();

        let offset = if in_place {
            KVec2::ZERO
        } else {
            self.paste_count += 1;
            let step = 16.0 / (self.view_zoom as f64);
            let k = self.paste_count as f64;
            KVec2::new(step * k, step * k)
        };

        let active_lid = self.active_scope_layer_id();
        let multi_layer_copy = self.clipboard.first().is_some_and(|first| {
            self.clipboard.iter().any(|s| s.layer_id != first.layer_id)
        });

        let mut gid_map: Vec<(u64, u64)> = Vec::new();
        let mut pasted_shapes = Vec::with_capacity(self.clipboard.len());
        let mut any_raw = false;

        for item in &self.clipboard {
            let mut new_path = self.editing_group_path.clone();
            if item.group_path.is_empty() {
                any_raw = true;
            } else {
                for &old_gid in &item.group_path {
                    let new_gid = if let Some(&(_, mapped)) =
                        gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                    {
                        mapped
                    } else {
                        let alloc = self.next_group_id;
                        self.next_group_id += 1;
                        gid_map.push((old_gid, alloc));
                        alloc
                    };
                    new_path.push(new_gid);
                }
            }
            let target_lid =
                if multi_layer_copy && self.timeline.is_layer_editable_by_id(item.layer_id) {
                    item.layer_id
                } else {
                    active_lid
                };
            let mut cloned = item
                .clone()
                .with_layer_id(target_lid)
                .with_group_path(new_path);
            if let Some(mut binding) = cloned.symbol_instance {
                if let Some(&(_, mapped_inst_gid)) =
                    gid_map.iter().find(|&&(orig, _)| orig == binding.instance_gid)
                {
                    binding.instance_gid = mapped_inst_gid;
                    cloned.symbol_instance = Some(binding);
                } else {
                    cloned.symbol_instance = None;
                }
            }
            if offset != KVec2::ZERO {
                cloned.translate(offset);
            }
            pasted_shapes.push(cloned);
        }

        self.insert_and_select_shapes_in_scope(pasted_shapes, any_raw);
        true
    }

    /// 現在の編集スコープの最前面挿入位置へ `new_shapes` を挿入し、それらを選択状態にしてレイヤー順に整列させる
    pub(crate) fn insert_and_select_shapes_in_scope(
        &mut self,
        new_shapes: Vec<FillShape>,
        mark_modified: bool,
    ) {
        let insert_pos = self.scope_insert_index();
        let count = new_shapes.len();
        self.shapes.splice(insert_pos..insert_pos, new_shapes);
        self.selected_shapes = (insert_pos..insert_pos + count).collect();
        self.sort_shapes_by_layer_order();
        self.selection_modified = mark_modified;
        self.transform_pivot = None;
        if !matches!(self.tool, Tool::Select | Tool::Transform) {
            self.set_tool(Tool::Select);
        }
    }

    pub(crate) fn commit_moved_selection(&mut self, undo_snapshot: Vec<FillShape>) {
        let (is_sel, any_valid) = self.selected_shape_mask();
        if !any_valid {
            return;
        }

        let snapshot = self.unsliced_canvas.take().unwrap_or(undo_snapshot);
        self.record_undo_snapshot(snapshot);
        self.selection_modified = true;

        let insert_limit = self.scope_insert_index();
        let mut unselected = Vec::with_capacity(self.shapes.len());
        let mut moved_shapes = Vec::with_capacity(self.selected_shapes.len());
        let mut trailing_outside = Vec::new();
        for (idx, shape) in self.shapes.drain(..).enumerate() {
            if is_sel[idx] {
                moved_shapes.push(shape);
            } else if idx < insert_limit {
                unselected.push(shape);
            } else {
                trailing_outside.push(shape);
            }
        }

        let start_sel = unselected.len();
        let end_sel = start_sel + moved_shapes.len();
        unselected.extend(moved_shapes);
        unselected.extend(trailing_outside);
        self.shapes = unselected;
        self.selected_shapes = (start_sel..end_sel).collect();
        self.sort_shapes_by_layer_order();
    }

    /// 選択オブジェクトの平行移動ドラッグ終了時の確定・ロールバック共通処理
    pub(crate) fn finish_move_selection_drag(
        &mut self,
        total_delta: KVec2,
        undo_snapshot: Vec<FillShape>,
    ) {
        let min_move = 1.0 / (self.view_zoom as f64);
        if total_delta.hypot() > min_move && !self.selected_shapes.is_empty() {
            self.commit_moved_selection(undo_snapshot);
        } else if total_delta.hypot() > 0.0 {
            self.shapes = undo_snapshot;
        }
    }

    /// 輪郭エッジつまみ（PullEdge）または頂点移動（MoveVertex）ドラッグ終了時の確定・図形分割・ロールバック共通処理
    pub(crate) fn finish_deform_shape_drag(
        &mut self,
        shape_idx: usize,
        drag_dist: f64,
        undo_snapshot: Vec<FillShape>,
        apply_deform: impl FnOnce(&mut FillShape),
    ) {
        let min_deform = 0.5 / (self.view_zoom as f64);
        if drag_dist > min_deform && shape_idx < self.shapes.len() {
            self.focus_active_layer_to_shape(shape_idx);
            self.record_undo_snapshot(undo_snapshot);
            let mut deformed = self.shapes.remove(shape_idx);
            apply_deform(&mut deformed);
            let resolved = resolve_deformed_shape(&deformed);
            merge_shapes_into_canvas(&mut self.shapes, resolved);
            self.sort_shapes_by_layer_order();
        } else {
            self.shapes = undo_snapshot;
        }
    }

    /// 選択中のシェイプ群の `(インデックス, FillShape クローン)` 一覧を返す
    pub(crate) fn selected_shapes_snapshot(&self) -> Vec<(usize, FillShape)> {
        self.selected_shapes
            .iter()
            .filter_map(|&idx| self.shapes.get(idx).map(|s| (idx, s.clone())))
            .collect()
    }

    /// 選択中シェイプ群の全体のバウンディングボックスを返す
    pub(crate) fn selection_bounding_box(&self) -> Option<KRect> {
        let mut iter = self
            .selected_shapes
            .iter()
            .filter_map(|&idx| self.shapes.get(idx))
            .map(|s| s.bounding_box());
        let first = iter.next()?;
        let mut bbox = iter.fold(first, |acc, r| acc.union(r));
        let min_extent = 6.0 / (self.view_zoom as f64);
        let half_extent = 0.5 * min_extent;
        if bbox.width() < min_extent {
            let cx = 0.5 * (bbox.x0 + bbox.x1);
            bbox.x0 = cx - half_extent;
            bbox.x1 = cx + half_extent;
        }
        if bbox.height() < min_extent {
            let cy = 0.5 * (bbox.y0 + bbox.y1);
            bbox.y0 = cy - half_extent;
            bbox.y1 = cy + half_extent;
        }
        Some(bbox)
    }


    // ------------------------------------------------------------------------
    // 4. 輪郭変形（PullEdge / MoveVertex）＆ ヒットテスト（アルゴリズム 4）
    // ------------------------------------------------------------------------

    /// 向こう側の輪郭エッジ付近でドロップされた際に確実に貫通して分割されるよう僅かに押し出す。
    pub(crate) fn punch_through_opposite_edge(
        shape: &FillShape,
        contour_idx: usize,
        ignore_segs: &[usize],
        anchor_a: Point,
        anchor_b: Point,
        start_pt: Point,
        drag_pt: Point,
    ) -> Point {
        let drag_vec = drag_pt - start_pt;
        if drag_vec.hypot() <= 6.0
            || (drag_pt - anchor_a).hypot() <= 10.0
            || (drag_pt - anchor_b).hypot() <= 10.0
        {
            return drag_pt;
        }

        let drag_dir = normalize_vec(drag_vec);
        let mut best_near: Option<(f64, Point)> = None;
        for c_idx in 0..shape.contour_count() {
            if let Some(c) = shape.get_contour(c_idx) {
                for (s_idx, seg) in c.segments.iter().enumerate() {
                    if c_idx == contour_idx && ignore_segs.contains(&s_idx) {
                        continue;
                    }
                    let near = seg.nearest(drag_pt, 0.05);
                    let d = near.distance_sq.sqrt();
                    if d <= 6.0 {
                        let near_pt = seg.eval(near.t);
                        if (near_pt - anchor_a).hypot() > 8.0
                            && (near_pt - anchor_b).hypot() > 8.0
                            && best_near.is_none_or(|(bd, _)| d < bd)
                        {
                            best_near = Some((d, near_pt));
                        }
                    }
                }
            }
        }
        if let Some((_, near_pt)) = best_near {
            let punched = near_pt + drag_dir * 4.5;
            if (punched - start_pt).dot(drag_dir) > (drag_pt - start_pt).dot(drag_dir) {
                return punched;
            }
        }
        drag_pt
    }

    /// エッジつまみ変形（PullEdge）を適用する。
    pub(crate) fn apply_pull_edge_to_shape(
        shape: &mut FillShape,
        contour_idx: usize,
        seg_idx: usize,
        grab_t: f64,
        orig_seg: QuadBez,
        drag_pt: Point,
        punch_opposite: bool,
    ) {
        let grab_pos = orig_seg.eval(grab_t);
        let effective_pt = if punch_opposite {
            Self::punch_through_opposite_edge(
                shape,
                contour_idx,
                &[seg_idx],
                orig_seg.p0,
                orig_seg.p2,
                grab_pos,
                drag_pt,
            )
        } else {
            drag_pt
        };

        let delta = effective_pt - grab_pos;
        let denom = 2.0 * grab_t * (1.0 - grab_t);
        let new_p1 = orig_seg.p1 + delta / denom;
        if let Some(contour) = shape.get_contour_mut(contour_idx) {
            if seg_idx < contour.segments.len() {
                contour.segments[seg_idx].p1 = new_p1;
            }
        }
        shape.rebuild_cache();
    }

    /// 頂点ドラッグ変形（MoveVertex）を適用する。
    pub(crate) fn apply_move_vertex_to_shape(
        shape: &mut FillShape,
        contour_idx: usize,
        seg_idx: usize,
        orig_vertex: Point,
        orig_ctrls: (Point, Point),
        drag_pt: Point,
        punch_opposite: bool,
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

        let effective_pt = if punch_opposite {
            Self::punch_through_opposite_edge(
                shape,
                contour_idx,
                &[prev_idx, seg_idx],
                prev_p0,
                curr_p2,
                orig_vertex,
                drag_pt,
            )
        } else {
            drag_pt
        };

        let delta = effective_pt - orig_vertex;
        if let Some(contour) = shape.get_contour_mut(contour_idx) {
            if seg_idx < contour.segments.len() {
                contour.segments[prev_idx].p2 = effective_pt;
                contour.segments[seg_idx].p0 = effective_pt;
                contour.segments[prev_idx].p1 = orig_ctrls.0 + delta * 0.5;
                contour.segments[seg_idx].p1 = orig_ctrls.1 + delta * 0.5;
            }
        }
        shape.rebuild_cache();
    }

    /// 指定座標 `pt` がシェイプの輪郭（外周・穴）から距離 `radius` 以内にあるかを判定する
    fn point_within_shape_contours(
        shape: &FillShape,
        pt: Point,
        radius: f64,
        inv_z: f64,
    ) -> bool {
        let r_sq = radius * radius;
        for c_idx in 0..shape.contour_count() {
            if let Some(contour) = shape.get_contour(c_idx) {
                for seg in &contour.segments {
                    if seg.nearest(pt, 0.05 * inv_z).distance_sq <= r_sq {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// アルゴリズム 4: マウス位置に対する頂点・輪郭エッジ・塗りのヒットテスト
    pub(crate) fn compute_hover_target(&self, mouse_pt: Point) -> HoverTarget {
        let inv_z = 1.0 / (self.view_zoom as f64);
        let vertex_hit_radius = 9.0 * inv_z;
        let edge_hit_outer_radius = 9.5 * inv_z;
        let edge_hit_inner_radius = 5.5 * inv_z;
        let anchor_edit_hit_radius = 12.0 * inv_z;
        let selected_grab_radius = 6.0 * inv_z;

        let (is_sel, has_sel) = self.selected_shape_mask();

        // 0. 通常の選択モード（Reshape）では、選択済みシェイプ（極細線の外周マージン含む）へのホバーを
        //    すべて移動対象（HoverTarget::Fill）として最優先し、選択済みシェイプの変形暴発を防ぐ
        if self.select_sub_mode == SelectSubMode::Reshape && has_sel {
            for &s_idx in self.selected_shapes.iter().rev() {
                let Some(shape) = self.shapes.get(s_idx) else {
                    continue;
                };
                if !shape.is_in_editing_scope(&self.editing_group_path)
                    || !shape
                        .bounding_box()
                        .inflate(selected_grab_radius, selected_grab_radius)
                        .contains(mouse_pt)
                {
                    continue;
                }
                if shape.contains_point(mouse_pt)
                    || Self::point_within_shape_contours(
                        shape,
                        mouse_pt,
                        selected_grab_radius,
                        inv_z,
                    )
                {
                    return HoverTarget::Fill { shape_idx: s_idx };
                }
            }
        }

        // 1. 輪郭の角（アンカーポイント）への近接判定
        if self.select_sub_mode != SelectSubMode::AddAnchor {
            let v_radius = if self.select_sub_mode == SelectSubMode::DeleteAnchor {
                anchor_edit_hit_radius
            } else {
                vertex_hit_radius
            };
            let mut best_vertex: Option<(f64, HoverTarget)> = None;
            for (s_idx, shape) in self.shapes.iter().enumerate().rev() {
                if (self.select_sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
                    || !shape.is_raw_at_scope(&self.editing_group_path)
                    || !shape
                        .bounding_box()
                        .inflate(v_radius, v_radius)
                        .contains(mouse_pt)
                {
                    continue;
                }
                for c_idx in 0..shape.contour_count() {
                    if let Some(contour) = shape.get_contour(c_idx) {
                        if self.select_sub_mode == SelectSubMode::DeleteAnchor
                            && contour.segments.len() <= 2
                        {
                            continue;
                        }
                        for (seg_idx, seg) in contour.segments.iter().enumerate() {
                            let dist = (mouse_pt - seg.p0).hypot();
                            if dist <= v_radius
                                && best_vertex.as_ref().is_none_or(|(d, _)| dist < *d)
                            {
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
            }
            if let Some((_, target)) = best_vertex {
                return target;
            }
            if self.select_sub_mode == SelectSubMode::DeleteAnchor {
                return HoverTarget::None;
            }
        }

        // 2. 輪郭の2次ベジェ曲線セグメント（エッジ）への近接判定
        let mut best_edge: Option<(f64, HoverTarget)> = None;
        for (s_idx, shape) in self.shapes.iter().enumerate().rev() {
            if (self.select_sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
                || !shape.is_raw_at_scope(&self.editing_group_path)
                || !shape
                    .bounding_box()
                    .inflate(anchor_edit_hit_radius, anchor_edit_hit_radius)
                    .contains(mouse_pt)
            {
                continue;
            }
            let max_dist = if self.select_sub_mode == SelectSubMode::AddAnchor {
                anchor_edit_hit_radius
            } else if shape.contains_point(mouse_pt) {
                edge_hit_inner_radius
            } else {
                edge_hit_outer_radius
            };

            for c_idx in 0..shape.contour_count() {
                if let Some(contour) = shape.get_contour(c_idx) {
                    for (seg_idx, seg) in contour.segments.iter().enumerate() {
                        let nearest = seg.nearest(mouse_pt, 0.05 * inv_z);
                        let t_clamped = nearest.t.clamp(0.08, 0.92);
                        let eval_pt = if self.select_sub_mode == SelectSubMode::AddAnchor {
                            seg.eval(t_clamped)
                        } else {
                            seg.eval(nearest.t)
                        };
                        let dist = nearest.distance_sq.sqrt();
                        if dist <= max_dist && best_edge.as_ref().is_none_or(|(d, _)| dist < *d) {
                            best_edge = Some((
                                dist,
                                HoverTarget::Edge {
                                    shape_idx: s_idx,
                                    contour_idx: c_idx,
                                    seg_idx,
                                    t: if self.select_sub_mode == SelectSubMode::AddAnchor {
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
        }
        if let Some((_, target)) = best_edge {
            return target;
        }
        if self.select_sub_mode == SelectSubMode::AddAnchor {
            return HoverTarget::None;
        }

        // 3. シェイプ内部の塗り領域判定
        if let Some(s_idx) = self.topmost_shape_at(mouse_pt) {
            return HoverTarget::Fill { shape_idx: s_idx };
        }

        HoverTarget::None
    }

    /// 頂点ドラッグ時に周囲の他の頂点へ吸着（スナップ）する座標を検索
    pub(crate) fn find_snap_vertex(
        &self,
        drag_pt: Point,
        ignore_shape: usize,
        ignore_contour: usize,
        ignore_seg: usize,
    ) -> Option<Point> {
        let snap_radius = 12.0 / (self.view_zoom as f64);
        let mut best_snap: Option<(f64, Point)> = None;

        for (s_idx, shape) in self.shapes.iter().enumerate() {
            if !shape.is_raw_at_scope(&self.editing_group_path)
                || !shape
                    .bounding_box()
                    .inflate(snap_radius, snap_radius)
                    .contains(drag_pt)
            {
                continue;
            }
            for c_idx in 0..shape.contour_count() {
                if let Some(contour) = shape.get_contour(c_idx) {
                    for (seg_idx, seg) in contour.segments.iter().enumerate() {
                        if s_idx == ignore_shape
                            && c_idx == ignore_contour
                            && seg_idx == ignore_seg
                        {
                            continue;
                        }
                        let dist = (drag_pt - seg.p0).hypot();
                        if dist <= snap_radius
                            && best_snap.as_ref().is_none_or(|(d, _)| dist < *d)
                        {
                            best_snap = Some((dist, seg.p0));
                        }
                    }
                }
            }
        }
        best_snap.map(|(_, pt)| pt)
    }


    /// Flash風ドット柄（Polka-Dot）選択オーバーレイを描画（キャッシュ済み Mesh を使用）
    pub(crate) fn draw_flash_selection_overlay(&mut self, painter: &egui::Painter, shape_idx: usize) {
        let zoom = self.view_zoom;
        let pan = self.view_pan;
        let sel_mesh = transform_mesh_to_screen(
            self.shapes[shape_idx].selection_overlay_mesh(zoom),
            zoom,
            pan,
            false,
        );
        painter.add(egui::Shape::mesh(sel_mesh));
        self.draw_shape_contour_lines(
            painter,
            &self.shapes[shape_idx],
            Stroke::new(1.5, Color32::from_rgb(0, 175, 255)),
            false,
        );
    }


    pub(crate) fn draw_shape_contour_lines(
        &self,
        painter: &egui::Painter,
        shape: &FillShape,
        stroke: Stroke,
        show_anchors: bool,
    ) {
        let z = self.view_zoom as f64;
        for contour in shape.contours() {
            for seg in &contour.segments {
                let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot() * z;
                if mid_dev <= 0.15 {
                    painter.line_segment(
                        [self.world_to_screen(seg.p0), self.world_to_screen(seg.p2)],
                        stroke,
                    );
                } else {
                    let mut prev = seg.p0;
                    let steps = (((seg.arclen(1.0) * z) / 8.0).ceil() as usize).clamp(3, 18);
                    for k in 1..=steps {
                        let curr = seg.eval(k as f64 / steps as f64);
                        painter.line_segment(
                            [self.world_to_screen(prev), self.world_to_screen(curr)],
                            stroke,
                        );
                        prev = curr;
                    }
                }
                if show_anchors {
                    let a = self.world_to_screen(seg.p0);
                    painter.circle_filled(a, 3.0, Color32::WHITE);
                    painter.circle_stroke(a, 3.0, Stroke::new(1.2, Color32::from_rgb(30, 120, 230)));
                }
            }
        }
    }


    pub(crate) fn handle_select_tool_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        mouse_pt: Option<Point>,
    ) {
        match self.select_sub_mode {
            SelectSubMode::AddAnchor => {
                self.handle_anchor_edit_input(ui, response, mouse_pt, true);
            }
            SelectSubMode::DeleteAnchor => {
                self.handle_anchor_edit_input(ui, response, mouse_pt, false);
            }
            SelectSubMode::Reshape => {
                let shift_held = ui.input(|i| i.modifiers.shift);
                if response.double_clicked() {
                    if let Some(pt) = mouse_pt {
                        if !self.enter_group_at(pt) {
                            self.exit_current_group();
                        }
                    }
                    return;
                }
                if response.drag_started() {
                    if let Some(pt) = mouse_pt {
                        self.start_reshape_drag(pt, shift_held);
                    }
                } else if response.dragged() {
                    if let Some(pt) = mouse_pt {
                        self.update_reshape_drag(pt);
                    }
                } else if response.drag_stopped() {
                    self.finish_reshape_drag(mouse_pt);
                } else if response.clicked() {
                    if let Some(pt) = mouse_pt {
                        self.handle_reshape_click(pt, shift_held);
                    }
                }

                if matches!(self.select_drag, SelectDragState::None) {
                    self.hover_target = mouse_pt
                        .map(|pt| self.compute_hover_target(pt))
                        .unwrap_or(HoverTarget::None);
                }
            }
        }
    }

    fn handle_anchor_edit_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        mouse_pt: Option<Point>,
        is_add: bool,
    ) {
        let just_pressed = response.hovered() && ui.input(|i| i.pointer.primary_pressed());
        if just_pressed {
            if let Some(pt) = mouse_pt {
                let target = self.compute_hover_target(pt);
                let snapshot = self.shapes.clone();
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
                    ) => self
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
                    ) => self
                        .shapes
                        .get_mut(shape_idx)
                        .and_then(|s| s.get_contour_mut(contour_idx))
                        .is_some_and(|c| c.delete_anchor_at(seg_idx))
                        .then_some(shape_idx),
                    _ => None,
                };
                if let Some(s_idx) = modified_shape {
                    if let Some(shape) = self.shapes.get_mut(s_idx) {
                        shape.rebuild_cache();
                    }
                    self.record_undo_snapshot(snapshot);
                }
            }
        }
        self.hover_target = mouse_pt
            .map(|pt| self.compute_hover_target(pt))
            .unwrap_or(HoverTarget::None);
    }

    fn start_reshape_drag(&mut self, pt: Point, shift_held: bool) {
        if shift_held {
            self.select_drag = SelectDragState::MarqueeSelect {
                start_pos: pt,
                current_pos: pt,
                additive: true,
            };
            return;
        }
        match self.compute_hover_target(pt) {
            HoverTarget::Vertex {
                shape_idx,
                contour_idx,
                seg_idx,
                pos,
            } => {
                let snapshot = self.take_or_clone_unsliced_snapshot();
                if let Some(contour) = self
                    .shapes
                    .get(shape_idx)
                    .and_then(|s| s.get_contour(contour_idx))
                {
                    let n = contour.segments.len();
                    let prev_idx = (seg_idx + n - 1) % n;
                    let orig_prev_ctrl = contour.segments[prev_idx].p1;
                    let orig_curr_ctrl = contour.segments[seg_idx].p1;
                    self.selected_shapes.clear();
                    self.select_drag = SelectDragState::MoveVertex {
                        shape_idx,
                        contour_idx,
                        seg_idx,
                        orig_vertex: pos,
                        last_pos: pos,
                        orig_prev_ctrl,
                        orig_curr_ctrl,
                        snapped_to: None,
                        undo_snapshot: snapshot,
                    };
                }
            }
            HoverTarget::Edge {
                shape_idx,
                contour_idx,
                seg_idx,
                t,
                closest_pos,
            } => {
                let snapshot = self.take_or_clone_unsliced_snapshot();
                if let Some(orig_seg) = self.get_contour_segment(shape_idx, contour_idx, seg_idx) {
                    self.selected_shapes.clear();
                    self.select_drag = SelectDragState::PullEdge {
                        shape_idx,
                        contour_idx,
                        seg_idx,
                        grab_t: t.clamp(0.18, 0.82),
                        grab_pos: closest_pos,
                        last_pos: closest_pos,
                        orig_seg,
                        undo_snapshot: snapshot,
                    };
                }
            }
            HoverTarget::Fill { shape_idx } => {
                if !self.selected_shapes.contains(&shape_idx) {
                    self.select_single_shape_at(pt);
                }
                self.select_drag = SelectDragState::MoveShape {
                    last_pos: pt,
                    total_delta: KVec2::ZERO,
                    undo_snapshot: self.shapes.clone(),
                };
            }
            HoverTarget::None => {
                self.select_drag = SelectDragState::MarqueeSelect {
                    start_pos: pt,
                    current_pos: pt,
                    additive: false,
                };
            }
        }
    }

    fn update_reshape_drag(&mut self, pt: Point) {
        let vertex_drag_info = if let SelectDragState::MoveVertex {
            shape_idx,
            contour_idx,
            seg_idx,
            orig_vertex,
            orig_prev_ctrl,
            orig_curr_ctrl,
            ..
        } = &self.select_drag
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
            let snap_opt = self.find_snap_vertex(pt, s_idx, c_idx, g_idx);
            let target_pt = snap_opt.unwrap_or(pt);
            if let SelectDragState::MoveVertex {
                snapped_to,
                last_pos,
                ..
            } = &mut self.select_drag
            {
                *snapped_to = snap_opt;
                *last_pos = target_pt;
            }
            if let Some(shape) = self.shapes.get_mut(s_idx) {
                Self::apply_move_vertex_to_shape(
                    shape,
                    c_idx,
                    g_idx,
                    orig_v,
                    (prev_c, curr_c),
                    target_pt,
                    false,
                );
            }
            return;
        }

        let mut move_delta: Option<KVec2> = None;
        match &mut self.select_drag {
            SelectDragState::MoveShape {
                last_pos,
                total_delta,
                ..
            } => {
                let step_delta = pt - *last_pos;
                *last_pos = pt;
                *total_delta += step_delta;
                move_delta = Some(step_delta);
            }
            SelectDragState::MarqueeSelect { current_pos, .. } => {
                *current_pos = pt;
            }
            SelectDragState::PullEdge {
                shape_idx,
                contour_idx,
                seg_idx,
                grab_t,
                last_pos,
                orig_seg,
                ..
            } => {
                *last_pos = pt;
                if let Some(shape) = self.shapes.get_mut(*shape_idx) {
                    Self::apply_pull_edge_to_shape(
                        shape,
                        *contour_idx,
                        *seg_idx,
                        *grab_t,
                        *orig_seg,
                        pt,
                        false,
                    );
                }
            }
            _ => {}
        }
        if let Some(delta) = move_delta {
            self.translate_selected_shapes(delta);
        }
    }

    fn finish_reshape_drag(&mut self, mouse_pt: Option<Point>) {
        let finished_drag = std::mem::replace(&mut self.select_drag, SelectDragState::None);
        match finished_drag {
            SelectDragState::MoveShape {
                total_delta,
                undo_snapshot,
                ..
            } => {
                self.finish_move_selection_drag(total_delta, undo_snapshot);
            }
            SelectDragState::MarqueeSelect {
                start_pos,
                current_pos,
                additive,
            } => {
                let end_pos = mouse_pt.unwrap_or(current_pos);
                self.apply_marquee_selection(start_pos, end_pos, additive);
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
            } => {
                self.finish_deform_shape_drag(
                    shape_idx,
                    (last_pos - grab_pos).hypot(),
                    undo_snapshot,
                    |deformed| {
                        Self::apply_pull_edge_to_shape(
                            deformed,
                            contour_idx,
                            seg_idx,
                            grab_t,
                            orig_seg,
                            last_pos,
                            true,
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
                self.finish_deform_shape_drag(
                    shape_idx,
                    (last_pos - orig_vertex).hypot(),
                    undo_snapshot,
                    |deformed| {
                        Self::apply_move_vertex_to_shape(
                            deformed,
                            contour_idx,
                            seg_idx,
                            orig_vertex,
                            (orig_prev_ctrl, orig_curr_ctrl),
                            last_pos,
                            true,
                        );
                    },
                );
            }
            SelectDragState::None => {}
        }
    }

    fn handle_reshape_click(&mut self, pt: Point, shift_held: bool) {
        let hit_shape = self
            .topmost_shape_at(pt)
            .or_else(|| self.compute_hover_target(pt).shape_idx());
        if shift_held {
            if let Some(shape_idx) = hit_shape {
                self.toggle_shape_selection(shape_idx);
            }
        } else {
            match hit_shape {
                Some(shape_idx) if self.selected_shapes.contains(&shape_idx) => {
                    self.selected_shapes = self.expand_to_group_unit(shape_idx);
                    self.transform_pivot = None;
                }
                Some(_) => {
                    self.select_single_shape_at(pt);
                }
                None => {
                    self.clear_selection();
                }
            }
        }
    }

}
