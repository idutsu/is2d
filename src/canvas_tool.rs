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
    /// 書き直す知らせの見分け（同じ見分けの知らせは、足さずに文を書き直す：読み込みの知らせを「ほか N か所」にまとめる）
    key: Option<NoticeKey>,
}

/// 書き直す知らせの見分け
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeKey {
    /// ファイルから読んだものの知らせ（地図の切り直し・何も無いものとして見せている地図・読めない音・短い音）
    Load,
}

/// 画面の左上に出す知らせの並び（足すのは `show_notice` の1つ。同じ文は1つにまとめて出ている時間を延ばし、同じ見分けの
/// 知らせは文を書き直す。消えるのは出ている時間が過ぎたときだけ：設計審査 b7208d9 の 4.。前は箱が1枠で、あとの知らせが
/// 前の知らせを上書きした）
#[derive(Clone, Debug, Default)]
pub(crate) struct Notices {
    items: Vec<StatusToast>,
}

impl Notices {
    /// 出ている知らせ（出した順）
    pub(crate) fn iter(&self) -> impl Iterator<Item = &StatusToast> {
        self.items.iter()
    }

    /// 見分け `key` の知らせが出ているか
    pub(crate) fn has(&self, key: NoticeKey) -> bool {
        self.items.iter().any(|t| t.key == Some(key))
    }

    /// 出ている時間が `now` までに過ぎた知らせを消す
    pub(crate) fn drop_expired(&mut self, now: Instant) {
        self.items.retain(|t| now < t.expires_at);
    }

    /// 知らせを足す（同じ見分けの知らせがあれば文を書き直し、同じ文があれば1つにまとめる。どちらも出ている時間を延ばす）
    fn push(&mut self, kind: NoticeKind, message: String, key: Option<NoticeKey>) {
        let expires_at = Instant::now() + DEFAULT_TOAST_DURATION;
        self.items.retain(|t| !(key.is_some() && t.key == key) && t.message != message);
        self.items.push(StatusToast { kind, message, expires_at, key });
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    Select,     // V キー
    Transform,  // Q キー（自由変形ツール）
    Brush,      // B キー
    Pencil,     // S キー（鉛筆：線だけを引く）
    Eraser,     // E キー
    Bucket,     // K キー
    InkBottle,  // O キー（インクボトル：面の輪郭や線に、線を乗せる）
    Eyedropper, // I キー（スポイトツール）
}

impl Tool {
    /// 全ツールバリアントの標準順序
    pub(crate) const ALL: [Tool; 8] = [
        Tool::Select,
        Tool::Transform,
        Tool::Brush,
        Tool::Pencil,
        Tool::Eraser,
        Tool::Bucket,
        Tool::InkBottle,
        Tool::Eyedropper,
    ];
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
    AddAnchor,    // ＋ アンカー追加モード（D キー）
    DeleteAnchor, // － アンカー削除モード（F キー）
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
        /// 地図の頂点（どの地図か, 文書の座標の位置）。`pos` はステージの座標の位置（表示と距離）、編集（`ReshapeEdit`・
        /// `AnchorEdit`）は文書の座標の位置で頂点を探す
        map_node: Option<(MapOwner, Point)>,
    },
    Edge {
        shape_id: ShapeId,
        t: f64,
        closest_pos: Point,
        /// 強調表示する曲線（地図の辺のステージの座標の形）
        seg: QuadBez,
        /// 地図の辺（どの地図か, 文書の座標の形）。編集（`ReshapeEdit`・`AnchorEdit`）はこの形で辺を探す
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
        /// まわりの辺もついてくる引っぱりか（`tool_select::drag_follows`。ドラッグを始めたときに決める）
        follow: bool,
    },
    /// 地図の `orig_vertex` にある頂点を `target` へ動かす（位置は地図の文書の座標）
    MoveVertex {
        layer_id: LayerId,
        group_path: Vec<u64>,
        base: Arc<LayerBase>,
        orig_vertex: Point,
        target: Point,
        /// まわりがついてくる引っぱりか（`tool_select::drag_follows`・`ReshapeEdit::vertex`。ドラッグを始めたときに決める）
        follow: bool,
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

    pub(in crate::canvas_tool) fn opposite(self) -> Self {
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
        orig_bbox: KRect,
        orig_pivot: Point,
        had_custom_pivot: bool,
        orig_shapes: Vec<FillShape>,
        last_affine: Affine,
        changed: bool,
        /// 置けるかを決めるもの（始めるときに1回だけ作る：`Session::transform_fit`）
        fit: Arc<TransformFit>,
        /// いまの変換で置けるか（確定と同じ判定：`TransformFit::fits`。置けなければ知らせを出す）
        fits: bool,
    },
    Rotate {
        orig_bbox: KRect,
        pivot: Point,
        had_custom_pivot: bool,
        start_angle: f64,
        current_angle: f64,
        orig_shapes: Vec<FillShape>,
        /// 置けるかを決めるものと、いまの変換で置けるか（`Scale` と同じ）
        fit: Arc<TransformFit>,
        fits: bool,
    },
}

/// 選んでいるものに変換をかけて置けるかを決めるもの（`Session::transform_fit`）：確定のときに変換をかける地図と、その地図に
/// 先にかかっている変換、ステージの座標から文書の座標への変換
#[derive(Debug)]
pub(crate) struct TransformFit {
    world: Affine,
    maps: Vec<(crate::floating::FitMap, Affine)>,
}

impl TransformFit {
    /// ステージの座標の変換 `affine` をかけて置けるか。変形の確定（`transform_selected_shapes`）と、拡大・回転のドラッグ中の
    /// 「置けない」の表示（`TransformDragState` の `fits`）の判定はこれ1つ：変換が潰れず（`invertible`）、地図にその変換を
    /// 重ねた点がみな格子に置ける（`PlanarMap::fits_grid`。確定と同じ式）
    pub(crate) fn fits(&self, affine: Affine) -> bool {
        let doc = self.world.inverse() * affine * self.world;
        crate::vector_engine::invertible(doc)
            && self
                .maps
                .iter()
                .all(|(m, pre)| m.map().fits_grid(doc * *pre))
    }
}

impl ActiveCanvasDrag {
    /// 拡大・回転のドラッグで、いまの変換では置けない（離すと断る：`Refusal::OffGrid`）
    pub(crate) fn unplaceable(&self) -> bool {
        matches!(
            self,
            ActiveCanvasDrag::Transform(
                TransformDragState::Scale { fits: false, .. }
                    | TransformDragState::Rotate { fits: false, .. }
            )
        )
    }
}

/// キャンバス上で進行中のドラッグ操作を表す排他的状態マシン
/// （ブラシ/消しゴムのストローク、選択ツールのドラッグ、自由変形ツールのドラッグが同時に複数存在できないことを型で保証する）
#[derive(Clone, Debug, Default)]
pub(crate) enum ActiveCanvasDrag {
    #[default]
    None,
    /// ブラシ・消しゴムのストローク。`stroke` は受け取った点と中心線、`committed_world_mesh` は確定した中心線を
    /// 太らせた形（文書の座標。確定するたびに足す）、`tail_mesh` は確定していない所を太らせた形（点が来るたびに作り直す）
    PaintStroke {
        stroke: crate::vector_stroke::CenterlineBuilder,
        committed_world_mesh: egui::Mesh,
        tail_mesh: egui::Mesh,
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
    /// キャンバスのズーム倍率（1.0 = 100%）。いつも `MIN_VIEW_ZOOM..=MAX_VIEW_ZOOM` の中（書くのは `set_zoom` だけ
    /// なので、読む所は範囲に収め直さない）
    zoom: f32,
    /// 画面の倍率（論理の1点あたりの画素の数。Retina なら 2）。ブラシ・消しゴムで、マウスの点の丸めの刻みを決める
    /// （`vector_stroke::input_step`）。ブラシ・消しゴムを使うたびに画面から取る（egui の値で、いつも正。初めは 1）
    pub(crate) pixels_per_point: f32,
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
    /// ライブラリパネルで選んだシンボルの番号（無い番号のこともある。見せるのは `library_symbol` が決める）
    selected_library_symbol: Option<u64>,
    /// ライブラリのシンボルのミニプレビューの写し
    pub(crate) symbol_previews: crate::symbol::SymbolPreviews,
    /// キャンバスでの前の主ボタンのクリック（画面の位置と時刻）。ダブルクリックを位置も比べて決めるため（`double_clicked`）
    pub(crate) last_click: Option<(Pos2, f64)>,
    /// まだキャンバスの大きさに合わせていない（新しく作った・開いた作品は、初めて画面に出すときに、ステージ全体が
    /// キャンバスに収まるようズーム・パンを合わせる：`fit_stage_to_window`）
    pub(crate) fit_pending: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pixels_per_point: 1.0,
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
            fit_pending: true,
        }
    }
}

/// 表示メニューで切り替える、タブごとの表示のオン・オフ（`View::switch`）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewSwitch {
    /// 枠外を隠す（`View::clip_outside_stage`）
    ClipOutsideStage,
    /// アンカーを表示（`View::show_wireframe`）
    Anchors,
    /// オニオンスキン（`View::onion_skin_enabled`）
    OnionSkin,
}

/// 画面で動いたとみなす最小の量（px）。ドラッグで「変わったか」はみなこれで決める（`View::moves_on_screen`）
pub(crate) const MIN_DRAG_SCREEN_PX: f64 = 0.5;

impl View {
    /// ステージの座標で長さ `len` の動きが、画面で動いたとみなす量（`MIN_DRAG_SCREEN_PX`）を超えるか。ドラッグで
    /// 「変わったか」はみなこれで決める（移動・ピボットの移動・輪郭の変形・拡大・回転。選択の大きさとズームによらない）
    pub(crate) fn moves_on_screen(&self, len: f64) -> bool {
        len * self.zoom as f64 > MIN_DRAG_SCREEN_PX
    }

    /// ライブラリ `library` で選んでいるシンボル：選んだシンボルがライブラリにあればそれ、無ければ（選んでいない・消した・
    /// 取り消しで無くなった）先頭。決める所はここだけ（前は消す・取り消す・入れ子の戻し・パネルの描画の4か所で直していた：
    /// 約束の棚卸し 5616fcc の A の P-19）
    pub(crate) fn library_symbol(&self, library: &Library) -> Option<u64> {
        self.selected_library_symbol
            .filter(|&id| library.get(id).is_some())
            .or_else(|| library.at(0).map(|s| s.id))
    }

