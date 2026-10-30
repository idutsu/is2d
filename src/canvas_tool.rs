use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui::{self, Color32, Key, Pos2, Stroke, Vec2};
use kurbo::{Affine, ParamCurve, ParamCurveNearest, Point, QuadBez, Rect as KRect, Vec2 as KVec2};

use crate::symbol::{
    resolve_cel_with_library, transform_layers_with_local_cache, SymbolDef, SymbolEditSession,
    SymbolEvalCtx, SymbolInstanceBinding, SymbolLoopMode, SymbolManager, TweenRotate,
};
use crate::timeline::{Cel, LoadedShapeSig, Timeline, TimelineCellSelection};
use crate::vector_boolean::{
    boolean_operation, merge_shapes_into_canvas, slice_shape_with_marquee_rect, BoolOp,
};
use crate::vector_engine::{
    make_marquee_rect_shape, next_shape_id, rects_overlap, union_bounding_boxes, FillShape,
};
use crate::Is2dApp;

/// タイムライングリッドの1フレームあたりのセル幅（px）
pub(crate) const CELL_W: f32 = 16.0;

// ============================================================================
// Material Icons (`fonts/MaterialIcons-Regular.ttf`) コードポイント定義
// ============================================================================

pub(crate) mod icons {
    pub(crate) const ADD: &str = "\u{e145}"; // add (＋)
    pub(crate) const REMOVE: &str = "\u{e15b}"; // remove (－)
    pub(crate) const FLIP_H: &str = "\u{e8d4}"; // swap_horiz (左右反転)
    pub(crate) const FLIP_V: &str = "\u{e8d5}"; // swap_vert (上下反転)
    pub(crate) const ROTATE_LEFT: &str = "\u{e419}"; // rotate_left (-90°回転)
    pub(crate) const ROTATE_RIGHT: &str = "\u{e41a}"; // rotate_right (+90°回転)
    pub(crate) const PIVOT_RESET: &str = "\u{e3b4}"; // center_focus_strong (ピボットリセット)
    pub(crate) const GROUP: &str = "\u{e1bd}"; // widgets (グループ表示)
    pub(crate) const SYMBOL: &str = "\u{e87b}"; // extension (シンボル表示)
    pub(crate) const WIREFRAME: &str = "\u{e922}"; // timeline (アンカー・パス表示)
    pub(crate) const DELETE: &str = "\u{e872}"; // delete (削除)
    pub(crate) const SCENE_MOVIE: &str = "\u{e02c}"; // movie (シーンアイコン)
    pub(crate) const ARROW_BACK: &str = "\u{e5c4}"; // arrow_back (1階層出る)
    pub(crate) const VISIBILITY: &str = "\u{e8f4}"; // visibility (レイヤー表示)
    pub(crate) const LOCK: &str = "\u{e897}"; // lock (レイヤーロック)
    pub(crate) const SKIP_PREV: &str = "\u{e045}"; // skip_previous (先頭フレームへ)
    pub(crate) const STEP_PREV: &str = "\u{e408}"; // navigate_before (前のフレームへ)
    pub(crate) const PLAY: &str = "\u{e037}"; // play_arrow (再生)
    pub(crate) const PAUSE: &str = "\u{e034}"; // pause (一時停止)
    pub(crate) const STEP_NEXT: &str = "\u{e409}"; // navigate_next (次のフレームへ)
    pub(crate) const SKIP_NEXT: &str = "\u{e044}"; // skip_next (最終フレームへ)
    pub(crate) const ONION_SKIN: &str = "\u{e3e0}"; // filter_none (オニオンスキン)
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SelectSubMode {
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

/// タイムライン右側領域（ルーラー・フレームグリッド・スクロールバー）でのマウスドラッグ状態
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum TimelineGridDrag {
    #[default]
    None,
    /// 上部フレーム番号ルーラーのドラッグによる再生ヘッドスクラブ（レイヤー個別選択を解除）
    ScrubRuler,
    /// 下部横スクロールバーのドラッグ操作
    ScrubScrollbarX { grab_offset_x: f32 },
    /// 右端縦スクロールバーのドラッグ操作
    ScrubScrollbarY { grab_offset_y: f32 },
    /// フレームグリッド上でのセル範囲選択ドラッグ（複数レイヤー・複数フレーム横断）
    SelectCells {
        start_layer: usize,
        start_frame: usize,
    },
    /// 選択済みフレーム範囲のドラッグ移動、または `Alt`（`Option`）押下による複製
    MoveFrames {
        orig_sel: TimelineCellSelection,
        grab_layer: usize,
        grab_frame: usize,
        curr_layer: usize,
        curr_frame: usize,
        copy_mode: bool,
        has_moved: bool,
    },
}

/// タイムラインパネルの再生状態・ループ/オニオンスキン表示設定・スクロール位置・ドラッグ操作等のUI状態
#[derive(Clone, Debug)]
pub(crate) struct TimelineUiState {
    pub(crate) is_playing: bool,
    pub(crate) loop_playback: bool,
    pub(crate) onion_skin_enabled: bool,
    pub(crate) onion_prev_frames: usize,
    pub(crate) onion_next_frames: usize,
    pub(crate) last_frame_advance_time: Option<f64>,
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f32,
    pub(crate) last_ensured_frame: usize,
    pub(crate) dragging_layer: Option<usize>,
    pub(crate) grid_drag: TimelineGridDrag,
    /// 総フレーム数 `DragValue` の連続ドラッグ・入力中にUndo履歴が1フレームごとに積まれるのを防ぐフラグ
    pub(crate) editing_total_frames: bool,
    /// FPS `DragValue` の連続ドラッグ・入力中にUndo履歴が1フレームごとに積まれるのを防ぐフラグ
    pub(crate) editing_fps: bool,
}

impl Default for TimelineUiState {
    fn default() -> Self {
        Self {
            is_playing: false,
            loop_playback: true,
            onion_skin_enabled: false,
            onion_prev_frames: 2,
            onion_next_frames: 2,
            last_frame_advance_time: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            last_ensured_frame: 0,
            dragging_layer: None,
            grid_drag: TimelineGridDrag::None,
            editing_total_frames: false,
            editing_fps: false,
        }
    }
}

impl TimelineUiState {
    pub(crate) fn cancel_drag(&mut self) {
        self.dragging_layer = None;
        self.grid_drag = TimelineGridDrag::None;
        self.editing_total_frames = false;
        self.editing_fps = false;
    }

    pub(crate) fn reset_for_timeline_switch(&mut self, current_frame: usize) {
        self.is_playing = false;
        self.last_frame_advance_time = None;
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.last_ensured_frame = current_frame;
        self.cancel_drag();
    }

    /// 再生ヘッド（`current_frame`）が移動して表示領域外に出た場合のみ、横スクロール位置 `scroll_x` を自動追従させる
    /// （手動のマウスホイールスクロールで再生ヘッドが画面外に出た場合は引き戻さない）
    pub(crate) fn ensure_current_frame_visible(&mut self, current_frame: usize, grid_width: f32) {
        if current_frame == self.last_ensured_frame {
            return;
        }
        self.last_ensured_frame = current_frame;
        if grid_width <= CELL_W * 2.0 {
            return;
        }
        let head_left = current_frame as f32 * CELL_W;
        let head_right = head_left + CELL_W;
        if head_left < self.scroll_x {
            self.scroll_x = head_left;
        } else if head_right > self.scroll_x + grid_width - CELL_W {
            self.scroll_x = (head_right - grid_width + CELL_W).max(0.0);
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
    app.timeline.active_layer_id()
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
        app.timeline.cell_selection = Some(crate::timeline::TimelineCellSelection::from_corners(
            l0, cf, l1, cf,
        ));
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
    clear_selection(app);
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
        cancel_active_drag(app);
        clear_selection(app);
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
            }
        }
    }
}

/// タイムライン上で選択されたセル範囲（単一キーフレームまたは複数レイヤー範囲）に属するすべての図形をキャンバス上で選択状態にする
pub(crate) fn select_shapes_for_timeline_selection(app: &mut Is2dApp) {
    clear_selection(app);
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

// ------------------------------------------------------------------------
// 4. 選択管理・マーキー選択・移動コミット・グループ階層進入・ヒットテスト共通コア
// ------------------------------------------------------------------------

/// `selection_modified == true` の選択中生シェイプを同一レイヤー・同一スコープのキャンバス図形へ結合・切り抜き（Merge Drawing）確定し、
/// タイムラインへ同期する（選択状態そのもののクリアは呼び出し側で行う）
pub(crate) fn flush_modified_raw_selection_merge(app: &mut Is2dApp) {
    if !app.canvas.selection_modified || app.canvas.selected_shapes.is_empty() {
        app.canvas.selection_modified = false;
        return;
    }
    let (is_sel, _) = app.canvas.selected_shape_mask();
    let mut base_canvas = Vec::with_capacity(app.canvas.shapes.len());
    let mut modified_raw_shapes = Vec::with_capacity(app.canvas.selected_shapes.len());
    for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
        if is_sel[idx] && shape.is_raw_at_scope(&app.canvas.editing_group_path) {
            modified_raw_shapes.push(shape);
        } else {
            base_canvas.push(shape);
        }
    }
    merge_shapes_into_canvas(&mut base_canvas, modified_raw_shapes);
    app.canvas.shapes = base_canvas;
    app.canvas.sort_shapes_by_layer_order(&app.timeline);
    sync_shapes_to_timeline(app);
    app.canvas.selection_modified = false;
}

pub(crate) fn clear_selection(app: &mut Is2dApp) {
    rollback_shape_drag(app);
    if let Some(mut unsliced) = app.canvas.take_unsliced_snapshot() {
        app.canvas.salvage_caches_into(&mut unsliced);
        let selected_ids = app.canvas.selected_shape_ids();
        app.canvas.shapes = unsliced;
        app.canvas.restore_selection_by_shape_ids(&selected_ids);
    }
    if app.canvas.selection_modified && !app.canvas.selected_shapes.is_empty() {
        flush_modified_raw_selection_merge(app);
    }
    app.canvas.selection_modified = false;
    app.canvas.selected_shapes.clear();
    app.canvas.transform_pivot = None;
    app.ui.inspect_timeline_frame = false;
}

/// 現在の選択図形のうち `keep_indices` のみを選択に残し、選択から外れた変更済み生シェイプのみをキャンバスへ結合・切り抜き確定する
/// （選択に残った図形は結合せず `selection_modified = true` のまま維持する）
pub(crate) fn retain_selection_and_flush_deselected(app: &mut Is2dApp, keep_indices: &[usize]) {
    let (is_sel, any_valid) = app.canvas.selected_shape_mask();
    if !any_valid {
        clear_selection(app);
        return;
    }
    let mut is_keep = vec![false; app.canvas.shapes.len()];
    let mut any_keep = false;
    for &idx in keep_indices {
        if idx < is_keep.len() && is_sel[idx] {
            is_keep[idx] = true;
            any_keep = true;
        }
    }
    if !any_keep {
        clear_selection(app);
        return;
    }

    if app.canvas.selection_modified {
        let keep_ids: Vec<u64> = app
            .canvas
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| is_keep[idx].then_some(s.shape_id))
            .collect();
        let insert_limit = app.canvas.scope_insert_index();
        let mut base_canvas = Vec::with_capacity(app.canvas.shapes.len());
        let mut deselected_raw_shapes = Vec::new();
        let mut kept_shapes = Vec::with_capacity(keep_ids.len());
        let mut trailing_outside = Vec::new();
        for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
            if is_keep[idx] {
                kept_shapes.push(shape);
            } else if is_sel[idx] && shape.is_raw_at_scope(&app.canvas.editing_group_path) {
                deselected_raw_shapes.push(shape);
            } else if idx < insert_limit {
                base_canvas.push(shape);
            } else {
                trailing_outside.push(shape);
            }
        }
        let merged_any = !deselected_raw_shapes.is_empty();
        if merged_any {
            merge_shapes_into_canvas(&mut base_canvas, deselected_raw_shapes);
        }
        base_canvas.extend(kept_shapes);
        base_canvas.extend(trailing_outside);
        app.canvas.shapes = base_canvas;
        app.canvas.sort_shapes_by_layer_order(&app.timeline);
        app.canvas.restore_selection_by_shape_ids(&keep_ids);
        if merged_any {
            sync_shapes_to_timeline(app);
        }
        app.canvas.selection_modified = !app.canvas.selected_shapes.is_empty();
    } else {
        app.canvas.selected_shapes = (0..is_keep.len()).filter(|&i| is_keep[i]).collect();
        let any_raw_kept = app.canvas.selected_shapes.iter().any(|&i| {
            app.canvas
                .shapes
                .get(i)
                .is_some_and(|s| s.is_raw_at_scope(&app.canvas.editing_group_path))
        });
        if !any_raw_kept {
            if let Some(mut unsliced) = app.canvas.take_unsliced_snapshot() {
                app.canvas.salvage_caches_into(&mut unsliced);
                let keep_ids = app.canvas.selected_shape_ids();
                app.canvas.shapes = unsliced;
                app.canvas.restore_selection_by_shape_ids(&keep_ids);
            }
        }
    }
    app.canvas.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(app);
}

