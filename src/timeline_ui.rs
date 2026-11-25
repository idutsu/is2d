use std::time::Duration;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::audio_out::AudioOut;
use crate::canvas_tool::{cancel_active_drag, CoalesceKey, Tab};
use crate::doc_actions::{apply, Action, ActionHost};
#[cfg(test)]
pub(crate) use crate::doc_actions::{
    scrub_ruler_to_frame, select_timeline_cells, set_layer_locked, set_layer_visible,
    switch_to_layer_and_frame, timeline_add_layer, timeline_delete_active_layer,
    timeline_delete_keyframe, timeline_extend_frame, timeline_insert_keyframe,
    timeline_move_or_copy_frames, timeline_remove_frame, timeline_reorder_layer,
};
use crate::sound::{frames_to_sound, SoundEdge};
use crate::symbol::TweenState;
use crate::timeline::{Layer, TimelineCellSelection};

#[cfg(test)]
thread_local! {
    /// テスト用：このスレッドで、タイムラインを描くときに調べたキーフレームの数
    static KEYFRAMES_EXAMINED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// テスト用：このスレッドで、タイムラインを描くときに調べたキーフレームの数
#[cfg(test)]
pub(crate) fn keyframes_examined() -> usize {
    KEYFRAMES_EXAMINED.with(|n| n.get())
}

/// タイムライングリッドの1フレームあたりのセル幅（px）
pub(crate) const CELL_W: f32 = 16.0;

/// 波形の絵の山の表の細かさ（`SOUND_RATE` の単位の長さ：`Sound::peak`）：タイムラインの画面の 1px に当たる音のいちばん
/// 短い長さ。1コマの幅 `CELL_W` に、いちばん短いコマ（fps の上限 `MAX_FPS` の1コマ）が入るので、1px は
/// 1 / (`MAX_FPS` × `CELL_W`) 秒（60fps・16px で 50）。これより細かく持っても画面に出ない
pub(crate) const WAVE_UNIT: u64 =
    crate::sound::SOUND_RATE / (crate::symbol::MAX_FPS as u64 * CELL_W as u64);

const LAYER_COL_W: f32 = 190.0;
const RULER_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const LAYER_VIS_HIT_RANGE: std::ops::Range<f32> = 6.0..30.0;
const LAYER_LOCK_HIT_RANGE: std::ops::Range<f32> = 30.0..54.0;
const LAYER_VIS_ICON_X: f32 = 18.0;
const LAYER_LOCK_ICON_X: f32 = 42.0;
const LAYER_NAME_X: f32 = 60.0;
/// レイヤー行の右端の、つかんで並べ替える印（≡）の幅
const LAYER_GRIP_W: f32 = 24.0;
/// ドラッグ中の自動スクロールの速さ：端の所に入った深さ 1px あたり、1秒に何 px スクロールするか（縦は、端そのもの
/// （深さ1行分）で1秒に 12 行ほど。横は、深さ1コマ分で1秒に 12 コマほど）
const DRAG_SCROLL_RATE: f32 = 12.0;
/// レイヤー名の文字の大きさ（行に描く名前と、名前の入力欄で同じ）
const LAYER_NAME_FONT_SIZE: f32 = 12.5;
/// レイヤー名の入力欄の、文字のまわりの余白（左右・上下。画面の px）
const LAYER_RENAME_PAD: Vec2 = Vec2::new(3.0, 2.0);

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
    pub(crate) const SOUND: &str = "\u{e3a1}"; // audiotrack (音)
    pub(crate) const WIREFRAME: &str = "\u{e922}"; // timeline (アンカー・パス表示)
    pub(crate) const DELETE: &str = "\u{e872}"; // delete (削除)
    pub(crate) const COPY: &str = "\u{e14d}"; // content_copy (複製)
    pub(crate) const SWAP: &str = "\u{ea18}"; // sync_alt (入れ替え)
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
    pub(crate) const REPEAT: &str = "\u{e040}"; // repeat (ループ再生)
}

// ============================================================================
// 0. タイムラインUI状態とホスト抽象化
// ============================================================================

/// タブ状態（`Tab`）とタイムラインUI状態（`TimelineUiState`）の組を抽象化するトレイト
pub(crate) trait TimelineHost: ActionHost {
    fn timeline_ui(&self) -> &TimelineUiState;
    fn timeline_ui_mut(&mut self) -> &mut TimelineUiState;
    fn tab_and_timeline_ui_mut(&mut self) -> (&mut Tab, &mut TimelineUiState);
}

/// タイムラインのスクロールバーの向き
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollAxis {
    X,
    Y,
}

impl ScrollAxis {
    #[inline]
    fn of(self, pos: Pos2) -> f32 {
        match self {
            Self::X => pos.x,
            Self::Y => pos.y,
        }
    }
}

/// タイムライン右側領域（ルーラー・フレームグリッド・スクロールバー）でのマウスドラッグ状態
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum TimelineGridDrag {
    #[default]
    None,
    /// 上部フレーム番号ルーラーのドラッグによる再生ヘッドスクラブ（レイヤー個別選択を解除）
    ScrubRuler,
    /// 横・縦スクロールバーのドラッグ操作
    ScrubScrollbar { axis: ScrollAxis, grab_offset: f32 },
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
    /// 音の波形の端 `edge` のドラッグ（レイヤー `layer` の、コマ `kf_start` に始まるキーフレームの音）。`from` はつかんだ
    /// ときの端のコマの境目、`frame` は今の（動かせる所に収めた）コマの境目。離すまで文書は変えない
    TrimSound {
        layer: usize,
        kf_start: usize,
        edge: SoundEdge,
        from: usize,
        frame: usize,
    },
    /// オニオンスキンの範囲の端のドラッグ（`Prev` は前の範囲の左端、`Next` は後ろの範囲の右端）。今のコマから端までの
    /// コマ数を、ドラッグに合わせて変える（表示の設定なので、文書も Undo も変えない）
    OnionEdge(OnionSide),
}

/// オニオンスキンの範囲のどちらの端か
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnionSide {
    Prev,
    Next,
}

/// オニオンスキンで透かす前後のコマ数の上限
const ONION_MAX_FRAMES: usize = 10;

/// タイムラインの識別キー `(doc_id, edit_stack_len, active_symbol_id)`（`timeline_key`）
type TimelineKey = (u64, usize, Option<u64>);

/// タブ `tab` の今のタイムラインの識別キー（タブの切り替え・シンボルの出入りで変わる）
fn timeline_key(tab: &Tab) -> TimelineKey {
    (
        tab.id,
        tab.session.edit_path.len(),
        tab.session.edit_path.last().map(|s| s.symbol_id),
    )
}

/// タイムラインパネルの再生状態・ループ設定・スクロール位置・ドラッグ操作等のUI状態
pub(crate) struct TimelineUiState {
    pub(crate) is_playing: bool,
    pub(crate) loop_playback: bool,
    /// 再生とスクラブの音を鳴らし、再生のコマを決める数を数える（音の時計）
    pub(crate) audio: AudioOut,
    /// 直近の画面で見た、タイムラインとコマ（コマが変わったらスクラブで鳴らす：`scrub_on_frame_change`）
    heard: Option<(TimelineKey, usize)>,
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f32,
    pub(crate) last_ensured_frame: usize,
    pub(crate) dragging_layer: Option<usize>,
    /// 名前を書き換えているレイヤー（どのタイムラインの、どの番号のレイヤーか）。名前をダブルクリックで始まり、入力欄から
    /// 離れると終わる
    renaming_layer: Option<(TimelineKey, u64)>,
    pub(crate) grid_drag: TimelineGridDrag,
    /// ドラッグ中の、直近の画面の終わりの文書の世代（`UndoHistory::current_commit_id`）。ドラッグ中でなければ `None`
    drag_generation: Option<u64>,
    /// 直近に同期したタイムライン識別キー
    last_timeline_key: TimelineKey,
}

impl Default for TimelineUiState {
    fn default() -> Self {
        Self {
            is_playing: false,
            loop_playback: true,
            audio: AudioOut::silent(),
            heard: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            last_ensured_frame: 0,
            dragging_layer: None,
            renaming_layer: None,
            grid_drag: TimelineGridDrag::None,
            drag_generation: None,
            last_timeline_key: (1, 0, None),
        }
    }
}

impl TimelineUiState {
    pub(crate) fn cancel_drag(&mut self) {
        self.dragging_layer = None;
        self.grid_drag = TimelineGridDrag::None;
        self.drag_generation = None;
    }

    /// ドラッグ中に、ほかの所（キー・メニュー・Undo など）で文書が変わっていたら（世代 `generation` が、直近の画面の終わりに
    /// 覚えた世代と違う）、ドラッグを捨てる。ドラッグが持つレイヤー・コマの番号は、始めたときの文書のものなので、変わった
    /// 文書に当てると、別のレイヤー・キーフレームを書き換える（レビュー f478c69 の N-5003）
    pub(crate) fn drop_drag_if_document_changed(&mut self, generation: u64) {
        if self.drag_generation.is_some_and(|g| g != generation) {
            self.cancel_drag();
        }
    }

    /// 画面の終わりに、ドラッグ中なら今の文書の世代 `generation` を覚える（ドラッグ自身が文書を変えたあとの世代）
    pub(crate) fn remember_drag_generation(&mut self, generation: u64) {
        let dragging = self.dragging_layer.is_some() || !matches!(self.grid_drag, TimelineGridDrag::None);
        self.drag_generation = dragging.then_some(generation);
    }

    pub(crate) fn reset_for_timeline_switch(&mut self) {
        self.is_playing = false;
        self.audio.stop();
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.last_ensured_frame = usize::MAX;
        self.cancel_drag();
    }

    /// 今のタイムライン（ルートか、編集中のシンボルの）を、今のコマから鳴らして再生する（くり返すかは `loop_playback`）。
    /// 再生のコマは鳴らした数から決める（`update_playback`）ので、fps・くり返しを変えたときも、ここで今のコマから鳴らし直す
    fn start_playback(&mut self, tab: &Tab) {
        let tl = tab.active_timeline();
        let end = tl.total_frames.max(1);
        self.audio
            .play(&tab.doc, tl, tl.current_frame, end, self.loop_playback);
        self.is_playing = true;
    }

    /// 再生中であれば停止し、停止時点のフレームの状態に合わせて編集スコープを整合させる
    pub(crate) fn stop_playback(&mut self, tab: &mut Tab) {
        if self.is_playing {
            self.is_playing = false;
            self.audio.stop();
            tab.refresh_scene_base();
            tab.session.sanitize_editing_scope();
        }
    }

