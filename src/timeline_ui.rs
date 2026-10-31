use std::time::Duration;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::canvas_tool::{CoalesceKey, Tab, commit_floating, load_shapes_from_timeline};
use crate::doc_actions::{Action, ActionHost, apply};
#[cfg(test)]
pub(crate) use crate::doc_actions::{
    scrub_ruler_to_frame, select_timeline_cells, set_layer_locked, set_layer_visible,
    switch_to_layer_and_frame, timeline_add_layer, timeline_delete_active_layer,
    timeline_delete_keyframe, timeline_extend_frame, timeline_insert_keyframe,
    timeline_move_active_layer, timeline_move_or_copy_frames, timeline_remove_frame,
    timeline_reorder_layer,
};
use crate::symbol::TweenState;
use crate::timeline::{MAX_TIMELINE_FRAMES, TimelineCellSelection};

/// タイムライングリッドの1フレームあたりのセル幅（px）
pub(crate) const CELL_W: f32 = 16.0;

const LAYER_COL_W: f32 = 190.0;
const RULER_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const LAYER_VIS_HIT_RANGE: std::ops::Range<f32> = 6.0..30.0;
const LAYER_LOCK_HIT_RANGE: std::ops::Range<f32> = 30.0..54.0;
const LAYER_VIS_ICON_X: f32 = 18.0;
const LAYER_LOCK_ICON_X: f32 = 42.0;
const LAYER_NAME_X: f32 = 60.0;

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
// 0. タイムラインUI状態とホスト抽象化
// ============================================================================

/// タブ状態（`Tab`）とタイムラインUI状態（`TimelineUiState`）の組を抽象化するトレイト
pub(crate) trait TimelineHost: ActionHost {
    fn timeline_ui(&self) -> &TimelineUiState;
    fn timeline_ui_mut(&mut self) -> &mut TimelineUiState;
    fn tab_and_timeline_ui_mut(&mut self) -> (&mut Tab, &mut TimelineUiState);
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

/// タイムラインパネルの再生状態・ループ設定・スクロール位置・ドラッグ操作等のUI状態
#[derive(Clone, Debug)]
pub(crate) struct TimelineUiState {
    pub(crate) is_playing: bool,
    pub(crate) loop_playback: bool,
    pub(crate) last_frame_advance_time: Option<f64>,
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f32,
    pub(crate) last_ensured_frame: usize,
    pub(crate) dragging_layer: Option<usize>,
    pub(crate) grid_drag: TimelineGridDrag,
    /// 直近に同期したタイムライン識別キー `(doc_id, edit_stack_len, active_symbol_id)`
    last_timeline_key: (u64, usize, Option<u64>),
}

impl Default for TimelineUiState {
    fn default() -> Self {
        Self {
            is_playing: false,
            loop_playback: true,
            last_frame_advance_time: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            last_ensured_frame: 0,
            dragging_layer: None,
            grid_drag: TimelineGridDrag::None,
            last_timeline_key: (1, 0, None),
        }
    }
}

impl TimelineUiState {
    pub(crate) fn cancel_drag(&mut self) {
        self.dragging_layer = None;
        self.grid_drag = TimelineGridDrag::None;
    }

    pub(crate) fn reset_for_timeline_switch(&mut self, _current_frame: usize) {
        self.is_playing = false;
        self.last_frame_advance_time = None;
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.last_ensured_frame = usize::MAX;
        self.cancel_drag();
    }

    /// 再生中であれば停止し、停止時点のフレームの図形群を作業用 `tab.session.shapes` に展開する
    pub(crate) fn stop_playback(&mut self, tab: &mut Tab) {
        if self.is_playing {
            self.is_playing = false;
            self.last_frame_advance_time = None;
            load_shapes_from_timeline(tab);
        }
    }

