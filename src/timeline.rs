use crate::{vector_engine::FillShape, FlashBrushApp};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::time::Duration;

const LAYER_COL_W: f32 = 190.0;
const RULER_H: f32 = 24.0;
const ROW_H: f32 = 26.0;
const CELL_W: f32 = 16.0;

// ============================================================================
// 1. タイムラインのデータモデル（Cel / Keyframe / Selection / Layer / Timeline）
// ============================================================================

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

/// タイムライン上で選択されているセル矩形範囲（複数レイヤー × 複数フレーム対応）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineCellSelection {
    pub(crate) min_layer: usize,
    pub(crate) max_layer: usize,
    pub(crate) min_frame: usize,
    pub(crate) max_frame: usize,
}

impl TimelineCellSelection {
    pub(crate) fn single(layer: usize, frame: usize) -> Self {
        Self {
            min_layer: layer,
            max_layer: layer,
            min_frame: frame,
            max_frame: frame,
        }
    }

    pub(crate) fn from_corners(l0: usize, f0: usize, l1: usize, f1: usize) -> Self {
        Self {
            min_layer: l0.min(l1),
            max_layer: l0.max(l1),
            min_frame: f0.min(f1),
            max_frame: f0.max(f1),
        }
    }

    pub(crate) fn contains(&self, layer: usize, frame: usize) -> bool {
        layer >= self.min_layer
            && layer <= self.max_layer
            && frame >= self.min_frame
            && frame <= self.max_frame
    }

    pub(crate) fn contains_layer(&self, layer: usize) -> bool {
        layer >= self.min_layer && layer <= self.max_layer
    }
}

/// タイムライン右側領域（ルーラー・フレームグリッド）でのマウスドラッグ状態
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimelineGridDrag {
    None,
    /// 上部フレーム番号ルーラーのドラッグによる再生ヘッドスクラブ（レイヤー個別選択を解除）
    ScrubRuler,
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

/// タイムラインの1レイヤー（固有ID、名前、表示/非表示、ロック状態、レイヤー固有フレーム長、開始フレーム順にソートされたキーフレーム配列）
#[derive(Clone, Debug)]
pub(crate) struct Layer {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
    pub(crate) frame_count: usize,
    pub(crate) keyframes: Vec<Keyframe>,
}

impl Layer {
    pub(crate) fn new_with_id_and_frames(
        id: u64,
        name: impl Into<String>,
        frame_count: usize,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            locked: false,
            frame_count: frame_count.max(1),
            keyframes: vec![Keyframe::new_blank(0)],
        }
    }

    pub(crate) fn new_with_id(id: u64, name: impl Into<String>) -> Self {
        Self::new_with_id_and_frames(id, name, 1)
    }

    /// 指定フレーム番号 `frame` において有効な（保持されている）キーフレームのインデックスを返す
    /// （そのレイヤーの `frame_count` を超えるフレームでは `None` を返す）
    pub(crate) fn keyframe_index_at(&self, frame: usize) -> Option<usize> {
        if frame >= self.frame_count {
            return None;
        }
        let idx = self.keyframes.partition_point(|kf| kf.start_frame <= frame);
        idx.checked_sub(1)
    }

    /// `kf_idx` 番目のキーフレームが保持される終端フレーム番号（inclusive）を返す
    pub(crate) fn span_end_for_keyframe(&self, kf_idx: usize) -> usize {
        let max_f = self.frame_count.saturating_sub(1);
        self.keyframes
            .get(kf_idx + 1)
            .map(|next| next.start_frame.saturating_sub(1).min(max_f))
            .unwrap_or(max_f)
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

    /// 指定フレーム `frame` にキーフレームを挿入する（`clone_prev == true`: `F6` 複製 / `false`: `F7` 空白）
    pub(crate) fn insert_keyframe(&mut self, frame: usize, clone_prev: bool) -> usize {
        if frame >= self.frame_count {
            self.frame_count = frame + 1;
        }
        if let Some(existing_idx) = self.keyframes.iter().position(|kf| kf.start_frame == frame) {
            if !clone_prev {
                self.keyframes[existing_idx].cel.shapes.clear();
            }
            return existing_idx;
        }

        let lid = self.id;
        let cloned_shapes = if clone_prev {
            self.cel_at(frame)
                .map(|c| c.shapes.iter().cloned().map(|s| s.with_layer_id(lid)).collect())
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
        if frame >= self.frame_count {
            self.frame_count = frame + 1;
        } else {
            for kf in &mut self.keyframes {
                if kf.start_frame > frame {
                    kf.start_frame += 1;
                }
            }
            self.frame_count += 1;
        }
    }

    /// Adobe Animate 仕様のフレーム削除（`Shift+F5`）：
    /// - 対象キーフレーム区間に保持フレーム（2コマ以上の長さ）がある場合は、キーフレームを残したまま保持尺を1フレーム縮め、後続キーフレームを左へ1フレーム詰める。
    /// - 対象キーフレーム区間が1フレーム幅（保持フレームなし）の場合は、そのキーフレーム自体を削除し、後続キーフレームを左へ1フレーム詰めて繰り上げる。
    pub(crate) fn remove_frame_at(&mut self, frame: usize) -> bool {
        if self.frame_count == 0 {
            return false;
        }
        let target_f = frame.min(self.frame_count - 1);
        let Some(kf_idx) = self.keyframe_index_at(target_f) else {
            return false;
        };
        let span_start = self.keyframes[kf_idx].start_frame;
        let span_end = self.span_end_for_keyframe(kf_idx);
        let span_len = span_end.saturating_sub(span_start) + 1;

        if span_len >= 2 {
            for kf in &mut self.keyframes[(kf_idx + 1)..] {
                kf.start_frame = kf.start_frame.saturating_sub(1);
            }
            self.frame_count = (self.frame_count - 1).max(1);
            true
        } else if self.frame_count <= 1 {
            if self.keyframes[0].cel.is_empty() {
                false
            } else {
                self.keyframes[0].cel.shapes.clear();
                true
            }
        } else {
            self.keyframes.remove(kf_idx);
            for kf in &mut self.keyframes[kf_idx..] {
                kf.start_frame = kf.start_frame.saturating_sub(1);
            }
            self.frame_count = (self.frame_count - 1).max(1);
            if self.keyframes.is_empty() || self.keyframes[0].start_frame > 0 {
                self.keyframes.insert(0, Keyframe::new_blank(0));
            }
            true
        }
    }
}

/// レイヤー配列・現在のフレーム・FPS・再生制御・オニオンスキン設定・セル範囲選択を管理するタイムライン構造体
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
    pub(crate) next_layer_id: u64,
    pub(crate) scroll_x: f32,
    pub(crate) scroll_y: f32,
    pub(crate) last_ensured_frame: usize,
    pub(crate) dragging_layer: Option<usize>,
    pub(crate) cell_selection: Option<TimelineCellSelection>,
    pub(crate) grid_drag: TimelineGridDrag,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            layers: vec![Layer::new_with_id(1, "Layer 1")],
            current_frame: 0,
            active_layer: 0,
            total_frames: 1,
            fps: 24,
            is_playing: false,
            last_frame_advance_time: None,
            onion_skin_enabled: false,
            onion_prev_frames: 2,
            onion_next_frames: 2,
            next_layer_number: 2,
            next_layer_id: 2,
            scroll_x: 0.0,
            scroll_y: 0.0,
            last_ensured_frame: 0,
            dragging_layer: None,
            cell_selection: Some(TimelineCellSelection::single(0, 0)),
            grid_drag: TimelineGridDrag::None,
        }
    }
}

