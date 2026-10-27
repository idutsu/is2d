use eframe::egui::{self, Color32, Mesh, Pos2, Stroke, Vec2};
use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::Tool;
use crate::timeline::{Cel, Layer, Timeline, TimelineCellSelection, TimelineGridDrag};
use crate::vector_engine::{
    append_mesh, transform_mesh_to_screen, transform_mesh_to_screen_onion, FillShape,
    SymbolInstanceBinding,
};
use crate::FlashBrushApp;

// ============================================================================
// 1. シンボル定義（SymbolDef）・編集中セッション退避（SymbolEditSession）・共通ヘルパー
// ============================================================================

impl SymbolInstanceBinding {
    /// ローカル座標系のシェイプ列 `local_shapes` を、このインスタンスの変換行列 `self.xform`・`layer_id`・
    /// スコープパス `scope_prefix`（末尾に `self.instance_gid` を自動付与）でステージ座標へ展開する
    pub(crate) fn instantiate_shapes(
        self,
        local_shapes: &[FillShape],
        layer_id: u64,
        scope_prefix: &[u64],
    ) -> Vec<FillShape> {
        let mut out = Vec::with_capacity(local_shapes.len());
        for s in local_shapes {
            let mut inst_s = s.clone();
            let mut full_path = Vec::with_capacity(scope_prefix.len() + 1 + inst_s.group_path.len());
            full_path.extend_from_slice(scope_prefix);
            full_path.push(self.instance_gid);
            full_path.extend(inst_s.group_path.iter().copied());
            inst_s.transform(self.xform);
            inst_s.layer_id = layer_id;
            inst_s.group_path = full_path;
            inst_s.symbol_instance = Some(self);
            out.push(inst_s);
        }
        out
    }

    /// 既存シェイプの `group_path` から、このインスタンス `self.instance_gid` より外側の親スコープパススライスを返す
    pub(crate) fn parent_scope_from_path(self, group_path: &[u64]) -> &[u64] {
        let pos = group_path
            .iter()
            .position(|&gid| gid == self.instance_gid)
            .unwrap_or(group_path.len());
        &group_path[..pos]
    }
}

/// シンボルの基準点（`＋` マーク）をスクリーン座標 `center` に描画する共通ヘルパー
pub(crate) fn draw_registration_cross(
    painter: &egui::Painter,
    center: Pos2,
    arm_len: f32,
    with_circle: bool,
) {
    let cyan = Color32::from_rgb(0, 155, 240);
    if with_circle {
        painter.circle_filled(center, 4.5, Color32::WHITE);
        painter.circle_stroke(center, 4.5, Stroke::new(1.3, cyan));
    }
    painter.line_segment(
        [center - Vec2::new(arm_len, 0.0), center + Vec2::new(arm_len, 0.0)],
        Stroke::new(1.3, cyan),
    );
    painter.line_segment(
        [center - Vec2::new(0.0, arm_len), center + Vec2::new(0.0, arm_len)],
        Stroke::new(1.3, cyan),
    );
}

/// レイヤー配列内の全キーフレーム・全シェイプに対してアフィン変換 `affine` を適用し、シンボル紐付けをクリアする
fn transform_layers_shapes(layers: &mut [Layer], affine: Affine) {
    for layer in layers {
        for kf in &mut layer.keyframes {
            for s in &mut kf.cel.shapes {
                s.transform(affine);
                s.symbol_instance = None;
            }
        }
    }
}

/// ライブラリに登録されたシンボル定義（独自のタイムライン `layers` とローカル基準点 `(0, 0)` を持つ）
#[derive(Clone, Debug)]
pub(crate) struct SymbolDef {
    pub(crate) id: u64,
    pub(crate) name: String,
    /// シンボル内部の独自タイムラインレイヤー群（各シェイプは基準点 `(0, 0)` 中心のローカル座標系）
    pub(crate) layers: Vec<Layer>,
    pub(crate) total_frames: usize,
    pub(crate) active_layer: usize,
    pub(crate) current_frame: usize,
    pub(crate) next_layer_number: usize,
    pub(crate) next_layer_id: u64,
}

