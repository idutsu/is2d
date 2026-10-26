use crate::{vector_engine::FillShape, FlashBrushApp};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::time::Duration;

const LAYER_COL_W: f32 = 190.0;
const RULER_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const CELL_W: f32 = 16.0;

/// 1枚のセル画（ベクター図形群 `Vec<FillShape>`）
#[derive(Clone, Debug, Default)]
pub(crate) struct Cel {
    pub(crate) shapes: Vec<FillShape>,
}

impl Cel {
    pub(crate) fn new(shapes: Vec<FillShape>) -> Self {
        Self { shapes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

/// タイムライン上のキーフレーム（開始フレーム番号 0-indexed とセル画 `Cel`）
#[derive(Clone, Debug)]
pub(crate) struct Keyframe {
    pub(crate) start_frame: usize,
    pub(crate) cel: Cel,
}

impl Keyframe {
    pub(crate) fn new_blank(start_frame: usize) -> Self {
        Self {
            start_frame,
            cel: Cel::default(),
        }
    }
}

/// タイムラインの1レイヤー（名前、表示/非表示、ロック状態、開始フレーム順にソートされたキーフレーム配列）
#[derive(Clone, Debug)]
pub(crate) struct Layer {
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
    pub(crate) keyframes: Vec<Keyframe>,
}

impl Layer {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            locked: false,
            keyframes: vec![Keyframe::new_blank(0)],
        }
    }

    /// 指定フレーム番号 `frame` において有効な（保持されている）キーフレームのインデックスを返す
    pub(crate) fn keyframe_index_at(&self, frame: usize) -> Option<usize> {
        let idx = self.keyframes.partition_point(|kf| kf.start_frame <= frame);
        idx.checked_sub(1)
    }

    /// 指定フレーム番号 `frame` で表示される `Cel` への不変参照を返す
    pub(crate) fn cel_at(&self, frame: usize) -> Option<&Cel> {
        let idx = self.keyframe_index_at(frame)?;
        Some(&self.keyframes[idx].cel)
    }

    /// 指定フレーム番号 `frame` で編集対象となる `Cel` への可変参照を返す
    pub(crate) fn cel_at_mut(&mut self, frame: usize) -> Option<&mut Cel> {
        let idx = self.keyframe_index_at(frame)?;
        Some(&mut self.keyframes[idx].cel)
    }

    /// 指定フレーム `frame` にキーフレームを挿入する。
    /// - `clone_prev == true`（`F6` キーフレーム挿入）：直前のキーフレームの図形群を複製して新しいキーフレームを作る。
    /// - `clone_prev == false`（`F7` 空白キーフレーム挿入）：空の `Cel` を持つ新しいキーフレームを作る。
    pub(crate) fn insert_keyframe(&mut self, frame: usize, clone_prev: bool) -> usize {
        if let Some(existing_idx) = self
            .keyframes
            .iter()
            .position(|kf| kf.start_frame == frame)
        {
            if !clone_prev {
                self.keyframes[existing_idx].cel.shapes.clear();
            }
            return existing_idx;
        }

        let cloned_shapes = if clone_prev {
            self.cel_at(frame)
                .map(|c| c.shapes.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let insert_idx = self.keyframes.partition_point(|kf| kf.start_frame < frame);
        self.keyframes.insert(
            insert_idx,
            Keyframe {
                start_frame: frame,
                cel: Cel::new(cloned_shapes),
            },
        );
        insert_idx
    }

    /// 指定フレーム `frame` を含むキーフレームを削除（フレーム0の場合は空白化）する（`Shift+F6`）
    pub(crate) fn delete_keyframe_at(&mut self, frame: usize) -> bool {
        let Some(idx) = self.keyframe_index_at(frame) else {
            return false;
        };
        if idx == 0 {
            if self.keyframes[0].cel.is_empty() {
                return false;
            }
            self.keyframes[0].cel.shapes.clear();
            true
        } else {
            self.keyframes.remove(idx);
            true
        }
    }

    /// 指定フレーム `frame` の保持尺を1フレーム延長し、後続のキーフレームを +1 フレーム後ろへずらす（`F5`）
    pub(crate) fn insert_frame_at(&mut self, frame: usize) {
        for kf in &mut self.keyframes {
            if kf.start_frame > frame {
                kf.start_frame += 1;
            }
        }
    }

    /// 指定フレーム `frame` の保持尺を1フレーム短縮し、後続のキーフレームを -1 フレーム前へ詰める（`Shift+F5`）
    pub(crate) fn remove_frame_at(&mut self, frame: usize) {
        for kf in &mut self.keyframes {
            if kf.start_frame > frame {
                kf.start_frame -= 1;
            }
        }
        self.keyframes.dedup_by_key(|kf| kf.start_frame);
        if self.keyframes.is_empty() {
            self.keyframes.push(Keyframe::new_blank(0));
        } else {
            self.keyframes[0].start_frame = 0;
        }
    }
}

/// レイヤー配列・現在のフレーム・FPS・再生制御・オニオンスキン設定を管理するタイムライン構造体
#[derive(Clone, Debug)]
pub(crate) struct Timeline {
    pub(crate) layers: Vec<Layer>,
    pub(crate) current_frame: usize,
    pub(crate) active_layer: usize,
    pub(crate) total_frames: usize,
    pub(crate) fps: u32,
    pub(crate) is_playing: bool,
    pub(crate) last_frame_advance_time: Option<f64>,
    pub(crate) onion_skin_enabled: bool,
    pub(crate) onion_prev_frames: usize,
    pub(crate) onion_next_frames: usize,
    pub(crate) next_layer_number: usize,
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f32,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            layers: vec![Layer::new("Layer 1")],
            current_frame: 0,
            active_layer: 0,
            total_frames: 24,
            fps: 24,
            is_playing: false,
            last_frame_advance_time: None,
            onion_skin_enabled: false,
            onion_prev_frames: 2,
            onion_next_frames: 2,
            next_layer_number: 2,
            scroll_x: 0.0,
            scroll_y: 0.0,
        }
    }
}

impl Timeline {
    /// 現在アクティブなレイヤー・フレームの `Cel` 内の図形スライスを返す
    pub(crate) fn active_cel_shapes(&self) -> &[FillShape] {
        self.layers
            .get(self.active_layer)
            .and_then(|layer| layer.cel_at(self.current_frame))
            .map(|cel| cel.shapes.as_slice())
            .unwrap_or(&[])
    }

