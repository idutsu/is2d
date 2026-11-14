use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Key, Pos2, Stroke, Vec2};
use kurbo::{Affine, Point, QuadBez, Rect as KRect, Vec2 as KVec2};

use crate::map_hover::ScopeMaps;
use crate::planar_map::LayerBase;
pub(crate) use crate::symbol::Document;
use crate::symbol::{EditPath, Library, SymbolDef, SymbolEvalCtx, SymbolEvalState};
use crate::timeline::{Timeline, TimelineCellSelection};
use crate::vector_engine::{FillShape, LayerId, ShapeId, SyncCache};

pub(crate) const MAX_UNDO_HISTORY: usize = 100;
const DEFAULT_TOAST_DURATION: Duration = Duration::from_millis(2800);

// ============================================================================
// 1. トースト通知・ツール・ドラッグ状態マシン
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

/// 操作時に一定時間表示して自動で消えるトースト通知メッセージ
#[derive(Clone, Debug)]
pub(crate) struct StatusToast {
    pub(crate) kind: NoticeKind,
    pub(crate) message: String,
    pub(crate) expires_at: Instant,
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
    /// 全ツールバリアントの標準順序
    pub(crate) const ALL: [Tool; 6] = [
        Tool::Select,
        Tool::Transform,
        Tool::Brush,
        Tool::Eraser,
        Tool::Bucket,
        Tool::Eyedropper,
    ];

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
        Self::ALL
            .into_iter()
            .find(|tool| key_pressed_no_repeat(i, tool.shortcut_key()))
    }
}

/// キーリピートを除外して「押された最初のフレームのみ `true`」を返すヘルパー
pub(crate) fn key_pressed_no_repeat(i: &egui::InputState, target: Key) -> bool {
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
}

/// 選択ツール (`Tool::Select`) と自由変形ツール (`Tool::Transform`) で共通利用する選択オブジェクト平行移動ドラッグ状態
#[derive(Clone, Debug)]
pub(crate) struct MoveSelectionDrag {
    pub(crate) last_pos: Point,
    pub(crate) total_delta: KVec2,
    pub(crate) alt_duplicate: bool,
}