/// ダブルクリック位置 `pt` にシンボルがあればそのシンボルの独自タイムラインへ入り、グループがあればインプレイス編集モードへ移行する
pub(crate) fn enter_group_at(app: &mut Is2dApp, pt: Point) -> bool {
    let Some(hit_idx) = app.canvas.topmost_shape_at(pt) else {
        return false;
    };
    let Some(gid) =
        app.canvas.shapes[hit_idx].active_group_id_at_scope(&app.canvas.editing_group_path)
    else {
        return false;
    };
    if let Some(binding) = app.canvas.shapes[hit_idx]
        .symbol_instance
        .filter(|b| b.instance_gid == gid)
    {
        return enter_symbol_timeline(app, binding);
    }
    clear_selection(app);
    app.canvas.editing_group_path.push(gid);
    true
}

/// 現在入っているグループまたはシンボルタイムライン編集階層から1段階上の親階層へ戻る
pub(crate) fn exit_current_group(app: &mut Is2dApp) -> bool {
    stop_playback(app);
    if !app.canvas.editing_group_path.is_empty() {
        clear_selection(app);
        let Some(exited_gid) = app.canvas.editing_group_path.pop() else {
            return false;
        };
        app.canvas.selected_shapes = (0..app.canvas.shapes.len())
            .filter(|&i| {
                app.canvas.shapes[i].active_group_id_at_scope(&app.canvas.editing_group_path)
                    == Some(exited_gid)
            })
            .collect();
        app.canvas.transform_pivot = None;
        return true;
    }
    exit_symbol_timeline(app)
}

