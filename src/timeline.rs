use crate::{
    symbol::SymbolDef,
    vector_engine::{
        append_mesh, transform_mesh_to_screen, transform_mesh_to_screen_onion, FillShape,
        SymbolInstanceBinding,
    },
    FlashBrushApp,
};
use eframe::egui::{self, Color32, Mesh, Vec2};
use std::{cell::RefCell, time::Duration};

// ============================================================================
// 1. タイムラインのデータモデル（Cel / Keyframe / Selection / Layer / Timeline）
// ============================================================================

/// セル内の全図形を1つに統合したワールド座標バッチメッシュと、直近の `(zoom, pan)` スクリーン座標変換済みキャッシュ
#[derive(Clone, Debug, Default)]
struct CelMeshCache {
    shape_revisions: Vec<(u64, Color32)>,
    world_batch_mesh: Mesh,
    screen_cache: Option<((u32, u32, u32), Mesh)>,
}

/// 1枚のセル画（ベクター図形群 `Vec<FillShape>` ＋ 高速再生・表示用の結合メッシュキャッシュ）
#[derive(Clone, Debug, Default)]
pub(crate) struct Cel {
    pub(crate) shapes: Vec<FillShape>,
    mesh_cache: RefCell<CelMeshCache>,
}

impl Cel {
    pub(crate) fn new(shapes: Vec<FillShape>) -> Self {
        Self {
            shapes,
            mesh_cache: RefCell::new(CelMeshCache::default()),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    fn ensure_world_batch_mesh(&self, cache: &mut CelMeshCache) {
        let is_valid = cache.shape_revisions.len() == self.shapes.len()
            && self
                .shapes
                .iter()
                .zip(&cache.shape_revisions)
                .all(|(s, &(rev, col))| s.mesh_revision == rev && s.color == col);
        if is_valid {
            return;
        }
        cache.shape_revisions = self
            .shapes
            .iter()
            .map(|s| (s.mesh_revision, s.color))
            .collect();
        let mut batch = Mesh::default();
        for shape in &self.shapes {
            append_mesh(&mut batch, &shape.fill_mesh);
        }
        cache.world_batch_mesh = batch;
        cache.screen_cache = None;
    }

    /// セル内の全図形を1つに結合し、指定された `(zoom, pan)` に変換したスクリーン座標 `Mesh` を返す（キャッシュヒット時は頂点計算ゼロ）
    pub(crate) fn cached_screen_mesh(&self, zoom: f32, pan: Vec2, dim_backdrop: bool) -> Mesh {
        let mut cache = self.mesh_cache.borrow_mut();
        self.ensure_world_batch_mesh(&mut cache);
        if dim_backdrop {
            return transform_mesh_to_screen(&cache.world_batch_mesh, zoom, pan, true);
        }
        let key = (zoom.to_bits(), pan.x.to_bits(), pan.y.to_bits());
        if cache.screen_cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let screen_mesh = transform_mesh_to_screen(&cache.world_batch_mesh, zoom, pan, false);
            cache.screen_cache = Some((key, screen_mesh));
        }
        cache.screen_cache.as_ref().unwrap().1.clone()
    }

    /// オニオンスキン表示用に、結合済みワールドメッシュをスクリーン座標＋ティント色へ一括変換して返す
    pub(crate) fn cached_onion_mesh(
        &self,
        zoom: f32,
        pan: Vec2,
        tint_rgb: [u8; 3],
        alpha: u8,
    ) -> Mesh {
        let mut cache = self.mesh_cache.borrow_mut();
        self.ensure_world_batch_mesh(&mut cache);
        transform_mesh_to_screen_onion(&cache.world_batch_mesh, zoom, pan, tint_rgb, alpha)
    }
}

/// タイムライン上のキーフレーム（開始フレーム番号 0-indexed、セル画 `Cel`、クラシックトゥイーン有効フラグ `tween`）
#[derive(Clone, Debug)]
pub(crate) struct Keyframe {
    pub(crate) start_frame: usize,
    pub(crate) cel: Cel,
    pub(crate) tween: bool,
}

impl Keyframe {
    pub(crate) fn new_blank(start_frame: usize) -> Self {
        Self {
            start_frame,
            cel: Cel::default(),
            tween: false,
        }
    }

