use std::collections::HashSet;

use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Stroke, Vec2};
use kurbo::{Point, QuadBez, Rect as KRect, Vec2 as KVec2};

use crate::timeline::{LoadedShapeSig, Timeline};
pub(crate) use crate::tool_select::{HoverTarget, SelectDragState, SelectSubMode};
pub(crate) use crate::tool_transform::{TransformDragState, TransformHoverTarget};
use crate::vector_engine::{arc_mesh_to_shape, FillShape};
use crate::Is2dApp;

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
            _ => self.label(),
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

/// 選択ツール (`Tool::Select`) と自由変形ツール (`Tool::Transform`) で共通利用する選択オブジェクト平行移動ドラッグ状態
#[derive(Clone, Debug)]
pub(crate) struct MoveSelectionDrag {
    pub(crate) last_pos: Point,
    pub(crate) total_delta: KVec2,
    pub(crate) undo_snapshot: Vec<FillShape>,
}

impl MoveSelectionDrag {
    pub(crate) fn new(start_pos: Point, mut undo_snapshot: Vec<FillShape>) -> Self {
        for s in &mut undo_snapshot {
            s.strip_transient_caches();
        }
        Self {
            last_pos: start_pos,
            total_delta: KVec2::ZERO,
            undo_snapshot,
        }
    }

    pub(crate) fn step(&mut self, pt: Point) -> KVec2 {
        let step_delta = pt - self.last_pos;
        self.last_pos = pt;
        self.total_delta += step_delta;
        step_delta
    }
}

/// 選択ツール (`Tool::Select`) と自由変形ツール (`Tool::Transform`) で共通利用する矩形マーキー選択ドラッグ状態
#[derive(Clone, Copy, Debug)]
pub(crate) struct MarqueeDrag {
    pub(crate) start_pos: Point,
    pub(crate) current_pos: Point,
    pub(crate) additive: bool,
}

impl MarqueeDrag {
    pub(crate) fn new(start_pos: Point, additive: bool) -> Self {
        Self {
            start_pos,
            current_pos: start_pos,
            additive,
        }
    }
}

/// キャンバス上で進行中のドラッグ操作を表す排他的状態マシン
/// （ブラシ/消しゴムのストローク、選択ツールのドラッグ、自由変形ツールのドラッグが同時に複数存在できないことを型で保証する）
#[derive(Clone, Debug, Default)]
pub(crate) enum ActiveCanvasDrag {
    #[default]
    None,
    PaintStroke {
        stroke: Vec<Point>,
        committed_mesh: egui::Mesh,
        tail_outline: Vec<Point>,
        tail_mesh: egui::Mesh,
        tail_start_idx: usize,
    },
    Select(SelectDragState),
    Transform(TransformDragState),
}

/// キャンバスのズーム・パン・ステージ出力枠を管理するビューポート状態構造体
pub(crate) struct ViewportState {
    /// キャンバスのズーム倍率（1.0 = 100%, 範囲 0.15..=16.0）
    pub(crate) zoom: f32,
    /// キャンバスのパンオフセット（スクリーンピクセル単位）
    pub(crate) pan: Vec2,
    /// スペースドラッグ / 中ボタンドラッグによるパン操作中の直前スクリーン座標
    pub(crate) pan_last_screen_pos: Option<Pos2>,
    /// ステージ（カメラ出力枠）のワールドサイズ（幅・高さ px、デフォルト 800×450 [16:9]）
    pub(crate) stage_size: Vec2,
    /// ステージ枠外にはみ出た図形をマスクして隠すかどうか（完成画面プレビュー用）
    pub(crate) clip_outside_stage: bool,
    /// 直近フレームの中央キャンバス描画領域（スクリーン座標、「ウィンドウに合わせる」計算用）
    pub(crate) last_canvas_rect: egui::Rect,
    /// キャンバスタブのステージサイズ `DragValue` をドラッグ・入力中か（連続変更時のUndo重複記録防止用）
    pub(crate) editing_stage_size: bool,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            pan_last_screen_pos: None,
            stage_size: Vec2::new(800.0, 450.0),
            clip_outside_stage: false,
            last_canvas_rect: egui::Rect::from_min_max(
                Pos2::new(52.0, 30.0),
                Pos2::new(1000.0, 590.0),
            ),
            editing_stage_size: false,
        }
    }
}

impl ViewportState {
    /// ワールド座標 `p` を現在のズーム・パンに応じたスクリーン座標 `Pos2` へ変換する
    #[inline]
    pub(crate) fn world_to_screen(&self, p: Point) -> Pos2 {
        Pos2::new(
            (p.x as f32) * self.zoom + self.pan.x,
            (p.y as f32) * self.zoom + self.pan.y,
        )
    }

    /// スクリーン座標 `pos` を現在のズーム・パンに応じたワールド座標 `Point` へ逆変換する
    #[inline]
    pub(crate) fn screen_to_world(&self, pos: Pos2) -> Point {
        Point::new(
            ((pos.x - self.pan.x) / self.zoom) as f64,
            ((pos.y - self.pan.y) / self.zoom) as f64,
        )
    }

    /// スクリーン上の指定位置 `anchor_screen`（マウスカーソル位置）がズーム前後で動かないように倍率 `factor` を掛ける
    pub(crate) fn zoom_around_screen_pos(&mut self, anchor_screen: Pos2, factor: f32) {
        let world_before = self.screen_to_world(anchor_screen);
        self.zoom = (self.zoom * factor).clamp(
            crate::vector_engine::MIN_VIEW_ZOOM,
            crate::vector_engine::MAX_VIEW_ZOOM,
        );
        self.pan = Vec2::new(
            anchor_screen.x - (world_before.x as f32) * self.zoom,
            anchor_screen.y - (world_before.y as f32) * self.zoom,
        );
    }

    /// ズーム倍率を 100%（`1.0`）、パン位置を原点（`Vec2::ZERO`）へリセットする（`Cmd+1` / `Cmd+0`）
    pub(crate) fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    /// ステージ枠全体が中央キャンバス領域に余白付きでぴったり収まるようにズーム倍率とパン位置を自動調整する（「ウィンドウに合わせる」）
    pub(crate) fn fit_stage_to_window(&mut self) {
        let stage_w = self.stage_size.x.max(100.0);
        let stage_h = self.stage_size.y.max(100.0);
        let avail_w = (self.last_canvas_rect.width() - 48.0).max(64.0);
        let avail_h = (self.last_canvas_rect.height() - 48.0).max(64.0);
        let zoom = (avail_w / stage_w)
            .min(avail_h / stage_h)
            .clamp(
                crate::vector_engine::MIN_VIEW_ZOOM,
                crate::vector_engine::MAX_VIEW_ZOOM,
            );
        let center = self.stage_center_world();
        let screen_center = self.last_canvas_rect.center();
        self.zoom = zoom;
        self.pan = Vec2::new(
            screen_center.x - (center.x as f32) * zoom,
            screen_center.y - (center.y as f32) * zoom,
        );
    }

