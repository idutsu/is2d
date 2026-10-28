use crate::{
    panels::icons,
    symbol::TweenState,
    timeline::{Timeline, TimelineCellSelection, TimelineGridDrag},
    FlashBrushApp,
};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

const LAYER_COL_W: f32 = 190.0;
const RULER_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const CELL_W: f32 = 16.0;

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
        let last_vis_frame =
            (((scroll_x + grid_w) / CELL_W).ceil() as usize).min(max_frames_shown);

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
        num_layers - 1 - row
    }

    fn frame_x(&self, frame: usize, scroll_x: f32) -> f32 {
        self.grid_left + (frame as f32) * CELL_W - scroll_x
    }

    fn row_y(&self, row: usize, scroll_y: f32) -> f32 {
        self.grid_top + (row as f32) * ROW_H - scroll_y
    }
}

impl Timeline {
    /// 再生ヘッド（`current_frame`）が移動して表示領域外に出た場合のみ、横スクロール位置 `scroll_x` を自動追従させる
    /// （手動のマウスホイールスクロールで再生ヘッドが画面外に出た場合は引き戻さない）
    pub(crate) fn ensure_current_frame_visible(&mut self, grid_width: f32) {
        if self.current_frame == self.last_ensured_frame {
            return;
        }
        self.last_ensured_frame = self.current_frame;
        if grid_width <= CELL_W * 2.0 {
            return;
        }
        let head_left = self.current_frame as f32 * CELL_W;
        let head_right = head_left + CELL_W;
        if head_left < self.scroll_x {
            self.scroll_x = head_left;
        } else if head_right > self.scroll_x + grid_width - CELL_W {
            self.scroll_x = (head_right - grid_width + CELL_W).max(0.0);
        }
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
    painter.rect_stroke(badge_rect, 0.0, Stroke::new(1.2, border_col));
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

impl FlashBrushApp {
    /// 画面下部の Adobe Animate 風タイムラインパネルを描画・操作する（ライトテーマ・角丸なし）
    pub(crate) fn draw_timeline_panel(&mut self, ctx: &egui::Context) {
        self.sync_shapes_to_timeline();

        egui::TopBottomPanel::bottom("timeline_panel")
            .resizable(true)
            .default_height(186.0)
            .height_range(124.0..=360.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(236, 238, 242))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(190, 196, 206))),
            )
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(Color32::from_rgb(35, 40, 48));
                self.draw_timeline_control_bar(ui);

                let (panel_rect, response) =
                    ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
                let painter = ui.painter_at(panel_rect);

                self.timeline
                    .ensure_current_frame_visible((panel_rect.width() - LAYER_COL_W).max(1.0));

                let sel_max = self
                    .timeline
                    .cell_selection
                    .map(|s| s.max_frame + 1)
                    .unwrap_or(0);
                let max_reach_frames = self
                    .timeline
                    .total_frames
                    .max(self.timeline.current_frame + 1)
                    .max(sel_max);
                let mut layout = TimelineLayout::new(
                    panel_rect,
                    max_reach_frames,
                    self.timeline.scroll_x,
                );
                self.handle_timeline_scroll(ui, &response, &layout);
                layout = TimelineLayout::new(
                    panel_rect,
                    max_reach_frames,
                    self.timeline.scroll_x,
                );

                let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
                let primary_down = ui.input(|i| i.pointer.primary_down());
                let alt_held = ui.input(|i| i.modifiers.alt);
                let pointer_pos = ui
                    .input(|i| i.pointer.interact_pos())
                    .or_else(|| response.interact_pointer_pos());
                let hover_pos = ui.input(|i| i.pointer.hover_pos());
                let just_clicked = response.hovered() && primary_pressed;

                let drag_layer_feedback = self.handle_timeline_pointer_input(
                    ui,
                    &layout,
                    just_clicked,
                    primary_down,
                    alt_held,
                    pointer_pos,
                    hover_pos,
                );

                let grid_rect = Rect::from_min_max(
                    Pos2::new(layout.grid_left, layout.grid_top),
                    panel_rect.right_bottom(),
                );
                let grid_painter = painter.with_clip_rect(grid_rect);

