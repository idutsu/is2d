use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Stroke, Vec2};
use kurbo::{
    Affine, ParamCurve, ParamCurveArclen, ParamCurveNearest, Point, QuadBez, Rect as KRect,
    Vec2 as KVec2,
};

use crate::vector_engine::{
    affine_rotate_around, affine_scale_around, boolean_operation, build_smoothed_stroke_shapes_at_zoom,
    erase_from_canvas, find_enclosed_region_at, generate_brush_outline_at_zoom, make_marquee_rect_shape,
    merge_disjoint_fill_into_canvas, merge_shapes_into_canvas, normalize_vec, rects_overlap,
    resolve_deformed_shape, tessellate_raw_polygon, to_pos2, transform_mesh_to_screen,
    transform_mesh_to_screen_onion, BoolOp, FillShape,
};
use crate::FlashBrushApp;

// ============================================================================
// ツールとインタラクション状態
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    Brush,      // B キー
    Eraser,     // E キー
    Bucket,     // K キー
    Eyedropper, // I キー（スポイトツール）
    Select,     // V キー
    Transform,  // Q キー（自由変形ツール）
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TransformHoverTarget {
    None,
    Pivot,
    Scale(ScaleHandle),
    Rotate,
    Move,
}

#[derive(Clone, Debug)]
pub(crate) enum TransformDragState {
    None,
    MovePivot,
    Move {
        last_pos: Point,
        total_delta: KVec2,
        undo_snapshot: Vec<FillShape>,
    },
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
    MarqueeSelect {
        start_pos: Point,
        current_pos: Point,
        additive: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SelectSubMode {
    Reshape,      // 通常の輪郭変形・頂点移動・図形移動（V キー）
    AddAnchor,    // ＋ アンカー追加モード（+ キー）
    DeleteAnchor, // － アンカー削除モード（- キー）
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HoverTarget {
    None,
    Vertex {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        pos: Point,
    },
    Edge {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        t: f64,
        closest_pos: Point,
    },
    Fill {
        shape_idx: usize,
    },
}

impl HoverTarget {
    pub(crate) fn shape_idx(&self) -> Option<usize> {
        match *self {
            HoverTarget::Vertex { shape_idx, .. }
            | HoverTarget::Edge { shape_idx, .. }
            | HoverTarget::Fill { shape_idx } => Some(shape_idx),
            HoverTarget::None => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SelectDragState {
    None,
    MoveShape {
        last_pos: Point,
        total_delta: KVec2,
        undo_snapshot: Vec<FillShape>,
    },
    MarqueeSelect {
        start_pos: Point,
        current_pos: Point,
        additive: bool,
    },
    PullEdge {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        grab_t: f64,
        grab_pos: Point,
        last_pos: Point,
        orig_seg: QuadBez,
        undo_snapshot: Vec<FillShape>,
    },
    MoveVertex {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        orig_vertex: Point,
        last_pos: Point,
        orig_prev_ctrl: Point,
        orig_curr_ctrl: Point,
        snapped_to: Option<Point>,
        undo_snapshot: Vec<FillShape>,
    },
}

// ============================================================================
// キャンバス操作・選択・変形・グループ編集・描画実装
// ============================================================================

impl FlashBrushApp {
    // ------------------------------------------------------------------------
    // 1. ビューポート座標変換・選択マスク・レイヤー追従ヘルパー
    // ------------------------------------------------------------------------

    /// ワールド座標 `p` を現在のズーム・パンに応じたスクリーン座標 `Pos2` へ変換する
    #[inline]
    pub(crate) fn world_to_screen(&self, p: Point) -> Pos2 {
        Pos2::new(
            (p.x as f32) * self.view_zoom + self.view_pan.x,
            (p.y as f32) * self.view_zoom + self.view_pan.y,
        )
    }

    /// スクリーン座標 `pos` を現在のズーム・パンに応じたワールド座標 `Point` へ逆変換する
    #[inline]
    pub(crate) fn screen_to_world(&self, pos: Pos2) -> Point {
        Point::new(
            ((pos.x - self.view_pan.x) / self.view_zoom) as f64,
            ((pos.y - self.view_pan.y) / self.view_zoom) as f64,
        )
    }

    /// スクリーン上の指定位置 `anchor_screen`（マウスカーソル位置）がズーム前後で動かないように倍率 `factor` を掛ける
    pub(crate) fn zoom_around_screen_pos(&mut self, anchor_screen: Pos2, factor: f32) {
        let world_before = self.screen_to_world(anchor_screen);
        self.view_zoom = (self.view_zoom * factor).clamp(0.15, 16.0);
        self.view_pan = Vec2::new(
            anchor_screen.x - (world_before.x as f32) * self.view_zoom,
            anchor_screen.y - (world_before.y as f32) * self.view_zoom,
        );
    }

    /// ズーム倍率を 100%（`1.0`）、パン位置を原点（`Vec2::ZERO`）へリセットする（`Cmd+0`）
    pub(crate) fn reset_view(&mut self) {
        self.view_zoom = 1.0;
        self.view_pan = Vec2::ZERO;
    }

    /// 矩形スライス前のキャンバス状態があれば取り出し、なければ現在の `self.shapes` をクローンして返す
    #[inline]
    fn take_or_clone_unsliced_snapshot(&mut self) -> Vec<FillShape> {
        self.unsliced_canvas
            .take()
            .unwrap_or_else(|| self.shapes.clone())
    }

    /// 現在の編集スコープ（`editing_group_path`）末尾に対応する挿入インデックスを返す
    fn scope_insert_index(&self) -> usize {
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
    fn get_contour_segment(
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

    /// 現在の `self.selected_shapes` に対応する選択マスク（`Vec<bool>`）と有効な選択が1つ以上あるかを返す
    pub(crate) fn selected_shape_mask(&self) -> (Vec<bool>, bool) {
        let mut is_sel = vec![false; self.shapes.len()];
        let mut any_valid = false;
        for &idx in &self.selected_shapes {
            if idx < is_sel.len() {
                is_sel[idx] = true;
                any_valid = true;
            }
        }
        (is_sel, any_valid)
    }

    /// 現在の編集スコープ（グループ内であればそのグループの所属レイヤー、そうでなければ現在のアクティブレイヤー）の `layer_id` を返す
    pub(crate) fn active_scope_layer_id(&self) -> u64 {
        if !self.editing_group_path.is_empty() {
            if let Some(s) = self
                .shapes
                .iter()
                .find(|s| s.is_in_editing_scope(&self.editing_group_path))
            {
                return s.layer_id;
            }
        }
        if self.timeline.is_active_layer_editable() {
            self.timeline.active_layer_id()
        } else {
            self.timeline
                .layers
                .iter()
                .rev()
                .find(|l| l.visible && !l.locked)
                .map(|l| l.id)
                .unwrap_or_else(|| self.timeline.active_layer_id())
        }
    }

    /// 指定したシェイプインデックス `shape_idx` が属するレイヤーへ `self.timeline.active_layer` およびタイムライン選択状態を追従させる
    pub(crate) fn focus_active_layer_to_shape(&mut self, shape_idx: usize) {
        if let Some(lid) = self.shapes.get(shape_idx).map(|s| s.layer_id) {
            if let Some(l_idx) = self.timeline.layer_index_by_id(lid) {
                self.timeline.active_layer = l_idx;
                let cf = self.timeline.current_frame;
                self.timeline.cell_selection =
                    Some(crate::timeline::TimelineCellSelection::single(l_idx, cf));
            }
        }
    }

    /// キャンバス上で選択されたシェイプ群（`self.selected_shapes`）が所属するレイヤーをタイムライン上で選択状態にする
    pub(crate) fn sync_timeline_layer_from_canvas_selection(&mut self) {
        let mut min_l: Option<usize> = None;
        let mut max_l: Option<usize> = None;
        let mut last_l: Option<usize> = None;
        for &idx in &self.selected_shapes {
            if let Some(lid) = self.shapes.get(idx).map(|s| s.layer_id) {
                if let Some(l_idx) = self.timeline.layer_index_by_id(lid) {
                    min_l = Some(min_l.map_or(l_idx, |m| m.min(l_idx)));
                    max_l = Some(max_l.map_or(l_idx, |m| m.max(l_idx)));
                    last_l = Some(l_idx);
                }
            }
        }
        if let (Some(l0), Some(l1), Some(act)) = (min_l, max_l, last_l) {
            self.timeline.active_layer = act;
            let cf = self.timeline.current_frame;
            self.timeline.cell_selection = Some(
                crate::timeline::TimelineCellSelection::from_corners(l0, cf, l1, cf),
            );
        }
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

        let target_lid = self
            .selected_shapes
            .iter()
            .filter_map(|&idx| self.shapes.get(idx).map(|s| s.layer_id))
            .max_by_key(|&lid| self.timeline.layer_index_by_id(lid).unwrap_or(0))
            .unwrap_or_else(|| self.active_scope_layer_id());

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

    /// 選択中のグループを1段階だけ解除（分離）する（`Cmd+B`）。
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
                    shape.group_path.remove(depth);
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

    /// ダブルクリック位置 `pt` にグループがあれば、そのグループの中に入ってインプレイス編集モードへ移行する
    pub(crate) fn enter_group_at(&mut self, pt: Point) -> bool {
        let Some(hit_idx) = self.topmost_shape_at(pt) else {
            return false;
        };
        let Some(gid) = self.shapes[hit_idx].active_group_id_at_scope(&self.editing_group_path)
        else {
            return false;
        };
        self.clear_selection();
        self.editing_group_path.push(gid);
        true
    }

    /// 現在入っているグループ編集階層から1段階上の親階層へ戻る
    pub(crate) fn exit_current_group(&mut self) -> bool {
        if self.editing_group_path.is_empty() {
            return false;
        }
        self.clear_selection();
        let exited_gid = self.editing_group_path.pop().unwrap();
        self.selected_shapes = (0..self.shapes.len())
            .filter(|&i| {
                self.shapes[i].active_group_id_at_scope(&self.editing_group_path)
                    == Some(exited_gid)
            })
            .collect();
        self.transform_pivot = None;
        true
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
            if offset != KVec2::ZERO {
                cloned.translate(offset);
            }
            pasted_shapes.push(cloned);
        }

        let insert_pos = self.scope_insert_index();
        let count = pasted_shapes.len();
        self.shapes.splice(insert_pos..insert_pos, pasted_shapes);
        self.selected_shapes = (insert_pos..insert_pos + count).collect();
        self.sort_shapes_by_layer_order();
        self.selection_modified = any_raw;
        self.transform_pivot = None;
        if !matches!(self.tool, Tool::Select | Tool::Transform) {
            self.set_tool(Tool::Select);
        }
        true
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

        // 0. 通常の選択モード（Reshape）では、選択済みシェイプ（極細線の外周マージン含む）へのホバーを
        //    すべて移動対象（HoverTarget::Fill）として最優先し、選択済みシェイプの変形暴発を防ぐ
        if self.select_sub_mode == SelectSubMode::Reshape && !self.selected_shapes.is_empty() {
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
                if (self.select_sub_mode == SelectSubMode::Reshape
                    && self.selected_shapes.contains(&s_idx))
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
            if (self.select_sub_mode == SelectSubMode::Reshape
                && self.selected_shapes.contains(&s_idx))
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

    pub(crate) fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.select_sub_mode = SelectSubMode::Reshape;
        self.select_drag = SelectDragState::None;
        self.transform_drag = TransformDragState::None;
        self.hover_target = HoverTarget::None;
        self.transform_hover = TransformHoverTarget::None;
    }

    pub(crate) fn toggle_select_sub_mode(&mut self, target_mode: SelectSubMode) {
        self.select_sub_mode = if self.select_sub_mode == target_mode {
            SelectSubMode::Reshape
        } else {
            target_mode
        };
        self.clear_selection();
        self.select_drag = SelectDragState::None;
    }

    // ------------------------------------------------------------------------
    // 5. ツール別マウス入力ハンドリング（Transform / Brush & Eraser / Select）
    // ------------------------------------------------------------------------

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

    // ------------------------------------------------------------------------
    // 6. カーソル視覚フィードバック & キャンバス合成描画
    // ------------------------------------------------------------------------

    pub(crate) fn draw_tool_cursor_and_feedback(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        pos: Pos2,
    ) {
        match self.tool {
            Tool::Brush | Tool::Eraser => {
                ctx.set_cursor_icon(CursorIcon::Crosshair);
                let r = if self.tool == Tool::Brush {
                    self.brush_radius
                } else {
                    self.eraser_radius
                };
                let ring_col = if self.tool == Tool::Brush {
                    self.brush_color
                } else {
                    Color32::from_rgb(220, 60, 60)
                };
                painter.circle_stroke(pos, r * self.view_zoom, Stroke::new(1.5, ring_col));
            }
            Tool::Bucket => {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                painter.circle_filled(pos, 5.5, self.brush_color);
                painter.circle_stroke(pos, 5.5, Stroke::new(1.8, Color32::WHITE));
            }
            Tool::Eyedropper => {
                let hovered_color = self.topmost_visible_color_at(self.screen_to_world(pos));
                ctx.set_cursor_icon(if hovered_color.is_some() {
                    CursorIcon::PointingHand
                } else {
                    CursorIcon::Crosshair
                });
                draw_eyedropper_cursor_badge(
                    painter,
                    pos,
                    hovered_color.unwrap_or(self.brush_color),
                    hovered_color.is_some(),
                );
            }
            Tool::Transform => self.draw_transform_tool_cursor(ctx, painter, pos),
            Tool::Select => self.draw_select_tool_cursor(ctx, painter, pos),
        }
    }

    fn draw_transform_tool_cursor(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        pos: Pos2,
    ) {
        match &self.transform_drag {
            TransformDragState::Move { .. } | TransformDragState::MovePivot => {
                ctx.set_cursor_icon(CursorIcon::Grabbing);
            }
            TransformDragState::Scale { handle, .. } => {
                ctx.set_cursor_icon(scale_handle_cursor_icon(*handle));
            }
            TransformDragState::Rotate { .. } => {
                ctx.set_cursor_icon(CursorIcon::Grabbing);
                draw_transform_rotate_cursor_badge(painter, pos);
            }
            TransformDragState::MarqueeSelect { .. } => {
                ctx.set_cursor_icon(CursorIcon::Crosshair);
            }
            TransformDragState::None => match self.transform_hover {
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

    fn draw_select_tool_cursor(&self, ctx: &egui::Context, painter: &egui::Painter, pos: Pos2) {
        match &self.select_drag {
            SelectDragState::PullEdge {
                shape_idx,
                contour_idx,
                seg_idx,
                ..
            } => {
                ctx.set_cursor_icon(CursorIcon::Grabbing);
                if let Some(seg) = self.get_contour_segment(*shape_idx, *contour_idx, *seg_idx) {
                    self.draw_highlighted_quad_segment(painter, seg);
                }
                draw_flash_curve_cursor_badge(painter, pos);
            }
            SelectDragState::MoveVertex { snapped_to, .. } => {
                ctx.set_cursor_icon(CursorIcon::Grabbing);
                if let Some(snap_pt) = snapped_to {
                    let sp = self.world_to_screen(*snap_pt);
                    painter.circle_stroke(
                        sp,
                        9.0,
                        Stroke::new(2.5, Color32::from_rgb(255, 50, 140)),
                    );
                    painter.circle_filled(sp, 4.0, Color32::from_rgb(255, 50, 140));
                }
                draw_flash_corner_cursor_badge(painter, pos);
            }
            SelectDragState::MoveShape { .. } => {
                ctx.set_cursor_icon(CursorIcon::Grabbing);
            }
            SelectDragState::MarqueeSelect { .. } => {
                ctx.set_cursor_icon(CursorIcon::Crosshair);
            }
            SelectDragState::None => match self.select_sub_mode {
                SelectSubMode::AddAnchor => {
                    draw_anchor_plus_cursor_badge(painter, pos);
                    if let HoverTarget::Edge {
                        shape_idx,
                        contour_idx,
                        seg_idx,
                        closest_pos,
                        ..
                    } = self.hover_target
                    {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                        if let Some(seg) =
                            self.get_contour_segment(shape_idx, contour_idx, seg_idx)
                        {
                            self.draw_highlighted_quad_segment(painter, seg);
                        }
                        let cp = self.world_to_screen(closest_pos);
                        painter.circle_filled(cp, 5.5, Color32::from_rgb(30, 200, 95));
                        painter.circle_stroke(cp, 5.5, Stroke::new(2.0, Color32::WHITE));
                    } else {
                        ctx.set_cursor_icon(CursorIcon::Default);
                    }
                }
                SelectSubMode::DeleteAnchor => {
                    draw_anchor_minus_cursor_badge(painter, pos);
                    if let HoverTarget::Vertex { pos: v_pos, .. } = self.hover_target {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                        let vp = self.world_to_screen(v_pos);
                        painter.circle_filled(vp, 6.0, Color32::from_rgb(235, 55, 55));
                        painter.circle_stroke(vp, 6.0, Stroke::new(2.0, Color32::WHITE));
                    } else {
                        ctx.set_cursor_icon(CursorIcon::Default);
                    }
                }
                SelectSubMode::Reshape => match self.hover_target {
                    HoverTarget::Vertex {
                        pos: v_pos,
                        shape_idx,
                        contour_idx,
                        ..
                    } => {
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                        if let Some(contour) = self
                            .shapes
                            .get(shape_idx)
                            .and_then(|s| s.get_contour(contour_idx))
                        {
                            for seg in &contour.segments {
                                let p = self.world_to_screen(seg.p0);
                                painter.circle_filled(p, 3.2, Color32::WHITE);
                                painter.circle_stroke(
                                    p,
                                    3.2,
                                    Stroke::new(1.2, Color32::from_rgb(30, 130, 240)),
                                );
                            }
                        }
                        let vp = self.world_to_screen(v_pos);
                        painter.circle_filled(vp, 5.5, Color32::from_rgb(255, 120, 40));
                        painter.circle_stroke(vp, 5.5, Stroke::new(2.0, Color32::WHITE));
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
                        if let Some(seg) =
                            self.get_contour_segment(shape_idx, contour_idx, seg_idx)
                        {
                            self.draw_highlighted_quad_segment(painter, seg);
                        }
                        painter.circle_filled(
                            self.world_to_screen(closest_pos),
                            4.0,
                            Color32::from_rgb(0, 175, 255),
                        );
                        draw_flash_curve_cursor_badge(painter, pos);
                    }
                    HoverTarget::Fill { .. } => {
                        ctx.set_cursor_icon(CursorIcon::Move);
                    }
                    HoverTarget::None => {
                        ctx.set_cursor_icon(CursorIcon::Default);
                    }
                },
            },
        }
    }

    /// ホバー・ドラッグ中の2次ベジェ曲線セグメントとその両端アンカーを強調描画（ハンドルは表示しない）
    pub(crate) fn draw_highlighted_quad_segment(&self, painter: &egui::Painter, seg: QuadBez) {
        let steps = 16;
        let mut prev = seg.p0;
        for k in 1..=steps {
            let curr = seg.eval(k as f64 / steps as f64);
            painter.line_segment(
                [self.world_to_screen(prev), self.world_to_screen(curr)],
                Stroke::new(3.0, Color32::from_rgb(0, 175, 255)),
            );
            prev = curr;
        }
        for end_pt in [seg.p0, seg.p2] {
            let p = self.world_to_screen(end_pt);
            painter.circle_filled(p, 4.0, Color32::WHITE);
            painter.circle_stroke(p, 4.0, Stroke::new(1.8, Color32::from_rgb(0, 140, 230)));
        }
    }

    /// キャンバス上の全シェイプ（オニオンスキン・全可視レイヤー合成）、選択オーバーレイ、グループ選択枠、自由変形ボックス、矩形選択枠、およびストロークプレビューを描画
    pub(crate) fn draw_canvas_contents(&mut self, painter: &egui::Painter) {
        let show_wf = self.show_wireframe
            || (self.tool == Tool::Select && self.select_sub_mode != SelectSubMode::Reshape);
        let active_affine_drag = matches!(
            self.transform_drag,
            TransformDragState::Scale { .. } | TransformDragState::Rotate { .. }
        );
        let in_group_edit = !self.editing_group_path.is_empty();
        let zoom = self.view_zoom;
        let pan = self.view_pan;

        // 0. オニオンスキン描画（前のフレーム＝青系半透明、次のフレーム＝緑系半透明）
        if self.timeline.onion_skin_enabled {
            self.draw_onion_skins(painter);
        }

        // 1. 下層レイヤーから上層レイヤーへ順に合成描画
        let mut selected_group_boxes: Vec<(u64, KRect)> = Vec::new();
        let num_layers = self.timeline.layers.len();
        let curr_f = self.timeline.current_frame;

        for layer_idx in 0..num_layers {
            if !self.timeline.layers[layer_idx].visible {
                continue;
            }
            if self.timeline.layers[layer_idx].locked {
                if let Some(cel) = self.timeline.layers[layer_idx].cel_at(curr_f) {
                    for shape in &cel.shapes {
                        let mesh =
                            transform_mesh_to_screen(&shape.fill_mesh, zoom, pan, in_group_edit);
                        painter.add(egui::Shape::mesh(mesh));
                    }
                }
                continue;
            }

            let lid = self.timeline.layers[layer_idx].id;
            for idx in 0..self.shapes.len() {
                if num_layers > 1 && self.shapes[idx].layer_id != lid {
                    continue;
                }
                let in_scope = self.shapes[idx].is_in_editing_scope(&self.editing_group_path);
                if in_group_edit && !in_scope {
                    let mesh =
                        transform_mesh_to_screen(&self.shapes[idx].fill_mesh, zoom, pan, true);
                    painter.add(egui::Shape::mesh(mesh));
                    continue;
                }

                let mesh = transform_mesh_to_screen(&self.shapes[idx].fill_mesh, zoom, pan, false);
                painter.add(egui::Shape::mesh(mesh));

                if show_wf && self.shapes[idx].is_raw_at_scope(&self.editing_group_path) {
                    self.draw_shape_contour_lines(
                        painter,
                        &self.shapes[idx],
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(30, 80, 180, 140)),
                        true,
                    );
                }

                if self.selected_shapes.contains(&idx) && !active_affine_drag {
                    if let Some(gid) =
                        self.shapes[idx].active_group_id_at_scope(&self.editing_group_path)
                    {
                        let s_bbox = self.shapes[idx].bounding_box();
                        if let Some((_, g_bbox)) =
                            selected_group_boxes.iter_mut().find(|(id, _)| *id == gid)
                        {
                            *g_bbox = g_bbox.union(s_bbox);
                        } else {
                            selected_group_boxes.push((gid, s_bbox));
                        }
                    } else {
                        self.draw_flash_selection_overlay(painter, idx);
                    }
                }
            }
        }

        // 選択中のグループ塊のバウンディングボックス枠（Adobe Animate 風の水色グループ枠）を描画
        for (_, g_bbox) in selected_group_boxes {
            let min_pos =
                self.world_to_screen(Point::new(g_bbox.x0, g_bbox.y0)) - Vec2::splat(2.0);
            let max_pos =
                self.world_to_screen(Point::new(g_bbox.x1, g_bbox.y1)) + Vec2::splat(2.0);
            let rect = egui::Rect::from_min_max(min_pos, max_pos);
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.8, Color32::from_rgb(0, 165, 255)),
            );
        }

        if self.tool == Tool::Transform {
            self.draw_free_transform_box(painter);
        }

        self.draw_active_marquee_box(painter);
        self.draw_active_stroke_preview(painter);
    }

    fn draw_onion_skins(&self, painter: &egui::Painter) {
        let curr_f = self.timeline.current_frame;
        for step in (1..=self.timeline.onion_prev_frames).rev() {
            if curr_f >= step {
                let alpha = (84 / step as u8).clamp(24, 84);
                self.draw_onion_skin_frame(painter, curr_f - step, [45, 115, 245], alpha);
            }
        }
        for step in 1..=self.timeline.onion_next_frames {
            let f = curr_f + step;
            if f < self.timeline.total_frames {
                let alpha = (84 / step as u8).clamp(24, 84);
                self.draw_onion_skin_frame(painter, f, [35, 185, 105], alpha);
            }
        }
    }

    fn draw_onion_skin_frame(
        &self,
        painter: &egui::Painter,
        frame: usize,
        tint_rgb: [u8; 3],
        alpha: u8,
    ) {
        let zoom = self.view_zoom;
        let pan = self.view_pan;
        for layer in &self.timeline.layers {
            if !layer.visible {
                continue;
            }
            if let Some(cel) = layer.cel_at(frame) {
                for shape in &cel.shapes {
                    let mesh = transform_mesh_to_screen_onion(
                        &shape.fill_mesh,
                        zoom,
                        pan,
                        tint_rgb,
                        alpha,
                    );
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
        }
    }

    fn draw_active_marquee_box(&self, painter: &egui::Painter) {
        let active_marquee = match (&self.select_drag, &self.transform_drag) {
            (
                SelectDragState::MarqueeSelect {
                    start_pos,
                    current_pos,
                    ..
                },
                _,
            )
            | (
                _,
                TransformDragState::MarqueeSelect {
                    start_pos,
                    current_pos,
                    ..
                },
            ) => Some((*start_pos, *current_pos)),
            _ => None,
        };
        if let Some((start_pos, current_pos)) = active_marquee {
            let min_pos = self.world_to_screen(Point::new(
                start_pos.x.min(current_pos.x),
                start_pos.y.min(current_pos.y),
            ));
            let max_pos = self.world_to_screen(Point::new(
                start_pos.x.max(current_pos.x),
                start_pos.y.max(current_pos.y),
            ));
            let marquee_rect = egui::Rect::from_min_max(min_pos, max_pos);
            painter.rect_filled(
                marquee_rect,
                0.0,
                Color32::from_rgba_unmultiplied(0, 145, 255, 32),
            );
            painter.rect_stroke(
                marquee_rect,
                0.0,
                Stroke::new(1.5, Color32::from_rgb(0, 145, 255)),
            );
        }
    }

    fn draw_active_stroke_preview(&mut self, painter: &egui::Painter) {
        if self.active_raw_outline.is_empty() {
            return;
        }
        let preview_color = if self.tool == Tool::Brush {
            self.brush_color
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 220)
        };
        let raw_mesh = tessellate_raw_polygon(
            &mut self.tessellator,
            &self.active_raw_outline,
            preview_color,
        );
        painter.add(egui::Shape::mesh(transform_mesh_to_screen(
            &raw_mesh,
            self.view_zoom,
            self.view_pan,
            false,
        )));

        if self.tool == Tool::Eraser {
            let n = self.active_raw_outline.len();
            for i in 0..n {
                painter.line_segment(
                    [
                        self.world_to_screen(self.active_raw_outline[i]),
                        self.world_to_screen(self.active_raw_outline[(i + 1) % n]),
                    ],
                    Stroke::new(1.2, Color32::from_rgb(220, 60, 60)),
                );
            }
        }
    }

    /// マウスホイール / トラックパッドピンチによるカーソル中心ズームと、
    /// Space + ドラッグ / 中ボタンドラッグによるキャンバスパン移動を処理する。
    /// 戻り値は `(is_panning, pan_gesture_active)`。
    pub(crate) fn handle_viewport_navigation(
        &mut self,
        ui: &egui::Ui,
        canvas_rect: egui::Rect,
        pointer_pos: Option<Pos2>,
    ) -> (bool, bool) {
        let mut is_panning = false;
        if let Some(pos) = pointer_pos {
            if canvas_rect.contains(pos) {
                let (scroll_y, pinch_zoom) =
                    ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                if (pinch_zoom - 1.0).abs() > 1e-4 {
                    self.zoom_around_screen_pos(pos, pinch_zoom);
                } else if scroll_y.abs() > 0.01 {
                    let factor = (1.0025_f32).powf(scroll_y);
                    self.zoom_around_screen_pos(pos, factor);
                }
            }
        }

        let (space_down, middle_down, primary_down) = ui.input(|i| {
            (
                i.key_down(Key::Space),
                i.pointer.middle_down(),
                i.pointer.primary_down(),
            )
        });
        let pan_gesture_active = middle_down || (space_down && primary_down);
        if pan_gesture_active {
            is_panning = true;
            if let Some(pos) = pointer_pos {
                if let Some(last_pos) = self.pan_last_screen_pos {
                    self.view_pan += pos - last_pos;
                }
                self.pan_last_screen_pos = Some(pos);
            }
        } else {
            self.pan_last_screen_pos = None;
            if space_down {
                is_panning = true;
            }
        }
        (is_panning, pan_gesture_active)
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
            Stroke::new(2.2, Color32::from_rgb(20, 20, 25)),
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
    let stroke = Stroke::new(2.2, Color32::from_rgb(20, 20, 25));
    painter.line_segment([p0, p1], stroke);
    painter.line_segment([p1, p2], stroke);
}

/// アンカー追加モード時の「＋」カーソルバッジを描画
fn draw_anchor_plus_cursor_badge(painter: &egui::Painter, cursor: Pos2) {
    let base = cursor + Vec2::new(15.0, 15.0);
    painter.circle_filled(base, 8.0, Color32::from_rgb(30, 190, 90));
    painter.circle_stroke(base, 8.0, Stroke::new(1.5, Color32::WHITE));
    let stroke = Stroke::new(2.2, Color32::WHITE);
    painter.line_segment(
        [Pos2::new(base.x - 4.5, base.y), Pos2::new(base.x + 4.5, base.y)],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(base.x, base.y - 4.5), Pos2::new(base.x, base.y + 4.5)],
        stroke,
    );
}

/// アンカー削除モード時の「－」カーソルバッジを描画
fn draw_anchor_minus_cursor_badge(painter: &egui::Painter, cursor: Pos2) {
    let base = cursor + Vec2::new(15.0, 15.0);
    painter.circle_filled(base, 8.0, Color32::from_rgb(230, 55, 55));
    painter.circle_stroke(base, 8.0, Stroke::new(1.5, Color32::WHITE));
    let stroke = Stroke::new(2.2, Color32::WHITE);
    painter.line_segment(
        [Pos2::new(base.x - 4.5, base.y), Pos2::new(base.x + 4.5, base.y)],
        stroke,
    );
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
    painter.circle_filled(base, 8.5, Color32::from_rgba_unmultiplied(255, 255, 255, 235));
    painter.circle_stroke(base, 8.5, Stroke::new(1.5, Color32::from_rgb(0, 125, 240)));
    let r = 5.0_f32;
    let steps = 10;
    let start_ang = -std::f32::consts::FRAC_PI_4 * 3.0;
    let end_ang = std::f32::consts::FRAC_PI_2;
    let stroke = Stroke::new(1.8, Color32::from_rgb(20, 25, 35));
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

/// スポイトツール（`I`）のカラーサンプルリング＆スポイトバッジを描画
fn draw_eyedropper_cursor_badge(
    painter: &egui::Painter,
    cursor: Pos2,
    sample_color: Color32,
    is_over_shape: bool,
) {
    // カーソル先端のターゲットドット
    painter.circle_filled(cursor, 2.5, sample_color);
    painter.circle_stroke(cursor, 2.5, Stroke::new(1.2, Color32::WHITE));

    // 右下に抽出色プレビューバッジを表示
    let badge = cursor + Vec2::new(16.0, 16.0);
    let r = if is_over_shape { 9.5 } else { 7.5 };
    painter.circle_filled(badge, r, sample_color);
    painter.circle_stroke(badge, r, Stroke::new(2.2, Color32::WHITE));
    painter.circle_stroke(
        badge,
        r + 1.2,
        Stroke::new(
            1.2,
            if is_over_shape {
                Color32::from_rgb(0, 145, 255)
            } else {
                Color32::from_rgb(30, 35, 45)
            },
        ),
    );
}