    /// キーフレーム内のセル図形とトゥイーン設定をクリアし、変更があったかどうかを返す
    pub(crate) fn clear_cel(&mut self) -> bool {
        if self.cel.is_empty() && !self.tween {
            false
        } else {
            self.cel.shapes.clear();
            self.tween = false;
            true
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
    #[cfg(test)]
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
                self.keyframes[existing_idx].clear_cel();
            }
            return existing_idx;
        }

        let lid = self.id;
        let split_tween_binding = if clone_prev {
            self.interpolated_tween_binding_at(frame)
        } else {
            None
        };

        let cloned_shapes = if clone_prev {
            self.cel_at(frame)
                .map(|c| {
                    c.shapes
                        .iter()
                        .cloned()
                        .map(|mut s| {
                            s.layer_id = lid;
                            if let Some((start_b, interp_b)) = split_tween_binding {
                                if start_b.xform.determinant().abs() > 1e-9 {
                                    let delta = interp_b.xform * start_b.xform.inverse();
                                    s.transform(delta);
                                    s.symbol_instance = Some(interp_b);
                                }
                            }
                            s
                        })
                        .collect()
                })
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
                tween: split_tween_binding.is_some(),
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
            self.keyframes[0].clear_cel()
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
            self.keyframes[0].clear_cel()
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

    /// 指定フレーム区間 `[min_frame..=max_frame]` 内のキーフレーム群を相対フレーム位置（`0..=max_frame - min_frame`）で抽出する
    pub(crate) fn extract_keyframes_in_range(
        &self,
        min_frame: usize,
        max_frame: usize,
    ) -> Vec<Keyframe> {
        let mut kfs: Vec<Keyframe> = self
            .keyframes
            .iter()
            .filter(|kf| kf.start_frame >= min_frame && kf.start_frame <= max_frame)
            .map(|kf| Keyframe {
                start_frame: kf.start_frame - min_frame,
                cel: kf.cel.clone(),
                tween: kf.tween,
            })
            .collect();

        if !kfs.iter().any(|kf| kf.start_frame == 0) {
            let fallback_cel = self.cel_at(min_frame).cloned().unwrap_or_default();
            let fallback_tween = self
                .keyframe_index_at(min_frame)
                .is_some_and(|idx| self.keyframes[idx].tween);
            kfs.insert(
                0,
                Keyframe {
                    start_frame: 0,
                    cel: fallback_cel,
                    tween: fallback_tween,
                },
            );
        }
        kfs
    }

    /// 指定フレーム区間 `[min_frame..=max_frame]` 内のキーフレームを除去し、フレーム0のキーフレームが空になった場合は補填する
    pub(crate) fn remove_keyframes_in_range(&mut self, min_frame: usize, max_frame: usize) {
        self.keyframes
            .retain(|kf| kf.start_frame < min_frame || kf.start_frame > max_frame);
        if self.keyframes.is_empty() || self.keyframes[0].start_frame > 0 {
            self.keyframes.insert(0, Keyframe::new_blank(0));
        }
    }

    /// 抽出された相対キーフレーム群 `kfs` を `[dst_min_frame..=dst_max_frame]` に配置する（尺を超える場合は自動延長）
    pub(crate) fn place_keyframes_at(
        &mut self,
        dst_min_frame: usize,
        dst_max_frame: usize,
        kfs: Vec<Keyframe>,
    ) {
        let dst_lid = self.id;
        if dst_max_frame >= self.frame_count {
            self.frame_count = dst_max_frame + 1;
        }
        for mut kf in kfs {
            for shape in &mut kf.cel.shapes {
                shape.layer_id = dst_lid;
            }
            let valid_tween = kf.tween && kf.cel.single_symbol_instance().is_some();
            let target_f = dst_min_frame + kf.start_frame;
            if let Some(existing) = self
                .keyframes
                .iter_mut()
                .find(|existing_kf| existing_kf.start_frame == target_f)
            {
                existing.cel = kf.cel;
                existing.tween = valid_tween;
            } else {
                let pos = self
                    .keyframes
                    .partition_point(|existing_kf| existing_kf.start_frame < target_f);
                self.keyframes.insert(
                    pos,
                    Keyframe {
                        start_frame: target_f,
                        cel: kf.cel,
                        tween: valid_tween,
                    },
                );
            }
        }
    }
}

/// `load_shapes_from_timeline` 読み込み時点のシェイプ署名（トゥイーン中間フレーム等の未編集判定用）
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LoadedShapeSig {
    layer_id: u64,
    mesh_revision: u64,
    color: Color32,
    group_path: Vec<u64>,
    symbol_instance: Option<SymbolInstanceBinding>,
}

impl LoadedShapeSig {
    fn from_shape(s: &FillShape) -> Self {
        Self {
            layer_id: s.layer_id,
            mesh_revision: s.mesh_revision,
            color: s.color,
            group_path: s.group_path.clone(),
            symbol_instance: s.symbol_instance,
        }
    }