impl SymbolDef {
    pub(crate) fn new(id: u64, name: String, mut local_shapes: Vec<FillShape>) -> Self {
        for s in &mut local_shapes {
            s.layer_id = 1;
        }
        let mut layer = Layer::new_with_id(1, "Layer 1");
        layer.keyframes[0].cel = Cel::new(local_shapes);
        Self {
            id,
            name,
            layers: vec![layer],
            total_frames: 1,
            active_layer: 0,
            current_frame: 0,
            next_layer_number: 2,
            next_layer_id: 2,
        }
    }

    fn collect_visible_local_shapes_at(&self, frame: usize) -> Vec<FillShape> {
        let mut out = Vec::new();
        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            if let Some(cel) = layer.cel_at(frame) {
                for s in &cel.shapes {
                    let mut cloned = s.clone();
                    cloned.layer_id = 1;
                    out.push(cloned);
                }
            }
        }
        out
    }

    /// 指定されたフレーム番号 `offset_frame` に対応するシンボル内部フレーム（`offset_frame % total_frames` でループ評価）の
    /// 全可視レイヤーのローカル図形列（背面→前面順）を返す
    pub(crate) fn evaluated_local_shapes_at(
        &self,
        offset_frame: usize,
        allow_empty: bool,
    ) -> Vec<FillShape> {
        let fc = self.total_frames.max(1);
        let out = self.collect_visible_local_shapes_at(offset_frame % fc);
        if !out.is_empty() || allow_empty {
            return out;
        }
        (0..fc)
            .find_map(|f| {
                let fallback = self.collect_visible_local_shapes_at(f);
                (!fallback.is_empty()).then_some(fallback)
            })
            .unwrap_or_default()
    }

    /// 作業用タイムライン `working_tl` と逆変換行列 `inv_xform` からシンボル定義のレイヤー・フレーム状態を更新する
    pub(crate) fn update_from_working_timeline(&mut self, working_tl: &Timeline, inv_xform: Affine) {
        let mut local_layers = working_tl.layers.clone();
        transform_layers_shapes(&mut local_layers, inv_xform);
        self.layers = local_layers;
        self.total_frames = working_tl.total_frames.max(1);
        self.active_layer = working_tl.active_layer;
        self.current_frame = working_tl.current_frame;
        self.next_layer_number = working_tl.next_layer_number;
        self.next_layer_id = working_tl.next_layer_id;
    }
}

/// シンボルをダブルクリックしてシンボル独自のタイムライン編集に入った際に退避する親シーンの状態
#[derive(Clone, Debug)]
pub(crate) struct SymbolEditSession {
    pub(crate) symbol_id: u64,
    pub(crate) symbol_name: String,
    pub(crate) instance_gid: u64,
    pub(crate) instance_xform: Affine,
    pub(crate) parent_timeline: Timeline,
    pub(crate) parent_editing_group_path: Vec<u64>,
    /// 親シーンの現在フレームにおける周辺図形（対象シンボルインスタンス自身を除いた背景表示用シェイプ群）
    pub(crate) backdrop_shapes: Vec<FillShape>,
}

// ============================================================================
// 2. タイムライン上のシンボルフレーム評価・メッシュ生成（impl Timeline）
// ============================================================================

impl Timeline {
    /// 与えられた図形スライス内に、複数フレームを持つシンボルインスタンスが含まれているかを返す
    pub(crate) fn has_multiframe_symbol(&self, shapes: &[FillShape]) -> bool {
        shapes.iter().any(|s| {
            s.symbol_instance.is_some_and(|b| {
                self.library
                    .iter()
                    .any(|sym| sym.id == b.symbol_id && sym.total_frames > 1)
            })
        })
    }

    /// 指定フレーム `frame` において、各シンボルインスタンスの内部タイムライン（`frame % sym.total_frames`）を評価して展開した図形列を返す
    pub(crate) fn resolve_shapes_with_symbols(
        &self,
        raw_shapes: &[FillShape],
        frame: usize,
        allow_empty: bool,
    ) -> Vec<FillShape> {
        if !raw_shapes.iter().any(|s| s.symbol_instance.is_some()) {
            return raw_shapes.to_vec();
        }

        let mut out = Vec::with_capacity(raw_shapes.len());
        let mut processed_gids: Vec<u64> = Vec::new();

        for shape in raw_shapes {
            let Some(b) = shape.symbol_instance else {
                out.push(shape.clone());
                continue;
            };
            if processed_gids.contains(&b.instance_gid) {
                continue;
            }
            processed_gids.push(b.instance_gid);

            let Some(sym) = self.library.iter().find(|sym| sym.id == b.symbol_id) else {
                out.push(shape.clone());
                continue;
            };

            let parent_scope = b.parent_scope_from_path(&shape.group_path);
            let local_shapes = sym.evaluated_local_shapes_at(frame, allow_empty);
            out.extend(b.instantiate_shapes(&local_shapes, shape.layer_id, parent_scope));
        }
        out
    }