/// キャンバス上のダブルクリックによるグループ/シンボルへのインプレイス進入、または空白背景ダブルクリックによる親階層への復帰を処理する
pub(crate) fn handle_canvas_double_click(app: &mut Is2dApp, pt: Point) {
    if !enter_group_at(app, pt) && app.canvas.topmost_shape_at(pt).is_none() {
        exit_current_group(app);
    }
}

/// Shift クリック等で指定シェイプ（またはグループ塊全体）の選択状態をトグルし、変形ピボットをリセットする
pub(crate) fn toggle_shape_selection(app: &mut Is2dApp, shape_idx: usize) {
    let unit = app.canvas.expand_to_group_unit(shape_idx);
    let (sel_mask, _) = app.canvas.selected_shape_mask();
    if sel_mask.get(shape_idx).copied().unwrap_or(false) {
        let remaining: Vec<usize> = app
            .canvas
            .selected_shapes
            .iter()
            .copied()
            .filter(|i| unit.binary_search(i).is_err())
            .collect();
        retain_selection_and_flush_deselected(app, &remaining);
    } else {
        for idx in unit {
            if !sel_mask.get(idx).copied().unwrap_or(false) {
                app.canvas.selected_shapes.push(idx);
            }
        }
        app.canvas.transform_pivot = None;
        sync_timeline_layer_from_canvas_selection(app);
    }
}

/// 既存の選択を確定解除した上で、指定座標 `pt` にある最前面シェイプ（塗り・境界線・頂点、またはグループ塊全体）を選択する
pub(crate) fn select_single_shape_at(app: &mut Is2dApp, pt: Point) -> bool {
    clear_selection(app);
    let zoom = app.viewport.zoom;
    let sub_mode = app.ui.select_sub_mode;
    if let Some(hit_idx) = app
        .canvas
        .topmost_shape_at(pt)
        .or_else(|| compute_hover_target(&app.canvas, pt, zoom, sub_mode).shape_idx())
    {
        app.canvas.selected_shapes = app.canvas.expand_to_group_unit(hit_idx);
        focus_active_layer_to_shape(app, hit_idx);
        sync_timeline_layer_from_canvas_selection(app);
        true
    } else {
        false
    }
}