    fn matches(&self, s: &FillShape) -> bool {
        self.mesh_revision == s.mesh_revision
            && self.color == s.color
            && self.group_path == s.group_path
            && self.symbol_instance == s.symbol_instance
    }
}

/// レイヤー配列・現在のフレーム・FPS・再生制御・オニオンスキン設定・セル範囲選択・シンボルライブラリを管理するタイムライン構造体
#[derive(Clone, Debug)]
pub(crate) struct Timeline {
    pub(crate) layers: Vec<Layer>,
    pub(crate) library: Vec<SymbolDef>,
    pub(crate) next_symbol_id: u64,
    pub(crate) selected_library_symbol: Option<u64>,
    pub(crate) loaded_signatures: Vec<LoadedShapeSig>,
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
            library: Vec::new(),
            next_symbol_id: 1,
            selected_library_symbol: None,
            loaded_signatures: Vec::new(),
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
    /// シンボル内部タイムライン編集モード用に、親タイムラインのライブラリ・設定を引き継いだ作業用 `Timeline` を生成する
    pub(crate) fn new_for_symbol_edit(&self, sym_def: &SymbolDef, sym_layers: Vec<Layer>) -> Self {
        let act_l = sym_def.active_layer.min(sym_layers.len().saturating_sub(1));
        let act_f = sym_def
            .current_frame
            .min(sym_def.total_frames.saturating_sub(1));
        Self {
            layers: sym_layers,
            library: self.library.clone(),
            next_symbol_id: self.next_symbol_id,
            selected_library_symbol: Some(sym_def.id),
            loaded_signatures: Vec::new(),
            current_frame: act_f,
            active_layer: act_l,
            total_frames: sym_def.total_frames.max(1),
            fps: self.fps,
            is_playing: false,
            last_frame_advance_time: None,
            onion_skin_enabled: self.onion_skin_enabled,
            onion_prev_frames: self.onion_prev_frames,
            onion_next_frames: self.onion_next_frames,
            next_layer_number: sym_def.next_layer_number.max(2),
            next_layer_id: sym_def.next_layer_id.max(2),
            scroll_x: 0.0,
            scroll_y: 0.0,
            last_ensured_frame: 0,
            dragging_layer: None,
            cell_selection: Some(TimelineCellSelection::single(act_l, act_f)),
            grid_drag: TimelineGridDrag::None,
        }
    }

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