    /// `tab` の現在のタイムライン識別キー `(id, edit_stack_len, active_symbol_id)` を読み取り、
    /// シンボル進入/退出やタブ切り替えが発生していればスクロール・ドラッグ状態を自動リセットする
    pub(crate) fn ensure_timeline_context(&mut self, tab: &Tab) {
        let key = (
            tab.id,
            tab.session.edit_path.len(),
            tab.session.edit_path.last().map(|s| s.symbol_id),
        );
        if self.last_timeline_key != key {
            self.last_timeline_key = key;
            self.reset_for_timeline_switch(tab.active_timeline().current_frame);
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
        let max_frames_shown = (max_reach_frames + 24).max(60);
        let first_vis_frame = (scroll_x / CELL_W).floor() as usize;
        let last_vis_frame = (((scroll_x + grid_w) / CELL_W).ceil() as usize).min(max_frames_shown);

        let add_hdr_rect = Rect::from_min_max(
            Pos2::new(grid_left - 44.0, panel_rect.top() + 2.0),
            Pos2::new(grid_left - 24.0, grid_top - 2.0),
        );
        let del_hdr_rect = Rect::from_min_max(
            Pos2::new(grid_left - 22.0, panel_rect.top() + 2.0),
            Pos2::new(grid_left - 2.0, grid_top - 2.0),
        );

        Self {
            panel_rect,
            grid_left,
            grid_top,
            grid_w,
            grid_h,
            max_frames_shown,
            first_vis_frame,
            last_vis_frame,
            add_hdr_rect,
            del_hdr_rect,
        }
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
        self.last_vis_frame = (((scroll_x + self.grid_w) / CELL_W).ceil() as usize)
            .min(self.max_frames_shown);
    }

    fn horizontal_scrollbar_metrics(&self, scroll_x: f32) -> Option<(Rect, Rect, f32, f32)> {
        let total_w = (self.max_frames_shown as f32) * CELL_W;
        if total_w <= self.grid_w || self.grid_w <= 16.0 {
            return None;
        }
        let bar_h = 5.0;
        let track_y0 = self.panel_rect.bottom() - bar_h - 1.0;
        let track_y1 = self.panel_rect.bottom() - 1.0;
        let thumb_w = ((self.grid_w / total_w) * self.grid_w).clamp(24.0, self.grid_w);
        let max_scroll_x = (total_w - self.grid_w).max(1.0);
        let ratio_x = (scroll_x / max_scroll_x).clamp(0.0, 1.0);
        let thumb_x0 = self.grid_left + ratio_x * (self.grid_w - thumb_w);
        let thumb_rect = Rect::from_min_max(
            Pos2::new(thumb_x0, track_y0),
            Pos2::new(thumb_x0 + thumb_w, track_y1),
        );
        let hit_rect = Rect::from_min_max(
            Pos2::new(self.grid_left, (self.panel_rect.bottom() - 9.0).max(self.grid_top)),
            Pos2::new(self.panel_rect.right() - 8.0, self.panel_rect.bottom()),
        );
        Some((hit_rect, thumb_rect, thumb_w, max_scroll_x))
    }

    fn vertical_scrollbar_metrics(
        &self,
        num_layers: usize,
        scroll_y: f32,
    ) -> Option<(Rect, Rect, f32, f32)> {
        let total_h = (num_layers as f32) * ROW_H;
        if total_h <= self.grid_h || self.grid_h <= 16.0 {
            return None;
        }
        let bar_w = 5.0;
        let track_x0 = self.panel_rect.right() - bar_w - 1.0;
        let track_x1 = self.panel_rect.right() - 1.0;
        let thumb_h = ((self.grid_h / total_h) * self.grid_h).clamp(20.0, self.grid_h);
        let max_scroll_y = (total_h - self.grid_h).max(1.0);
        let ratio_y = (scroll_y / max_scroll_y).clamp(0.0, 1.0);
        let thumb_y0 = self.grid_top + ratio_y * (self.grid_h - thumb_h);
        let thumb_rect = Rect::from_min_max(
            Pos2::new(track_x0, thumb_y0),
            Pos2::new(track_x1, thumb_y0 + thumb_h),
        );
        let hit_rect = Rect::from_min_max(
            Pos2::new((self.panel_rect.right() - 9.0).max(self.grid_left), self.grid_top),
            Pos2::new(self.panel_rect.right(), self.panel_rect.bottom()),
        );
        Some((hit_rect, thumb_rect, thumb_h, max_scroll_y))
    }
}

/// ドラッグ中（フレーム移動/複製・レイヤー入れ替え）にカーソル脇へ表示するフローティングバッジを描画する
fn draw_floating_drag_badge(
    painter: &egui::Painter,
    badge_rect: Rect,
    border_col: Color32,
    label: &str,
    font_size: f32,
) {
    painter.rect_filled(
        badge_rect,
        0.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, 242),
    );
    painter.rect_stroke(badge_rect, 0.0, Stroke::new(1.2_f32, border_col));
    painter.text(
        badge_rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(font_size),
        Color32::from_rgb(20, 25, 35),
    );
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
            egui::Frame::none()
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
            let sel_max = tl
                .cell_selection
                .map(|s| s.max_frame + 1)
                .unwrap_or(0);
            let max_reach_frames = tl
                .total_frames
                .max(current_frame + 1)
                .max(sel_max);
            let mut layout =
                TimelineLayout::new(panel_rect, max_reach_frames, host.timeline_ui().scroll_x);
            handle_timeline_scroll(host, ui, &response, &mut layout);