/// Adobe Animate 風の矩形選択（Marquee Selection）
pub(crate) fn apply_marquee_selection(app: &mut Is2dApp, p0: Point, p1: Point, additive: bool) {
    let min_x = p0.x.min(p1.x);
    let max_x = p0.x.max(p1.x);
    let min_y = p0.y.min(p1.y);
    let max_y = p0.y.max(p1.y);

    let min_marquee = 2.0 / (app.viewport.zoom as f64);
    if (max_x - min_x) < min_marquee || (max_y - min_y) < min_marquee {
        if !additive {
            clear_selection(app);
        }
        return;
    }

    if !additive {
        clear_selection(app);
    } else if app.canvas.selection_modified && !app.canvas.selected_shapes.is_empty() {
        let selected_ids = app.canvas.selected_shape_ids();
        flush_modified_raw_selection_merge(app);
        app.canvas.restore_selection_by_shape_ids(&selected_ids);
    }

    let rect = KRect::new(min_x, min_y, max_x, max_y);
    let clip_rect_shape = make_marquee_rect_shape(rect);
    let (was_selected, _) = if additive {
        app.canvas.selected_shape_mask()
    } else {
        (vec![false; app.canvas.shapes.len()], false)
    };
    let hit_group_ids = collect_marquee_hit_groups(&app.canvas, rect, &clip_rect_shape);
    let pre_slice_snapshot =
        (!app.canvas.has_unsliced_snapshot()).then(|| app.canvas.shapes.clone());

    let mut any_sliced = false;
    let mut new_shapes = Vec::with_capacity(app.canvas.shapes.len() + 4);
    let mut new_selected = Vec::new();

    for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
        if was_selected[idx] {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
            continue;
        }
        if !shape.is_in_editing_scope(&app.canvas.editing_group_path) {
            new_shapes.push(shape);
            continue;
        }
        if let Some(gid) = shape.active_group_id_at_scope(&app.canvas.editing_group_path) {
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
        if bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1 {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
            continue;
        }

        let lid = shape.layer_id;
        let gp = shape.group_path.clone();
        let (inside, outside) = slice_shape_with_marquee_rect(&shape, &clip_rect_shape);
        if inside.is_empty() {
            new_shapes.push(shape);
        } else if outside.is_empty() {
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

    if any_sliced && !app.canvas.has_unsliced_snapshot() {
        if let Some(snap) = pre_slice_snapshot {
            app.canvas.restore_unsliced_snapshot(snap);
        }
    }
    app.canvas.shapes = new_shapes;
    app.canvas.selected_shapes = new_selected;
    if app.canvas.selected_shapes.is_empty() && !app.canvas.selection_modified {
        if let Some(unsliced) = app.canvas.take_unsliced_snapshot() {
            app.canvas.shapes = unsliced;
        }
    }
    app.canvas.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(app);
}

/// 相対グループパスを持つシェイプ群に対し、新しいグループID・シンボルインスタンスIDを採番して現在の編集スコープ用に変換する
pub(crate) fn instantiate_relative_shapes_in_scope(
    app: &mut Is2dApp,
    rel_shapes: &[FillShape],
    offset: KVec2,
    preserve_original_layer: bool,
) -> (Vec<FillShape>, bool) {
    let active_lid = active_scope_layer_id(app);
    let mut gid_map: Vec<(u64, u64)> = Vec::new();
    let mut seen_instance_gids: HashSet<u64> = HashSet::new();
    let mut instantiated = Vec::with_capacity(rel_shapes.len());
    let mut any_raw = false;

    for item in rel_shapes {
        let target_lid =
            if preserve_original_layer && app.timeline.is_layer_editable_by_id(item.layer_id) {
                item.layer_id
            } else {
                active_lid
            };

        if let Some(orig_binding) = item
            .symbol_instance
            .filter(|b| item.group_path.contains(&b.instance_gid))
        {
            if !seen_instance_gids.insert(orig_binding.instance_gid) {
                continue;
            }
            let mut new_parent_scope = app.canvas.editing_group_path.clone();
            for &old_gid in orig_binding.parent_scope_from_path(&item.group_path) {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = app.symbols.alloc_group_id();
                    gid_map.push((old_gid, alloc));
                    alloc
                };
                new_parent_scope.push(new_gid);
            }
            let mapped_inst_gid = if let Some(&(_, mapped)) = gid_map
                .iter()
                .find(|&&(orig, _)| orig == orig_binding.instance_gid)
            {
                mapped
            } else {
                let alloc = app.symbols.alloc_group_id();
                gid_map.push((orig_binding.instance_gid, alloc));
                alloc
            };

            let mut new_binding = orig_binding;
            new_binding.instance_gid = mapped_inst_gid;
            if offset != KVec2::ZERO {
                new_binding.xform = Affine::translate(offset) * new_binding.xform;
            }

            if let Some(sym) = app.symbols.find_symbol(new_binding.symbol_id) {
                let current_frame = app.timeline.current_frame;
                let kf_start_frame = app
                    .timeline
                    .layer_index_by_id(target_lid)
                    .and_then(|l_idx| {
                        let layer = &app.timeline.layers[l_idx];
                        layer
                            .keyframe_index_at(current_frame)
                            .map(|kf_idx| layer.keyframes[kf_idx].start_frame)
                    })
                    .unwrap_or(current_frame);
                let local_shapes = sym.evaluated_local_shapes_for_instance(
                    new_binding,
                    kf_start_frame,
                    current_frame,
                    SymbolEvalCtx::new(&app.symbols.library),
                );
                if local_shapes.is_empty() {
                    instantiated.push(FillShape::new_empty_symbol_marker(
                        new_binding,
                        target_lid,
                        &new_parent_scope,
                    ));
                } else {
                    instantiated.extend(new_binding.instantiate_shapes(
                        &local_shapes,
                        target_lid,
                        &new_parent_scope,
                    ));
                }
                continue;
            }
        }

        let mut new_path = app.canvas.editing_group_path.clone();
        if item.group_path.is_empty() {
            any_raw = true;
        } else {
            for &old_gid in &item.group_path {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = app.symbols.alloc_group_id();
                    gid_map.push((old_gid, alloc));
                    alloc
                };
                new_path.push(new_gid);
            }
        }
        let mut cloned = item
            .clone()
            .with_layer_id(target_lid)
            .with_group_path(new_path);
        cloned.shape_id = next_shape_id();
        if let Some(mut binding) = cloned.symbol_instance {
            if let Some(&(_, mapped_inst_gid)) = gid_map
                .iter()
                .find(|&&(orig, _)| orig == binding.instance_gid)
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
        instantiated.push(cloned);
    }

    (instantiated, any_raw)
}

/// 現在選択中のオブジェクト（生シェイプ・グループ・シンボル）をその場に複製し、
/// 複製側を選択状態に切り替えた上でドラッグ移動用の `undo_snapshot` を返す（`Alt + ドラッグ` 複製用）
pub(crate) fn duplicate_selected_shapes_for_alt_drag(app: &mut Is2dApp) -> Vec<FillShape> {
    let rel_shapes = app.canvas.extract_selected_relative_to_scope();
    if rel_shapes.is_empty() {
        return app.canvas.snapshot_for_drag();
    }
    let saved_pivot = app.canvas.transform_pivot;
    clear_selection(app);
    let undo_snapshot = app.canvas.snapshot_for_drag();
    let (duplicated, _) = instantiate_relative_shapes_in_scope(app, &rel_shapes, KVec2::ZERO, true);
    insert_and_select_shapes_in_scope(app, duplicated, false);
    app.canvas.transform_pivot = saved_pivot;
    sync_timeline_layer_from_canvas_selection(app);
    undo_snapshot
}

/// 現在の編集スコープの最前面挿入位置へ `new_shapes` を挿入し、それらを選択状態にしてレイヤー順に整列させる
pub(crate) fn insert_and_select_shapes_in_scope(
    app: &mut Is2dApp,
    new_shapes: Vec<FillShape>,
    mark_modified: bool,
) {
    let insert_pos = app.canvas.scope_insert_index();
    let count = new_shapes.len();
    app.canvas.shapes.splice(insert_pos..insert_pos, new_shapes);
    app.canvas.selected_shapes = (insert_pos..insert_pos + count).collect();
    app.canvas.sort_shapes_by_layer_order(&app.timeline);
    app.canvas.selection_modified = mark_modified;
    app.canvas.transform_pivot = None;
    if !matches!(app.ui.tool, Tool::Select | Tool::Transform) {
        set_tool(app, Tool::Select);
    }
}

/// 選択中の生シェイプ群を現在の編集スコープ内の最前面へ並べ替え、レイヤー順に整列させる
/// （グループ・シンボルのみが選択されている場合は既存の重ね順とキャッシュを維持する）
pub(crate) fn reorder_selected_to_scope_front(app: &mut Is2dApp) -> bool {
    let (is_sel, any_valid) = app.canvas.selected_shape_mask();
    if !any_valid {
        return false;
    }
    let scope = app.canvas.editing_group_path.clone();
    let any_raw_selected = app
        .canvas
        .shapes
        .iter()
        .enumerate()
        .any(|(idx, s)| is_sel[idx] && s.is_raw_at_scope(&scope));
    if !any_raw_selected {
        return true;
    }

    let insert_limit = app.canvas.scope_insert_index();
    let mut unselected = Vec::with_capacity(app.canvas.shapes.len());
    let mut moved_shapes = Vec::with_capacity(app.canvas.selected_shapes.len());
    let mut trailing_outside = Vec::new();
    for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
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
    app.canvas.shapes = unselected;
    app.canvas.selected_shapes = (start_sel..end_sel).collect();
    app.canvas.sort_shapes_by_layer_order(&app.timeline);
    true
}

pub(crate) fn commit_moved_selection(app: &mut Is2dApp, undo_snapshot: Vec<FillShape>) {
    if !reorder_selected_to_scope_front(app) {
        return;
    }
    let snapshot = app.canvas.take_unsliced_snapshot().unwrap_or(undo_snapshot);
    app.canvas.selection_modified = true;
    app.commit_canvas_mutation(snapshot);
}

/// 選択オブジェクトの平行移動ドラッグ終了時の確定・ロールバック共通処理
pub(crate) fn finish_move_selection_drag(
    app: &mut Is2dApp,
    total_delta: KVec2,
    undo_snapshot: Vec<FillShape>,
) {
    let min_move = 1.0 / (app.viewport.zoom as f64);
    if total_delta.hypot() > min_move && !app.canvas.selected_shapes.is_empty() {
        commit_moved_selection(app, undo_snapshot);
    } else if (total_delta.hypot() > 0.0 || app.canvas.shapes.len() != undo_snapshot.len())
        && app.canvas.restore_drag_snapshot(undo_snapshot)
    {
        sync_shapes_to_timeline(app);
    }
}

/// 選択オブジェクトの移動ドラッグ（`Alt` 複製移動を含む）の初期状態を構築する共通ヘルパー
pub(crate) fn begin_move_selection_drag(
    app: &mut Is2dApp,
    pt: Point,
    alt_held: bool,
) -> MoveSelectionDrag {
    if alt_held {
        let orig_selected_ids = app.canvas.selected_shape_ids();
        let undo_snapshot = duplicate_selected_shapes_for_alt_drag(app);
        MoveSelectionDrag::new(pt, undo_snapshot).with_orig_selected_ids(orig_selected_ids)
    } else {
        let undo_snapshot = app.canvas.snapshot_for_drag();
        MoveSelectionDrag::new(pt, undo_snapshot)
    }
}

/// 2次ベジェ曲線の3制御点 `(p0, p1, p2)` の凸包AABB（余白 `margin` 付き）に点 `pt` が含まれるかを高速判定する
#[inline]
pub(crate) fn quad_control_bbox_contains(seg: &QuadBez, pt: Point, margin: f64) -> bool {
    let min_x = seg.p0.x.min(seg.p1.x).min(seg.p2.x) - margin;
    let max_x = seg.p0.x.max(seg.p1.x).max(seg.p2.x) + margin;
    let min_y = seg.p0.y.min(seg.p1.y).min(seg.p2.y) - margin;
    let max_y = seg.p0.y.max(seg.p1.y).max(seg.p2.y) + margin;
    pt.x >= min_x && pt.x <= max_x && pt.y >= min_y && pt.y <= max_y
}

/// 指定座標 `pt` がシェイプの輪郭（外周・穴）から距離 `radius` 以内にあるかを判定する
fn point_within_shape_contours(shape: &FillShape, pt: Point, radius: f64, inv_z: f64) -> bool {
    let r_sq = radius * radius;
    for contour in shape.contours() {
        for seg in contour.segments.iter() {
            if !quad_control_bbox_contains(seg, pt, radius) {
                continue;
            }
            if seg.nearest(pt, 0.05 * inv_z).distance_sq <= r_sq {
                return true;
            }
        }
    }
    false
}

/// 矩形 `rect` と交差する現在の編集スコープ直下の子グループID一覧を収集する
pub(crate) fn collect_marquee_hit_groups(
    canvas: &CanvasState,
    rect: KRect,
    clip_rect_shape: &FillShape,
) -> HashSet<u64> {
    let mut hit_group_ids: HashSet<u64> = HashSet::new();
    for shape in &canvas.shapes {
        let Some(gid) = shape.active_group_id_at_scope(&canvas.editing_group_path) else {
            continue;
        };
        if hit_group_ids.contains(&gid) {
            continue;
        }
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !rects_overlap(bbox, rect) {
            continue;
        }
        let in_rect =
            bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1;
        if in_rect
            || !boolean_operation(
                std::slice::from_ref(shape),
                std::slice::from_ref(clip_rect_shape),
                BoolOp::Intersection,
                shape.color,
            )
            .is_empty()
        {
            hit_group_ids.insert(gid);
        }
    }
    hit_group_ids
}

/// アルゴリズム 4: マウス位置に対する頂点・輪郭エッジ・塗りのヒットテスト
pub(crate) fn compute_hover_target(
    canvas: &CanvasState,
    mouse_pt: Point,
    zoom: f32,
    sub_mode: SelectSubMode,
) -> HoverTarget {
    let inv_z = 1.0 / (zoom as f64);
    let vertex_hit_radius = 9.0 * inv_z;
    let edge_hit_outer_radius = 9.5 * inv_z;
    let edge_hit_inner_radius = 5.5 * inv_z;
    let anchor_edit_hit_radius = 12.0 * inv_z;
    let selected_grab_radius = 6.0 * inv_z;

    let (is_sel, has_sel) = canvas.selected_shape_mask();

    if sub_mode == SelectSubMode::Reshape && has_sel {
        for &s_idx in canvas.selected_shapes.iter().rev() {
            let Some(shape) = canvas.shapes.get(s_idx) else {
                continue;
            };
            if !shape.is_in_editing_scope(&canvas.editing_group_path)
                || !shape
                    .bounding_box()
                    .inflate(selected_grab_radius, selected_grab_radius)
                    .contains(mouse_pt)
            {
                continue;
            }
            if shape.contains_point(mouse_pt)
                || point_within_shape_contours(shape, mouse_pt, selected_grab_radius, inv_z)
            {
                return HoverTarget::Fill { shape_idx: s_idx };
            }
        }
    }

    let topmost_fill_idx = canvas.topmost_shape_at(mouse_pt);

    if sub_mode != SelectSubMode::AddAnchor {
        let v_radius = if sub_mode == SelectSubMode::DeleteAnchor {
            anchor_edit_hit_radius
        } else {
            vertex_hit_radius
        };
        let mut best_vertex: Option<(f64, HoverTarget)> = None;
        for (s_idx, shape) in canvas.shapes.iter().enumerate().rev() {
            if topmost_fill_idx.is_some_and(|top| s_idx < top) {
                continue;
            }
            let Some(bbox) = shape.geometry_bounding_box() else {
                continue;
            };
            if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
                || !shape.is_raw_at_scope(&canvas.editing_group_path)
                || !bbox.inflate(v_radius, v_radius).contains(mouse_pt)
            {
                continue;
            }
            for (c_idx, contour) in shape.contours().enumerate() {
                if sub_mode == SelectSubMode::DeleteAnchor && contour.segments.len() <= 2 {
                    continue;
                }
                for (seg_idx, seg) in contour.segments.iter().enumerate() {
                    let dist = (mouse_pt - seg.p0).hypot();
                    if dist <= v_radius && best_vertex.as_ref().is_none_or(|(d, _)| dist < *d) {
                        let occluded_by_front = (s_idx + 1..canvas.shapes.len()).any(|front_idx| {
                            canvas.shapes[front_idx].is_in_editing_scope(&canvas.editing_group_path)
                                && canvas.shapes[front_idx].contains_point(seg.p0)
                        });
                        if occluded_by_front {
                            continue;
                        }
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
        if let Some((_, target)) = best_vertex {
            return target;
        }
        if sub_mode == SelectSubMode::DeleteAnchor {
            return HoverTarget::None;
        }
    }

    let mut best_edge: Option<(f64, HoverTarget)> = None;
    for (s_idx, shape) in canvas.shapes.iter().enumerate().rev() {
        if topmost_fill_idx.is_some_and(|top| s_idx < top) {
            continue;
        }
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
            || !shape.is_raw_at_scope(&canvas.editing_group_path)
            || !bbox
                .inflate(anchor_edit_hit_radius, anchor_edit_hit_radius)
                .contains(mouse_pt)
        {
            continue;
        }
        let max_dist = if sub_mode == SelectSubMode::AddAnchor {
            anchor_edit_hit_radius
        } else if shape.contains_point(mouse_pt) {
            edge_hit_inner_radius
        } else {
            edge_hit_outer_radius
        };

        for (c_idx, contour) in shape.contours().enumerate() {
            for (seg_idx, seg) in contour.segments.iter().enumerate() {
                if !quad_control_bbox_contains(seg, mouse_pt, max_dist) {
                    continue;
                }
                let nearest = seg.nearest(mouse_pt, 0.05 * inv_z);
                let t_clamped = nearest.t.clamp(0.08, 0.92);
                let eval_pt = if sub_mode == SelectSubMode::AddAnchor {
                    seg.eval(t_clamped)
                } else {
                    seg.eval(nearest.t)
                };
                let dist = nearest.distance_sq.sqrt();
                if dist <= max_dist && best_edge.as_ref().is_none_or(|(d, _)| dist < *d) {
                    let occluded_by_front = (s_idx + 1..canvas.shapes.len()).any(|front_idx| {
                        canvas.shapes[front_idx].is_in_editing_scope(&canvas.editing_group_path)
                            && canvas.shapes[front_idx].contains_point(eval_pt)
                    });
                    if occluded_by_front {
                        continue;
                    }
                    best_edge = Some((
                        dist,
                        HoverTarget::Edge {
                            shape_idx: s_idx,
                            contour_idx: c_idx,
                            seg_idx,
                            t: if sub_mode == SelectSubMode::AddAnchor {
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
    if let Some((_, target)) = best_edge {
        return target;
    }
    if sub_mode == SelectSubMode::AddAnchor {
        return HoverTarget::None;
    }

    if let Some(s_idx) = topmost_fill_idx {
        return HoverTarget::Fill { shape_idx: s_idx };
    }

    HoverTarget::None
}

// ------------------------------------------------------------------------
// 5. シンボル変換・ライブラリ配置/削除・シンボルタイムライン階層遷移・トゥイーン制御
// ------------------------------------------------------------------------

/// 選択中のオブジェクト（生シェイプ・グループ問わず）を新しい「シンボル」に変換してライブラリに登録する（`F8`）
pub(crate) fn convert_selection_to_symbol(app: &mut Is2dApp) -> Option<u64> {
    stop_playback(app);
    cancel_active_drag(app);
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

    let target_lid = topmost_layer_id_for_indices(app, &valid_indices);
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
    let snapshot = app.canvas.take_unsliced_snapshot().unwrap_or(snapshot);
    app.record_undo_snapshot(snapshot);
    app.symbols.register_new_symbol(local_shapes);
    reorder_selected_to_scope_front(app);
    sync_shapes_to_timeline(app);
    app.canvas.selection_modified = false;
    app.canvas.transform_pivot = None;
    app.ui.inspect_timeline_frame = false;
    Some(sym_id)
}

/// ライブラリ内の指定シンボル `symbol_id` の新しいインスタンスを現在のステージ中央へ配置する
pub(crate) fn place_symbol_instance_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool {
    stop_playback(app);
    let target_lid = active_scope_layer_id(app);
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
        let target_lid = active_scope_layer_id(app);
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
        insert_and_select_shapes_in_scope(app, placed, false);
    });
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の名前を変更し、Undo履歴に記録する
pub(crate) fn rename_symbol_in_library(
    app: &mut Is2dApp,
    symbol_id: u64,
    new_name: impl Into<String>,
) -> bool {
    stop_playback(app);
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
    stop_playback(app);
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
    clear_selection(app);
    app.push_undo();

    app.symbols
        .remove_symbol_and_detach(symbol_id, &mut app.timeline, &mut app.canvas.shapes);

    app.symbols.sync_active_edit(&app.timeline);
    load_shapes_from_timeline(app);
    true
}

/// 現在のキーフレーム（または保持フレームが属する開始キーフレーム）に対してクラシックトゥイーンを ON / OFF トグルする（`T` キー）。
pub(crate) fn timeline_toggle_tween(app: &mut Is2dApp) -> bool {
    let targets = app.timeline.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    let prev_selected_ids = app.canvas.selected_shape_ids();
    flush_canvas_before_timeline_op(app);

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
        load_shapes_from_timeline(app);
        select_shapes_for_timeline_selection(app);
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

    mutate_timeline_with_undo(app, |tl| {
        if let Some(layer_mut) = tl.layers.get_mut(tl.active_layer) {
            if let Some(kf) = layer_mut.keyframes.get_mut(kf_idx) {
                kf.tween_rotate = rotate_mode;
            }
        }
    });
    select_shapes_for_timeline_selection(app);
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

/// 現在選択されている単一シンボルインスタンスの再生モードおよび開始/指定フレーム番号（0始まり）を更新する
pub(crate) fn set_selected_symbol_playback(
    app: &mut Is2dApp,
    loop_mode: SymbolLoopMode,
    first_frame: usize,
) -> bool {
    stop_playback(app);
    let Some((binding, _, total_frames)) = selected_single_symbol_info(app) else {
        return false;
    };
    let clamped_first = first_frame.min(total_frames.saturating_sub(1));
    if binding.loop_mode == loop_mode && binding.first_frame == clamped_first {
        return false;
    }

    let inst_gid = binding.instance_gid;
    let sym_id = binding.symbol_id;
    clear_selection(app);
    sync_shapes_to_timeline(app);
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
        load_shapes_from_timeline(app);
        app.canvas.select_symbol_instance_by_gid(inst_gid);
    }
    updated
}

/// シンボルインスタンスをダブルクリックして、そのシンボル専用の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_timeline(app: &mut Is2dApp, binding: SymbolInstanceBinding) -> bool {
    let Some(sym_def) = app.symbols.find_symbol(binding.symbol_id).cloned() else {
        return false;
    };

    flush_canvas_before_timeline_op(app);

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

    load_shapes_from_timeline(app);
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_from_library(app: &mut Is2dApp, symbol_id: u64) -> bool {
    stop_playback(app);
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
    flush_canvas_before_timeline_op(app);

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

    app.symbols.sync_active_edit(&app.timeline);
    load_shapes_from_timeline(app);

    app.canvas
        .select_symbol_instance_by_gid(session.instance_gid);
    true
}

// ------------------------------------------------------------------------
// 6. 別タブ間のクリップボード・シンボル定義自動移植ロジック
// ------------------------------------------------------------------------

fn collect_symbol_and_descendants(
    sym_id: u64,
    library: &[SymbolDef],
    reusable: &[Arc<SymbolDef>],
    visited: &mut Vec<u64>,
    out: &mut Vec<Arc<SymbolDef>>,
) {
    if visited.contains(&sym_id) {
        return;
    }
    visited.push(sym_id);
    let Some(sym) = library.iter().find(|s| s.id == sym_id) else {
        return;
    };
    for layer in &sym.timeline.layers {
        for kf in &layer.keyframes {
            for inst in kf.cel.instances.iter() {
                collect_symbol_and_descendants(
                    inst.binding.symbol_id,
                    library,
                    reusable,
                    visited,
                    out,
                );
            }
        }
    }
    if let Some(existing) = reusable.iter().find(|s| s.id == sym_id) {
        out.push(Arc::clone(existing));
    } else {
        let mut cloned = sym.clone();
        cloned.strip_transient_caches();
        out.push(Arc::new(cloned));
    }
}

/// シンボルの内部タイムライン内で使用されている最大のグループID（`group_path` / `instance_gid` / `parent_scope`）を返す
pub(crate) fn max_group_id_in_timeline(tl: &Timeline) -> u64 {
    let mut max_gid = 0u64;
    for layer in &tl.layers {
        for kf in &layer.keyframes {
            for sh in kf.cel.shapes.iter() {
                if let Some(&gid) = sh.group_path.iter().max() {
                    max_gid = max_gid.max(gid);
                }
            }
            for inst in kf.cel.instances.iter() {
                max_gid = max_gid.max(inst.binding.instance_gid);
                if let Some(&gid) = inst.parent_scope.iter().max() {
                    max_gid = max_gid.max(gid);
                }
            }
        }
    }
    max_gid
}

/// 別タブから移植されるシンボルの内部タイムライン内の全グループIDを貼り付け先タブの `SymbolManager` で新規採番したIDへ一括リマップする
fn remap_timeline_group_ids(tl: &mut Timeline, symbols: &mut SymbolManager) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    let mut map_gid = |old_gid: u64, symbols: &mut SymbolManager| -> u64 {
        *gid_map
            .entry(old_gid)
            .or_insert_with(|| symbols.alloc_group_id())
    };
    for layer in &mut tl.layers {
        for kf in &mut layer.keyframes {
            if kf
                .cel
                .shapes
                .iter()
                .any(|s| !s.group_path.is_empty() || s.symbol_instance.is_some())
            {
                for sh in kf.cel.shapes_mut() {
                    for gid in &mut sh.group_path {
                        *gid = map_gid(*gid, symbols);
                    }
                    if let Some(mut b) = sh.symbol_instance {
                        b.instance_gid = map_gid(b.instance_gid, symbols);
                        sh.symbol_instance = Some(b);
                    }
                }
            }
            if !kf.cel.instances.is_empty() {
                for inst in kf.cel.instances_mut() {
                    inst.binding.instance_gid = map_gid(inst.binding.instance_gid, symbols);
                    for gid in &mut inst.parent_scope {
                        *gid = map_gid(*gid, symbols);
                    }
                }
            }
        }
    }
}

/// `copy_selected_shapes`（`Cmd+C`）実行時に、クリップボード内の図形が参照しているシンボル定義（入れ子含む）を `Arc` 共有スナップショットとして退避する
pub(crate) fn capture_clipboard_symbols(app: &mut Is2dApp) {
    let active_doc_id = app.tabs.active_doc_id();
    if active_doc_id != app.tabs.clipboard_source_doc_id {
        app.tabs.clipboard_imported_map.clear();
    }
    let reusable = if active_doc_id == app.tabs.clipboard_source_doc_id
        && app.symbols.revision == app.tabs.clipboard_source_symbols_revision
    {
        std::mem::take(&mut app.tabs.clipboard_symbols)
    } else {
        app.tabs.clipboard_symbols.clear();
        Vec::new()
    };

    app.tabs.clipboard_source_doc_id = active_doc_id;
    app.tabs.clipboard_source_symbol_id = app.symbols.edit_stack.last().map(|s| s.symbol_id);
    app.tabs.clipboard_source_symbols_revision = app.symbols.revision;

    let mut visited = Vec::new();
    let mut collected = Vec::new();
    for shape in &app.canvas.clipboard {
        if let Some(b) = shape.symbol_instance {
            collect_symbol_and_descendants(
                b.symbol_id,
                &app.symbols.library,
                &reusable,
                &mut visited,
                &mut collected,
            );
        }
    }
    app.tabs.clipboard_symbols = collected;
}

fn existing_reusable_target_symbol_id(app: &Is2dApp, src_sym_id: u64) -> Option<u64> {
    let target_doc_id = app.tabs.active_doc_id();
    let source_doc_id = app.tabs.clipboard_source_doc_id;
    let candidate = (target_doc_id == source_doc_id)
        .then_some(src_sym_id)
        .filter(|&id| app.symbols.find_symbol(id).is_some())
        .or_else(|| {
            app.tabs
                .clipboard_imported_map
                .get(&(target_doc_id, src_sym_id))
                .copied()
                .filter(|&id| app.symbols.find_symbol(id).is_some())
        })?;

    let sym_def = app.symbols.find_symbol(candidate)?;
    let all_children_exist = sym_def.timeline.layers.iter().all(|l| {
        l.keyframes.iter().all(|kf| {
            kf.cel
                .instances
                .iter()
                .all(|inst| app.symbols.find_symbol(inst.binding.symbol_id).is_some())
        })
    });
    all_children_exist.then_some(candidate)
}

fn can_paste_clipboard_symbol_inner(
    app: &Is2dApp,
    src_sym_id: u64,
    visited: &mut Vec<u64>,
) -> bool {
    if app.symbols.edit_stack.is_empty() {
        return true;
    }
    if visited.contains(&src_sym_id) {
        return true;
    }
    visited.push(src_sym_id);

    if let Some(live_id) = existing_reusable_target_symbol_id(app, src_sym_id) {
        return app.symbols.can_place_symbol_in_current_context(live_id);
    }

    if app.tabs.active_doc_id() == app.tabs.clipboard_source_doc_id
        && app
            .symbols
            .edit_stack
            .iter()
            .any(|sess| sess.symbol_id == src_sym_id)
    {
        return false;
    }

    let Some(src_sym) = app
        .tabs
        .clipboard_symbols
        .iter()
        .find(|s| s.id == src_sym_id)
    else {
        return true;
    };

    for layer in &src_sym.timeline.layers {
        for kf in &layer.keyframes {
            for inst in kf.cel.instances.iter() {
                if !can_paste_clipboard_symbol_inner(app, inst.binding.symbol_id, visited) {
                    return false;
                }
            }
        }
    }
    true
}

/// クリップボード内のシンボル `src_sym_id` が現在の編集コンテキストに対して循環参照を起こさずに配置可能かを判定する
pub(crate) fn can_paste_clipboard_symbol(app: &Is2dApp, src_sym_id: u64) -> bool {
    let mut visited = Vec::new();
    can_paste_clipboard_symbol_inner(app, src_sym_id, &mut visited)
}

/// `paste_clipboard`（`Cmd+V`）実行前に、現在のタブでクリップボード内の図形がすべて自己・親シンボルの循環参照により配置不可能かを判定する
pub(crate) fn would_clipboard_paste_all_cycle(app: &Is2dApp) -> bool {
    if app.canvas.clipboard.is_empty() || app.symbols.edit_stack.is_empty() {
        return false;
    }
    app.canvas.clipboard.iter().all(|shape| {
        shape
            .symbol_instance
            .is_some_and(|b| !can_paste_clipboard_symbol(app, b.symbol_id))
    })
}

fn collect_needed_clipboard_imports(
    app: &Is2dApp,
    src_sym_id: u64,
    local_sym_map: &mut HashMap<u64, u64>,
    needed_imports: &mut HashSet<u64>,
    visited: &mut HashSet<u64>,
) {
    if !visited.insert(src_sym_id) {
        return;
    }
    if let Some(valid_id) = existing_reusable_target_symbol_id(app, src_sym_id) {
        local_sym_map.insert(src_sym_id, valid_id);
        return;
    }
    needed_imports.insert(src_sym_id);
    if let Some(src_sym) = app
        .tabs
        .clipboard_symbols
        .iter()
        .find(|s| s.id == src_sym_id)
    {
        for layer in &src_sym.timeline.layers {
            for kf in &layer.keyframes {
                for inst in kf.cel.instances.iter() {
                    collect_needed_clipboard_imports(
                        app,
                        inst.binding.symbol_id,
                        local_sym_map,
                        needed_imports,
                        visited,
                    );
                }
            }
        }
    }
}

/// `paste_clipboard`（`Cmd+V`）実行時に、クリップボードの不変スナップショットを破壊せず、
/// 現在のタブのライブラリに不足しているシンボル定義のみを自動移植してペースト用 `(Vec<FillShape>, had_cycle)` を返す
pub(crate) fn resolve_clipboard_for_paste(app: &mut Is2dApp) -> (Vec<FillShape>, bool) {
    let mut had_cycle = false;
    let mut resolved_shapes = Vec::with_capacity(app.canvas.clipboard.len());
    for shape in &app.canvas.clipboard {
        if let Some(b) = shape.symbol_instance {
            if !can_paste_clipboard_symbol(app, b.symbol_id) {
                had_cycle = true;
                continue;
            }
        }
        resolved_shapes.push(shape.clone());
    }

    if resolved_shapes.is_empty() || app.tabs.clipboard_symbols.is_empty() {
        return (resolved_shapes, had_cycle);
    }

    let target_doc_id = app.tabs.active_doc_id();
    let source_doc_id = app.tabs.clipboard_source_doc_id;
    let mut local_sym_map: HashMap<u64, u64> = HashMap::new();
    let mut needed_imports: HashSet<u64> = HashSet::new();
    let mut visited: HashSet<u64> = HashSet::new();

    for shape in &resolved_shapes {
        if let Some(b) = shape.symbol_instance {
            collect_needed_clipboard_imports(
                app,
                b.symbol_id,
                &mut local_sym_map,
                &mut needed_imports,
                &mut visited,
            );
        }
    }

    let mut any_imported = false;

    for idx in 0..app.tabs.clipboard_symbols.len() {
        let src_sym_id = app.tabs.clipboard_symbols[idx].id;
        if !needed_imports.contains(&src_sym_id) {
            continue;
        }
        let mut imported = (*app.tabs.clipboard_symbols[idx]).clone();

        let new_id =
            if target_doc_id == source_doc_id && app.symbols.find_symbol(imported.id).is_none() {
                app.symbols.next_symbol_id = app
                    .symbols
                    .next_symbol_id
                    .max(imported.id.saturating_add(1));
                imported.id
            } else {
                let id = app.symbols.next_symbol_id;
                app.symbols.next_symbol_id = app.symbols.next_symbol_id.saturating_add(1);
                id
            };

        local_sym_map.insert(imported.id, new_id);
        app.tabs
            .clipboard_imported_map
            .insert((target_doc_id, imported.id), new_id);

        imported.id = new_id;
        if app.symbols.library.iter().any(|s| s.name == imported.name) {
            let base_name = imported.name.clone();
            let mut suffix = 2usize;
            loop {
                let candidate = format!("{base_name} ({suffix})");
                if !app.symbols.library.iter().any(|s| s.name == candidate) {
                    imported.name = candidate;
                    break;
                }
                suffix = suffix.saturating_add(1);
            }
        }
        for layer in &mut imported.timeline.layers {
            for kf in &mut layer.keyframes {
                for inst in kf.cel.instances_mut() {
                    if let Some(&mapped_child_id) = local_sym_map.get(&inst.binding.symbol_id) {
                        inst.binding.symbol_id = mapped_child_id;
                    }
                }
            }
        }
        remap_timeline_group_ids(&mut imported.timeline, &mut app.symbols);
        let max_internal_gid = max_group_id_in_timeline(&imported.timeline);
        app.symbols.next_group_id = app
            .symbols
            .next_group_id
            .max(max_internal_gid.saturating_add(1));

        app.symbols.library.push(imported);
        if app.symbols.selected_library_symbol.is_none() {
            app.symbols.selected_library_symbol = Some(new_id);
        }
        any_imported = true;
    }

    if any_imported {
        app.symbols.bump_revision();
    }

    for shape in &mut resolved_shapes {
        if let Some(mut b) = shape.symbol_instance {
            if let Some(&mapped_id) = local_sym_map.get(&b.symbol_id) {
                b.symbol_id = mapped_id;
                shape.symbol_instance = Some(b);
            }
        }
    }
    (resolved_shapes, had_cycle)
}