    /// 指定した `(layer_idx, frame_idx)` をアクティブレイヤー・現在フレーム・単一セル選択状態に設定する
    /// （`frame_idx` は現在の `total_frames` より後ろの未作成フレーム番号も選択可能）
    pub(crate) fn select_single_cell(&mut self, layer_idx: usize, frame_idx: usize) {
        let clamped_layer = layer_idx.min(self.layers.len().saturating_sub(1));
        self.active_layer = clamped_layer;
        self.current_frame = frame_idx;
        self.cell_selection = Some(TimelineCellSelection::single(clamped_layer, frame_idx));
    }

    /// 現在アクティブなレイヤーの固有 `layer_id` を返す
    pub(crate) fn active_layer_id(&self) -> u64 {
        self.layers.get(self.active_layer).map(|l| l.id).unwrap_or(1)
    }

    /// 指定した `layer_id` を持つレイヤーのインデックス（`0..layers.len()`）を返す
    pub(crate) fn layer_index_by_id(&self, layer_id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == layer_id)
    }

    /// 指定した `layer_id` のレイヤーが現在フレームにおいて表示中かつ非ロック（編集可能）であるかを返す
    pub(crate) fn is_layer_editable_by_id(&self, layer_id: u64) -> bool {
        self.layers.iter().any(|l| {
            l.id == layer_id && l.visible && !l.locked && self.current_frame < l.frame_count
        })
    }

    /// 現在フレームにおいて表示中かつ非ロック（編集可能）なレイヤーが1つ以上存在するかを返す
    pub(crate) fn has_any_editable_layer(&self) -> bool {
        self.layers
            .iter()
            .any(|l| l.visible && !l.locked && self.current_frame < l.frame_count)
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

    /// `load_shapes_from_timeline` 読み込み時点のシェイプ署名一覧を記録する
    pub(crate) fn record_loaded_signatures(&mut self, shapes: &[FillShape]) {
        self.loaded_signatures = shapes.iter().map(LoadedShapeSig::from_shape).collect();
    }

    /// 指定レイヤー `lid` のシェイプ群が直近の `record_loaded_signatures` 時点から未変更であるかを判定する
    fn is_layer_unchanged_since_load(&self, lid: u64, shapes: &[FillShape]) -> bool {
        let mut loaded_iter = self.loaded_signatures.iter().filter(|sig| sig.layer_id == lid);
        shapes
            .iter()
            .filter(|s| s.layer_id == lid)
            .all(|s| loaded_iter.next().is_some_and(|sig| sig.matches(s)))
            && loaded_iter.next().is_none()
    }

    /// 与えられた図形リスト `shapes` を、現在フレームの各「表示中かつ非ロック」レイヤーの `Cel` に `layer_id` ごとに振り分けて書き込む
    /// （変更がないレイヤーはクローンとキャッシュ破棄をスキップする）
    pub(crate) fn write_shapes_to_editable_cels(&mut self, shapes: &[FillShape]) {
        let frame = self.current_frame;
        for l_idx in 0..self.layers.len() {
            if !self.layers[l_idx].visible || self.layers[l_idx].locked {
                continue;
            }
            let lid = self.layers[l_idx].id;
            let is_tween_mid = self.layers[l_idx].interpolated_tween_binding_at(frame).is_some();
            let has_multiframe = self.layers[l_idx]
                .cel_at(frame)
                .is_some_and(|c| self.has_multiframe_symbol(&c.shapes));

            if (is_tween_mid || has_multiframe) && self.is_layer_unchanged_since_load(lid, shapes) {
                continue;
            }

            if is_tween_mid {
                let new_shapes: Vec<FillShape> = shapes
                    .iter()
                    .filter(|s| s.layer_id == lid)
                    .cloned()
                    .collect();
                let kf_idx = self.layers[l_idx].insert_keyframe(frame, false);
                self.layers[l_idx].keyframes[kf_idx].cel.shapes = new_shapes;
                self.layers[l_idx].keyframes[kf_idx].tween = self.layers[l_idx].keyframes[kf_idx]
                    .cel
                    .single_symbol_instance()
                    .is_some();
                continue;
            }

            if let Some(kf_idx) = self.layers[l_idx].keyframe_index_at(frame) {
                let kf = &mut self.layers[l_idx].keyframes[kf_idx];
                let mut layer_iter = shapes.iter().filter(|s| s.layer_id == lid);
                let unchanged = kf.cel.shapes.iter().all(|c_shape| {
                    layer_iter
                        .next()
                        .is_some_and(|s| LoadedShapeSig::from_shape(c_shape).matches(s))
                }) && layer_iter.next().is_none();
                if !unchanged {
                    kf.cel.shapes = shapes
                        .iter()
                        .filter(|s| s.layer_id == lid)
                        .cloned()
                        .collect();
                    if kf.tween && kf.cel.single_symbol_instance().is_none() {
                        kf.tween = false;
                    }
                }
            }
        }
        self.record_loaded_signatures(shapes);
    }

    /// 現在アクティブなレイヤーが現在フレームにおいて編集可能（表示中・非ロック・フレーム作成済み）であるかを返す
    pub(crate) fn is_active_layer_editable(&self) -> bool {
        self.layers
            .get(self.active_layer)
            .is_some_and(|l| l.visible && !l.locked && self.current_frame < l.frame_count)
    }
}

// ============================================================================
// 2. キャンバス ⇔ タイムライン同期・セル選択操作
// ============================================================================

impl FlashBrushApp {
    /// タイムラインの構造変更前に、未確定のキャンバス選択スライスを確定解除し `self.shapes` を現在フレームのセルへ同期する
    /// （再生中の場合は古い `self.shapes` で現在フレームを上書きしないよう、再生停止して現在フレームの図形を読み込む）
    pub(crate) fn flush_canvas_before_timeline_op(&mut self) {
        if self.timeline.is_playing {
            self.stop_playback();
        } else {
            self.clear_selection();
            self.sync_shapes_to_timeline();
        }
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
        if self.timeline.is_playing {
            return;
        }
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
        self.sync_active_symbol_edit();
    }