    /// ライブラリでシンボル `id` を選ぶ
    pub(crate) fn select_library_symbol(&mut self, id: u64) {
        self.selected_library_symbol = Some(id);
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

    /// ズーム倍率（いつも `MIN_VIEW_ZOOM..=MAX_VIEW_ZOOM` の中）
    pub(crate) fn zoom(&self) -> f32 {
        self.zoom
    }

    /// ズーム倍率を `zoom` にする（`MIN_VIEW_ZOOM..=MAX_VIEW_ZOOM` に収める。有限でない倍率は使わない）。倍率を書くのは
    /// これ1つ
    pub(in crate::canvas_tool) fn set_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.zoom = zoom.clamp(
                crate::vector_engine::MIN_VIEW_ZOOM,
                crate::vector_engine::MAX_VIEW_ZOOM,
            );
        }
    }

    /// スクリーン上の指定位置 `anchor_screen`（マウスカーソル位置）がズーム前後で動かないように倍率 `factor` を掛ける
    pub(crate) fn zoom_around_screen_pos(&mut self, anchor_screen: Pos2, factor: f32) {
        let world_before = self.screen_to_world(anchor_screen);
        self.set_zoom(self.zoom * factor);
        self.pan = Vec2::new(
            anchor_screen.x - (world_before.x as f32) * self.zoom,
            anchor_screen.y - (world_before.y as f32) * self.zoom,
        );
    }

    /// ズーム倍率を 100%（`1.0`）、パン位置を原点（`Vec2::ZERO`）へリセットする（`Cmd+1` / `Cmd+0`）
    pub(crate) fn reset_view(&mut self) {
        self.set_zoom(1.0);
        self.pan = Vec2::ZERO;
    }

    /// 表示のオン・オフ `switch` の今の状態
    pub(crate) fn switch(&self, switch: ViewSwitch) -> bool {
        match switch {
            ViewSwitch::ClipOutsideStage => self.clip_outside_stage,
            ViewSwitch::Anchors => self.show_wireframe,
            ViewSwitch::OnionSkin => self.onion_skin_enabled,
        }
    }

    /// 表示のオン・オフ `switch` を切り替える
    pub(crate) fn toggle(&mut self, switch: ViewSwitch) {
        let flag = match switch {
            ViewSwitch::ClipOutsideStage => &mut self.clip_outside_stage,
            ViewSwitch::Anchors => &mut self.show_wireframe,
            ViewSwitch::OnionSkin => &mut self.onion_skin_enabled,
        };
        *flag = !*flag;
    }

    /// ステージ枠全体が中央キャンバス領域 `canvas_rect` に余白付きでぴったり収まるようにズーム倍率とパン位置を自動調整する（「ウィンドウに合わせる」）
    /// （ステージの大きさ `stage_size` は文書の大きさ。範囲は `symbol::stage_size_of` でそろえてある）
    pub(crate) fn fit_stage_to_window(&mut self, stage_size: Vec2, canvas_rect: egui::Rect) {
        let avail_w = (canvas_rect.width() - 48.0).max(64.0);
        let avail_h = (canvas_rect.height() - 48.0).max(64.0);
        self.set_zoom((avail_w / stage_size.x).min(avail_h / stage_size.y));
        let zoom = self.zoom;
        let center = Self::DEFAULT_STAGE_CENTER;
        let screen_center = canvas_rect.center();
        self.pan = Vec2::new(
            screen_center.x - (center.x as f32) * zoom,
            screen_center.y - (center.y as f32) * zoom,
        );
    }

    /// ステージ（カメラ出力枠）の初期ワールド中心座標
    pub(crate) const DEFAULT_STAGE_CENTER: Point = Point::new(536.0, 310.0);

    /// ステージ（カメラ出力枠）のワールド座標矩形 `KRect` を返す（大きさ `stage_size` は文書の大きさ：`symbol::stage_size_of`）
    #[inline]
    pub(in crate::canvas_tool) fn stage_world_rect(&self, stage_size: Vec2) -> KRect {
        let center = Self::DEFAULT_STAGE_CENTER;
        let half_w = stage_size.x as f64 * 0.5;
        let half_h = stage_size.y as f64 * 0.5;
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
    pub(in crate::canvas_tool) fn stage_screen_rect(&self, stage_size: Vec2) -> egui::Rect {
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
            egui::StrokeKind::Middle,
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
    /// クリップボードにコピーされた時点のコピー元ドキュメントID（別タブへのペースト判定用。書くのは `set_source` だけ）
    source_doc_id: u64,
    /// コピー元のタブの番号が、もういない（閉じた・作品を入れ替えた）か（`forget_tab`・`set_source`）
    source_retired: bool,
    /// クリップボード内のシェイプが参照しているシンボル定義のスナップショット群（依存順）
    pub(crate) symbols: Vec<Arc<SymbolDef>>,
    /// `symbols` のタイムラインの音の区間が指す音（コピー元の音のライブラリの音を共有する。移植で貼り先へ持ち込む）
    pub(crate) sounds: crate::sound::SoundLibrary,
    /// 各コピー元タブから貼り付け先タブへ既にインポート済みのシンボルID対応表 `(source_doc_id, source_sym_id, target_doc_id) -> target_sym_id`。
    /// 入口 `apply` が毎回クリップボードを写すので、共有の箱に入れ、書くときだけ写す（`note_imported`。設計審査 6fb4faa の
    /// D-4）。いなくなったタブの記録は捨てる（`forget_tab`）
    imported_map: Arc<HashMap<(u64, u64, u64), u64>>,
    /// 最後に V を離してから、V を押した合図（`Key`）か貼り付けの合図（`Paste`）が来たか（`swallowed_paste`）
    pub(crate) v_press_seen: bool,
    /// `Cmd+V` による通常ペーストの連続回数（少しずつずらして配置するためのカウンタ）
    pub(crate) paste_count: usize,
    /// `Cmd+V` による通常ペーストの累積ワールド座標オフセット（途中でズーム変更しても階段状にずらす用）
    pub(crate) paste_offset: KVec2,
}

impl Clipboard {
    /// コピー元のタブの番号
    pub(crate) fn source_doc_id(&self) -> u64 {
        self.source_doc_id
    }

    /// 写した中身のコピー元をタブ `id` にする（写すたびに呼ぶ）。前のコピー元のタブがもういなければ、それをコピー元とする
    /// 記録を捨てる（もう使う中身が無い）
    pub(crate) fn set_source(&mut self, id: u64) {
        if self.source_retired {
            let old = self.source_doc_id;
            Arc::make_mut(&mut self.imported_map).retain(|&(s, _, _), _| s != old);
        }
        self.source_doc_id = id;
        self.source_retired = false;
    }

    /// 移した対応（コピー元のタブ・シンボル → 貼り先のタブ・シンボル）
    pub(crate) fn imported(&self, from_doc: u64, from_sym: u64, to_doc: u64) -> Option<u64> {
        self.imported_map.get(&(from_doc, from_sym, to_doc)).copied()
    }

    /// 移した対応を、行きと帰りの両方向で足す（書くときだけ表を写す）
    pub(crate) fn note_imported(&mut self, (from_doc, from_sym): (u64, u64), (to_doc, to_sym): (u64, u64)) {
        let map = Arc::make_mut(&mut self.imported_map);
        map.insert((from_doc, from_sym, to_doc), to_sym);
        map.insert((to_doc, to_sym, from_doc), from_sym);
    }

    /// タブの番号 `id` がいなくなった（タブを閉じた・タブに作品を入れ替えた）：そのタブを貼り先とする記録を捨てる。そのタブを
    /// コピー元とする記録は、今の中身がそのタブのものなら残し（閉じたタブから写した中身を何度貼っても、同じシンボルを使い
    /// 回す）、中身を写し直したときに捨てる（`set_source`）。そうでなければ今捨てる
    pub(crate) fn forget_tab(&mut self, id: u64) {
        let keep_source = self.source_doc_id == id;
        self.source_retired |= keep_source;
        if self.imported_map.keys().any(|&(s, _, t)| t == id || (s == id && !keep_source)) {
            Arc::make_mut(&mut self.imported_map).retain(|&(s, _, t), _| t != id && (s != id || keep_source));
        }
    }

    /// 入力の合図 `events` に、捨てられた `Cmd+V` があったか。egui-winit は `Cmd+V` の押下を、OS のクリップボードに文字が
    /// あれば貼り付けの合図（`Paste`）にし、無ければ捨てる（どちらでも、押した合図の `Key` は送らない。離した合図は送る）。
    /// なので、V を押した合図も貼り付けの合図も来ないまま V を離したら、それは捨てられた `Cmd+V`。修飾キーや時間を見ないので、
    /// ほかのキーのすぐ後にふつうに V（選択ツール）を押して離しても貼らない（レビュー f01d518 の N-5003）。OS のクリップ
    /// ボードには文字を置かない（#15）。毎コマ、ほかの判定より先に呼ぶ（文字の入力中も）
    pub(crate) fn swallowed_paste(&mut self, events: &[egui::Event]) -> bool {
        let mut swallowed = false;
        for ev in events {
            match ev {
                egui::Event::Paste(_)
                | egui::Event::Key {
                    key: Key::V,
                    pressed: true,
                    ..
                } => self.v_press_seen = true,
                egui::Event::Key {
                    key: Key::V,
                    pressed: false,
                    ..
                } => {
                    swallowed |= !self.v_press_seen;
                    self.v_press_seen = false;
                }
                _ => {}
            }
        }
        swallowed
    }
}

pub(crate) type InstanceGid = u64;

/// キャンバス上の選択状態（図形ID集合とシンボルインスタンスGID集合）
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) shapes: HashSet<ShapeId>,
    pub(crate) instances: HashSet<InstanceGid>,
    /// 選んだときに見えていた絵（項目の入っているレイヤーごと。覚えるのも、見える絵が変わったレイヤーの項目を外すのも
    /// `Tab::settle_selection` の1つ：設計審査 b7208d9 の 2.）
    anchors: std::collections::BTreeMap<u64, crate::timeline::ShownPicture>,
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
        self.anchors.clear();
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
    pub(in crate::canvas_tool) fn insert_shape(&mut self, shape: &FillShape) {
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
// 作品の中身を書く所はみなこの部屋（`canvas_tool`）の中に置く：入口・選択・シンボル・貼り付け・セル画を書く所。作品
// （`Tab::doc`）と取り消しの記録（`Tab::history`）は部屋の中だけで書け、部屋の外は読むだけ（設計：入口の部屋の設計_27c4085）
pub(crate) mod clipboard_ops;
pub(crate) mod doc_actions;
pub(crate) mod select_ops;
pub(crate) mod symbol_ops;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
pub(crate) use self::cel_writes::*;

pub(crate) use crate::vector_engine::CelTint;
pub(crate) use scene_base_cache::SceneBaseCache;
use scene_base_cache::{CelMeshCache, CelScope};

/// `Overlay` で追加する図形の重なり順指定
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Placement {
    /// 隠した元の図形（ドラッグで動かしている、グループ・インスタンスの図形そのもの）と同じ重なり順に置く。元の図形は、
    /// この上乗せを作った土台にあり、ドラッグの間は土台が変わらない（文書を変えるとドラッグは終わり、上乗せも作り直す）
    ReplaceAt(ShapeId),
    /// そのレイヤー・その階層の生の図形の地図の中（浮いた片の残り・輪郭の変形の仮表示・複製のドラッグ中の確定した絵。
    /// その地図の図形の代わりに出すもの）。並べ方は地図の描く順と同じ決まり（`planar_map::draw_rank`：塗りはその地図の
    /// 塗りのあと、線はその地図の線のあと）
    RawOf(LayerId, Scope),
    /// そのレイヤー・その階層の生の図形のすぐ上（グループ・インスタンスより下）の、組の番号（下から）ごとの置き場。浮いた片
    /// と、動かしている生の図形の置き場（生の図形はいつもグループ・インスタンスより下：Animate と同じ。確定した後と同じ
    /// 重なり。前はレイヤーのいちばん上に置いたので、選んだ生の絵がグループを隠した）。1つの組は1つの地図から来た図形で、
    /// 組の中は地図の描く順（`planar_map::draw_rank`）、組と組は下から順（確定で片の組を下から順に重ねるのと同じ）
    AboveRaw(LayerId, Scope, usize),
    /// そのレイヤー・その編集スコープの一番上に置く
    TopOfLayer(LayerId, Scope),
}

/// キャンバスの土台図形列に合成する上乗せ図形状態（ドラッグ中の仮表示や Floating）
#[derive(Clone, Debug, Default)]
pub(crate) struct Overlay {
    /// 画面から隠す元の図形
    pub(crate) hide: HashSet<ShapeId>,
    /// 追加で見せる図形（分け合える箱。複製のドラッグ中の確定した絵は、離したあとの画面の土台が同じ箱を使い回す：
    /// `SceneBaseCache::offer`）
    pub(crate) add: Vec<(Placement, Arc<FillShape>)>,
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
    base: &[Arc<FillShape>],
    overlay: &Overlay,
    layer_order_ids: &[LayerId],
) -> Vec<SceneEntry> {
    let mut before_all: Vec<(usize, usize)> = Vec::new();
    // 土台の図形のあとに足す上乗せ：（並べる鍵（置き場の順, 組の順, 描く順）, 上乗せの番号）。同じ鍵は足した順
    type OrderKey = (usize, usize, u8);
    let mut after_base: Vec<Vec<(OrderKey, usize)>> = vec![Vec::new(); base.len()];
    let mut replace_at_base: Vec<Vec<usize>> = vec![Vec::new(); base.len()];
    // 置き場の表：（引き方, レイヤー, 階層）ごとの土台の位置を、初めて引くときに土台を1回なめて控える（上乗せ1つごとに
    // 土台をなめない。上乗せの置き場の（レイヤー, 階層）は、ふつう数個。前は上乗せ1つごとに土台の並び全体をなめたので、
    // 動かし始め・浮いた片が変わるたびの1コマが、選んだ数 × コマの図形の数で重くなった：レビュー de92ceb の N-3002）
    #[derive(Clone, Copy, PartialEq)]
    enum Find {
        FirstInScope,
        LastRaw,
        LastRawFill,
        LastInScope,
        LastOfLayer,
    }
    let mut slots: Vec<((Find, LayerId, Scope), Option<usize>)> = Vec::new();
    let mut slot = |find: Find, lid: LayerId, scope: &[u64]| -> Option<usize> {
        if let Some(&(_, pos)) = slots
            .iter()
            .find(|(k, _)| k.0 == find && k.1 == lid && k.2 == scope)
        {
            return pos;
        }
        let pos = match find {
            Find::FirstInScope => base
                .iter()
                .position(|s| s.layer_id == lid && s.is_in_editing_scope(scope)),
            Find::LastRaw => base
                .iter()
                .rposition(|s| s.layer_id == lid && s.is_raw_at_scope(scope)),
            Find::LastRawFill => base.iter().rposition(|s| {
                s.layer_id == lid && s.is_raw_at_scope(scope) && crate::planar_map::draw_rank(s) == 0
            }),
            Find::LastInScope => base
                .iter()
                .rposition(|s| s.layer_id == lid && s.is_in_editing_scope(scope)),
            Find::LastOfLayer => base.iter().rposition(|s| s.layer_id == lid),
        };
        slots.push(((find, lid, scope.to_vec()), pos));
        pos
    };
    let mut index_of: Option<HashMap<ShapeId, usize>> = None;

    for (add_idx, (placement, shape)) in overlay.add.iter().enumerate() {
        // そのレイヤー・階層に土台の図形が無いとき（空のレイヤー）に、レイヤーの順番で入れる位置を決めるレイヤー
        let layer_order_fallback = match placement {
            Placement::ReplaceAt(target_id) => {
                // 元の図形は、この上乗せを作った土台にいつもある（`Placement::ReplaceAt`。レイヤーの順で入れる逃げ道は
                // 置かない）
                let pos = *index_of
                    .get_or_insert_with(|| base.iter().enumerate().map(|(i, s)| (s.shape_id, i)).collect())
                    .get(target_id)
                    .expect("ReplaceAt の相手は、上乗せを作った土台にある");
                replace_at_base[pos].push(add_idx);
                None
            }
            Placement::RawOf(lid, scope) | Placement::AboveRaw(lid, scope, _) => {
                // 地図の描く順と同じ決まり（`draw_rank`）：その地図の中のもの（`RawOf`）は、塗りはその階層の生の塗りの
                // あと、線はその階層の生の線のあと。地図のすぐ上の組（`AboveRaw`）は、生の図形のあとに、組の順、組の中は
                // 描く順。置き場の生の図形が無ければ、その階層のいちばん下
                let rank = crate::planar_map::draw_rank(shape);
                let (anchor, key) = match placement {
                    Placement::AboveRaw(_, _, g) => (slot(Find::LastRaw, *lid, scope), (1 + g, rank)),
                    _ if rank == 0 => (slot(Find::LastRawFill, *lid, scope), (0, 0)),
                    _ => (slot(Find::LastRaw, *lid, scope), (0, 1)),
                };
                match anchor {
                    Some(pos) => {
                        after_base[pos].push(((0, key.0, key.1), add_idx));
                        None
                    }
                    None => match slot(Find::FirstInScope, *lid, scope) {
                        Some(pos) => {
                            replace_at_base[pos].push(add_idx);
                            None
                        }
                        None => Some(*lid),
                    },
                }
            }
            Placement::TopOfLayer(lid, scope) => {
                let scope_pos = (!scope.is_empty())
                    .then(|| slot(Find::LastInScope, *lid, scope))
                    .flatten();
                if let Some(pos) = scope_pos {
                    after_base[pos].push(((1, 0, 0), add_idx));
                    None
                } else if let Some(pos) = slot(Find::LastOfLayer, *lid, scope) {
                    after_base[pos].push(((2, 0, 0), add_idx));
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
                after_base[pos].push(((3 + ord.min(usize::MAX - 3), 0, 0), add_idx));
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
            after_base[base_idx].sort_by_key(|&(key, _)| key);
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
    base: &'a [Arc<FillShape>],
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
            return self.base.get(idx).map(|s| &**s);
        }
        match self.order.get(idx)? {
            SceneEntry::Base(i) => self.base.get(*i).map(|s| &**s),
            SceneEntry::Overlay(j) => self.overlay.add.get(*j).map(|(_, s)| &**s),
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
    pub(in crate::canvas_tool) fn topmost_shape_at(self, pt: Point, editing_group_path: &[u64]) -> Option<usize> {
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
    pub(in crate::canvas_tool) fn expand_to_group_unit(
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
    pub(in crate::canvas_tool) fn is_shape_selected(self, scene_idx: usize, selection: &Selection) -> bool {
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
    pub(in crate::canvas_tool) fn selected_shapes<'s>(
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
    pub(in crate::canvas_tool) fn selected_has_any_in_scope(
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
    pub(in crate::canvas_tool) fn selection_bounding_box(self, selection: &Selection, zoom: f32) -> Option<KRect> {
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
    pub(in crate::canvas_tool) fn selected_single_symbol_binding(
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
    pub(in crate::canvas_tool) fn active_transform_pivot(
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

pub(crate) use crate::floating::{Floating, FloatingView};
pub(crate) use crate::floating::Stage;

/// 親の背景を描く文書の写し：（鍵, 浮いた片を確定した後の文書。確定できなければ `None`）
/// 親の背景を描く文書の写しと、その鍵（浮いた片の番号・ライブラリの版・見ている場所）
type BackdropDoc = ((u64, u64, EditPath, Vec<u64>), Option<Arc<Document>>);

/// タブごとの編集セッション状態（土台キャッシュ・上乗せ・選択・変形・排他ドラッグ・編集階層、保存しない）
#[derive(Default)]
pub(crate) struct Session {
    /// シンボルインプレイス編集の経路（空ならルート）。書くのは `Tab::move_view`（と、取り消しで状態をまるごと戻す
    /// `Tab::restore_state_snapshot`）だけ
    edit_path: EditPath,
    /// シンボル編集中の親の背景（薄く描く）のセル画の結合メッシュの写し
    pub(crate) backdrop_cache: SyncCache<CelMeshCache>,
    /// 親の背景を描く、浮いた片を確定した後の文書（鍵（`floating_rev`, `Document::revision`）と文書。確定できなければ
    /// `None`：`Tab::backdrop_document`）
    backdrop_doc: SyncCache<Option<BackdropDoc>>,
    /// 浮いた片が変わるたびに増える番号（浮いた片の書き換えは `set_floating`・`take_floating`・`update_floating`・
    /// `with_floating` だけを通り、みな `sync_floating` か `take_floating` で数える）
    pub(crate) floating_rev: u64,
    /// オニオンスキンで描くセル画の結合メッシュの写し
    pub(crate) onion_cache: SyncCache<CelMeshCache>,
    /// ロック中・再生中のレイヤーのセル画の結合メッシュの写し
    pub(crate) cel_cache: SyncCache<CelMeshCache>,
    /// 描くたびのシンボルの評価（オニオンスキン・再生・ロック中のレイヤー）で使い回す、シンボルの中身のメモ
    pub(crate) symbol_memo: SyncCache<SymbolEvalState>,
    /// `Document` から構築されるレイヤー別の読み取り専用土台キャッシュ（外からは読むだけ：`base_cache`。作り直す・捨てる
    /// のは `Session`・`Tab` の関数で、そこで並びの写しも捨てる：約束の棚卸し 5616fcc の B の A1）
    base_cache: SceneBaseCache,
    /// 土台図形列に対する上乗せ図形状態（Floating およびドラッグ中の仮表示。書くのは `Session` の関数だけ）
    overlay: Overlay,
    /// `Alt` 複製ドラッグ中など、`Overlay` 内の可動図形のみを選択表示・境界枠対象とするフラグ
    pub(crate) overlay_only_selected: bool,
    /// `overlay.add` のうちドラッグで移動・変形する図形の開始インデックス（`0..overlay_movable_start` はスライス残り等の固定背景）
    pub(crate) overlay_movable_start: usize,
    /// `Scene` の合成順序リストキャッシュ（土台または上乗せが変わったときだけ再構築する）
    scene_order: std::sync::OnceLock<Vec<SceneEntry>>,
    /// キャンバス上で進行中のドラッグ操作（ブラシ/消しゴム・選択・自由変形の排他的状態マシン）
    pub(crate) drag: ActiveCanvasDrag,
    /// 選択（書くのは `Session` の選ぶ・外す関数だけ：設計 claude/約束を入口で守る設計_5616fcc.md の 5.）
    selection: Selection,
    /// 浮いた片の中身（分け合える箱。取り消しの控えは箱を分け合う：`shared_floating`）と、その見た目
    floating: Option<Arc<Floating>>,
    floating_view: FloatingView,
    pub(crate) hover_target: HoverTarget,
    /// 現在インプレイス編集で潜っているグループ階層のパス（空 `[]` ならルートシーン）。書くのは `edit_path` と同じ所だけ
    editing_group_path: Vec<u64>,
    /// 自由変形の中心点（`None`：選んだものの真ん中）。書くのは部屋の中だけ（選択が変わったら `change_selection` が真ん中に
    /// 戻す）。部屋の外の道具は、中心点のドラッグの関数（`drag_pivot_to`・`carry_pivot`）を通す
    transform_pivot: Option<Point>,
    pub(crate) transform_hover: TransformHoverTarget,
    /// 輪郭の変形のカーソル合わせに使う、境目の線で持つ地図（生の図形が変わるまで使い回す）
    pub(crate) planar_maps: SyncCache<Option<Arc<ScopeMaps>>>,
    /// この操作で編集を断った理由（`apply` が知らせる）。書くのは部屋の中だけ
    refusal: Option<Refusal>,
    /// ドラッグを始めるときに作った、離したときの計算の答え（離したときに使い回す：`DropAhead`）
    drop_ahead: Option<DropAhead>,
}

/// ドラッグを始めるときに、離したときと同じ計算で作った答え（ドラッグ中の見た目に使い、離したときに使い回す：設計
/// claude/浮いた片と選択の決まりを1つにする設計_ddcd41e.md の 2.2）。使い回すのは、作ったときと今とで、浮いた片
/// （同じ中身かは `Floating::same_content`）・選択・階層・計算に使ったセル画の地図がみな同じときだけ（違えば
/// 作り直す。同じ入力からは同じ答えなので、使い回しても作り直しても答えは同じ）
struct DropAhead {
    floating: Option<Arc<Floating>>,
    selection: Selection,
    scope: Vec<u64>,
    /// 計算に使ったセル画の地図（生かしておくので、番地が別の地図に使い回されない）
    cels: Vec<Arc<LayerBase>>,
    /// 選んだ生の図形を持ち上げた浮いた片（移動・拡大・回転：`Session::lifted_floating`）
    lifted: Option<Result<Option<Floating>, ()>>,
    /// 浮いた片の確定の計画（複製：`plan_commit`）
    commit: Option<CommitPlan>,
    /// 写す中身（複製：`clipboard_ops::selected_content`）
    content: Option<crate::clipboard_ops::Content>,
    /// 複製のドラッグ中の確定した絵：確定の答えの地図の写し（`LayerBase::written_view`）と、その地図の図形の画面の写し
    /// （レイヤー, 元の地図の図形, 画面の写し）。離したとき、書き戻しは同じ写しを使い（`LayerBase::of_local`）、画面の
    /// 土台は同じ画面の写しを使い回す（`SceneBaseCache::offer`）
    committed: Vec<Arc<LayerBase>>,
    placed: Vec<scene_base_cache::PlacedCopy>,
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
    pub(in crate::canvas_tool) fn invalidate_scene_order(&mut self) {
        let _ = self.scene_order.take();
    }

    /// 自由変形の中心点（`None`：選んだものの真ん中）
    pub(crate) fn transform_pivot(&self) -> Option<Point> {
        self.transform_pivot
    }

    /// 中心点のドラッグ：中心点を `p` に置く
    pub(crate) fn drag_pivot_to(&mut self, p: Point) {
        self.transform_pivot = Some(p);
    }

    /// 拡大のドラッグ：置いた中心点があれば、ドラッグを始めたときの中心点 `orig` を変換 `affine` で動かした所へ運ぶ（真ん中
    /// のままなら真ん中のまま）
    pub(crate) fn carry_pivot(&mut self, affine: Affine, orig: Point) {
        if self.transform_pivot.is_some() {
            self.transform_pivot = Some(affine * orig);
        }
    }

    /// 画面の土台（読むだけ）
    pub(crate) fn base_cache(&self) -> &SceneBaseCache {
        &self.base_cache
    }

    /// 画面の土台を捨てる（途中で止まった操作のあと：次の `refresh_scene_base` が全部作り直す）。並びの写しも捨てる
    pub(in crate::canvas_tool) fn drop_scene_base(&mut self) {
        self.base_cache = Default::default();
        self.invalidate_scene_order();
    }

    /// シンボルインプレイス編集の経路（空ならルート）
    #[inline]
    pub(crate) fn edit_path(&self) -> &EditPath {
        &self.edit_path
    }

    /// 編集中のグループの階層（空ならその段の生の図形）
    #[inline]
    pub(crate) fn editing_group_path(&self) -> &[u64] {
        &self.editing_group_path
    }

    /// 現在の `Floating` への読み取り専用参照を返す
    #[inline]
    pub(crate) fn floating(&self) -> Option<&Floating> {
        self.floating.as_deref()
    }

    /// 浮いた片の見た目（片・残りの図形、隠す図形）
    #[inline]
    pub(crate) fn floating_view(&self) -> &FloatingView {
        &self.floating_view
    }

    /// 取り消しの控えにする浮いた片（箱を分け合う。箱は預かり物を持たない：`put_floating`）
    pub(crate) fn shared_floating(&self) -> Option<Arc<Floating>> {
        self.floating.clone()
    }

    /// `Floating` を置き換え、`overlay` を同期再構築する（重ね順は土台 `stage` の：`put_floating`）
    pub(in crate::canvas_tool) fn set_floating(&mut self, fl: Option<Floating>, stage: &Stage) {
        self.put_floating(fl, stage);
    }

    /// `Floating` を取り出し、`overlay` を同期再構築する（控えと分け合っていれば中身を写す）
    pub(in crate::canvas_tool) fn take_floating(&mut self) -> Option<Floating> {
        self.floating_rev += 1;
        let fl = self.floating.take().map(Arc::unwrap_or_clone);
        self.floating_view = FloatingView::default();
        self.rebuild_overlay_from_floating();
        fl
    }

    /// クロージャで `Floating` を更新し、`overlay` を同期再構築する（控えと分け合っていれば中身を写して書く）
    pub(in crate::canvas_tool) fn update_floating<R>(
        &mut self,
        stage: &Stage,
        f: impl FnOnce(&mut Option<Floating>) -> R,
    ) -> R {
        let mut fl = self.floating.take().map(Arc::unwrap_or_clone);
        let res = f(&mut fl);
        self.put_floating(fl, stage);
        res
    }

    /// 書き換えた浮いた片 `fl` を箱に入れる（浮いた片を書き換える入口はみなここを通る）：土台 `stage` の重ね順（文書の
    /// タイムラインの：`Stage::order`）に並べて空のレイヤーを外し
    /// （片も残りも無ければ外す）、見た目を作り（計算に使う写しは見た目が持つ）、中身の預かり物を外してから箱に入れる
    /// （箱と取り消しの控えは預かり物を持たない：`Floating::drop_fresh`）。そして `overlay` を同期再構築し、片をみな選んで、
    /// もう無い図形を選択から外す（浮いた片はいつもみな選ばれている。浮いた片が変わる所はみなここを通るので、片の番号が
    /// 変わる道を足しても漏れない。前は片の番号が変わる道ごとに呼ぶ側が選び直し、1つの道（動かしていない片を戻す道）だけ
    /// 選び直さず、選択の表示に無い塗りが、ドラッグで動き、Delete で消えた：設計審査 de92ceb の D-1 の (a)）
    fn put_floating(&mut self, fl: Option<Floating>, stage: &Stage) {
        self.floating_rev += 1;
        let world = self.edit_path.world_xform();
        let fl = fl.and_then(|mut fl| {
            fl.tidy(stage.order());
            (!fl.is_empty()).then_some(fl)
        });
        // テストでは毎回、見せる残りと確定で書く残りの関係を確かめる（`Floating::check_views`）
        #[cfg(test)]
        if crate::planar_map::test_checks() {
            if let Some(fl) = &fl {
                if let Err(e) = fl.check_views(&self.editing_group_path) {
                    panic!("持ち上げた絵の2つの残りが食い違う：{e}");
                }
            }
        }
        self.floating_view = fl.as_ref().map_or_else(FloatingView::default, |fl| {
            FloatingView::of(fl, world, stage)
        });
        self.floating = fl.map(|mut fl| {
            fl.drop_fresh();
            Arc::new(fl)
        });
        self.rebuild_overlay_from_floating();
        self.selection.shapes.extend(self.floating_view.piece_ids());
        self.retain_existing_selection();
    }

    /// 浮いた片の操作 `f` をする（浮いた片が無ければ、`modified` が `false` の空の浮いた片に。控えと分け合っていれば中身を
    /// 写して書く。片の選び直しは `put_floating`）
    pub(in crate::canvas_tool) fn with_floating<R>(&mut self, stage: &Stage, f: impl FnOnce(&mut Floating, &Stage) -> R) -> R {
        let mut fl = self
            .floating
            .take()
            .map_or_else(Floating::new, Arc::unwrap_or_clone);
        let res = f(&mut fl, stage);
        self.put_floating(Some(fl), stage);
        res
    }

    /// 現在の `self.floating` の状態から `self.overlay` を再構築する（ドラッグ中でない通常状態の表示用）
    fn rebuild_overlay_from_floating(&mut self) {
        let had_overlay = !self.overlay.is_empty() || self.overlay_only_selected;
        self.overlay.clear();
        self.overlay_only_selected = false;
        self.overlay_movable_start = 0;
        if self.floating.is_some() {
            let scope = self.editing_group_path.clone();
            let view = &self.floating_view;
            self.overlay.hide = view.hidden().clone();
            self.overlay.add.extend(
                view.placed_remainder()
                    .iter()
                    .chain(view.placed_lines_over())
                    .map(|(at, s)| (at.clone(), Arc::new(s.clone()))),
            );
            self.overlay_movable_start = self.overlay.add.len();
            let pieces: Vec<(Placement, Arc<FillShape>)> = view
                .placed_pieces(&scope)
                .map(|(at, p)| (at, Arc::new(p.clone())))
                .collect();
            self.overlay.add.extend(pieces);
            self.invalidate_scene_order();
        } else if had_overlay {
            self.invalidate_scene_order();
        }
    }

    /// ドラッグ用の仮表示 `Overlay` を解除し、`Floating` のみの上乗せ状態に戻す
    #[inline]
    pub(in crate::canvas_tool) fn clear_overlay(&mut self) {
        self.rebuild_overlay_from_floating();
    }

    /// 輪郭の変形の仮表示を、確定と同じ計算の結果（消す元の図形 `removed`、新しく入れる塗りの形 `added`）にする。
    /// 新しい形は、その階層の生の図形の置き場に置く（確定と同じ。生の図形どうしは重ならないので、並びは見た目に出ない）
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
        let placement = Placement::RawOf(layer_id, group_path.to_vec());
        self.overlay
            .add
            .extend(added.into_iter().map(|s| (placement.clone(), Arc::new(s))));
        self.invalidate_scene_order();
    }

    /// 選択移動または `Alt` 複製移動の仮表示用に、選択中図形を `Overlay` に載せる。複製のときは、写す中身 `content` と、
    /// 動かす生の絵 `copy`（離したときに置く写しの絵：`clipboard_ops::begin_move_selection_drag`）
    pub(in crate::canvas_tool) fn begin_move_overlay(
        &mut self,
        stage: &Stage,
        alt_duplicate: bool,
        content: Option<crate::clipboard_ops::Content>,
        copy: Vec<FillShape>,
    ) {
        if alt_duplicate {
            let mut layers: Vec<LayerId> = content
                .iter()
                .flat_map(|c| c.layers.iter().map(|l| l.0))
                .collect();
            layers.extend(self.floating.iter().flat_map(|fl| fl.layers().iter().map(|l| l.layer_id)));
            let mut ahead = self.new_ahead(stage, &layers);
            ahead.content = content;
            self.drop_ahead = Some(ahead);
        }
        self.begin_selection_overlay(stage, alt_duplicate, false, copy);
    }

    /// 今の浮いた片・選択・階層と、レイヤー `layers` のセル画の地図で作る答えの入れ物（答えはまだ無い）
    fn new_ahead(&self, stage: &Stage, layers: &[LayerId]) -> DropAhead {
        let scope = self.editing_group_path.clone();
        let mut cels: Vec<Arc<LayerBase>> = Vec::new();
        let mut seen: Vec<LayerId> = Vec::new();
        for &lid in layers {
            if seen.contains(&lid) {
                continue;
            }
            seen.push(lid);
            if let Some(cel) = stage.base_at(lid, &scope) {
                cels.push(cel);
            }
        }
        DropAhead {
            floating: self.floating.clone(),
            selection: self.selection.clone(),
            scope,
            cels,
            lifted: None,
            commit: None,
            content: None,
            committed: Vec::new(),
            placed: Vec::new(),
        }
    }

    /// 答え `ahead` を、今（浮いた片 `floating`・土台 `stage`）使い回してよいか：作ったときと、浮いた片・選択・階層・
    /// セル画の地図がみな同じ
    fn ahead_holds(&self, ahead: &DropAhead, floating: Option<&Floating>, stage: &Stage) -> bool {
        let same_floating = match (&ahead.floating, floating) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_content(b),
            _ => false,
        };
        same_floating
            && ahead.selection == self.selection
            && ahead.scope == self.editing_group_path
            && ahead.cels.iter().all(|b| {
                stage
                    .cels()
                    .any(|(_, c)| c.scope_base(&ahead.scope).stored.id() == b.stored.id())
            })
    }

    /// ドラッグを始めるときに作った、浮いた片 `fl` の確定の計画（使い回してよいときだけ。取り出す）。確定の答えの地図の
    /// 写しと、その図形の画面の写しは、画面の土台に渡す（書き戻しと、次の画面の土台の作り直しが使い回す）
    pub(in crate::canvas_tool) fn take_ahead_commit(&mut self, fl: &Floating, stage: &Stage) -> Option<CommitPlan> {
        let ahead = self.drop_ahead.as_ref()?;
        if !self.ahead_holds(ahead, Some(fl), stage) {
            return None;
        }
        let ahead = self.drop_ahead.as_mut()?;
        let plan = ahead.commit.take()?;
        let world = self.edit_path.world_xform();
        self.base_cache.offer(
            world,
            std::mem::take(&mut ahead.committed),
            std::mem::take(&mut ahead.placed),
        );
        Some(plan)
    }

    /// ドラッグを始めるときに作った、写す中身（使い回してよいときだけ。取り出す）
    pub(in crate::canvas_tool) fn take_ahead_content(&mut self, stage: &Stage) -> Option<crate::clipboard_ops::Content> {
        let ahead = self.drop_ahead.as_ref()?;
        if !self.ahead_holds(ahead, self.floating.as_deref(), stage) {
            return None;
        }
        self.drop_ahead.as_mut()?.content.take()
    }

    /// 離したときの答えを捨てる（ドラッグを確定したあと）
    pub(in crate::canvas_tool) fn drop_ahead_done(&mut self) {
        self.drop_ahead = None;
    }

    /// 自由変形（拡大・回転）の仮表示用に、選択中図形を `Overlay` に載せ、変形元のクローン列を返す
    pub(crate) fn begin_transform_overlay(&mut self, stage: &Stage) -> Vec<FillShape> {
        self.begin_selection_overlay(stage, false, true, Vec::new())
    }

    fn begin_selection_overlay(
        &mut self,
        stage: &Stage,
        alt_duplicate: bool,
        for_transform: bool,
        copy: Vec<FillShape>,
    ) -> Vec<FillShape> {
        self.overlay.clear();
        self.overlay_only_selected = alt_duplicate;
        self.overlay_movable_start = 0;
        if self.selection.is_empty() {
            self.rebuild_overlay_from_floating();
            return Vec::new();
        }
        // ドラッグ中の見た目は、離したときと同じ計算から作る（設計 claude/浮いた片と選択の決まりを1つにする設計_ddcd41e.md の
        // 2.2）：止まった絵は離したあとに残る絵、動かす生の絵は離したあとの浮いた片の片。計算した答えは、離したときに使い
        // 回す（`DropAhead`）。前は、選んだ面・線・片・残りを場面ごとに拾って並べ直したので、離すと見た目が変わることがあった
        // （設計審査 de92ceb の D-1 の (c)・レビュー de92ceb の N-3001）
        let scope = self.editing_group_path.clone();
        let world = self.edit_path.world_xform();
        let moving_raw: Vec<(Placement, Arc<FillShape>)> = if alt_duplicate {
            // 複製は、離すとき、今の浮いた片を確定してから写しを置く（`duplicate_selected_in_working_copy`）ので、止まった絵は
            // 確定した絵（確定の計算 `FloatLayer::commit_edit`。動かしていない切り口は確定で捨てて元に戻るので、セル画のまま）、
            // 動かす生の絵は、離したときに置く写しの絵（`copy`：`clipboard_ops::begin_move_selection_drag`。レイヤーごとに1つの
            // 地図なので、組は1つ）
            let mut plan = Some(crate::canvas_tool::CommitPlan::new());
            if let Some(fl) = self.floating.as_ref().filter(|fl| fl.modified()) {
                let outcomes = fl.commit_edits(stage);
                for (l, outcome) in fl.layers().iter().zip(outcomes) {
                    match outcome {
                        Some((crate::planar_map::LocalEdit::Changed(w), base)) => {
                            // 確定で変わる所だけを隠して出す（浮いた片の残りと同じ見た目の変わり方：
                            // `LayerBase::written_view`。手間は変わる所の分）
                            let (committed, hidden, shown) = base.written_view(w.clone(), &scope);
                            self.overlay.hide.extend(hidden);
                            let shown: HashSet<ShapeId> = shown.into_iter().collect();
                            let at = Placement::RawOf(l.layer_id, scope.clone());
                            let mut placed = Vec::new();
                            for src in committed.drawn_shapes(&scope) {
                                if !shown.contains(&src.shape_id) {
                                    continue;
                                }
                                let mut f = (*src).clone();
                                f.layer_id = l.layer_id;
                                f.place(world);
                                let f = Arc::new(f);
                                self.overlay.add.push((at.clone(), f.clone()));
                                placed.push((l.layer_id, src, f));
                            }
                            if let Some(ahead) = &mut self.drop_ahead {
                                ahead.committed.push(committed);
                                ahead.placed.extend(placed);
                            }
                            if let Some(p) = &mut plan {
                                p.push((l.layer_id, w, base));
                            }
                        }
                        Some((crate::planar_map::LocalEdit::Same, _)) => {}
                        _ => plan = None,
                    }
                }
            }
            if let Some(ahead) = &mut self.drop_ahead {
                ahead.commit = plan;
            }
            copy.into_iter()
                .map(|f| (Placement::AboveRaw(f.layer_id, scope.clone(), 0), Arc::new(f)))
                .collect()
        } else {
            match self.lift_ahead(stage) {
                Ok(Some(mut fl)) => {
                    fl.tidy(stage.order());
                    let view = FloatingView::of(&fl, world, stage);
                    self.overlay.hide = view.hidden().clone();
                    self.overlay.add.extend(
                        view.placed_remainder()
                            .iter()
                            .chain(view.placed_lines_over())
                            .map(|(at, s)| (at.clone(), Arc::new(s.clone()))),
                    );
                    view.placed_pieces(&scope)
                        .map(|(at, p)| (at, Arc::new(p.clone())))
                        .collect()
                }
                Ok(None) => Vec::new(),
                // 持ち上げられなければ、離したときに断るので、何も動かさない
                Err(()) => {
                    self.rebuild_overlay_from_floating();
                    return Vec::new();
                }
            }
        };
        self.overlay_movable_start = self.overlay.add.len();
        let mut orig_shapes = Vec::new();
        let moving = moving_raw.into_iter();
        // 選んだグループ・インスタンスの図形（生の図形でないもの）は、その図形を動かす（離したあとは、グループの地図・
        // インスタンスの置き方の変換を動かす。複製は、写しを今の階層のいちばん上に置く）
        let nested: Vec<(Placement, Arc<FillShape>)> = self
            .base_cache
            .shapes()
            .iter()
            .filter(|s| !s.is_raw_at_scope(&scope) && self.selection.contains_shape(s))
            .map(|s| {
                let at = if alt_duplicate {
                    Placement::TopOfLayer(s.layer_id, scope.clone())
                } else {
                    Placement::ReplaceAt(s.shape_id)
                };
                (at, s.clone())
            })
            .collect();
        for (at, f) in moving.chain(nested) {
            if !alt_duplicate {
                if let Placement::ReplaceAt(id) = at {
                    self.overlay.hide.insert(id);
                }
            }
            if for_transform {
                let _ = f.fill_mesh();
                orig_shapes.push((*f).clone());
            }
            self.overlay.add.push((at, f));
        }
        self.invalidate_scene_order();
        orig_shapes
    }

    /// 自由変形ドラッグ中、`orig_shapes` に対してキャッシュ済みメッシュ頂点バッファを再利用して `affine` を `overlay.add` へ適用する
    pub(crate) fn update_transform_overlay(&mut self, orig_shapes: &[FillShape], affine: Affine) {
        let start = self.overlay_movable_start.min(self.overlay.add.len());
        let world = self.edit_path.world_xform();
        for ((_, overlay_shape), orig_shape) in
            self.overlay.add[start..].iter_mut().zip(orig_shapes.iter())
        {
            // 生の絵とグループの線は、拡大しても太さが変わらない（確定と同じ形。シンボルの中の線は図形ごと写して太くなる）
            let overlay_shape = Arc::make_mut(overlay_shape);
            if orig_shape.line.is_some() && orig_shape.symbol_instance.is_none() {
                overlay_shape.clone_from(orig_shape);
                overlay_shape.transform_keeping_line_width(affine, world);
            } else {
                overlay_shape.transform_from_origin(orig_shape, affine);
            }
        }
    }

    /// 選択移動ドラッグ中、`overlay.add` 内の可動図形とカスタムピボットを `delta` だけ平行移動する
    pub(in crate::canvas_tool) fn translate_move_overlay(&mut self, delta: KVec2) {
        let start = self.overlay_movable_start.min(self.overlay.add.len());
        for (_, shape) in &mut self.overlay.add[start..] {
            Arc::make_mut(shape).translate(delta);
        }
        if let Some(piv) = &mut self.transform_pivot {
            *piv += delta;
        }
    }

    /// 確定時に結合が必要な変更済み `Floating` が保持されているかを返す
    #[cfg(test)]
    #[inline]
    pub(in crate::canvas_tool) fn has_modified_floating(&self) -> bool {
        self.floating.as_ref().is_some_and(|fl| fl.modified())
    }

    /// 選択中の生の図形（隠していないセル画の図形と、浮いている残り）を片に持ち上げる（動かす・変形・色を変える・グループに
    /// する前。持ち上げただけの組は置かない組で、動かす・色を変える・取り出すときに置く組になる：`Floating::place_all`）。レイヤーごとに、その面の所を選ぶ所にして切る（`Floating::cut`）。生の図形はいつも
    /// 片の地図で書くので、切れないレイヤー（地図にできない）があれば、浮いた片を元に戻して `false`（操作を断る呼ぶ側が
    /// `refusal` に記す）
    pub(in crate::canvas_tool) fn lift_selected_raw_shapes_to_floating(&mut self, stage: &Stage) -> bool {
        // ドラッグを始めるときに同じ計算をしてあれば、それを使い回す（`DropAhead`）
        let holds = self
            .drop_ahead
            .as_ref()
            .is_some_and(|a| self.ahead_holds(a, self.floating.as_deref(), stage));
        let ahead = holds
            .then(|| self.drop_ahead.as_mut().and_then(|a| a.lifted.take()))
            .flatten();
        match ahead.unwrap_or_else(|| self.lifted_floating(stage)) {
            Ok(None) => true,
            Ok(Some(fl)) => {
                self.put_floating(Some(fl), stage);
                true
            }
            Err(()) => false,
        }
    }

    /// 選んでいる生の図形（隠していないセル画の図形と、浮いている残り）を片に持ち上げた浮いた片（文書と今の浮いた片は
    /// 変えない。持ち上げの計算はこれ1つ：確定する前の `lift_selected_raw_shapes_to_floating` と、ドラッグ中の見た目
    /// `lift_ahead`）。レイヤーごとに、その面の所を選ぶ所にして切る（`Floating::cut`）。`Ok(None)`：持ち上げるものも
    /// 浮いた片も無い。`Err`：切れないレイヤー（地図にできない）がある
    fn lifted_floating(&self, stage: &Stage) -> Result<Option<Floating>, ()> {
        let by_layer = self.selected_raw_ids_by_layer(true);
        if by_layer.is_empty() && self.floating.is_none() {
            return Ok(None);
        }
        let mut fl = self
            .floating
            .as_deref()
            .cloned()
            .unwrap_or_else(Floating::new);
        if !fl.cut(stage, &by_layer, None).is_empty() {
            return Err(());
        }
        Ok(Some(fl))
    }

    /// ドラッグを始めるときに、離したときと同じ持ち上げをして（`lifted_floating`）、答えを離したときのためにしまう
    fn lift_ahead(&mut self, stage: &Stage) -> Result<Option<Floating>, ()> {
        let lifted = self.lifted_floating(stage);
        let mut layers: Vec<LayerId> = self.selected_raw_ids_by_layer(true).into_iter().map(|l| l.0).collect();
        layers.extend(self.floating.iter().flat_map(|fl| fl.layers().iter().map(|l| l.layer_id)));
        let mut ahead = self.new_ahead(stage, &layers);
        ahead.lifted = Some(lifted.clone());
        self.drop_ahead = Some(ahead);
        lifted
    }

    /// 選んでいるものに変換をかけて置けるかを決めるもの（`TransformFit`）：浮いた片と、まだ持ち上げていない選んだ生の
    /// 図形（`floating::piece_maps`）と、選んだグループの中の生の図形の階層の地図（`selected_group_scopes`）。インスタンスは
    /// 地図を持たないので、変換が潰れなければ置ける。拡大・回転のドラッグは、始めるときに1回だけ作って持つ
    pub(crate) fn transform_fit(&self, stage: &Stage) -> TransformFit {
        let mut maps = crate::floating::piece_maps(
            self.floating.as_deref(),
            stage,
            &self.selected_raw_ids_by_layer(true),
        );
        maps.extend(
            self.selected_group_scopes()
                .iter()
                .filter_map(|(layer_id, path)| {
                    Some((
                        crate::floating::FitMap::Held(stage.base_at(*layer_id, path)?),
                        Affine::IDENTITY,
                    ))
                }),
        );
        TransformFit {
            world: self.edit_path.world_xform(),
            maps,
        }
    }

    /// 選んでいるグループの中の生の図形の階層：（レイヤー, 階層）。浮いた片が隠すセル画の図形と、今の階層の生の図形
    /// （浮いた片にして動かす）は入れない
    pub(in crate::canvas_tool) fn selected_group_scopes(&self) -> Vec<(LayerId, Vec<u64>)> {
        let hidden = self.floating_view.hidden();
        let mut scopes: Vec<(LayerId, Vec<u64>)> = Vec::new();
        for s in self.base_cache.shapes() {
            if !hidden.contains(&s.shape_id)
                && self.selection.contains_shape(s)
                && s.symbol_instance.is_none()
                && !s.is_raw_at_scope(&self.editing_group_path)
                && !scopes
                    .iter()
                    .any(|x| x.0 == s.layer_id && x.1 == s.group_path)
            {
                scopes.push((s.layer_id, s.group_path.clone()));
            }
        }
        scopes
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
        let hidden = self.floating_view.hidden();
        if base {
            for s in self.base_cache.shapes() {
                if !hidden.contains(&s.shape_id)
                    && self.selection.contains_shape(s)
                    && s.is_raw_at_scope(scope)
                {
                    add(s);
                }
            }
        }
        for r in self.floating_view.remainder() {
            if self.selection.shapes.contains(&r.shape_id) {
                add(r);
            }
        }
        by_layer
    }

    /// 地図 `maps`（レイヤーごと、下から順。文書の座標。面の番号が片の番号）を、それぞれ変換をかける浮いた片の組として
    /// 足す（置く組。確定のときに、変換をかけて、その階層の地図に重ねる）。グループの解除はグループの地図そのもの、
    /// 新しい中身（貼り付け・Alt 複製・シンボルの分解）は中身の生の図形の地図（`clipboard_ops::put_placements`）
    pub(in crate::canvas_tool) fn add_floating_maps(
        &mut self,
        stage: &Stage,
        maps: Vec<(LayerId, crate::planar_map::CheckedMap, Affine)>,
    ) {
        if maps.is_empty() {
            return;
        }
        self.with_floating(stage, |fl, _| {
            for (layer_id, map, xform) in maps {
                fl.add_map(layer_id, map, xform);
            }
        });
    }

    /// 浮いている残りの図形が選択に入ったら、片に持ち上げる（動かしていなければ、ほかの片と一緒に切り直して1つの組に
    /// する：同じ色でつながる片は1つになる）
    pub(in crate::canvas_tool) fn promote_selected_remainder_to_pieces(&mut self, stage: &Stage) {
        let by_layer = self.selected_raw_ids_by_layer(false);
        if by_layer.is_empty() {
            return;
        }
        self.with_floating(stage, |fl, stage| {
            fl.cut(stage, &by_layer, None);
        });
    }

    /// 未変形スライスの `Floating` で選択中の片がなくなった場合は破棄する
    pub(in crate::canvas_tool) fn drop_unmodified_slice_if_unselected(&mut self, stage: &Stage) -> bool {
        if self.floating.as_ref().is_some_and(|fl| {
            !fl.modified()
                && !self
                    .floating_view
                    .pieces()
                    .iter()
                    .any(|p| self.selection.shapes.contains(&p.shape_id))
        }) {
            self.set_floating(None, stage);
            true
        } else {
            false
        }
    }

    /// 現在選択されている図形の `Scene` インデックス列（昇順）を返す
    pub(in crate::canvas_tool) fn selected_indices(&self) -> Vec<usize> {
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

    /// 選択
    #[inline]
    pub(crate) fn selection(&self) -> &Selection {
        &self.selection
    }

    /// 選ぶ・外す操作で選択を `f` で変える（選択を変える所はみなここを通る）。選択が変わったら自由変形の中心点を真ん中に
    /// 戻すのは、入口 `apply` が操作のあとに決める（見ている場所が変わったときと一緒に1か所で）。浮いた片の片をみな選ぶのは、
    /// 浮いた片を書き換える入口（`put_floating`）：選択の一部を外すときは、外した片を確定してから浮いた片を書き換えるので、
    /// ここで選び直すと外せなくなる
    fn change_selection(&mut self, f: impl FnOnce(&mut Self)) {
        f(self);
    }

    /// 選択を `sel` に入れ替える（選ぶ・外す操作）
    pub(in crate::canvas_tool) fn replace_selection(&mut self, sel: Selection) {
        self.change_selection(|s| s.selection = sel);
    }

    /// 選択を取り出し、選択を空にする（選んだものを消すとき）
    pub(in crate::canvas_tool) fn take_selection(&mut self) -> Selection {
        let mut out = Selection::default();
        self.change_selection(|s| out = std::mem::take(&mut s.selection));
        out
    }

    /// 図形 `shapes`・インスタンス `instances` を選択に足す（貼り付けた中身を選ぶとき）
    pub(in crate::canvas_tool) fn add_to_selection(
        &mut self,
        shapes: impl IntoIterator<Item = ShapeId>,
        instances: impl IntoIterator<Item = InstanceGid>,
    ) {
        self.change_selection(|s| {
            s.selection.shapes.extend(shapes);
            s.selection.instances.extend(instances);
        });
    }

    /// 断った操作で、選択を前のまま `sel` に戻す（選ぶ操作ではないので、中心点は変えない）
    fn restore_selection(&mut self, sel: Selection) {
        self.selection = sel;
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
    pub(in crate::canvas_tool) fn set_selected_indices(&mut self, stage: &Stage, indices: impl IntoIterator<Item = usize>) {
        self.change_selection(|s| {
            s.selection.clear();
            s.select_shape_indices(stage, indices);
            s.drop_unmodified_slice_if_unselected(stage);
        });
    }

    /// 指定した `Scene` インデックス列に対応する図形・シンボルインスタンスを選択状態に追加する
    pub(in crate::canvas_tool) fn select_shape_indices(&mut self, stage: &Stage, indices: impl IntoIterator<Item = usize>) {
        let scene = self.scene();
        let keys: Vec<_> = indices
            .into_iter()
            .filter_map(|idx| {
                scene
                    .get(idx)
                    .map(|s| (s.symbol_instance.map(|b| b.instance_gid), s.shape_id))
            })
            .collect();
        self.change_selection(|s| {
            s.insert_selection_keys(keys);
            s.promote_selected_remainder_to_pieces(stage);
        });
    }

    /// 現在の `self.selection` に対応する `Scene` 選択マスク（`Vec<bool>`）と有効な選択が1つ以上あるかを返す
    pub(crate) fn selected_shape_mask(&self) -> (Vec<bool>, bool) {
        self.scene().selected_shape_mask(&self.selection)
    }

    /// `Scene` 上に現在存在する図形・シンボルインスタンスのみを選択に残す
    pub(in crate::canvas_tool) fn retain_existing_selection(&mut self) {
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
        // 浮いた片があるときは階層を変えない（浮いた片の片は今の階層にあるので、空にはならない。見る場所を変えるのは
        // 貼り戻してから：`Tab::move_view`）
        while self.floating.is_none() && !self.editing_group_path.is_empty() {
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
    pub(in crate::canvas_tool) fn select_shapes_in_layers(&mut self, layer_ids: &HashSet<u64>) {
        let keys: Vec<_> = self
            .scene()
            .iter()
            .filter(|s| {
                s.is_in_editing_scope(&self.editing_group_path) && layer_ids.contains(&s.layer_id)
            })
            .map(|s| (s.symbol_instance.map(|b| b.instance_gid), s.shape_id))
            .collect();
        self.change_selection(|s| {
            s.selection.clear();
            s.insert_selection_keys(keys);
        });
    }

    /// 指定レイヤーID集合 `layer_ids` に属する図形・シンボルインスタンスの選択のみを残し、それ以外を選択から外す
    pub(in crate::canvas_tool) fn retain_selection_in_layers(&mut self, layer_ids: &HashSet<u64>) {
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
    }

    /// 指定した `instance_gid` に属するキャンバス上の図形をすべて選択状態にする
    pub(in crate::canvas_tool) fn select_symbol_instance_by_gid(&mut self, instance_gid: u64) {
        let present = self.base_cache.shapes().iter().any(|s| {
            s.symbol_instance
                .is_some_and(|b| b.instance_gid == instance_gid)
        });
        self.change_selection(|s| {
            s.selection.clear();
            if present {
                s.selection.instances.insert(instance_gid);
            }
        });
    }

    /// 選択ツールまたは自由変形ツールで進行中だった図形変形・移動ドラッグのみを中断して仮表示 `Overlay` を破棄する（ブラシのストローク状態には干渉しない）
    pub(in crate::canvas_tool) fn rollback_shape_drag(&mut self) {
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
        // ドラッグをやめたら、仮表示と、離したときのために作った答えを捨てる
        self.drop_ahead = None;
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
    pub(crate) sounds: crate::sound::SoundLibrary,
    pub(crate) floating: Option<Arc<Floating>>,
    /// その時の選択（取り消し・やり直しで、絵と一緒に戻す。取り消すと、操作の前の選択に戻る）
    pub(crate) selection: Selection,
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
    RenameSound(u64),
    RenameLayer(u64),
    NudgeSelection,
}

/// 続けた編集のまとめを区切る時
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CoalesceEnd {
    /// 数値の欄の編集が一区切りした（どれかがフォーカスを失った、またはどれもドラッグ中・フォーカス中でない）
    FieldIdle,
    /// 名前の欄がフォーカスを失った
    LostFocus,
    /// キーを押し直した（押しっぱなしの繰り返しはまとめる）
    KeyPressedAgain,
}

impl CoalesceKey {
    /// このまとめを区切る時（まとめのキーごとの表。区切る時を決めるのはここだけ：前は区切りを送る8か所が、それぞれ
    /// 自分の決まりで送っていた：約束の棚卸し 5616fcc の C の B7）
    pub(crate) const fn ends(self) -> CoalesceEnd {
        match self {
            Self::StageSize | Self::Fps | Self::SymbolPlayback(_) | Self::SymbolColorEffect(_) => {
                CoalesceEnd::FieldIdle
            }
            Self::RenameSymbol(_) | Self::RenameSound(_) | Self::RenameLayer(_) => CoalesceEnd::LostFocus,
            Self::NudgeSelection => CoalesceEnd::KeyPressedAgain,
        }
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
    /// 直前の変更操作の `CoalesceKey`（同一キーが連続した場合は新規 Undo エントリを積まずに合流する）
    pub(crate) last_coalesce_key: Option<CoalesceKey>,
    /// `apply` 実行中の、操作を始める前の状態（取り消しの1手にするときにしまうのはこれ。操作の中で選択を確定して外して
    /// いても、取り消すと操作を始める前の絵と選択に戻る）。`apply` の中だけで持つ
    pub(crate) before: Option<UndoEntry>,
    /// 中身の世代：`apply` が中身の変化を見つけるたびに進む（続けた編集をまとめて取り消しの1手を積まないときも進む）。
    /// 「中身が変わったか」で控え直す所（復旧ファイルに書くタブの控え `recovery::TabCopies`、タイムラインの
    /// ドラッグを捨てる判定）はこれを見る（前は取り消しの世代 `current_commit_id` を見たので、まとめた2回目以降の編集を
    /// 取りこぼした：約束の棚卸し 5616fcc の C の A2）
    pub(crate) content_gen: u64,
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
            before: None,
            content_gen: 0,
        }
    }
}

impl UndoHistory {
    /// 連続変更の Undo 合流セッションを終了する
    #[inline]
    pub(in crate::canvas_tool) fn end_coalesce(&mut self) {
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

    /// 新しい Undo エントリをスタックへ追加し、Redo スタックをクリアして現在の状態世代IDを進める
    pub(crate) fn push(&mut self, mut entry: UndoEntry) {
        entry.commit_id = self.current_commit_id;
        self.current_commit_id = self.next_commit_id;
        self.next_commit_id = self.next_commit_id.saturating_add(1);
        self.push_undo_from_redo(entry);
        self.redo_stack.clear();
    }

    /// Undo スタック末尾から直前のスナップショットを取り出す
    pub(in crate::canvas_tool) fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.last_coalesce_key = None;
        self.undo_stack.pop_back()
    }

    /// Redo スタック末尾から次のスナップショットを取り出す
    pub(in crate::canvas_tool) fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.last_coalesce_key = None;
        self.redo_stack.pop()
    }

    /// Undo 実行前の現在状態を Redo スタックへ退避する
    pub(in crate::canvas_tool) fn push_redo(&mut self, entry: UndoEntry) {
        self.redo_stack.push(entry);
    }

    /// Redo 実行前の現在状態を Undo スタックへ戻す（Redo スタックは消去しない）
    pub(in crate::canvas_tool) fn push_undo_from_redo(&mut self, entry: UndoEntry) {
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
    /// 鉛筆で引く線・インクボトルで乗せる線
    pub(crate) stroke: crate::planar_map::EdgeStroke,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            tool: Tool::Brush,
            select_sub_mode: SelectSubMode::Reshape,
            brush_radius: 8.0,
            eraser_radius: 22.0,
            brush_color: Color32::from_rgb(40, 45, 55),
            stroke: crate::planar_map::EdgeStroke {
                color: Color32::BLACK,
                width: 2,
            },
        }
    }
}

/// キャンバス左上に一定時間自動で消える知らせを足す（知らせを足すのはここ1つ：`Notices::push`）
pub(in crate::canvas_tool) fn show_notice(notices: &mut Notices, kind: NoticeKind, message: impl Into<String>) {
    notices.push(kind, message.into(), None);
}

/// 見分け `key` の知らせを、文 `message` に書き直す（無ければ足す）
pub(crate) fn show_keyed_notice(notices: &mut Notices, key: NoticeKey, kind: NoticeKind, message: String) {
    notices.push(kind, message, Some(key));
}

// ============================================================================
// 5. タブ（1つの開いているファイル/ドキュメント編集セッション：Tab）
// ============================================================================

/// 1つの開いているタブ（保存対象の `Document`・描画および当たり判定用の読み取り専用 `Scene`・操作中の `Session`・タブごとの表示設定 `View`・Undo履歴 `UndoHistory`）
pub(crate) struct Tab {
    /// タブの番号（「どの作品か」の見分け：タブ間クリップボード・シンボル移植の同一タブ判定・復旧の書き手）。外からは
    /// 書けず（読むのは `id()`）、決まるのは、新しいタブを作るときと作品を入れるとき（どちらも新しい番号 `FreshTabId` を
    /// 受け取る）だけ（設計審査 392e9af の D-4）
    id: u64,
    /// 未保存タブ時の連番（`Untitled 1`, `Untitled 2`, ...）
    pub(crate) untitled_number: usize,
    /// 現在開いている・保存先の `.is2d` プロジェクトファイルパス（未保存の新規ドキュメント時は `None`）
    pub(crate) file: Option<PathBuf>,
    /// 保存対象のデータ（ルートタイムライン・シンボルライブラリ・ステージサイズ・採番カウンタ）。書けるのはこの部屋
    /// （`canvas_tool`：入口と作業）の中だけ。部屋の外は読むだけ（`Tab::doc`）
    doc: Document,
    /// 今フレームの作業図形・選択・変形・排他ドラッグ・編集スタック
    pub(crate) session: Session,
    /// タブごとのキャンバス表示設定（ズーム・パン・枠外クリップ・ワイヤーフレーム・オニオンスキン・選択シンボル）
    pub(crate) view: View,
    /// Undo / Redo 履歴スタック。書けるのはこの部屋の中だけ（部屋の外は読むだけ：`Tab::history`。保存した・していないの
    /// 印は `Tab::mark_saved`・`mark_unsaved`）
    history: UndoHistory,
}

impl Tab {
    /// 作品（読むだけ。書くのは部屋の中の入口と作業）
    pub(crate) fn doc(&self) -> &Document {
        &self.doc
    }

    /// 取り消しの記録（読むだけ）
    pub(crate) fn history(&self) -> &UndoHistory {
        &self.history
    }

    /// タブの番号（「どの作品か」の見分け）
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// タブに作品 `doc` を入れ、タブの番号を新しい番号 `id` にする（新しい作品を入れる道はみなここを通る：新規・開く・
    /// 最後のタブを閉じる：`project_io::install_document`）。取り消しの記録・作業の状態・表示を初めに戻し、画面の土台を作る
    pub(crate) fn install(&mut self, id: FreshTabId, doc: Document) {
        self.id = id.0;
        self.doc = doc;
        self.history = UndoHistory::default();
        self.session = Session::default();
        self.view = View::default();
        self.refresh_scene_base();
    }

    /// 今の作品を保存したと記す（作品の中身は変えない）
    pub(crate) fn mark_saved(&mut self) {
        self.history.mark_saved();
    }

    /// 今の作品を保存していないと記す（作品の中身は変えない）
    pub(crate) fn mark_unsaved(&mut self) {
        self.history.mark_unsaved();
    }
}

/// 文書 `doc` に、浮いている片 `floating`（編集位置 `edit_path`・グループの階層 `editing_group_path` で持ち上げたもの）を
/// 置いた写し（タブ `(id, untitled_number)` の一時的な写しの上で確定し、元の文書には触れない）。片が無いか変わっていなければ
/// 文書の写しのまま。`None`：浮いた片を置けない（`commit_floating_layers` が断る）。確定した文書の写しを作るのはこれ1つ
/// （`Tab::committed_document_copy` と、復旧ファイルに書くタブの控え：`recovery::TabCopy`）
pub(crate) fn committed_document(
    (id, untitled_number): (u64, usize),
    doc: &Document,
    edit_path: &EditPath,
    editing_group_path: &[u64],
    floating: Option<&Floating>,
) -> Option<Document> {
    let Some(fl) = floating.filter(|fl| fl.modified()) else {
        return Some(doc.clone());
    };
    let mut temp_tab = Tab::with_identity(id, untitled_number);
    temp_tab.doc = doc.clone();
    // 浮いた片はまだ入れていないので、貼り戻すものは無い
    let moved = temp_tab.move_view(|e, g| {
        *e = edit_path.clone();
        *g = editing_group_path.to_vec();
    });
    debug_assert!(moved);
    // 浮いた片の土台は文書から作る（画面の土台を先に作っておく約束は要らない：設計審査 b7208d9 の 8.）
    let stage = temp_tab.stage();
    temp_tab.session.set_floating(Some(fl.clone()), &stage);
    commit_floating(&mut temp_tab).then_some(temp_tab.doc)
}

/// 新しいタブの番号（「どの作品か」の見分け：クリップボードの使い回し・復旧の書き手が使う）。作るのは番号を配る係
/// `TabIds::fresh` だけで、値で渡すので1回しか使えない。タブに作品を入れる入口 `Tab::install` はこれを受け取るので、作品を
/// 入れるたびに必ず新しい番号になる（設計審査 392e9af の D-4。前は入れる入口が番号を替えず、替えるのは呼ぶ側の3か所だった）
pub(crate) struct FreshTabId(u64);

/// タブの番号を配る係（アプリに1つ。1 から配り、配った番号は二度と配らない）
#[derive(Debug)]
pub(crate) struct TabIds {
    next: u64,
}

impl Default for TabIds {
    fn default() -> Self {
        Self { next: 1 }
    }
}

impl TabIds {

    /// 新しい番号
    pub(crate) fn fresh(&mut self) -> FreshTabId {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        FreshTabId(id)
    }
}

impl Tab {
    /// 指定された `id` と `untitled_number` で空の初期タブを生成する
    pub(crate) fn new_empty(id: FreshTabId, untitled_number: usize) -> Self {
        Self::with_identity(id.0, untitled_number)
    }

    /// タブの番号 `id`（今あるタブの番号）の、空のタブ（部屋の中だけ：持ち上げた絵を置く計算の一時的なタブ
    /// `committed_document`。部屋の外で番号を選んでタブを作る道は無い）
    fn with_identity(id: u64, untitled_number: usize) -> Self {
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

    /// 編集中のシンボルの番号（ルートを編集中か、シンボルがライブラリに無ければ `None`。2分探索）
    fn active_symbol_id(&self) -> Option<u64> {
        let id = self.session.edit_path.last()?.symbol_id;
        self.doc.library().get(id).map(|_| id)
    }

    /// テスト用：画面の土台が最新か（`refresh_scene_base` で作り直すものが無い）
    #[cfg(test)]
    pub(in crate::canvas_tool) fn scene_base_is_fresh(&self) -> bool {
        self.session.base_cache.is_fresh(
            self.active_timeline(),
            self.doc.library(),
            self.session.edit_path.world_xform(),
        )
    }

    /// `Document` の現在フレームおよび `edit_path` から `Scene` のレイヤー別土台キャッシュを再構築する
    pub(crate) fn refresh_scene_base(&mut self) {
        let world_xform = self.session.edit_path.world_xform();
        let tl = match self.active_symbol_id().and_then(|id| self.doc.library().get(id)) {
            Some(sym) => &sym.timeline,
            None => &self.doc.root,
        };
        if self
            .session
            .base_cache
            .rebuild(tl, self.doc.library(), world_xform)
        {
            self.session.invalidate_scene_order();
        }
    }

    /// 選択が残るかの決まり（決まりはこれ1つ。入口 `apply` が操作の前とあとに呼ぶ）：項目の入っているレイヤーごとに、
    /// 選んだときに見えていた絵（どのキーフレームの絵か・中割りか：`Layer::shown_picture`）を覚え（まだ覚えていないレイヤーは
    /// 今見えている絵）、今見えている絵が違うレイヤーの項目を外す。コマを移る・見る場所を変える・タイムラインを書き換える・
    /// 取り消す、のどれでも同じ決まり（設計審査 b7208d9 の 2.。前は選択を番号だけで持ち、F6 の写しは元と同じ番号なので、道
    /// ごとに別の決まりで外していて、`Shift+F6` のあと選択がコマ 1 の絵に残った）。見える絵が変わるのは、持ち上げた絵を確定
    /// したあとだけ（コマ・見る場所・タイムラインを変える入口が先に確定する）なので、持ち上げた絵の選択は外さない。手間は
    /// 画面の図形の数（選択があるときだけ）
    pub(in crate::canvas_tool) fn settle_selection(&mut self) {
        if self.session.selection.is_empty() {
            self.session.selection.anchors.clear();
            return;
        }
        let tl = self.active_timeline();
        let frame = tl.current_frame;
        let shown = |lid: u64| tl.layers.iter().find(|l| l.id == lid).and_then(|l| l.shown_picture(frame));
        let sel = &self.session.selection;
        let mut layers: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        for s in self.session.scene().iter() {
            let picked = match s.symbol_instance {
                Some(b) => sel.instances.contains(&b.instance_gid),
                None => sel.shapes.contains(&s.shape_id),
            };
            if picked {
                layers.insert(s.layer_id);
            }
        }
        let moved: std::collections::BTreeSet<u64> =
            layers.iter().copied().filter(|&l| sel.anchors.get(&l).is_some_and(|&a| Some(a) != shown(l))).collect();
        let anchors: std::collections::BTreeMap<u64, crate::timeline::ShownPicture> = layers
            .iter()
            .filter(|l| !moved.contains(l))
            .filter_map(|&l| sel.anchors.get(&l).copied().or_else(|| shown(l)).map(|a| (l, a)))
            .collect();
        if !moved.is_empty() {
            let kept: HashSet<u64> = layers.difference(&moved).copied().collect();
            self.session.retain_selection_in_layers(&kept);
        }
        self.session.selection.anchors = anchors;
    }

    /// 浮いた片の操作（選ぶ・持ち上げる・ドラッグ中の見た目・確定・戻し・一部の確定のあと）の土台（文書から作る：編集中の
    /// タイムラインの今のコマのセル画とその場所・レイヤーの重ね順・編集中の階層。浮いた片の操作の土台を作るのはこれ1つ：
    /// `Stage::of`。設計審査 b7208d9 の 8.。前は画面の土台 `SceneBaseCache` から読み、入口の外から浮いた片を扱う道は、土台を
    /// 先に作り直す約束を覚えている必要があった）
    pub(crate) fn stage(&self) -> Stage {
        let path = self.session.edit_path.levels.iter().map(|l| l.symbol_id).collect();
        Stage::of(self.active_timeline(), path, &self.session.editing_group_path)
    }

    /// 指定した `Scene` インデックス列の図形・インスタンスだけを選ぶ（浮いた片の土台は文書から：`Tab::stage`。作業状態の
    /// 選び方は `Session::set_selected_indices` の1つ）
    pub(in crate::canvas_tool) fn set_selected_indices(&mut self, indices: impl IntoIterator<Item = usize>) {
        let stage = self.stage();
        self.session.set_selected_indices(&stage, indices);
    }

    /// 指定した `Scene` インデックス列の図形・インスタンスを選択に足す（`Session::select_shape_indices`）
    pub(in crate::canvas_tool) fn select_shape_indices(&mut self, indices: impl IntoIterator<Item = usize>) {
        let stage = self.stage();
        self.session.select_shape_indices(&stage, indices);
    }

    /// 現在編集中のタイムラインへの不変参照を返す
    #[inline]
    pub(crate) fn active_timeline(&self) -> &Timeline {
        match self.active_symbol_id().and_then(|id| self.doc.library().get(id)) {
            Some(sym) => &sym.timeline,
            None => &self.doc.root,
        }
    }

    /// 編集中のタイムラインの、中身でない状態（見ているコマ・選んだレイヤー・セルの範囲選択。版は上げない）だけを書く窓
    /// （ルートでもシンボルの中でも同じ形：`TimelineView`。中身を書くのは `write_active_timeline`）
    #[inline]
    fn active_timeline_raw_mut(&mut self) -> crate::timeline::TimelineView<'_> {
        match self.active_symbol_id() {
            Some(id) => self.doc.symbol_view_mut(id).expect("編集中のシンボルはライブラリにある"),
            None => self.doc.root.view_mut(),
        }
    }

    /// 編集中タイムラインの再生ヘッド（`current_frame`）を変え、画面の土台を作り直す（版は上げない）。コマを変える入口は
    /// これ1つ（再生中に進める `play_to_frame` は、持ち上げた絵があれば断る）：先に持ち上げた絵を貼り
    /// 戻し（持ち上げた絵は今のコマのセル画から持ち上げたもの）、断られたら変えずに `false`（設計
    /// claude/約束を入口で守る設計_5616fcc.md の 3.）
    #[must_use]
    fn set_active_frame(&mut self, frame: usize) -> bool {
        if !commit_floating(self) {
            return false;
        }
        *self.active_timeline_raw_mut().current_frame = frame;
        self.refresh_scene_base();
        true
    }

    /// 見る場所（シンボルの中の位置 `Session::edit_path`・グループの階層 `Session::editing_group_path`）を `f` で変える。見る
    /// 場所を変える入口はこれ1つ（取り消しで状態をまるごと戻す `restore_state_snapshot` を除く）：先に持ち上げた絵を貼り戻し
    /// （持ち上げた絵は、今の見る場所の地図から持ち上げたもの）、断られたら変えずに `false`（設計
    /// claude/約束を入口で守る設計_5616fcc.md の 3.）
    #[must_use]
    fn move_view(&mut self, f: impl FnOnce(&mut EditPath, &mut Vec<u64>)) -> bool {
        if !commit_floating(self) {
            return false;
        }
        f(&mut self.session.edit_path, &mut self.session.editing_group_path);
        true
    }

    /// 再生がコマを `frame` へ進める（部屋の外から見ている場所を変える入口はこれ1つ）。見ている場所だけを変え、画面の土台は
    /// 作り直さない（再生中はセル画の結合メッシュの写しから描く）。持ち上げた絵があれば変えずに `false`（持ち上げた絵を貼り
    /// 戻すのは作品を書くことなので、部屋の中の入口だけがする。再生を始める所は、先に入口で選択を外してある：設計
    /// 入口の部屋の設計_27c4085 の 2.1）
    #[must_use]
    pub(crate) fn play_to_frame(&mut self, frame: usize) -> bool {
        if self.session.floating().is_some() {
            return false;
        }
        *self.active_timeline_raw_mut().current_frame = frame;
        true
    }

    /// 編集中タイムラインの選択レイヤー（`active_layer`）のみを変更し、リビジョンは上げない
    fn set_active_layer(&mut self, idx: usize) {
        let tl = self.active_timeline_raw_mut();
        if tl.layer_count > 0 {
            *tl.active_layer = idx.min(tl.layer_count - 1);
        }
    }

    /// 編集中タイムラインのセル範囲選択（`cell_selection`）のみを変更し、リビジョンは上げない
    fn set_active_cell_selection(&mut self, sel: Option<TimelineCellSelection>) {
        *self.active_timeline_raw_mut().cell_selection = sel;
    }

    /// 現在編集中のタイムラインへの可変参照と文書への可変参照を渡して絵を変える編集を実行し（`write_active_timeline`）、
    /// 画面の土台を作り直す
    fn with_active_timeline_mut<R>(
        &mut self,
        f: impl FnOnce(&mut Timeline, &mut crate::symbol::DocParts<'_>) -> R,
    ) -> R {
        let res = self.write_active_timeline(f);
        self.refresh_scene_base();
        res
    }

    /// 現在編集中のタイムラインを `f(タイムライン, 文書)` で書く。編集中のタイムラインの中身を書く所はこれ1つ：シンボルの
    /// 中なら、版を上げる入口 `Document::write_symbol_timeline` を通る（ルートでは `Document::revision` は上げない）。
    /// 画面の土台は作り直さない（書いた後の後始末は呼ぶ側：`with_active_timeline_mut`・`finish_cel_writes`）
    fn write_active_timeline<R>(
        &mut self,
        f: impl FnOnce(&mut Timeline, &mut crate::symbol::DocParts<'_>) -> R,
    ) -> R {
        if let Some(id) = self.active_symbol_id() {
            return self.doc.write_symbol_timeline(id, f);
        }
        let (root, mut parts) = self.doc.root_and_parts();
        f(root, &mut parts)
    }

    /// シンボルの中を編集しているときの、親の背景（親の段ごと、下のレイヤーから順に、薄く描く画面座標の結合メッシュ。
    /// 編集しているインスタンスは除く）。写し（`CelMeshCache`）から。ルートでは空
    pub(crate) fn cached_backdrop_screen_meshes(
        &self,
        zoom: f32,
        pan: Vec2,
        visible: KRect,
    ) -> Vec<Arc<egui::Mesh>> {
        let edit_path = &self.session.edit_path;
        let mut meshes = Vec::new();
        let mut cache = self.session.backdrop_cache.lock();
        // 動かした浮いた片があれば、それを確定した後の文書から描く（同じシンボルのほかのインスタンスにも、確定した後の形が
        // 映る。グループを動かしたときは文書に書くので映る：`Session::backdrop_document`）
        let committed = self.backdrop_document();
        let mut draw = |doc: &Document, ctx: SymbolEvalCtx<'_>| {
            for lvl_idx in edit_path.first_level_showing_parent()..edit_path.levels.len() {
                let lvl = &edit_path.levels[lvl_idx];
                let (timeline, parent_tl) = if lvl_idx == 0 {
                    (None, &doc.root)
                } else {
                    let sym_id = edit_path.levels[lvl_idx - 1].symbol_id;
                    let Some(sym) = doc.find_symbol(sym_id) else {
                        continue;
                    };
                    (Some(sym_id), &sym.timeline)
                };
                let scope = CelScope {
                    timeline,
                    excluded_instance: Some(lvl.instance_gid),
                };
                let world_xform = edit_path.levels[..lvl_idx]
                    .iter()
                    .fold(Affine::IDENTITY, |acc, l| acc * l.xform_in_parent);
                let frame = lvl
                    .parent_frame
                    .min(parent_tl.total_frames.saturating_sub(1));
                for (layer_idx, layer) in parent_tl.layers.iter().enumerate() {
                    if !layer.visible {
                        continue;
                    }
                    meshes.extend(cache.screen_mesh(
                        parent_tl,
                        scope,
                        ctx.clone(),
                        layer_idx,
                        frame,
                        world_xform,
                        zoom,
                        pan,
                        visible,
                        CelTint::Dim,
                    ));
                }
            }
        };
        match &committed {
            Some(doc) => draw(doc, SymbolEvalCtx::new(doc.library())),
            None => self.with_symbol_eval(|ctx| draw(&self.doc, ctx)),
        }
        meshes
    }

    /// 親の背景を描く文書：シンボルの中を編集していて、動かした浮いた片（`modified`）があれば、それを確定した後の文書
    /// （`committed_document`）。それ以外は `None`（今の文書から描く）。確定した後の文書は、浮いた片か文書が変わったとき
    /// だけ作り直す（`Session::floating_rev`・`Document::revision`・見ている場所を鍵に持つ。前は見ている場所を鍵に入れず、
    /// 見る場所を変える前に貼り戻す決まりに頼っていた：約束の棚卸し 5616fcc の B の B6）。浮いた片を確定できないとき
    /// （置けない）は、確定する前の今の文書のまま
    fn backdrop_document(&self) -> Option<Arc<Document>> {
        let fl = self.session.floating().filter(|fl| fl.modified())?;
        if self.session.edit_path.levels.is_empty() {
            return None;
        }
        let key = (
            self.session.floating_rev,
            self.doc.revision,
            self.session.edit_path.clone(),
            self.session.editing_group_path.clone(),
        );
        let mut slot = self.session.backdrop_doc.lock();
        if let Some((k, doc)) = &*slot {
            if *k == key {
                return doc.clone();
            }
        }
        let doc = committed_document(
            (self.id, self.untitled_number),
            &self.doc,
            &self.session.edit_path,
            &self.session.editing_group_path,
            Some(fl),
        )
        .map(Arc::new);
        *slot = Some((key, doc.clone()));
        doc
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
        visible: KRect,
        (tint_rgb, alpha): ([u8; 3], u8),
    ) -> Option<Arc<egui::Mesh>> {
        self.cel_screen_mesh(
            &self.session.onion_cache,
            layer_idx,
            frame,
            (zoom, pan, visible),
            CelTint::Onion(tint_rgb, alpha),
        )
    }

    /// オニオンスキンで描くコマの範囲（今のコマの前後。タイムラインの中に切り詰める）。描く所（`panels::draw_onion_skins`）
    /// と先読み（`Is2dApp::prefetch_next_cels`）の範囲の決まりはこれ1つ
    pub(crate) fn onion_window(&self) -> (usize, usize) {
        let tl = self.active_timeline();
        let curr = tl.current_frame;
        let first = curr.saturating_sub(self.view.onion_prev_frames);
        let last = (curr + self.view.onion_next_frames).min(tl.total_frames.saturating_sub(1));
        (first, last)
    }

    /// オニオンスキンで描くセル画 `cels`（レイヤーの番号、コマ）のうち写しの無いものを、描く前に同時に作る
    /// （`CelMeshCache::prepare`）
    pub(crate) fn prepare_onion_cels(&self, cels: &[(usize, usize)]) {
        let world_xform = self.session.edit_path.world_xform();
        self.with_symbol_eval(|ctx| {
            self.session.onion_cache.lock().prepare(
                self.active_timeline(),
                self.active_cel_scope(),
                ctx,
                cels,
                world_xform,
            );
        });
    }

    /// 編集しているタイムラインのセル画の所（ルートか、編集しているシンボル。除くインスタンスは無い）
    fn active_cel_scope(&self) -> CelScope {
        CelScope {
            timeline: self.session.edit_path.last().map(|l| l.symbol_id),
            excluded_instance: None,
        }
    }

    /// ロック中・再生中のレイヤー `layer_idx` のコマ `frame` のセル画の画面座標の結合メッシュ（写しから：`CelMeshCache`）
    pub(crate) fn cached_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        visible: KRect,
        tint: CelTint,
    ) -> Option<Arc<egui::Mesh>> {
        self.cel_screen_mesh(&self.session.cel_cache, layer_idx, frame, (zoom, pan, visible), tint)
    }

    /// ロック中のレイヤー `layer_idx` のコマ `frame` のセル画の、点 `pt`（ステージの座標）で見えているもの（色と、線なら
    /// その線。描くのと同じ写しの図形を、上に描くものから順に点の内外の判定でたどる：`CelMeshCache::seen_at`）
    pub(in crate::canvas_tool) fn cached_cel_seen_at(
        &self,
        layer_idx: usize,
        frame: usize,
        pt: Point,
    ) -> Option<(Color32, Option<crate::planar_map::EdgeStroke>)> {
        let tl = self.active_timeline();
        let world_xform = self.session.edit_path.world_xform();
        let scope = self.active_cel_scope();
        self.with_symbol_eval(|ctx| {
            self.session.cel_cache.lock().seen_at(
                tl,
                scope,
                ctx,
                layer_idx,
                frame,
                world_xform,
                pt,
            )
        })
    }

    fn cel_screen_mesh(
        &self,
        cache: &SyncCache<CelMeshCache>,
        layer_idx: usize,
        frame: usize,
        (zoom, pan, visible): (f32, Vec2, KRect),
        tint: CelTint,
    ) -> Option<Arc<egui::Mesh>> {
        let tl = self.active_timeline();
        let world_xform = self.session.edit_path.world_xform();
        let scope = self.active_cel_scope();
        self.with_symbol_eval(|ctx| {
            cache.lock().screen_mesh(
                tl,
                scope,
                ctx,
                layer_idx,
                frame,
                world_xform,
                zoom,
                pan,
                visible,
                tint,
            )
        })
    }

    /// 1回の描画の始めに呼ぶ：シンボルの中身のメモ（`SymbolEvalState::start_pass`）と、セル画の結合メッシュの写し
    /// （オニオンスキン・ロック中や再生中のレイヤー・親の背景の `CelMeshCache::start_pass`）の、直前の描画で使わなかったものを捨てる
    pub(crate) fn start_draw_pass(&self) {
        self.session
            .symbol_memo
            .lock()
            .start_pass();
        self.session.onion_cache.lock().start_pass();
        self.session.cel_cache.lock().start_pass();
        self.session.backdrop_cache.lock().start_pass();
    }

    /// 描くたびのシンボルの評価を、セッションのメモ（`Session::symbol_memo`）を使って行う
    pub(in crate::canvas_tool) fn with_symbol_eval<R>(&self, f: impl FnOnce(SymbolEvalCtx<'_>) -> R) -> R {
        let state = std::mem::take(&mut *self.session.symbol_memo.lock());
        let ctx = SymbolEvalCtx::with_state(self.doc.library(), state);
        let out = f(ctx.clone());
        *self.session.symbol_memo.lock() = ctx.into_state();
        out
    }

    /// ダーティマーク（` *`）を除いたタブのベース表示名を返す（ファイル名。無ければ `name_stem` の「無題-番号」）
    pub(crate) fn base_title(&self) -> String {
        match self
            .file
            .as_deref()
            .and_then(std::path::Path::file_name)
            .and_then(|n| n.to_str())
        {
            Some(name) if !name.is_empty() => name.to_owned(),
            _ => self.name_stem(),
        }
    }

    /// タブの名前の元：ファイル名の拡張子を除いた所。無ければ「無題-番号」。保存と動画の書き出しの初めのファイル名は、
    /// これに拡張子を付ける（無題の名前の決まりはこれ1つ：レビュー de92ceb の N-5003）
    pub(crate) fn name_stem(&self) -> String {
        match self
            .file
            .as_deref()
            .and_then(std::path::Path::file_stem)
            .and_then(|s| s.to_str())
        {
            Some(stem) if !stem.is_empty() => stem.to_owned(),
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

    /// ライブラリで選んでいるシンボル（`View::library_symbol`）
    pub(crate) fn library_symbol(&self) -> Option<u64> {
        self.view.library_symbol(self.doc.library())
    }

    /// シンボル `sym_id` とそのインスタンスをみな消す（シンボルを消す入口はこれ1つ）。消すシンボルの段にいれば出て、選択を
    /// 外してから消す（前は消す操作が段から出るのを手で書いた：約束の棚卸し 5616fcc の A の P-19。ライブラリで選んでいる
    /// シンボルは、読む所 `View::library_symbol` が決めるので直さない）。段から出られない・選択を外せない（浮いた片の確定が
    /// 断られた）ときは、文書を変えずに断る
    fn remove_symbol(&mut self, sym_id: u64) -> bool {
        if self.doc.find_symbol(sym_id).is_none() {
            return false;
        }
        while self.session.edit_path.levels.iter().any(|lvl| lvl.symbol_id == sym_id) {
            if !crate::symbol_ops::exit_symbol_timeline(self) {
                return false;
            }
        }
        if !clear_selection(self) {
            return false;
        }
        self.doc.remove_symbol_and_instances(sym_id);
        self.refresh_scene_base();
        reset_session_after_timeline_change(self)
    }

    /// 浮いている片（`Floating`）がある場合は一時タブ上で確定した `Document` の写しを返し、元のタブや文書には触れない。
    /// `None`：浮いた片を置けない（`commit_floating_layers` が断る。文書は、片を持ち上げる前のまま）
    pub(crate) fn committed_document_copy(&self) -> Option<Document> {
        committed_document(
            (self.id, self.untitled_number),
            &self.doc,
            &self.session.edit_path,
            &self.session.editing_group_path,
            self.session.floating(),
        )
    }

    fn restore_state_snapshot(&mut self, entry: UndoEntry) -> UndoEntry {
        self.history.last_coalesce_key = None;
        let prev_floating = self.session.shared_floating();
        // 控えと文書の項目は全部書く（どちらかに項目を足すと、ここが組み立てで止まるので、戻すか戻さないかを必ず決める。
        // 控えを作る所 `doc_actions::undo_snapshot` も同じ：設計審査 de92ceb の D-5 の (3)）。戻さないもの：番号の数え札・
        // 壊れていた地図の知らせ先（`undo_snapshot` を見よ）
        let UndoEntry {
            timeline,
            library,
            sounds,
            floating,
            selection,
            edit_path,
            editing_group_path,
            stage_size,
            fps,
            symbols_revision,
            commit_id,
        } = entry;
        // 文書の中身を入れ替える（項目は全部書く：`Document::replace_content`）
        let prev_doc = self.doc.replace_content(crate::symbol::DocContent {
            root: timeline,
            library,
            sounds,
            stage_size,
            fps,
            revision: symbols_revision,
        });
        let prev_entry = UndoEntry {
            timeline: prev_doc.root,
            library: prev_doc.library,
            sounds: prev_doc.sounds,
            floating: prev_floating,
            selection: std::mem::replace(&mut self.session.selection, selection),
            edit_path: std::mem::replace(&mut self.session.edit_path, edit_path),
            editing_group_path: std::mem::replace(
                &mut self.session.editing_group_path,
                editing_group_path,
            ),
            stage_size: prev_doc.stage_size,
            fps: prev_doc.fps,
            symbols_revision: prev_doc.revision,
            commit_id: std::mem::replace(&mut self.history.current_commit_id, commit_id),
        };
        self.doc.validate_edit_path(
            &mut self.session.edit_path,
            &mut self.session.editing_group_path,
        );
        cancel_active_drag(self);
        self.refresh_scene_base();
        let stage = self.stage();
        self.session.set_floating(floating.map(Arc::unwrap_or_clone), &stage);
        self.session.retain_existing_selection();
        if self.session.floating().is_some() {
            // 確定を断ったら、浮いた片と選択はそのまま（知らせは `apply`）
            let keep = self.session.selected_indices();
            let _ = commit_floating_except(self, &keep);
        }
        self.session.sanitize_editing_scope();
        prev_entry
    }

    fn undo(&mut self) -> bool {
        cancel_active_drag(self);
        if let Some(prev_entry) = self.history.pop_undo() {
            let curr = self.restore_state_snapshot(prev_entry);
            self.history.push_redo(curr);
            true
        } else {
            false
        }
    }

    fn redo(&mut self) -> bool {
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