            let primary_down = ui.input(|i| i.pointer.primary_down());
            let alt_held = ui.input(|i| i.modifiers.alt);
            let pointer_pos = ui
                .input(|i| i.pointer.interact_pos())
                .or_else(|| response.interact_pointer_pos());
            let hover_pos = ui.input(|i| i.pointer.hover_pos());

            let drag_layer_feedback =
                handle_timeline_pointer_input(host, ui, &response, &mut layout);

            let grid_rect = Rect::from_min_max(
                Pos2::new(layout.grid_left, layout.grid_top),
                panel_rect.right_bottom(),
            );
            let grid_painter = painter.with_clip_rect(grid_rect);

            draw_timeline_frame_grid(host, &grid_painter, &layout, alt_held, pointer_pos);
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
        });
}

/// 1. 上部タイムライン操作バーの描画とボタンイベント処理
fn draw_timeline_control_bar(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    egui::Frame::none()
        .fill(Color32::from_rgb(242, 244, 248))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(202, 208, 218)))
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                draw_control_bar_layer_buttons(host, ui);
                ui.separator();
                draw_control_bar_transport_buttons(host, ui);
                ui.separator();
                draw_control_bar_keyframe_buttons(host, ui);
                ui.separator();
                draw_control_bar_onion_controls(host.tab_mut(), ui);
            });
            let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
            timeline_ui.ensure_timeline_context(tab);
        });
}

/// コントロールバー：レイヤー追加・削除・上下移動ボタン
fn draw_control_bar_layer_buttons(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    let tl = host.tab().active_timeline();
    let len = tl.layers.len();
    let act = tl.active_layer;
    if ui
        .button("＋ レイヤー")
        .on_hover_text("新しいレイヤーを現在のレイヤーの前面に追加")
        .clicked()
    {
        apply(host, Action::AddLayer);
    }
    if ui
        .add_enabled(
            len > 1,
            egui::Button::new(format!("{} 削除", icons::DELETE)),
        )
        .on_hover_text("現在選択中のレイヤーを削除")
        .clicked()
    {
        apply(host, Action::DeleteActiveLayer);
    }
    if ui
        .add_enabled(act + 1 < len, egui::Button::new("▲"))
        .on_hover_text("レイヤーを1つ前面（上）へ移動（ドラッグ＆ドロップでも入れ替え可能）")
        .clicked()
    {
        apply(host, Action::MoveActiveLayer { up: true });
    }
    if ui
        .add_enabled(act > 0, egui::Button::new("▼"))
        .on_hover_text("レイヤーを1つ背面（下）へ移動（ドラッグ＆ドロップでも入れ替え可能）")
        .clicked()
    {
        apply(host, Action::MoveActiveLayer { up: false });
    }
}

/// コントロールバー：再生・コマ送り・総フレーム数・FPS設定
fn draw_control_bar_transport_buttons(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    if ui
        .button(icons::SKIP_PREV)
        .on_hover_text("最初のフレームへ (Home)")
        .clicked()
    {
        let act = host.tab().active_timeline().active_layer;
        apply(
            host,
            Action::SwitchLayerAndFrame {
                layer_idx: act,
                frame: 0,
            },
        );
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
        format!("{} 停止 (Enter)", icons::PAUSE)
    } else {
        format!("{} 再生 (Enter)", icons::PLAY)
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
        let tl = host.tab().active_timeline();
        let last = tl.total_frames.saturating_sub(1);
        let act = tl.active_layer;
        apply(
            host,
            Action::SwitchLayerAndFrame {
                layer_idx: act,
                frame: last,
            },
        );
    }
    let loop_pb = host.timeline_ui().loop_playback;
    if ui
        .selectable_label(loop_pb, "↻")
        .on_hover_text("ループ再生 ON / OFF（OFF時は最終フレームで自動停止）")
        .clicked()
    {
        host.timeline_ui_mut().loop_playback = !loop_pb;
    }

    ui.separator();

    ui.label(format!(
        "フレーム: {:>2} /",
        host.tab().active_timeline().current_frame + 1
    ));
    let mut total_f = host.tab().active_timeline().total_frames as u32;
    let total_resp = ui
        .add(
            egui::DragValue::new(&mut total_f)
                .range(1..=(MAX_TIMELINE_FRAMES as u32))
                .clamp_existing_to_range(false)
                .update_while_editing(false)
                .speed(0.25)
                .suffix(" f"),
        )
        .on_hover_text(format!("総フレーム数 (1〜{})", MAX_TIMELINE_FRAMES));
    if total_resp.changed() {
        let next_total = (total_f as usize).max(1);
        apply(host, Action::SetTotalFrames { total: next_total });
    }
    if (total_resp.drag_stopped()
        || total_resp.lost_focus()
        || (!total_resp.dragged() && !total_resp.has_focus()))
        && host.tab().history.last_coalesce_key == Some(CoalesceKey::TotalFrames)
    {
        host.tab_mut().history.end_coalesce();
    }

    ui.label("FPS:");
    let mut fps_val = host.tab().active_timeline().fps;
    let fps_resp = ui.add(
        egui::DragValue::new(&mut fps_val)
            .range(1..=60)
            .update_while_editing(false)
            .speed(0.25),
    );
    if fps_resp.changed() {
        let next_fps = fps_val.clamp(1, 60);
        if apply(host, Action::SetFps { fps: next_fps }).as_bool() && host.timeline_ui().is_playing
        {
            host.timeline_ui_mut().last_frame_advance_time = None;
        }
    }
    if (fps_resp.drag_stopped()
        || fps_resp.lost_focus()
        || (!fps_resp.dragged() && !fps_resp.has_focus()))
        && host.tab().history.last_coalesce_key == Some(CoalesceKey::Fps)
    {
        host.tab_mut().history.end_coalesce();
    }
}