impl Timeline {
    /// 全レイヤーの `frame_count` の最大値からタイムライン全体の `total_frames` を再計算する
    pub(crate) fn sync_total_frames(&mut self) {
        self.total_frames = self
            .layers
            .iter()
            .map(|l| l.frame_count)
            .max()
            .unwrap_or(1)
            .max(1);
    }

    /// 現在アクティブなレイヤーの固有 `layer_id` を返す
    pub(crate) fn active_layer_id(&self) -> u64 {
        self.layers.get(self.active_layer).map(|l| l.id).unwrap_or(1)
    }

    /// 指定した `layer_id` を持つレイヤーのインデックス（`0..layers.len()`）を返す
    pub(crate) fn layer_index_by_id(&self, layer_id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == layer_id)
    }

    /// 指定した `layer_id` のレイヤーが表示中かつ非ロック（編集可能）であるかを返す
    pub(crate) fn is_layer_editable_by_id(&self, layer_id: u64) -> bool {
        self.layers
            .iter()
            .any(|l| l.id == layer_id && l.visible && !l.locked)
    }

    /// 表示中かつ非ロック（編集可能）なレイヤーが1つ以上存在するかを返す
    pub(crate) fn has_any_editable_layer(&self) -> bool {
        self.layers.iter().any(|l| l.visible && !l.locked)
    }

    /// `F5` / `Shift+F5` 等のフレーム操作対象となるレイヤーインデックス一覧を返す：
    /// - 特定のレイヤー（または複数レイヤー範囲）が選択されていれば、その選択レイヤー群のみ。
    /// - どのレイヤーも選択されていなければ（`cell_selection == None`）、全非ロックレイヤー。
    pub(crate) fn target_layers_for_frame_op(&self) -> Vec<usize> {
        if let Some(sel) = self.cell_selection {
            (sel.min_layer..=sel.max_layer)
                .filter(|&i| self.layers.get(i).is_some_and(|l| l.visible && !l.locked))
                .collect()
        } else {
            (0..self.layers.len())
                .filter(|&i| !self.layers[i].locked)
                .collect()
        }
    }

    /// 現在アクティブなレイヤー・フレームの `Cel` 内の図形スライスを返す
    #[cfg(test)]
    pub(crate) fn active_cel_shapes(&self) -> &[FillShape] {
        self.layers
            .get(self.active_layer)
            .and_then(|layer| layer.cel_at(self.current_frame))
            .map(|cel| cel.shapes.as_slice())
            .unwrap_or(&[])
    }

    /// 与えられた図形リスト `shapes` を、現在フレームの各「表示中かつ非ロック」レイヤーの `Cel` に `layer_id` ごとに振り分けて書き込む
    pub(crate) fn write_shapes_to_editable_cels(&mut self, shapes: &[FillShape]) {
        let frame = self.current_frame;
        for layer in &mut self.layers {
            if !layer.visible || layer.locked {
                continue;
            }
            let lid = layer.id;
            if let Some(cel) = layer.cel_at_mut(frame) {
                cel.shapes = shapes
                    .iter()
                    .filter(|s| s.layer_id == lid)
                    .cloned()
                    .collect();
            }
        }
    }

    /// 現在アクティブなレイヤーが編集可能（表示中かつ非ロック）であるかを返す
    pub(crate) fn is_active_layer_editable(&self) -> bool {
        self.layers
            .get(self.active_layer)
            .is_some_and(|l| l.visible && !l.locked)
    }

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

// ============================================================================
// 2. キャンバス ⇔ タイムライン同期・セル選択操作
// ============================================================================

impl FlashBrushApp {
    /// タイムラインの構造変更前に、未確定のキャンバス選択スライスを確定解除し `self.shapes` を現在フレームのセルへ同期する
    fn flush_canvas_before_timeline_op(&mut self) {
        self.clear_selection();
        self.sync_shapes_to_timeline();
    }

    /// `self.shapes` 内の図形列をタイムラインのレイヤー重ね順（背面→前面）で安定ソートし、
    /// `self.selected_shapes` のインデックスも追従更新する
    pub(crate) fn sort_shapes_by_layer_order(&mut self) {
        if self.timeline.layers.len() <= 1 || self.shapes.len() <= 1 {
            return;
        }
        let is_already_sorted = self.shapes.windows(2).all(|w| {
            let i0 = self.timeline.layer_index_by_id(w[0].layer_id).unwrap_or(0);
            let i1 = self.timeline.layer_index_by_id(w[1].layer_id).unwrap_or(0);
            i0 <= i1
        });
        if is_already_sorted {
            return;
        }
        let (is_sel, _) = self.selected_shape_mask();
        let mut indexed: Vec<(usize, bool, FillShape)> = self
            .shapes
            .drain(..)
            .enumerate()
            .map(|(idx, s)| {
                let l_ord = self.timeline.layer_index_by_id(s.layer_id).unwrap_or(0);
                (l_ord, is_sel[idx], s)
            })
            .collect();
        indexed.sort_by_key(|&(l_ord, _, _)| l_ord);
        self.selected_shapes.clear();
        for (new_idx, (_, sel, shape)) in indexed.into_iter().enumerate() {
            if sel {
                self.selected_shapes.push(new_idx);
            }
            self.shapes.push(shape);
        }
    }

    /// キャンバスの作業用 `self.shapes` を、各図形の `layer_id` ごとに現在フレームの `Cel` に振り分けて書き戻す
    pub(crate) fn sync_shapes_to_timeline(&mut self) {
        let fallback_lid = if self.timeline.is_active_layer_editable() {
            self.timeline.active_layer_id()
        } else {
            self.timeline
                .layers
                .iter()
                .find(|l| l.visible && !l.locked)
                .map(|l| l.id)
                .unwrap_or_else(|| self.timeline.active_layer_id())
        };
        for shape in &mut self.shapes {
            if !self.timeline.is_layer_editable_by_id(shape.layer_id) {
                shape.layer_id = fallback_lid;
            }
        }
        self.sort_shapes_by_layer_order();
        self.timeline.write_shapes_to_editable_cels(&self.shapes);
    }