impl MoveSelectionDrag {
    pub(crate) fn new(start_pos: Point, alt_duplicate: bool) -> Self {
        Self {
            last_pos: start_pos,
            total_delta: KVec2::ZERO,
            alt_duplicate,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum SelectSubMode {
    #[default]
    Reshape, // 通常の輪郭変形・頂点移動・図形移動（V キー）
    AddAnchor,    // ＋ アンカー追加モード（+ キー）
    DeleteAnchor, // － アンカー削除モード（- キー）
}

/// 地図の頂点・辺が、どの地図のものか
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MapOwner {
    /// レイヤーの、編集中の階層の生の図形（選択中の図形を除く）の地図
    Layer(LayerId),
    /// 動かして浮いている片のうち、そのレイヤーの、その番目の片の組の地図
    Floating(LayerId, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) enum HoverTarget {
    #[default]
    None,
    Vertex {
        shape_id: ShapeId,
        /// 頂点のある輪郭（`shape_id` の図形の何番目の輪郭か。ワイヤーフレームで、その輪郭の頂点を出す）
        contour_idx: usize,
        pos: Point,
        /// 地図の頂点（どの地図か, 頂点の番号, 文書の座標の位置）。`pos` はステージの座標の位置（表示と距離）、編集
        /// （`MapEdit`）は文書の座標の位置で頂点を探す
        map_node: Option<(MapOwner, usize, Point)>,
    },
    Edge {
        shape_id: ShapeId,
        t: f64,
        closest_pos: Point,
        /// 強調表示する曲線（地図の辺のステージの座標の形）
        seg: QuadBez,
        /// 地図の辺（どの地図か, 文書の座標の形）。編集（`MapEdit`）はこの形で辺を探す
        map_edge: Option<(MapOwner, QuadBez)>,
    },
    Fill {
        shape_id: ShapeId,
    },
}

impl HoverTarget {
    pub(crate) fn shape_id(&self) -> Option<ShapeId> {
        match *self {
            HoverTarget::Vertex { shape_id, .. }
            | HoverTarget::Edge { shape_id, .. }
            | HoverTarget::Fill { shape_id } => Some(shape_id),
            HoverTarget::None => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SelectDragState {
    /// 地図の、形が `orig_seg` の辺の `grab_t` の所を `target` へ引っぱる（形と位置は地図の文書の座標）
    PullEdge {
        layer_id: LayerId,
        group_path: Vec<u64>,
        base: Arc<LayerBase>,
        grab_t: f64,
        /// マウスから、つかんだ所までのずれ（ステージの座標）
        grab_offset: KVec2,
        target: Point,
        orig_seg: QuadBez,
    },
    /// 地図の頂点 `node`（`orig_vertex` にある）を `target` へ動かす（位置は地図の文書の座標）
    MoveVertex {
        layer_id: LayerId,
        group_path: Vec<u64>,
        base: Arc<LayerBase>,
        node: usize,
        orig_vertex: Point,
        target: Point,
        /// 吸い付いた頂点の、ステージの座標の位置（表示）
        snapped_to: Option<Point>,
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
        had_custom_pivot: bool,
        last_pivot: Point,
        changed: bool,
    },
    Scale {
        handle: ScaleHandle,
        anchor: Point,
        handle_start: Point,
        orig_pivot: Point,
        had_custom_pivot: bool,
        orig_shapes: Vec<FillShape>,
        last_affine: Affine,
        changed: bool,
    },
    Rotate {
        orig_bbox: KRect,
        pivot: Point,
        had_custom_pivot: bool,
        start_angle: f64,
        current_angle: f64,
        orig_shapes: Vec<FillShape>,
    },
}

/// キャンバス上で進行中のドラッグ操作を表す排他的状態マシン
/// （ブラシ/消しゴムのストローク、選択ツールのドラッグ、自由変形ツールのドラッグが同時に複数存在できないことを型で保証する）
#[derive(Clone, Debug, Default)]
pub(crate) enum ActiveCanvasDrag {
    #[default]
    None,
    PaintStroke {
        stroke: Vec<Point>,
        committed_world_mesh: egui::Mesh,
        tail_mesh: egui::Mesh,
        tail_start_idx: usize,
    },
    /// 選択した図形の移動（選択ツール・自由変形ツール共通）
    MoveSelection(MoveSelectionDrag),
    /// 矩形選択（選択ツール・自由変形ツール共通）
    Marquee(MarqueeDrag),
    Select(SelectDragState),
    Transform(TransformDragState),
}

// ============================================================================
// 2. タブごとの表示状態（View）
// ============================================================================

/// タブごとの表示状態（ズーム・パン・枠外マスク・アンカー表示・オニオンスキン・ライブラリ選択、保存しない）
#[derive(Clone)]
pub(crate) struct View {
    /// キャンバスのズーム倍率（1.0 = 100%, 範囲 0.15..=16.0）
    pub(crate) zoom: f32,
    /// キャンバスのパンオフセット（スクリーンピクセル単位）
    pub(crate) pan: Vec2,
    /// スペースドラッグ / 中ボタンドラッグによるパン操作中の直前スクリーン座標
    pub(crate) pan_last_screen_pos: Option<Pos2>,
    /// ステージ枠外にはみ出た図形をマスクして隠すかどうか（完成画面プレビュー用）
    pub(crate) clip_outside_stage: bool,
    /// 選択ツール使用時に全生シェイプのベクター輪郭・アンカーを表示するか
    pub(crate) show_wireframe: bool,
    /// オニオンスキン表示の有効/無効
    pub(crate) onion_skin_enabled: bool,
    /// オニオンスキンで表示する前のフレーム数
    pub(crate) onion_prev_frames: usize,
    /// オニオンスキンで表示する次のフレーム数
    pub(crate) onion_next_frames: usize,
    /// プロパティパネルの下部表示対象（`true`: タイムラインのフレーム/トゥイーン属性、`false`: キャンバス上のシンボル属性）
    pub(crate) inspect_timeline_frame: bool,
    /// ライブラリパネルで現在選択中のシンボルID
    pub(crate) selected_library_symbol: Option<u64>,
    /// ライブラリのシンボルのミニプレビューの写し
    pub(crate) symbol_previews: crate::symbol::SymbolPreviews,
    /// キャンバスでの前の主ボタンのクリック（画面の位置と時刻）。ダブルクリックを位置も比べて決めるため（`double_clicked`）
    pub(crate) last_click: Option<(Pos2, f64)>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            pan_last_screen_pos: None,
            clip_outside_stage: false,
            show_wireframe: false,
            onion_skin_enabled: false,
            onion_prev_frames: 2,
            onion_next_frames: 2,
            inspect_timeline_frame: false,
            selected_library_symbol: None,
            symbol_previews: Default::default(),
            last_click: None,
        }
    }
}

impl View {
    /// ライブラリで選択中のシンボルIDが有効か検証し、未選択または削除済みの場合は先頭シンボルを選択する
    pub(crate) fn ensure_valid_library_selection(&mut self, library: &Library) {
        if self
            .selected_library_symbol
            .is_none_or(|id| !library.iter().any(|s| s.id == id))
        {
            self.selected_library_symbol = library.first().map(|s| s.id);
        }
    }

    /// ワールド座標 `p` を現在のズーム・パンに応じたスクリーン座標 `Pos2` へ変換する
    #[inline]
    pub(crate) fn world_to_screen(&self, p: Point) -> Pos2 {
        crate::vector_engine::to_screen(p.x as f32, p.y as f32, self.zoom, self.pan)
    }

    /// スクリーン座標 `pos` を現在のズーム・パンに応じたワールド座標 `Point` へ逆変換する
    #[inline]
    pub(crate) fn screen_to_world(&self, pos: Pos2) -> Point {
        Point::new(
            ((pos.x - self.pan.x) / self.zoom) as f64,
            ((pos.y - self.pan.y) / self.zoom) as f64,
        )
    }

    /// ポインタを押した位置をワールド座標で返す（押していなければ `None`）
    pub(crate) fn press_origin_world(&self, ctx: &egui::Context) -> Option<Point> {
        ctx.input(|i| i.pointer.press_origin())
            .map(|p| self.screen_to_world(p))
    }

    /// キャンバスでの主ボタンのダブルクリックか（ダブルクリックの判定はこれ1つ）：前のクリックから egui の
    /// `max_double_click_delay` 以内に、画面で egui の `max_click_dist`（クリックとみなす動きの上限）以内の所を、もう1回
    /// クリックしたとき。egui の `double_clicked` は時間だけで決めるので、A をクリックしてすぐ B をクリックすると、B の
    /// ダブルクリックになっていた。ダブルクリックのあとの3回目は、新しい1回目として数える。クリックのあるフレームでは
    /// 必ず呼ぶこと（1回目のクリックを覚える）
    pub(crate) fn double_clicked(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
    ) -> bool {
        if !response.clicked_by(egui::PointerButton::Primary) {
            return false;
        }
        let (pos, time) = ctx.input(|i| (i.pointer.interact_pos(), i.time));
        let (delay, dist) = ctx.options(|o| {
            (
                o.input_options.max_double_click_delay,
                o.input_options.max_click_dist,
            )
        });
        let Some(pos) = pos else {
            self.last_click = None;
            return false;
        };
        let double = self
            .last_click
            .is_some_and(|(p, t)| time - t < delay && p.distance(pos) <= dist);
        self.last_click = (!double).then_some((pos, time));
        double
    }

    /// ドラッグを始めたときに、ポインタが押した位置 `press`（ワールド座標）から `now` へもう動いているか（この判定はこれ1つ）
    pub(crate) fn moved_since_press(press: Point, now: Point) -> bool {
        (now - press).hypot() > crate::vector_engine::SAME_TOL
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

    /// ステージ枠全体が中央キャンバス領域 `canvas_rect` に余白付きでぴったり収まるようにズーム倍率とパン位置を自動調整する（「ウィンドウに合わせる」）
    pub(crate) fn fit_stage_to_window(&mut self, stage_size: Vec2, canvas_rect: egui::Rect) {
        let stage_w = stage_size.x.max(100.0);
        let stage_h = stage_size.y.max(100.0);
        let avail_w = (canvas_rect.width() - 48.0).max(64.0);
        let avail_h = (canvas_rect.height() - 48.0).max(64.0);
        let zoom = (avail_w / stage_w).min(avail_h / stage_h).clamp(
            crate::vector_engine::MIN_VIEW_ZOOM,
            crate::vector_engine::MAX_VIEW_ZOOM,
        );
        let center = Self::DEFAULT_STAGE_CENTER;
        let screen_center = canvas_rect.center();
        self.zoom = zoom;
        self.pan = Vec2::new(
            screen_center.x - (center.x as f32) * zoom,
            screen_center.y - (center.y as f32) * zoom,
        );
    }

    /// ステージ（カメラ出力枠）の初期ワールド中心座標
    pub(crate) const DEFAULT_STAGE_CENTER: Point = Point::new(536.0, 310.0);

    /// ステージ（カメラ出力枠）のワールド座標矩形 `KRect` を返す
    #[inline]
    pub(crate) fn stage_world_rect(&self, stage_size: Vec2) -> KRect {
        let center = Self::DEFAULT_STAGE_CENTER;
        let half_w = (stage_size.x.max(100.0) as f64) * 0.5;
        let half_h = (stage_size.y.max(100.0) as f64) * 0.5;
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
    pub(crate) fn stage_screen_rect(&self, stage_size: Vec2) -> egui::Rect {
        self.world_rect_to_screen(self.stage_world_rect(stage_size))
    }

    /// 現在スクリーン上に表示されるワールド座標範囲 `KRect` を返す（画面外・ステージ枠外カリング用）
    #[inline]
    pub(crate) fn visible_world_rect(
        &self,
        painter: &egui::Painter,
        stage_size: Vec2,
        shows_stage: bool,
    ) -> KRect {
        let clip_scr = painter.clip_rect();
        let view_rect = KRect::from_points(
            self.screen_to_world(clip_scr.min),
            self.screen_to_world(clip_scr.max),
        );
        if shows_stage && self.clip_outside_stage {
            view_rect.intersect(self.stage_world_rect(stage_size))
        } else {
            view_rect
        }
    }

    /// 図形描画の前に、ペーストボード（枠外のグレー背景）とステージ矩形（白いキャンバス枠）を描画する。グループ・シンボルの
    /// 中をその場で編集しているときも同じ（中身は置いてある所のまま描くので、ステージはいつもワールド座標の同じ所にある）。
    /// ステージを見せないとき（ライブラリから入ったシンボルの編集：`EditPath::shows_stage`）は、全面を白にする
    pub(crate) fn draw_stage_background(
        &self,
        painter: &egui::Painter,
        canvas_rect: egui::Rect,
        stage_size: Vec2,
        shows_stage: bool,
    ) {
        if !shows_stage {
            painter.rect_filled(canvas_rect, 0.0, Color32::WHITE);
            return;
        }
        let pasteboard_col = Color32::from_rgb(218, 222, 228);
        painter.rect_filled(canvas_rect, 0.0, pasteboard_col);

        let stage_rect = self.stage_screen_rect(stage_size);
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
        stage_size: Vec2,
        shows_stage: bool,
    ) -> egui::Painter {
        if shows_stage && self.clip_outside_stage {
            painter.with_clip_rect(canvas_rect.intersect(self.stage_screen_rect(stage_size)))
        } else {
            painter.clone()
        }
    }

    /// 図形描画の後に、上からステージの境界枠線を描画する
    pub(crate) fn draw_stage_foreground_overlay(
        &self,
        painter: &egui::Painter,
        stage_size: Vec2,
        shows_stage: bool,
    ) {
        if !shows_stage {
            return;
        }
        let stage_rect = self.stage_screen_rect(stage_size);
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
        allow_pan: bool,
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

        let wants_kb = ui.ctx().wants_keyboard_input();
        let (space_down, middle_down, primary_down, press_origin) = ui.input(|i| {
            (
                !wants_kb && i.key_down(Key::Space),
                i.pointer.middle_down(),
                i.pointer.primary_down(),
                i.pointer.press_origin(),
            )
        });
        let started_in_canvas = press_origin
            .or(pointer_pos)
            .is_some_and(|p| canvas_rect.contains(p));
        let pan_gesture_active = allow_pan
            && (middle_down || (space_down && primary_down))
            && (self.pan_last_screen_pos.is_some() || started_in_canvas);
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
            if space_down && allow_pan {
                is_panning = true;
            }
        }
        (is_panning, pan_gesture_active)
    }
}

// ============================================================================
// 3. 共有クリップボード（Clipboard）& タブごとの編集セッション状態（Session）
// ============================================================================

/// アプリ全体で1つだけ保持される内部クリップボード状態
#[derive(Clone, Default)]
pub(crate) struct Clipboard {
    /// コピー / カットした中身（地図とインスタンス。階層の番号の並びは、写した所からの相対）
    pub(crate) content: Option<crate::clipboard_ops::Content>,
    /// クリップボードにコピーされた時点のコピー元ドキュメントID（別タブへのペースト判定用）
    pub(crate) source_doc_id: u64,
    /// クリップボード内のシェイプが参照しているシンボル定義のスナップショット群（依存順）
    pub(crate) symbols: Vec<Arc<SymbolDef>>,
    /// 各コピー元タブから貼り付け先タブへ既にインポート済みのシンボルID対応表 `(source_doc_id, source_sym_id, target_doc_id) -> target_sym_id`
    pub(crate) imported_map: HashMap<(u64, u64, u64), u64>,
    /// `Cmd+V` 押下時に `Event::Paste` または `Key::V` (pressed) で既にペーストを実行済みかのフラグ（OSクリップボード空時のキーリリースフォールバック二重発火防止用）
    pub(crate) paste_triggered_on_press: bool,
    /// 直近フレームで `Cmd`/`Ctrl` 修飾キーが押されていた猶予フレーム数（`V` より先に `Cmd` を離した場合のフォールバック用）
    pub(crate) cmd_held_grace_frames: u8,
    /// `Cmd+V` による通常ペーストの連続回数（少しずつずらして配置するためのカウンタ）
    pub(crate) paste_count: usize,
    /// `Cmd+V` による通常ペーストの累積ワールド座標オフセット（途中でズーム変更しても階段状にずらす用）
    pub(crate) paste_offset: KVec2,
}

pub(crate) type InstanceGid = u64;

/// キャンバス上の選択状態（図形ID集合とシンボルインスタンスGID集合）
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) shapes: HashSet<ShapeId>,
    pub(crate) instances: HashSet<InstanceGid>,
}

impl Selection {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.shapes.is_empty() && self.instances.is_empty()
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.shapes.clear();
        self.instances.clear();
    }

    #[inline]
    pub(crate) fn contains_shape(&self, shape: &FillShape) -> bool {
        if let Some(binding) = shape.symbol_instance {
            self.instances.contains(&binding.instance_gid)
        } else {
            self.shapes.contains(&shape.shape_id)
        }
    }

    #[inline]
    pub(crate) fn insert_shape(&mut self, shape: &FillShape) {
        if let Some(binding) = shape.symbol_instance {
            self.instances.insert(binding.instance_gid);
        } else {
            self.shapes.insert(shape.shape_id);
        }
    }
}

pub(crate) type Scope = Vec<u64>;

mod cel_writes;
mod scene_base_cache;
pub(crate) use self::cel_writes::*;

pub(crate) use crate::vector_engine::CelTint;
pub(crate) use scene_base_cache::SceneBaseCache;
use scene_base_cache::{BackdropCache, CelMeshCache};

/// `Overlay` で追加する図形の重なり順指定
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// 隠した元の図形と同じ重なり順に置く
    ReplaceAt(ShapeId),
    /// そのレイヤー・その編集スコープの一番上に置く
    TopOfLayer(LayerId, Scope),
}

/// キャンバスの土台図形列に合成する上乗せ図形状態（ドラッグ中の仮表示や Floating）
#[derive(Clone, Debug, Default)]
pub(crate) struct Overlay {
    /// 画面から隠す元の図形
    pub(crate) hide: HashSet<ShapeId>,
    /// 追加で見せる図形
    pub(crate) add: Vec<(Placement, FillShape)>,
}

impl Overlay {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.hide.is_empty() && self.add.is_empty()
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.hide.clear();
        self.add.clear();
    }
}

/// `Scene` の表示順リストの1要素（土台の何番目か、上乗せの何番目か）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SceneEntry {
    Base(usize),
    Overlay(usize),
}