    /// 現在フレームにおける全ての「表示中かつ非ロック（`visible && !locked`）」レイヤーの `Cel` から
    /// 背面レイヤー→前面レイヤーの順で（トゥイーン補間およびシンボルの内部タイムラインフレームも評価しつつ）`self.shapes` を一括読み込みする
    pub(crate) fn load_shapes_from_timeline(&mut self) {
        let frame = self.timeline.current_frame;
        let mut loaded = Vec::new();
        for layer_idx in 0..self.timeline.layers.len() {
            let layer = &self.timeline.layers[layer_idx];
            if !layer.visible || layer.locked {
                continue;
            }
            loaded.extend(self.timeline.evaluate_layer_shapes_at(layer_idx, frame, false));
        }
        self.timeline.record_loaded_signatures(&loaded);
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
    /// （2フレーム目以降のまだ尺が伸びていない空マスも直接選択でき、そのまま `F5` / `F6` / `F7` でその位置まで尺を伸ばせる）
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
        let (l0, l1) = (start_layer.min(max_l), end_layer.min(max_l));
        let (f0, f1) = (start_frame, end_frame);

        self.switch_to_layer_and_frame(l1, f1);
        self.timeline.cell_selection = Some(TimelineCellSelection::from_corners(l0, f0, l1, f1));
        self.select_shapes_for_timeline_selection();
    }