/// コントロールバー：キーフレーム挿入・削除・フレーム延長/短縮・クラシックトゥイーンボタン
fn draw_control_bar_keyframe_buttons(host: &mut impl TimelineHost, ui: &mut egui::Ui) {
    let tl = host.tab().active_timeline();
    let targets = tl.target_layers_for_frame_op();
    let editable = !targets.is_empty();
    let can_remove_frame = tl.can_remove_frame_on_targets(&targets);
    let can_delete_kf = tl.can_delete_keyframe_on_targets(&targets);
    if ui
        .add_enabled(editable, egui::Button::new("● キーフレーム (F6)"))
        .on_hover_text("現在の絵を複製して新しいキーフレームを挿入 (F6)")
        .clicked()
    {
        apply(host, Action::InsertKeyframe { copy_prev: true });
    }
    if ui
        .add_enabled(editable, egui::Button::new("○ 空白KF (F7)"))
        .on_hover_text("空の新しいキーフレームを挿入 (F7)")
        .clicked()
    {
        apply(host, Action::InsertKeyframe { copy_prev: false });
    }
    if ui
        .add_enabled(editable, egui::Button::new("＋ 延長 (F5)"))
        .on_hover_text("選択レイヤー（未選択時は全レイヤー）のフレーム尺を1コマ延長 (F5)")
        .clicked()
    {
        apply(host, Action::ExtendFrame);
    }
    if ui
        .add_enabled(can_remove_frame, egui::Button::new("－ 短縮"))
        .on_hover_text("選択レイヤー（未選択時は全レイヤー）のフレームを1コマ削除・短縮 (Shift+F5)")
        .clicked()
    {
        apply(host, Action::RemoveFrame);
    }
    if ui
        .add_enabled(can_delete_kf, egui::Button::new("× KF削除"))
        .on_hover_text("現在のキーフレームを削除 / クリア (Shift+F6)")
        .clicked()
    {
        apply(host, Action::DeleteKeyframe);
    }

    let (can_tween, is_tweened) = host
        .tab()
        .active_timeline()
        .can_toggle_tween_at_current_frame();
    let tween_btn = egui::Button::new(egui::RichText::new("⇄ トゥイーン (T)").color(
        if is_tweened {
            Color32::from_rgb(75, 30, 150)
        } else if can_tween {
            Color32::from_rgb(35, 40, 48)
        } else {
            Color32::from_rgb(150, 156, 168)
        },
    ))
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
        .selectable_label(
            tab.view.onion_skin_enabled,
            format!("{} オニオンスキン (O)", icons::ONION_SKIN),
        )
        .on_hover_text("オニオンスキン（前後のフレームの透かし表示）切替 (O)")
        .clicked()
    {
        tab.view.onion_skin_enabled = !tab.view.onion_skin_enabled;
    }
    if tab.view.onion_skin_enabled {
        let mut prev_n = tab.view.onion_prev_frames as u32;
        let mut next_n = tab.view.onion_next_frames as u32;
        ui.label("前:");
        if ui
            .add(
                egui::DragValue::new(&mut prev_n)
                    .range(0..=10)
                    .update_while_editing(false)
                    .speed(0.1),
            )
            .changed()
        {
            tab.view.onion_prev_frames = prev_n as usize;
        }
        ui.label("後:");
        if ui
            .add(
                egui::DragValue::new(&mut next_n)
                    .range(0..=10)
                    .update_while_editing(false)
                    .speed(0.1),
            )
            .changed()
        {
            tab.view.onion_next_frames = next_n as usize;
        }
    }
}