#[inline]
fn layer_order_index(layer_order_ids: &[LayerId], lid: LayerId) -> usize {
    layer_order_ids
        .iter()
        .position(|&id| id == lid)
        .unwrap_or(usize::MAX)
}

/// 土台の図形列 `base` と上乗せ `overlay` から画面の重なり順リストを構築する（レイヤーID順序配列指定）
pub(crate) fn build_scene_order_by_layer_ids(
    base: &[FillShape],
    overlay: &Overlay,
    layer_order_ids: &[LayerId],
) -> Vec<SceneEntry> {
    let mut before_all: Vec<(usize, usize)> = Vec::new();
    let mut after_base: Vec<Vec<(usize, usize)>> = vec![Vec::new(); base.len()];
    let mut replace_at_base: Vec<Vec<usize>> = vec![Vec::new(); base.len()];

    for (add_idx, (placement, shape)) in overlay.add.iter().enumerate() {
        // 置き場が見つからなかったときに、レイヤーの順番で入れる位置を決めるレイヤー
        let layer_order_fallback = match placement {
            Placement::ReplaceAt(target_id) => {
                if let Some(pos) = base.iter().position(|s| s.shape_id == *target_id) {
                    replace_at_base[pos].push(add_idx);
                    None
                } else {
                    Some(shape.layer_id)
                }
            }
            Placement::TopOfLayer(lid, scope) => {
                let scope_pos = (!scope.is_empty())
                    .then(|| {
                        base.iter()
                            .rposition(|s| s.layer_id == *lid && s.is_in_editing_scope(scope))
                    })
                    .flatten();
                if let Some(pos) = scope_pos {
                    after_base[pos].push((0, add_idx));
                    None
                } else if let Some(pos) = base.iter().rposition(|s| s.layer_id == *lid) {
                    after_base[pos].push((1, add_idx));
                    None
                } else {
                    Some(*lid)
                }
            }
        };
        if let Some(lid) = layer_order_fallback {
            let ord = layer_order_index(layer_order_ids, lid);
            if let Some(pos) = base
                .iter()
                .rposition(|s| layer_order_index(layer_order_ids, s.layer_id) <= ord)
            {
                after_base[pos].push((2 + ord.min(usize::MAX - 2), add_idx));
            } else {
                before_all.push((ord, add_idx));
            }
        }
    }

    let mut out = Vec::with_capacity(base.len() + overlay.add.len());
    before_all.sort_by_key(|&(ord, _)| ord);
    for (_, add_idx) in before_all {
        out.push(SceneEntry::Overlay(add_idx));
    }
    for base_idx in 0..base.len() {
        for &add_idx in &replace_at_base[base_idx] {
            out.push(SceneEntry::Overlay(add_idx));
        }
        if !overlay.hide.contains(&base[base_idx].shape_id) {
            out.push(SceneEntry::Base(base_idx));
        }
        if !after_base[base_idx].is_empty() {
            after_base[base_idx].sort_by_key(|&(prio, _)| prio);
            for &(_, add_idx) in &after_base[base_idx] {
                out.push(SceneEntry::Overlay(add_idx));
            }
        }
    }
    out
}

/// `Scene` の図形を重なり順に走査するイテレータ
#[derive(Clone)]
pub(crate) struct SceneIter<'a> {
    scene: Scene<'a>,
    front: usize,
    back: usize,
}

impl<'a> Iterator for SceneIter<'a> {
    type Item = &'a FillShape;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while self.front < self.back {
            let idx = self.front;
            self.front += 1;
            if let Some(s) = self.scene.get(idx) {
                return Some(s);
            }
        }
        None
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.back.saturating_sub(self.front);
        (rem, Some(rem))
    }
}

impl<'a> DoubleEndedIterator for SceneIter<'a> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        while self.front < self.back {
            self.back -= 1;
            if let Some(s) = self.scene.get(self.back) {
                return Some(s);
            }
        }
        None
    }
}

impl<'a> ExactSizeIterator for SceneIter<'a> {}

/// 現在のフレームと編集位置で解決した、描画・当たり判定用の読み取り専用図形ビュー（土台＋上乗せの合成）
#[derive(Clone, Copy)]
pub(crate) struct Scene<'a> {
    base: &'a [FillShape],
    overlay: &'a Overlay,
    order: &'a [SceneEntry],
    /// 上乗せが空なので、土台をそのまま読む（`order` は空）
    base_only: bool,
    overlay_only_selected: bool,
    overlay_movable_start: usize,
}

impl<'a> Scene<'a> {
    #[inline]
    pub(crate) fn len(self) -> usize {
        if self.base_only {
            self.base.len()
        } else {
            self.order.len()
        }
    }