    /// `tab` の現在のタイムライン識別キー `(id, edit_stack_len, active_symbol_id)` を読み取り、
    /// シンボル進入/退出やタブ切り替えが発生していればスクロール・ドラッグ状態を自動リセットする
    pub(crate) fn ensure_timeline_context(&mut self, tab: &Tab) {
        let key = timeline_key(tab);
        if self.last_timeline_key != key {
            self.last_timeline_key = key;
            self.reset_for_timeline_switch();
        }
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

// ============================================================================
// 1. タイムラインUIのレイアウト計算・スクロール追従
// ============================================================================

/// タイムライン下部パネルの領域座標・スクロール変換を一元管理するレイアウト構造体
struct TimelineLayout {
    panel_rect: Rect,
    grid_left: f32,
    grid_top: f32,
    grid_w: f32,
    grid_h: f32,
    max_frames_shown: usize,
    first_vis_frame: usize,
    last_vis_frame: usize,
    add_hdr_rect: Rect,
    del_hdr_rect: Rect,
}

impl TimelineLayout {
    fn new(panel_rect: Rect, max_reach_frames: usize, scroll_x: f32) -> Self {
        let grid_left = panel_rect.left() + LAYER_COL_W;
        let grid_top = panel_rect.top() + RULER_H;
        let grid_w = (panel_rect.right() - grid_left).max(1.0);
        let grid_h = (panel_rect.bottom() - grid_top).max(1.0);
        let max_frames_shown = (max_reach_frames + 24).max(3000);

        let add_hdr_rect = Rect::from_min_max(
            Pos2::new(grid_left - 44.0, panel_rect.top() + 2.0),
            Pos2::new(grid_left - 24.0, grid_top - 2.0),
        );
        let del_hdr_rect = Rect::from_min_max(
            Pos2::new(grid_left - 22.0, panel_rect.top() + 2.0),
            Pos2::new(grid_left - 2.0, grid_top - 2.0),
        );

        let mut layout = Self {
            panel_rect,
            grid_left,
            grid_top,
            grid_w,
            grid_h,
            max_frames_shown,
            first_vis_frame: 0,
            last_vis_frame: 0,
            add_hdr_rect,
            del_hdr_rect,
        };
        layout.sync_visible_frames(scroll_x);
        layout
    }

    fn frame_at_x(&self, x: f32, scroll_x: f32) -> usize {
        (((x - self.grid_left + scroll_x) / CELL_W).floor() as isize).max(0) as usize
    }

    fn row_at_y(&self, y: f32, scroll_y: f32) -> isize {
        ((y - self.grid_top + scroll_y) / ROW_H).floor() as isize
    }

    fn layer_at_y(&self, y: f32, scroll_y: f32, num_layers: usize) -> Option<usize> {
        let row = self.row_at_y(y, scroll_y);
        (row >= 0 && (row as usize) < num_layers).then(|| num_layers - 1 - (row as usize))
    }

    /// 横のスクロールのいちばん大きい量（出しているコマのうち、見えていない分の幅）
    fn max_scroll_x(&self) -> f32 {
        ((self.max_frames_shown as f32) * CELL_W - self.grid_w).max(0.0)
    }

    /// 縦のスクロールのいちばん大きい量（`num_layers` 行のうち、見えていない分の高さ）
    fn max_scroll_y(&self, num_layers: usize) -> f32 {
        ((num_layers as f32) * ROW_H - self.grid_h).max(0.0)
    }

    fn clamped_layer_at_y(&self, y: f32, scroll_y: f32, num_layers: usize) -> usize {
        let row = self
            .row_at_y(y, scroll_y)
            .clamp(0, (num_layers.saturating_sub(1)) as isize) as usize;
        num_layers.saturating_sub(1 + row)
    }

    fn frame_x(&self, frame: usize, scroll_x: f32) -> f32 {
        self.grid_left + (frame as f32) * CELL_W - scroll_x
    }

    fn row_y(&self, row: usize, scroll_y: f32) -> f32 {
        self.grid_top + (row as f32) * ROW_H - scroll_y
    }

    fn sync_visible_frames(&mut self, scroll_x: f32) {
        self.first_vis_frame = (scroll_x / CELL_W).floor() as usize;
        self.last_vis_frame =
            (((scroll_x + self.grid_w) / CELL_W).ceil() as usize).min(self.max_frames_shown);
    }

    /// スクロールバーのつまみを掴んだ位置と現在のポインタ位置からスクロール量を計算する
    fn scroll_from_thumb(
        &self,
        axis: ScrollAxis,
        pos: Pos2,
        grab_offset: f32,
        thumb_len: f32,
        max_scroll: f32,
    ) -> f32 {
        let (view_len, start) = match axis {
            ScrollAxis::X => (self.grid_w, self.grid_left),
            ScrollAxis::Y => (self.grid_h, self.grid_top),
        };
        let track_span = (view_len - thumb_len).max(1.0);
        ((axis.of(pos) - grab_offset - start) / track_span).clamp(0.0, 1.0) * max_scroll
    }

    /// 指定した軸のスクロールバーの当たり判定矩形・つまみ矩形・つまみ長・最大スクロール量を返す（不要なら `None`）
    fn scrollbar_metrics(
        &self,
        axis: ScrollAxis,
        num_layers: usize,
        scroll: f32,
    ) -> Option<(Rect, Rect, f32, f32)> {
        let (total, view_len, start, min_thumb) = match axis {
            ScrollAxis::X => (
                (self.max_frames_shown as f32) * CELL_W,
                self.grid_w,
                self.grid_left,
                24.0,
            ),
            ScrollAxis::Y => (
                (num_layers as f32) * ROW_H,
                self.grid_h,
                self.grid_top,
                20.0,
            ),
        };
        if total <= view_len || view_len <= 16.0 {
            return None;
        }
        let thumb_len = ((view_len / total) * view_len).clamp(min_thumb, view_len);
        let max_scroll = (total - view_len).max(1.0);
        let ratio = (scroll / max_scroll).clamp(0.0, 1.0);
        let thumb_0 = start + ratio * (view_len - thumb_len);
        let (hit_rect, thumb_rect) = match axis {
            ScrollAxis::X => (
                Rect::from_min_max(
                    Pos2::new(
                        self.grid_left,
                        (self.panel_rect.bottom() - 9.0).max(self.grid_top),
                    ),
                    Pos2::new(self.panel_rect.right() - 8.0, self.panel_rect.bottom()),
                ),
                Rect::from_min_max(
                    Pos2::new(thumb_0, self.panel_rect.bottom() - 6.0),
                    Pos2::new(thumb_0 + thumb_len, self.panel_rect.bottom() - 1.0),
                ),
            ),
            ScrollAxis::Y => (
                Rect::from_min_max(
                    Pos2::new(
                        (self.panel_rect.right() - 9.0).max(self.grid_left),
                        self.grid_top,
                    ),
                    Pos2::new(self.panel_rect.right(), self.panel_rect.bottom()),
                ),
                Rect::from_min_max(
                    Pos2::new(self.panel_rect.right() - 6.0, thumb_0),
                    Pos2::new(self.panel_rect.right() - 1.0, thumb_0 + thumb_len),
                ),
            ),
        };
        Some((hit_rect, thumb_rect, thumb_len, max_scroll))
    }
}

// ============================================================================
// 2. 下部タイムラインパネル UI（コントロールバー・入力処理・各領域描画）
// ============================================================================

/// 画面下部の Adobe Animate 風タイムラインパネルを描画・操作する（ライトテーマ・角丸なし）
pub(crate) fn draw_timeline_panel(host: &mut impl TimelineHost, ctx: &egui::Context) {
    {
        let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
        timeline_ui.ensure_timeline_context(tab);
    }
    egui::TopBottomPanel::bottom("timeline_panel")
        .resizable(true)
        .default_height(186.0)
        .height_range(124.0..=360.0)
        .frame(
            egui::Frame::NONE
                .fill(Color32::from_rgb(236, 238, 242))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(190, 196, 206))),
        )
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(Color32::from_rgb(35, 40, 48));
            draw_timeline_control_bar(host, ui);

            let (panel_rect, response) =
                ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
            let painter = ui.painter_at(panel_rect);

            let current_frame = host.tab().active_timeline().current_frame;
            host.timeline_ui_mut().ensure_current_frame_visible(
                current_frame,
                (panel_rect.width() - LAYER_COL_W).max(1.0),
            );

            let tl = host.tab().active_timeline();
            let sel_max = tl.cell_selection.map(|s| s.max_frame + 1).unwrap_or(0);
            let max_reach_frames = tl.total_frames.max(current_frame + 1).max(sel_max);
            let mut layout =
                TimelineLayout::new(panel_rect, max_reach_frames, host.timeline_ui().scroll_x);
            handle_timeline_scroll(host, ui, &response, &mut layout);

            let primary_down = ui.input(|i| i.pointer.primary_down());
            let alt_held = ui.input(|i| i.modifiers.alt);
            let pointer_pos = ui
                .input(|i| i.pointer.interact_pos())
                .or_else(|| response.interact_pointer_pos());
            let hover_pos = ui.input(|i| i.pointer.hover_pos());
            let ptr = (primary_down, alt_held, pointer_pos, hover_pos);
            let drag_layer_feedback =
                handle_timeline_pointer_input(host, ui, &response, &mut layout, ptr);

            let grid_rect = Rect::from_min_max(
                Pos2::new(layout.grid_left, layout.grid_top),
                panel_rect.right_bottom(),
            );
            let grid_painter = painter.with_clip_rect(grid_rect);

            draw_timeline_frame_grid(host, &grid_painter, &layout, alt_held);
            draw_timeline_ruler(host, &painter, &grid_painter, &layout);
            draw_timeline_layer_list(
                host,
                &painter,
                &layout,
                primary_down,
                hover_pos,
                drag_layer_feedback,
            );
            draw_timeline_scrollbars(host, &painter, &layout);
            draw_layer_rename_field(host, ui, &layout);
        });
}

/// レイヤー名の入力欄の番号（ダブルクリックで、この欄に先に入力を向けて、名前を全部選んでおく）
fn layer_rename_id(layer_id: u64) -> egui::Id {
    egui::Id::new(("layer_rename", layer_id))
}

/// 位置 `pos`（レイヤー列の名前の所）のレイヤーの名前を書き換え始める
fn start_layer_rename(
    host: &mut impl TimelineHost,
    ctx: &egui::Context,
    layout: &TimelineLayout,
    pos: Pos2,
) {
    let tl = host.tab().active_timeline();
    let in_names = pos.x < layout.grid_left
        && pos.y >= layout.grid_top
        && pos.x - layout.panel_rect.left() >= LAYER_LOCK_HIT_RANGE.end;
    let Some(layer) = in_names
        .then(|| layout.layer_at_y(pos.y, host.timeline_ui().scroll_y, tl.layers.len()))
        .flatten()
        .map(|i| &tl.layers[i])
    else {
        return;
    };
    let (id, chars) = (layer_rename_id(layer.id), layer.name.chars().count());
    let key = (timeline_key(host.tab()), layer.id);
    let mut state = egui::text_edit::TextEditState::default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(chars),
        )));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
    finish_layer_rename(host);
    host.timeline_ui_mut().renaming_layer = Some(key);
}

/// 名前を書き換えているレイヤーの行に入力欄を出し、打つそばから名前を変える（`Action::RenameLayer`）。入力欄から離れたか、
/// そのレイヤーが今のタイムラインに無いか、行が見えていなければ終える
fn draw_layer_rename_field(
    host: &mut impl TimelineHost,
    ui: &mut egui::Ui,
    layout: &TimelineLayout,
) {
    let Some((key, layer_id)) = host.timeline_ui().renaming_layer else {
        return;
    };
    // 入力欄は、文字の高さに余白を足した高さで行の上下の真ん中に置き、左は余白の分だけ前に出す（入力中も、名前の文字が
    // 行に描くときと同じ位置に来る）
    let font = FontId::proportional(LAYER_NAME_FONT_SIZE);
    let height = ui.fonts(|f| f.row_height(&font)) + 2.0 * LAYER_RENAME_PAD.y;
    let tl = host.tab().active_timeline();
    let field = (key == timeline_key(host.tab()))
        .then(|| tl.layer_index_by_id(layer_id))
        .flatten()
        .map(|i| {
            let y0 = layout.row_y(tl.layers.len() - 1 - i, host.timeline_ui().scroll_y);
            let cy = y0 + ROW_H * 0.5;
            let rect = Rect::from_min_max(
                Pos2::new(
                    layout.panel_rect.left() + LAYER_NAME_X - LAYER_RENAME_PAD.x,
                    cy - height * 0.5,
                ),
                Pos2::new(layout.grid_left - LAYER_GRIP_W, cy + height * 0.5),
            );
            (rect, tl.layers[i].name.clone())
        })
        .filter(|(r, _)| r.top() >= layout.grid_top && r.bottom() <= layout.panel_rect.bottom());
    let Some((rect, mut name)) = field else {
        finish_layer_rename(host);
        return;
    };
    // 枠は自分で描く（細く淡い線・直角の角）。入力欄の枠は出さない
    ui.painter().rect(
        rect,
        0.0,
        Color32::WHITE,
        Stroke::new(1.0_f32, Color32::from_rgb(190, 196, 206)),
        egui::StrokeKind::Inside,
    );
    let resp = ui.put(
        rect,
        egui::TextEdit::singleline(&mut name)
            .id(layer_rename_id(layer_id))
            .font(font)
            .frame(false)
            .margin(egui::Margin::symmetric(
                LAYER_RENAME_PAD.x as i8,
                LAYER_RENAME_PAD.y as i8,
            ))
            .desired_width(rect.width() - 2.0 * LAYER_RENAME_PAD.x),
    );
    if resp.changed() {
        apply(
            host,
            Action::RenameLayer {
                layer_id,
                new_name: name,
            },
        );
    }
    if resp.lost_focus() {
        finish_layer_rename(host);
    }
}