    /// 現在アクティブなレイヤー・フレームの `Cel` 内の図形ベクタへの可変参照を返す
    pub(crate) fn active_cel_shapes_mut(&mut self) -> Option<&mut Vec<FillShape>> {
        let frame = self.current_frame;
        self.layers
            .get_mut(self.active_layer)
            .and_then(|layer| layer.cel_at_mut(frame))
            .map(|cel| &mut cel.shapes)
    }

    /// 現在アクティブなレイヤーが編集可能（表示中かつ非ロック）であるかを返す
    pub(crate) fn is_active_layer_editable(&self) -> bool {
        self.layers
            .get(self.active_layer)
            .is_some_and(|l| l.visible && !l.locked)
    }

    /// 再生ヘッド（`current_frame`）が表示領域外に出た場合、横スクロール位置 `scroll_x` を自動追従させる
    pub(crate) fn ensure_current_frame_visible(&mut self, grid_width: f32) {
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

// ============================================================================
// FlashBrushApp と Timeline の同期・操作・下部タイムラインパネルUI
// ============================================================================

impl FlashBrushApp {
    /// キャンバスの作業用 `self.shapes` を、現在アクティブな（レイヤー, フレーム）の `Cel` に書き戻す
    pub(crate) fn sync_shapes_to_timeline(&mut self) {
        let current_shapes = self.shapes.clone();
        if let Some(cel_shapes) = self.timeline.active_cel_shapes_mut() {
            *cel_shapes = current_shapes;
        }
    }

    /// 現在アクティブな（レイヤー, フレーム）の `Cel` から `self.shapes` を読み込む
    pub(crate) fn load_shapes_from_timeline(&mut self) {
        self.shapes = self.timeline.active_cel_shapes().to_vec();
        self.selected_shapes.clear();
        self.unsliced_canvas = None;
        self.selection_modified = false;
        self.select_drag = crate::canvas_tool::SelectDragState::None;
        self.transform_drag = crate::canvas_tool::TransformDragState::None;
        self.transform_pivot = None;
        self.sanitize_editing_group_path();
    }

    /// 指定したレイヤー・フレームへ切り替える（切り替え前に未確定の選択スライス等を確定して保存する）
    pub(crate) fn switch_to_layer_and_frame(&mut self, layer_idx: usize, frame_idx: usize) {
        if self.timeline.layers.is_empty() {
            return;
        }
        let clamped_layer = layer_idx.min(self.timeline.layers.len() - 1);
        let clamped_frame = frame_idx.min(self.timeline.total_frames.saturating_sub(1));
        if clamped_layer == self.timeline.active_layer
            && clamped_frame == self.timeline.current_frame
        {
            return;
        }
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.timeline.active_layer = clamped_layer;
        self.timeline.current_frame = clamped_frame;
        self.load_shapes_from_timeline();
    }

    /// 前後のフレームへコマ送り移動する（`,` / `.` キーやボタン操作）
    pub(crate) fn step_frame(&mut self, delta: isize) {
        self.timeline.is_playing = false;
        self.timeline.last_frame_advance_time = None;
        let total = self.timeline.total_frames.max(1) as isize;
        let next = (self.timeline.current_frame as isize + delta).rem_euclid(total) as usize;
        self.switch_to_layer_and_frame(self.timeline.active_layer, next);
    }

    /// 再生 / 一時停止を切り替える（`Enter` キーまたは Play ボタン）
    pub(crate) fn toggle_playback(&mut self) {
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.timeline.is_playing = !self.timeline.is_playing;
        self.timeline.last_frame_advance_time = None;
    }

    /// 再生中のフレーム自動進行を処理する
    pub(crate) fn update_playback(&mut self, ctx: &egui::Context) {
        if !self.timeline.is_playing {
            self.timeline.last_frame_advance_time = None;
            return;
        }
        let fps = self.timeline.fps.clamp(1, 60);
        let frame_interval = 1.0 / (fps as f64);
        let now = ctx.input(|i| i.time);

        if let Some(last_time) = self.timeline.last_frame_advance_time {
            let elapsed = now - last_time;
            if elapsed >= frame_interval {
                let steps = ((elapsed / frame_interval).floor() as usize).max(1);
                self.timeline.last_frame_advance_time =
                    Some(last_time + (steps as f64) * frame_interval);
                let total = self.timeline.total_frames.max(1);
                let next_frame = (self.timeline.current_frame + steps) % total;
                self.sync_shapes_to_timeline();
                self.timeline.current_frame = next_frame;
                self.load_shapes_from_timeline();
            }
        } else {
            self.timeline.last_frame_advance_time = Some(now);
        }

        ctx.request_repaint_after(Duration::from_secs_f64(frame_interval * 0.5));
    }

    /// キーフレーム挿入（`F6`: `clone_prev = true` で前の絵を複製 / `F7`: `clone_prev = false` で空白セル作成）
    /// - 現在のフレームが保持フレーム（スパンの途中）なら、そのフレーム位置にキーフレームを挿入する。
    /// - 現在のフレームがすでにキーフレーム先頭である場合、`F6`（または絵がある状態での `F7`）は
    ///   1つ次のフレームへ進んでキーフレームを作成するため、連続してポーズを描き進められる。
    pub(crate) fn timeline_insert_keyframe(&mut self, clone_prev: bool) -> bool {
        if !self.timeline.is_active_layer_editable() {
            return false;
        }
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.push_undo();

        let layer_idx = self.timeline.active_layer;
        let curr_f = self.timeline.current_frame;
        let is_exact_kf = self.timeline.layers[layer_idx]
            .keyframes
            .iter()
            .any(|kf| kf.start_frame == curr_f);
        let curr_non_empty = !self.shapes.is_empty();

        let target_frame = if is_exact_kf && (clone_prev || curr_non_empty) {
            curr_f + 1
        } else {
            curr_f
        };

        if target_frame >= self.timeline.total_frames {
            self.timeline.total_frames = target_frame + 1;
        }

        self.timeline.layers[layer_idx].insert_keyframe(target_frame, clone_prev);
        self.timeline.current_frame = target_frame;
        self.load_shapes_from_timeline();
        true
    }

    /// 現在フレームのキーフレームを削除（フレーム0の場合は空白化）する（`Shift+F6`）
    pub(crate) fn timeline_delete_keyframe(&mut self) -> bool {
        if !self.timeline.is_active_layer_editable() {
            return false;
        }
        self.clear_selection();
        self.sync_shapes_to_timeline();
        let layer_idx = self.timeline.active_layer;
        let curr_f = self.timeline.current_frame;
        let can_delete = self.timeline.layers[layer_idx]
            .keyframe_index_at(curr_f)
            .is_some_and(|idx| idx > 0 || !self.timeline.layers[layer_idx].keyframes[0].cel.is_empty());
        if !can_delete {
            return false;
        }
        self.push_undo();
        self.timeline.layers[layer_idx].delete_keyframe_at(curr_f);
        self.load_shapes_from_timeline();
        true
    }

    /// 現在フレームのスパンを1フレーム延長する（`F5`）
    pub(crate) fn timeline_extend_frame(&mut self) -> bool {
        if !self.timeline.is_active_layer_editable() {
            return false;
        }
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.push_undo();
        let layer_idx = self.timeline.active_layer;
        let curr_f = self.timeline.current_frame;
        self.timeline.layers[layer_idx].insert_frame_at(curr_f);
        self.timeline.total_frames += 1;
        self.load_shapes_from_timeline();
        true
    }

    /// 現在フレームのスパンを1フレーム短縮する（`Shift+F5`）
    pub(crate) fn timeline_remove_frame(&mut self) -> bool {
        if !self.timeline.is_active_layer_editable() || self.timeline.total_frames <= 1 {
            return false;
        }
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.push_undo();
        let layer_idx = self.timeline.active_layer;
        let curr_f = self.timeline.current_frame;
        self.timeline.layers[layer_idx].remove_frame_at(curr_f);
        let max_kf_start = self
            .timeline
            .layers
            .iter()
            .filter_map(|l| l.keyframes.last().map(|kf| kf.start_frame))
            .max()
            .unwrap_or(0);
        if self.timeline.total_frames > max_kf_start + 1 {
            self.timeline.total_frames -= 1;
        }
        if self.timeline.current_frame >= self.timeline.total_frames {
            self.timeline.current_frame = self.timeline.total_frames - 1;
        }
        self.load_shapes_from_timeline();
        true
    }

    /// 新しいレイヤーを現在のアクティブレイヤーの1つ上（前面）に追加する
    pub(crate) fn timeline_add_layer(&mut self) {
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.push_undo();
        let name = format!("Layer {}", self.timeline.next_layer_number);
        self.timeline.next_layer_number += 1;
        let insert_idx = (self.timeline.active_layer + 1).min(self.timeline.layers.len());
        self.timeline.layers.insert(insert_idx, Layer::new(name));
        self.timeline.active_layer = insert_idx;
        self.load_shapes_from_timeline();
    }

    /// 現在アクティブなレイヤーを削除する（最低1レイヤーは残す）
    pub(crate) fn timeline_delete_active_layer(&mut self) -> bool {
        if self.timeline.layers.len() <= 1 {
            return false;
        }
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.push_undo();
        self.timeline.layers.remove(self.timeline.active_layer);
        if self.timeline.active_layer >= self.timeline.layers.len() {
            self.timeline.active_layer = self.timeline.layers.len() - 1;
        }
        self.load_shapes_from_timeline();
        true
    }

    /// 現在アクティブなレイヤーを1つ上（前面: `up = true`）または1つ下（背面: `up = false`）へ移動する
    pub(crate) fn timeline_move_active_layer(&mut self, up: bool) -> bool {
        let len = self.timeline.layers.len();
        let curr = self.timeline.active_layer;
        let target = if up {
            if curr + 1 >= len {
                return false;
            }
            curr + 1
        } else {
            if curr == 0 {
                return false;
            }
            curr - 1
        };
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.push_undo();
        self.timeline.layers.swap(curr, target);
        self.timeline.active_layer = target;
        true
    }

    /// 画面下部の Adobe Animate 風タイムラインパネルを描画・操作する
    pub(crate) fn draw_timeline_panel(&mut self, ctx: &egui::Context) {
        // 常に最新の self.shapes をアクティブセルへ反映してからタイムラインを描画
        self.sync_shapes_to_timeline();

        egui::TopBottomPanel::bottom("timeline_panel")
            .resizable(true)
            .default_height(186.0)
            .height_range(124.0..=360.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(34, 37, 44))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(20, 22, 28))),
            )
            .show(ctx, |ui| {
                ui.visuals_mut().override_text_color = Some(Color32::from_rgb(225, 230, 238));

                // 1. 上部タイムライン操作バー
                egui::Frame::none()
                    .fill(Color32::from_rgb(42, 46, 54))
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;

                            if ui
                                .button("⏮")
                                .on_hover_text("最初のフレームへ (Home)")
                                .clicked()
                            {
                                self.timeline.is_playing = false;
                                self.switch_to_layer_and_frame(self.timeline.active_layer, 0);
                            }
                            if ui
                                .button("◀")
                                .on_hover_text("前のフレームへ (, / ←)")
                                .clicked()
                            {
                                self.step_frame(-1);
                            }

                            let play_label = if self.timeline.is_playing {
                                "⏸ Stop (Enter)"
                            } else {
                                "▶ Play (Enter)"
                            };
                            if ui
                                .selectable_label(self.timeline.is_playing, play_label)
                                .on_hover_text("再生 / 一時停止 (Enter)")
                                .clicked()
                            {
                                self.toggle_playback();
                            }

                            if ui
                                .button("▶")
                                .on_hover_text("次のフレームへ (. / →)")
                                .clicked()
                            {
                                self.step_frame(1);
                            }
                            if ui
                                .button("⏭")
                                .on_hover_text("最後のフレームへ (End)")
                                .clicked()
                            {
                                self.timeline.is_playing = false;
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
                                self.timeline.total_frames = (total_f as usize).max(1);
                                if self.timeline.current_frame >= self.timeline.total_frames {
                                    self.switch_to_layer_and_frame(
                                        self.timeline.active_layer,
                                        self.timeline.total_frames - 1,
                                    );
                                }
                            }

                            ui.label("FPS:");
                            ui.add(
                                egui::DragValue::new(&mut self.timeline.fps)
                                    .range(1..=60)
                                    .speed(0.25),
                            );

                            ui.separator();

                            let editable = self.timeline.is_active_layer_editable();
                            if ui
                                .add_enabled(editable, egui::Button::new("● キーフレーム (F6)"))
                                .on_hover_text(
                                    "現在の絵を複製して新しいキーフレームを挿入 (F6)",
                                )
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
                                .on_hover_text("現在のキーフレームの保持尺を1コマ延長 (F5)")
                                .clicked()
                            {
                                self.timeline_extend_frame();
                            }
                            if ui
                                .add_enabled(
                                    editable && self.timeline.total_frames > 1,
                                    egui::Button::new("－ 短縮"),
                                )
                                .on_hover_text("現在のキーフレームの保持尺を1コマ短縮 (Shift+F5)")
                                .clicked()
                            {
                                self.timeline_remove_frame();
                            }
                            if ui
                                .add_enabled(editable, egui::Button::new("✕ KF削除"))
                                .on_hover_text("現在のキーフレームを削除 / クリア (Shift+F6)")
                                .clicked()
                            {
                                self.timeline_delete_keyframe();
                            }

                            ui.separator();

                            if ui
                                .selectable_label(
                                    self.timeline.onion_skin_enabled,
                                    "🧅 Onion (O)",
                                )
                                .on_hover_text("オニオンスキン（前後のフレームの透かし表示）切替 (O)")
                                .clicked()
                            {
                                self.timeline.onion_skin_enabled =
                                    !self.timeline.onion_skin_enabled;
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

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let len = self.timeline.layers.len();
                                    let act = self.timeline.active_layer;
                                    if ui
                                        .add_enabled(act > 0, egui::Button::new("▼"))
                                        .on_hover_text("レイヤーを1つ背面へ移動")
                                        .clicked()
                                    {
                                        self.timeline_move_active_layer(false);
                                    }
                                    if ui
                                        .add_enabled(act + 1 < len, egui::Button::new("▲"))
                                        .on_hover_text("レイヤーを1つ前面へ移動")
                                        .clicked()
                                    {
                                        self.timeline_move_active_layer(true);
                                    }
                                    if ui
                                        .add_enabled(len > 1, egui::Button::new("🗑 レイヤー削除"))
                                        .on_hover_text("現在のレイヤーを削除")
                                        .clicked()
                                    {
                                        self.timeline_delete_active_layer();
                                    }
                                    if ui
                                        .button("＋ レイヤー追加")
                                        .on_hover_text("新しいレイヤーを前面に追加")
                                        .clicked()
                                    {
                                        self.timeline_add_layer();
                                    }
                                },
                            );
                        });
                    });

                // 2. 下部：左側レイヤーリスト ＋ 右側フレームルーラー＆キーフレームグリッド
                let avail = ui.available_size();
                let (panel_rect, response) =
                    ui.allocate_exact_size(avail, Sense::click_and_drag());
                let painter = ui.painter_at(panel_rect);

                let grid_left = panel_rect.left() + LAYER_COL_W;
                let grid_top = panel_rect.top() + RULER_H;
                let grid_w = (panel_rect.right() - grid_left).max(1.0);
                let grid_h = (panel_rect.bottom() - grid_top).max(1.0);

                // 再生中やキー操作でフレームが移動した際、再生ヘッドが画面内に収まるようスクロール追従
                self.timeline.ensure_current_frame_visible(grid_w);

                // マウスホイールによるタイムラインの横・縦スクロール
                if response.hovered() {
                    let scroll_delta = ui.input(|i| i.smooth_scroll_delta);
                    if scroll_delta.x.abs() > 0.1 || scroll_delta.y.abs() > 0.1 {
                        let pointer_in_layers = response
                            .hover_pos()
                            .is_some_and(|p| p.x < grid_left && p.y > grid_top);
                        if pointer_in_layers {
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

                let max_frames_shown = (self.timeline.total_frames + 12).max(36);
                let max_scroll_x =
                    ((max_frames_shown as f32) * CELL_W - grid_w).max(0.0);
                let max_scroll_y =
                    ((self.timeline.layers.len() as f32) * ROW_H - grid_h).max(0.0);
                self.timeline.scroll_x = self.timeline.scroll_x.clamp(0.0, max_scroll_x);
                self.timeline.scroll_y = self.timeline.scroll_y.clamp(0.0, max_scroll_y);

                // --- マウスクリック＆ドラッグの当たり判定（レイヤー切替・👁表示・🔒ロック・フレームスクラブ） ---
                let just_clicked =
                    response.hovered() && ui.input(|i| i.pointer.primary_pressed());
                let is_dragging = response.dragged() && ui.input(|i| i.pointer.primary_down());

                if let Some(pos) = response.interact_pointer_pos() {
                    if pos.x >= grid_left && (just_clicked || is_dragging) {
                        // 右側（ルーラーまたはフレームグリッド）のクリック・ドラッグ -> スクラブ移動
                        let col =
                            (((pos.x - grid_left + self.timeline.scroll_x) / CELL_W).floor()
                                as isize)
                                .max(0) as usize;
                        let target_frame = col.min(self.timeline.total_frames.saturating_sub(1));
                        let target_layer = if pos.y >= grid_top {
                            let row = ((pos.y - grid_top + self.timeline.scroll_y) / ROW_H).floor()
                                as usize;
                            let num_layers = self.timeline.layers.len();
                            if row < num_layers {
                                num_layers - 1 - row
                            } else {
                                self.timeline.active_layer
                            }
                        } else {
                            self.timeline.active_layer
                        };
                        self.timeline.is_playing = false;
                        self.switch_to_layer_and_frame(target_layer, target_frame);
                    } else if pos.x < grid_left && pos.y >= grid_top && just_clicked {
                        // 左側レイヤー一覧列のクリック -> 👁 / 🔒 トグルまたはアクティブレイヤー選択
                        let row =
                            ((pos.y - grid_top + self.timeline.scroll_y) / ROW_H).floor() as usize;
                        let num_layers = self.timeline.layers.len();
                        if row < num_layers {
                            let layer_idx = num_layers - 1 - row;
                            let rel_x = pos.x - panel_rect.left();
                            if (6.0..30.0).contains(&rel_x) {
                                self.clear_selection();
                                self.timeline.layers[layer_idx].visible =
                                    !self.timeline.layers[layer_idx].visible;
                            } else if (30.0..54.0).contains(&rel_x) {
                                self.clear_selection();
                                self.timeline.layers[layer_idx].locked =
                                    !self.timeline.layers[layer_idx].locked;
                            } else {
                                self.switch_to_layer_and_frame(
                                    layer_idx,
                                    self.timeline.current_frame,
                                );
                            }
                        }
                    }
                }

                // --- (A) 右側フレームグリッドの描画 ---
                let grid_rect = Rect::from_min_max(
                    Pos2::new(grid_left, grid_top),
                    panel_rect.right_bottom(),
                );
                let grid_painter = painter.with_clip_rect(grid_rect);
                grid_painter.rect_filled(grid_rect, 0.0, Color32::from_rgb(30, 33, 39));

                let first_vis_frame = (self.timeline.scroll_x / CELL_W).floor() as usize;
                let last_vis_frame = (((self.timeline.scroll_x + grid_w) / CELL_W).ceil() as usize)
                    .min(max_frames_shown);

                let num_layers = self.timeline.layers.len();
                for row in 0..num_layers {
                    let layer_idx = num_layers - 1 - row;
                    let y0 = grid_top + (row as f32) * ROW_H - self.timeline.scroll_y;
                    let y1 = y0 + ROW_H;
                    if y1 < grid_top || y0 > panel_rect.bottom() {
                        continue;
                    }

                    let layer = &self.timeline.layers[layer_idx];
                    let is_active_layer = layer_idx == self.timeline.active_layer;

                    // 総フレーム範囲外の背景（薄暗いグレー）と各フレームの縦グリッド線
                    for f in first_vis_frame..=last_vis_frame {
                        let x0 = grid_left + (f as f32) * CELL_W - self.timeline.scroll_x;
                        let cell_rect =
                            Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + CELL_W, y1));
                        let bg = if f < self.timeline.total_frames {
                            if is_active_layer {
                                Color32::from_rgb(44, 48, 58)
                            } else {
                                Color32::from_rgb(38, 41, 49)
                            }
                        } else {
                            Color32::from_rgb(27, 29, 35)
                        };
                        grid_painter.rect_filled(cell_rect, 0.0, bg);
                    }

                    // キーフレーム区間（スパン）ごとの帯描画・黒丸 `●` / 白丸 `○`・終端マーク `▯`
                    for (kf_idx, kf) in layer.keyframes.iter().enumerate() {
                        let span_start = kf.start_frame;
                        if span_start >= self.timeline.total_frames {
                            continue;
                        }
                        let span_end = layer
                            .keyframes
                            .get(kf_idx + 1)
                            .map(|next_kf| {
                                next_kf
                                    .start_frame
                                    .saturating_sub(1)
                                    .min(self.timeline.total_frames - 1)
                            })
                            .unwrap_or(self.timeline.total_frames - 1);
                        if span_end < first_vis_frame || span_start > last_vis_frame {
                            continue;
                        }

                        let sx0 =
                            grid_left + (span_start as f32) * CELL_W - self.timeline.scroll_x;
                        let sx1 = grid_left + ((span_end + 1) as f32) * CELL_W
                            - self.timeline.scroll_x;
                        let span_rect = Rect::from_min_max(
                            Pos2::new(sx0 + 0.5, y0 + 1.0),
                            Pos2::new(sx1 - 0.5, y1 - 1.0),
                        );

                        let has_art = !kf.cel.is_empty();
                        let span_fill = if has_art {
                            if is_active_layer {
                                Color32::from_rgb(222, 228, 238)
                            } else {
                                Color32::from_rgb(192, 198, 210)
                            }
                        } else if is_active_layer {
                            Color32::from_rgb(76, 82, 96)
                        } else {
                            Color32::from_rgb(62, 67, 79)
                        };
                        grid_painter.rect_filled(span_rect, 2.0, span_fill);

                        // スパン内部の1コマ区切り線（薄い縦線）
                        let draw_f_start = span_start.max(first_vis_frame);
                        let draw_f_end = span_end.min(last_vis_frame);
                        for f in (draw_f_start + 1)..=draw_f_end {
                            let fx = grid_left + (f as f32) * CELL_W - self.timeline.scroll_x;
                            grid_painter.line_segment(
                                [Pos2::new(fx, y0 + 2.0), Pos2::new(fx, y1 - 2.0)],
                                Stroke::new(
                                    1.0,
                                    if has_art {
                                        Color32::from_rgba_unmultiplied(120, 130, 145, 90)
                                    } else {
                                        Color32::from_rgba_unmultiplied(40, 44, 52, 110)
                                    },
                                ),
                            );
                        }

                        grid_painter.rect_stroke(
                            span_rect,
                            2.0,
                            Stroke::new(1.0, Color32::from_rgb(35, 38, 46)),
                        );

                        // キーフレーム先頭記号：絵があれば黒丸 `●`、空なら白抜き丸 `○`
                        let kf_center = Pos2::new(sx0 + CELL_W * 0.5, (y0 + y1) * 0.5 + 1.0);
                        if has_art {
                            grid_painter.circle_filled(
                                kf_center,
                                3.8,
                                Color32::from_rgb(22, 25, 32),
                            );
                        } else {
                            grid_painter.circle_stroke(
                                kf_center,
                                3.6,
                                Stroke::new(1.5, Color32::from_rgb(215, 222, 235)),
                            );
                        }

                        // 延長フレーム（Hold Frame）の水平ラインと終端の縦長四角マーク `▯`
                        if span_end > span_start {
                            let line_col = if has_art {
                                Color32::from_rgba_unmultiplied(50, 58, 72, 140)
                            } else {
                                Color32::from_rgba_unmultiplied(170, 178, 195, 110)
                            };
                            let end_mark_x = sx1 - CELL_W * 0.45;
                            grid_painter.line_segment(
                                [
                                    Pos2::new(kf_center.x + 5.5, kf_center.y),
                                    Pos2::new(end_mark_x - 3.0, kf_center.y),
                                ],
                                Stroke::new(1.2, line_col),
                            );
                            let end_box = Rect::from_center_size(
                                Pos2::new(end_mark_x, kf_center.y),
                                Vec2::new(4.2, 8.5),
                            );
                            grid_painter.rect_filled(end_box, 1.0, Color32::WHITE);
                            grid_painter.rect_stroke(
                                end_box,
                                1.0,
                                Stroke::new(1.2, line_col),
                            );
                        }
                    }

                    // 現在選択されている（アクティブレイヤー × 現在フレーム）のセルを水色ハイライト
                    if is_active_layer {
                        let cx0 = grid_left + (self.timeline.current_frame as f32) * CELL_W
                            - self.timeline.scroll_x;
                        let active_cell = Rect::from_min_max(
                            Pos2::new(cx0 + 0.5, y0 + 0.5),
                            Pos2::new(cx0 + CELL_W - 0.5, y1 - 0.5),
                        );
                        grid_painter.rect_filled(
                            active_cell,
                            1.5,
                            Color32::from_rgba_unmultiplied(0, 155, 255, 85),
                        );
                        grid_painter.rect_stroke(
                            active_cell,
                            1.5,
                            Stroke::new(1.6, Color32::from_rgb(0, 175, 255)),
                        );
                    }

                    // 行の下境界線
                    grid_painter.line_segment(
                        [
                            Pos2::new(grid_left, y1),
                            Pos2::new(panel_rect.right(), y1),
                        ],
                        Stroke::new(1.0, Color32::from_rgb(24, 26, 32)),
                    );
                }

                // 5フレームごとの目盛り縦線
                for f in first_vis_frame..=last_vis_frame {
                    let fx = grid_left + (f as f32) * CELL_W - self.timeline.scroll_x;
                    if (f + 1) % 5 == 0 {
                        grid_painter.line_segment(
                            [Pos2::new(fx + CELL_W, grid_top), Pos2::new(fx + CELL_W, panel_rect.bottom())],
                            Stroke::new(1.0, Color32::from_rgba_unmultiplied(15, 18, 24, 100)),
                        );
                    }
                }

                // --- (B) 上部フレーム番号ルーラーの描画 ---
                let ruler_rect = Rect::from_min_max(
                    Pos2::new(grid_left, panel_rect.top()),
                    Pos2::new(panel_rect.right(), grid_top),
                );
                let ruler_painter = painter.with_clip_rect(ruler_rect);
                ruler_painter.rect_filled(ruler_rect, 0.0, Color32::from_rgb(45, 49, 58));

                // オニオンスキン有効時はルーラー上に前（青）・後（緑）の範囲帯を表示
                if self.timeline.onion_skin_enabled {
                    let cf = self.timeline.current_frame;
                    let prev_start = cf.saturating_sub(self.timeline.onion_prev_frames);
                    if prev_start < cf {
                        let ox0 =
                            grid_left + (prev_start as f32) * CELL_W - self.timeline.scroll_x;
                        let ox1 = grid_left + (cf as f32) * CELL_W - self.timeline.scroll_x;
                        ruler_painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(ox0, ruler_rect.top() + 2.0),
                                Pos2::new(ox1, ruler_rect.bottom() - 2.0),
                            ),
                            2.0,
                            Color32::from_rgba_unmultiplied(55, 130, 245, 75),
                        );
                    }
                    let next_end = (cf + self.timeline.onion_next_frames)
                        .min(self.timeline.total_frames.saturating_sub(1));
                    if next_end > cf {
                        let ox0 =
                            grid_left + ((cf + 1) as f32) * CELL_W - self.timeline.scroll_x;
                        let ox1 = grid_left + ((next_end + 1) as f32) * CELL_W
                            - self.timeline.scroll_x;
                        ruler_painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(ox0, ruler_rect.top() + 2.0),
                                Pos2::new(ox1, ruler_rect.bottom() - 2.0),
                            ),
                            2.0,
                            Color32::from_rgba_unmultiplied(45, 195, 110, 75),
                        );
                    }
                }

                for f in first_vis_frame..=last_vis_frame {
                    let fx = grid_left + (f as f32) * CELL_W - self.timeline.scroll_x;
                    let frame_num = f + 1;
                    let is_major = frame_num == 1 || frame_num % 5 == 0;
                    let tick_h = if is_major { 8.0 } else { 4.0 };
                    ruler_painter.line_segment(
                        [
                            Pos2::new(fx, grid_top - tick_h),
                            Pos2::new(fx, grid_top),
                        ],
                        Stroke::new(1.0, Color32::from_rgb(115, 122, 136)),
                    );
                    if is_major {
                        ruler_painter.text(
                            Pos2::new(fx + 3.0, panel_rect.top() + 3.0),
                            Align2::LEFT_TOP,
                            format!("{frame_num}"),
                            FontId::proportional(11.0),
                            if f < self.timeline.total_frames {
                                Color32::from_rgb(210, 216, 226)
                            } else {
                                Color32::from_rgb(115, 120, 130)
                            },
                        );
                    }
                }

                // 再生ヘッド（赤いマーカー矩形 ＋ 全レイヤーを貫く赤い縦線）
                let head_x = grid_left
                    + (self.timeline.current_frame as f32 + 0.5) * CELL_W
                    - self.timeline.scroll_x;
                if head_x >= grid_left - CELL_W && head_x <= panel_rect.right() + CELL_W {
                    let head_box = Rect::from_min_max(
                        Pos2::new(head_x - CELL_W * 0.5, panel_rect.top() + 1.0),
                        Pos2::new(head_x + CELL_W * 0.5, grid_top - 1.0),
                    );
                    ruler_painter.rect_filled(head_box, 2.5, Color32::from_rgb(230, 55, 55));
                    ruler_painter.text(
                        head_box.center(),
                        Align2::CENTER_CENTER,
                        format!("{}", self.timeline.current_frame + 1),
                        FontId::proportional(11.0),
                        Color32::WHITE,
                    );
                    grid_painter.line_segment(
                        [Pos2::new(head_x, grid_top), Pos2::new(head_x, panel_rect.bottom())],
                        Stroke::new(1.8, Color32::from_rgb(235, 60, 60)),
                    );
                }

                ruler_painter.line_segment(
                    [
                        Pos2::new(grid_left, grid_top),
                        Pos2::new(panel_rect.right(), grid_top),
                    ],
                    Stroke::new(1.0, Color32::from_rgb(22, 24, 30)),
                );

                // --- (C) 左側レイヤー一覧列の描画 ---
                let layer_list_rect = Rect::from_min_max(
                    Pos2::new(panel_rect.left(), grid_top),
                    Pos2::new(grid_left, panel_rect.bottom()),
                );
                let layer_painter = painter.with_clip_rect(layer_list_rect);
                layer_painter.rect_filled(layer_list_rect, 0.0, Color32::from_rgb(38, 42, 50));

                for row in 0..num_layers {
                    let layer_idx = num_layers - 1 - row;
                    let y0 = grid_top + (row as f32) * ROW_H - self.timeline.scroll_y;
                    let y1 = y0 + ROW_H;
                    if y1 < grid_top || y0 > panel_rect.bottom() {
                        continue;
                    }
                    let row_rect = Rect::from_min_max(
                        Pos2::new(panel_rect.left(), y0),
                        Pos2::new(grid_left, y1),
                    );
                    let layer = &self.timeline.layers[layer_idx];
                    let is_active = layer_idx == self.timeline.active_layer;

                    layer_painter.rect_filled(
                        row_rect,
                        0.0,
                        if is_active {
                            Color32::from_rgb(52, 82, 126)
                        } else {
                            Color32::from_rgb(40, 44, 52)
                        },
                    );

                    // 👁 表示/非表示アイコン
                    layer_painter.text(
                        Pos2::new(panel_rect.left() + 18.0, (y0 + y1) * 0.5),
                        Align2::CENTER_CENTER,
                        if layer.visible { "👁" } else { "・" },
                        FontId::proportional(13.0),
                        if layer.visible {
                            Color32::from_rgb(230, 235, 245)
                        } else {
                            Color32::from_rgb(115, 120, 132)
                        },
                    );

                    // 🔒 ロックアイコン
                    layer_painter.text(
                        Pos2::new(panel_rect.left() + 42.0, (y0 + y1) * 0.5),
                        Align2::CENTER_CENTER,
                        if layer.locked { "🔒" } else { "·" },
                        FontId::proportional(12.5),
                        if layer.locked {
                            Color32::from_rgb(245, 190, 65)
                        } else {
                            Color32::from_rgb(115, 120, 132)
                        },
                    );

                    // レイヤー名
                    let name_color = if !layer.visible {
                        Color32::from_rgb(130, 136, 148)
                    } else if is_active {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(215, 220, 230)
                    };
                    layer_painter.text(
                        Pos2::new(panel_rect.left() + 60.0, (y0 + y1) * 0.5),
                        Align2::LEFT_CENTER,
                        &layer.name,
                        FontId::proportional(12.5),
                        name_color,
                    );

                    layer_painter.line_segment(
                        [Pos2::new(panel_rect.left(), y1), Pos2::new(grid_left, y1)],
                        Stroke::new(1.0, Color32::from_rgb(26, 29, 36)),
                    );
                }

                // 左上ヘッダーセル ("👁 / 🔒 / Layers")
                let corner_rect = Rect::from_min_max(
                    panel_rect.left_top(),
                    Pos2::new(grid_left, grid_top),
                );
                painter.rect_filled(corner_rect, 0.0, Color32::from_rgb(45, 49, 58));
                painter.text(
                    Pos2::new(panel_rect.left() + 18.0, panel_rect.top() + RULER_H * 0.5),
                    Align2::CENTER_CENTER,
                    "👁",
                    FontId::proportional(11.5),
                    Color32::from_rgb(175, 182, 195),
                );
                painter.text(
                    Pos2::new(panel_rect.left() + 42.0, panel_rect.top() + RULER_H * 0.5),
                    Align2::CENTER_CENTER,
                    "🔒",
                    FontId::proportional(11.5),
                    Color32::from_rgb(175, 182, 195),
                );
                painter.text(
                    Pos2::new(panel_rect.left() + 60.0, panel_rect.top() + RULER_H * 0.5),
                    Align2::LEFT_CENTER,
                    "Layers",
                    FontId::proportional(11.5),
                    Color32::from_rgb(190, 196, 208),
                );
                painter.line_segment(
                    [
                        Pos2::new(panel_rect.left(), grid_top),
                        Pos2::new(grid_left, grid_top),
                    ],
                    Stroke::new(1.0, Color32::from_rgb(22, 24, 30)),
                );
                painter.line_segment(
                    [
                        Pos2::new(grid_left, panel_rect.top()),
                        Pos2::new(grid_left, panel_rect.bottom()),
                    ],
                    Stroke::new(1.0, Color32::from_rgb(22, 24, 30)),
                );
            });
    }
}