    #[inline]
    pub(crate) fn get(self, idx: usize) -> Option<&'a FillShape> {
        if self.base_only {
            return self.base.get(idx);
        }
        match self.order.get(idx)? {
            SceneEntry::Base(i) => self.base.get(*i),
            SceneEntry::Overlay(j) => self.overlay.add.get(*j).map(|(_, s)| s),
        }
    }

    /// 重なり順（背面→前面）で図形を走査するイテレータを返す
    #[inline]
    pub(crate) fn iter(self) -> SceneIter<'a> {
        SceneIter {
            scene: self,
            front: 0,
            back: self.len(),
        }
    }

    /// 指定した `shape_id` を持つ図形を `Scene` 内から検索する
    #[inline]
    pub(crate) fn shape_by_id(self, shape_id: ShapeId) -> Option<&'a FillShape> {
        self.iter().find(|s| s.shape_id == shape_id)
    }

    /// 指定した `shape_id` を持つ図形の `Scene` インデックスを返す
    #[inline]
    pub(crate) fn index_of_shape_id(self, shape_id: ShapeId) -> Option<usize> {
        (0..self.len()).find(|&idx| self.get(idx).is_some_and(|s| s.shape_id == shape_id))
    }

    /// 編集スコープ `editing_group_path` 内で、指定座標 `pt` にヒットする最前面シェイプの `Scene` インデックスを返す
    pub(crate) fn topmost_shape_at(self, pt: Point, editing_group_path: &[u64]) -> Option<usize> {
        (0..self.len()).rev().find(|&idx| {
            self.get(idx)
                .is_some_and(|s| s.is_in_editing_scope(editing_group_path) && s.contains_point(pt))
        })
    }

    /// 指定座標 `pt` で最前面の図形にヒットしなければ、`compute_hover_target` の図形の `Scene` インデックスを返す
    pub(crate) fn hit_shape_or_hover_at(
        self,
        session: &Session,
        pt: Point,
        zoom: f32,
        sub_mode: SelectSubMode,
    ) -> Option<usize> {
        self.topmost_shape_at(pt, &session.editing_group_path)
            .or_else(|| {
                crate::select_ops::compute_hover_target(self, session, pt, zoom, sub_mode)
                    .shape_id()
                    .and_then(|id| self.index_of_shape_id(id))
            })
    }

    /// 指定シェイプが編集スコープ `editing_group_path` で子グループに属していれば、同じ子グループに属する全シェイプの `Scene` インデックス列を返す。
    /// 生シェイプであれば `vec![scene_idx]` のみを返す。
    pub(crate) fn expand_to_group_unit(
        self,
        scene_idx: usize,
        editing_group_path: &[u64],
    ) -> Vec<usize> {
        let Some(shape) = self.get(scene_idx) else {
            return Vec::new();
        };
        if let Some(gid) = shape.active_group_id_at_scope(editing_group_path) {
            (0..self.len())
                .filter(|&i| {
                    self.get(i)
                        .and_then(|s| s.active_group_id_at_scope(editing_group_path))
                        == Some(gid)
                })
                .collect()
        } else {
            vec![scene_idx]
        }
    }

    /// 指定 `Scene` インデックスの図形が現在選択されているかを返す
    #[inline]
    pub(crate) fn is_shape_selected(self, scene_idx: usize, selection: &Selection) -> bool {
        if self.overlay_only_selected && !self.base_only {
            match self.order.get(scene_idx) {
                Some(SceneEntry::Overlay(ov_idx)) if *ov_idx >= self.overlay_movable_start => {}
                _ => return false,
            }
        }
        self.get(scene_idx)
            .is_some_and(|s| selection.contains_shape(s))
    }

    /// `selection` で選択されている図形を `Scene` の順に返す
    pub(crate) fn selected_shapes<'s>(
        self,
        selection: &'s Selection,
    ) -> impl Iterator<Item = &'a FillShape> + use<'a, 's> {
        (0..self.len())
            .filter(move |&idx| self.is_shape_selected(idx, selection))
            .filter_map(move |idx| self.get(idx))
    }

    /// `selection` に対応する選択マスク（`Vec<bool>`）と有効な選択が1つ以上あるかを返す
    pub(crate) fn selected_shape_mask(self, selection: &Selection) -> (Vec<bool>, bool) {
        let len = self.len();
        if selection.is_empty() {
            return (vec![false; len], false);
        }
        let mut is_sel = vec![false; len];
        let mut any_valid = false;
        for (idx, flag) in is_sel.iter_mut().enumerate() {
            if self.is_shape_selected(idx, selection) {
                *flag = true;
                any_valid = true;
            }
        }
        (is_sel, any_valid)
    }

    /// 現在の選択対象に編集スコープ `editing_group_path` 内の図形が1つ以上含まれているか
    pub(crate) fn selected_has_any_in_scope(
        self,
        selection: &Selection,
        editing_group_path: &[u64],
    ) -> bool {
        if selection.is_empty() {
            return false;
        }
        self.selected_shapes(selection)
            .any(|s| s.is_in_editing_scope(editing_group_path))
    }

    /// 現在の選択対象にグループ化されたオブジェクトが含まれているか
    pub(crate) fn selection_has_group(
        self,
        selection: &Selection,
        editing_group_path: &[u64],
    ) -> bool {
        if selection.is_empty() {
            return false;
        }
        self.selected_shapes(selection)
            .any(|s| s.active_group_id_at_scope(editing_group_path).is_some())
    }

    /// 選択中シェイプ群の全体のバウンディングボックスを返す
    /// （可視シェイプが含まれる場合は `geometry_bounding_box()` の外接矩形を返し、空白フレームシンボルマーカーのみの場合はその基準点から算出する）
    pub(crate) fn selection_bounding_box(self, selection: &Selection, zoom: f32) -> Option<KRect> {
        if selection.is_empty() {
            return None;
        }
        let mut geom_iter = self
            .selected_shapes(selection)
            .filter_map(|s| s.geometry_bounding_box());
        let mut bbox = match geom_iter.next() {
            Some(first) => geom_iter.fold(first, |acc, r| acc.union(r)),
            None => {
                let mut fallback = self.selected_shapes(selection).map(|s| s.bounding_box());
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

    /// 現在選択されている図形が単一のシンボルインスタンスである場合、その `SymbolInstanceBinding` を返す
    pub(crate) fn selected_single_symbol_binding(
        self,
        selection: &Selection,
    ) -> Option<crate::symbol::SymbolInstanceBinding> {
        if !selection.shapes.is_empty() || selection.instances.len() != 1 {
            return None;
        }
        let &gid = selection.instances.iter().next()?;
        self.selected_shapes(selection)
            .find_map(|s| s.symbol_instance.filter(|b| b.instance_gid == gid))
    }

    /// 自由変形ツールの現在の基準点（ピボット座標）を返す
    /// （単一シンボルインスタンス選択時はインスタンスに保存されたピボット座標 `binding.world_pivot()` を返す）
    pub(crate) fn active_transform_pivot(
        self,
        selection: &Selection,
        transform_pivot: Option<Point>,
        bbox: KRect,
    ) -> Point {
        if let Some(piv) = transform_pivot {
            return piv;
        }
        if let Some(binding) = self.selected_single_symbol_binding(selection) {
            return binding.world_pivot();
        }
        bbox.center()
    }

    /// 現在の選択対象に初期位置から移動されたカスタム変形中心（ピボット）が設定されているかを返す
    pub(crate) fn has_custom_transform_pivot(
        self,
        selection: &Selection,
        transform_pivot: Option<Point>,
    ) -> bool {
        if transform_pivot.is_some() {
            return true;
        }
        self.selected_single_symbol_binding(selection)
            .is_some_and(|b| crate::vector_engine::is_custom_pivot(b.pivot))
    }
}

pub(crate) use crate::floating::Floating;
use crate::floating::Stage;

/// タブごとの編集セッション状態（土台キャッシュ・上乗せ・選択・変形・排他ドラッグ・編集階層、保存しない）
#[derive(Default)]
pub(crate) struct Session {
    /// シンボルインプレイス編集の経路（空ならルート）
    pub(crate) edit_path: EditPath,
    /// シンボル編集中の親シーン背景薄表示のレイヤー単位キャッシュ
    pub(crate) backdrop_cache: SyncCache<BackdropCache>,
    /// オニオンスキンで描くセル画の結合メッシュの写し
    pub(crate) onion_cache: SyncCache<CelMeshCache>,
    /// ロック中・再生中のレイヤーのセル画の結合メッシュの写し
    pub(crate) cel_cache: SyncCache<CelMeshCache>,
    /// 描くたびのシンボルの評価（オニオンスキン・再生・ロック中のレイヤー）で使い回す、シンボルの中身のメモ
    pub(crate) symbol_memo: SyncCache<SymbolEvalState>,
    /// `Document` から構築されるレイヤー別の読み取り専用土台キャッシュ
    pub(crate) base_cache: SceneBaseCache,
    /// 土台図形列に対する上乗せ図形状態（Floating およびドラッグ中の仮表示）
    pub(crate) overlay: Overlay,
    /// `Alt` 複製ドラッグ中など、`Overlay` 内の可動図形のみを選択表示・境界枠対象とするフラグ
    pub(crate) overlay_only_selected: bool,
    /// `overlay.add` のうちドラッグで移動・変形する図形の開始インデックス（`0..overlay_movable_start` はスライス残り等の固定背景）
    pub(crate) overlay_movable_start: usize,
    /// `Scene` の合成順序リストキャッシュ（土台または上乗せが変わったときだけ再構築する）
    pub(crate) scene_order: std::sync::OnceLock<Vec<SceneEntry>>,
    /// キャンバス上で進行中のドラッグ操作（ブラシ/消しゴム・選択・自由変形の排他的状態マシン）
    pub(crate) drag: ActiveCanvasDrag,
    pub(crate) selection: Selection,
    floating: Option<Floating>,
    pub(crate) hover_target: HoverTarget,
    /// 現在インプレイス編集で潜っているグループ階層のパス（空 `[]` ならルートシーン）
    pub(crate) editing_group_path: Vec<u64>,
    pub(crate) transform_pivot: Option<Point>,
    pub(crate) transform_hover: TransformHoverTarget,
    /// 輪郭の変形のカーソル合わせに使う、境目の線で持つ地図（生の図形が変わるまで使い回す）
    pub(crate) planar_maps: SyncCache<Option<Arc<ScopeMaps>>>,
    /// この操作で編集を断った理由（`apply` が知らせて戻す）
    pub(crate) refusal: Option<Refusal>,
}

impl Session {
    /// `Session` 単体から読み取り専用の `Scene` ビューを返す
    #[inline]
    pub(crate) fn scene(&self) -> Scene<'_> {
        let base = self.base_cache.shapes();
        let base_only = self.overlay.is_empty();
        let order: &[SceneEntry] = if base_only {
            &[]
        } else {
            self.scene_order.get_or_init(|| {
                build_scene_order_by_layer_ids(
                    base,
                    &self.overlay,
                    self.base_cache.layer_order_ids(),
                )
            })
        };
        Scene {
            base,
            overlay: &self.overlay,
            order,
            base_only,
            overlay_only_selected: self.overlay_only_selected,
            overlay_movable_start: self.overlay_movable_start,
        }
    }

    /// `Scene` の合成順序キャッシュを無効化する（土台または `overlay` の構成が変わったときに呼ぶ）
    #[inline]
    pub(crate) fn invalidate_scene_order(&mut self) {
        let _ = self.scene_order.take();
    }

    /// 現在の `Floating` への読み取り専用参照を返す
    #[inline]
    pub(crate) fn floating(&self) -> Option<&Floating> {
        self.floating.as_ref()
    }

    /// `Floating` を置き換え、`overlay` を同期再構築する
    pub(crate) fn set_floating(&mut self, fl: Option<Floating>) {
        self.floating = fl;
        self.sync_floating();
    }

    /// `Floating` を取り出し、`overlay` を同期再構築する
    pub(crate) fn take_floating(&mut self) -> Option<Floating> {
        let fl = self.floating.take();
        self.rebuild_overlay_from_floating();
        fl
    }

    /// クロージャで `Floating` を更新し、`overlay` を同期再構築する
    pub(crate) fn update_floating<R>(&mut self, f: impl FnOnce(&mut Option<Floating>) -> R) -> R {
        let res = f(&mut self.floating);
        self.sync_floating();
        res
    }

    /// 浮いた片の操作に要る画面の土台（セル画の図形のステージの写し・編集中の階層・ステージへの変換）
    pub(crate) fn stage(&self) -> Stage<'_> {
        Stage {
            base: &self.base_cache,
            scope: &self.editing_group_path,
        }
    }

    /// 浮いた片の写しを作り直し（片も残りも無ければ外す）、`overlay` を同期再構築する
    fn sync_floating(&mut self) {
        let world = self.edit_path.world_xform();
        if let Some(fl) = &mut self.floating {
            fl.refresh(
                world,
                &self.editing_group_path,
                self.base_cache.layer_order_ids(),
            );
            if fl.is_empty() {
                self.floating = None;
            }
        }
        self.rebuild_overlay_from_floating();
    }

    /// 浮いた片の操作 `f` をする（浮いた片が無ければ、`modified` が `false` の空の浮いた片に）。片の番号が変わるので、
    /// 選択は、片をみな選び、もう無い図形を外す（浮いた片はいつもみな選ばれている）
    pub(crate) fn with_floating<R>(&mut self, f: impl FnOnce(&mut Floating, Stage<'_>) -> R) -> R {
        let mut fl = self.floating.take().unwrap_or_else(|| Floating::new(false));
        let res = f(&mut fl, self.stage());
        self.floating = Some(fl);
        self.sync_floating();
        self.select_floating_pieces();
        res
    }

    /// 浮いた片をみな選び、もう無い図形を選択から外す（片の番号が変わる操作のあと。浮いた片はいつもみな選ばれている）
    pub(crate) fn select_floating_pieces(&mut self) {
        if let Some(fl) = &self.floating {
            self.selection.shapes.extend(fl.piece_ids());
        }
        self.retain_existing_selection();
    }

    /// 現在の `self.floating` の状態から `self.overlay` を再構築する（ドラッグ中でない通常状態の表示用）
    fn rebuild_overlay_from_floating(&mut self) {
        let had_overlay = !self.overlay.is_empty() || self.overlay_only_selected;
        self.overlay.clear();
        self.overlay_only_selected = false;
        self.overlay_movable_start = 0;
        if let Some(fl) = &self.floating {
            let scope = self.editing_group_path.clone();
            self.overlay.hide = fl.hidden().clone();
            self.overlay
                .add
                .extend(fl.placed_remainder().iter().cloned());
            self.overlay_movable_start = self.overlay.add.len();
            for piece in fl.pieces() {
                self.overlay.add.push((
                    Placement::TopOfLayer(piece.layer_id, scope.clone()),
                    piece.clone(),
                ));
            }
            self.invalidate_scene_order();
        } else if had_overlay {
            self.invalidate_scene_order();
        }
    }

    /// ドラッグ用の仮表示 `Overlay` を解除し、`Floating` のみの上乗せ状態に戻す
    #[inline]
    pub(crate) fn clear_overlay(&mut self) {
        self.rebuild_overlay_from_floating();
    }

    /// 輪郭の変形の仮表示を、確定と同じ計算の結果（消す元の図形 `removed`、新しく入れる塗りの形 `added`）にする。
    /// 新しい形は、消した図形のうち最初のものの位置に置く（生の図形どうしは重ならないので、並びは見た目に出ない）
    pub(crate) fn set_reshape_overlay(
        &mut self,
        removed: &[ShapeId],
        added: Vec<FillShape>,
        layer_id: LayerId,
        group_path: &[u64],
    ) {
        self.overlay.clear();
        self.overlay_only_selected = false;
        self.overlay_movable_start = 0;
        self.overlay.hide.extend(removed.iter().copied());
        let placement = match removed.first() {
            Some(&id) => Placement::ReplaceAt(id),
            None => Placement::TopOfLayer(layer_id, group_path.to_vec()),
        };
        self.overlay
            .add
            .extend(added.into_iter().map(|s| (placement.clone(), s)));
        self.invalidate_scene_order();
    }

    /// 選択移動または `Alt` 複製移動の仮表示用に、選択中図形を `Overlay` に載せる
    pub(crate) fn begin_move_overlay(&mut self, alt_duplicate: bool) {
        self.begin_selection_overlay(alt_duplicate, false);
    }

    /// 自由変形（拡大・回転）の仮表示用に、選択中図形を `Overlay` に載せ、変形元のクローン列を返す
    pub(crate) fn begin_transform_overlay(&mut self) -> Vec<FillShape> {
        self.begin_selection_overlay(false, true)
    }

    fn begin_selection_overlay(
        &mut self,
        alt_duplicate: bool,
        for_transform: bool,
    ) -> Vec<FillShape> {
        self.overlay.clear();
        self.overlay_only_selected = alt_duplicate;
        self.overlay_movable_start = 0;
        if self.selection.is_empty() {
            self.rebuild_overlay_from_floating();
            return Vec::new();
        }
        let scope = self.editing_group_path.clone();
        let single_symbol_gid = (for_transform
            && self.selection.shapes.is_empty()
            && self.selection.instances.len() == 1)
            .then(|| *self.selection.instances.iter().next().unwrap());
        let custom_piv = self.transform_pivot;

        if let Some(fl) = &self.floating {
            self.overlay.hide = fl.hidden().clone();
            self.overlay
                .add
                .extend(fl.placed_remainder().iter().cloned());
            if alt_duplicate {
                for piece in fl.pieces() {
                    self.overlay.add.push((
                        Placement::TopOfLayer(piece.layer_id, scope.clone()),
                        piece.clone(),
                    ));
                }
            }
        }
        self.overlay_movable_start = self.overlay.add.len();

        let mut orig_shapes = Vec::new();
        for shape in self.base_cache.shapes() {
            if self
                .floating
                .as_ref()
                .is_some_and(|fl| fl.hidden().contains(&shape.shape_id))
            {
                continue;
            }
            if self.selection.contains_shape(shape) {
                if for_transform {
                    let _ = shape.fill_mesh();
                }
                let mut cloned = shape.clone();
                if let (Some(piv), Some(gid)) = (custom_piv, single_symbol_gid) {
                    if let Some(b) = cloned.symbol_instance.as_mut().filter(|b| {
                        b.instance_gid == gid && crate::vector_engine::invertible(b.xform)
                    }) {
                        b.pivot = b.xform.inverse() * piv;
                    }
                }
                if !alt_duplicate {
                    self.overlay.hide.insert(shape.shape_id);
                }
                let placement = if alt_duplicate || shape.is_raw_at_scope(&scope) {
                    Placement::TopOfLayer(shape.layer_id, scope.clone())
                } else {
                    Placement::ReplaceAt(shape.shape_id)
                };
                if for_transform {
                    orig_shapes.push(cloned.clone());
                }
                self.overlay.add.push((placement, cloned));
            }
        }
        if let Some(fl) = &self.floating {
            for piece in fl.pieces() {
                if self.selection.contains_shape(piece) {
                    if for_transform {
                        let _ = piece.fill_mesh();
                    }
                    let cloned = piece.clone();
                    if for_transform {
                        orig_shapes.push(cloned.clone());
                    }
                    self.overlay
                        .add
                        .push((Placement::TopOfLayer(piece.layer_id, scope.clone()), cloned));
                }
            }
        }
        self.invalidate_scene_order();
        orig_shapes
    }

    /// 自由変形ドラッグ中、`orig_shapes` に対してキャッシュ済みメッシュ頂点バッファを再利用して `affine` を `overlay.add` へ適用する
    pub(crate) fn update_transform_overlay(&mut self, orig_shapes: &[FillShape], affine: Affine) {
        let start = self.overlay_movable_start.min(self.overlay.add.len());
        for ((_, overlay_shape), orig_shape) in
            self.overlay.add[start..].iter_mut().zip(orig_shapes.iter())
        {
            overlay_shape.transform_from_origin(orig_shape, affine);
        }
    }

    /// 選択移動ドラッグ中、`overlay.add` 内の可動図形とカスタムピボットを `delta` だけ平行移動する
    pub(crate) fn translate_move_overlay(&mut self, delta: KVec2) {
        let start = self.overlay_movable_start.min(self.overlay.add.len());
        for (_, shape) in &mut self.overlay.add[start..] {
            shape.translate(delta);
        }
        if let Some(piv) = &mut self.transform_pivot {
            *piv += delta;
        }
    }

    /// 確定時に結合が必要な変更済み `Floating` が保持されているかを返す
    #[cfg(test)]
    #[inline]
    pub(crate) fn has_modified_floating(&self) -> bool {
        self.floating.as_ref().is_some_and(|fl| fl.modified)
    }

    /// 選択中の生の図形（隠していないセル画の図形と、浮いている残り）を片に持ち上げ、動かした扱いにする（動かす・変形・
    /// 色を変える・グループにする前）。レイヤーごとに、その面の所を選ぶ所にして切る（`Floating::cut`）。生の図形はいつも
    /// 片の地図で書くので、切れないレイヤー（地図にできない）があれば、浮いた片を元に戻して `false`（操作を断る呼ぶ側が
    /// `refusal` に記す）
    pub(crate) fn lift_selected_raw_shapes_to_floating(&mut self) -> bool {
        let by_layer = self.selected_raw_ids_by_layer(true);
        if by_layer.is_empty() && self.floating.is_none() {
            return true;
        }
        let before = self.floating.clone();
        let mut failed = false;
        self.with_floating(|fl, stage| {
            failed = !fl.cut(stage, &by_layer, None).is_empty();
            fl.modified = true;
        });
        if failed {
            self.set_floating(before);
        }
        !failed
    }

    /// 選択中の生の図形の、レイヤーごとの番号。`base` なら隠していないセル画の図形も、そうでなければ浮いている残りだけ
    fn selected_raw_ids_by_layer(&self, base: bool) -> Vec<(LayerId, HashSet<ShapeId>)> {
        let scope = &self.editing_group_path;
        let mut by_layer: Vec<(LayerId, HashSet<ShapeId>)> = Vec::new();
        let mut add = |s: &FillShape| match by_layer.iter_mut().find(|x| x.0 == s.layer_id) {
            Some(x) => {
                x.1.insert(s.shape_id);
            }
            None => by_layer.push((s.layer_id, HashSet::from([s.shape_id]))),
        };
        let hidden = self.floating.as_ref().map(|fl| fl.hidden());
        if base {
            for s in self.base_cache.shapes() {
                if !hidden.is_some_and(|h| h.contains(&s.shape_id))
                    && self.selection.contains_shape(s)
                    && s.is_raw_at_scope(scope)
                {
                    add(s);
                }
            }
        }
        if let Some(fl) = &self.floating {
            for r in fl.remainder() {
                if self.selection.shapes.contains(&r.shape_id) {
                    add(r);
                }
            }
        }
        by_layer
    }

    /// 地図 `maps`（レイヤーごと、下から順。文書の座標。面の番号が片の番号）を、それぞれ変換をかける浮いた片の組として
    /// 足す（動かした扱い。確定のときに、変換をかけて、その階層の地図に重ねる）。グループの解除はグループの地図そのもの、
    /// 新しい中身（貼り付け・Alt 複製・シンボルの分解）は中身の生の図形の地図（`clipboard_ops::put_placements`）
    pub(crate) fn add_floating_maps(
        &mut self,
        maps: Vec<(LayerId, crate::planar_map::CheckedMap, Affine)>,
    ) {
        if maps.is_empty() {
            return;
        }
        self.with_floating(|fl, _| {
            for (layer_id, map, xform) in maps {
                fl.add_map(layer_id, map, xform);
            }
        });
    }

    /// 浮いている残りの図形が選択に入ったら、片に持ち上げる（動かしていなければ、ほかの片と一緒に切り直して1つの組に
    /// する：同じ色でつながる片は1つになる）
    pub(crate) fn promote_selected_remainder_to_pieces(&mut self) {
        let by_layer = self.selected_raw_ids_by_layer(false);
        if by_layer.is_empty() {
            return;
        }
        self.with_floating(|fl, stage| {
            fl.cut(stage, &by_layer, None);
        });
    }

    /// 未変形スライスの `Floating` で選択中の片がなくなった場合は破棄する
    pub(crate) fn drop_unmodified_slice_if_unselected(&mut self) -> bool {
        if self.floating.as_ref().is_some_and(|fl| {
            !fl.modified
                && !fl
                    .pieces()
                    .iter()
                    .any(|p| self.selection.shapes.contains(&p.shape_id))
        }) {
            self.set_floating(None);
            true
        } else {
            false
        }
    }

    /// 現在選択されている図形の `Scene` インデックス列（昇順）を返す
    pub(crate) fn selected_indices(&self) -> Vec<usize> {
        if self.selection.is_empty() {
            return Vec::new();
        }
        let scene = self.scene();
        (0..scene.len())
            .filter(|&idx| scene.is_shape_selected(idx, &self.selection))
            .collect()
    }

    /// 選択の外接矩形と、自由変形の基準点（ピボット）を返す（選択が無ければ `None`）
    pub(crate) fn selection_bbox_and_pivot(&self, zoom: f32) -> Option<(KRect, Point)> {
        let scene = self.scene();
        let bbox = scene.selection_bounding_box(&self.selection, zoom)?;
        let pivot = scene.active_transform_pivot(&self.selection, self.transform_pivot, bbox);
        Some((bbox, pivot))
    }

    /// 指定 `Scene` インデックスの図形が現在選択されているかを返す
    pub(crate) fn is_selected_at(&self, scene_idx: usize) -> bool {
        self.scene().is_shape_selected(scene_idx, &self.selection)
    }

    /// 指定した `shape_id` を持つ図形が現在選択されているかを返す
    pub(crate) fn is_shape_id_selected(&self, shape_id: ShapeId) -> bool {
        self.scene()
            .index_of_shape_id(shape_id)
            .is_some_and(|idx| self.is_selected_at(idx))
    }

    fn insert_selection_keys(
        &mut self,
        keys: impl IntoIterator<Item = (Option<InstanceGid>, ShapeId)>,
    ) {
        for (inst_gid, shape_id) in keys {
            if let Some(gid) = inst_gid {
                self.selection.instances.insert(gid);
            } else {
                self.selection.shapes.insert(shape_id);
            }
        }
    }

    /// 指定した `Scene` インデックス列に対応する図形・シンボルインスタンスのみを選択状態にする
    pub(crate) fn set_selected_indices(&mut self, indices: impl IntoIterator<Item = usize>) {
        self.selection.clear();
        self.select_shape_indices(indices);
        self.drop_unmodified_slice_if_unselected();
    }

    /// 指定した `Scene` インデックス列に対応する図形・シンボルインスタンスを選択状態に追加する
    pub(crate) fn select_shape_indices(&mut self, indices: impl IntoIterator<Item = usize>) {
        let scene = self.scene();
        let keys: Vec<_> = indices
            .into_iter()
            .filter_map(|idx| {
                scene
                    .get(idx)
                    .map(|s| (s.symbol_instance.map(|b| b.instance_gid), s.shape_id))
            })
            .collect();
        self.insert_selection_keys(keys);
        self.promote_selected_remainder_to_pieces();
    }

    /// 現在の `self.selection` に対応する `Scene` 選択マスク（`Vec<bool>`）と有効な選択が1つ以上あるかを返す
    pub(crate) fn selected_shape_mask(&self) -> (Vec<bool>, bool) {
        self.scene().selected_shape_mask(&self.selection)
    }

    /// `Scene` 上に現在存在する図形・シンボルインスタンスのみを選択に残す
    pub(crate) fn retain_existing_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let all_lids: HashSet<u64> = self.scene().iter().map(|s| s.layer_id).collect();
        self.retain_selection_in_layers(&all_lids);
    }

    /// キャンバス図形更新後に、存在しなくなった `editing_group_path`・消滅した `selection`・無効化した `hover_target` を自動補正する
    pub(crate) fn sanitize_editing_scope(&mut self) {
        self.retain_existing_selection();
        if let Some(id) = self.hover_target.shape_id() {
            if self.scene().shape_by_id(id).is_none() {
                self.hover_target = HoverTarget::None;
            }
        }
        while !self.editing_group_path.is_empty() {
            if self
                .scene()
                .iter()
                .any(|s| s.is_in_editing_scope(&self.editing_group_path))
            {
                break;
            }
            self.editing_group_path.pop();
        }
    }

    /// 現在の編集スコープ内で指定レイヤーID集合 `layer_ids` に属する全図形を選択状態にする
    pub(crate) fn select_shapes_in_layers(&mut self, layer_ids: &HashSet<u64>) {
        let keys: Vec<_> = self
            .scene()
            .iter()
            .filter(|s| {
                s.is_in_editing_scope(&self.editing_group_path) && layer_ids.contains(&s.layer_id)
            })
            .map(|s| (s.symbol_instance.map(|b| b.instance_gid), s.shape_id))
            .collect();
        self.selection.clear();
        self.insert_selection_keys(keys);
        self.transform_pivot = None;
    }

    /// 指定レイヤーID集合 `layer_ids` に属する図形・シンボルインスタンスの選択のみを残し、それ以外を選択から外す
    pub(crate) fn retain_selection_in_layers(&mut self, layer_ids: &HashSet<u64>) {
        if self.selection.is_empty() {
            return;
        }
        let (valid_shapes, valid_instances) = {
            let scene = self.scene();
            let valid_shapes: HashSet<ShapeId> = scene
                .iter()
                .filter(|s| s.symbol_instance.is_none() && layer_ids.contains(&s.layer_id))
                .map(|s| s.shape_id)
                .collect();
            let valid_instances: HashSet<InstanceGid> = scene
                .iter()
                .filter(|s| layer_ids.contains(&s.layer_id))
                .filter_map(|s| s.symbol_instance.map(|b| b.instance_gid))
                .collect();
            (valid_shapes, valid_instances)
        };
        if !self.selection.shapes.is_empty() {
            self.selection.shapes.retain(|id| valid_shapes.contains(id));
        }
        if !self.selection.instances.is_empty() {
            self.selection
                .instances
                .retain(|gid| valid_instances.contains(gid));
        }
        if self.selection.is_empty() {
            self.transform_pivot = None;
        }
    }

    /// 指定した `instance_gid` に属するキャンバス上の図形をすべて選択状態にする
    pub(crate) fn select_symbol_instance_by_gid(&mut self, instance_gid: u64) {
        self.selection.clear();
        if self.base_cache.shapes().iter().any(|s| {
            s.symbol_instance
                .is_some_and(|b| b.instance_gid == instance_gid)
        }) {
            self.selection.instances.insert(instance_gid);
        }
        self.transform_pivot = None;
    }

    /// 選択ツールまたは自由変形ツールで進行中だった図形変形・移動ドラッグのみを中断して仮表示 `Overlay` を破棄する（ブラシのストローク状態には干渉しない）
    pub(crate) fn rollback_shape_drag(&mut self) {
        if matches!(
            self.drag,
            ActiveCanvasDrag::None | ActiveCanvasDrag::PaintStroke { .. }
        ) {
            return;
        }
        match std::mem::replace(&mut self.drag, ActiveCanvasDrag::None) {
            ActiveCanvasDrag::MoveSelection(drag) => {
                if let Some(piv) = &mut self.transform_pivot {
                    *piv -= drag.total_delta;
                }
            }
            ActiveCanvasDrag::Transform(
                TransformDragState::Scale {
                    orig_pivot,
                    had_custom_pivot,
                    ..
                }
                | TransformDragState::MovePivot {
                    orig_pivot,
                    had_custom_pivot,
                    ..
                },
            ) => {
                self.transform_pivot = had_custom_pivot.then_some(orig_pivot);
            }
            ActiveCanvasDrag::Select(_)
            | ActiveCanvasDrag::Transform(TransformDragState::Rotate { .. })
            | ActiveCanvasDrag::Marquee(_)
            | ActiveCanvasDrag::PaintStroke { .. }
            | ActiveCanvasDrag::None => {}
        }
        self.clear_overlay();
    }
}