/// レイヤーの名前の書き換えを終える（続けて打った文字を1つの取り消しにまとめるのを、ここで区切る）
fn finish_layer_rename(host: &mut impl TimelineHost) {
    if let Some((_, layer_id)) = host.timeline_ui_mut().renaming_layer.take() {
        apply(
            host,
            Action::EndCoalesce {
                key: Some(CoalesceKey::RenameLayer(layer_id)),
            },
        );
    }
}

/// 1. 上部タイムライン操作バーの描画とボタンイベント処理
fn draw_timeline_control_bar(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    egui::Frame::NONE
        .fill(Color32::from_rgb(242, 244, 248))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(202, 208, 218)))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                draw_control_bar_transport_buttons(host, ui);
                ui.separator();
                draw_control_bar_keyframe_buttons(host, ui);
                ui.separator();
                draw_control_bar_onion_controls(host.tab_mut(), ui);
                ui.separator();
                draw_control_bar_frame_and_fps(host, ui);
            });
        });
}

/// コントロールバー：再生・コマ送り・ループ設定
fn draw_control_bar_transport_buttons(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    if ui
        .button(icons::SKIP_PREV)
        .on_hover_text("最初のフレームへ (Home)")
        .clicked()
    {
        go_to_frame(host, 0);
    }
    if ui
        .button(icons::STEP_PREV)
        .on_hover_text("前のフレームへ (, / ←)")
        .clicked()
    {
        step_frame(host, -1);
    }
    let is_playing = host.timeline_ui().is_playing;
    let play_label = if is_playing {
        icons::PAUSE
    } else {
        icons::PLAY
    };
    if ui
        .selectable_label(is_playing, play_label)
        .on_hover_text("再生 / 一時停止 (Enter)")
        .clicked()
    {
        toggle_playback(host);
    }
    if ui
        .button(icons::STEP_NEXT)
        .on_hover_text("次のフレームへ (. / →)")
        .clicked()
    {
        step_frame(host, 1);
    }
    if ui
        .button(icons::SKIP_NEXT)
        .on_hover_text("最後のフレームへ (End)")
        .clicked()
    {
        let last = host.tab().active_timeline().total_frames.saturating_sub(1);
        go_to_frame(host, last);
    }
    let loop_pb = host.timeline_ui().loop_playback;
    if ui
        .selectable_label(loop_pb, icons::REPEAT)
        .on_hover_text("ループ再生 ON / OFF（OFF時は最終フレームで自動停止）")
        .clicked()
    {
        host.timeline_ui_mut().loop_playback = !loop_pb;
        if host.timeline_ui().is_playing {
            let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
            timeline_ui.start_playback(tab);
        }
    }
}

/// コントロールバー右端：現在フレーム表示・FPS設定
fn draw_control_bar_frame_and_fps(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    let current_frame = host.tab().active_timeline().current_frame + 1;
    ui.label(format!("{current_frame}F"));

    let mut fps_val = host.tab().doc.fps;
    let fps_resp = ui.add(
        egui::DragValue::new(&mut fps_val)
            .range(1..=crate::symbol::MAX_FPS)
            .update_while_editing(false)
            .speed(0.25),
    );
    ui.label("FPS");
    if fps_resp.changed() {
        let next_fps = fps_val.clamp(1, crate::symbol::MAX_FPS);
        if apply(host, Action::SetFps { fps: next_fps }) && host.timeline_ui().is_playing {
            let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
            timeline_ui.start_playback(tab);
        }
    }
    if editing_idle(&[&fps_resp]) {
        apply(
            host,
            Action::EndCoalesce {
                key: Some(CoalesceKey::Fps),
            },
        );
    }
}

/// 数値の欄 `resps` の編集が一区切りした（どれかがフォーカスを失った、またはどれもドラッグ中・フォーカス中でない）かを返す。
/// `true` のフレームで連続操作のまとめ（`EndCoalesce`）を区切る
pub(crate) fn editing_idle(resps: &[&egui::Response]) -> bool {
    resps.iter().any(|r| r.lost_focus()) || !resps.iter().any(|r| r.dragged() || r.has_focus())
}

/// コントロールバー：キーフレーム挿入・削除・フレーム延長/短縮・クラシックトゥイーンボタン
fn draw_control_bar_keyframe_buttons(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    let tl = host.tab().active_timeline();
    let targets = tl.target_layers_for_frame_op();
    let editable = !targets.is_empty();
    let can_remove_frame = tl.can_remove_frame_on_targets(&targets);
    let can_delete_kf = tl.can_delete_keyframe_on_targets(&targets);
    if ui
        .add_enabled(editable, egui::Button::new("●"))
        .on_hover_text("現在の絵を複製して新しいキーフレームを挿入 (F6)")
        .clicked()
    {
        apply(host, Action::InsertKeyframe { copy_prev: true });
    }
    if ui
        .add_enabled(editable, egui::Button::new("○"))
        .on_hover_text("空の新しいキーフレームを挿入 (F7)")
        .clicked()
    {
        apply(host, Action::InsertKeyframe { copy_prev: false });
    }
    if ui
        .add_enabled(editable, egui::Button::new("＋"))
        .on_hover_text("選択レイヤー（未選択時は全レイヤー）のフレーム尺を1コマ延長 (F5)")
        .clicked()
    {
        apply(host, Action::ExtendFrame);
    }
    if ui
        .add_enabled(can_remove_frame, egui::Button::new("－"))
        .on_hover_text("選択レイヤー（未選択時は全レイヤー）のフレームを1コマ削除・短縮 (Shift+F5)")
        .clicked()
    {
        apply(host, Action::RemoveFrame);
    }
    if ui
        .add_enabled(can_delete_kf, egui::Button::new("×"))
        .on_hover_text("現在のキーフレームを削除 / クリア (Shift+F6)")
        .clicked()
    {
        apply(host, Action::DeleteKeyframe);
    }

    let (can_tween, is_tweened) = host
        .tab()
        .active_timeline()
        .can_toggle_tween_at_current_frame();
    let tween_btn = egui::Button::new(egui::RichText::new("⇄").color(if is_tweened {
        Color32::from_rgb(75, 30, 150)
    } else if can_tween {
        Color32::from_rgb(35, 40, 48)
    } else {
        Color32::from_rgb(150, 156, 168)
    }))
    .fill(if is_tweened {
        Color32::from_rgb(218, 198, 250)
    } else {
        Color32::from_rgb(248, 249, 252)
    })
    .stroke(Stroke::new(
        1.0_f32,
        if is_tweened {
            Color32::from_rgb(135, 95, 205)
        } else {
            Color32::from_rgb(198, 204, 214)
        },
    ));
    if ui
        .add_enabled(can_tween, tween_btn)
        .on_hover_text("クラシックトゥイーンの ON / OFF を切り替え (T)")
        .on_disabled_hover_text(
            "トゥイーンを設定するには、開始キーフレームにシンボルが1つだけ存在する必要があります (F8 でシンボル化)",
        )
        .clicked()
    {
        apply(host, Action::ToggleTween);
    }
}

/// コントロールバー：オニオンスキン ON/OFF・前後フレーム数設定
fn draw_control_bar_onion_controls(tab: &mut Tab, ui: &mut egui::Ui) {
    if ui
        .selectable_label(tab.view.onion_skin_enabled, icons::ONION_SKIN)
        .on_hover_text("オニオンスキン（前後のフレームの透かし表示）切替")
        .clicked()
    {
        tab.view.onion_skin_enabled = !tab.view.onion_skin_enabled;
    }
}

/// 2. マウスホイールによるタイムライン横・縦スクロール更新と範囲クランプ
fn handle_timeline_scroll(
    host: &mut impl TimelineHost,
    ui: &egui::Ui,
    response: &egui::Response,
    layout: &mut TimelineLayout,
) {
    let max_scroll_x = layout.max_scroll_x();
    let max_scroll_y = layout.max_scroll_y(host.tab().active_timeline().layers.len());

    let pointer_in_timeline = response.hovered() || ui.rect_contains_pointer(ui.max_rect());
    if pointer_in_timeline {
        let (scroll_delta, shift_held) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.shift));
        if scroll_delta.x.abs() > 0.1 || scroll_delta.y.abs() > 0.1 {
            let pointer_in_layers = ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|p| p.x < layout.grid_left && p.y > layout.grid_top);
            let timeline_ui = host.timeline_ui_mut();
            if scroll_delta.x.abs() > 0.1 && scroll_delta.y.abs() > 0.1 {
                timeline_ui.scroll_x -= scroll_delta.x;
                timeline_ui.scroll_y -= scroll_delta.y;
            } else if (pointer_in_layers || shift_held || max_scroll_x <= 0.0)
                && max_scroll_y > 0.0
                && scroll_delta.y.abs() >= scroll_delta.x.abs()
            {
                timeline_ui.scroll_y -= scroll_delta.y;
            } else {
                let dx = if scroll_delta.x.abs() > scroll_delta.y.abs() {
                    scroll_delta.x
                } else {
                    scroll_delta.y
                };
                timeline_ui.scroll_x -= dx;
            }
        }
    }

    let timeline_ui = host.timeline_ui_mut();
    timeline_ui.scroll_x = timeline_ui.scroll_x.clamp(0.0, max_scroll_x);
    timeline_ui.scroll_y = timeline_ui.scroll_y.clamp(0.0, max_scroll_y);
    layout.sync_visible_frames(timeline_ui.scroll_x);
}

/// 3. タイムラインパネル内のクリック・ドラッグ入力処理（ルーラースクラブ・セル範囲選択・フレーム移動/複製・レイヤーD&D・スクロールバー操作）
fn handle_timeline_pointer_input(
    host: &mut impl TimelineHost,
    ui: &egui::Ui,
    response: &egui::Response,
    layout: &mut TimelineLayout,
    (primary_down, alt_held, pointer_pos, hover_pos): (bool, bool, Option<Pos2>, Option<Pos2>),
) -> Option<(usize, usize)> {
    let (primary_pressed, shift_held) =
        ui.input(|i| (i.pointer.primary_pressed(), i.modifiers.shift));
    let generation = host.tab().history.current_commit_id;
    host.timeline_ui_mut().drop_drag_if_document_changed(generation);
    let handle_w = handle_width(ui.ctx());
    if response.hovered() && primary_pressed {
        if let Some(pos) = pointer_pos {
            handle_timeline_click_start(host, layout, pos, alt_held, shift_held, handle_w);
        }
    }
    if response.double_clicked() {
        if let Some(pos) = pointer_pos {
            start_layer_rename(host, ui.ctx(), layout, pos);
        }
    }
    update_timeline_grid_drag(host, ui.ctx(), layout, primary_down, alt_held, pointer_pos);
    let drag_layer_feedback =
        update_timeline_layer_drag(host, ui.ctx(), layout, primary_down, pointer_pos);
    let generation = host.tab().history.current_commit_id;
    host.timeline_ui_mut().remember_drag_generation(generation);
    update_timeline_cursor_icon(
        host,
        ui.ctx(),
        layout,
        alt_held,
        hover_pos,
        drag_layer_feedback.is_some(),
        handle_w,
    );
    drag_layer_feedback
}