                self.draw_timeline_frame_grid(&grid_painter, &layout, alt_held, pointer_pos);
                self.draw_timeline_ruler(&painter, &grid_painter, &layout);
                self.draw_timeline_layer_list(
                    &painter,
                    &layout,
                    primary_down,
                    hover_pos,
                    drag_layer_feedback,
                );
            });
    }

    /// 1. 上部タイムライン操作バーの描画とボタンイベント処理
    fn draw_timeline_control_bar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(Color32::from_rgb(242, 244, 248))
            .stroke(Stroke::new(1.0, Color32::from_rgb(202, 208, 218)))
            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    self.draw_control_bar_layer_buttons(ui);
                    ui.separator();
                    self.draw_control_bar_transport_buttons(ui);
                    ui.separator();
                    self.draw_control_bar_keyframe_buttons(ui);
                    ui.separator();
                    self.draw_control_bar_onion_controls(ui);
                });
            });
    }

    /// コントロールバー：レイヤー追加・削除・上下移動ボタン
    fn draw_control_bar_layer_buttons(&mut self, ui: &mut egui::Ui) {
        let len = self.timeline.layers.len();
        let act = self.timeline.active_layer;
        if ui
            .button("＋ レイヤー")
            .on_hover_text("新しいレイヤーを現在のレイヤーの前面に追加")
            .clicked()
        {
            self.timeline_add_layer();
        }
        if ui
            .add_enabled(len > 1, egui::Button::new(format!("{} 削除", icons::DELETE)))
            .on_hover_text("現在選択中のレイヤーを削除")
            .clicked()
        {
            self.timeline_delete_active_layer();
        }
        if ui
            .add_enabled(act + 1 < len, egui::Button::new("▲"))
            .on_hover_text("レイヤーを1つ前面（上）へ移動（ドラッグ＆ドロップでも入れ替え可能）")
            .clicked()
        {
            self.timeline_move_active_layer(true);
        }
        if ui
            .add_enabled(act > 0, egui::Button::new("▼"))
            .on_hover_text("レイヤーを1つ背面（下）へ移動（ドラッグ＆ドロップでも入れ替え可能）")
            .clicked()
        {
            self.timeline_move_active_layer(false);
        }
    }

    /// コントロールバー：再生・コマ送り・総フレーム数・FPS設定
    fn draw_control_bar_transport_buttons(&mut self, ui: &mut egui::Ui) {
        if ui
            .button(icons::SKIP_PREV)
            .on_hover_text("最初のフレームへ (Home)")
            .clicked()
        {
            self.switch_to_layer_and_frame(self.timeline.active_layer, 0);
        }
        if ui
            .button(icons::STEP_PREV)
            .on_hover_text("前のフレームへ (, / ←)")
            .clicked()
        {
            self.step_frame(-1);
        }
        let play_label = if self.timeline.is_playing {
            format!("{} Stop (Enter)", icons::PAUSE)
        } else {
            format!("{} Play (Enter)", icons::PLAY)
        };
        if ui
            .selectable_label(self.timeline.is_playing, play_label)
            .on_hover_text("再生 / 一時停止 (Enter)")
            .clicked()
        {
            self.toggle_playback();
        }
        if ui
            .button(icons::STEP_NEXT)
            .on_hover_text("次のフレームへ (. / →)")
            .clicked()
        {
            self.step_frame(1);
        }
        if ui
            .button(icons::SKIP_NEXT)
            .on_hover_text("最後のフレームへ (End)")
            .clicked()
        {
            let last = self.timeline.total_frames.saturating_sub(1);
            self.switch_to_layer_and_frame(self.timeline.active_layer, last);
        }

        ui.separator();

        ui.label(format!("Frame: {:>2} /", self.timeline.current_frame + 1));
        let mut total_f = self.timeline.total_frames as u32;
        if ui
            .add(
                egui::DragValue::new(&mut total_f)
                    .range(1..=600)
                    .speed(0.25)
                    .suffix(" f"),
            )
            .on_hover_text("総フレーム数 (1〜600)")
            .changed()
        {
            let next_total = (total_f as usize).max(1);
            for layer in &mut self.timeline.layers {
                layer.frame_count = next_total;
            }
            self.timeline.sync_total_frames();
            if self.timeline.current_frame >= self.timeline.total_frames {
                self.switch_to_layer_and_frame(
                    self.timeline.active_layer,
                    self.timeline.total_frames - 1,
                );
            }
        }

        ui.label("FPS:");
        ui.add(egui::DragValue::new(&mut self.timeline.fps).range(1..=60).speed(0.25));
    }

    /// コントロールバー：キーフレーム挿入・削除・フレーム延長/短縮・クラシックトゥイーンボタン
    fn draw_control_bar_keyframe_buttons(&mut self, ui: &mut egui::Ui) {
        let editable = !self.timeline.target_layers_for_frame_op().is_empty();
        if ui
            .add_enabled(editable, egui::Button::new("● キーフレーム (F6)"))
            .on_hover_text("現在の絵を複製して新しいキーフレームを挿入 (F6)")
            .clicked()
        {
            self.timeline_insert_keyframe(true);
        }
        if ui
            .add_enabled(editable, egui::Button::new("○ 空白KF (F7)"))
            .on_hover_text("空の新しいキーフレームを挿入 (F7)")
            .clicked()
        {
            self.timeline_insert_keyframe(false);
        }
        if ui
            .add_enabled(editable, egui::Button::new("＋ 延長 (F5)"))
            .on_hover_text("選択レイヤー（未選択時は全レイヤー）のフレーム尺を1コマ延長 (F5)")
            .clicked()
        {
            self.timeline_extend_frame();
        }
        if ui
            .add_enabled(
                editable && self.timeline.total_frames > 1,
                egui::Button::new("－ 短縮"),
            )
            .on_hover_text(
                "選択レイヤー（未選択時は全レイヤー）のフレームを1コマ削除・短縮 (Shift+F5)",
            )
            .clicked()
        {
            self.timeline_remove_frame();
        }
        if ui
            .add_enabled(editable, egui::Button::new("× KF削除"))
            .on_hover_text("現在のキーフレームを削除 / クリア (Shift+F6)")
            .clicked()
        {
            self.timeline_delete_keyframe();
        }

        let (can_tween, is_tweened) = self.can_toggle_tween_at_current_frame();
        let tween_btn = egui::Button::new(
            egui::RichText::new("⇄ トゥイーン (T)").color(if is_tweened {
                Color32::from_rgb(75, 30, 150)
            } else if can_tween {
                Color32::from_rgb(35, 40, 48)
            } else {
                Color32::from_rgb(150, 156, 168)
            }),
        )
        .fill(if is_tweened {
            Color32::from_rgb(218, 198, 250)
        } else {
            Color32::from_rgb(248, 249, 252)
        })
        .stroke(Stroke::new(
            1.0,
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
            self.timeline_toggle_tween();
        }
    }

    /// コントロールバー：オニオンスキン ON/OFF・前後フレーム数設定
    fn draw_control_bar_onion_controls(&mut self, ui: &mut egui::Ui) {
        if ui
            .selectable_label(
                self.timeline.onion_skin_enabled,
                format!("{} Onion (O)", icons::ONION_SKIN),
            )
            .on_hover_text("オニオンスキン（前後のフレームの透かし表示）切替 (O)")
            .clicked()
        {
            self.timeline.onion_skin_enabled = !self.timeline.onion_skin_enabled;
        }
        if self.timeline.onion_skin_enabled {
            let mut prev_n = self.timeline.onion_prev_frames as u32;
            let mut next_n = self.timeline.onion_next_frames as u32;
            ui.label("前:");
            if ui
                .add(egui::DragValue::new(&mut prev_n).range(0..=5).speed(0.1))
                .changed()
            {
                self.timeline.onion_prev_frames = prev_n as usize;
            }
            ui.label("後:");
            if ui
                .add(egui::DragValue::new(&mut next_n).range(0..=5).speed(0.1))
                .changed()
            {
                self.timeline.onion_next_frames = next_n as usize;
            }
        }
    }

    /// 2. マウスホイールによるタイムライン横・縦スクロール更新と範囲クランプ
    fn handle_timeline_scroll(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        layout: &TimelineLayout,
    ) {
        let max_scroll_x = ((layout.max_frames_shown as f32) * CELL_W - layout.grid_w).max(0.0);
        let max_scroll_y = ((self.timeline.layers.len() as f32) * ROW_H - layout.grid_h).max(0.0);

        let pointer_in_timeline =
            response.hovered() || ui.rect_contains_pointer(ui.max_rect());
        if pointer_in_timeline {
            let scroll_delta = ui.input(|i| i.smooth_scroll_delta);
            if scroll_delta.x.abs() > 0.1 || scroll_delta.y.abs() > 0.1 {
                let pointer_in_layers = ui
                    .input(|i| i.pointer.hover_pos())
                    .is_some_and(|p| p.x < layout.grid_left && p.y > layout.grid_top);
                if pointer_in_layers
                    && max_scroll_y > 0.0
                    && scroll_delta.y.abs() >= scroll_delta.x.abs()
                {
                    self.timeline.scroll_y -= scroll_delta.y;
                } else {
                    let dx = if scroll_delta.x.abs() > scroll_delta.y.abs() {
                        scroll_delta.x
                    } else {
                        scroll_delta.y
                    };
                    self.timeline.scroll_x -= dx;
                }
            }
        }

        self.timeline.scroll_x = self.timeline.scroll_x.clamp(0.0, max_scroll_x);
        self.timeline.scroll_y = self.timeline.scroll_y.clamp(0.0, max_scroll_y);
    }

    /// 3. タイムラインパネル内のクリック・ドラッグ入力処理（ルーラースクラブ・セル範囲選択・フレーム移動/複製・レイヤーD&D）
    #[allow(clippy::too_many_arguments)]
    fn handle_timeline_pointer_input(
        &mut self,
        ui: &egui::Ui,
        layout: &TimelineLayout,
        just_clicked: bool,
        primary_down: bool,
        alt_held: bool,
        pointer_pos: Option<Pos2>,
        hover_pos: Option<Pos2>,
    ) -> Option<(usize, usize, Pos2)> {
        if just_clicked {
            if let Some(pos) = pointer_pos {
                self.handle_timeline_click_start(layout, pos, alt_held);
            }
        }
        self.update_timeline_grid_drag(ui, layout, primary_down, alt_held, pointer_pos);
        self.update_timeline_layer_drag(ui, layout, primary_down, pointer_pos, hover_pos)
    }

    /// タイムライン上のマウス押下開始時のクリック判定（レイヤー列 / ルーラー / フレームグリッド）
    fn handle_timeline_click_start(
        &mut self,
        layout: &TimelineLayout,
        pos: Pos2,
        alt_held: bool,
    ) {
        if layout.add_hdr_rect.contains(pos) {
            self.timeline_add_layer();
        } else if layout.del_hdr_rect.contains(pos) && self.timeline.layers.len() > 1 {
            self.timeline_delete_active_layer();
        } else if pos.x < layout.grid_left && pos.y >= layout.grid_top {
            let num_layers = self.timeline.layers.len();
            if let Some(layer_idx) = layout.layer_at_y(pos.y, self.timeline.scroll_y, num_layers) {
                let rel_x = pos.x - layout.panel_rect.left();
                if (6.0..30.0).contains(&rel_x) {
                    let next_vis = !self.timeline.layers[layer_idx].visible;
                    self.set_layer_visible(layer_idx, next_vis);
                } else if (30.0..54.0).contains(&rel_x) {
                    let next_lock = !self.timeline.layers[layer_idx].locked;
                    self.set_layer_locked(layer_idx, next_lock);
                } else {
                    let cf = self.timeline.current_frame;
                    self.select_timeline_cells(layer_idx, cf, layer_idx, cf);
                    self.timeline.dragging_layer = Some(layer_idx);
                }
            } else {
                self.timeline.cell_selection = None;
            }
        } else if self.timeline.dragging_layer.is_none() && pos.x >= layout.grid_left {
            let col = layout.frame_at_x(pos.x, self.timeline.scroll_x);
            if pos.y < layout.grid_top {
                let target_frame = col.min(self.timeline.total_frames.saturating_sub(1));
                self.timeline.grid_drag = TimelineGridDrag::ScrubRuler;
                self.scrub_ruler_to_frame(target_frame);
            } else {
                let num_layers = self.timeline.layers.len();
                if let Some(target_layer) =
                    layout.layer_at_y(pos.y, self.timeline.scroll_y, num_layers)
                {
                    let layer_fc = self.timeline.layers[target_layer].frame_count;
                    let target_frame = col;
                    self.stop_playback();

                    // 実際に存在するフレーム（col < layer_fc）の上をクリックしたときのみ MoveFrames 判定を行う
                    // （未作成の後ろの空マスをクリックした際はその空マスを選択状態にし、F5 / F6 / F7 でそこまで尺を伸ばせるようにする）
                    let clicked_on_existing_frame = col < layer_fc;
                    let clicked_in_existing_sel = clicked_on_existing_frame
                        && self
                            .timeline
                            .cell_selection
                            .is_some_and(|s| s.contains(target_layer, col));

                    if clicked_in_existing_sel || (clicked_on_existing_frame && alt_held) {
                        let orig_sel = if let Some(sel) = self
                            .timeline
                            .cell_selection
                            .filter(|s| s.contains(target_layer, col))
                        {
                            sel
                        } else {
                            self.select_timeline_cells(target_layer, col, target_layer, col);
                            TimelineCellSelection::single(target_layer, col)
                        };
                        self.timeline.grid_drag = TimelineGridDrag::MoveFrames {
                            orig_sel,
                            grab_layer: target_layer,
                            grab_frame: col,
                            curr_layer: target_layer,
                            curr_frame: col,
                            copy_mode: alt_held,
                            has_moved: false,
                        };
                    } else {
                        self.select_timeline_cells(
                            target_layer,
                            target_frame,
                            target_layer,
                            target_frame,
                        );
                        self.timeline.grid_drag = TimelineGridDrag::SelectCells {
                            start_layer: target_layer,
                            start_frame: target_frame,
                        };
                    }
                } else {
                    self.timeline.cell_selection = None;
                }
            }
        }
    }

    /// ルーラースクラブ・セル範囲選択・フレーム移動/複製ドラッグの継続および確定処理
    fn update_timeline_grid_drag(
        &mut self,
        ui: &egui::Ui,
        layout: &TimelineLayout,
        primary_down: bool,
        alt_held: bool,
        pointer_pos: Option<Pos2>,
    ) {
        match self.timeline.grid_drag {
            TimelineGridDrag::None => {}
            TimelineGridDrag::ScrubRuler => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        let col = layout.frame_at_x(pos.x, self.timeline.scroll_x);
                        self.scrub_ruler_to_frame(
                            col.min(self.timeline.total_frames.saturating_sub(1)),
                        );
                    }
                } else {
                    self.timeline.grid_drag = TimelineGridDrag::None;
                }
            }
            TimelineGridDrag::SelectCells {
                start_layer,
                start_frame,
            } => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        let target_frame = layout.frame_at_x(pos.x, self.timeline.scroll_x);
                        let target_layer = layout.clamped_layer_at_y(
                            pos.y,
                            self.timeline.scroll_y,
                            self.timeline.layers.len(),
                        );
                        self.select_timeline_cells(
                            start_layer,
                            start_frame,
                            target_layer,
                            target_frame,
                        );
                    }
                } else {
                    self.timeline.grid_drag = TimelineGridDrag::None;
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
                        let col = layout.frame_at_x(pos.x, self.timeline.scroll_x);
                        let next_frame = col.min(layout.max_frames_shown.saturating_sub(1));
                        let next_layer = layout.clamped_layer_at_y(
                            pos.y,
                            self.timeline.scroll_y,
                            self.timeline.layers.len(),
                        );
                        let moved =
                            has_moved || next_layer != grab_layer || next_frame != grab_frame;
                        self.timeline.grid_drag = TimelineGridDrag::MoveFrames {
                            orig_sel,
                            grab_layer,
                            grab_frame,
                            curr_layer: next_layer,
                            curr_frame: next_frame,
                            copy_mode: alt_held,
                            has_moved: moved,
                        };
                        if moved {
                            ui.ctx().set_cursor_icon(if alt_held {
                                egui::CursorIcon::Copy
                            } else {
                                egui::CursorIcon::Grabbing
                            });
                        }
                    }
                } else {
                    self.timeline.grid_drag = TimelineGridDrag::None;
                    if has_moved {
                        let layer_delta = curr_layer as isize - grab_layer as isize;
                        let frame_delta = curr_frame as isize - grab_frame as isize;
                        self.timeline_move_or_copy_frames(
                            orig_sel,
                            layer_delta,
                            frame_delta,
                            copy_mode || alt_held,
                        );
                    } else {
                        self.select_timeline_cells(
                            grab_layer,
                            grab_frame,
                            grab_layer,
                            grab_frame,
                        );
                    }
                }
            }
        }
    }

    /// レイヤーD&D並び替えの継続およびドロップ確定処理
    fn update_timeline_layer_drag(
        &mut self,
        ui: &egui::Ui,
        layout: &TimelineLayout,
        primary_down: bool,
        pointer_pos: Option<Pos2>,
        hover_pos: Option<Pos2>,
    ) -> Option<(usize, usize, Pos2)> {
        let mut drag_layer_feedback: Option<(usize, usize, Pos2)> = None;
        if let Some(src_idx) = self.timeline.dragging_layer {
            let num_layers = self.timeline.layers.len();
            if src_idx >= num_layers {
                self.timeline.dragging_layer = None;
            } else if let Some(pos) = pointer_pos {
                let dst_idx = layout.clamped_layer_at_y(pos.y, self.timeline.scroll_y, num_layers);
                if primary_down {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    drag_layer_feedback = Some((src_idx, dst_idx, pos));
                } else {
                    self.timeline.dragging_layer = None;
                    self.timeline_reorder_layer(src_idx, dst_idx);
                }
            } else if !primary_down {
                self.timeline.dragging_layer = None;
            }
        } else if let Some(hpos) = hover_pos {
            if layout.panel_rect.contains(hpos)
                && hpos.x < layout.grid_left
                && hpos.y >= layout.grid_top
                && (hpos.x - layout.panel_rect.left()) >= 54.0
                && layout
                    .layer_at_y(hpos.y, self.timeline.scroll_y, self.timeline.layers.len())
                    .is_some()
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
        drag_layer_feedback
    }

    /// 4. 右側フレームグリッド（キーフレーム帯・セル選択ハイライト・フレーム移動ゴースト枠）の描画
    fn draw_timeline_frame_grid(
        &self,
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

        let num_layers = self.timeline.layers.len();
        let (scroll_x, scroll_y) = (self.timeline.scroll_x, self.timeline.scroll_y);

        for row in 0..num_layers {
            let layer_idx = num_layers - 1 - row;
            let y0 = layout.row_y(row, scroll_y);
            let y1 = y0 + ROW_H;
            if y1 < layout.grid_top || y0 > layout.panel_rect.bottom() {
                continue;
            }

            let layer = &self.timeline.layers[layer_idx];
            let is_layer_selected = self
                .timeline
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
                    Stroke::new(1.0, Color32::from_rgb(214, 218, 226)),
                );
            }

            if layer.frame_count > 0 {
                for (kf_idx, kf) in layer.keyframes.iter().enumerate() {
                    let span_start = kf.start_frame;
                    if span_start >= layer.frame_count {
                        continue;
                    }
                    let span_end = layer.span_end_for_keyframe(kf_idx);
                    if span_end >= layout.first_vis_frame && span_start <= layout.last_vis_frame {
                        let tween_state = layer.tween_state_at_keyframe(kf_idx);
                        Self::draw_keyframe_span(
                            grid_painter,
                            layout,
                            scroll_x,
                            y0,
                            y1,
                            span_start,
                            span_end,
                            !kf.cel.is_empty(),
                            is_layer_selected,
                            tween_state,
                        );
                    }
                }
            }

            if let Some(sel) = self
                .timeline
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
                    Stroke::new(1.6, Color32::from_rgb(0, 115, 230)),
                );
            }

            grid_painter.line_segment(
                [
                    Pos2::new(layout.grid_left, y1),
                    Pos2::new(layout.panel_rect.right(), y1),
                ],
                Stroke::new(1.0, Color32::from_rgb(195, 200, 210)),
            );
        }

        // フレーム移動／Alt複製ドラッグ中のゴーストプレビュー矩形とバッジ
        if let TimelineGridDrag::MoveFrames {
            orig_sel,
            grab_layer,
            grab_frame,
            curr_layer,
            curr_frame,
            copy_mode,
            has_moved: true,
        } = self.timeline.grid_drag
        {
            let layer_delta = curr_layer as isize - grab_layer as isize;
            let frame_delta = curr_frame as isize - grab_frame as isize;
            let max_l = (num_layers.saturating_sub(1)) as isize;
            let dst_min_l = (orig_sel.min_layer as isize + layer_delta).clamp(0, max_l) as usize;
            let dst_max_l = (orig_sel.max_layer as isize + layer_delta).clamp(0, max_l) as usize;
            let dst_min_f = (orig_sel.min_frame as isize + frame_delta).max(0) as usize;
            let dst_max_f = (orig_sel.max_frame as isize + frame_delta).max(0) as usize;

            let ghost_rect = Rect::from_min_max(
                Pos2::new(
                    layout.frame_x(dst_min_f, scroll_x),
                    layout.row_y(num_layers - 1 - dst_max_l, scroll_y),
                ),
                Pos2::new(
                    layout.frame_x(dst_max_f + 1, scroll_x),
                    layout.row_y(num_layers - dst_min_l, scroll_y),
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
            grid_painter.rect_stroke(ghost_rect, 0.0, Stroke::new(2.0, accent));

            if let Some(pos) = pointer_pos {
                let badge_label = if is_copy {
                    "＋ Copy Frames (Alt)"
                } else {
                    "↔ Move Frames"
                };
                let badge_rect = Rect::from_min_size(
                    Pos2::new(
                        (pos.x + 12.0).clamp(
                            layout.grid_left + 4.0,
                            layout.panel_rect.right() - 135.0,
                        ),
                        (pos.y - 22.0).clamp(
                            layout.grid_top + 2.0,
                            layout.panel_rect.bottom() - 22.0,
                        ),
                    ),
                    Vec2::new(130.0, 20.0),
                );
                draw_floating_drag_badge(grid_painter, badge_rect, accent, badge_label, 11.0);
            }
        }

        for f in layout.first_vis_frame..=layout.last_vis_frame {
            if (f + 1) % 5 == 0 {
                let fx = layout.frame_x(f + 1, scroll_x);
                grid_painter.line_segment(
                    [
                        Pos2::new(fx, layout.grid_top),
                        Pos2::new(fx, layout.panel_rect.bottom()),
                    ],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(150, 158, 174, 130)),
                );
            }
        }
    }

    /// 1つのキーフレーム保持区間（帯・1コマ区切り線・先頭丸記号 `●`/`○`・終端マーク `▯` またはトゥイーン点線/矢印）を描画する
    #[allow(clippy::too_many_arguments)]
    fn draw_keyframe_span(
        grid_painter: &egui::Painter,
        layout: &TimelineLayout,
        scroll_x: f32,
        y0: f32,
        y1: f32,
        span_start: usize,
        span_end: usize,
        has_art: bool,
        is_layer_selected: bool,
        tween_state: TweenState,
    ) {
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
        for f in
            (span_start.max(layout.first_vis_frame) + 1)..=span_end.min(layout.last_vis_frame)
        {
            let fx = layout.frame_x(f, scroll_x);
            grid_painter.line_segment(
                [Pos2::new(fx, y0 + 1.0), Pos2::new(fx, y1 - 1.0)],
                Stroke::new(1.0, sep_col),
            );
        }
        let border_col = if is_tween {
            Color32::from_rgb(146, 116, 194)
        } else {
            Color32::from_rgb(155, 162, 175)
        };
        grid_painter.rect_stroke(span_rect, 0.0, Stroke::new(1.0, border_col));

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
                Stroke::new(1.5, Color32::from_rgb(75, 82, 95)),
            );
        }

        match tween_state {
            TweenState::None => {
                if span_end > span_start {
                    let end_mark_x = sx1 - CELL_W * 0.45;
                    let end_box = Rect::from_center_size(
                        Pos2::new(end_mark_x, kf_center.y),
                        Vec2::new(4.2, 8.5),
                    );
                    grid_painter.rect_filled(end_box, 0.0, Color32::WHITE);
                    grid_painter.rect_stroke(
                        end_box,
                        0.0,
                        Stroke::new(1.2, Color32::from_rgb(90, 98, 112)),
                    );
                }
            }
            TweenState::Incomplete => {
                // 終了キーフレームが存在しない（または条件不一致）→ 横点線（`----`）を描画
                let line_x0 = sx0 + CELL_W * 0.5 + 5.0;
                let line_x1 = sx1 - 2.0;
                if line_x1 > line_x0 {
                    let dash_col = Color32::from_rgb(90, 50, 160);
                    let mut x = line_x0;
                    while x < line_x1 {
                        let x_end = (x + 4.0).min(line_x1);
                        grid_painter.line_segment(
                            [Pos2::new(x, kf_center.y), Pos2::new(x_end, kf_center.y)],
                            Stroke::new(1.5, dash_col),
                        );
                        x += 7.0;
                    }
                }
            }
            TweenState::Complete { .. } => {
                // 終了キーフレームが存在する完全トゥイーン → 実線矢印（`───▶`）を描画
                let line_x0 = sx0 + CELL_W * 0.5 + 5.0;
                let line_x1 = sx1 - 2.0;
                let arrow_col = Color32::from_rgb(72, 32, 145);
                if line_x1 > line_x0 {
                    let shaft_end = (line_x1 - 4.0).max(line_x0);
                    grid_painter.line_segment(
                        [Pos2::new(line_x0, kf_center.y), Pos2::new(shaft_end, kf_center.y)],
                        Stroke::new(1.6, arrow_col),
                    );
                }
                let tip = Pos2::new(line_x1, kf_center.y);
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
        &self,
        painter: &egui::Painter,
        grid_painter: &egui::Painter,
        layout: &TimelineLayout,
    ) {
        let ruler_rect = Rect::from_min_max(
            Pos2::new(layout.grid_left, layout.panel_rect.top()),
            Pos2::new(layout.panel_rect.right(), layout.grid_top),
        );
        let ruler_painter = painter.with_clip_rect(ruler_rect);
        ruler_painter.rect_filled(ruler_rect, 0.0, Color32::from_rgb(228, 231, 237));

        let scroll_x = self.timeline.scroll_x;
        if self.timeline.onion_skin_enabled {
            let cf = self.timeline.current_frame;
            let prev_start = cf.saturating_sub(self.timeline.onion_prev_frames);
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
            let next_end = (cf + self.timeline.onion_next_frames)
                .min(self.timeline.total_frames.saturating_sub(1));
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
                Stroke::new(1.0, Color32::from_rgb(145, 152, 165)),
            );
            if is_major {
                let col = if f < self.timeline.total_frames {
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

        let head_x = layout.frame_x(self.timeline.current_frame, scroll_x) + CELL_W * 0.5;
        if head_x >= layout.grid_left - CELL_W && head_x <= layout.panel_rect.right() + CELL_W {
            let head_box = Rect::from_min_max(
                Pos2::new(head_x - CELL_W * 0.5, layout.panel_rect.top() + 1.0),
                Pos2::new(head_x + CELL_W * 0.5, layout.grid_top - 1.0),
            );
            ruler_painter.rect_filled(head_box, 0.0, Color32::from_rgb(225, 50, 50));
            ruler_painter.rect_stroke(head_box, 0.0, Stroke::new(1.0, Color32::from_rgb(175, 25, 25)));
            ruler_painter.text(
                head_box.center(),
                Align2::CENTER_CENTER,
                format!("{}", self.timeline.current_frame + 1),
                FontId::proportional(11.0),
                Color32::WHITE,
            );
            grid_painter.line_segment(
                [
                    Pos2::new(head_x, layout.grid_top),
                    Pos2::new(head_x, layout.panel_rect.bottom()),
                ],
                Stroke::new(1.8, Color32::from_rgb(225, 50, 50)),
            );
        }

        ruler_painter.line_segment(
            [
                Pos2::new(layout.grid_left, layout.grid_top),
                Pos2::new(layout.panel_rect.right(), layout.grid_top),
            ],
            Stroke::new(1.0, Color32::from_rgb(180, 186, 198)),
        );
    }

    /// 6. 左側レイヤー一覧列・D&Dインジケータ・左上ヘッダーセルの描画
    fn draw_timeline_layer_list(
        &self,
        painter: &egui::Painter,
        layout: &TimelineLayout,
        primary_down: bool,
        hover_pos: Option<Pos2>,
        drag_layer_feedback: Option<(usize, usize, Pos2)>,
    ) {
        let layer_list_rect = Rect::from_min_max(
            Pos2::new(layout.panel_rect.left(), layout.grid_top),
            Pos2::new(layout.grid_left, layout.panel_rect.bottom()),
        );
        let layer_painter = painter.with_clip_rect(layer_list_rect);
        layer_painter.rect_filled(layer_list_rect, 0.0, Color32::from_rgb(238, 240, 245));

        let num_layers = self.timeline.layers.len();
        let scroll_y = self.timeline.scroll_y;

        for row in 0..num_layers {
            let layer_idx = num_layers - 1 - row;
            let y0 = layout.row_y(row, scroll_y);
            let y1 = y0 + ROW_H;
            if y1 < layout.grid_top || y0 > layout.panel_rect.bottom() {
                continue;
            }
            let row_rect = Rect::from_min_max(
                Pos2::new(layout.panel_rect.left(), y0),
                Pos2::new(layout.grid_left, y1),
            );
            let layer = &self.timeline.layers[layer_idx];
            let is_selected_layer = self
                .timeline
                .cell_selection
                .is_some_and(|s| s.contains_layer(layer_idx));
            let is_active = layer_idx == self.timeline.active_layer;
            let is_being_dragged = self.timeline.dragging_layer == Some(layer_idx) && primary_down;

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
                Pos2::new(layout.panel_rect.left() + 18.0, cy),
                Align2::CENTER_CENTER,
                if layer.visible { icons::VISIBILITY } else { "・" },
                FontId::proportional(13.0),
                vis_col,
            );

            let lock_col = if layer.locked {
                Color32::from_rgb(215, 135, 20)
            } else {
                Color32::from_rgb(155, 162, 174)
            };
            layer_painter.text(
                Pos2::new(layout.panel_rect.left() + 42.0, cy),
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
                Pos2::new(layout.panel_rect.left() + 60.0, cy),
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
                Stroke::new(1.0, Color32::from_rgb(202, 208, 218)),
            );
        }

        if let Some((src_idx, dst_idx, drag_pos)) = drag_layer_feedback {
            if dst_idx != src_idx && dst_idx < num_layers {
                let dst_row = num_layers - 1 - dst_idx;
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
                    Stroke::new(2.5, indicator_col),
                );
                painter.circle_filled(
                    Pos2::new(layout.panel_rect.left() + 6.0, drop_y),
                    3.5,
                    indicator_col,
                );
            }
            if let Some(src_layer) = self.timeline.layers.get(src_idx) {
                let badge_rect = Rect::from_min_size(
                    Pos2::new(
                        drag_pos
                            .x
                            .clamp(layout.panel_rect.left() + 8.0, layout.grid_left - 90.0),
                        (drag_pos.y - 11.0)
                            .clamp(layout.grid_top + 2.0, layout.panel_rect.bottom() - 22.0),
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

        // 左上ヘッダーセル ("👁 / 🔒 / Layers" + [＋][🗑] ミニボタン)
        let corner_rect = Rect::from_min_max(
            layout.panel_rect.left_top(),
            Pos2::new(layout.grid_left, layout.grid_top),
        );
        painter.rect_filled(corner_rect, 0.0, Color32::from_rgb(228, 231, 237));
        let header_cy = layout.panel_rect.top() + RULER_H * 0.5;
        painter.text(
            Pos2::new(layout.panel_rect.left() + 18.0, header_cy),
            Align2::CENTER_CENTER,
            icons::VISIBILITY,
            FontId::proportional(11.5),
            Color32::from_rgb(75, 82, 95),
        );
        painter.text(
            Pos2::new(layout.panel_rect.left() + 42.0, header_cy),
            Align2::CENTER_CENTER,
            icons::LOCK,
            FontId::proportional(11.5),
            Color32::from_rgb(75, 82, 95),
        );
        painter.text(
            Pos2::new(layout.panel_rect.left() + 60.0, header_cy),
            Align2::LEFT_CENTER,
            "Layers",
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
            Stroke::new(1.0, Color32::from_rgb(180, 186, 198)),
        );
        painter.line_segment(
            [
                Pos2::new(layout.grid_left, layout.panel_rect.top()),
                Pos2::new(layout.grid_left, layout.panel_rect.bottom()),
            ],
            Stroke::new(1.0, Color32::from_rgb(180, 186, 198)),
        );
    }
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
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_rgb(175, 182, 194)));
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