// ============================================================================
// 4. Undo / Redo 履歴管理（UndoEntry / UndoHistory）& アプリUI状態（AppUiState）
// ============================================================================

#[derive(Clone, Debug)]
pub(crate) struct UndoEntry {
    pub(crate) timeline: Timeline,
    pub(crate) library: Library,
    pub(crate) floating: Option<Arc<Floating>>,
    pub(crate) edit_path: EditPath,
    pub(crate) editing_group_path: Vec<u64>,
    pub(crate) stage_size: Vec2,
    pub(crate) fps: u32,
    pub(crate) symbols_revision: u64,
    /// この履歴スナップショットが表すドキュメント状態世代ID（未保存変更 `is_dirty` 判定用）
    pub(crate) commit_id: u64,
}

/// 連続操作（ドラッグ・キーリピート・連続タイピング）を1つの Undo エントリに合流させる識別キー
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CoalesceKey {
    StageSize,
    Fps,
    SymbolPlayback(u64),
    SymbolColorEffect(u64),
    RenameSymbol(u64),
    NudgeSelection,
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
    /// 直前の変更操作の `CoalesceKey`（同一キーが連続した場合は新規 Undo エントリを積まずに合流する）
    pub(crate) last_coalesce_key: Option<CoalesceKey>,
    /// `apply` 実行中に保持される変更前スナップショットと合流キー（`apply` の末尾でのみ確定または破棄される）
    pub(crate) pending: Option<(UndoEntry, Option<CoalesceKey>)>,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: Vec::new(),
            current_commit_id: 0,
            saved_commit_id: Some(0),
            next_commit_id: 1,
            last_coalesce_key: None,
            pending: None,
        }
    }
}