/// タイムライン上のマウス押下開始時のクリック判定（レイヤー列 / ルーラー / スクロールバー / フレームグリッド）
fn handle_timeline_click_start(
    host: &mut impl TimelineHost,
    layout: &mut TimelineLayout,
    pos: Pos2,
    alt_held: bool,
    shift_held: bool,
    handle_w: f32,
) {
    if layout.add_hdr_rect.contains(pos) {
        apply(host, Action::AddLayer);
    } else if layout.del_hdr_rect.contains(pos) && host.tab().active_timeline().layers.len() > 1 {
        apply(host, Action::DeleteActiveLayer);
    } else if pos.x < layout.grid_left && pos.y >= layout.grid_top {
        let num_layers = host.tab().active_timeline().layers.len();
        let scroll_y = host.timeline_ui().scroll_y;
        if let Some(layer_idx) = layout.layer_at_y(pos.y, scroll_y, num_layers) {
            let rel_x = pos.x - layout.panel_rect.left();
            if LAYER_VIS_HIT_RANGE.contains(&rel_x) {
                let next_vis = !host.tab().active_timeline().layers[layer_idx].visible;
                apply(
                    host,
                    Action::SetLayerVisible {
                        layer_idx,
                        visible: next_vis,
                    },
                );
            } else if LAYER_LOCK_HIT_RANGE.contains(&rel_x) {
                let next_lock = !host.tab().active_timeline().layers[layer_idx].locked;
                apply(
                    host,
                    Action::SetLayerLocked {
                        layer_idx,
                        locked: next_lock,
                    },
                );
            } else {
                let tl = host.tab().active_timeline();
                let cf = tl.current_frame;
                let anchor_layer = if shift_held {
                    tl.active_layer.min(num_layers.saturating_sub(1))
                } else {
                    layer_idx
                };
                apply(
                    host,
                    Action::SelectTimelineCells {
                        l0: anchor_layer,
                        f0: cf,
                        l1: layer_idx,
                        f1: cf,
                    },
                );
                if !shift_held {
                    host.timeline_ui_mut().dragging_layer = Some(layer_idx);
                }
            }
        } else {
            apply(host, Action::ClearTimelineCells);
        }
    } else if host.timeline_ui().dragging_layer.is_none() && pos.x >= layout.grid_left {
        let num_layers = host.tab().active_timeline().layers.len();
        for axis in [ScrollAxis::Y, ScrollAxis::X] {
            let scroll = match axis {
                ScrollAxis::X => host.timeline_ui().scroll_x,
                ScrollAxis::Y => host.timeline_ui().scroll_y,
            };
            if let Some((hit_rect, thumb_rect, thumb_len, _)) =
                layout.scrollbar_metrics(axis, num_layers, scroll)
            {
                if hit_rect.contains(pos) {
                    let p = axis.of(pos);
                    let grab_offset =
                        if p >= axis.of(thumb_rect.min) && p <= axis.of(thumb_rect.max) {
                            p - axis.of(thumb_rect.min)
                        } else {
                            thumb_len * 0.5
                        };
                    drag_scrollbar_thumb(host, layout, axis, pos, grab_offset);
                    host.timeline_ui_mut().grid_drag =
                        TimelineGridDrag::ScrubScrollbar { axis, grab_offset };
                    return;
                }
            }
        }

        // オニオンスキンの範囲の端のつまみ：ルーラーのスクラブより先に見る
        if let Some(side) = onion_handle_at(host, layout, pos, handle_w) {
            host.timeline_ui_mut().grid_drag = TimelineGridDrag::OnionEdge(side);
            return;
        }

        // 音の波形の端のつまみ：コマの選択・移動より先に見るので、つまみの細い範囲の上だけはつまみが優先される
        if let Some((layer, kf_start, edge, from)) = sound_handle_at(host, layout, pos, handle_w) {
            host.stop_playback();
            host.timeline_ui_mut().grid_drag = TimelineGridDrag::TrimSound {
                layer,
                kf_start,
                edge,
                from,
                frame: from,
            };
            return;
        }

        let col = layout.frame_at_x(pos.x, host.timeline_ui().scroll_x);
        if pos.y < layout.grid_top {
            let target_frame = col.min(host.tab().active_timeline().total_frames.saturating_sub(1));
            host.timeline_ui_mut().grid_drag = TimelineGridDrag::ScrubRuler;
            apply(
                host,
                Action::ScrubRulerToFrame {
                    frame: target_frame,
                },
            );
        } else if let Some(target_layer) =
            layout.layer_at_y(pos.y, host.timeline_ui().scroll_y, num_layers)
        {
            let tl = host.tab().active_timeline();
            let layer_fc = tl.layers[target_layer].frame_count;
            let target_frame = col;

            if shift_held {
                let anchor_layer = tl.active_layer.min(num_layers.saturating_sub(1));
                let anchor_frame = tl.current_frame;
                apply(
                    host,
                    Action::SelectTimelineCells {
                        l0: anchor_layer,
                        f0: anchor_frame,
                        l1: target_layer,
                        f1: target_frame,
                    },
                );
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::SelectCells {
                    start_layer: anchor_layer,
                    start_frame: anchor_frame,
                };
                return;
            }

            // 実際に存在するフレーム（col < layer_fc）の上をクリックしたときのみ MoveFrames 判定を行う
            // （未作成の後ろの空マスをクリックした際はその空マスを選択状態にし、F5 / F6 / F7 でそこまで尺を伸ばせるようにする）
            let clicked_on_existing_frame = col < layer_fc;
            let clicked_in_existing_sel = clicked_on_existing_frame
                && tl
                    .cell_selection
                    .is_some_and(|s| s.contains(target_layer, col));

            if clicked_in_existing_sel || (clicked_on_existing_frame && alt_held) {
                let orig_sel = if let Some(sel) =
                    tl.cell_selection.filter(|s| s.contains(target_layer, col))
                {
                    host.stop_playback();
                    apply(host, Action::EndCoalesce { key: None });
                    sel
                } else {
                    apply(
                        host,
                        Action::SelectTimelineCells {
                            l0: target_layer,
                            f0: col,
                            l1: target_layer,
                            f1: col,
                        },
                    );
                    TimelineCellSelection::single(target_layer, col)
                };
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::MoveFrames {
                    orig_sel,
                    grab_layer: target_layer,
                    grab_frame: col,
                    curr_layer: target_layer,
                    curr_frame: col,
                    copy_mode: alt_held,
                    has_moved: false,
                };
            } else {
                apply(
                    host,
                    Action::SelectTimelineCells {
                        l0: target_layer,
                        f0: target_frame,
                        l1: target_layer,
                        f1: target_frame,
                    },
                );
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::SelectCells {
                    start_layer: target_layer,
                    start_frame: target_frame,
                };
            }
        } else {
            apply(host, Action::ClearTimelineCells);
        }
    }
}

/// スクロールバーのつまみドラッグ（またはトラッククリック）に応じて指定した軸のスクロール位置を更新する
fn drag_scrollbar_thumb(
    host: &mut impl TimelineHost,
    layout: &mut TimelineLayout,
    axis: ScrollAxis,
    pos: Pos2,
    grab_offset: f32,
) {
    let num_layers = host.tab().active_timeline().layers.len();
    let scroll = match axis {
        ScrollAxis::X => host.timeline_ui().scroll_x,
        ScrollAxis::Y => host.timeline_ui().scroll_y,
    };
    if let Some((_, _, thumb_len, max_scroll)) = layout.scrollbar_metrics(axis, num_layers, scroll)
    {
        let new_scroll = layout.scroll_from_thumb(axis, pos, grab_offset, thumb_len, max_scroll);
        match axis {
            ScrollAxis::X => {
                host.timeline_ui_mut().scroll_x = new_scroll;
                layout.sync_visible_frames(new_scroll);
            }
            ScrollAxis::Y => {
                host.timeline_ui_mut().scroll_y = new_scroll;
            }
        }
    }
}