    /// 現在フレームにおける全ての「表示中かつ非ロック（`visible && !locked`）」レイヤーの `Cel` から
    /// 背面レイヤー→前面レイヤーの順で `self.shapes` を一括読み込みする
    pub(crate) fn load_shapes_from_timeline(&mut self) {
        let frame = self.timeline.current_frame;
        let mut loaded = Vec::new();
        for layer in &self.timeline.layers {
            if !layer.visible || layer.locked {
                continue;
            }
            let lid = layer.id;
            if let Some(cel) = layer.cel_at(frame) {
                loaded.extend(cel.shapes.iter().cloned().map(|s| s.with_layer_id(lid)));
            }
        }
        self.shapes = loaded;
        self.selected_shapes.clear();
        self.unsliced_canvas = None;
        self.selection_modified = false;
        self.select_drag = crate::canvas_tool::SelectDragState::None;
        self.transform_drag = crate::canvas_tool::TransformDragState::None;
        self.transform_pivot = None;
        self.sanitize_editing_group_path();
    }

    /// タイムライン上で選択されたセル範囲（単一キーフレームまたは複数レイヤー範囲）に属するすべての図形をキャンバス上で選択状態にする
    pub(crate) fn select_shapes_for_timeline_selection(&mut self) {
        self.clear_selection();
        let Some(sel) = self.timeline.cell_selection else {
            return;
        };
        let selected_lids: Vec<u64> = (sel.min_layer..=sel.max_layer)
            .filter_map(|l_idx| self.timeline.layers.get(l_idx))
            .filter(|l| l.visible && !l.locked)
            .map(|l| l.id)
            .collect();
        self.selected_shapes = (0..self.shapes.len())
            .filter(|&i| {
                self.shapes[i].is_in_editing_scope(&self.editing_group_path)
                    && selected_lids.contains(&self.shapes[i].layer_id)
            })
            .collect();
        self.transform_pivot = None;
    }

    /// タイムラインのセル範囲を選択し、そのレイヤー群の現在フレームにある全図形をキャンバス上で選択状態にする
    pub(crate) fn select_timeline_cells(
        &mut self,
        start_layer: usize,
        start_frame: usize,
        end_layer: usize,
        end_frame: usize,
    ) {
        if self.timeline.layers.is_empty() {
            return;
        }
        let max_l = self.timeline.layers.len() - 1;
        let max_f = self.timeline.total_frames.saturating_sub(1);
        let (l0, l1) = (start_layer.min(max_l), end_layer.min(max_l));
        let (f0, f1) = (start_frame.min(max_f), end_frame.min(max_f));

        self.timeline.is_playing = false;
        self.switch_to_layer_and_frame(l1, f1);
        self.timeline.cell_selection = Some(TimelineCellSelection::from_corners(l0, f0, l1, f1));
        self.select_shapes_for_timeline_selection();
    }

    /// タイムラインのレイヤー個別選択を解除し、指定フレームへ再生ヘッドのみ移動する（ルーラー操作時など）
    pub(crate) fn scrub_ruler_to_frame(&mut self, frame_idx: usize) {
        self.timeline.is_playing = false;
        let clamped_frame = frame_idx.min(self.timeline.total_frames.saturating_sub(1));
        self.flush_canvas_before_timeline_op();
        self.timeline.current_frame = clamped_frame;
        self.timeline.cell_selection = None;
        self.load_shapes_from_timeline();
    }

    /// 指定したレイヤー・フレームへ切り替える
    pub(crate) fn switch_to_layer_and_frame(&mut self, layer_idx: usize, frame_idx: usize) {
        if self.timeline.layers.is_empty() {
            return;
        }
        let clamped_layer = layer_idx.min(self.timeline.layers.len() - 1);
        let clamped_frame = frame_idx.min(self.timeline.total_frames.saturating_sub(1));
        self.flush_canvas_before_timeline_op();
        self.timeline.active_layer = clamped_layer;
        self.timeline.current_frame = clamped_frame;
        self.timeline.cell_selection =
            Some(TimelineCellSelection::single(clamped_layer, clamped_frame));
        self.load_shapes_from_timeline();
    }
}

// ============================================================================
// 3. 再生制御 ＆ キーフレーム・フレーム操作（F5 / Shift+F5 / F6 / F7 / 移動・複製）
// ============================================================================

impl FlashBrushApp {
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
        self.flush_canvas_before_timeline_op();
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
    pub(crate) fn timeline_insert_keyframe(&mut self, clone_prev: bool) -> bool {
        let targets = self.timeline.target_layers_for_frame_op();
        if targets.is_empty() {
            return false;
        }
        self.flush_canvas_before_timeline_op();
        self.push_undo();

        let curr_f = self.timeline.current_frame;
        let is_exact_kf = targets.iter().all(|&l_idx| {
            self.timeline.layers[l_idx]
                .keyframes
                .iter()
                .any(|kf| kf.start_frame == curr_f)
        });
        let curr_non_empty = targets.iter().any(|&l_idx| {
            self.timeline.layers[l_idx]
                .cel_at(curr_f)
                .is_some_and(|c| !c.is_empty())
        });

        let target_frame = if is_exact_kf && (clone_prev || curr_non_empty) {
            curr_f + 1
        } else {
            curr_f
        };

        for &l_idx in &targets {
            self.timeline.layers[l_idx].insert_keyframe(target_frame, clone_prev);
        }
        self.timeline.sync_total_frames();
        self.timeline.current_frame = target_frame;
        if let Some(sel) = &mut self.timeline.cell_selection {
            sel.min_frame = target_frame;
            sel.max_frame = target_frame;
        }
        self.load_shapes_from_timeline();
        if clone_prev {
            self.select_shapes_for_timeline_selection();
        }
        true
    }

    /// 現在フレームのキーフレームを削除（フレーム0の場合は空白化）する（`Shift+F6`）
    pub(crate) fn timeline_delete_keyframe(&mut self) -> bool {
        let targets = self.timeline.target_layers_for_frame_op();
        if targets.is_empty() {
            return false;
        }
        self.flush_canvas_before_timeline_op();
        let curr_f = self.timeline.current_frame;
        let can_delete = targets.iter().any(|&l_idx| {
            let layer = &self.timeline.layers[l_idx];
            layer
                .keyframe_index_at(curr_f)
                .is_some_and(|idx| idx > 0 || !layer.keyframes[0].cel.is_empty())
        });
        if !can_delete {
            return false;
        }
        self.push_undo();
        for &l_idx in &targets {
            self.timeline.layers[l_idx].delete_keyframe_at(curr_f);
        }
        self.load_shapes_from_timeline();
        true
    }

    /// 現在フレームの保持尺を1フレーム延長する（`F5` / Adobe Animate 仕様）：
    /// - 特定のレイヤーが選択されていれば選択レイヤーのみ、未選択時（`cell_selection == None`）は全非ロックレイヤーを +1 フレーム延長する。
    pub(crate) fn timeline_extend_frame(&mut self) -> bool {
        let targets = self.timeline.target_layers_for_frame_op();
        if targets.is_empty() {
            return false;
        }
        self.flush_canvas_before_timeline_op();
        self.push_undo();
        let curr_f = self.timeline.current_frame;
        for &l_idx in &targets {
            self.timeline.layers[l_idx].insert_frame_at(curr_f);
        }
        self.timeline.sync_total_frames();
        self.load_shapes_from_timeline();
        true
    }