impl UndoHistory {
    /// 連続変更の Undo 合流セッションを終了する
    #[inline]
    pub(crate) fn end_coalesce(&mut self) {
        self.last_coalesce_key = None;
    }

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

    /// `Floating` が直近の履歴エントリから未変更であれば `Arc` を $O(1)$ で共有し、
    /// 変更されている場合は一時キャッシュを除去した新しい `Arc<Floating>` を返す
    pub(crate) fn snapshot_floating(&self, floating: Option<&Floating>) -> Option<Arc<Floating>> {
        let fl = floating?;
        let prev_entry = self.undo_stack.back().or_else(|| self.redo_stack.last());
        if let Some(prev_fl) = prev_entry
            .and_then(|e| e.floating.as_ref())
            .filter(|prev| prev.matches_undo_snapshot(fl))
        {
            return Some(Arc::clone(prev_fl));
        }
        let mut stripped = fl.clone();
        stripped.strip_transient_caches();
        Some(Arc::new(stripped))
    }

    /// 新しい Undo エントリをスタックへ追加し、Redo スタックをクリアして現在の状態世代IDを進める
    pub(crate) fn push(&mut self, mut entry: UndoEntry) {
        entry.commit_id = self.current_commit_id;
        self.current_commit_id = self.next_commit_id;
        self.next_commit_id = self.next_commit_id.saturating_add(1);
        self.push_undo_from_redo(entry);
        self.redo_stack.clear();
    }