/// ルーラースクラブ・スクロールバー操作・セル範囲選択・フレーム移動/複製ドラッグの継続および確定処理
fn update_timeline_grid_drag(
    host: &mut impl TimelineHost,
    ctx: &egui::Context,
    layout: &mut TimelineLayout,
    primary_down: bool,
    alt_held: bool,
    pointer_pos: Option<Pos2>,
) {
    match host.timeline_ui().grid_drag {
        TimelineGridDrag::None => {}
        TimelineGridDrag::ScrubRuler => {
            if primary_down {
                if let Some(pos) = pointer_pos {
                    let col = layout.frame_at_x(pos.x, host.timeline_ui().scroll_x);
                    let target =
                        col.min(host.tab().active_timeline().total_frames.saturating_sub(1));
                    apply(host, Action::ScrubRulerToFrame { frame: target });
                }
            } else {
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::None;
            }
        }
        TimelineGridDrag::ScrubScrollbar { axis, grab_offset } => {
            if primary_down {
                if let Some(pos) = pointer_pos {
                    drag_scrollbar_thumb(host, layout, axis, pos, grab_offset);
                }
            } else {
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::None;
            }
        }
        TimelineGridDrag::SelectCells {
            start_layer,
            start_frame,
        } => {
            if primary_down {
                if let Some(pos) = pointer_pos {
                    scroll_at_edges(host, ctx, layout, pos, true);
                    let scroll_x = host.timeline_ui().scroll_x;
                    let scroll_y = host.timeline_ui().scroll_y;
                    let tl = host.tab().active_timeline();
                    let num_layers = tl.layers.len();
                    let target_frame = layout.frame_at_x(pos.x, scroll_x);
                    let target_layer = layout.clamped_layer_at_y(pos.y, scroll_y, num_layers);
                    let max_l = num_layers.saturating_sub(1);
                    let l0 = start_layer.min(max_l);
                    let l1 = target_layer.min(max_l);
                    let desired_sel =
                        TimelineCellSelection::from_corners(l0, start_frame, l1, target_frame);
                    if tl.cell_selection != Some(desired_sel)
                        || tl.active_layer != l1
                        || tl.current_frame != target_frame
                    {
                        apply(
                            host,
                            Action::SelectTimelineCells {
                                l0: start_layer,
                                f0: start_frame,
                                l1: target_layer,
                                f1: target_frame,
                            },
                        );
                    }
                }
            } else {
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::None;
            }
        }
        TimelineGridDrag::OnionEdge(side) => {
            if !primary_down {
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::None;
            } else if let Some(pos) = pointer_pos {
                // マウスにいちばん近いコマの境目までの、今のコマからのコマ数（上限 `ONION_MAX_FRAMES`）
                let scroll_x = host.timeline_ui().scroll_x;
                let boundary = layout.frame_at_x(pos.x + CELL_W * 0.5, scroll_x);
                let cf = host.tab().active_timeline().current_frame;
                let view = &mut host.tab_mut().view;
                match side {
                    OnionSide::Prev => {
                        view.onion_prev_frames = cf.saturating_sub(boundary).min(ONION_MAX_FRAMES);
                    }
                    OnionSide::Next => {
                        view.onion_next_frames =
                            boundary.saturating_sub(cf + 1).min(ONION_MAX_FRAMES);
                    }
                }
            }
        }
        TimelineGridDrag::TrimSound {
            layer,
            kf_start,
            edge,
            from,
            frame,
        } => {
            if primary_down {
                // マウスにいちばん近いコマの境目を、動かせる所（`Layer::sound_trim_range`）に収める
                let tab = host.tab();
                let tl = tab.active_timeline();
                let range = tl.layers.get(layer).and_then(|l| {
                    let k = l.keyframe_index_starting_at(kf_start)?;
                    l.sound_trim_range(k, edge, &tab.doc.sounds, tab.doc.fps)
                });
                if let (Some(pos), Some(range)) = (pointer_pos, range) {
                    let scroll_x = host.timeline_ui().scroll_x;
                    let nearest = layout.frame_at_x(pos.x + CELL_W * 0.5, scroll_x);
                    let frame = nearest.clamp(*range.start(), *range.end());
                    host.timeline_ui_mut().grid_drag = TimelineGridDrag::TrimSound {
                        layer,
                        kf_start,
                        edge,
                        from,
                        frame,
                    };
                }
            } else {
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::None;
                if frame != from {
                    apply(
                        host,
                        Action::TrimSound {
                            layer,
                            kf_start,
                            edge,
                            frame,
                        },
                    );
                }
            }
        }
        TimelineGridDrag::MoveFrames {
            orig_sel,
            grab_layer,
            grab_frame,
            curr_layer,
            curr_frame,
            copy_mode,
            has_moved,
        } => {
            if primary_down {
                if let Some(pos) = pointer_pos {
                    scroll_at_edges(host, ctx, layout, pos, true);
                    let scroll_x = host.timeline_ui().scroll_x;
                    let scroll_y = host.timeline_ui().scroll_y;
                    let num_layers = host.tab().active_timeline().layers.len();
                    let col = layout.frame_at_x(pos.x, scroll_x);
                    let next_frame = col.min(layout.max_frames_shown.saturating_sub(1));
                    let next_layer = layout.clamped_layer_at_y(pos.y, scroll_y, num_layers);
                    let moved = has_moved || next_layer != grab_layer || next_frame != grab_frame;
                    host.timeline_ui_mut().grid_drag = TimelineGridDrag::MoveFrames {
                        orig_sel,
                        grab_layer,
                        grab_frame,
                        curr_layer: next_layer,
                        curr_frame: next_frame,
                        copy_mode: alt_held,
                        has_moved: moved,
                    };
                }
            } else {
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::None;
                if has_moved {
                    let layer_delta = curr_layer as isize - grab_layer as isize;
                    let frame_delta = curr_frame as isize - grab_frame as isize;
                    apply(
                        host,
                        Action::MoveOrCopyFrames {
                            sel: orig_sel,
                            layer_delta,
                            frame_delta,
                            copy_mode: copy_mode || alt_held,
                        },
                    );
                } else {
                    apply(
                        host,
                        Action::SelectTimelineCells {
                            l0: grab_layer,
                            f0: grab_frame,
                            l1: grab_layer,
                            f1: grab_frame,
                        },
                    );
                }
            }
        }
    }
}

/// レイヤーD&D並び替えの継続およびドロップ確定処理
fn update_timeline_layer_drag(
    host: &mut impl TimelineHost,
    ctx: &egui::Context,
    layout: &mut TimelineLayout,
    primary_down: bool,
    pointer_pos: Option<Pos2>,
) -> Option<(usize, usize)> {
    let src_idx = host.timeline_ui().dragging_layer?;
    let num_layers = host.tab().active_timeline().layers.len();
    if src_idx >= num_layers {
        host.timeline_ui_mut().dragging_layer = None;
        return None;
    }
    if let Some(pos) = pointer_pos {
        if primary_down {
            scroll_at_edges(host, ctx, layout, pos, false);
        }
        let dst_idx = layout.clamped_layer_at_y(pos.y, host.timeline_ui().scroll_y, num_layers);
        if primary_down {
            return Some((src_idx, dst_idx));
        }
        host.timeline_ui_mut().dragging_layer = None;
        apply(host, Action::ReorderLayer { src_idx, dst_idx });
    } else if !primary_down {
        host.timeline_ui_mut().dragging_layer = None;
    }
    None
}

/// ドラッグしている間、マウス（`pos`）が、レイヤーの並び（縦）の上下の端から1行分の所、または `horizontal` ならコマの
/// 並び（横）の左右の端から1コマ分の所に入ったら、その向きへスクロールする。速さは、その所に入った深さ（端を越えた分
/// も含む）に比例し（`DRAG_SCROLL_RATE`）、マウスを止めていても端にある間はスクロールし続ける（描き直しを頼む）。見えて
/// いない所のレイヤー・コマへも運べる。端の行・列を押しただけ（クリックかもしれない間）はスクロールしない（egui の
/// 「クリックでなくドラッグと決まった」：`is_decidedly_dragging`）
fn scroll_at_edges(
    host: &mut impl TimelineHost,
    ctx: &egui::Context,
    layout: &mut TimelineLayout,
    pos: Pos2,
    horizontal: bool,
) {
    if !ctx.input(|i| i.pointer.is_decidedly_dragging()) {
        return;
    }
    // 範囲 `lo..hi` の端から `zone` の所に入った深さ（前の端は負、後ろの端は正。入っていなければ 0）
    let depth = |p: f32, lo: f32, hi: f32, zone: f32| {
        if p < lo + zone {
            p - (lo + zone)
        } else if p > hi - zone {
            p - (hi - zone)
        } else {
            0.0
        }
    };
    let dy = depth(
        pos.y,
        layout.grid_top,
        layout.grid_top + layout.grid_h,
        ROW_H,
    );
    let dx = if horizontal {
        depth(
            pos.x,
            layout.grid_left,
            layout.grid_left + layout.grid_w,
            CELL_W,
        )
    } else {
        0.0
    };
    if dx == 0.0 && dy == 0.0 {
        return;
    }
    let step = DRAG_SCROLL_RATE * ctx.input(|i| i.stable_dt);
    let (max_x, max_y) = (
        layout.max_scroll_x(),
        layout.max_scroll_y(host.tab().active_timeline().layers.len()),
    );
    let ui_state = host.timeline_ui_mut();
    ui_state.scroll_x = (ui_state.scroll_x + dx * step).clamp(0.0, max_x);
    ui_state.scroll_y = (ui_state.scroll_y + dy * step).clamp(0.0, max_y);
    layout.sync_visible_frames(ui_state.scroll_x);
    ctx.request_repaint();
}