    /// 指定レイヤー・フレームの結合済みワールド座標メッシュを（複数フレームシンボル評価込みで）構築する
    fn build_resolved_world_batch_mesh(&self, shapes: &[FillShape], frame: usize) -> Option<Mesh> {
        let resolved = self.resolve_shapes_with_symbols(shapes, frame, true);
        if resolved.is_empty() {
            return None;
        }
        let mut batch = Mesh::default();
        for s in &resolved {
            append_mesh(&mut batch, &s.fill_mesh);
        }
        Some(batch)
    }

    /// 指定レイヤー・フレームの描画用スクリーン `Mesh` を返す（複数フレームシンボルを含む場合はフレーム評価を行い、それ以外は高速キャッシュを返す）
    pub(crate) fn resolved_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
    ) -> Option<Mesh> {
        let cel = self.layers.get(layer_idx)?.cel_at(frame)?;
        if cel.is_empty() {
            return None;
        }
        if !self.has_multiframe_symbol(&cel.shapes) {
            return Some(cel.cached_screen_mesh(zoom, pan, dim_backdrop));
        }
        let batch = self.build_resolved_world_batch_mesh(&cel.shapes, frame)?;
        Some(transform_mesh_to_screen(&batch, zoom, pan, dim_backdrop))
    }

    /// 指定レイヤー・フレームのオニオンスキン描画用スクリーン `Mesh` を返す（複数フレームシンボル対応）
    pub(crate) fn resolved_cel_onion_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        tint_rgb: [u8; 3],
        alpha: u8,
    ) -> Option<Mesh> {
        let cel = self.layers.get(layer_idx)?.cel_at(frame)?;
        if cel.is_empty() {
            return None;
        }
        if !self.has_multiframe_symbol(&cel.shapes) {
            return Some(cel.cached_onion_mesh(zoom, pan, tint_rgb, alpha));
        }
        let batch = self.build_resolved_world_batch_mesh(&cel.shapes, frame)?;
        Some(transform_mesh_to_screen_onion(
            &batch, zoom, pan, tint_rgb, alpha,
        ))
    }
}

// ============================================================================
// 3. シンボル変換・ライブラリ配置/削除・シンボルタイムライン階層遷移（impl FlashBrushApp）
// ============================================================================

