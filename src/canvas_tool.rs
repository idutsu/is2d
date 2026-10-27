use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Stroke, Vec2};
use kurbo::{ParamCurve, Point, QuadBez, Rect as KRect, Vec2 as KVec2};

use crate::vector_engine::{
    tessellate_raw_polygon, to_pos2, transform_mesh_to_screen, FillShape,
};
use crate::FlashBrushApp;
// ============================================================================
// ツールとインタラクション状態
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    Select,     // V キー
    Transform,  // Q キー（自由変形ツール）
    Brush,      // B キー
    Eraser,     // E キー
    Bucket,     // K キー
    Eyedropper, // I キー（スポイトツール）
}

impl Tool {
    /// 左端縦ツールバーに並べるツールの標準順序（Adobe Animate 準拠）
    pub(crate) const ALL: [Tool; 6] = [
        Tool::Select,
        Tool::Transform,
        Tool::Brush,
        Tool::Eraser,
        Tool::Bucket,
        Tool::Eyedropper,
    ];

    /// 左ツールバーのボタンに表示するアイコン文字列（`MaterialIcons-Regular.ttf` のグリフ）
    pub(crate) const fn icon(self) -> &'static str {
        match self {
            Tool::Select => "\u{e569}",     // near_me（選択ポインタ矢印）
            Tool::Transform => "\u{e3c2}",  // crop_free（自由変形バウンディング枠）
            Tool::Brush => "\u{e3ae}",      // brush（ブラシ）
            Tool::Eraser => "\u{e664}",     // auto_fix_normal（消しゴム）
            Tool::Bucket => "\u{e23a}",     // format_color_fill（バケツ塗り）
            Tool::Eyedropper => "\u{e3b8}", // colorize（スポイト）
        }
    }

    /// プロパティパネル等に表示するツール名（ショートカット表記付き）
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Tool::Select => "選択ツール (V)",
            Tool::Transform => "自由変形ツール (Q)",
            Tool::Brush => "ブラシツール (B)",
            Tool::Eraser => "消しゴムツール (E)",
            Tool::Bucket => "バケツ塗りツール (K)",
            Tool::Eyedropper => "スポイトツール (I)",
        }
    }

    /// ツールバーのホバー時に表示するツールチップ説明文
    pub(crate) const fn tooltip(self) -> &'static str {
        match self {
            Tool::Select => "選択・変形ツール (V)",
            Tool::Transform => "自由変形ツール (Q)",
            Tool::Brush => "ブラシツール (B)",
            Tool::Eraser => "消しゴムツール (E)",
            Tool::Bucket => "バケツ塗りツール (K)",
            Tool::Eyedropper => "スポイトツール (I)",
        }
    }

    /// ツール切り替え用の単一キーショートカット
    pub(crate) const fn shortcut_key(self) -> Key {
        match self {
            Tool::Select => Key::V,
            Tool::Transform => Key::Q,
            Tool::Brush => Key::B,
            Tool::Eraser => Key::E,
            Tool::Bucket => Key::K,
            Tool::Eyedropper => Key::I,
        }
    }

    /// 押下されたショートカットキーから対応するツールを返す
    pub(crate) fn from_shortcut_input(i: &egui::InputState) -> Option<Tool> {
        Self::ALL
            .into_iter()
            .find(|tool| i.key_pressed(tool.shortcut_key()))
    }
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

    /// ステージ（カメラ出力枠）のワールド中心座標を返す（初期 100% ビューで中央キャンバス領域の真ん中に配置）
    #[inline]
    pub(crate) fn stage_center_world(&self) -> Point {
        Point::new(536.0, 310.0)
    }

    /// ステージ（カメラ出力枠）のワールド座標矩形 `KRect` を返す
    #[inline]
    pub(crate) fn stage_world_rect(&self) -> KRect {
        let center = self.stage_center_world();
        let half_w = (self.stage_size.x.max(100.0) as f64) * 0.5;
        let half_h = (self.stage_size.y.max(100.0) as f64) * 0.5;
        KRect::new(
            center.x - half_w,
            center.y - half_h,
            center.x + half_w,
            center.y + half_h,
        )
    }

    /// 現在のズーム・パンに応じたステージのスクリーン座標矩形 `egui::Rect` を返す
    #[inline]
    pub(crate) fn stage_screen_rect(&self) -> egui::Rect {
        let r = self.stage_world_rect();
        egui::Rect::from_min_max(
            self.world_to_screen(Point::new(r.x0, r.y0)),
            self.world_to_screen(Point::new(r.x1, r.y1)),
        )
    }

    /// 図形描画の前に、ペーストボード（枠外のグレー背景）とステージ矩形（白いキャンバス枠）を描画する
    pub(crate) fn draw_stage_background(&self, painter: &egui::Painter, canvas_rect: egui::Rect) {
        if !self.symbol_edit_stack.is_empty() {
            // シンボル内部の編集モード中は部品編集用の明るい全面背景にする
            painter.rect_filled(canvas_rect, 0.0, Color32::from_rgb(250, 250, 252));
            return;
        }
        let pasteboard_col = Color32::from_rgb(218, 222, 228);
        painter.rect_filled(canvas_rect, 0.0, pasteboard_col);

        let stage_rect = self.stage_screen_rect();
        if !self.clip_outside_stage {
            painter.rect_filled(
                stage_rect.translate(Vec2::new(2.0, 2.0)),
                0.0,
                Color32::from_rgba_unmultiplied(0, 0, 0, 20),
            );
        }
        painter.rect_filled(stage_rect, 0.0, self.stage_color);
    }

    /// 図形描画の後に、上からステージの境界枠線（および「枠外を隠す」ON時の外周マスク）を描画する
    pub(crate) fn draw_stage_foreground_overlay(
        &self,
        painter: &egui::Painter,
        canvas_rect: egui::Rect,
    ) {
        if !self.symbol_edit_stack.is_empty() {
            return;
        }
        let stage_rect = self.stage_screen_rect();
        if self.clip_outside_stage {
            let pasteboard_col = Color32::from_rgb(218, 222, 228);
            let sy0 = stage_rect.min.y.clamp(canvas_rect.min.y, canvas_rect.max.y);
            let sy1 = stage_rect.max.y.clamp(canvas_rect.min.y, canvas_rect.max.y);
            let sx0 = stage_rect.min.x.clamp(canvas_rect.min.x, canvas_rect.max.x);
            let sx1 = stage_rect.max.x.clamp(canvas_rect.min.x, canvas_rect.max.x);

            let strips = [
                egui::Rect::from_min_max(canvas_rect.min, Pos2::new(canvas_rect.max.x, sy0)),
                egui::Rect::from_min_max(Pos2::new(canvas_rect.min.x, sy1), canvas_rect.max),
                egui::Rect::from_min_max(
                    Pos2::new(canvas_rect.min.x, sy0),
                    Pos2::new(sx0, sy1),
                ),
                egui::Rect::from_min_max(
                    Pos2::new(sx1, sy0),
                    Pos2::new(canvas_rect.max.x, sy1),
                ),
            ];
            for strip in strips {
                if strip.width() > 0.0 && strip.height() > 0.0 {
                    painter.rect_filled(strip, 0.0, pasteboard_col);
                }
            }
        }
        painter.rect_stroke(
            stage_rect,
            0.0,
            Stroke::new(1.2, Color32::from_rgb(148, 155, 168)),
        );
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

    /// 指定されたシェイプインデックス群が所属するレイヤーのうち最前面の `layer_id` を返す（該当なし時は `active_scope_layer_id`）
    pub(crate) fn topmost_layer_id_for_indices(&self, indices: &[usize]) -> u64 {
        indices
            .iter()
            .filter_map(|&idx| self.shapes.get(idx).map(|s| s.layer_id))
            .max_by_key(|&lid| self.timeline.layer_index_by_id(lid).unwrap_or(0))
            .unwrap_or_else(|| self.active_scope_layer_id())
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
    // 2. ツールモード切り替え（set_tool / toggle_select_sub_mode）
    // ------------------------------------------------------------------------

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
                if self.show_wireframe {
                    if let Some(seg) = self.get_contour_segment(*shape_idx, *contour_idx, *seg_idx)
                    {
                        self.draw_highlighted_quad_segment(painter, seg);
                    }
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
                        if self.show_wireframe {
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
                        if self.show_wireframe {
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
                        }
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
        let show_wf = self.tool == Tool::Select
            && (self.show_wireframe || self.select_sub_mode != SelectSubMode::Reshape);
        let active_affine_drag = matches!(
            self.transform_drag,
            TransformDragState::Scale { .. } | TransformDragState::Rotate { .. }
        );
        let in_group_edit = !self.editing_group_path.is_empty();
        let zoom = self.view_zoom;
        let pan = self.view_pan;

        // 0. シンボル内部タイムライン編集中の場合、親シーンの背景図形を薄暗く（インプレース背景）描画
        if let Some(session) = self.symbol_edit_stack.last() {
            for s in &session.backdrop_shapes {
                let mesh = transform_mesh_to_screen(&s.fill_mesh, zoom, pan, true);
                painter.add(egui::Shape::mesh(mesh));
            }
        }

        // 0.5 オニオンスキン描画（前のフレーム＝青系半透明、次のフレーム＝緑系半透明）
        if self.timeline.onion_skin_enabled {
            self.draw_onion_skins(painter);
        }

        // 1. 下層レイヤーから上層レイヤーへ順に合成描画
        let mut selected_group_boxes: Vec<(u64, KRect, Option<Point>)> = Vec::new();
        let num_layers = self.timeline.layers.len();
        let curr_f = self.timeline.current_frame;

        for layer_idx in 0..num_layers {
            if !self.timeline.layers[layer_idx].visible {
                continue;
            }
            // 再生中またはロック中レイヤーは、Cel 内の結合済みスクリーンキャッシュ Mesh（複数フレームシンボルはフレーム評価込み）を描画
            if self.timeline.is_playing || self.timeline.layers[layer_idx].locked {
                if let Some(mesh) = self.timeline.resolved_cel_screen_mesh(
                    layer_idx,
                    curr_f,
                    zoom,
                    pan,
                    in_group_edit && !self.timeline.is_playing,
                ) {
                    painter.add(egui::Shape::mesh(mesh));
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
                        let sym_reg = self.shapes[idx]
                            .symbol_instance
                            .filter(|b| b.instance_gid == gid)
                            .map(|b| b.xform * Point::ZERO);
                        if let Some((_, g_bbox, _)) =
                            selected_group_boxes.iter_mut().find(|(id, _, _)| *id == gid)
                        {
                            *g_bbox = g_bbox.union(s_bbox);
                        } else {
                            selected_group_boxes.push((gid, s_bbox, sym_reg));
                        }
                    } else {
                        self.draw_flash_selection_overlay(painter, idx);
                    }
                }
            }
        }

        // 選択中のグループ塊（緑枠）およびシンボルインスタンス（水色枠＋基準点 `＋` マーク）を描画
        for (_, g_bbox, sym_reg) in selected_group_boxes {
            let min_pos =
                self.world_to_screen(Point::new(g_bbox.x0, g_bbox.y0)) - Vec2::splat(2.0);
            let max_pos =
                self.world_to_screen(Point::new(g_bbox.x1, g_bbox.y1)) + Vec2::splat(2.0);
            let rect = egui::Rect::from_min_max(min_pos, max_pos);
            if let Some(origin) = sym_reg {
                let cyan = Color32::from_rgb(0, 165, 255);
                painter.rect_stroke(rect, 0.0, Stroke::new(1.8, cyan));
                crate::symbol::draw_registration_cross(
                    painter,
                    self.world_to_screen(origin),
                    3.2,
                    true,
                );
            } else {
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.8, Color32::from_rgb(30, 185, 95)),
                );
            }
        }

        // シンボル内部タイムライン編集中は、シンボルの基準点 `(0, 0)` に常時 `+` クロスヘアを表示
        if let Some(session) = self.symbol_edit_stack.last() {
            let origin = session.instance_xform * Point::ZERO;
            crate::symbol::draw_registration_cross(
                painter,
                self.world_to_screen(origin),
                5.5,
                true,
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
        for (layer_idx, layer) in self.timeline.layers.iter().enumerate() {
            if !layer.visible {
                continue;
            }
            if let Some(mesh) =
                self.timeline
                    .resolved_cel_onion_mesh(layer_idx, frame, zoom, pan, tint_rgb, alpha)
            {
                painter.add(egui::Shape::mesh(mesh));
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