/// タイムライン上のドラッグ状態およびホバー位置に応じてOSカーソルアイコンを更新する
fn update_timeline_cursor_icon(
    host: &impl TimelineHost,
    ctx: &egui::Context,
    layout: &TimelineLayout,
    alt_held: bool,
    hover_pos: Option<Pos2>,
    is_dragging_layer: bool,
    handle_w: f32,
) {
    let on_handle = || {
        matches!(host.timeline_ui().grid_drag, TimelineGridDrag::None)
            && host.timeline_ui().dragging_layer.is_none()
            && hover_pos.is_some_and(|p| {
                onion_handle_at(host, layout, p, handle_w).is_some()
                    || sound_handle_at(host, layout, p, handle_w).is_some()
            })
    };
    if matches!(
        host.timeline_ui().grid_drag,
        TimelineGridDrag::TrimSound { .. } | TimelineGridDrag::OnionEdge(_)
    ) || on_handle()
    {
        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    } else if let TimelineGridDrag::MoveFrames {
        has_moved: true, ..
    } = host.timeline_ui().grid_drag
    {
        ctx.set_cursor_icon(if alt_held {
            egui::CursorIcon::Copy
        } else {
            egui::CursorIcon::Grabbing
        });
    } else if is_dragging_layer {
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if let Some(hpos) = hover_pos {
        if layout.panel_rect.contains(hpos)
            && hpos.x < layout.grid_left
            && hpos.y >= layout.grid_top
            && (hpos.x - layout.panel_rect.left()) >= LAYER_LOCK_HIT_RANGE.end
            && layout
                .layer_at_y(
                    hpos.y,
                    host.timeline_ui().scroll_y,
                    host.tab().active_timeline().layers.len(),
                )
                .is_some()
        {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
}

/// 音の波形の端・オニオンスキンの範囲の端のつまみの幅（画面の px）：egui が窓・パネルの端をつかんで大きさを変える所の幅
/// （`resize_grab_radius_side`）と同じにして、どこでも同じ手ざわりにする（タイムラインの拡大・縮小によらない）。つまみは
/// 波形の内側に置き、波形はいちばん短くて1コマ（`CELL_W`）なので、左右のつまみが重ならないよう1コマの半分を越えない
fn handle_width(ctx: &egui::Context) -> f32 {
    ctx.style()
        .interaction
        .resize_grab_radius_side
        .min(CELL_W * 0.5)
}

/// 位置 `pos`（ルーラーの上）にある、オニオンスキンの範囲の端のつまみ。つまみは、前の範囲の左端と後ろの範囲の右端の
/// コマの境目から、左右に幅 `handle_w` の所（範囲が 0 コマなら、今のコマの左右の境目）。オニオンスキンが切りのときは無い
fn onion_handle_at(
    host: &impl TimelineHost,
    layout: &TimelineLayout,
    pos: Pos2,
    handle_w: f32,
) -> Option<OnionSide> {
    let tab = host.tab();
    if !tab.view.onion_skin_enabled || pos.y >= layout.grid_top || pos.x < layout.grid_left {
        return None;
    }
    let (cf, scroll_x) = (
        tab.active_timeline().current_frame,
        host.timeline_ui().scroll_x,
    );
    let edges = [
        (
            OnionSide::Prev,
            cf.saturating_sub(tab.view.onion_prev_frames),
        ),
        (OnionSide::Next, cf + 1 + tab.view.onion_next_frames),
    ];
    edges
        .into_iter()
        .find(|&(_, b)| (pos.x - layout.frame_x(b, scroll_x)).abs() <= handle_w)
        .map(|(side, _)| side)
}

/// 位置 `pos` にある、音の波形の端のつまみ `(レイヤー, キーフレームの始まりのコマ, 端, 端のコマの境目)`。つまみは、
/// 波形（`Layer::sounding_frames`）の内側の、左端・右端から幅 `handle_w` の所。タイムラインの操作を受けない（ロックした）
/// レイヤーには無い
fn sound_handle_at(
    host: &impl TimelineHost,
    layout: &TimelineLayout,
    pos: Pos2,
    handle_w: f32,
) -> Option<(usize, usize, SoundEdge, usize)> {
    if pos.x < layout.grid_left || pos.y < layout.grid_top {
        return None;
    }
    let tab = host.tab();
    let tl = tab.active_timeline();
    let (scroll_x, scroll_y) = (host.timeline_ui().scroll_x, host.timeline_ui().scroll_y);
    let layer_idx = layout.layer_at_y(pos.y, scroll_y, tl.layers.len())?;
    let layer = &tl.layers[layer_idx];
    if !layer.accepts_frame_ops() {
        return None;
    }
    let k = layer.keyframe_index_at(layout.frame_at_x(pos.x, scroll_x))?;
    let frames = layer.sounding_frames(k, tab.doc.fps)?;
    let (xl, xr) = (
        layout.frame_x(frames.start, scroll_x),
        layout.frame_x(frames.end, scroll_x),
    );
    if pos.x < xl || pos.x > xr {
        return None;
    }
    let (to_left, to_right) = (pos.x - xl, xr - pos.x);
    let (edge, at) = if to_left <= to_right {
        (SoundEdge::Start, frames.start)
    } else {
        (SoundEdge::End, frames.end)
    };
    (to_left.min(to_right) <= handle_w).then_some((layer_idx, frames.start, edge, at))
}

/// 波形の端をドラッグしているとき、そのレイヤー `layer_idx` に、離したときと同じ操作（`Layer::trim_sound`）を行った写し
fn sound_trim_preview(host: &impl TimelineHost, layer_idx: usize) -> Option<Layer> {
    let TimelineGridDrag::TrimSound {
        layer,
        kf_start,
        edge,
        frame,
        ..
    } = host.timeline_ui().grid_drag
    else {
        return None;
    };
    let tab = host.tab();
    let mut copy = tab
        .active_timeline()
        .layers
        .get(layer)
        .filter(|_| layer == layer_idx)?
        .clone();
    let k = copy.keyframe_index_starting_at(kf_start)?;
    copy.trim_sound(k, edge, frame, &tab.doc.sounds, tab.doc.fps);
    Some(copy)
}

/// 4. 右側フレームグリッド（キーフレーム帯・セル選択ハイライト・フレーム移動ゴースト枠）の描画
fn draw_timeline_frame_grid(
    host: &impl TimelineHost,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
    alt_held: bool,
) {
    let grid_rect = Rect::from_min_max(
        Pos2::new(layout.grid_left, layout.grid_top),
        layout.panel_rect.right_bottom(),
    );
    grid_painter.rect_filled(grid_rect, 0.0, Color32::from_rgb(232, 235, 240));

    let num_layers = host.tab().active_timeline().layers.len();
    let (scroll_x, scroll_y) = (host.timeline_ui().scroll_x, host.timeline_ui().scroll_y);

    for row in 0..num_layers {
        let layer_idx = num_layers.saturating_sub(1 + row);
        let y0 = layout.row_y(row, scroll_y);
        let y1 = y0 + ROW_H;
        if y1 < layout.grid_top || y0 > layout.panel_rect.bottom() {
            continue;
        }
        // 波形の端をドラッグしているレイヤーは、離したときと同じ操作（`Layer::trim_sound`）を写しに行った形を仮に描く
        let preview = sound_trim_preview(host, layer_idx);
        let layer = preview
            .as_ref()
            .unwrap_or(&host.tab().active_timeline().layers[layer_idx]);
        draw_timeline_layer_row(host, grid_painter, layout, layer_idx, layer, (y0, y1));
    }

    draw_move_frames_ghost(host, grid_painter, layout, alt_held);

    for f in layout.first_vis_frame..=layout.last_vis_frame {
        if (f + 1) % 5 == 0 {
            let fx = layout.frame_x(f + 1, scroll_x);
            grid_painter.line_segment(
                [
                    Pos2::new(fx, layout.grid_top),
                    Pos2::new(fx, layout.panel_rect.bottom()),
                ],
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(150, 158, 174, 130)),
            );
        }
    }
}

/// 単一レイヤー行 `layer_idx` の背景セル・キーフレームスパン帯・セル選択枠を描画する
fn draw_timeline_layer_row(
    host: &impl TimelineHost,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
    layer_idx: usize,
    layer: &Layer,
    (y0, y1): (f32, f32),
) {
    let tab = host.tab();
    let tl = tab.active_timeline();
    let scroll_x = host.timeline_ui().scroll_x;
    let is_layer_selected = tl
        .cell_selection
        .is_some_and(|s| s.contains_layer(layer_idx));

    for f in layout.first_vis_frame..=layout.last_vis_frame {
        let x0 = layout.frame_x(f, scroll_x);
        let cell_rect = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + CELL_W, y1));
        let bg = if f < layer.frame_count {
            if is_layer_selected {
                Color32::from_rgb(236, 243, 252)
            } else {
                Color32::from_rgb(248, 249, 252)
            }
        } else {
            Color32::from_rgb(228, 231, 237)
        };
        grid_painter.rect_filled(cell_rect, 0.0, bg);
        grid_painter.line_segment(
            [Pos2::new(x0 + CELL_W, y0), Pos2::new(x0 + CELL_W, y1)],
            Stroke::new(1.0_f32, Color32::from_rgb(214, 218, 226)),
        );
    }

    // 見えている範囲の始めにかかるキーフレームから回し始め、範囲の終わりで止める（キーフレームは開始コマの順。1コマの
    // 仕事が、見えている範囲のキーフレームの数だけで決まる：レビュー f01d518 の N-5005）
    let first = layer
        .keyframes
        .partition_point_key(|s| s <= layout.first_vis_frame)
        .saturating_sub(1);
    for kf_idx in first..layer.keyframes.len() {
        let start = layer.start_of(kf_idx);
        #[cfg(test)]
        KEYFRAMES_EXAMINED.with(|n| n.set(n.get() + 1));
        if start >= layer.frame_count || start > layout.last_vis_frame {
            break;
        }
        draw_keyframe_span(
            host,
            grid_painter,
            layout,
            layer_idx,
            layer,
            kf_idx,
            (y0, y1),
        );
    }

    if let Some(sel) = tl.cell_selection.filter(|s| s.contains_layer(layer_idx)) {
        let sx0 = layout.frame_x(sel.min_frame, scroll_x);
        let sx1 = layout.frame_x(sel.max_frame + 1, scroll_x);
        let sel_rect = Rect::from_min_max(
            Pos2::new(sx0 + 0.5, y0 + 0.5),
            Pos2::new(sx1 - 0.5, y1 - 0.5),
        );
        grid_painter.rect_filled(
            sel_rect,
            0.0,
            Color32::from_rgba_unmultiplied(0, 125, 245, 65),
        );
        grid_painter.rect_stroke(
            sel_rect,
            0.0,
            Stroke::new(1.6_f32, Color32::from_rgb(0, 115, 230)),
            egui::StrokeKind::Middle,
        );
    }

    grid_painter.line_segment(
        [
            Pos2::new(layout.grid_left, y1),
            Pos2::new(layout.panel_rect.right(), y1),
        ],
        Stroke::new(1.0_f32, Color32::from_rgb(195, 200, 210)),
    );
}

/// フレーム移動／Alt複製ドラッグ中のゴーストプレビュー矩形とフローティングバッジを描画する
fn draw_move_frames_ghost(
    host: &impl TimelineHost,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
    alt_held: bool,
) {
    let TimelineGridDrag::MoveFrames {
        orig_sel,
        grab_layer,
        grab_frame,
        curr_layer,
        curr_frame,
        copy_mode,
        has_moved: true,
    } = host.timeline_ui().grid_drag
    else {
        return;
    };
    let tl = host.tab().active_timeline();
    let num_layers = tl.layers.len();
    let (scroll_x, scroll_y) = (host.timeline_ui().scroll_x, host.timeline_ui().scroll_y);
    let layer_delta = curr_layer as isize - grab_layer as isize;
    let frame_delta = curr_frame as isize - grab_frame as isize;
    let Some((dst_sel, _)) = tl.resolve_frame_move_bounds(orig_sel, layer_delta, frame_delta)
    else {
        return;
    };

    let ghost_rect = Rect::from_min_max(
        Pos2::new(
            layout.frame_x(dst_sel.min_frame, scroll_x),
            layout.row_y(num_layers.saturating_sub(1 + dst_sel.max_layer), scroll_y),
        ),
        Pos2::new(
            layout.frame_x(dst_sel.max_frame + 1, scroll_x),
            layout.row_y(num_layers.saturating_sub(dst_sel.min_layer), scroll_y),
        ),
    );
    let is_copy = copy_mode || alt_held;
    let (accent, fill) = if is_copy {
        (
            Color32::from_rgb(20, 165, 80),
            Color32::from_rgba_unmultiplied(30, 185, 95, 60),
        )
    } else {
        (
            Color32::from_rgb(225, 130, 15),
            Color32::from_rgba_unmultiplied(245, 155, 30, 60),
        )
    };
    grid_painter.rect_filled(ghost_rect, 0.0, fill);
    grid_painter.rect_stroke(
        ghost_rect,
        0.0,
        Stroke::new(2.0_f32, accent),
        egui::StrokeKind::Middle,
    );
}

/// 1つのキーフレーム保持区間（帯・1コマ区切り線・先頭丸記号 `●`/`○`・終端マーク `▯` またはトゥイーン点線/矢印）を描画する
fn draw_keyframe_span(
    host: &impl TimelineHost,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
    layer_idx: usize,
    layer: &Layer,
    kf_idx: usize,
    (y0, y1): (f32, f32),
) {
    let tl = host.tab().active_timeline();
    let kf = &layer.keyframes[kf_idx];
    let span_start = layer.start_of(kf_idx);
    let span_end = layer.span_end_for_keyframe(kf_idx);
    let has_art = !kf.cel.is_empty();
    let is_layer_selected = tl
        .cell_selection
        .is_some_and(|s| s.contains_layer(layer_idx));
    let tween_state = layer.tween_state_at_keyframe(kf_idx);
    let scroll_x = host.timeline_ui().scroll_x;
    let sx0 = layout.frame_x(span_start, scroll_x);
    let sx1 = layout.frame_x(span_end + 1, scroll_x);
    let span_rect = Rect::from_min_max(Pos2::new(sx0, y0), Pos2::new(sx1, y1));
    let is_tween = !matches!(tween_state, TweenState::None);

    let span_fill = if is_tween {
        if is_layer_selected {
            Color32::from_rgb(206, 184, 245)
        } else {
            Color32::from_rgb(222, 205, 250)
        }
    } else {
        match (has_art, is_layer_selected) {
            (true, true) => Color32::from_rgb(196, 212, 232),
            (true, false) => Color32::from_rgb(212, 218, 228),
            (false, true) => Color32::from_rgb(244, 248, 255),
            (false, false) => Color32::from_rgb(255, 255, 255),
        }
    };
    grid_painter.rect_filled(span_rect, 0.0, span_fill);

    let sep_col = if is_tween {
        Color32::from_rgba_unmultiplied(150, 122, 196, 95)
    } else if has_art {
        Color32::from_rgba_unmultiplied(145, 155, 172, 110)
    } else {
        Color32::from_rgba_unmultiplied(195, 202, 214, 130)
    };
    for f in (span_start.max(layout.first_vis_frame) + 1)..=span_end.min(layout.last_vis_frame) {
        let fx = layout.frame_x(f, scroll_x);
        grid_painter.line_segment(
            [Pos2::new(fx, y0 + 1.0), Pos2::new(fx, y1 - 1.0)],
            Stroke::new(1.0_f32, sep_col),
        );
    }
    draw_sound_wave(host, grid_painter, layout, layer, kf_idx, (y0, y1));
    let border_col = if is_tween {
        Color32::from_rgb(146, 116, 194)
    } else {
        Color32::from_rgb(155, 162, 175)
    };
    grid_painter.rect_stroke(
        span_rect,
        0.0,
        Stroke::new(1.0_f32, border_col),
        egui::StrokeKind::Middle,
    );

    let kf_center = Pos2::new(sx0 + CELL_W * 0.5, (y0 + y1) * 0.5 + 1.0);
    if has_art {
        let dot_col = if is_tween {
            Color32::from_rgb(45, 20, 95)
        } else {
            Color32::from_rgb(25, 28, 35)
        };
        grid_painter.circle_filled(kf_center, 3.8, dot_col);
    } else {
        grid_painter.circle_stroke(
            kf_center,
            3.6,
            Stroke::new(1.5_f32, Color32::from_rgb(75, 82, 95)),
        );
    }

    draw_keyframe_tween_or_end_marker(
        grid_painter,
        tween_state,
        (span_start, span_end),
        (sx0, sx1),
        kf_center.y,
    );
}