impl FlashBrushApp {
    /// 選択中のオブジェクト（生シェイプ・グループ問わず）を新しい「シンボル」に変換してライブラリに登録する（`F8`）
    pub(crate) fn convert_selection_to_symbol(&mut self) -> Option<u64> {
        if self.selected_shapes.is_empty() {
            return None;
        }
        let snapshot = self.take_or_clone_unsliced_snapshot();
        let valid_indices: Vec<usize> = self
            .selected_shapes
            .iter()
            .copied()
            .filter(|&idx| {
                self.shapes
                    .get(idx)
                    .is_some_and(|s| s.is_in_editing_scope(&self.editing_group_path))
            })
            .collect();
        if valid_indices.is_empty() {
            self.unsliced_canvas = Some(snapshot);
            return None;
        }

        let mut bbox = self.shapes[valid_indices[0]].bounding_box();
        for &idx in &valid_indices[1..] {
            bbox = bbox.union(self.shapes[idx].bounding_box());
        }
        let center = Point::new((bbox.x0 + bbox.x1) * 0.5, (bbox.y0 + bbox.y1) * 0.5);

        let sym_id = self.timeline.next_symbol_id;
        let sym_name = format!("Symbol {sym_id}");
        let inst_gid = self.next_group_id;
        self.next_group_id += 1;

        let depth = self.editing_group_path.len();
        let to_local = KVec2::new(-center.x, -center.y);
        let mut local_shapes = Vec::with_capacity(valid_indices.len());
        for &idx in &valid_indices {
            let mut local_s = self.shapes[idx].clone();
            local_s.layer_id = 1;
            local_s.group_path = local_s.group_path[depth..].to_vec();
            local_s.symbol_instance = None;
            local_s.translate(to_local);
            local_shapes.push(local_s);
        }

        let target_lid = valid_indices
            .iter()
            .map(|&idx| self.shapes[idx].layer_id)
            .max_by_key(|&lid| self.timeline.layer_index_by_id(lid).unwrap_or(0))
            .unwrap_or_else(|| self.active_scope_layer_id());
        let binding = SymbolInstanceBinding {
            instance_gid: inst_gid,
            symbol_id: sym_id,
            xform: Affine::translate(KVec2::new(center.x, center.y)),
        };
        for &idx in &valid_indices {
            if let Some(shape) = self.shapes.get_mut(idx) {
                shape.layer_id = target_lid;
                shape.group_path.insert(depth, inst_gid);
                shape.symbol_instance = Some(binding);
            }
        }

        if let Some(l_idx) = self.timeline.layer_index_by_id(target_lid) {
            self.timeline.active_layer = l_idx;
        }
        // Undo スナップショットに変換前のライブラリ状態が残るよう、先に commit してからライブラリへ追加する
        self.commit_moved_selection(snapshot);
        self.timeline.next_symbol_id += 1;
        let sym_def = SymbolDef::new(sym_id, sym_name, local_shapes);
        self.timeline.library.push(sym_def.clone());
        self.timeline.selected_library_symbol = Some(sym_id);
        for sess in &mut self.symbol_edit_stack {
            sess.parent_timeline.next_symbol_id = self.timeline.next_symbol_id;
            sess.parent_timeline.library.push(sym_def.clone());
        }
        self.selection_modified = false;
        self.transform_pivot = None;
        Some(sym_id)
    }

    /// ライブラリ内の指定シンボル `symbol_id` の新しいインスタンスを現在のステージ中央へ配置する
    pub(crate) fn place_symbol_instance_from_library(&mut self, symbol_id: u64) -> bool {
        let Some(sym_shapes) = self
            .timeline
            .library
            .iter()
            .find(|s| s.id == symbol_id)
            .map(|s| s.evaluated_local_shapes_at(self.timeline.current_frame, false))
        else {
            return false;
        };
        if sym_shapes.is_empty() || !self.timeline.has_any_editable_layer() {
            return false;
        }

        self.clear_selection();
        self.push_undo();

        let inst_gid = self.next_group_id;
        self.next_group_id += 1;
        let stage_center = self.screen_to_world(Pos2::new(500.0, 300.0));
        let binding = SymbolInstanceBinding {
            instance_gid: inst_gid,
            symbol_id,
            xform: Affine::translate(KVec2::new(stage_center.x, stage_center.y)),
        };
        let target_lid = self.active_scope_layer_id();
        let placed = binding.instantiate_shapes(&sym_shapes, target_lid, &self.editing_group_path);

        let insert_pos = self.scope_insert_index();
        let count = placed.len();
        self.shapes.splice(insert_pos..insert_pos, placed);
        self.selected_shapes = (insert_pos..insert_pos + count).collect();
        self.sort_shapes_by_layer_order();
        self.selection_modified = false;
        self.transform_pivot = None;
        if !matches!(self.tool, Tool::Select | Tool::Transform) {
            self.set_tool(Tool::Select);
        }
        true
    }

    /// ライブラリから指定シンボル `symbol_id` を削除し、全フレーム・キャンバス上のインスタンスも除去する
    pub(crate) fn delete_symbol_from_library(&mut self, symbol_id: u64) -> bool {
        if !self.timeline.library.iter().any(|s| s.id == symbol_id) {
            return false;
        }
        self.clear_selection();
        self.push_undo();

        self.timeline.library.retain(|s| s.id != symbol_id);
        if self.timeline.selected_library_symbol == Some(symbol_id) {
            self.timeline.selected_library_symbol = self.timeline.library.first().map(|s| s.id);
        }
        self.shapes
            .retain(|s| s.symbol_instance.is_none_or(|b| b.symbol_id != symbol_id));
        for layer in &mut self.timeline.layers {
            for kf in &mut layer.keyframes {
                kf.cel
                    .shapes
                    .retain(|s| s.symbol_instance.is_none_or(|b| b.symbol_id != symbol_id));
            }
        }
        self.sanitize_editing_group_path();
        true
    }