    /// 現在フレームの保持尺を1フレーム短縮する（`Shift+F5` / Adobe Animate 仕様）：
    /// - 保持フレームがあれば保持尺を -1 フレーム短縮し、1フレーム幅のキーフレームに当たった場合はそのキーフレーム自体を削除して後続を左へ詰める。
    pub(crate) fn timeline_remove_frame(&mut self) -> bool {
        let targets = self.timeline.target_layers_for_frame_op();
        if targets.is_empty() {
            return false;
        }
        self.flush_canvas_before_timeline_op();
        let curr_f = self.timeline.current_frame;
        let can_remove = targets.iter().any(|&l_idx| {
            let l = &self.timeline.layers[l_idx];
            l.frame_count > 1 || l.keyframes.first().is_some_and(|kf| !kf.cel.is_empty())
        });
        if !can_remove {
            return false;
        }
        self.push_undo();
        let mut any_changed = false;
        for &l_idx in &targets {
            if self.timeline.layers[l_idx].remove_frame_at(curr_f) {
                any_changed = true;
            }
        }
        self.timeline.sync_total_frames();
        if self.timeline.current_frame >= self.timeline.total_frames {
            self.timeline.current_frame = self.timeline.total_frames - 1;
        }
        if let Some(sel) = &mut self.timeline.cell_selection {
            sel.min_frame = sel.min_frame.min(self.timeline.current_frame);
            sel.max_frame = sel.max_frame.min(self.timeline.current_frame);
        }
        self.load_shapes_from_timeline();
        any_changed
    }

    /// 選択されたフレーム範囲 `sel`（単一・複数フレーム・複数レイヤー横断）を `(delta_layer, delta_frame)` だけ移動、
    /// または `copy_mode == true`（`Alt` / `Option` 押下）の場合は移動元を残したまま移動先へ複製する
    pub(crate) fn timeline_move_or_copy_frames(
        &mut self,
        sel: TimelineCellSelection,
        delta_layer: isize,
        delta_frame: isize,
        copy_mode: bool,
    ) -> bool {
        let num_layers = self.timeline.layers.len();
        if num_layers == 0 {
            return false;
        }
        let span_layers = sel.max_layer.saturating_sub(sel.min_layer);
        let span_frames = sel.max_frame.saturating_sub(sel.min_frame);

        let max_min_layer = num_layers.saturating_sub(1 + span_layers);
        let dst_min_layer =
            (sel.min_layer as isize + delta_layer).clamp(0, max_min_layer as isize) as usize;
        let dst_max_layer = dst_min_layer + span_layers;
        let actual_dl = dst_min_layer as isize - sel.min_layer as isize;

        let dst_min_frame = (sel.min_frame as isize + delta_frame).max(0) as usize;
        let dst_max_frame = dst_min_frame + span_frames;
        let actual_df = dst_min_frame as isize - sel.min_frame as isize;

        if actual_dl == 0 && actual_df == 0 {
            return false;
        }

        for src_l in sel.min_layer..=sel.max_layer {
            let dst_l = (src_l as isize + actual_dl) as usize;
            if self.timeline.layers[src_l].locked || self.timeline.layers[dst_l].locked {
                return false;
            }
        }

        self.flush_canvas_before_timeline_op();
        self.push_undo();

        // 1. 各移動元レイヤーから選択フレーム区間内のキーフレーム（または先頭セル画）を抽出
        let mut extracted_per_layer: Vec<(usize, Vec<(usize, Cel)>)> = Vec::new();
        for src_l in sel.min_layer..=sel.max_layer {
            let dst_l = (src_l as isize + actual_dl) as usize;
            let layer = &self.timeline.layers[src_l];
            let mut kfs_in_range: Vec<(usize, Cel)> = layer
                .keyframes
                .iter()
                .filter(|kf| kf.start_frame >= sel.min_frame && kf.start_frame <= sel.max_frame)
                .map(|kf| (kf.start_frame - sel.min_frame, kf.cel.clone()))
                .collect();

            if !kfs_in_range.iter().any(|(off, _)| *off == 0) {
                let fallback_cel = layer.cel_at(sel.min_frame).cloned().unwrap_or_default();
                kfs_in_range.insert(0, (0, fallback_cel));
            }
            extracted_per_layer.push((dst_l, kfs_in_range));
        }

        // 2. 通常移動モード（`!copy_mode`）の場合は移動元の選択範囲のキーフレームをクリア・除去
        if !copy_mode {
            for src_l in sel.min_layer..=sel.max_layer {
                let layer = &mut self.timeline.layers[src_l];
                layer
                    .keyframes
                    .retain(|kf| kf.start_frame < sel.min_frame || kf.start_frame > sel.max_frame);
                if layer.keyframes.is_empty() || layer.keyframes[0].start_frame > 0 {
                    layer.keyframes.insert(0, Keyframe::new_blank(0));
                }
            }
        }

        // 3. 移動先レイヤー・フレーム位置へキーフレーム群を配置（終端を超える場合は尺を自動延長）
        for (dst_l, kfs_in_range) in extracted_per_layer {
            let dst_lid = self.timeline.layers[dst_l].id;
            let dst_layer = &mut self.timeline.layers[dst_l];
            if dst_max_frame >= dst_layer.frame_count {
                dst_layer.frame_count = dst_max_frame + 1;
            }
            for (rel_off, mut cel) in kfs_in_range {
                for shape in &mut cel.shapes {
                    shape.layer_id = dst_lid;
                }
                let target_f = dst_min_frame + rel_off;
                if let Some(existing) = dst_layer
                    .keyframes
                    .iter_mut()
                    .find(|kf| kf.start_frame == target_f)
                {
                    existing.cel = cel;
                } else {
                    let pos = dst_layer
                        .keyframes
                        .partition_point(|kf| kf.start_frame < target_f);
                    dst_layer.keyframes.insert(
                        pos,
                        Keyframe {
                            start_frame: target_f,
                            cel,
                        },
                    );
                }
            }
        }

        self.timeline.sync_total_frames();
        self.timeline.active_layer = dst_max_layer;
        self.timeline.current_frame = dst_min_frame;
        self.timeline.cell_selection = Some(TimelineCellSelection {
            min_layer: dst_min_layer,
            max_layer: dst_max_layer,
            min_frame: dst_min_frame,
            max_frame: dst_max_frame,
        });
        self.load_shapes_from_timeline();
        self.select_shapes_for_timeline_selection();
        true
    }
}

// ============================================================================
// 4. レイヤー管理操作（表示/非表示・ロック・追加・削除・並べ替え）
// ============================================================================

impl FlashBrushApp {
    /// 指定レイヤーの表示/非表示（👁）を設定し、作業キャンバス `self.shapes` を再同期する
    pub(crate) fn set_layer_visible(&mut self, layer_idx: usize, visible: bool) {
        if layer_idx >= self.timeline.layers.len() {
            return;
        }
        self.flush_canvas_before_timeline_op();
        self.timeline.layers[layer_idx].visible = visible;
        self.load_shapes_from_timeline();
    }