/// キーフレーム `kf_idx` の音が鳴るコマ（`Layer::sounding_frames`）に、その音の区間の波形を薄く描く（レイヤーの行の
/// 高さに収める）。見えているコマだけを、画面の 1px ごとに、その px に当たる音の範囲の値の幅（`Sound::peak`）の縦線で
/// 描く。コマの中の音の位置は、鳴らすのと同じ決まり（コマの始まりは `frames_to_sound`、その中は時間に比例）
fn draw_sound_wave(
    host: &impl TimelineHost,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
    layer: &Layer,
    kf_idx: usize,
    (y0, y1): (f32, f32),
) {
    let doc = &host.tab().doc;
    let Some(frames) = layer.sounding_frames(kf_idx, doc.fps) else {
        return;
    };
    let Some(clip) = layer.keyframes[kf_idx].sound else {
        return;
    };
    let Some(sound) = doc.sounds.iter().find(|s| s.id == clip.sound) else {
        return;
    };
    let scroll_x = host.timeline_ui().scroll_x;
    let band = Rect::from_min_max(
        Pos2::new(layout.frame_x(frames.start, scroll_x), y0 + 1.0),
        Pos2::new(layout.frame_x(frames.end, scroll_x), y1 - 1.0),
    );
    grid_painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(40, 130, 210, 30));
    let (mid, half) = (band.center().y, band.height() * 0.5);
    let wave = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(30, 110, 190, 120));
    let px = CELL_W as usize;
    let kf_start = frames.start;
    for f in frames.start.max(layout.first_vis_frame)..frames.end.min(layout.last_vis_frame + 1) {
        let d = f - kf_start;
        let Some(p0) = clip.pos_at(d, doc.fps) else {
            break;
        };
        let p1 = clip.start + frames_to_sound(d + 1, doc.fps);
        let x0 = layout.frame_x(f, scroll_x);
        for j in 0..px {
            let a = p0 + (p1 - p0) * j as u64 / px as u64;
            let b = (p0 + (p1 - p0) * (j as u64 + 1) / px as u64).min(clip.end);
            if a >= b {
                break;
            }
            if let Some((lo, hi)) = sound.peak(a, b, WAVE_UNIT) {
                let x = x0 + j as f32 + 0.5;
                grid_painter.line_segment(
                    [Pos2::new(x, mid - hi * half), Pos2::new(x, mid - lo * half)],
                    wave,
                );
            }
        }
    }
}

/// キーフレームスパンの終端マーク `▯`、未完トゥイーン横点線 `----`、または完全トゥイーン矢印 `───▶` を描画
fn draw_keyframe_tween_or_end_marker(
    grid_painter: &egui::Painter,
    tween_state: TweenState,
    (span_start, span_end): (usize, usize),
    (sx0, sx1): (f32, f32),
    center_y: f32,
) {
    match tween_state {
        TweenState::None => {
            if span_end > span_start {
                let end_mark_x = sx1 - CELL_W * 0.45;
                let end_box =
                    Rect::from_center_size(Pos2::new(end_mark_x, center_y), Vec2::new(4.2, 8.5));
                grid_painter.rect_filled(end_box, 0.0, Color32::WHITE);
                grid_painter.rect_stroke(
                    end_box,
                    0.0,
                    Stroke::new(1.2_f32, Color32::from_rgb(90, 98, 112)),
                    egui::StrokeKind::Middle,
                );
            }
        }
        TweenState::Incomplete => {
            let line_x0 = sx0 + CELL_W * 0.5 + 5.0;
            let line_x1 = sx1 - 2.0;
            if line_x1 > line_x0 {
                let dash_col = Color32::from_rgb(90, 50, 160);
                let mut x = line_x0;
                while x < line_x1 {
                    let x_end = (x + 4.0).min(line_x1);
                    grid_painter.line_segment(
                        [Pos2::new(x, center_y), Pos2::new(x_end, center_y)],
                        Stroke::new(1.5_f32, dash_col),
                    );
                    x += 7.0;
                }
            }
        }
        TweenState::Complete { .. } => {
            let line_x0 = sx0 + CELL_W * 0.5 + 5.0;
            let line_x1 = sx1 - 2.0;
            let arrow_col = Color32::from_rgb(72, 32, 145);
            if line_x1 > line_x0 {
                let shaft_end = (line_x1 - 4.0).max(line_x0);
                grid_painter.line_segment(
                    [Pos2::new(line_x0, center_y), Pos2::new(shaft_end, center_y)],
                    Stroke::new(1.6_f32, arrow_col),
                );
            }
            let tip = Pos2::new(line_x1, center_y);
            grid_painter.add(egui::Shape::convex_polygon(
                vec![
                    tip,
                    Pos2::new(tip.x - 5.5, tip.y - 3.4),
                    Pos2::new(tip.x - 5.5, tip.y + 3.4),
                ],
                arrow_col,
                Stroke::NONE,
            ));
        }
    }
}

/// 5. 上部フレーム番号ルーラー・オニオンスキン帯・再生ヘッドの描画
fn draw_timeline_ruler(
    host: &impl TimelineHost,
    painter: &egui::Painter,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
) {
    let tab = host.tab();
    let tl = tab.active_timeline();
    let timeline_ui = host.timeline_ui();
    let ruler_rect = Rect::from_min_max(
        Pos2::new(layout.grid_left, layout.panel_rect.top()),
        Pos2::new(layout.panel_rect.right(), layout.grid_top),
    );
    let ruler_painter = painter.with_clip_rect(ruler_rect);
    ruler_painter.rect_filled(ruler_rect, 0.0, Color32::from_rgb(228, 231, 237));

    let scroll_x = timeline_ui.scroll_x;
    if tab.view.onion_skin_enabled {
        let cf = tl.current_frame;
        let prev_start = cf.saturating_sub(tab.view.onion_prev_frames);
        if prev_start < cf {
            ruler_painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(layout.frame_x(prev_start, scroll_x), ruler_rect.top() + 2.0),
                    Pos2::new(layout.frame_x(cf, scroll_x), ruler_rect.bottom() - 2.0),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(45, 125, 240, 65),
            );
        }
        // 範囲は、端のつまみ（`onion_handle_at`）と同じ所まで描く（タイムラインの終わりを越えた分は透かす絵が無い）
        let next_end = cf + tab.view.onion_next_frames;
        if next_end > cf {
            ruler_painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(layout.frame_x(cf + 1, scroll_x), ruler_rect.top() + 2.0),
                    Pos2::new(
                        layout.frame_x(next_end + 1, scroll_x),
                        ruler_rect.bottom() - 2.0,
                    ),
                ),
                0.0,
                Color32::from_rgba_unmultiplied(35, 185, 95, 65),
            );
        }
    }

    for f in layout.first_vis_frame..=layout.last_vis_frame {
        let fx = layout.frame_x(f, scroll_x);
        let frame_num = f + 1;
        let is_major = frame_num == 1 || frame_num % 5 == 0;
        let tick_h = if is_major { 8.0 } else { 4.0 };
        ruler_painter.line_segment(
            [
                Pos2::new(fx, layout.grid_top - tick_h),
                Pos2::new(fx, layout.grid_top),
            ],
            Stroke::new(1.0_f32, Color32::from_rgb(145, 152, 165)),
        );
        if is_major {
            let col = if f < tl.total_frames {
                Color32::from_rgb(40, 45, 55)
            } else {
                Color32::from_rgb(140, 146, 158)
            };
            ruler_painter.text(
                Pos2::new(fx + 3.0, layout.panel_rect.top() + 3.0),
                Align2::LEFT_TOP,
                format!("{frame_num}"),
                FontId::proportional(11.0),
                col,
            );
        }
    }

    let head_x = layout.frame_x(tl.current_frame, scroll_x) + CELL_W * 0.5;
    if head_x >= layout.grid_left - CELL_W && head_x <= layout.panel_rect.right() + CELL_W {
        let head_box = Rect::from_min_max(
            Pos2::new(head_x - CELL_W * 0.5, layout.panel_rect.top() + 1.0),
            Pos2::new(head_x + CELL_W * 0.5, layout.grid_top - 1.0),
        );
        ruler_painter.rect_filled(head_box, 0.0, Color32::from_rgb(225, 50, 50));
        ruler_painter.rect_stroke(
            head_box,
            0.0,
            Stroke::new(1.0_f32, Color32::from_rgb(175, 25, 25)),
            egui::StrokeKind::Middle,
        );
        grid_painter.line_segment(
            [
                Pos2::new(head_x, layout.grid_top),
                Pos2::new(head_x, layout.panel_rect.bottom()),
            ],
            Stroke::new(1.8_f32, Color32::from_rgb(225, 50, 50)),
        );
    }

    ruler_painter.line_segment(
        [
            Pos2::new(layout.grid_left, layout.grid_top),
            Pos2::new(layout.panel_rect.right(), layout.grid_top),
        ],
        Stroke::new(1.0_f32, Color32::from_rgb(180, 186, 198)),
    );
}