    /// 現在選択されている図形が単一のシンボルインスタンスである場合、その `(SymbolInstanceBinding, シンボル名)` を返す
    pub(crate) fn selected_single_symbol_info(&self) -> Option<(SymbolInstanceBinding, String)> {
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
        if !all_same {
            return None;
        }
        let name = self
            .timeline
            .library
            .iter()
            .find(|sym| sym.id == first_binding.symbol_id)
            .map(|sym| sym.name.clone())?;
        Some((first_binding, name))
    }

    /// シンボルインスタンスをダブルクリックして、そのシンボル専用の内部タイムライン編集モードへ入る
    pub(crate) fn enter_symbol_timeline(&mut self, binding: SymbolInstanceBinding) -> bool {
        let Some(sym_def) = self
            .timeline
            .library
            .iter()
            .find(|s| s.id == binding.symbol_id)
            .cloned()
        else {
            return false;
        };

        self.flush_canvas_before_timeline_op();

        // 親シーン（およびさらに外側の親シンボル）の現在フレーム図形のうち、今回入るインスタンス以外の図形を背景用に退避
        let mut backdrop_shapes: Vec<FillShape> = self
            .symbol_edit_stack
            .last()
            .map(|s| s.backdrop_shapes.clone())
            .unwrap_or_default();
        let curr_f = self.timeline.current_frame;
        for layer in &self.timeline.layers {
            if !layer.visible {
                continue;
            }
            if let Some(cel) = layer.cel_at(curr_f) {
                let resolved = self
                    .timeline
                    .resolve_shapes_with_symbols(&cel.shapes, curr_f, false);
                backdrop_shapes.extend(resolved.into_iter().filter(|s| {
                    s.symbol_instance
                        .is_none_or(|b| b.instance_gid != binding.instance_gid)
                }));
            }
        }

        // シンボル定義のローカルレイヤー群をインスタンスのステージ変換 `binding.xform` で展開して作業タイムラインを構築
        let mut sym_layers = sym_def.layers.clone();
        transform_layers_shapes(&mut sym_layers, binding.xform);

        let act_l = sym_def.active_layer.min(sym_layers.len().saturating_sub(1));
        let act_f = sym_def
            .current_frame
            .min(sym_def.total_frames.saturating_sub(1));
        let sym_timeline = Timeline {
            layers: sym_layers,
            library: self.timeline.library.clone(),
            next_symbol_id: self.timeline.next_symbol_id,
            selected_library_symbol: Some(binding.symbol_id),
            current_frame: act_f,
            active_layer: act_l,
            total_frames: sym_def.total_frames.max(1),
            fps: self.timeline.fps,
            is_playing: false,
            last_frame_advance_time: None,
            onion_skin_enabled: self.timeline.onion_skin_enabled,
            onion_prev_frames: self.timeline.onion_prev_frames,
            onion_next_frames: self.timeline.onion_next_frames,
            next_layer_number: sym_def.next_layer_number.max(2),
            next_layer_id: sym_def.next_layer_id.max(2),
            scroll_x: 0.0,
            scroll_y: 0.0,
            last_ensured_frame: 0,
            dragging_layer: None,
            cell_selection: Some(TimelineCellSelection::single(act_l, act_f)),
            grid_drag: TimelineGridDrag::None,
        };

        let parent_timeline = std::mem::replace(&mut self.timeline, sym_timeline);
        let parent_editing_group_path = std::mem::take(&mut self.editing_group_path);
        self.symbol_edit_stack.push(SymbolEditSession {
            symbol_id: binding.symbol_id,
            symbol_name: sym_def.name,
            instance_gid: binding.instance_gid,
            instance_xform: binding.xform,
            parent_timeline,
            parent_editing_group_path,
            backdrop_shapes,
        });

        self.load_shapes_from_timeline();
        true
    }