/// 2. マウスホイールによるタイムライン横・縦スクロール更新と範囲クランプ
fn handle_timeline_scroll(
    host: &mut impl TimelineHost,
    ui: &egui::Ui,
    response: &egui::Response,
    layout: &mut TimelineLayout,
) {
    let max_scroll_x = ((layout.max_frames_shown as f32) * CELL_W - layout.grid_w).max(0.0);
    let max_scroll_y =
        ((host.tab().active_timeline().layers.len() as f32) * ROW_H - layout.grid_h).max(0.0);

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
    layout.first_vis_frame = (timeline_ui.scroll_x / CELL_W).floor() as usize;
    layout.last_vis_frame = (((timeline_ui.scroll_x + layout.grid_w) / CELL_W).ceil() as usize)
        .min(layout.max_frames_shown);
}

/// 3. タイムラインパネル内のクリック・ドラッグ入力処理（ルーラースクラブ・セル範囲選択・フレーム移動/複製・レイヤーD&D・スクロールバー操作）
fn handle_timeline_pointer_input(
    host: &mut impl TimelineHost,
    ui: &egui::Ui,
    response: &egui::Response,
    layout: &mut TimelineLayout,
) -> Option<(usize, usize, Pos2)> {
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let alt_held = ui.input(|i| i.modifiers.alt);
    let shift_held = ui.input(|i| i.modifiers.shift);
    let pointer_pos = ui
        .input(|i| i.pointer.interact_pos())
        .or_else(|| response.interact_pointer_pos());
    let hover_pos = ui.input(|i| i.pointer.hover_pos());
    if response.hovered() && primary_pressed {
        if let Some(pos) = pointer_pos {
            handle_timeline_click_start(host, layout, pos, alt_held, shift_held);
        }
    }
    update_timeline_grid_drag(host, layout, primary_down, alt_held, pointer_pos);
    let drag_layer_feedback = update_timeline_layer_drag(host, layout, primary_down, pointer_pos);
    update_timeline_cursor_icon(
        host,
        ui.ctx(),
        layout,
        alt_held,
        hover_pos,
        drag_layer_feedback.is_some(),
    );
    let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
    timeline_ui.ensure_timeline_context(tab);
    drag_layer_feedback
}

/// タイムライン上のマウス押下開始時のクリック判定（レイヤー列 / ルーラー / スクロールバー / フレームグリッド）
fn handle_timeline_click_start(
    host: &mut impl TimelineHost,
    layout: &mut TimelineLayout,
    pos: Pos2,
    alt_held: bool,
    shift_held: bool,
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
            host.tab_mut()
                .with_active_timeline_mut(|tl, _| tl.cell_selection = None);
        }
    } else if host.timeline_ui().dragging_layer.is_none() && pos.x >= layout.grid_left {
        let num_layers = host.tab().active_timeline().layers.len();
        let scroll_y = host.timeline_ui().scroll_y;
        if let Some((hit_rect, thumb_rect, thumb_h, max_scroll_y)) =
            layout.vertical_scrollbar_metrics(num_layers, scroll_y)
        {
            if hit_rect.contains(pos) {
                let grab_offset_y = if pos.y >= thumb_rect.top() && pos.y <= thumb_rect.bottom() {
                    pos.y - thumb_rect.top()
                } else {
                    thumb_h * 0.5
                };
                let track_span = (layout.grid_h - thumb_h).max(1.0);
                let ratio_y =
                    ((pos.y - grab_offset_y - layout.grid_top) / track_span).clamp(0.0, 1.0);
                let timeline_ui = host.timeline_ui_mut();
                timeline_ui.scroll_y = ratio_y * max_scroll_y;
                timeline_ui.grid_drag = TimelineGridDrag::ScrubScrollbarY { grab_offset_y };
                return;
            }
        }
        let scroll_x = host.timeline_ui().scroll_x;
        if let Some((hit_rect, thumb_rect, thumb_w, max_scroll_x)) =
            layout.horizontal_scrollbar_metrics(scroll_x)
        {
            if hit_rect.contains(pos) {
                let grab_offset_x = if pos.x >= thumb_rect.left() && pos.x <= thumb_rect.right() {
                    pos.x - thumb_rect.left()
                } else {
                    thumb_w * 0.5
                };
                let track_span = (layout.grid_w - thumb_w).max(1.0);
                let ratio_x =
                    ((pos.x - grab_offset_x - layout.grid_left) / track_span).clamp(0.0, 1.0);
                let new_scroll_x = ratio_x * max_scroll_x;
                let timeline_ui = host.timeline_ui_mut();
                timeline_ui.scroll_x = new_scroll_x;
                layout.sync_visible_frames(new_scroll_x);
                timeline_ui.grid_drag = TimelineGridDrag::ScrubScrollbarX { grab_offset_x };
                return;
            }
        }

        let col = layout.frame_at_x(pos.x, host.timeline_ui().scroll_x);
        if pos.y < layout.grid_top {
            let target_frame =
                col.min(host.tab().active_timeline().total_frames.saturating_sub(1));
            host.timeline_ui_mut().grid_drag = TimelineGridDrag::ScrubRuler;
            apply(host, Action::ScrubRulerToFrame { frame: target_frame });
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
                let orig_sel = if let Some(sel) = tl
                    .cell_selection
                    .filter(|s| s.contains(target_layer, col))
                {
                    apply(host, Action::EndCoalesce);
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
            host.tab_mut()
                .with_active_timeline_mut(|tl, _| tl.cell_selection = None);
        }
    }
}