    /// ステージ（カメラ出力枠）の初期ワールド中心座標
    pub(crate) const DEFAULT_STAGE_CENTER: Point = Point::new(536.0, 310.0);

    /// ステージ（カメラ出力枠）のワールド中心座標を返す（初期 100% ビューで中央キャンバス領域の真ん中に配置）
    #[inline]
    pub(crate) fn stage_center_world(&self) -> Point {
        Self::DEFAULT_STAGE_CENTER
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

    /// ワールド座標矩形 `r` をスクリーン座標矩形 `egui::Rect` へ変換する
    #[inline]
    pub(crate) fn world_rect_to_screen(&self, r: KRect) -> egui::Rect {
        egui::Rect::from_min_max(
            self.world_to_screen(Point::new(r.x0, r.y0)),
            self.world_to_screen(Point::new(r.x1, r.y1)),
        )
    }

    /// 現在のズーム・パンに応じたステージのスクリーン座標矩形 `egui::Rect` を返す
    #[inline]
    pub(crate) fn stage_screen_rect(&self) -> egui::Rect {
        self.world_rect_to_screen(self.stage_world_rect())
    }

    /// 現在スクリーン上に表示されるワールド座標範囲 `KRect` を返す（画面外・ステージ枠外カリング用）
    #[inline]
    pub(crate) fn visible_world_rect(
        &self,
        painter: &egui::Painter,
        in_symbol_edit: bool,
    ) -> KRect {
        let clip_scr = painter.clip_rect();
        let view_rect = KRect::from_points(
            self.screen_to_world(clip_scr.min),
            self.screen_to_world(clip_scr.max),
        );
        if self.clip_outside_stage && !in_symbol_edit {
            view_rect.intersect(self.stage_world_rect())
        } else {
            view_rect
        }
    }

    /// 図形描画の前に、ペーストボード（枠外のグレー背景）とステージ矩形（白いキャンバス枠）を描画する
    pub(crate) fn draw_stage_background(
        &self,
        painter: &egui::Painter,
        canvas_rect: egui::Rect,
        in_symbol_edit: bool,
    ) {
        if in_symbol_edit {
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
        painter.rect_filled(stage_rect, 0.0, Color32::WHITE);
    }

    /// 「枠外を隠す (`clip_outside_stage`)」有効時に、アートワーク（背景図形・オニオンスキン・シェイプ本体）のみを
    /// ステージ矩形でクリッピングした専用 `Painter` を返す（選択枠や変形ハンドルは親 `painter` に描画するため隠れない）
    pub(crate) fn artwork_painter(
        &self,
        painter: &egui::Painter,
        canvas_rect: egui::Rect,
        in_symbol_edit: bool,
    ) -> egui::Painter {
        if !in_symbol_edit && self.clip_outside_stage {
            painter.with_clip_rect(canvas_rect.intersect(self.stage_screen_rect()))
        } else {
            painter.clone()
        }
    }

    /// 図形描画の後に、上からステージの境界枠線を描画する
    pub(crate) fn draw_stage_foreground_overlay(
        &self,
        painter: &egui::Painter,
        _canvas_rect: egui::Rect,
        in_symbol_edit: bool,
    ) {
        if in_symbol_edit {
            return;
        }
        let stage_rect = self.stage_screen_rect();
        painter.rect_stroke(
            stage_rect,
            0.0,
            Stroke::new(1.2, Color32::from_rgb(148, 155, 168)),
        );
    }

    /// マウスホイール / トラックパッドピンチによるカーソル中心ズームと、
    /// Space + ドラッグ / 中ボタンドラッグによるキャンバスパン移動を処理する。
    /// 戻り値は `(is_panning, pan_gesture_active)`。
    pub(crate) fn handle_navigation(
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
                    self.pan += pos - last_pos;
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

/// 現在フレームの作業図形・選択・変形・排他ドラッグ・クリップボードを管理するキャンバスセッション状態構造体
#[derive(Default)]
pub(crate) struct CanvasState {
    pub(crate) shapes: Vec<FillShape>,
    /// 直近の `load_shapes_from_timeline` 読み込み時点のシェイプ署名一覧（`Timeline` スナップショットに含めない作業用キャッシュ）
    loaded_signatures: Vec<LoadedShapeSig>,
    /// キャンバス上で進行中のドラッグ操作（ブラシ/消しゴム・選択・自由変形の排他的状態マシン）
    pub(crate) drag: ActiveCanvasDrag,
    pub(crate) selected_shapes: Vec<usize>,
    unsliced_canvas: Option<Vec<FillShape>>,
    pub(crate) selection_modified: bool,
    pub(crate) hover_target: HoverTarget,
    /// 現在インプレイス編集で潜っているグループ階層のパス（空 `[]` ならルートシーン）
    pub(crate) editing_group_path: Vec<u64>,
    /// コピー / カットされたシェイプ群（`group_path` はコピー時の編集スコープからの相対パス）
    pub(crate) clipboard: Vec<FillShape>,
    /// `Cmd+V` による通常ペーストの連続回数（少しずつずらして配置するためのカウンタ）
    pub(crate) paste_count: usize,
    pub(crate) transform_pivot: Option<Point>,
    pub(crate) transform_hover: TransformHoverTarget,
}

impl CanvasState {
    /// 直近の読み込み・同期時点のシェイプ署名スライスを返す
    #[inline]
    pub(crate) fn loaded_signatures(&self) -> &[LoadedShapeSig] {
        &self.loaded_signatures
    }

    /// 矩形スライス前の未スライス状態スナップショットが保持されているかを返す
    #[inline]
    pub(crate) fn has_unsliced_snapshot(&self) -> bool {
        self.unsliced_canvas.is_some()
    }

    /// 矩形スライス前の未スライス状態スナップショットを取り出す
    #[inline]
    pub(crate) fn take_unsliced_snapshot(&mut self) -> Option<Vec<FillShape>> {
        self.unsliced_canvas.take()
    }

    /// 矩形スライス前の未スライス状態スナップショットを保存・復元する
    #[inline]
    pub(crate) fn restore_unsliced_snapshot(&mut self, snapshot: Vec<FillShape>) {
        self.unsliced_canvas = Some(snapshot);
    }

    /// 矩形スライス前の未スライス状態スナップショットを破棄する
    #[inline]
    pub(crate) fn clear_unsliced_snapshot(&mut self) {
        self.unsliced_canvas = None;
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

    /// 矩形スライス前のキャンバス状態があれば取り出し、なければ現在の `self.shapes` をクローンして返す
    #[inline]
    pub(crate) fn take_or_clone_unsliced_snapshot(&mut self) -> Vec<FillShape> {
        let mut snap = self
            .unsliced_canvas
            .take()
            .unwrap_or_else(|| self.shapes.clone());
        for s in &mut snap {
            s.strip_transient_caches();
        }
        snap
    }

    /// ドラッグ開始用に現在の `self.shapes` のスナップショットを取得する
    /// （スナップショット側の一時メッシュキャッシュを除去しておくことで、ドラッグ中の `Arc::make_mut` による頂点配列クローンを回避する）
    #[inline]
    pub(crate) fn snapshot_for_drag(&self) -> Vec<FillShape> {
        let mut snap = self.shapes.clone();
        for s in &mut snap {
            s.strip_transient_caches();
        }
        snap
    }

    /// ドラッグが閾値未満でキャンセルされた際に `snapshot` から `self.shapes` を復元し、署名キャッシュを同期する
    pub(crate) fn restore_drag_snapshot(&mut self, snapshot: Vec<FillShape>) -> bool {
        let len_changed = self.shapes.len() != snapshot.len();
        self.shapes = snapshot;
        self.selected_shapes.retain(|&idx| idx < self.shapes.len());
        if self.unsliced_canvas.is_none() {
            self.record_loaded_signatures();
        }
        len_changed
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

    /// 現在の編集スコープ（`editing_group_path`）内で、指定座標 `pt` にヒットする最前面シェイプのインデックスを返す
    pub(crate) fn topmost_shape_at(&self, pt: Point) -> Option<usize> {
        (0..self.shapes.len()).rev().find(|&idx| {
            self.shapes[idx].is_in_editing_scope(&self.editing_group_path)
                && self.shapes[idx].contains_point(pt)
        })
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

    /// 選択中のシェイプ群の `(インデックス, FillShape クローン)` 一覧を返す
    /// （ドラッグ変形中にキャッシュ頂点を直接変換できるよう、クローン前に `fill_mesh` を初期化しておく）
    pub(crate) fn selected_shapes_snapshot(&self) -> Vec<(usize, FillShape)> {
        self.selected_shapes
            .iter()
            .filter_map(|&idx| {
                self.shapes.get(idx).map(|s| {
                    let _ = s.fill_mesh();
                    (idx, s.clone())
                })
            })
            .collect()
    }

    /// 選択中シェイプ群の全体のバウンディングボックスを返す
    /// （可視シェイプが含まれる場合は `geometry_bounding_box()` の外接矩形を返し、空白フレームシンボルマーカーのみの場合はその基準点から算出する）
    pub(crate) fn selection_bounding_box(&self, zoom: f32) -> Option<KRect> {
        let mut geom_iter = self
            .selected_shapes
            .iter()
            .filter_map(|&idx| self.shapes.get(idx))
            .filter_map(|s| s.geometry_bounding_box());
        let mut bbox = match geom_iter.next() {
            Some(first) => geom_iter.fold(first, |acc, r| acc.union(r)),
            None => {
                let mut fallback = self
                    .selected_shapes
                    .iter()
                    .filter_map(|&idx| self.shapes.get(idx))
                    .map(|s| s.bounding_box());
                let first_fb = fallback.next()?;
                fallback.fold(first_fb, |acc, r| acc.union(r))
            }
        };
        let min_extent = 6.0 / (zoom as f64);
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

    /// 選択中の全シェイプとカスタムピボット点を `delta` だけ平行移動する
    pub(crate) fn translate_selected(&mut self, delta: KVec2) {
        for &sel_idx in &self.selected_shapes {
            if let Some(shape) = self.shapes.get_mut(sel_idx) {
                shape.translate(delta);
            }
        }
        if let Some(piv) = &mut self.transform_pivot {
            *piv += delta;
        }
    }

    /// 選択中の全シェイプに `affine` 変換を直接適用する
    pub(crate) fn transform_selected(&mut self, affine: kurbo::Affine) -> bool {
        let mut any_transformed = false;
        for &sel_idx in &self.selected_shapes {
            if let Some(shape) = self.shapes.get_mut(sel_idx) {
                shape.transform(affine);
                any_transformed = true;
            }
        }
        any_transformed
    }

    /// 自由変形ドラッグ中、ドラッグ開始時点の `orig_shapes` に対してキャッシュ済みメッシュ頂点変換で `affine` を適用する
    /// （ドラッグ中の毎フレームの再テッセレーションを回避し、マウスリリース確定時にのみ再構築する）
    pub(crate) fn transform_selected_from_origins(
        shapes: &mut [FillShape],
        orig_shapes: &[(usize, FillShape)],
        affine: kurbo::Affine,
    ) {
        for (sel_idx, orig_shape) in orig_shapes.iter() {
            if let Some(shape) = shapes.get_mut(*sel_idx) {
                shape.clone_from(orig_shape);
                shape.transform_with_cached_mesh(affine);
            }
        }
    }

    /// キャンバス図形更新後に、存在しなくなった `editing_group_path`・範囲外の `selected_shapes`・`hover_target` を自動補正する
    pub(crate) fn sanitize_editing_scope(&mut self) {
        self.selected_shapes.retain(|&idx| idx < self.shapes.len());
        if self
            .hover_target
            .shape_idx()
            .is_some_and(|idx| idx >= self.shapes.len())
        {
            self.hover_target = HoverTarget::None;
        }
        while !self.editing_group_path.is_empty() {
            if self
                .shapes
                .iter()
                .any(|s| s.is_in_editing_scope(&self.editing_group_path))
            {
                break;
            }
            self.editing_group_path.pop();
        }
    }

    /// `self.shapes` の現在のシェイプ署名一覧を `loaded_signatures` に記録する（既存バッファを再利用してアロケーションを回避）
    pub(crate) fn record_loaded_signatures(&mut self) {
        self.loaded_signatures.clear();
        self.loaded_signatures
            .extend(self.shapes.iter().map(LoadedShapeSig::from_shape));
    }

    /// 選択インデックス・部分スライス復元用バックアップ・変形ピボット等の選択状態を一括クリアする
    pub(crate) fn reset_selection_state(&mut self) {
        self.selected_shapes.clear();
        self.unsliced_canvas = None;
        self.selection_modified = false;
        self.transform_pivot = None;
    }

    /// タイムラインから読み込んだ図形列で `self.shapes` を置き換える
    pub(crate) fn replace_loaded_shapes(&mut self, loaded: Vec<FillShape>) {
        self.shapes = loaded;
        self.record_loaded_signatures();
        self.reset_selection_state();
        self.sanitize_editing_scope();
    }

    /// 現在の編集スコープ内で指定レイヤーID集合 `layer_ids` に属する全図形を選択状態にする
    pub(crate) fn select_shapes_in_layers(&mut self, layer_ids: &std::collections::HashSet<u64>) {
        self.selected_shapes = (0..self.shapes.len())
            .filter(|&i| {
                self.shapes[i].is_in_editing_scope(&self.editing_group_path)
                    && layer_ids.contains(&self.shapes[i].layer_id)
            })
            .collect();
        self.transform_pivot = None;
    }

    /// `self.shapes` 全体が直近の読み込み・同期時点から一切変更されていないかを判定する（ヒープ確保なし）
    pub(crate) fn is_all_shapes_unchanged(&self) -> bool {
        self.loaded_signatures.len() == self.shapes.len()
            && self
                .loaded_signatures
                .iter()
                .zip(self.shapes.iter())
                .all(|(sig, s)| sig.matches_with_layer(s))
    }

    /// 現在選択中のシェイプ群を現在の編集スコープ（`editing_group_path`）からの相対グループパス付きで抽出する
    pub(crate) fn extract_selected_relative_to_scope(&self) -> Vec<FillShape> {
        let (is_sel, any_valid) = self.selected_shape_mask();
        if !any_valid {
            return Vec::new();
        }
        let depth = self.editing_group_path.len();
        let mut extracted = Vec::with_capacity(self.selected_shapes.len());
        for (idx, shape) in self.shapes.iter().enumerate() {
            if is_sel[idx] {
                let rel_group_path = if shape.group_path.starts_with(&self.editing_group_path) {
                    shape.group_path[depth..].to_vec()
                } else {
                    Vec::new()
                };
                extracted.push(shape.clone().with_group_path(rel_group_path));
            }
        }
        extracted
    }

    /// 指定した `instance_gid` に属するキャンバス上の図形をすべて選択状態にする
    pub(crate) fn select_symbol_instance_by_gid(&mut self, instance_gid: u64) {
        self.selected_shapes = (0..self.shapes.len())
            .filter(|&i| {
                self.shapes[i]
                    .symbol_instance
                    .is_some_and(|b| b.instance_gid == instance_gid)
            })
            .collect();
        self.transform_pivot = None;
    }

    /// 現在選択されている図形が単一のシンボルインスタンスである場合、その `SymbolInstanceBinding` を返す
    pub(crate) fn selected_single_symbol_binding(
        &self,
    ) -> Option<crate::symbol::SymbolInstanceBinding> {
        if self.selected_shapes.is_empty() {
            return None;
        }
        let first_binding = self.shapes.get(self.selected_shapes[0])?.symbol_instance?;
        let all_same = self.selected_shapes.iter().all(|&idx| {
            self.shapes.get(idx).is_some_and(|s| {
                s.symbol_instance
                    .is_some_and(|b| b.instance_gid == first_binding.instance_gid)
            })
        });
        all_same.then_some(first_binding)
    }

    /// 指定された `instance_gid` を持つキャンバス上の全シンボルインスタンス図形に対して `update` を一括適用する
    pub(crate) fn update_symbol_instance_binding(
        &mut self,
        instance_gid: u64,
        mut update: impl FnMut(&mut crate::symbol::SymbolInstanceBinding),
    ) -> bool {
        let mut any_updated = false;
        for shape in &mut self.shapes {
            if let Some(b) = shape
                .symbol_instance
                .as_mut()
                .filter(|b| b.instance_gid == instance_gid)
            {
                update(b);
                any_updated = true;
            }
        }
        any_updated
    }

    /// 選択ツールまたは自由変形ツールで進行中だった図形変形・移動ドラッグのみを中断して巻き戻し、
    /// 図形数が変化していた場合（タイムライン再同期が必要な場合）は `true` を返す（ブラシのストローク状態には干渉しない）
    pub(crate) fn rollback_shape_drag(&mut self) -> bool {
        if matches!(
            self.drag,
            ActiveCanvasDrag::None | ActiveCanvasDrag::PaintStroke { .. }
        ) {
            return false;
        }
        let mut rollback_snapshot: Option<Vec<FillShape>> = None;
        match std::mem::replace(&mut self.drag, ActiveCanvasDrag::None) {
            ActiveCanvasDrag::Select(SelectDragState::MoveShape(drag))
            | ActiveCanvasDrag::Transform(TransformDragState::Move(drag)) => {
                rollback_snapshot = Some(drag.undo_snapshot);
            }
            ActiveCanvasDrag::Select(
                SelectDragState::PullEdge { undo_snapshot, .. }
                | SelectDragState::MoveVertex { undo_snapshot, .. },
            )
            | ActiveCanvasDrag::Transform(TransformDragState::Rotate { undo_snapshot, .. }) => {
                rollback_snapshot = Some(undo_snapshot);
            }
            ActiveCanvasDrag::Transform(
                TransformDragState::Scale {
                    orig_pivot,
                    undo_snapshot,
                    ..
                }
                | TransformDragState::MovePivot {
                    orig_pivot,
                    undo_snapshot,
                    ..
                },
            ) => {
                rollback_snapshot = Some(undo_snapshot);
                if self.transform_pivot.is_some() {
                    self.transform_pivot = Some(orig_pivot);
                }
            }
            ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect(_))
            | ActiveCanvasDrag::Transform(TransformDragState::MarqueeSelect(_))
            | ActiveCanvasDrag::PaintStroke { .. }
            | ActiveCanvasDrag::None => {}
        }

        if let Some(snapshot) = rollback_snapshot {
            self.restore_drag_snapshot(snapshot)
        } else {
            false
        }
    }

    /// 現在選択されている図形群の不変エンティティID（`shape_id`）一覧を昇順ソート済みで返す
    pub(crate) fn selected_shape_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self
            .selected_shapes
            .iter()
            .filter_map(|&idx| self.shapes.get(idx).map(|s| s.shape_id))
            .collect();
        ids.sort_unstable();
        ids
    }

    /// 指定された `shape_id` 群に一致するキャンバス図形をすべて選択状態に復元する
    pub(crate) fn restore_selection_by_shape_ids(&mut self, selected_ids: &[u64]) {
        self.selected_shapes.clear();
        if selected_ids.is_empty() {
            return;
        }
        let is_sorted = selected_ids.windows(2).all(|w| w[0] <= w[1]);
        if is_sorted {
            for (idx, s) in self.shapes.iter().enumerate() {
                if selected_ids.binary_search(&s.shape_id).is_ok() {
                    self.selected_shapes.push(idx);
                }
            }
        } else {
            let mut sorted = selected_ids.to_vec();
            sorted.sort_unstable();
            for (idx, s) in self.shapes.iter().enumerate() {
                if sorted.binary_search(&s.shape_id).is_ok() {
                    self.selected_shapes.push(idx);
                }
            }
        }
    }

    /// キャンバス上のシェイプ群が `timeline.layers` の下層→上層順（同一レイヤー内では元の前後順）になるよう安定ソートし、
    /// `FillShape::shape_id` に基づいて `self.selected_shapes` を追従させる
    pub(crate) fn sort_shapes_by_layer_order(&mut self, timeline: &Timeline) {
        if self.shapes.len() <= 1 || timeline.layers.len() <= 1 {
            return;
        }
        let is_already_sorted = self.shapes.windows(2).all(|w| {
            let o0 = timeline
                .layer_index_by_id(w[0].layer_id)
                .unwrap_or(usize::MAX);
            let o1 = timeline
                .layer_index_by_id(w[1].layer_id)
                .unwrap_or(usize::MAX);
            o0 <= o1
        });
        if is_already_sorted {
            return;
        }

        let selected_ids = self.selected_shape_ids();
        self.shapes.sort_by_key(|s| {
            timeline
                .layer_index_by_id(s.layer_id)
                .unwrap_or(usize::MAX)
        });
        self.restore_selection_by_shape_ids(&selected_ids);
    }

    /// 自由変形ツールの現在の基準点（ピボット座標）を返す
    /// （単一シンボルインスタンス選択時はインスタンスに保存されたピボット座標 `binding.world_pivot()` を返す）
    pub(crate) fn active_transform_pivot(&self, bbox: KRect) -> Point {
        if let Some(piv) = self.transform_pivot {
            return piv;
        }
        if let Some(binding) = self.selected_single_symbol_binding() {
            return binding.world_pivot();
        }
        bbox.center()
    }

    /// 現在の選択対象に初期位置から移動されたカスタム変形中心（ピボット）が設定されているかを返す
    pub(crate) fn has_custom_transform_pivot(&self) -> bool {
        if self.transform_pivot.is_some() {
            return true;
        }
        self.selected_single_symbol_binding()
            .is_some_and(|b| (b.pivot - Point::ZERO).hypot() > 1e-6)
    }
}

// ============================================================================
// キャンバス操作・選択・変形・グループ編集・描画実装
// ============================================================================

// ------------------------------------------------------------------------
// 1. レイヤー解決と選択同期（Canvas × Timeline 協調）
// ------------------------------------------------------------------------

/// 現在の編集スコープ（グループ内であればそのグループの所属レイヤー、そうでなければ現在のアクティブレイヤー）の `layer_id` を返す
pub(crate) fn active_scope_layer_id(app: &Is2dApp) -> u64 {
    if !app.canvas.editing_group_path.is_empty() {
        if let Some(s) = app
            .canvas
            .shapes
            .iter()
            .find(|s| s.is_in_editing_scope(&app.canvas.editing_group_path))
        {
            return s.layer_id;
        }
    }
    if app.timeline.is_active_layer_editable() {
        app.timeline.active_layer_id()
    } else {
        app.timeline
            .layers
            .iter()
            .rev()
            .find(|l| l.visible && !l.locked)
            .map(|l| l.id)
            .unwrap_or_else(|| app.timeline.active_layer_id())
    }
}

/// 指定されたシェイプインデックス群が所属するレイヤーのうち最前面の `layer_id` を返す（該当なし時は `active_scope_layer_id`）
pub(crate) fn topmost_layer_id_for_indices(app: &Is2dApp, indices: &[usize]) -> u64 {
    indices
        .iter()
        .filter_map(|&idx| app.canvas.shapes.get(idx).map(|s| s.layer_id))
        .max_by_key(|&lid| app.timeline.layer_index_by_id(lid).unwrap_or(0))
        .unwrap_or_else(|| active_scope_layer_id(app))
}

/// 指定したシェイプインデックス `shape_idx` が属するレイヤーへ `app.timeline.active_layer` およびタイムライン選択状態を追従させる
pub(crate) fn focus_active_layer_to_shape(app: &mut Is2dApp, shape_idx: usize) {
    app.ui.inspect_timeline_frame = false;
    if let Some(lid) = app.canvas.shapes.get(shape_idx).map(|s| s.layer_id) {
        if let Some(l_idx) = app.timeline.layer_index_by_id(lid) {
            app.timeline.active_layer = l_idx;
            let cf = app.timeline.current_frame;
            app.timeline.cell_selection =
                Some(crate::timeline::TimelineCellSelection::single(l_idx, cf));
        }
    }
}

/// キャンバス上で選択されたシェイプ群（`app.canvas.selected_shapes`）が所属するレイヤーをタイムライン上で選択状態にする
pub(crate) fn sync_timeline_layer_from_canvas_selection(app: &mut Is2dApp) {
    app.ui.inspect_timeline_frame = false;
    let mut min_l: Option<usize> = None;
    let mut max_l: Option<usize> = None;
    let mut last_l: Option<usize> = None;
    for &idx in &app.canvas.selected_shapes {
        if let Some(lid) = app.canvas.shapes.get(idx).map(|s| s.layer_id) {
            if let Some(l_idx) = app.timeline.layer_index_by_id(lid) {
                min_l = Some(min_l.map_or(l_idx, |m| m.min(l_idx)));
                max_l = Some(max_l.map_or(l_idx, |m| m.max(l_idx)));
                last_l = Some(l_idx);
            }
        }
    }
    if let (Some(l0), Some(l1), Some(act)) = (min_l, max_l, last_l) {
        app.timeline.active_layer = act;
        let cf = app.timeline.current_frame;
        app.timeline.cell_selection = Some(
            crate::timeline::TimelineCellSelection::from_corners(l0, cf, l1, cf),
        );
    }
}

// ------------------------------------------------------------------------
// 2. ドラッグ中断ロールバック & ツールモード切り替え（rollback_shape_drag / cancel_active_drag / set_tool）
// ------------------------------------------------------------------------

/// 選択ツールまたは自由変形ツールで進行中だった図形変形・移動ドラッグのみを中断し、
/// プレビュー変形中だった図形をドラッグ開始前の状態へ安全に巻き戻す（ブラシのストローク状態には干渉しない）
pub(crate) fn rollback_shape_drag(app: &mut Is2dApp) {
    if app.canvas.rollback_shape_drag() {
        sync_shapes_to_timeline(app);
    }
}

/// 進行中のすべてのキャンバスドラッグ操作（ブラシ/消しゴム・輪郭変形・移動・自由変形）を中断し、
/// プレビュー変形中だった図形をドラッグ開始前の状態へ安全に巻き戻す
pub(crate) fn cancel_active_drag(app: &mut Is2dApp) {
    rollback_shape_drag(app);
    app.canvas.drag = ActiveCanvasDrag::None;
    app.canvas.hover_target = HoverTarget::None;
    app.canvas.transform_hover = TransformHoverTarget::None;
}

pub(crate) fn set_tool(app: &mut Is2dApp, tool: Tool) {
    cancel_active_drag(app);
    app.ui.tool = tool;
    app.ui.select_sub_mode = SelectSubMode::Reshape;
}

pub(crate) fn toggle_select_sub_mode(app: &mut Is2dApp, target_mode: SelectSubMode) {
    app.ui.select_sub_mode = if app.ui.select_sub_mode == target_mode {
        SelectSubMode::Reshape
    } else {
        target_mode
    };
    crate::tool_select::clear_selection(app);
}

// ------------------------------------------------------------------------
// 3. キャンバス ⇔ タイムライン状態同期（sync / load / mutate_timeline_with_undo）
// ------------------------------------------------------------------------

/// 再生中であれば停止し、停止時点のフレームの図形群を作業用 `app.canvas.shapes` に展開する
pub(crate) fn stop_playback(app: &mut Is2dApp) {
    if app.timeline_ui.is_playing {
        app.timeline_ui.is_playing = false;
        app.timeline_ui.last_frame_advance_time = None;
        load_shapes_from_timeline(app);
    }
}

/// タイムラインの構造変更前に、未確定のキャンバス選択スライスを確定解除し `app.canvas.shapes` を現在フレームのセルへ同期する
/// （再生中の場合は古い `app.canvas.shapes` で現在フレームを上書きしないよう、再生停止して現在フレームの図形を読み込む）
pub(crate) fn flush_canvas_before_timeline_op(app: &mut Is2dApp) {
    if app.timeline_ui.is_playing {
        stop_playback(app);
    } else {
        crate::tool_select::clear_selection(app);
        sync_shapes_to_timeline(app);
    }
}

/// 作業キャンバスを確定同期し、Undo履歴を記録した上でタイムライン変更処理 `mutate` を実行し、完了後に `app.canvas.shapes` を再読み込みする
pub(crate) fn mutate_timeline_with_undo(app: &mut Is2dApp, mutate: impl FnOnce(&mut Timeline)) {
    flush_canvas_before_timeline_op(app);
    app.push_undo();
    mutate(&mut app.timeline);
    app.symbols.sync_active_edit(&app.timeline);
    load_shapes_from_timeline(app);
}

/// キャンバスの作業用 `app.canvas.shapes` を、各図形の `layer_id` ごとに現在フレームの `Cel` に振り分けて書き戻す
pub(crate) fn sync_shapes_to_timeline(app: &mut Is2dApp) {
    app.canvas.sanitize_editing_scope();
    if app.timeline_ui.is_playing || app.canvas.is_all_shapes_unchanged() {
        return;
    }
    app.timeline
        .normalize_shape_layer_ids(&mut app.canvas.shapes);
    app.canvas.sort_shapes_by_layer_order(&app.timeline);
    let any_modified = app.timeline.write_shapes_to_editable_cels(
        &app.canvas.shapes,
        &app.symbols.library,
        app.canvas.loaded_signatures(),
    );
    app.canvas.record_loaded_signatures();
    if any_modified {
        app.symbols.sync_active_edit(&app.timeline);
    }
}

/// 現在フレームにおける全ての「表示中かつ非ロック（`visible && !locked`）」レイヤーの `Cel` から
/// 背面レイヤー→前面レイヤーの順で（トゥイーン補間およびシンボルの内部タイムラインフレームも評価しつつ）`app.canvas.shapes` を一括読み込みする
pub(crate) fn load_shapes_from_timeline(app: &mut Is2dApp) {
    cancel_active_drag(app);
    let frame = app.timeline.current_frame;
    let mut loaded = Vec::new();
    for layer_idx in 0..app.timeline.layers.len() {
        let layer = &app.timeline.layers[layer_idx];
        if !layer.visible || layer.locked {
            continue;
        }
        loaded.extend(app.timeline.evaluate_layer_shapes_at(
            layer_idx,
            frame,
            &app.symbols.library,
        ));
    }
    app.canvas.replace_loaded_shapes(loaded);
    if let Some(sym_id) = app.symbols.edit_stack.last().map(|s| s.symbol_id) {
        let cf = app.timeline.current_frame;
        let al = app.timeline.active_layer;
        if let Some(sym_def) = app.symbols.find_symbol_mut(sym_id) {
            if sym_def.timeline.current_frame != cf || sym_def.timeline.active_layer != al {
                sym_def.timeline.current_frame = cf;
                sym_def.timeline.active_layer = al;
                app.symbols.bump_revision();
            }
        }
    }
}

/// タイムライン上で選択されたセル範囲（単一キーフレームまたは複数レイヤー範囲）に属するすべての図形をキャンバス上で選択状態にする
pub(crate) fn select_shapes_for_timeline_selection(app: &mut Is2dApp) {
    crate::tool_select::clear_selection(app);
    let Some(sel) = app.timeline.cell_selection else {
        return;
    };
    let selected_lids: HashSet<u64> = (sel.min_layer..=sel.max_layer)
        .filter_map(|l_idx| app.timeline.layers.get(l_idx))
        .filter(|l| l.visible && !l.locked)
        .map(|l| l.id)
        .collect();
    app.canvas.select_shapes_in_layers(&selected_lids);
    app.ui.inspect_timeline_frame = true;
}

// ------------------------------------------------------------------------
// 4. カーソル視覚フィードバック & キャンバス合成描画
// ------------------------------------------------------------------------

pub(crate) fn draw_tool_cursor_and_feedback(
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match app.ui.tool {
        Tool::Brush | Tool::Eraser => {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            let r = if app.ui.tool == Tool::Brush {
                app.ui.brush_radius
            } else {
                app.ui.eraser_radius
            };
            let ring_col = if app.ui.tool == Tool::Brush {
                app.ui.brush_color
            } else {
                Color32::from_rgb(220, 60, 60)
            };
            painter.circle_stroke(pos, r * app.viewport.zoom, Stroke::new(1.5, ring_col));
        }
        Tool::Bucket => {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            painter.circle_filled(pos, 5.5, app.ui.brush_color);
            painter.circle_stroke(pos, 5.5, Stroke::new(1.8, Color32::WHITE));
        }
        Tool::Eyedropper => {
            let hovered_color =
                crate::tool_paint::topmost_visible_color_at(app, app.viewport.screen_to_world(pos));
            ctx.set_cursor_icon(if hovered_color.is_some() {
                CursorIcon::PointingHand
            } else {
                CursorIcon::Crosshair
            });
            crate::tool_paint::draw_eyedropper_cursor_badge(
                painter,
                pos,
                hovered_color.unwrap_or(app.ui.brush_color),
                hovered_color.is_some(),
            );
        }
        Tool::Transform => crate::tool_transform::draw_transform_tool_cursor(app, ctx, painter, pos),
        Tool::Select => crate::tool_select::draw_select_tool_cursor(app, ctx, painter, pos),
    }
}

/// キャンバス上の全シェイプ（オニオンスキン・全可視レイヤー合成）、選択オーバーレイ、グループ選択枠、自由変形ボックス、矩形選択枠、およびストロークプレビューを描画
pub(crate) fn draw_canvas_contents(app: &mut Is2dApp, painter: &egui::Painter) {
    let in_group_edit = !app.canvas.editing_group_path.is_empty();
    let in_symbol_edit = !app.symbols.edit_stack.is_empty();
    let zoom = app.viewport.zoom;
    let pan = app.viewport.pan;
    let artwork_painter =
        app.viewport
            .artwork_painter(painter, painter.clip_rect(), in_symbol_edit);

    // 0. シンボル内部タイムライン編集中の場合、親シーンの背景図形を1つの結合メッシュで薄暗く（インプレース背景）描画
    if let Some(session) = app.symbols.edit_stack.last() {
        if let Some(mesh) = session.backdrop_cel.cached_backdrop_screen_mesh(
            session.parent_timeline.current_frame,
            zoom,
            pan,
            &app.symbols.library,
        ) {
            artwork_painter.add(arc_mesh_to_shape(mesh));
        }
    }

    // 0.5 オニオンスキン描画（前のフレーム＝青系半透明、次のフレーム＝緑系半透明）
    if app.timeline_ui.onion_skin_enabled {
        draw_onion_skins(app, &artwork_painter);
    }

    // 1. 下層レイヤーから上層レイヤーへ順に合成描画（`app.canvas.shapes` はレイヤー順に整列されているため単一カーソルで O(N) 走査）
    let num_layers = app.timeline.layers.len();
    let mut selected_group_boxes: Vec<(u64, Option<KRect>, KRect, Option<Point>)> = Vec::new();
    let (is_sel, has_sel) = app.canvas.selected_shape_mask();
    let curr_f = app.timeline.current_frame;
    let view_world_rect = app
        .viewport
        .visible_world_rect(&artwork_painter, in_symbol_edit);
    let mut shape_cursor = 0usize;

    for layer_idx in 0..num_layers {
        let Some((lid, is_visible, is_locked)) = app
            .timeline
            .layers
            .get(layer_idx)
            .map(|l| (l.id, l.visible, l.locked))
        else {
            continue;
        };
        if !is_visible {
            continue;
        }
        if app.timeline_ui.is_playing || is_locked {
            if let Some(mesh) = app.timeline.resolved_cel_screen_mesh(
                layer_idx,
                curr_f,
                zoom,
                pan,
                in_group_edit && !app.timeline_ui.is_playing,
                &app.symbols.library,
            ) {
                artwork_painter.add(arc_mesh_to_shape(mesh));
            }
            continue;
        }

        if num_layers > 1 {
            while shape_cursor < app.canvas.shapes.len()
                && app.canvas.shapes[shape_cursor].layer_id != lid
                && app
                    .timeline
                    .layer_index_by_id(app.canvas.shapes[shape_cursor].layer_id)
                    .is_none_or(|ord| ord < layer_idx)
            {
                shape_cursor += 1;
            }
        }
        let start_idx = shape_cursor;
        while shape_cursor < app.canvas.shapes.len()
            && (num_layers <= 1 || app.canvas.shapes[shape_cursor].layer_id == lid)
        {
            shape_cursor += 1;
        }
        draw_editable_layer_shapes(
            app,
            (&artwork_painter, painter),
            start_idx..shape_cursor,
            view_world_rect,
            (&is_sel, has_sel),
            &mut selected_group_boxes,
        );
    }

    app.viewport
        .draw_stage_foreground_overlay(painter, painter.clip_rect(), in_symbol_edit);
    draw_selected_group_and_symbol_boxes(app, painter, &selected_group_boxes);

    if app.ui.tool == Tool::Transform {
        crate::tool_transform::draw_free_transform_box(&app.canvas, painter, &app.viewport);
    }
    draw_active_marquee_box(app, painter);
    crate::tool_paint::draw_active_stroke_preview(app, &artwork_painter, painter);
}

/// 編集可能レイヤー1枚分の図形スライス範囲 `range` を描画し、選択中のグループ／シンボル外枠情報を集計する
fn draw_editable_layer_shapes(
    app: &mut Is2dApp,
    (artwork_painter, gizmo_painter): (&egui::Painter, &egui::Painter),
    range: std::ops::Range<usize>,
    view_world_rect: KRect,
    (is_sel, has_sel): (&[bool], bool),
    selected_group_boxes: &mut Vec<(u64, Option<KRect>, KRect, Option<Point>)>,
) {
    let show_wf = app.ui.tool == Tool::Select
        && (app.ui.show_wireframe || app.ui.select_sub_mode != SelectSubMode::Reshape);
    let active_affine_drag = matches!(
        app.canvas.drag,
        ActiveCanvasDrag::Transform(
            TransformDragState::Scale { .. } | TransformDragState::Rotate { .. }
        )
    );
    let in_group_edit = !app.canvas.editing_group_path.is_empty();
    let zoom = app.viewport.zoom;
    let pan = app.viewport.pan;

    for idx in range {
        let Some(shape) = app.canvas.shapes.get(idx) else {
            continue;
        };
        let in_scope = shape.is_in_editing_scope(&app.canvas.editing_group_path);
        let shape_sel = in_scope
            && has_sel
            && !active_affine_drag
            && is_sel.get(idx).copied().unwrap_or(false);
        let sel_gid = if shape_sel {
            let gid = shape.active_group_id_at_scope(&app.canvas.editing_group_path);
            if let Some(gid) = gid {
                let geom_bbox = shape.geometry_bounding_box();
                let fallback_bbox = shape.bounding_box();
                let sym_reg = shape
                    .symbol_instance
                    .filter(|b| b.instance_gid == gid)
                    .map(|b| b.xform * Point::ZERO);
                if let Some((_, g_geom, g_fallback, _)) =
                    selected_group_boxes.iter_mut().find(|(id, _, _, _)| *id == gid)
                {
                    if let Some(gb) = geom_bbox {
                        *g_geom = Some(g_geom.map_or(gb, |acc| acc.union(gb)));
                    }
                    *g_fallback = g_fallback.union(fallback_bbox);
                } else {
                    selected_group_boxes.push((gid, geom_bbox, fallback_bbox, sym_reg));
                }
            }
            gid
        } else {
            None
        };

        let Some(geom_bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !crate::vector_engine::rects_overlap(geom_bbox, view_world_rect) {
            continue;
        }

        let dim = in_group_edit && !in_scope;
        artwork_painter.add(arc_mesh_to_shape(shape.cached_screen_mesh(zoom, pan, dim)));
        if !in_scope {
            continue;
        }
        if show_wf && shape.is_raw_at_scope(&app.canvas.editing_group_path) {
            crate::tool_select::draw_shape_contour_lines(
                app,
                gizmo_painter,
                shape,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(30, 80, 180, 140)),
                true,
            );
        }
        if shape_sel && sel_gid.is_none() {
            crate::tool_select::draw_flash_selection_overlay(app, gizmo_painter, idx);
        }
    }
}

/// シンボル基準点（Registration Point `+`）をスクリーン座標 `center` に描画する共通ヘルパー
pub(crate) fn draw_registration_cross(
    painter: &egui::Painter,
    center: Pos2,
    arm: f32,
    with_white_halo: bool,
) {
    if with_white_halo {
        let halo = Stroke::new(3.0, Color32::WHITE);
        painter.line_segment(
            [
                Pos2::new(center.x - arm, center.y),
                Pos2::new(center.x + arm, center.y),
            ],
            halo,
        );
        painter.line_segment(
            [
                Pos2::new(center.x, center.y - arm),
                Pos2::new(center.x, center.y + arm),
            ],
            halo,
        );
    }
    let fg = Stroke::new(1.4, Color32::from_rgb(0, 125, 220));
    painter.line_segment(
        [
            Pos2::new(center.x - arm, center.y),
            Pos2::new(center.x + arm, center.y),
        ],
        fg,
    );
    painter.line_segment(
        [
            Pos2::new(center.x, center.y - arm),
            Pos2::new(center.x, center.y + arm),
        ],
        fg,
    );
}

/// 選択中のグループ塊（緑枠）およびシンボルインスタンス（水色枠＋基準点 `＋` マーク）、シンボル編集中の基準点を描画
fn draw_selected_group_and_symbol_boxes(
    app: &Is2dApp,
    painter: &egui::Painter,
    selected_group_boxes: &[(u64, Option<KRect>, KRect, Option<Point>)],
) {
    for &(_, geom_bbox, fallback_bbox, sym_reg) in selected_group_boxes {
        let g_bbox = geom_bbox.unwrap_or(fallback_bbox);
        let rect = app.viewport.world_rect_to_screen(g_bbox).expand(2.0);
        if let Some(origin) = sym_reg {
            painter.rect_stroke(rect, 0.0, Stroke::new(1.8, Color32::from_rgb(0, 165, 255)));
            draw_registration_cross(
                painter,
                app.viewport.world_to_screen(origin),
                3.2,
                true,
            );
        } else {
            painter.rect_stroke(rect, 0.0, Stroke::new(1.8, Color32::from_rgb(30, 185, 95)));
        }
    }

    if let Some(session) = app.symbols.edit_stack.last() {
        let origin = session.instance_xform * Point::ZERO;
        draw_registration_cross(
            painter,
            app.viewport.world_to_screen(origin),
            5.5,
            true,
        );
    }
}

fn draw_onion_skins(app: &Is2dApp, painter: &egui::Painter) {
    let curr_f = app.timeline.current_frame;
    for step in (1..=app.timeline_ui.onion_prev_frames).rev() {
        if curr_f >= step {
            let alpha = (84 / step as u8).clamp(24, 84);
            draw_onion_skin_frame(app, painter, curr_f - step, [45, 115, 245], alpha);
        }
    }
    for step in 1..=app.timeline_ui.onion_next_frames {
        let f = curr_f + step;
        if f < app.timeline.total_frames {
            let alpha = (84 / step as u8).clamp(24, 84);
            draw_onion_skin_frame(app, painter, f, [35, 185, 105], alpha);
        }
    }
}

fn draw_onion_skin_frame(
    app: &Is2dApp,
    painter: &egui::Painter,
    frame: usize,
    tint_rgb: [u8; 3],
    alpha: u8,
) {
    let zoom = app.viewport.zoom;
    let pan = app.viewport.pan;
    for (layer_idx, layer) in app.timeline.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        if let Some(mesh) = app.timeline.resolved_cel_onion_mesh(
            layer_idx,
            frame,
            zoom,
            pan,
            (tint_rgb, alpha),
            &app.symbols.library,
        ) {
            painter.add(arc_mesh_to_shape(mesh));
        }
    }
}

fn draw_active_marquee_box(app: &Is2dApp, painter: &egui::Painter) {
    let active_marquee = match &app.canvas.drag {
        ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect(m))
        | ActiveCanvasDrag::Transform(TransformDragState::MarqueeSelect(m)) => Some(*m),
        _ => None,
    };
    if let Some(m) = active_marquee {
        let marquee_rect = app
            .viewport
            .world_rect_to_screen(KRect::from_points(m.start_pos, m.current_pos));
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