    /// Undo スタック末尾から直前のスナップショットを取り出す
    pub(crate) fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.last_coalesce_key = None;
        self.undo_stack.pop_back()
    }

    /// Redo スタック末尾から次のスナップショットを取り出す
    pub(crate) fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.last_coalesce_key = None;
        self.redo_stack.pop()
    }

    /// Undo 実行前の現在状態を Redo スタックへ退避する
    pub(crate) fn push_redo(&mut self, entry: UndoEntry) {
        self.redo_stack.push(entry);
    }

    /// Redo 実行前の現在状態を Undo スタックへ戻す（Redo スタックは消去しない）
    pub(crate) fn push_undo_from_redo(&mut self, entry: UndoEntry) {
        if self.undo_stack.len() >= MAX_UNDO_HISTORY {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(entry);
    }
}

/// アプリ全体で共有するツール選択・ブラシ/消しゴム設定
#[derive(Clone)]
pub(crate) struct ToolSettings {
    pub(crate) tool: Tool,
    pub(crate) select_sub_mode: SelectSubMode,
    pub(crate) brush_radius: f32,
    pub(crate) eraser_radius: f32,
    pub(crate) brush_color: Color32,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            tool: Tool::Brush,
            select_sub_mode: SelectSubMode::Reshape,
            brush_radius: 8.0,
            eraser_radius: 22.0,
            brush_color: Color32::from_rgb(40, 45, 55),
        }
    }
}

/// キャンバス左上に一定時間自動で消えるトースト通知メッセージを登録する
pub(crate) fn show_notice(
    toast: &mut Option<StatusToast>,
    kind: NoticeKind,
    message: impl Into<String>,
) {
    *toast = Some(StatusToast {
        kind,
        message: message.into(),
        expires_at: Instant::now() + DEFAULT_TOAST_DURATION,
    });
}

// ============================================================================
// 5. タブ（1つの開いているファイル/ドキュメント編集セッション：Tab）
// ============================================================================

/// 1つの開いているタブ（保存対象の `Document`・描画および当たり判定用の読み取り専用 `Scene`・操作中の `Session`・タブごとの表示設定 `View`・Undo履歴 `UndoHistory`）
pub(crate) struct Tab {
    /// タブの一意な識別ID（タブ間クリップボード・シンボル移植の同一タブ判定用）
    pub(crate) id: u64,
    /// 未保存タブ時の連番（`Untitled 1`, `Untitled 2`, ...）
    pub(crate) untitled_number: usize,
    /// 現在開いている・保存先の `.is2d` プロジェクトファイルパス（未保存の新規ドキュメント時は `None`）
    pub(crate) file: Option<PathBuf>,
    /// 保存対象のデータ（ルートタイムライン・シンボルライブラリ・ステージサイズ・採番カウンタ）
    pub(crate) doc: Document,
    /// 今フレームの作業図形・選択・変形・排他ドラッグ・編集スタック
    pub(crate) session: Session,
    /// タブごとのキャンバス表示設定（ズーム・パン・枠外クリップ・ワイヤーフレーム・オニオンスキン・選択シンボル）
    pub(crate) view: View,
    /// Undo / Redo 履歴スタック
    pub(crate) history: UndoHistory,
}

impl Default for Tab {
    fn default() -> Self {
        Self::new_empty(1, 1)
    }
}

impl Tab {
    /// 指定された `id` と `untitled_number` で空の初期タブを生成する
    pub(crate) fn new_empty(id: u64, untitled_number: usize) -> Self {
        let mut tab = Self {
            id,
            untitled_number,
            file: None,
            doc: Document::default(),
            session: Session::default(),
            view: View::default(),
            history: UndoHistory::default(),
        };
        tab.refresh_scene_base();
        tab
    }