/// 6. 左側レイヤー一覧列・D&Dインジケータ・左上ヘッダーセルの描画
fn draw_timeline_layer_list(
    host: &impl TimelineHost,
    painter: &egui::Painter,
    layout: &TimelineLayout,
    primary_down: bool,
    hover_pos: Option<Pos2>,
    drag_layer_feedback: Option<(usize, usize)>,
) {
    let tl = host.tab().active_timeline();
    let timeline_ui = host.timeline_ui();
    let layer_list_rect = Rect::from_min_max(
        Pos2::new(layout.panel_rect.left(), layout.grid_top),
        Pos2::new(layout.grid_left, layout.panel_rect.bottom()),
    );
    let layer_painter = painter.with_clip_rect(layer_list_rect);
    layer_painter.rect_filled(layer_list_rect, 0.0, Color32::from_rgb(238, 240, 245));

    let num_layers = tl.layers.len();
    let scroll_y = timeline_ui.scroll_y;

    for row in 0..num_layers {
        let layer_idx = num_layers.saturating_sub(1 + row);
        let y0 = layout.row_y(row, scroll_y);
        let y1 = y0 + ROW_H;
        if y1 < layout.grid_top || y0 > layout.panel_rect.bottom() {
            continue;
        }
        let row_rect = Rect::from_min_max(
            Pos2::new(layout.panel_rect.left(), y0),
            Pos2::new(layout.grid_left, y1),
        );
        let layer = &tl.layers[layer_idx];
        let is_selected_layer = tl
            .cell_selection
            .is_some_and(|s| s.contains_layer(layer_idx));
        let is_active = layer_idx == tl.active_layer;
        let is_being_dragged = timeline_ui.dragging_layer == Some(layer_idx) && primary_down;

        let row_bg = if is_being_dragged {
            Color32::from_rgb(185, 215, 250)
        } else if is_selected_layer {
            Color32::from_rgb(208, 226, 248)
        } else {
            Color32::from_rgb(245, 247, 250)
        };
        layer_painter.rect_filled(row_rect, 0.0, row_bg);

        if is_active && !is_selected_layer {
            let bar = Rect::from_min_max(
                Pos2::new(layout.panel_rect.left(), y0 + 2.0),
                Pos2::new(layout.panel_rect.left() + 3.0, y1 - 2.0),
            );
            layer_painter.rect_filled(bar, 0.0, Color32::from_rgb(25, 120, 230));
        }

        let cy = (y0 + y1) * 0.5;
        let vis_col = if layer.visible {
            Color32::from_rgb(40, 45, 55)
        } else {
            Color32::from_rgb(155, 162, 174)
        };
        layer_painter.text(
            Pos2::new(layout.panel_rect.left() + LAYER_VIS_ICON_X, cy),
            Align2::CENTER_CENTER,
            if layer.visible {
                icons::VISIBILITY
            } else {
                "・"
            },
            FontId::proportional(13.0),
            vis_col,
        );

        let lock_col = if layer.locked {
            Color32::from_rgb(215, 135, 20)
        } else {
            Color32::from_rgb(155, 162, 174)
        };
        layer_painter.text(
            Pos2::new(layout.panel_rect.left() + LAYER_LOCK_ICON_X, cy),
            Align2::CENTER_CENTER,
            if layer.locked { icons::LOCK } else { "·" },
            FontId::proportional(12.5),
            lock_col,
        );

        let name_color = if !layer.visible {
            Color32::from_rgb(145, 152, 164)
        } else if is_selected_layer || is_active {
            Color32::from_rgb(15, 22, 35)
        } else {
            Color32::from_rgb(45, 50, 62)
        };
        layer_painter.text(
            Pos2::new(layout.panel_rect.left() + LAYER_NAME_X, cy),
            Align2::LEFT_CENTER,
            &layer.name,
            FontId::proportional(LAYER_NAME_FONT_SIZE),
            name_color,
        );

        let grip_col = if is_selected_layer || is_active {
            Color32::from_rgb(75, 105, 150)
        } else {
            Color32::from_rgb(150, 158, 170)
        };
        layer_painter.text(
            Pos2::new(layout.grid_left - LAYER_GRIP_W / 2.0, cy),
            Align2::CENTER_CENTER,
            "≡",
            FontId::proportional(12.0),
            grip_col,
        );

        layer_painter.line_segment(
            [
                Pos2::new(layout.panel_rect.left(), y1),
                Pos2::new(layout.grid_left, y1),
            ],
            Stroke::new(1.0_f32, Color32::from_rgb(202, 208, 218)),
        );
    }

    if let Some((src_idx, dst_idx)) = drag_layer_feedback {
        if dst_idx != src_idx && dst_idx < num_layers {
            let dst_row = num_layers.saturating_sub(1 + dst_idx);
            let drop_y = if dst_idx > src_idx {
                layout.row_y(dst_row, scroll_y)
            } else {
                layout.row_y(dst_row + 1, scroll_y)
            };
            let indicator_col = Color32::from_rgb(0, 130, 235);
            painter.line_segment(
                [
                    Pos2::new(layout.panel_rect.left() + 2.0, drop_y),
                    Pos2::new(layout.panel_rect.right(), drop_y),
                ],
                Stroke::new(2.5_f32, indicator_col),
            );
            painter.circle_filled(
                Pos2::new(layout.panel_rect.left() + 6.0, drop_y),
                3.5,
                indicator_col,
            );
        }
    }

    draw_layer_list_header(painter, layout, num_layers, hover_pos);
}

/// レイヤー一覧左上ヘッダーセル（`"👁 / 🔒 / レイヤー"` + `[＋][🗑]` ミニボタン）と境界線を描画する
fn draw_layer_list_header(
    painter: &egui::Painter,
    layout: &TimelineLayout,
    num_layers: usize,
    hover_pos: Option<Pos2>,
) {
    let corner_rect = Rect::from_min_max(
        layout.panel_rect.left_top(),
        Pos2::new(layout.grid_left, layout.grid_top),
    );
    painter.rect_filled(corner_rect, 0.0, Color32::from_rgb(228, 231, 237));
    let header_cy = layout.panel_rect.top() + RULER_H * 0.5;
    painter.text(
        Pos2::new(layout.panel_rect.left() + LAYER_VIS_ICON_X, header_cy),
        Align2::CENTER_CENTER,
        icons::VISIBILITY,
        FontId::proportional(11.5),
        Color32::from_rgb(75, 82, 95),
    );
    painter.text(
        Pos2::new(layout.panel_rect.left() + LAYER_LOCK_ICON_X, header_cy),
        Align2::CENTER_CENTER,
        icons::LOCK,
        FontId::proportional(11.5),
        Color32::from_rgb(75, 82, 95),
    );
    painter.text(
        Pos2::new(layout.panel_rect.left() + LAYER_NAME_X, header_cy),
        Align2::LEFT_CENTER,
        "レイヤー",
        FontId::proportional(11.5),
        Color32::from_rgb(55, 62, 74),
    );

    let add_hovered = hover_pos.is_some_and(|p| layout.add_hdr_rect.contains(p));
    let del_enabled = num_layers > 1;
    let del_hovered = del_enabled && hover_pos.is_some_and(|p| layout.del_hdr_rect.contains(p));
    draw_header_mini_button(
        painter,
        layout.add_hdr_rect,
        "＋",
        12.0,
        true,
        add_hovered,
        Color32::from_rgb(55, 130, 225),
    );
    draw_header_mini_button(
        painter,
        layout.del_hdr_rect,
        icons::DELETE,
        11.0,
        del_enabled,
        del_hovered,
        Color32::from_rgb(215, 70, 70),
    );

    painter.line_segment(
        [
            Pos2::new(layout.panel_rect.left(), layout.grid_top),
            Pos2::new(layout.grid_left, layout.grid_top),
        ],
        Stroke::new(1.0_f32, Color32::from_rgb(180, 186, 198)),
    );
    painter.line_segment(
        [
            Pos2::new(layout.grid_left, layout.panel_rect.top()),
            Pos2::new(layout.grid_left, layout.panel_rect.bottom()),
        ],
        Stroke::new(1.0_f32, Color32::from_rgb(180, 186, 198)),
    );
}

/// レイヤー一覧左上ヘッダーのミニボタン（`[＋]` / `[🗑]`）を描画するヘルパー
fn draw_header_mini_button(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    font_size: f32,
    enabled: bool,
    hovered: bool,
    hover_fill: Color32,
) {
    painter.rect_filled(
        rect,
        0.0,
        if hovered {
            hover_fill
        } else {
            Color32::from_rgb(212, 217, 226)
        },
    );
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0_f32, Color32::from_rgb(175, 182, 194)),
        egui::StrokeKind::Middle,
    );
    let text_col = if hovered {
        Color32::WHITE
    } else if enabled {
        Color32::from_rgb(35, 40, 50)
    } else {
        Color32::from_rgb(155, 160, 172)
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(font_size),
        text_col,
    );
}

/// タイムラインの横・縦スクロール領域が画面幅・高さを超える場合にスクロールバーを描画する
fn draw_timeline_scrollbars(
    host: &impl TimelineHost,
    painter: &egui::Painter,
    layout: &TimelineLayout,
) {
    let num_layers = host.tab().active_timeline().layers.len();
    for (axis, scroll) in [
        (ScrollAxis::X, host.timeline_ui().scroll_x),
        (ScrollAxis::Y, host.timeline_ui().scroll_y),
    ] {
        if let Some((_, thumb_rect, _, _)) = layout.scrollbar_metrics(axis, num_layers, scroll) {
            painter.rect_filled(
                thumb_rect,
                2.0,
                Color32::from_rgba_unmultiplied(110, 118, 134, 150),
            );
        }
    }
}

// ============================================================================
// 7. タイムライン再生制御（コマ送り・再生/停止・自動フレーム進行）
// ============================================================================

/// 前後のフレームへコマ送り移動する（`,` / `.` キーやボタン操作）
pub(crate) fn step_frame(host: &mut impl TimelineHost, delta: isize) {
    let tl = host.tab().active_timeline();
    let total = tl.total_frames.max(1) as isize;
    let next = if host.timeline_ui().loop_playback {
        (tl.current_frame as isize + delta).rem_euclid(total) as usize
    } else {
        (tl.current_frame as isize + delta).clamp(0, total - 1) as usize
    };
    go_to_frame(host, next);
}

/// 今のレイヤーのまま、コマ `frame` へ移る（コマ送り・Home / End キー・コントロールバーのボタン）
pub(crate) fn go_to_frame(host: &mut impl TimelineHost, frame: usize) {
    let layer_idx = host.tab().active_timeline().active_layer;
    apply(host, Action::SwitchLayerAndFrame { layer_idx, frame });
}

/// 再生 / 一時停止を切り替える（`Enter` キーまたは Play ボタン）
pub(crate) fn toggle_playback(host: &mut impl TimelineHost) {
    if host.timeline_ui().is_playing {
        host.stop_playback();
        apply(host, Action::EndCoalesce { key: None });
    } else {
        // 浮いた片の確定を断られたら、再生を始めない（確定は再生を始める前に行う。断った理由は `apply` が知らせている。
        // レビュー 41766f9 の N-5001：前は断られても再生を始めてコマが進み、止めたあとに確定すると、浮いた片と持ち上げた元の
        // コマの残りが、止まったコマのセル画に書かれてそのコマの絵が消えた）
        if !apply(host, Action::Deselect) {
            return;
        }
        let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
        cancel_active_drag(tab);
        timeline_ui.cancel_drag();
        let loop_pb = timeline_ui.loop_playback;
        let (curr_f, total_f) = {
            let tl = tab.active_timeline();
            (tl.current_frame, tl.total_frames)
        };
        let last_f = total_f.saturating_sub(1);
        if curr_f >= total_f || (!loop_pb && curr_f >= last_f) {
            tab.set_active_frame(0);
        }
        timeline_ui.start_playback(tab);
    }
}

/// 再生中のコマを、音の出力が鳴らした所から決める（音の時計：設計の 5.1。再生を始めたコマの始まりに、鳴らしたサンプルの
/// 数を足した所が入るコマ）。くり返さないときは、最後のコマを鳴らし終えたら最後のコマで止める。音の出力が使えなくなって
/// いたら知らせる
/// （再生中は毎コマの `FillShape` クローンを行わず、`Cel` の結合メッシュキャッシュから直接高速描画する）
pub(crate) fn update_playback(host: &mut impl TimelineHost, ctx: &egui::Context) {
    if let Some(why) = host.timeline_ui_mut().audio.tick() {
        host.notify_warning(why);
    }
    let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
    timeline_ui.ensure_timeline_context(tab);
    if !timeline_ui.is_playing {
        return;
    }
    let fps = tab.doc.fps;
    let now = ctx.input(|i| i.time);
    let rate = timeline_ui.audio.rate();
    match timeline_ui.audio.position(now) {
        Some(pos) => tab.advance_playback_frame(crate::sound::frame_at(pos, rate, fps)),
        None => {
            let last = tab.active_timeline().total_frames.max(1) - 1;
            tab.advance_playback_frame(last);
            timeline_ui.stop_playback(tab);
            return;
        }
    }
    ctx.request_repaint_after(Duration::from_secs_f64(0.5 / f64::from(fps.max(1))));
}

/// 再生していないときに、同じタイムラインの今のコマが変わったら、そのコマの音を1コマ分だけ鳴らす（スクラブ：設計の
/// 5.2。前のスクラブの音は打ち切る）。画面の操作をすべて終えた後に1回呼ぶ。タブの切り替え・シンボルの出入りは、コマを
/// 動かしたのではないので鳴らさない
pub(crate) fn scrub_on_frame_change(host: &mut impl TimelineHost) {
    let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
    let tl = tab.active_timeline();
    let seen = (timeline_key(tab), tl.current_frame);
    let before = timeline_ui.heard.replace(seen);
    if timeline_ui.is_playing || before.is_none_or(|(key, frame)| key != seen.0 || frame == seen.1)
    {
        return;
    }
    timeline_ui
        .audio
        .play(&tab.doc, tl, seen.1, seen.1 + 1, false);
}