/// ルーラースクラブ・スクロールバー操作・セル範囲選択・フレーム移動/複製ドラッグの継続および確定処理
fn update_timeline_grid_drag(
    host: &mut impl TimelineHost,
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
        TimelineGridDrag::ScrubScrollbarX { grab_offset_x } => {
            if primary_down {
                let scroll_x = host.timeline_ui().scroll_x;
                if let (Some(pos), Some((_, _, thumb_w, max_scroll_x))) =
                    (pointer_pos, layout.horizontal_scrollbar_metrics(scroll_x))
                {
                    let track_span = (layout.grid_w - thumb_w).max(1.0);
                    let ratio_x =
                        ((pos.x - grab_offset_x - layout.grid_left) / track_span).clamp(0.0, 1.0);
                    let new_scroll_x = ratio_x * max_scroll_x;
                    host.timeline_ui_mut().scroll_x = new_scroll_x;
                    layout.sync_visible_frames(new_scroll_x);
                }
            } else {
                host.timeline_ui_mut().grid_drag = TimelineGridDrag::None;
            }
        }
        TimelineGridDrag::ScrubScrollbarY { grab_offset_y } => {
            if primary_down {
                let num_layers = host.tab().active_timeline().layers.len();
                let scroll_y = host.timeline_ui().scroll_y;
                if let (Some(pos), Some((_, _, thumb_h, max_scroll_y))) = (
                    pointer_pos,
                    layout.vertical_scrollbar_metrics(num_layers, scroll_y),
                ) {
                    let track_span = (layout.grid_h - thumb_h).max(1.0);
                    let ratio_y =
                        ((pos.y - grab_offset_y - layout.grid_top) / track_span).clamp(0.0, 1.0);
                    host.timeline_ui_mut().scroll_y = ratio_y * max_scroll_y;
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
    layout: &TimelineLayout,
    primary_down: bool,
    pointer_pos: Option<Pos2>,
) -> Option<(usize, usize, Pos2)> {
    let src_idx = host.timeline_ui().dragging_layer?;
    let num_layers = host.tab().active_timeline().layers.len();
    if src_idx >= num_layers {
        host.timeline_ui_mut().dragging_layer = None;
        return None;
    }
    if let Some(pos) = pointer_pos {
        let dst_idx = layout.clamped_layer_at_y(pos.y, host.timeline_ui().scroll_y, num_layers);
        if primary_down {
            return Some((src_idx, dst_idx, pos));
        }
        host.timeline_ui_mut().dragging_layer = None;
        apply(host, Action::ReorderLayer { src_idx, dst_idx });
    } else if !primary_down {
        host.timeline_ui_mut().dragging_layer = None;
    }
    None
}

/// タイムライン上のドラッグ状態およびホバー位置に応じてOSカーソルアイコンを更新する
fn update_timeline_cursor_icon(
    host: &impl TimelineHost,
    ctx: &egui::Context,
    layout: &TimelineLayout,
    alt_held: bool,
    hover_pos: Option<Pos2>,
    is_dragging_layer: bool,
) {
    if let TimelineGridDrag::MoveFrames {
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

/// 4. 右側フレームグリッド（キーフレーム帯・セル選択ハイライト・フレーム移動ゴースト枠）の描画
fn draw_timeline_frame_grid(
    host: &impl TimelineHost,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
    alt_held: bool,
    pointer_pos: Option<Pos2>,
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
        draw_timeline_layer_row(host, grid_painter, layout, layer_idx, (y0, y1));
    }

    draw_move_frames_ghost(host, grid_painter, layout, alt_held, pointer_pos);

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
    (y0, y1): (f32, f32),
) {
    let tab = host.tab();
    let tl = tab.active_timeline();
    let scroll_x = host.timeline_ui().scroll_x;
    let layer = &tl.layers[layer_idx];
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

    if layer.frame_count > 0 {
        for (kf_idx, kf) in layer.keyframes.iter().enumerate() {
            if kf.start_frame < layer.frame_count
                && layer.span_end_for_keyframe(kf_idx) >= layout.first_vis_frame
                && kf.start_frame <= layout.last_vis_frame
            {
                draw_keyframe_span(host, grid_painter, layout, layer_idx, kf_idx, (y0, y1));
            }
        }
    }

    if let Some(sel) = tl
        .cell_selection
        .filter(|s| s.contains_layer(layer_idx))
    {
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
    pointer_pos: Option<Pos2>,
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
    grid_painter.rect_stroke(ghost_rect, 0.0, Stroke::new(2.0_f32, accent));

    if let Some(pos) = pointer_pos {
        let badge_label = if is_copy {
            "＋ コマを複製 (Alt)"
        } else {
            "↔ コマを移動"
        };
        let min_bx = layout.grid_left + 4.0;
        let max_bx = (layout.panel_rect.right() - 135.0).max(min_bx);
        let min_by = layout.grid_top + 2.0;
        let max_by = (layout.panel_rect.bottom() - 22.0).max(min_by);
        let badge_rect = Rect::from_min_size(
            Pos2::new(
                (pos.x + 12.0).clamp(min_bx, max_bx),
                (pos.y - 22.0).clamp(min_by, max_by),
            ),
            Vec2::new(130.0, 20.0),
        );
        draw_floating_drag_badge(grid_painter, badge_rect, accent, badge_label, 11.0);
    }
}

/// 1つのキーフレーム保持区間（帯・1コマ区切り線・先頭丸記号 `●`/`○`・終端マーク `▯` またはトゥイーン点線/矢印）を描画する
fn draw_keyframe_span(
    host: &impl TimelineHost,
    grid_painter: &egui::Painter,
    layout: &TimelineLayout,
    layer_idx: usize,
    kf_idx: usize,
    (y0, y1): (f32, f32),
) {
    let tl = host.tab().active_timeline();
    let layer = &tl.layers[layer_idx];
    let kf = &layer.keyframes[kf_idx];
    let span_start = kf.start_frame;
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
    let border_col = if is_tween {
        Color32::from_rgb(146, 116, 194)
    } else {
        Color32::from_rgb(155, 162, 175)
    };
    grid_painter.rect_stroke(span_rect, 0.0, Stroke::new(1.0_f32, border_col));

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
        let next_end = (cf + tab.view.onion_next_frames).min(tl.total_frames.saturating_sub(1));
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
        );
        ruler_painter.text(
            head_box.center(),
            Align2::CENTER_CENTER,
            format!("{}", tl.current_frame + 1),
            FontId::proportional(11.0),
            Color32::WHITE,
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
    drag_layer_feedback: Option<(usize, usize, Pos2)>,
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
            FontId::proportional(12.5),
            name_color,
        );

        let grip_col = if is_selected_layer || is_active {
            Color32::from_rgb(75, 105, 150)
        } else {
            Color32::from_rgb(150, 158, 170)
        };
        layer_painter.text(
            Pos2::new(layout.grid_left - 12.0, cy),
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

    if let Some((src_idx, dst_idx, drag_pos)) = drag_layer_feedback {
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
        if let Some(src_layer) = tl.layers.get(src_idx) {
            let min_bx = layout.panel_rect.left() + 8.0;
            let max_bx = (layout.grid_left - 90.0).max(min_bx);
            let min_by = layout.grid_top + 2.0;
            let max_by = (layout.panel_rect.bottom() - 22.0).max(min_by);
            let badge_rect = Rect::from_min_size(
                Pos2::new(
                    drag_pos.x.clamp(min_bx, max_bx),
                    (drag_pos.y - 11.0).clamp(min_by, max_by),
                ),
                Vec2::new(105.0, 20.0),
            );
            draw_floating_drag_badge(
                painter,
                badge_rect,
                Color32::from_rgb(0, 130, 235),
                &format!("↕ {}", src_layer.name),
                11.5,
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
    if let Some((_, thumb_rect, _, _)) =
        layout.horizontal_scrollbar_metrics(host.timeline_ui().scroll_x)
    {
        painter.rect_filled(
            thumb_rect,
            2.0,
            Color32::from_rgba_unmultiplied(110, 118, 134, 150),
        );
    }

    if let Some((_, thumb_rect, _, _)) = layout.vertical_scrollbar_metrics(
        host.tab().active_timeline().layers.len(),
        host.timeline_ui().scroll_y,
    ) {
        painter.rect_filled(
            thumb_rect,
            2.0,
            Color32::from_rgba_unmultiplied(110, 118, 134, 150),
        );
    }
}

// ============================================================================
// 7. タイムライン再生制御（コマ送り・再生/停止・自動フレーム進行）
// ============================================================================

/// 前後のフレームへコマ送り移動する（`,` / `.` キーやボタン操作）
pub(crate) fn step_frame(host: &mut impl TimelineHost, delta: isize) {
    let (active_layer, next) = {
        let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
        timeline_ui.ensure_timeline_context(tab);
        let tl = tab.active_timeline();
        let total = tl.total_frames.max(1) as isize;
        let next = if timeline_ui.loop_playback {
            (tl.current_frame as isize + delta).rem_euclid(total) as usize
        } else {
            (tl.current_frame as isize + delta).clamp(0, total - 1) as usize
        };
        (tl.active_layer, next)
    };
    apply(
        host,
        Action::SwitchLayerAndFrame {
            layer_idx: active_layer,
            frame: next,
        },
    );
}

/// 再生 / 一時停止を切り替える（`Enter` キーまたは Play ボタン）
pub(crate) fn toggle_playback(host: &mut impl TimelineHost) {
    let is_playing = {
        let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
        timeline_ui.ensure_timeline_context(tab);
        timeline_ui.is_playing
    };
    if is_playing {
        apply(host, Action::EndCoalesce);
    } else {
        apply(host, Action::Deselect);
        let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
        commit_floating(tab);
        tab.history.end_coalesce();
        timeline_ui.cancel_drag();
        let loop_pb = timeline_ui.loop_playback;
        tab.with_active_timeline_mut(|tl, _| {
            let last_f = tl.total_frames.saturating_sub(1);
            if tl.current_frame >= tl.total_frames || (!loop_pb && tl.current_frame >= last_f) {
                tl.current_frame = 0;
            }
        });
        timeline_ui.is_playing = true;
        timeline_ui.last_frame_advance_time = None;
    }
}

/// 再生中のフレーム自動進行を処理する
/// （再生中は毎コマの `FillShape` クローンを行わず、`Cel` の結合メッシュキャッシュから直接高速描画する）
pub(crate) fn update_playback(host: &mut impl TimelineHost, ctx: &egui::Context) {
    let (tab, timeline_ui) = host.tab_and_timeline_ui_mut();
    timeline_ui.ensure_timeline_context(tab);
    if !timeline_ui.is_playing {
        timeline_ui.last_frame_advance_time = None;
        return;
    }
    let fps = tab.active_timeline().fps.clamp(1, 60);
    let frame_interval = 1.0 / (fps as f64);
    let now = ctx.input(|i| i.time);

    if let Some(last_time) = timeline_ui.last_frame_advance_time {
        let elapsed = now - last_time;
        if elapsed >= frame_interval {
            let steps = ((elapsed / frame_interval).floor() as usize).max(1);
            timeline_ui.last_frame_advance_time =
                Some(last_time + (steps as f64) * frame_interval);
            let loop_pb = timeline_ui.loop_playback;
            let stopped = tab.with_active_timeline_mut(|tl, _| {
                let total = tl.total_frames.max(1);
                if loop_pb {
                    tl.current_frame = (tl.current_frame + steps) % total;
                    false
                } else {
                    let next = tl.current_frame + steps;
                    if next >= total - 1 {
                        tl.current_frame = total - 1;
                        true
                    } else {
                        tl.current_frame = next;
                        false
                    }
                }
            });
            if stopped {
                timeline_ui.is_playing = false;
                timeline_ui.last_frame_advance_time = None;
                load_shapes_from_timeline(tab);
                return;
            }
        }
    } else {
        timeline_ui.last_frame_advance_time = Some(now);
    }

    ctx.request_repaint_after(Duration::from_secs_f64(frame_interval * 0.5));
}
