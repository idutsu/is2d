use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Key, Pos2, Stroke, Vec2};
use kurbo::{Point, QuadBez, Rect as KRect, Vec2 as KVec2};

use crate::symbol::{SymbolDef, SymbolEditSession, SymbolManager};
use crate::timeline::{LoadedShapeSig, Timeline, TimelineCellSelection};
use crate::vector_boolean::merge_shapes_into_canvas;
use crate::vector_engine::FillShape;

#[cfg(test)]
pub(crate) const MAX_UNDO_HISTORY: usize = 100;
#[cfg(not(test))]
const MAX_UNDO_HISTORY: usize = 100;
const DEFAULT_TOAST_DURATION: Duration = Duration::from_millis(2800);

// ============================================================================
// 1. トースト通知・右パネルタブ・ツール・ドラッグ状態マシン
// ============================================================================

/// キャンバス左上に表示する通知・警告メッセージの種別
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    /// 操作不可や条件不足の警告（黄色系バッジ）
    Warning,
    /// エラー通知（赤系バッジ）
    Error,
    /// 通常の案内・情報（青系バッジ）
    Info,
}

impl NoticeKind {
    /// 種別に応じた `(背景色, 枠線色, 文字色)` を返す
    pub(crate) fn colors(self) -> (Color32, Color32, Color32) {
        match self {
            NoticeKind::Warning => (
                Color32::from_rgba_unmultiplied(255, 248, 225, 244),
                Color32::from_rgb(215, 165, 55),
                Color32::from_rgb(145, 90, 10),
            ),
            NoticeKind::Error => (
                Color32::from_rgba_unmultiplied(255, 236, 236, 244),
                Color32::from_rgb(220, 75, 75),
                Color32::from_rgb(155, 25, 25),
            ),
            NoticeKind::Info => (
                Color32::from_rgba_unmultiplied(235, 245, 255, 244),
                Color32::from_rgb(55, 140, 225),
                Color32::from_rgb(20, 75, 145),
            ),
        }
    }
}

/// 操作時に一定時間表示して自動で消えるトースト通知メッセージ
#[derive(Clone, Debug)]
pub(crate) struct StatusToast {
    pub(crate) kind: NoticeKind,
    pub(crate) message: String,
    pub(crate) expires_at: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum RightPanelTab {
    #[default]
    Properties,
    Color,
    Library,
    Canvas,
}

impl RightPanelTab {
    /// 右パネル上部タブバーに並べる標準順序
    pub(crate) const ALL: [RightPanelTab; 4] = [
        RightPanelTab::Properties,
        RightPanelTab::Color,
        RightPanelTab::Library,
        RightPanelTab::Canvas,
    ];

    /// 右パネルタブバーに表示する見出し文字列
    pub(crate) const fn label(self) -> &'static str {
        match self {
            RightPanelTab::Properties => "プロパティ",
            RightPanelTab::Color => "カラー",
            RightPanelTab::Library => "ライブラリ",
            RightPanelTab::Canvas => "キャンバス",
        }
    }
}

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

    /// 押下されたショートカットキー（キーリピート除外）から対応するツールを返す
    pub(crate) fn from_shortcut_input(i: &egui::InputState) -> Option<Tool> {
        Self::ALL.into_iter().find(|tool| {
            let target = tool.shortcut_key();
            i.events.iter().any(|ev| {
                matches!(
                    ev,
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        ..
                    } if *key == target
                )
            })
        })
    }
}

/// 選択ツール (`Tool::Select`) と自由変形ツール (`Tool::Transform`) で共通利用する選択オブジェクト平行移動ドラッグ状態
#[derive(Clone, Debug)]
pub(crate) struct MoveSelectionDrag {
    pub(crate) last_pos: Point,
    pub(crate) total_delta: KVec2,
    pub(crate) undo_snapshot: Vec<FillShape>,
    pub(crate) orig_selected_ids: Vec<u64>,
}

impl MoveSelectionDrag {
    pub(crate) fn new(start_pos: Point, undo_snapshot: Vec<FillShape>) -> Self {
        Self {
            last_pos: start_pos,
            total_delta: KVec2::ZERO,
            undo_snapshot,
            orig_selected_ids: Vec::new(),
        }
    }