    /// 指定レイヤーのロック状態（🔒）を設定し、作業キャンバス `self.shapes` を再同期する
    pub(crate) fn set_layer_locked(&mut self, layer_idx: usize, locked: bool) {
        if layer_idx >= self.timeline.layers.len() {
            return;
        }
        self.flush_canvas_before_timeline_op();
        self.timeline.layers[layer_idx].locked = locked;
        self.load_shapes_from_timeline();
    }

    /// 新しいレイヤーを現在のアクティブレイヤーの1つ上（前面）に追加する
    pub(crate) fn timeline_add_layer(&mut self) {
        self.flush_canvas_before_timeline_op();
        self.push_undo();
        let name = format!("Layer {}", self.timeline.next_layer_number);
        self.timeline.next_layer_number += 1;
        let lid = self.timeline.next_layer_id;
        self.timeline.next_layer_id += 1;
        let insert_idx = (self.timeline.active_layer + 1).min(self.timeline.layers.len());
        let fc = self.timeline.total_frames.max(1);
        self.timeline
            .layers
            .insert(insert_idx, Layer::new_with_id_and_frames(lid, name, fc));
        self.timeline.active_layer = insert_idx;
        self.timeline.cell_selection =
            Some(TimelineCellSelection::single(insert_idx, self.timeline.current_frame));
        self.load_shapes_from_timeline();
    }

    /// 現在アクティブなレイヤーを削除する（最低1レイヤーは残す）
    pub(crate) fn timeline_delete_active_layer(&mut self) -> bool {
        if self.timeline.layers.len() <= 1 {
            return false;
        }
        self.flush_canvas_before_timeline_op();
        self.push_undo();
        self.timeline.layers.remove(self.timeline.active_layer);
        if self.timeline.active_layer >= self.timeline.layers.len() {
            self.timeline.active_layer = self.timeline.layers.len() - 1;
        }
        self.timeline.sync_total_frames();
        if self.timeline.current_frame >= self.timeline.total_frames {
            self.timeline.current_frame = self.timeline.total_frames - 1;
        }
        self.timeline.cell_selection = Some(TimelineCellSelection::single(
            self.timeline.active_layer,
            self.timeline.current_frame,
        ));
        self.load_shapes_from_timeline();
        true
    }

    /// 指定したレイヤー `from_idx` を `to_idx` の重ね順位置へ移動する（ドラッグ＆ドロップ入れ替え対応）
    pub(crate) fn timeline_reorder_layer(&mut self, from_idx: usize, to_idx: usize) -> bool {
        let len = self.timeline.layers.len();
        if from_idx >= len || to_idx >= len || from_idx == to_idx {
            return false;
        }
        self.flush_canvas_before_timeline_op();
        self.push_undo();
        let layer = self.timeline.layers.remove(from_idx);
        self.timeline.layers.insert(to_idx, layer);
        self.timeline.active_layer = to_idx;
        self.timeline.cell_selection =
            Some(TimelineCellSelection::single(to_idx, self.timeline.current_frame));
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
        self.timeline_reorder_layer(curr, target)
    }
}