    /// 現在のフレームと編集位置で解決された読み取り専用の図形ビューを返す
    #[inline]
    pub(crate) fn scene(&self) -> Scene<'_> {
        self.session.scene()
    }

    /// 編集中のシンボルのライブラリでの位置を返す（ルートを編集中なら `None`）
    fn active_symbol_index(&self) -> Option<usize> {
        let last = self.session.edit_path.last()?;
        self.doc.library.iter().position(|s| s.id == last.symbol_id)
    }

    /// `Document` の現在フレームおよび `edit_path` から `Scene` のレイヤー別土台キャッシュを再構築する
    pub(crate) fn refresh_scene_base(&mut self) {
        let world_xform = self.session.edit_path.world_xform();
        let tl = match self.active_symbol_index() {
            Some(idx) => &self.doc.library[idx].timeline,
            None => &self.doc.root,
        };
        if self
            .session
            .base_cache
            .rebuild(tl, &self.doc.library, world_xform)
        {
            self.session.invalidate_scene_order();
        }
    }

    /// 現在編集中のタイムラインへの不変参照を返す
    #[inline]
    pub(crate) fn active_timeline(&self) -> &Timeline {
        match self.active_symbol_index() {
            Some(idx) => &self.doc.library[idx].timeline,
            None => &self.doc.root,
        }
    }

    #[inline]
    fn active_timeline_raw_mut(&mut self) -> &mut Timeline {
        match self.active_symbol_index() {
            Some(idx) => &mut Arc::make_mut(&mut self.doc.library[idx]).timeline,
            None => &mut self.doc.root,
        }
    }

    /// 編集中タイムラインの再生ヘッド（`current_frame`）のみを変更し、リビジョンは上げずに土台キャッシュを更新する
    pub(crate) fn set_active_frame(&mut self, frame: usize) {
        self.active_timeline_raw_mut().current_frame = frame;
        self.refresh_scene_base();
    }

    /// 再生中のフレーム進行用に、土台キャッシュを再構築せず編集中タイムラインの再生ヘッド（`current_frame`）のみを進める
    pub(crate) fn advance_playback_frame(&mut self, frame: usize) {
        self.active_timeline_raw_mut().current_frame = frame;
    }

    /// 編集中タイムラインの選択レイヤー（`active_layer`）のみを変更し、リビジョンは上げない
    pub(crate) fn set_active_layer(&mut self, idx: usize) {
        let tl = self.active_timeline_raw_mut();
        if !tl.layers.is_empty() {
            tl.active_layer = idx.min(tl.layers.len() - 1);
        }
    }

    /// 編集中タイムラインのセル範囲選択（`cell_selection`）のみを変更し、リビジョンは上げない
    pub(crate) fn set_active_cell_selection(&mut self, sel: Option<TimelineCellSelection>) {
        self.active_timeline_raw_mut().cell_selection = sel;
    }

    /// 現在編集中のタイムラインへの可変参照と文書への可変参照を渡して絵を変える編集を実行し、リビジョンを上げる
    pub(crate) fn with_active_timeline_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Timeline, &mut Document) -> R,
    ) -> R {
        if let Some(idx) = self.active_symbol_index() {
            let mut taken_tl =
                std::mem::take(&mut Arc::make_mut(&mut self.doc.library[idx]).timeline);
            let res = f(&mut taken_tl, &mut self.doc);
            let sym = Arc::make_mut(&mut self.doc.library[idx]);
            sym.timeline = taken_tl;
            sym.revision = crate::vector_engine::next_mesh_revision();
            self.doc.bump_revision();
            self.refresh_scene_base();
            return res;
        }
        let mut taken_root = std::mem::take(&mut self.doc.root);
        let res = f(&mut taken_root, &mut self.doc);
        self.doc.root = taken_root;
        self.refresh_scene_base();
        res
    }

    /// シンボルインプレイス編集中の親シーン背景薄表示メッシュをレイヤー単位キャッシュ経由で返す
    pub(crate) fn cached_backdrop_screen_mesh(
        &self,
        zoom: f32,
        pan: Vec2,
    ) -> Option<Arc<egui::Mesh>> {
        self.session.backdrop_cache.lock().cached_screen_mesh(
            &self.doc,
            &self.session.edit_path,
            zoom,
            pan,
        )
    }

    /// オニオンスキンの結合メッシュキャッシュを破棄する
    pub(crate) fn clear_onion_skin_cache(&self) {
        self.session.onion_cache.lock().clear();
    }

    /// オニオンスキンで描くセル画（レイヤー `layer_idx`・コマ `frame`）の画面座標の結合メッシュ（写しから：`CelMeshCache`）
    pub(crate) fn cached_onion_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        (tint_rgb, alpha): ([u8; 3], u8),
    ) -> Option<Arc<egui::Mesh>> {
        self.cel_screen_mesh(
            &self.session.onion_cache,
            layer_idx,
            frame,
            (zoom, pan),
            CelTint::Onion(tint_rgb, alpha),
        )
    }

    /// ロック中・再生中のレイヤー `layer_idx` のコマ `frame` のセル画の画面座標の結合メッシュ（写しから：`CelMeshCache`）
    pub(crate) fn cached_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        tint: CelTint,
    ) -> Option<Arc<egui::Mesh>> {
        self.cel_screen_mesh(&self.session.cel_cache, layer_idx, frame, (zoom, pan), tint)
    }

    fn cel_screen_mesh(
        &self,
        cache: &SyncCache<CelMeshCache>,
        layer_idx: usize,
        frame: usize,
        (zoom, pan): (f32, Vec2),
        tint: CelTint,
    ) -> Option<Arc<egui::Mesh>> {
        let tl = self.active_timeline();
        let world_xform = self.session.edit_path.world_xform();
        self.with_symbol_eval(|ctx| {
            cache
                .lock()
                .screen_mesh(tl, ctx, layer_idx, frame, world_xform, zoom, pan, tint)
        })
    }

    /// 1回の描画の始めに呼ぶ：シンボルの中身のメモ（`SymbolEvalState::start_pass`）と、セル画の結合メッシュの写し
    /// （`CelMeshCache::start_pass`）の、直前の描画で使わなかったものを捨てる
    pub(crate) fn start_draw_pass(&self) {
        self.session
            .symbol_memo
            .lock()
            .start_pass(&self.doc.library);
        self.session.onion_cache.lock().start_pass();
        self.session.cel_cache.lock().start_pass();
    }

    /// 描くたびのシンボルの評価を、セッションのメモ（`Session::symbol_memo`）を使って行う
    pub(crate) fn with_symbol_eval<R>(&self, f: impl FnOnce(SymbolEvalCtx<'_>) -> R) -> R {
        let state = std::mem::take(&mut *self.session.symbol_memo.lock());
        let ctx = SymbolEvalCtx::with_state(&self.doc.library, state);
        let out = f(ctx.clone());
        *self.session.symbol_memo.lock() = ctx.into_state();
        out
    }

    /// ダーティマーク（` *`）を除いたタブのベース表示名を返す
    pub(crate) fn base_title(&self) -> String {
        match self
            .file
            .as_deref()
            .and_then(std::path::Path::file_name)
            .and_then(|n| n.to_str())
        {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => format!("無題-{}", self.untitled_number.max(1)),
        }
    }

    /// タブバーやタイトルバーに表示するタイトル文字列（未保存変更がある場合は末尾に ` *` 付き）を返す
    pub(crate) fn display_title(&self) -> String {
        let base = self.base_title();
        if self.history.is_dirty() {
            format!("{base} *")
        } else {
            base
        }
    }

    /// 現在編集中コンテキスト（インプレース編集経路考慮）に指定シンボルを配置可能か（循環参照しないか）判定する
    #[inline]
    pub(crate) fn can_place_symbol_in_current_context(&self, symbol_id: u64) -> bool {
        self.doc
            .can_place_symbol_in_current_context(&self.session.edit_path, symbol_id)
    }

    /// ライブラリ選択中のシンボルIDが存在しない場合に先頭シンボルへ補正する
    pub(crate) fn ensure_valid_library_selection(&mut self) {
        self.view.ensure_valid_library_selection(&self.doc.library);
    }

    /// 浮いている片（`Floating`）がある場合は一時タブ上で確定した `Document` の写しを返し、元のタブや文書には触れない。
    /// `None`：浮いた片を置けない（`commit_floating_layers` が断る。文書は、片を持ち上げる前のまま）
    pub(crate) fn committed_document_copy(&self) -> Option<Document> {
        if let Some(fl) = self.session.floating().filter(|fl| fl.modified) {
            let mut temp_tab = Self::new_empty(self.id, self.untitled_number);
            temp_tab.doc = self.doc.clone();
            temp_tab.session.edit_path = self.session.edit_path.clone();
            temp_tab.session.editing_group_path = self.session.editing_group_path.clone();
            temp_tab.session.set_floating(Some(fl.clone()));
            temp_tab.refresh_scene_base();
            commit_floating(&mut temp_tab).then_some(temp_tab.doc)
        } else {
            Some(self.doc.clone())
        }
    }

    pub(crate) fn restore_state_snapshot(&mut self, entry: UndoEntry) -> UndoEntry {
        self.history.last_coalesce_key = None;
        let prev_selection = self.session.selection.clone();
        let prev_floating = self.history.snapshot_floating(self.session.floating());
        let prev_entry = UndoEntry {
            timeline: std::mem::replace(&mut self.doc.root, entry.timeline),
            library: self.doc.library.clone(),
            floating: prev_floating,
            edit_path: std::mem::replace(&mut self.session.edit_path, entry.edit_path),
            editing_group_path: std::mem::replace(
                &mut self.session.editing_group_path,
                entry.editing_group_path,
            ),
            stage_size: std::mem::replace(&mut self.doc.stage_size, entry.stage_size),
            fps: std::mem::replace(&mut self.doc.fps, entry.fps),
            symbols_revision: self.doc.revision,
            commit_id: std::mem::replace(&mut self.history.current_commit_id, entry.commit_id),
        };
        self.doc.library = entry.library;
        self.doc.revision = entry.symbols_revision;
        self.doc.validate_edit_path(
            &mut self.session.edit_path,
            &mut self.session.editing_group_path,
        );
        self.ensure_valid_library_selection();
        cancel_active_drag(self);
        self.session.transform_pivot = None;
        self.refresh_scene_base();
        self.session
            .set_floating(entry.floating.map(Arc::unwrap_or_clone));
        self.session.selection = prev_selection;
        self.session.retain_existing_selection();
        if self.session.floating().is_some() {
            // 確定を断ったら、浮いた片と選択はそのまま（知らせは `apply`）
            let keep = self.session.selected_indices();
            let _ = commit_floating_except(self, &keep);
        }
        self.session.sanitize_editing_scope();
        prev_entry
    }

    pub(crate) fn undo(&mut self) -> bool {
        cancel_active_drag(self);
        if let Some(prev_entry) = self.history.pop_undo() {
            let curr = self.restore_state_snapshot(prev_entry);
            self.history.push_redo(curr);
            true
        } else {
            false
        }
    }

    pub(crate) fn redo(&mut self) -> bool {
        cancel_active_drag(self);
        if let Some(next_entry) = self.history.pop_redo() {
            let curr = self.restore_state_snapshot(next_entry);
            self.history.push_undo_from_redo(curr);
            true
        } else {
            false
        }
    }
}