    pub(crate) fn with_orig_selected_ids(mut self, orig_selected_ids: Vec<u64>) -> Self {
        self.orig_selected_ids = orig_selected_ids;
        self
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum SelectSubMode {
    #[default]
    Reshape,      // 通常の輪郭変形・頂点移動・図形移動（V キー）
    AddAnchor,    // ＋ アンカー追加モード（+ キー）
    DeleteAnchor, // － アンカー削除モード（- キー）
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) enum HoverTarget {
    #[default]
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
    MoveShape(MoveSelectionDrag),
    MarqueeSelect(MarqueeDrag),
    PullEdge {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        grab_t: f64,
        grab_pos: Point,
        grab_offset: KVec2,
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

// ============================================================================
// 2. ステージ・ビューポート状態（ViewportState）
// ============================================================================

/// キャンバスのズーム・パン・ステージ出力枠を管理するビューポート状態構造体
#[derive(Clone)]
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
        let zoom = (avail_w / stage_w).min(avail_h / stage_h).clamp(
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
            Stroke::new(1.2_f32, Color32::from_rgb(148, 155, 168)),
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

// ============================================================================
// 3. キャンバスセッション状態（SharedClipboard / CanvasState）
// ============================================================================

/// タブ切り替えを跨いで共有されるクリップボード状態
#[derive(Clone, Default)]
pub(crate) struct SharedClipboard {
    pub(crate) clipboard: Vec<FillShape>,
    pub(crate) clipboard_source_doc_id: u64,
    pub(crate) clipboard_source_symbol_id: Option<u64>,
    pub(crate) clipboard_source_symbols_revision: u64,
    pub(crate) clipboard_symbols: Vec<Arc<SymbolDef>>,
    pub(crate) clipboard_imported_map: HashMap<(u64, u64), u64>,
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
    /// クリップボードにコピーされた時点のコピー元ドキュメントID（別タブへのペースト判定用）
    pub(crate) clipboard_source_doc_id: u64,
    /// クリップボードにコピーされた時点のインプレイス編集中のシンボルID（ルートシーン時は `None`）
    pub(crate) clipboard_source_symbol_id: Option<u64>,
    /// クリップボードにコピーされた時点のコピー元 `SymbolManager::revision`
    pub(crate) clipboard_source_symbols_revision: u64,
    /// クリップボード内のシェイプが参照しているシンボル定義のスナップショット群（依存順）
    pub(crate) clipboard_symbols: Vec<Arc<SymbolDef>>,
    /// 現在のコピー元から各貼り付け先タブへ既にインポート済みのシンボルID対応表 `(target_doc_id, source_sym_id) -> target_sym_id`
    pub(crate) clipboard_imported_map: HashMap<(u64, u64), u64>,
    /// `Cmd+V` 押下時に `Event::Paste` または `Key::V` (pressed) で既にペーストを実行済みかのフラグ（OSクリップボード空時のキーリリースフォールバック二重発火防止用）
    pub(crate) paste_triggered_on_press: bool,
    /// `Cmd+V` による通常ペーストの連続回数（少しずつずらして配置するためのカウンタ）
    pub(crate) paste_count: usize,
    /// `Cmd+V` による通常ペーストの累積ワールド座標オフセット（途中でズーム変更しても階段状にずらす用）
    pub(crate) paste_offset: KVec2,
    pub(crate) transform_pivot: Option<Point>,
    pub(crate) transform_hover: TransformHoverTarget,
}

impl CanvasState {
    /// タブ切り替え時に共有クリップボード一式を取り出す（元のスロットには空のクリップボードを残す）
    pub(crate) fn take_shared_clipboard(&mut self) -> SharedClipboard {
        self.paste_count = 0;
        self.paste_offset = KVec2::ZERO;
        SharedClipboard {
            clipboard: std::mem::take(&mut self.clipboard),
            clipboard_source_doc_id: self.clipboard_source_doc_id,
            clipboard_source_symbol_id: self.clipboard_source_symbol_id,
            clipboard_source_symbols_revision: self.clipboard_source_symbols_revision,
            clipboard_symbols: std::mem::take(&mut self.clipboard_symbols),
            clipboard_imported_map: std::mem::take(&mut self.clipboard_imported_map),
        }
    }

    /// タブ切り替え後に共有クリップボード一式を復元する
    pub(crate) fn restore_shared_clipboard(&mut self, shared: SharedClipboard) {
        self.clipboard = shared.clipboard;
        self.clipboard_source_doc_id = shared.clipboard_source_doc_id;
        self.clipboard_source_symbol_id = shared.clipboard_source_symbol_id;
        self.clipboard_source_symbols_revision = shared.clipboard_source_symbols_revision;
        self.clipboard_symbols = shared.clipboard_symbols;
        self.clipboard_imported_map = shared.clipboard_imported_map;
        self.paste_count = 0;
        self.paste_offset = KVec2::ZERO;
    }

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

    /// 現在の `self.shapes` から、幾何輪郭と色が一致する `target_shapes` へ描画・当たり判定キャッシュを O(1) で引き継ぐ
    pub(crate) fn salvage_caches_into(&self, target_shapes: &mut [FillShape]) {
        if self.shapes.is_empty() || target_shapes.is_empty() {
            return;
        }
        for (idx, target) in target_shapes.iter_mut().enumerate() {
            if let Some(same_pos) = self.shapes.get(idx) {
                if target.reuse_caches_from(same_pos) {
                    continue;
                }
            }
            for donor in &self.shapes {
                if target.reuse_caches_from(donor) {
                    break;
                }
            }
        }
    }

    /// 矩形スライス前のキャンバス状態があれば取り出し（既存キャッシュを引き継ぎ）、なければ現在の `self.shapes` をクローンして返す
    #[inline]
    pub(crate) fn take_or_clone_unsliced_snapshot(&mut self) -> Vec<FillShape> {
        if let Some(mut snap) = self.unsliced_canvas.take() {
            self.salvage_caches_into(&mut snap);
            snap
        } else {
            self.shapes.clone()
        }
    }

    /// ドラッグ開始用に現在の `self.shapes` のスナップショットを取得する
    /// （微小ドラッグやキャンセル時に再テッセレーションが起きないよう `Arc<Mesh>` キャッシュを共有保持し、Undoスタック格納時にのみ除去する）
    #[inline]
    pub(crate) fn snapshot_for_drag(&self) -> Vec<FillShape> {
        self.shapes.clone()
    }

    /// ドラッグが閾値未満でキャンセルされた際に `snapshot` から `self.shapes` を復元し、署名キャッシュを同期する
    pub(crate) fn restore_drag_snapshot(&mut self, mut snapshot: Vec<FillShape>) -> bool {
        self.salvage_caches_into(&mut snapshot);
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
            self.shapes.get(idx).is_some_and(|s| {
                s.active_group_id_at_scope(&self.editing_group_path)
                    .is_some()
            })
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

    /// 自由変形ドラッグ中、ドラッグ開始時点の `orig_shapes` に対してキャッシュ済みメッシュ頂点バッファを再利用して `affine` を適用する
    /// （ドラッグ中の毎フレームの再テッセレーションおよび頂点バッファ再確保を回避し、マウスリリース確定時にのみ再構築する）
    pub(crate) fn transform_selected_from_origins(
        shapes: &mut [FillShape],
        orig_shapes: &[(usize, FillShape)],
        affine: kurbo::Affine,
    ) {
        for (sel_idx, orig_shape) in orig_shapes.iter() {
            if let Some(shape) = shapes.get_mut(*sel_idx) {
                shape.transform_from_origin(orig_shape, affine);
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

    /// タイムラインから読み込んだ図形列で `self.shapes` を置き換える（既存シェイプと輪郭・色が一致するものは描画キャッシュを引き継ぐ）
    pub(crate) fn replace_loaded_shapes(&mut self, mut loaded: Vec<FillShape>) {
        self.salvage_caches_into(&mut loaded);
        self.shapes = loaded;
        self.record_loaded_signatures();
        self.reset_selection_state();
        self.sanitize_editing_scope();
    }

    /// 現在の編集スコープ内で指定レイヤーID集合 `layer_ids` に属する全図形を選択状態にする
    pub(crate) fn select_shapes_in_layers(&mut self, layer_ids: &HashSet<u64>) {
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
        let mut rollback_selected_ids: Vec<u64> = Vec::new();
        match std::mem::replace(&mut self.drag, ActiveCanvasDrag::None) {
            ActiveCanvasDrag::Select(SelectDragState::MoveShape(drag))
            | ActiveCanvasDrag::Transform(TransformDragState::Move(drag)) => {
                rollback_snapshot = Some(drag.undo_snapshot);
                rollback_selected_ids = drag.orig_selected_ids;
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
            let len_changed = self.restore_drag_snapshot(snapshot);
            if !rollback_selected_ids.is_empty() {
                self.restore_selection_by_shape_ids(&rollback_selected_ids);
            }
            len_changed
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
        self.shapes
            .sort_by_key(|s| timeline.layer_index_by_id(s.layer_id).unwrap_or(usize::MAX));
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
// 4. Undo / Redo 履歴管理（UndoEntry / UndoHistory）& アプリUI状態（AppUiState）
// ============================================================================

#[derive(Clone, Debug)]
pub(crate) struct UndoEntry {
    pub(crate) timeline: Timeline,
    pub(crate) library: Arc<Vec<SymbolDef>>,
    pub(crate) editing_group_path: Vec<u64>,
    pub(crate) symbol_edit_stack: Arc<Vec<SymbolEditSession>>,
    pub(crate) stage_size: Vec2,
    pub(crate) symbols_revision: u64,
    /// この履歴スナップショットが表すドキュメント状態世代ID（未保存変更 `is_dirty` 判定用）
    pub(crate) commit_id: u64,
}

impl UndoEntry {
    /// スナップショットから随時再生成可能な一時メッシュキャッシュ（`fill_mesh`, `selection_mesh`, `CelMeshCache`）を除去し、
    /// 直前の履歴スナップショット `prev_timeline` と未変更のセルは `Arc` を $O(1)$ 共有した軽量状態にする
    pub(crate) fn strip_transient_caches(mut self, prev_timeline: Option<&Timeline>) -> Self {
        self.timeline
            .strip_transient_caches_with_prev(prev_timeline);
        self
    }
}

/// Undo / Redo スナップショット履歴および保存時点のドキュメント世代IDを管理する構造体
pub(crate) struct UndoHistory {
    pub(crate) undo_stack: VecDeque<UndoEntry>,
    pub(crate) redo_stack: Vec<UndoEntry>,
    /// 現在のドキュメント状態の世代ID（初期状態・読み込み直後は `0`）
    pub(crate) current_commit_id: u64,
    /// 直近の保存時点（または未編集の初期状態）の世代ID（`Some(current_commit_id)` と一致していれば未変更）
    pub(crate) saved_commit_id: Option<u64>,
    /// 次に発行する新しい状態世代ID（単調増加）
    pub(crate) next_commit_id: u64,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            current_commit_id: 0,
            saved_commit_id: Some(0),
            next_commit_id: 1,
        }
    }
}

impl UndoHistory {
    /// 現在のドキュメントに未保存の変更があるか（保存時点の世代IDと現在世代IDが異なるか）を返す
    #[inline]
    pub(crate) fn is_dirty(&self) -> bool {
        self.saved_commit_id != Some(self.current_commit_id)
    }

    /// 現在のドキュメント世代IDを「保存済み」として記録する
    #[inline]
    pub(crate) fn mark_saved(&mut self) {
        self.saved_commit_id = Some(self.current_commit_id);
    }

    /// 保存済み世代IDをクリアして常時「未保存」状態にする（別タブによる同名ファイル上書き時のデタッチ用）
    #[inline]
    pub(crate) fn mark_unsaved(&mut self) {
        self.saved_commit_id = None;
    }

    /// シンボルライブラリおよび編集スタックが直近の履歴エントリから未変更（同一 `revision`）であれば
    /// `Arc` ポインタを $O(1)$ で共有し、一部シンボルのみ変更されている場合は未変更シンボルおよび
    /// 退避中の親タイムライン未変更セルの `Arc` を引き継ぎながら差分シンボルのみキャッシュ除去して新しい `Arc` を返す
    pub(crate) fn snapshot_symbols(
        &self,
        symbols: &SymbolManager,
    ) -> (Arc<Vec<SymbolDef>>, Arc<Vec<SymbolEditSession>>, u64) {
        let prev_entry = self.undo_stack.back().or_else(|| self.redo_stack.last());
        if let Some(reused) = prev_entry.filter(|e| e.symbols_revision == symbols.revision) {
            return (
                Arc::clone(&reused.library),
                Arc::clone(&reused.symbol_edit_stack),
                symbols.revision,
            );
        }
        let mut lib = Vec::with_capacity(symbols.library.len());
        for sym in &symbols.library {
            if let Some(prev_sym) = prev_entry.and_then(|pe| {
                pe.library
                    .iter()
                    .find(|s| s.id == sym.id && s.revision == sym.revision)
            }) {
                lib.push(prev_sym.clone());
            } else {
                let prev_sym_tl = prev_entry.and_then(|pe| {
                    pe.library
                        .iter()
                        .find(|s| s.id == sym.id)
                        .map(|s| &s.timeline)
                });
                let mut cloned = sym.clone();
                cloned.timeline.strip_transient_caches_with_prev(prev_sym_tl);
                cloned.clear_preview_and_anim_caches();
                lib.push(cloned);
            }
        }
        let mut stack = Vec::with_capacity(symbols.edit_stack.len());
        for (idx, sess) in symbols.edit_stack.iter().enumerate() {
            let prev_sess = prev_entry
                .and_then(|pe| pe.symbol_edit_stack.get(idx))
                .filter(|ps| ps.symbol_id == sess.symbol_id && ps.instance_gid == sess.instance_gid);
            let mut cloned_sess = sess.clone();
            cloned_sess.strip_transient_caches_with_prev(prev_sess);
            stack.push(cloned_sess);
        }
        (Arc::new(lib), Arc::new(stack), symbols.revision)
    }

    /// 新しい Undo エントリをスタックへ追加し、Redo スタックをクリアして現在の状態世代IDを進める
    pub(crate) fn push(&mut self, mut entry: UndoEntry) {
        entry.commit_id = self.current_commit_id;
        self.current_commit_id = self.next_commit_id;
        self.next_commit_id = self.next_commit_id.saturating_add(1);
        let stripped = {
            let prev_tl = self
                .undo_stack
                .back()
                .or_else(|| self.redo_stack.last())
                .map(|e| &e.timeline);
            entry.strip_transient_caches(prev_tl)
        };
        if self.undo_stack.len() >= MAX_UNDO_HISTORY {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(stripped);
        self.redo_stack.clear();
    }

    /// Undo スタック末尾から直前のスナップショットを取り出す
    pub(crate) fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo_stack.pop_back()
    }

    /// Redo スタック末尾から次のスナップショットを取り出す
    pub(crate) fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo_stack.pop()
    }

    /// Undo 実行前の現在状態を Redo スタックへ退避する
    pub(crate) fn push_redo(&mut self, entry: UndoEntry) {
        let stripped = {
            let prev_tl = self
                .redo_stack
                .last()
                .or_else(|| self.undo_stack.back())
                .map(|e| &e.timeline);
            entry.strip_transient_caches(prev_tl)
        };
        self.redo_stack.push(stripped);
    }

    /// Redo 実行前の現在状態を Undo スタックへ戻す（Redo スタックは消去しない）
    pub(crate) fn push_undo_from_redo(&mut self, entry: UndoEntry) {
        let stripped = {
            let prev_tl = self
                .undo_stack
                .back()
                .or_else(|| self.redo_stack.last())
                .map(|e| &e.timeline);
            entry.strip_transient_caches(prev_tl)
        };
        if self.undo_stack.len() >= MAX_UNDO_HISTORY {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(stripped);
    }
}

/// ツール選択・ブラシ/消しゴム設定・右パネルタブ・トースト通知などアプリ全体のUI設定状態
#[derive(Clone)]
pub(crate) struct AppUiState {
    pub(crate) tool: Tool,
    pub(crate) select_sub_mode: SelectSubMode,
    pub(crate) right_panel_tab: RightPanelTab,
    pub(crate) brush_radius: f32,
    pub(crate) eraser_radius: f32,
    pub(crate) brush_color: Color32,
    pub(crate) show_wireframe: bool,
    /// プロパティパネルの下部表示対象（`true`: タイムラインのフレーム/トゥイーン属性、`false`: キャンバス上のシンボル属性）
    pub(crate) inspect_timeline_frame: bool,
    /// シンボル再生オプションの開始/表示フレーム数値フィールドを連続編集（ドラッグ/入力）中かどうか
    pub(crate) editing_symbol_playback: bool,
    /// 現在開いている・保存先の `.is2d` プロジェクトファイルパス（未保存の新規ドキュメント時は `None`）
    pub(crate) current_file_path: Option<PathBuf>,
    /// 操作トリガーで一定時間表示するトースト通知メッセージ
    pub(crate) status_toast: Option<StatusToast>,
}

impl Default for AppUiState {
    fn default() -> Self {
        Self {
            tool: Tool::Brush,
            select_sub_mode: SelectSubMode::Reshape,
            right_panel_tab: RightPanelTab::default(),
            brush_radius: 8.0,
            eraser_radius: 22.0,
            brush_color: Color32::from_rgb(40, 45, 55),
            show_wireframe: false,
            inspect_timeline_frame: false,
            editing_symbol_playback: false,
            current_file_path: None,
            status_toast: None,
        }
    }
}

impl AppUiState {
    /// キャンバス左上に一定時間自動で消えるトースト通知メッセージを登録する
    pub(crate) fn show_notice(&mut self, kind: NoticeKind, message: impl Into<String>) {
        self.status_toast = Some(StatusToast {
            kind,
            message: message.into(),
            expires_at: Instant::now() + DEFAULT_TOAST_DURATION,
        });
    }
}

// ============================================================================
// 5. 単一ドキュメント編集基盤（EditorDoc）
// ============================================================================

/// 1ドキュメント分の編集状態と Undo/Redo・通知・同期トランザクションを担う基盤構造体（L3 編集コア）
pub(crate) struct EditorDoc {
    /// 現在のドキュメントの一意な識別ID（タブ間クリップボード・シンボル移植の同一タブ判定用）
    pub(crate) doc_id: u64,
    /// タイムライン再生中フラグ（L3 操作による自動停止 `stop_playback` と L4 `TimelineUiState` を橋渡し）
    pub(crate) is_playing: bool,
    /// シンボルタイムライン進入/退出やドキュメント切り替え時に L4 `TimelineUiState` のスクロール・ドラッグをリセットする要求フラグ
    pub(crate) timeline_switched: bool,
    /// ツール設定・右パネルタブ・ステータストースト通知などのUI状態
    pub(crate) ui: AppUiState,
    /// キャンバスのズーム・パン・ステージ枠設定
    pub(crate) viewport: ViewportState,
    /// 現在フレームの作業図形・選択・変形・排他ドラッグ・クリップボード状態
    pub(crate) canvas: CanvasState,
    /// タイムライン・レイヤー・キーフレーム・セル構造
    pub(crate) timeline: Timeline,
    /// プロジェクト共通のシンボルライブラリ・シンボルインプレイス編集スタック・ID採番
    pub(crate) symbols: SymbolManager,
    /// Undo / Redo 履歴スタック
    pub(crate) history: UndoHistory,
}

impl Default for EditorDoc {
    fn default() -> Self {
        Self {
            doc_id: 1,
            is_playing: false,
            timeline_switched: false,
            ui: AppUiState::default(),
            viewport: ViewportState::default(),
            canvas: CanvasState::default(),
            timeline: Timeline::default(),
            symbols: SymbolManager::default(),
            history: UndoHistory::default(),
        }
    }
}

impl EditorDoc {
    /// 指定された `doc_id` で空の初期ドキュメントを生成する
    pub(crate) fn new_with_id(doc_id: u64) -> Self {
        Self {
            doc_id,
            ..Self::default()
        }
    }

    pub(crate) fn restore_state_snapshot(&mut self, entry: UndoEntry) -> UndoEntry {
        self.timeline_switched = true;
        self.viewport.editing_stage_size = false;
        self.symbols.actively_renaming_symbol = None;
        self.ui.editing_symbol_playback = false;
        let (prev_lib, prev_stack, prev_rev) = self.history.snapshot_symbols(&self.symbols);
        let prev_entry = UndoEntry {
            timeline: std::mem::replace(&mut self.timeline, entry.timeline),
            library: prev_lib,
            editing_group_path: std::mem::replace(
                &mut self.canvas.editing_group_path,
                entry.editing_group_path,
            ),
            symbol_edit_stack: prev_stack,
            stage_size: std::mem::replace(&mut self.viewport.stage_size, entry.stage_size),
            symbols_revision: prev_rev,
            commit_id: std::mem::replace(&mut self.history.current_commit_id, entry.commit_id),
        };
        self.symbols.library = Arc::unwrap_or_clone(entry.library);
        self.symbols.edit_stack = Arc::unwrap_or_clone(entry.symbol_edit_stack);
        self.symbols.revision = entry.symbols_revision;
        self.symbols.sync_after_restore();
        load_shapes_from_timeline(self);
        prev_entry
    }

    pub(crate) fn record_undo_snapshot(&mut self, mut snapshot: Vec<FillShape>) {
        let mut tl_snap = self.timeline.clone();
        tl_snap.normalize_shape_layer_ids(&mut snapshot);
        tl_snap.write_shapes_to_editable_cels(
            &snapshot,
            &self.symbols.library,
            self.canvas.loaded_signatures(),
        );
        let (library, symbol_edit_stack, symbols_revision) =
            self.history.snapshot_symbols(&self.symbols);
        let entry = UndoEntry {
            timeline: tl_snap,
            library,
            editing_group_path: self.canvas.editing_group_path.clone(),
            symbol_edit_stack,
            stage_size: self.viewport.stage_size,
            symbols_revision,
            commit_id: self.history.current_commit_id,
        };
        self.history.push(entry);
    }

    pub(crate) fn push_undo(&mut self) {
        if self.is_playing {
            stop_playback(self);
        } else if let Some(mut unsliced) = self.canvas.take_unsliced_snapshot() {
            let selected_ids = self.canvas.selected_shape_ids();
            self.timeline.normalize_shape_layer_ids(&mut unsliced);
            self.canvas.shapes = unsliced;
            self.canvas.restore_selection_by_shape_ids(&selected_ids);
            sync_shapes_to_timeline(self);
        } else {
            sync_shapes_to_timeline(self);
        }
        let (library, symbol_edit_stack, symbols_revision) =
            self.history.snapshot_symbols(&self.symbols);
        let entry = UndoEntry {
            timeline: self.timeline.clone(),
            library,
            editing_group_path: self.canvas.editing_group_path.clone(),
            symbol_edit_stack,
            stage_size: self.viewport.stage_size,
            symbols_revision,
            commit_id: self.history.current_commit_id,
        };
        self.history.push(entry);
    }

    /// キャンバス図形に対する変更トランザクションを一括実行する：
    /// 1. 再生中であれば停止して現在フレームの図形を展開 (`stop_playback`)
    /// 2. 必要に応じて未確定選択を確定解除 (`clear_selection`)
    /// 3. 変更前の Undo スナップショットを記録
    /// 4. `mutate` クロージャでキャンバス図形を更新
    /// 5. 変更後の図形を現在フレームのタイムライン `Cel` へ自動同期 (`sync_shapes_to_timeline`)
    pub(crate) fn mutate_canvas_with_undo(
        &mut self,
        clear_selection_first: bool,
        mutate: impl FnOnce(&mut Self),
    ) {
        stop_playback(self);
        if clear_selection_first {
            clear_selection(self);
        }
        let snapshot = self.canvas.take_or_clone_unsliced_snapshot();
        self.record_undo_snapshot(snapshot);
        mutate(self);
        sync_shapes_to_timeline(self);
    }

    /// 事前取得済みの変更前スナップショット `pre_mutation_snapshot` を Undo 履歴に記録し、
    /// 現在の `self.canvas.shapes` をタイムライン `Cel` へ自動同期する単一コミットAPI
    pub(crate) fn commit_canvas_mutation(&mut self, pre_mutation_snapshot: Vec<FillShape>) {
        stop_playback(self);
        self.record_undo_snapshot(pre_mutation_snapshot);
        sync_shapes_to_timeline(self);
    }

    pub(crate) fn undo(&mut self) {
        flush_canvas_before_timeline_op(self);
        if let Some(prev_entry) = self.history.pop_undo() {
            let curr = self.restore_state_snapshot(prev_entry);
            self.history.push_redo(curr);
        }
    }

    pub(crate) fn redo(&mut self) {
        flush_canvas_before_timeline_op(self);
        if let Some(next_entry) = self.history.pop_redo() {
            let curr = self.restore_state_snapshot(next_entry);
            self.history.push_undo_from_redo(curr);
        }
    }

    /// 警告トースト通知を表示する
    #[inline]
    pub(crate) fn notify_warning(&mut self, message: impl Into<String>) {
        self.ui.show_notice(NoticeKind::Warning, message);
    }

    /// エラートースト通知を表示する
    #[inline]
    pub(crate) fn notify_error(&mut self, message: impl Into<String>) {
        self.ui.show_notice(NoticeKind::Error, message);
    }

    /// 情報トースト通知を表示する
    #[inline]
    pub(crate) fn notify_info(&mut self, message: impl Into<String>) {
        self.ui.show_notice(NoticeKind::Info, message);
    }
}

// ============================================================================
// 6. レイヤー解決・ドラッグロールバック・キャンバス⇔タイムライン状態同期
// ============================================================================

/// 現在の編集スコープ（グループ内であればそのグループの所属レイヤー、そうでなければ現在のアクティブレイヤー）の `layer_id` を返す
pub(crate) fn active_scope_layer_id(doc: &EditorDoc) -> u64 {
    if !doc.canvas.editing_group_path.is_empty() {
        if let Some(s) = doc
            .canvas
            .shapes
            .iter()
            .find(|s| s.is_in_editing_scope(&doc.canvas.editing_group_path))
        {
            return s.layer_id;
        }
    }
    doc.timeline.active_layer_id()
}

/// 指定されたシェイプインデックス群が所属するレイヤーのうち最前面の `layer_id` を返す（該当なし時は `active_scope_layer_id`）
pub(crate) fn topmost_layer_id_for_indices(doc: &EditorDoc, indices: &[usize]) -> u64 {
    indices
        .iter()
        .filter_map(|&idx| doc.canvas.shapes.get(idx).map(|s| s.layer_id))
        .max_by_key(|&lid| doc.timeline.layer_index_by_id(lid).unwrap_or(0))
        .unwrap_or_else(|| active_scope_layer_id(doc))
}

/// 指定したシェイプインデックス `shape_idx` が属するレイヤーへ `doc.timeline.active_layer` およびタイムライン選択状態を追従させる
pub(crate) fn focus_active_layer_to_shape(doc: &mut EditorDoc, shape_idx: usize) {
    doc.ui.inspect_timeline_frame = false;
    if let Some(lid) = doc.canvas.shapes.get(shape_idx).map(|s| s.layer_id) {
        if let Some(l_idx) = doc.timeline.layer_index_by_id(lid) {
            doc.timeline.active_layer = l_idx;
            let cf = doc.timeline.current_frame;
            doc.timeline.cell_selection = Some(TimelineCellSelection::single(l_idx, cf));
        }
    }
}

/// キャンバス上で選択されたシェイプ群（`doc.canvas.selected_shapes`）が所属するレイヤーをタイムライン上で選択状態にする
pub(crate) fn sync_timeline_layer_from_canvas_selection(doc: &mut EditorDoc) {
    doc.ui.inspect_timeline_frame = false;
    let mut min_l: Option<usize> = None;
    let mut max_l: Option<usize> = None;
    let mut last_l: Option<usize> = None;
    for &idx in &doc.canvas.selected_shapes {
        if let Some(lid) = doc.canvas.shapes.get(idx).map(|s| s.layer_id) {
            if let Some(l_idx) = doc.timeline.layer_index_by_id(lid) {
                min_l = Some(min_l.map_or(l_idx, |m| m.min(l_idx)));
                max_l = Some(max_l.map_or(l_idx, |m| m.max(l_idx)));
                last_l = Some(l_idx);
            }
        }
    }
    if let (Some(l0), Some(l1), Some(act)) = (min_l, max_l, last_l) {
        doc.timeline.active_layer = act;
        let cf = doc.timeline.current_frame;
        doc.timeline.cell_selection = Some(TimelineCellSelection::from_corners(l0, cf, l1, cf));
    }
}

/// 選択ツールまたは自由変形ツールで進行中だった図形変形・移動ドラッグのみを中断し、
/// プレビュー変形中だった図形をドラッグ開始前の状態へ安全に巻き戻す（ブラシのストローク状態には干渉しない）
pub(crate) fn rollback_shape_drag(doc: &mut EditorDoc) {
    if doc.canvas.rollback_shape_drag() {
        sync_shapes_to_timeline(doc);
    }
}

/// 進行中のすべてのキャンバスドラッグ操作（ブラシ/消しゴム・輪郭変形・移動・自由変形）を中断し、
/// プレビュー変形中だった図形をドラッグ開始前の状態へ安全に巻き戻す
pub(crate) fn cancel_active_drag(doc: &mut EditorDoc) {
    rollback_shape_drag(doc);
    doc.canvas.drag = ActiveCanvasDrag::None;
    doc.canvas.hover_target = HoverTarget::None;
    doc.canvas.transform_hover = TransformHoverTarget::None;
}

pub(crate) fn set_tool(doc: &mut EditorDoc, tool: Tool) {
    cancel_active_drag(doc);
    doc.ui.tool = tool;
    doc.ui.select_sub_mode = SelectSubMode::Reshape;
}

pub(crate) fn toggle_select_sub_mode(doc: &mut EditorDoc, target_mode: SelectSubMode) {
    doc.ui.select_sub_mode = if doc.ui.select_sub_mode == target_mode {
        SelectSubMode::Reshape
    } else {
        target_mode
    };
    clear_selection(doc);
}

/// 再生中であれば停止し、停止時点のフレームの図形群を作業用 `doc.canvas.shapes` に展開する
pub(crate) fn stop_playback(doc: &mut EditorDoc) {
    if doc.is_playing {
        doc.is_playing = false;
        load_shapes_from_timeline(doc);
    }
}

/// タイムラインの構造変更前に、未確定のキャンバス選択スライスを確定解除し `doc.canvas.shapes` を現在フレームのセルへ同期する
/// （再生中の場合は古い `doc.canvas.shapes` で現在フレームを上書きしないよう、再生停止して現在フレームの図形を読み込む）
pub(crate) fn flush_canvas_before_timeline_op(doc: &mut EditorDoc) {
    if doc.is_playing {
        stop_playback(doc);
    } else {
        cancel_active_drag(doc);
        clear_selection(doc);
        sync_shapes_to_timeline(doc);
    }
}

/// 作業キャンバスを確定同期し、Undo履歴を記録した上でタイムライン変更処理 `mutate` を実行し、完了後に `doc.canvas.shapes` を再読み込みする
pub(crate) fn mutate_timeline_with_undo(doc: &mut EditorDoc, mutate: impl FnOnce(&mut Timeline)) {
    flush_canvas_before_timeline_op(doc);
    doc.push_undo();
    mutate(&mut doc.timeline);
    doc.symbols.sync_active_edit(&doc.timeline);
    load_shapes_from_timeline(doc);
}

/// キャンバスの作業用 `doc.canvas.shapes` を、各図形の `layer_id` ごとに現在フレームの `Cel` に振り分けて書き戻す
pub(crate) fn sync_shapes_to_timeline(doc: &mut EditorDoc) {
    doc.canvas.sanitize_editing_scope();
    if doc.is_playing || doc.canvas.is_all_shapes_unchanged() {
        return;
    }
    doc.timeline
        .normalize_shape_layer_ids(&mut doc.canvas.shapes);
    doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
    let any_modified = doc.timeline.write_shapes_to_editable_cels(
        &doc.canvas.shapes,
        &doc.symbols.library,
        doc.canvas.loaded_signatures(),
    );
    doc.canvas.record_loaded_signatures();
    if any_modified {
        doc.symbols.sync_active_edit(&doc.timeline);
    }
}

/// 現在フレームにおける全ての「表示中かつ非ロック（`visible && !locked`）」レイヤーの `Cel` から
/// 背面レイヤー→前面レイヤーの順で（トゥイーン補間およびシンボルの内部タイムラインフレームも評価しつつ）`doc.canvas.shapes` を一括読み込みする
pub(crate) fn load_shapes_from_timeline(doc: &mut EditorDoc) {
    cancel_active_drag(doc);
    let frame = doc.timeline.current_frame;
    let mut loaded = Vec::new();
    for layer_idx in 0..doc.timeline.layers.len() {
        let layer = &doc.timeline.layers[layer_idx];
        if !layer.visible || layer.locked {
            continue;
        }
        loaded.extend(doc.timeline.evaluate_layer_shapes_at(
            layer_idx,
            frame,
            &doc.symbols.library,
        ));
    }
    doc.canvas.replace_loaded_shapes(loaded);
    if let Some(sym_id) = doc.symbols.edit_stack.last().map(|s| s.symbol_id) {
        let cf = doc.timeline.current_frame;
        let al = doc.timeline.active_layer;
        if let Some(sym_def) = doc.symbols.find_symbol_mut(sym_id) {
            if sym_def.timeline.current_frame != cf || sym_def.timeline.active_layer != al {
                sym_def.timeline.current_frame = cf;
                sym_def.timeline.active_layer = al;
            }
        }
    }
}

/// タイムライン上で選択されたセル範囲（単一キーフレームまたは複数レイヤー範囲）に属するすべての図形をキャンバス上で選択状態にする
pub(crate) fn select_shapes_for_timeline_selection(doc: &mut EditorDoc) {
    clear_selection(doc);
    let Some(sel) = doc.timeline.cell_selection else {
        return;
    };
    let selected_lids: HashSet<u64> = (sel.min_layer..=sel.max_layer)
        .filter_map(|l_idx| doc.timeline.layers.get(l_idx))
        .filter(|l| l.visible && !l.locked)
        .map(|l| l.id)
        .collect();
    doc.canvas.select_shapes_in_layers(&selected_lids);
    doc.ui.inspect_timeline_frame = true;
}

/// シンボル基準点（Registration Point `+`）をスクリーン座標 `center` に描画する共通ヘルパー
pub(crate) fn draw_registration_cross(
    painter: &egui::Painter,
    center: Pos2,
    arm: f32,
    with_white_halo: bool,
) {
    if with_white_halo {
        let halo = Stroke::new(3.0_f32, Color32::WHITE);
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
    let fg = Stroke::new(1.4_f32, Color32::from_rgb(0, 125, 220));
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

/// `selection_modified == true` の選択中生シェイプを同一レイヤー・同一スコープのキャンバス図形へ結合・切り抜き（Merge Drawing）確定し、
/// タイムラインへ同期する（選択状態そのもののクリアは呼び出し側で行う）
pub(crate) fn flush_modified_raw_selection_merge(doc: &mut EditorDoc) {
    if !doc.canvas.selection_modified || doc.canvas.selected_shapes.is_empty() {
        doc.canvas.selection_modified = false;
        return;
    }
    let (is_sel, _) = doc.canvas.selected_shape_mask();
    let mut base_canvas = Vec::with_capacity(doc.canvas.shapes.len());
    let mut modified_raw_shapes = Vec::with_capacity(doc.canvas.selected_shapes.len());
    for (idx, shape) in doc.canvas.shapes.drain(..).enumerate() {
        if is_sel[idx] && shape.is_raw_at_scope(&doc.canvas.editing_group_path) {
            modified_raw_shapes.push(shape);
        } else {
            base_canvas.push(shape);
        }
    }
    merge_shapes_into_canvas(&mut base_canvas, modified_raw_shapes);
    doc.canvas.shapes = base_canvas;
    doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
    sync_shapes_to_timeline(doc);
    doc.canvas.selection_modified = false;
}

pub(crate) fn clear_selection(doc: &mut EditorDoc) {
    rollback_shape_drag(doc);
    if let Some(mut unsliced) = doc.canvas.take_unsliced_snapshot() {
        doc.canvas.salvage_caches_into(&mut unsliced);
        let selected_ids = doc.canvas.selected_shape_ids();
        doc.canvas.shapes = unsliced;
        doc.canvas.restore_selection_by_shape_ids(&selected_ids);
    }
    if doc.canvas.selection_modified && !doc.canvas.selected_shapes.is_empty() {
        flush_modified_raw_selection_merge(doc);
    }
    doc.canvas.selection_modified = false;
    doc.canvas.selected_shapes.clear();
    doc.canvas.transform_pivot = None;
    doc.ui.inspect_timeline_frame = false;
}

/// 現在の編集スコープの最前面挿入位置へ `new_shapes` を挿入し、それらを選択状態にしてレイヤー順に整列させる
pub(crate) fn insert_and_select_shapes_in_scope(
    doc: &mut EditorDoc,
    new_shapes: Vec<FillShape>,
    mark_modified: bool,
) {
    let insert_pos = doc.canvas.scope_insert_index();
    let count = new_shapes.len();
    doc.canvas.shapes.splice(insert_pos..insert_pos, new_shapes);
    doc.canvas.selected_shapes = (insert_pos..insert_pos + count).collect();
    doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
    doc.canvas.selection_modified = mark_modified;
    doc.canvas.transform_pivot = None;
    if !matches!(doc.ui.tool, Tool::Select | Tool::Transform) {
        set_tool(doc, Tool::Select);
    }
}

/// 選択中の生シェイプ群を現在の編集スコープ内の最前面へ並べ替え、レイヤー順に整列させる
/// （グループ・シンボルのみが選択されている場合は既存の重ね順とキャッシュを維持する）
pub(crate) fn reorder_selected_to_scope_front(doc: &mut EditorDoc) -> bool {
    let (is_sel, any_valid) = doc.canvas.selected_shape_mask();
    if !any_valid {
        return false;
    }
    let scope = doc.canvas.editing_group_path.clone();
    let any_raw_selected = doc
        .canvas
        .shapes
        .iter()
        .enumerate()
        .any(|(idx, s)| is_sel[idx] && s.is_raw_at_scope(&scope));
    if !any_raw_selected {
        return true;
    }

    let insert_limit = doc.canvas.scope_insert_index();
    let mut unselected = Vec::with_capacity(doc.canvas.shapes.len());
    let mut moved_shapes = Vec::with_capacity(doc.canvas.selected_shapes.len());
    let mut trailing_outside = Vec::new();
    for (idx, shape) in doc.canvas.shapes.drain(..).enumerate() {
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
    doc.canvas.shapes = unselected;
    doc.canvas.selected_shapes = (start_sel..end_sel).collect();
    doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
    true
}

pub(crate) fn commit_moved_selection(doc: &mut EditorDoc, undo_snapshot: Vec<FillShape>) {
    if !reorder_selected_to_scope_front(doc) {
        return;
    }
    let snapshot = doc.canvas.take_unsliced_snapshot().unwrap_or(undo_snapshot);
    doc.canvas.selection_modified = true;
    doc.commit_canvas_mutation(snapshot);
}

/// 選択オブジェクトの平行移動ドラッグ終了時の確定・ロールバック共通処理
pub(crate) fn finish_move_selection_drag(
    doc: &mut EditorDoc,
    total_delta: KVec2,
    undo_snapshot: Vec<FillShape>,
) {
    let min_move = 1.0 / (doc.viewport.zoom as f64);
    if total_delta.hypot() > min_move && !doc.canvas.selected_shapes.is_empty() {
        commit_moved_selection(doc, undo_snapshot);
    } else if (total_delta.hypot() > 0.0 || doc.canvas.shapes.len() != undo_snapshot.len())
        && doc.canvas.restore_drag_snapshot(undo_snapshot)
    {
        sync_shapes_to_timeline(doc);
    }
}