// ============================================================================
// 5. 下部タイムラインパネル UI（レイアウト計算・入力処理・各領域描画）
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
    fn new(panel_rect: Rect, total_frames: usize, scroll_x: f32) -> Self {
        let grid_left = panel_rect.left() + LAYER_COL_W;
        let grid_top = panel_rect.top() + RULER_H;
        let grid_w = (panel_rect.right() - grid_left).max(1.0);
        let grid_h = (panel_rect.bottom() - grid_top).max(1.0);
        let max_frames_shown = (total_frames + 12).max(36);
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

                let mut layout = TimelineLayout::new(
                    panel_rect,
                    self.timeline.total_frames,
                    self.timeline.scroll_x,
                );
                self.handle_timeline_scroll(ui, &response, &layout);
                layout = TimelineLayout::new(
                    panel_rect,
                    self.timeline.total_frames,
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

                    let len = self.timeline.layers.len();
                    let act = self.timeline.active_layer;
                    if ui.button("＋ レイヤー").on_hover_text("新しいレイヤーを現在のレイヤーの前面に追加").clicked() {
                        self.timeline_add_layer();
                    }
                    if ui.add_enabled(len > 1, egui::Button::new("🗑 削除")).on_hover_text("現在選択中のレイヤーを削除").clicked() {
                        self.timeline_delete_active_layer();
                    }
                    if ui.add_enabled(act + 1 < len, egui::Button::new("▲")).on_hover_text("レイヤーを1つ前面（上）へ移動（ドラッグ＆ドロップでも入れ替え可能）").clicked() {
                        self.timeline_move_active_layer(true);
                    }
                    if ui.add_enabled(act > 0, egui::Button::new("▼")).on_hover_text("レイヤーを1つ背面（下）へ移動（ドラッグ＆ドロップでも入れ替え可能）").clicked() {
                        self.timeline_move_active_layer(false);
                    }

                    ui.separator();

                    if ui.button("⏮").on_hover_text("最初のフレームへ (Home)").clicked() {
                        self.timeline.is_playing = false;
                        self.switch_to_layer_and_frame(self.timeline.active_layer, 0);
                    }
                    if ui.button("◀").on_hover_text("前のフレームへ (, / ←)").clicked() {
                        self.step_frame(-1);
                    }
                    let play_label = if self.timeline.is_playing { "⏸ Stop (Enter)" } else { "▶ Play (Enter)" };
                    if ui.selectable_label(self.timeline.is_playing, play_label).on_hover_text("再生 / 一時停止 (Enter)").clicked() {
                        self.toggle_playback();
                    }
                    if ui.button("▶").on_hover_text("次のフレームへ (. / →)").clicked() {
                        self.step_frame(1);
                    }
                    if ui.button("⏭").on_hover_text("最後のフレームへ (End)").clicked() {
                        self.timeline.is_playing = false;
                        let last = self.timeline.total_frames.saturating_sub(1);
                        self.switch_to_layer_and_frame(self.timeline.active_layer, last);
                    }

                    ui.separator();

                    ui.label(format!("Frame: {:>2} /", self.timeline.current_frame + 1));
                    let mut total_f = self.timeline.total_frames as u32;
                    if ui
                        .add(egui::DragValue::new(&mut total_f).range(1..=600).speed(0.25).suffix(" f"))
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

                    ui.separator();

                    let editable = !self.timeline.target_layers_for_frame_op().is_empty();
                    if ui.add_enabled(editable, egui::Button::new("● キーフレーム (F6)")).on_hover_text("現在の絵を複製して新しいキーフレームを挿入 (F6)").clicked() {
                        self.timeline_insert_keyframe(true);
                    }
                    if ui.add_enabled(editable, egui::Button::new("○ 空白KF (F7)")).on_hover_text("空の新しいキーフレームを挿入 (F7)").clicked() {
                        self.timeline_insert_keyframe(false);
                    }
                    if ui.add_enabled(editable, egui::Button::new("＋ 延長 (F5)")).on_hover_text("選択レイヤー（未選択時は全レイヤー）のフレーム尺を1コマ延長 (F5)").clicked() {
                        self.timeline_extend_frame();
                    }
                    if ui.add_enabled(editable && self.timeline.total_frames > 1, egui::Button::new("－ 短縮")).on_hover_text("選択レイヤー（未選択時は全レイヤー）のフレームを1コマ削除・短縮 (Shift+F5)").clicked() {
                        self.timeline_remove_frame();
                    }
                    if ui.add_enabled(editable, egui::Button::new("✕ KF削除")).on_hover_text("現在のキーフレームを削除 / クリア (Shift+F6)").clicked() {
                        self.timeline_delete_keyframe();
                    }

                    ui.separator();

                    if ui.selectable_label(self.timeline.onion_skin_enabled, "🧅 Onion (O)").on_hover_text("オニオンスキン（前後のフレームの透かし表示）切替 (O)").clicked() {
                        self.timeline.onion_skin_enabled = !self.timeline.onion_skin_enabled;
                    }
                    if self.timeline.onion_skin_enabled {
                        let mut prev_n = self.timeline.onion_prev_frames as u32;
                        let mut next_n = self.timeline.onion_next_frames as u32;
                        ui.label("前:");
                        if ui.add(egui::DragValue::new(&mut prev_n).range(0..=5).speed(0.1)).changed() {
                            self.timeline.onion_prev_frames = prev_n as usize;
                        }
                        ui.label("後:");
                        if ui.add(egui::DragValue::new(&mut next_n).range(0..=5).speed(0.1)).changed() {
                            self.timeline.onion_next_frames = next_n as usize;
                        }
                    }
                });
            });
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
                if pointer_in_layers && max_scroll_y > 0.0 && scroll_delta.y.abs() >= scroll_delta.x.abs() {
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
        if let Some(pos) = pointer_pos {
            if just_clicked && layout.add_hdr_rect.contains(pos) {
                self.timeline_add_layer();
            } else if just_clicked && layout.del_hdr_rect.contains(pos) && self.timeline.layers.len() > 1 {
                self.timeline_delete_active_layer();
            } else if pos.x < layout.grid_left && pos.y >= layout.grid_top && just_clicked {
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
            } else if self.timeline.dragging_layer.is_none() && pos.x >= layout.grid_left && just_clicked {
                let col = layout.frame_at_x(pos.x, self.timeline.scroll_x);
                if pos.y < layout.grid_top {
                    let target_frame = col.min(self.timeline.total_frames.saturating_sub(1));
                    self.timeline.grid_drag = TimelineGridDrag::ScrubRuler;
                    self.scrub_ruler_to_frame(target_frame);
                } else {
                    let num_layers = self.timeline.layers.len();
                    if let Some(target_layer) = layout.layer_at_y(pos.y, self.timeline.scroll_y, num_layers) {
                        let layer_fc = self.timeline.layers[target_layer].frame_count;
                        let max_f = layer_fc.max(self.timeline.total_frames);
                        let target_frame = col.min(max_f.saturating_sub(1));
                        self.timeline.is_playing = false;

                        // 実際に存在するフレーム（col < layer_fc）の上をクリックしたときのみ MoveFrames 判定を行う
                        // （24フレームより後ろの余白領域をクリックした際に、24フレーム目を掴んで移動したと誤判定されるのを防ぐ）
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
                            self.select_timeline_cells(target_layer, target_frame, target_layer, target_frame);
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

        // グリッド・ルーラーのドラッグ継続／確定処理
        match self.timeline.grid_drag {
            TimelineGridDrag::None => {}
            TimelineGridDrag::ScrubRuler => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        let col = layout.frame_at_x(pos.x, self.timeline.scroll_x);
                        self.scrub_ruler_to_frame(col.min(self.timeline.total_frames.saturating_sub(1)));
                    }
                } else {
                    self.timeline.grid_drag = TimelineGridDrag::None;
                }
            }
            TimelineGridDrag::SelectCells { start_layer, start_frame } => {
                if primary_down {
                    if let Some(pos) = pointer_pos {
                        let col = layout.frame_at_x(pos.x, self.timeline.scroll_x);
                        let target_frame = col.min(self.timeline.total_frames.saturating_sub(1));
                        let target_layer = layout.clamped_layer_at_y(pos.y, self.timeline.scroll_y, self.timeline.layers.len());
                        self.select_timeline_cells(start_layer, start_frame, target_layer, target_frame);
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
                        let next_layer = layout.clamped_layer_at_y(pos.y, self.timeline.scroll_y, self.timeline.layers.len());
                        let moved = has_moved || next_layer != grab_layer || next_frame != grab_frame;
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
                            ui.ctx().set_cursor_icon(if alt_held { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
                        }
                    }
                } else {
                    self.timeline.grid_drag = TimelineGridDrag::None;
                    if has_moved {
                        let layer_delta = curr_layer as isize - grab_layer as isize;
                        let frame_delta = curr_frame as isize - grab_frame as isize;
                        self.timeline_move_or_copy_frames(orig_sel, layer_delta, frame_delta, copy_mode || alt_held);
                    } else {
                        self.select_timeline_cells(grab_layer, grab_frame, grab_layer, grab_frame);
                    }
                }
            }
        }

        // レイヤーD&Dの継続・ドロップ確定判定
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
                && layout.layer_at_y(hpos.y, self.timeline.scroll_y, self.timeline.layers.len()).is_some()
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
        let grid_rect = Rect::from_min_max(Pos2::new(layout.grid_left, layout.grid_top), layout.panel_rect.right_bottom());
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
            let is_layer_selected = self.timeline.cell_selection.is_some_and(|s| s.contains_layer(layer_idx));

            for f in layout.first_vis_frame..=layout.last_vis_frame {
                let x0 = layout.frame_x(f, scroll_x);
                let cell_rect = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x0 + CELL_W, y1));
                let bg = if f < layer.frame_count {
                    if is_layer_selected { Color32::from_rgb(236, 243, 252) } else { Color32::from_rgb(248, 249, 252) }
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
                        );
                    }
                }
            }

            if let Some(sel) = self.timeline.cell_selection.filter(|s| s.contains_layer(layer_idx)) {
                let sx0 = layout.frame_x(sel.min_frame, scroll_x);
                let sx1 = layout.frame_x(sel.max_frame + 1, scroll_x);
                let sel_rect = Rect::from_min_max(Pos2::new(sx0 + 0.5, y0 + 0.5), Pos2::new(sx1 - 0.5, y1 - 0.5));
                grid_painter.rect_filled(sel_rect, 0.0, Color32::from_rgba_unmultiplied(0, 125, 245, 65));
                grid_painter.rect_stroke(sel_rect, 0.0, Stroke::new(1.6, Color32::from_rgb(0, 115, 230)));
            }

            grid_painter.line_segment(
                [Pos2::new(layout.grid_left, y1), Pos2::new(layout.panel_rect.right(), y1)],
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
                Pos2::new(layout.frame_x(dst_min_f, scroll_x), layout.row_y(num_layers - 1 - dst_max_l, scroll_y)),
                Pos2::new(layout.frame_x(dst_max_f + 1, scroll_x), layout.row_y(num_layers - dst_min_l, scroll_y)),
            );
            let is_copy = copy_mode || alt_held;
            let (accent, fill) = if is_copy {
                (Color32::from_rgb(20, 165, 80), Color32::from_rgba_unmultiplied(30, 185, 95, 60))
            } else {
                (Color32::from_rgb(225, 130, 15), Color32::from_rgba_unmultiplied(245, 155, 30, 60))
            };
            grid_painter.rect_filled(ghost_rect, 0.0, fill);
            grid_painter.rect_stroke(ghost_rect, 0.0, Stroke::new(2.0, accent));

            if let Some(pos) = pointer_pos {
                let badge_label = if is_copy { "＋ Copy Frames (Alt)" } else { "↔ Move Frames" };
                let badge_rect = Rect::from_min_size(
                    Pos2::new(
                        (pos.x + 12.0).clamp(layout.grid_left + 4.0, layout.panel_rect.right() - 135.0),
                        (pos.y - 22.0).clamp(layout.grid_top + 2.0, layout.panel_rect.bottom() - 22.0),
                    ),
                    Vec2::new(130.0, 20.0),
                );
                grid_painter.rect_filled(badge_rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 242));
                grid_painter.rect_stroke(badge_rect, 0.0, Stroke::new(1.2, accent));
                grid_painter.text(badge_rect.center(), Align2::CENTER_CENTER, badge_label, FontId::proportional(11.0), Color32::from_rgb(20, 25, 35));
            }
        }

        for f in layout.first_vis_frame..=layout.last_vis_frame {
            if (f + 1) % 5 == 0 {
                let fx = layout.frame_x(f + 1, scroll_x);
                grid_painter.line_segment(
                    [Pos2::new(fx, layout.grid_top), Pos2::new(fx, layout.panel_rect.bottom())],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(150, 158, 174, 130)),
                );
            }
        }
    }

    /// 1つのキーフレーム保持区間（帯・1コマ区切り線・先頭丸記号 `●`/`○`・終端マーク `▯`）を描画する
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
    ) {
        let sx0 = layout.frame_x(span_start, scroll_x);
        let sx1 = layout.frame_x(span_end + 1, scroll_x);
        let span_rect = Rect::from_min_max(Pos2::new(sx0, y0), Pos2::new(sx1, y1));

        let span_fill = match (has_art, is_layer_selected) {
            (true, true) => Color32::from_rgb(196, 212, 232),
            (true, false) => Color32::from_rgb(212, 218, 228),
            (false, true) => Color32::from_rgb(244, 248, 255),
            (false, false) => Color32::from_rgb(255, 255, 255),
        };
        grid_painter.rect_filled(span_rect, 0.0, span_fill);

        let sep_col = if has_art {
            Color32::from_rgba_unmultiplied(145, 155, 172, 110)
        } else {
            Color32::from_rgba_unmultiplied(195, 202, 214, 130)
        };
        for f in (span_start.max(layout.first_vis_frame) + 1)..=span_end.min(layout.last_vis_frame) {
            let fx = layout.frame_x(f, scroll_x);
            grid_painter.line_segment([Pos2::new(fx, y0 + 1.0), Pos2::new(fx, y1 - 1.0)], Stroke::new(1.0, sep_col));
        }
        grid_painter.rect_stroke(span_rect, 0.0, Stroke::new(1.0, Color32::from_rgb(155, 162, 175)));

        let kf_center = Pos2::new(sx0 + CELL_W * 0.5, (y0 + y1) * 0.5 + 1.0);
        if has_art {
            grid_painter.circle_filled(kf_center, 3.8, Color32::from_rgb(25, 28, 35));
        } else {
            grid_painter.circle_stroke(kf_center, 3.6, Stroke::new(1.5, Color32::from_rgb(75, 82, 95)));
        }

        if span_end > span_start {
            let line_col = Color32::from_rgb(105, 114, 128);
            let end_mark_x = sx1 - CELL_W * 0.45;
            grid_painter.line_segment(
                [Pos2::new(kf_center.x + 5.5, kf_center.y), Pos2::new(end_mark_x - 3.0, kf_center.y)],
                Stroke::new(1.2, line_col),
            );
            let end_box = Rect::from_center_size(Pos2::new(end_mark_x, kf_center.y), Vec2::new(4.2, 8.5));
            grid_painter.rect_filled(end_box, 0.0, Color32::WHITE);
            grid_painter.rect_stroke(end_box, 0.0, Stroke::new(1.2, Color32::from_rgb(90, 98, 112)));
        }
    }

    /// 5. 上部フレーム番号ルーラー・オニオンスキン帯・再生ヘッドの描画
    fn draw_timeline_ruler(
        &self,
        painter: &egui::Painter,
        grid_painter: &egui::Painter,
        layout: &TimelineLayout,
    ) {
        let ruler_rect = Rect::from_min_max(Pos2::new(layout.grid_left, layout.panel_rect.top()), Pos2::new(layout.panel_rect.right(), layout.grid_top));
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
            let next_end = (cf + self.timeline.onion_next_frames).min(self.timeline.total_frames.saturating_sub(1));
            if next_end > cf {
                ruler_painter.rect_filled(
                    Rect::from_min_max(
                        Pos2::new(layout.frame_x(cf + 1, scroll_x), ruler_rect.top() + 2.0),
                        Pos2::new(layout.frame_x(next_end + 1, scroll_x), ruler_rect.bottom() - 2.0),
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
                [Pos2::new(fx, layout.grid_top - tick_h), Pos2::new(fx, layout.grid_top)],
                Stroke::new(1.0, Color32::from_rgb(145, 152, 165)),
            );
            if is_major {
                let col = if f < self.timeline.total_frames {
                    Color32::from_rgb(40, 45, 55)
                } else {
                    Color32::from_rgb(140, 146, 158)
                };
                ruler_painter.text(Pos2::new(fx + 3.0, layout.panel_rect.top() + 3.0), Align2::LEFT_TOP, format!("{frame_num}"), FontId::proportional(11.0), col);
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
            ruler_painter.text(head_box.center(), Align2::CENTER_CENTER, format!("{}", self.timeline.current_frame + 1), FontId::proportional(11.0), Color32::WHITE);
            grid_painter.line_segment(
                [Pos2::new(head_x, layout.grid_top), Pos2::new(head_x, layout.panel_rect.bottom())],
                Stroke::new(1.8, Color32::from_rgb(225, 50, 50)),
            );
        }

        ruler_painter.line_segment(
            [Pos2::new(layout.grid_left, layout.grid_top), Pos2::new(layout.panel_rect.right(), layout.grid_top)],
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
        let layer_list_rect = Rect::from_min_max(Pos2::new(layout.panel_rect.left(), layout.grid_top), Pos2::new(layout.grid_left, layout.panel_rect.bottom()));
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
            let row_rect = Rect::from_min_max(Pos2::new(layout.panel_rect.left(), y0), Pos2::new(layout.grid_left, y1));
            let layer = &self.timeline.layers[layer_idx];
            let is_selected_layer = self.timeline.cell_selection.is_some_and(|s| s.contains_layer(layer_idx));
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
                let bar = Rect::from_min_max(Pos2::new(layout.panel_rect.left(), y0 + 2.0), Pos2::new(layout.panel_rect.left() + 3.0, y1 - 2.0));
                layer_painter.rect_filled(bar, 0.0, Color32::from_rgb(25, 120, 230));
            }

            let cy = (y0 + y1) * 0.5;
            let vis_col = if layer.visible { Color32::from_rgb(40, 45, 55) } else { Color32::from_rgb(155, 162, 174) };
            layer_painter.text(Pos2::new(layout.panel_rect.left() + 18.0, cy), Align2::CENTER_CENTER, if layer.visible { "👁" } else { "・" }, FontId::proportional(13.0), vis_col);

            let lock_col = if layer.locked { Color32::from_rgb(215, 135, 20) } else { Color32::from_rgb(155, 162, 174) };
            layer_painter.text(Pos2::new(layout.panel_rect.left() + 42.0, cy), Align2::CENTER_CENTER, if layer.locked { "🔒" } else { "·" }, FontId::proportional(12.5), lock_col);

            let name_color = if !layer.visible {
                Color32::from_rgb(145, 152, 164)
            } else if is_selected_layer || is_active {
                Color32::from_rgb(15, 22, 35)
            } else {
                Color32::from_rgb(45, 50, 62)
            };
            layer_painter.text(Pos2::new(layout.panel_rect.left() + 60.0, cy), Align2::LEFT_CENTER, &layer.name, FontId::proportional(12.5), name_color);

            let grip_col = if is_selected_layer || is_active { Color32::from_rgb(75, 105, 150) } else { Color32::from_rgb(150, 158, 170) };
            layer_painter.text(Pos2::new(layout.grid_left - 12.0, cy), Align2::CENTER_CENTER, "≡", FontId::proportional(12.0), grip_col);

            layer_painter.line_segment(
                [Pos2::new(layout.panel_rect.left(), y1), Pos2::new(layout.grid_left, y1)],
                Stroke::new(1.0, Color32::from_rgb(202, 208, 218)),
            );
        }

        if let Some((src_idx, dst_idx, drag_pos)) = drag_layer_feedback {
            if dst_idx != src_idx && dst_idx < num_layers {
                let dst_row = num_layers - 1 - dst_idx;
                let drop_y = if dst_idx > src_idx { layout.row_y(dst_row, scroll_y) } else { layout.row_y(dst_row + 1, scroll_y) };
                let indicator_col = Color32::from_rgb(0, 130, 235);
                painter.line_segment([Pos2::new(layout.panel_rect.left() + 2.0, drop_y), Pos2::new(layout.panel_rect.right(), drop_y)], Stroke::new(2.5, indicator_col));
                painter.circle_filled(Pos2::new(layout.panel_rect.left() + 6.0, drop_y), 3.5, indicator_col);
            }
            if let Some(src_layer) = self.timeline.layers.get(src_idx) {
                let badge_rect = Rect::from_min_size(
                    Pos2::new(
                        drag_pos.x.clamp(layout.panel_rect.left() + 8.0, layout.grid_left - 90.0),
                        (drag_pos.y - 11.0).clamp(layout.grid_top + 2.0, layout.panel_rect.bottom() - 22.0),
                    ),
                    Vec2::new(105.0, 20.0),
                );
                painter.rect_filled(badge_rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 242));
                painter.rect_stroke(badge_rect, 0.0, Stroke::new(1.2, Color32::from_rgb(0, 130, 235)));
                painter.text(badge_rect.center(), Align2::CENTER_CENTER, format!("↕ {}", src_layer.name), FontId::proportional(11.5), Color32::from_rgb(20, 25, 35));
            }
        }

        // 左上ヘッダーセル ("👁 / 🔒 / Layers" + [＋][🗑] ミニボタン)
        let corner_rect = Rect::from_min_max(layout.panel_rect.left_top(), Pos2::new(layout.grid_left, layout.grid_top));
        painter.rect_filled(corner_rect, 0.0, Color32::from_rgb(228, 231, 237));
        let header_cy = layout.panel_rect.top() + RULER_H * 0.5;
        painter.text(Pos2::new(layout.panel_rect.left() + 18.0, header_cy), Align2::CENTER_CENTER, "👁", FontId::proportional(11.5), Color32::from_rgb(75, 82, 95));
        painter.text(Pos2::new(layout.panel_rect.left() + 42.0, header_cy), Align2::CENTER_CENTER, "🔒", FontId::proportional(11.5), Color32::from_rgb(75, 82, 95));
        painter.text(Pos2::new(layout.panel_rect.left() + 60.0, header_cy), Align2::LEFT_CENTER, "Layers", FontId::proportional(11.5), Color32::from_rgb(55, 62, 74));

        let add_hovered = hover_pos.is_some_and(|p| layout.add_hdr_rect.contains(p));
        let del_enabled = num_layers > 1;
        let del_hovered = del_enabled && hover_pos.is_some_and(|p| layout.del_hdr_rect.contains(p));
        painter.rect_filled(layout.add_hdr_rect, 0.0, if add_hovered { Color32::from_rgb(55, 130, 225) } else { Color32::from_rgb(212, 217, 226) });
        painter.rect_stroke(layout.add_hdr_rect, 0.0, Stroke::new(1.0, Color32::from_rgb(175, 182, 194)));
        painter.text(layout.add_hdr_rect.center(), Align2::CENTER_CENTER, "＋", FontId::proportional(12.0), if add_hovered { Color32::WHITE } else { Color32::from_rgb(35, 40, 50) });

        painter.rect_filled(layout.del_hdr_rect, 0.0, if del_hovered { Color32::from_rgb(215, 70, 70) } else { Color32::from_rgb(212, 217, 226) });
        painter.rect_stroke(layout.del_hdr_rect, 0.0, Stroke::new(1.0, Color32::from_rgb(175, 182, 194)));
        let del_col = if del_hovered {
            Color32::WHITE
        } else if del_enabled {
            Color32::from_rgb(35, 40, 50)
        } else {
            Color32::from_rgb(155, 160, 172)
        };
        painter.text(layout.del_hdr_rect.center(), Align2::CENTER_CENTER, "🗑", FontId::proportional(11.0), del_col);

        painter.line_segment([Pos2::new(layout.panel_rect.left(), layout.grid_top), Pos2::new(layout.grid_left, layout.grid_top)], Stroke::new(1.0, Color32::from_rgb(180, 186, 198)));
        painter.line_segment([Pos2::new(layout.grid_left, layout.panel_rect.top()), Pos2::new(layout.grid_left, layout.panel_rect.bottom())], Stroke::new(1.0, Color32::from_rgb(180, 186, 198)));
    }
}