    /// タイムラインのレイヤー個別選択を解除し、指定フレームへ再生ヘッドのみ移動する（ルーラー操作時など）
    pub(crate) fn scrub_ruler_to_frame(&mut self, frame_idx: usize) {
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
        self.flush_canvas_before_timeline_op();
        self.timeline.select_single_cell(layer_idx, frame_idx);
        self.load_shapes_from_timeline();
    }
}

// ============================================================================
// 3. 再生制御 ＆ キーフレーム・フレーム操作（F5 / Shift+F5 / F6 / F7 / 移動・複製）
// ============================================================================

impl FlashBrushApp {
    /// 前後のフレームへコマ送り移動する（`,` / `.` キーやボタン操作）
    pub(crate) fn step_frame(&mut self, delta: isize) {
        let total = self.timeline.total_frames.max(1) as isize;
        let next = (self.timeline.current_frame as isize + delta).rem_euclid(total) as usize;
        self.switch_to_layer_and_frame(self.timeline.active_layer, next);
    }

    /// 再生中であれば停止し、停止時点のフレームの図形群を作業用 `self.shapes` に展開する
    pub(crate) fn stop_playback(&mut self) {
        if self.timeline.is_playing {
            self.timeline.is_playing = false;
            self.timeline.last_frame_advance_time = None;
            self.load_shapes_from_timeline();
        }
    }

    /// 再生 / 一時停止を切り替える（`Enter` キーまたは Play ボタン）
    pub(crate) fn toggle_playback(&mut self) {
        if self.timeline.is_playing {
            self.stop_playback();
        } else {
            self.flush_canvas_before_timeline_op();
            if self.timeline.current_frame >= self.timeline.total_frames {
                self.timeline.current_frame = 0;
            }
            self.timeline.is_playing = true;
            self.timeline.last_frame_advance_time = None;
        }
    }

    /// 再生中のフレーム自動進行を処理する
    /// （再生中は毎コマの `FillShape` クローンを行わず、`Cel` の結合メッシュキャッシュから直接高速描画する）
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
                self.timeline.current_frame = (self.timeline.current_frame + steps) % total;
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

    /// 現在フレーム（または選択セル範囲の終端フレーム）の保持尺を延長する（`F5` / Adobe Animate 仕様）：
    /// - 2フレーム目以降の未作成の空マスを選択している場合は、そのフレーム位置まで一気に尺を伸ばす。
    /// - 既存のフレーム内を選択している場合は、選択レイヤー（未選択時は全非ロックレイヤー）を +1 フレーム延長する。
    pub(crate) fn timeline_extend_frame(&mut self) -> bool {
        let targets = self.timeline.target_layers_for_frame_op();
        if targets.is_empty() {
            return false;
        }
        self.flush_canvas_before_timeline_op();
        self.push_undo();
        let target_f = self
            .timeline
            .cell_selection
            .map(|s| s.max_frame.max(self.timeline.current_frame))
            .unwrap_or(self.timeline.current_frame);
        for &l_idx in &targets {
            self.timeline.layers[l_idx].insert_frame_at(target_f);
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

        let extracted: Vec<(usize, Vec<Keyframe>)> = (sel.min_layer..=sel.max_layer)
            .map(|src_l| {
                let dst_l = (src_l as isize + actual_dl) as usize;
                let kfs = self.timeline.layers[src_l]
                    .extract_keyframes_in_range(sel.min_frame, sel.max_frame);
                (dst_l, kfs)
            })
            .collect();

        if !copy_mode {
            for src_l in sel.min_layer..=sel.max_layer {
                self.timeline.layers[src_l].remove_keyframes_in_range(sel.min_frame, sel.max_frame);
            }
        }

        for (dst_l, kfs) in extracted {
            self.timeline.layers[dst_l].place_keyframes_at(dst_min_frame, dst_max_frame, kfs);
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
        self.timeline
            .select_single_cell(insert_idx, self.timeline.current_frame);
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
        self.timeline.sync_total_frames();
        self.timeline
            .select_single_cell(self.timeline.active_layer, self.timeline.current_frame);
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
        self.timeline
            .select_single_cell(to_idx, self.timeline.current_frame);
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