    /// 現在入っているシンボル内部タイムライン編集モードから1段階上の親シーン（または親シンボル）タイムラインへ戻る
    pub(crate) fn exit_symbol_timeline(&mut self) -> bool {
        if self.symbol_edit_stack.is_empty() {
            return false;
        }
        self.flush_canvas_before_timeline_op();

        let session = self.symbol_edit_stack.pop().unwrap();
        let sym_timeline = std::mem::replace(&mut self.timeline, session.parent_timeline);
        self.editing_group_path = session.parent_editing_group_path;
        self.timeline.library = sym_timeline.library.clone();
        self.timeline.next_symbol_id = sym_timeline.next_symbol_id;

        if session.instance_xform.determinant().abs() > 1e-9 {
            let inv_xform = session.instance_xform.inverse();
            if let Some(sym_def) = self
                .timeline
                .library
                .iter_mut()
                .find(|s| s.id == session.symbol_id)
            {
                sym_def.update_from_working_timeline(&sym_timeline, inv_xform);
            }
        }

        // 親タイムライン内の該当シンボルインスタンスの格納図形を最新化
        self.refresh_symbol_instances_in_timeline(session.symbol_id);
        self.load_shapes_from_timeline();

        // 戻った親階層で元のシンボルインスタンスを選択状態にする
        self.selected_shapes = (0..self.shapes.len())
            .filter(|&i| {
                self.shapes[i]
                    .symbol_instance
                    .is_some_and(|b| b.instance_gid == session.instance_gid)
            })
            .collect();
        self.transform_pivot = None;
        true
    }

    /// シンボル内部タイムライン編集中に、現在の `self.timeline`（シンボルタイムライン）の状態を
    /// ライブラリ定義（`SymbolDef`）および背景プレビュー（`backdrop_shapes`）へ同期する
    pub(crate) fn sync_active_symbol_edit(&mut self) {
        let Some(session) = self.symbol_edit_stack.last() else {
            return;
        };
        if session.instance_xform.determinant().abs() <= 1e-9 {
            return;
        }
        let inv_xform = session.instance_xform.inverse();
        let sym_id = session.symbol_id;
        let working_snapshot = self.timeline.clone();

        if let Some(sym_def) = self.timeline.library.iter_mut().find(|s| s.id == sym_id) {
            sym_def.update_from_working_timeline(&working_snapshot, inv_xform);
        }
        if let Some(session_mut) = self.symbol_edit_stack.last_mut() {
            if let Some(parent_sym) = session_mut
                .parent_timeline
                .library
                .iter_mut()
                .find(|s| s.id == sym_id)
            {
                parent_sym.update_from_working_timeline(&working_snapshot, inv_xform);
            }
            let parent_f = session_mut.parent_timeline.current_frame;
            session_mut.backdrop_shapes = session_mut.parent_timeline.resolve_shapes_with_symbols(
                &session_mut.backdrop_shapes,
                parent_f,
                false,
            );
        }
    }

    /// `self.timeline.layers` 内の全キーフレームにある指定シンボル `symbol_id` のインスタンス図形を最新定義で更新する
    pub(crate) fn refresh_symbol_instances_in_timeline(&mut self, symbol_id: u64) {
        let Some(sym_def) = self
            .timeline
            .library
            .iter()
            .find(|s| s.id == symbol_id)
            .cloned()
        else {
            return;
        };
        for layer in &mut self.timeline.layers {
            for kf in &mut layer.keyframes {
                let has_sym = kf.cel.shapes.iter().any(|s| {
                    s.symbol_instance
                        .is_some_and(|b| b.symbol_id == symbol_id)
                });
                if !has_sym {
                    continue;
                }
                let template_shapes = sym_def.evaluated_local_shapes_at(kf.start_frame, false);
                let mut rebuilt = Vec::with_capacity(kf.cel.shapes.len());
                let mut processed_gids: Vec<u64> = Vec::new();
                for shape in kf.cel.shapes.drain(..) {
                    let Some(b) = shape
                        .symbol_instance
                        .filter(|b| b.symbol_id == symbol_id)
                    else {
                        rebuilt.push(shape);
                        continue;
                    };
                    if processed_gids.contains(&b.instance_gid) {
                        continue;
                    }
                    processed_gids.push(b.instance_gid);

                    let parent_scope = b.parent_scope_from_path(&shape.group_path);
                    rebuilt.extend(b.instantiate_shapes(
                        &template_shapes,
                        shape.layer_id,
                        parent_scope,
                    ));
                }
                kf.cel.shapes = rebuilt;
            }
        }
    }
}
