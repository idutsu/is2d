use eframe::egui::{self, Color32, Mesh, Pos2, Stroke, Vec2};
use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::panels::icons;
use crate::timeline::{Cel, Layer, Timeline};
use crate::vector_engine::{
    append_mesh, interpolate_affine_with_pivot, transform_mesh_to_screen,
    transform_mesh_to_screen_onion, FillShape,
};
use crate::FlashBrushApp;

// ============================================================================
// 1. シンボル定義（SymbolDef）・インスタンス紐付け・トゥイーン状態・編集セッション
// ============================================================================

/// シンボルインスタンスのフレーム再生モード（Adobe Animate のグラフィックシンボル相当）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum SymbolLoopMode {
    /// ループ再生：キーフレーム開始位置からシンボル内部フレームを繰り返し再生する
    #[default]
    Loop,
    /// 1回再生：キーフレーム開始位置からシンボル内部の最終フレームまで再生して停止する
    PlayOnce,
    /// 任意のフレーム：指定された `first_frame` の1コマだけを固定表示する
    SingleFrame,
}

/// シンボルインスタンスに属するシェイプが持つ紐付け情報（インスタンスのグループID、ライブラリ内のシンボル定義ID、ローカル基準点 `(0, 0)` からワールド座標へのアフィン変換、ローカル変形中心ピボット、再生モード、開始/指定フレーム）
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SymbolInstanceBinding {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    pub(crate) xform: Affine,
    /// シンボルローカル座標系での変形中心点（ピボット）。初期値は `Point::ZERO` `(0.0, 0.0)`
    pub(crate) pivot: Point,
    pub(crate) loop_mode: SymbolLoopMode,
    pub(crate) first_frame: usize,
}

impl SymbolInstanceBinding {
    #[inline]
    pub(crate) fn new(instance_gid: u64, symbol_id: u64, xform: Affine) -> Self {
        Self {
            instance_gid,
            symbol_id,
            xform,
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
        }
    }

    /// このインスタンスの現在のワールド座標における変形中心点（ピボット）を返す
    #[inline]
    pub(crate) fn world_pivot(self) -> Point {
        self.xform * self.pivot
    }
}

/// キーフレーム区間におけるクラシックトゥイーン（`T` キー）の有効状態
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TweenState {
    /// トゥイーン未設定（または開始キーフレームが単一シンボル条件を満たしていない）
    None,
    /// 開始キーフレームは単一シンボルでトゥイーンONだが、終了キーフレームが存在しない（または条件不一致）→ 横点線（`----`）表示
    Incomplete,
    /// 開始・終了キーフレームとも同一シンボルの単一インスタンスが存在する完全トゥイーン → 実線矢印（`───▶`）表示＆TRS補間
    Complete {
        start_frame: usize,
        end_frame: usize,
        start_binding: SymbolInstanceBinding,
        end_binding: SymbolInstanceBinding,
    },
}

impl Cel {
    /// このセル（キーフレーム）に「ルート直下の単一シンボルインスタンスのみ（1個だけ）」が存在する場合、
    /// その `SymbolInstanceBinding` を返す。
    /// （空セル、生シェイプのみ、通常グループのみ、複数シンボル、シンボル＋生シェイプ、シンボル＋グループ等の場合はすべて `None`）
    pub(crate) fn single_symbol_instance(&self) -> Option<SymbolInstanceBinding> {
        let first = self.shapes.first()?;
        let binding = first.symbol_instance?;
        if first.group_path.first() != Some(&binding.instance_gid) {
            return None;
        }
        let all_same_instance = self.shapes.iter().all(|s| {
            s.symbol_instance.is_some_and(|b| {
                b.instance_gid == binding.instance_gid && b.symbol_id == binding.symbol_id
            }) && s.group_path.first() == Some(&binding.instance_gid)
        });
        all_same_instance.then_some(binding)
    }

    /// セル内の図形群のうち、`map_binding` が `Some(updated_binding)` を返す対象シンボルインスタンスを
    /// シンボル定義 `sym_def`（開始フレーム `kf_start_frame`）で再展開して更新する
    pub(crate) fn rebuild_symbol_instances_with(
        &mut self,
        kf_start_frame: usize,
        sym_def: &SymbolDef,
        mut map_binding: impl FnMut(SymbolInstanceBinding) -> Option<SymbolInstanceBinding>,
    ) -> bool {
        let mut rebuilt = Vec::with_capacity(self.shapes.len());
        let mut processed_gids: Vec<u64> = Vec::new();
        let mut any_updated = false;

        for shape in self.shapes.drain(..) {
            let Some(orig_b) = shape.symbol_instance else {
                rebuilt.push(shape);
                continue;
            };
            let Some(new_b) = map_binding(orig_b) else {
                rebuilt.push(shape);
                continue;
            };
            if processed_gids.contains(&orig_b.instance_gid) {
                continue;
            }
            processed_gids.push(orig_b.instance_gid);

            let parent_scope = new_b.parent_scope_from_path(&shape.group_path);
            let template_shapes = sym_def.evaluated_local_shapes_for_instance(
                new_b,
                kf_start_frame,
                kf_start_frame,
                false,
            );
            rebuilt.extend(new_b.instantiate_shapes(&template_shapes, shape.layer_id, parent_scope));
            any_updated = true;
        }
        self.shapes = rebuilt;
        any_updated
    }
}

impl Layer {
    /// `kf_idx` 番目のキーフレームにおけるトゥイーン状態（未設定 / 点線の未完トゥイーン / 実線矢印の完全トゥイーン）を判定する
    pub(crate) fn tween_state_at_keyframe(&self, kf_idx: usize) -> TweenState {
        let Some(kf) = self.keyframes.get(kf_idx) else {
            return TweenState::None;
        };
        if !kf.tween {
            return TweenState::None;
        }
        let Some(start_binding) = kf.cel.single_symbol_instance() else {
            return TweenState::None;
        };
        if let Some(next_kf) = self
            .keyframes
            .get(kf_idx + 1)
            .filter(|next| next.start_frame < self.frame_count && next.start_frame > kf.start_frame)
        {
            if let Some(end_binding) = next_kf
                .cel
                .single_symbol_instance()
                .filter(|b| b.symbol_id == start_binding.symbol_id)
            {
                return TweenState::Complete {
                    start_frame: kf.start_frame,
                    end_frame: next_kf.start_frame,
                    start_binding,
                    end_binding,
                };
            }
        }
        TweenState::Incomplete
    }

    /// 指定フレーム `frame` が完全トゥイーン（`TweenState::Complete`）の中間フレーム（`start_frame < frame < end_frame`）である場合、
    /// TRS補間後の `(開始バインディング, 補間後バインディング)` を返す
    pub(crate) fn interpolated_tween_binding_at(
        &self,
        frame: usize,
    ) -> Option<(SymbolInstanceBinding, SymbolInstanceBinding)> {
        let kf_idx = self.keyframe_index_at(frame)?;
        match self.tween_state_at_keyframe(kf_idx) {
            TweenState::Complete {
                start_frame,
                end_frame,
                start_binding,
                end_binding,
            } if frame > start_frame && frame < end_frame => {
                let t = (frame - start_frame) as f64 / (end_frame - start_frame) as f64;
                let (interp_xform, interp_pivot) = interpolate_affine_with_pivot(
                    start_binding.xform,
                    start_binding.pivot,
                    end_binding.xform,
                    end_binding.pivot,
                    t,
                );
                Some((
                    start_binding,
                    SymbolInstanceBinding {
                        instance_gid: start_binding.instance_gid,
                        symbol_id: start_binding.symbol_id,
                        xform: interp_xform,
                        pivot: interp_pivot,
                        loop_mode: start_binding.loop_mode,
                        first_frame: start_binding.first_frame,
                    },
                ))
            }
            _ => None,
        }
    }
}

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

/// レイヤー配列内の全キーフレーム・全シェイプに対してアフィン変換 `affine` を適用する
fn transform_layers_shapes(layers: &mut [Layer], affine: Affine) {
    for layer in layers {
        for kf in &mut layer.keyframes {
            for s in &mut kf.cel.shapes {
                s.transform(affine);
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
                let delta_xform = layer
                    .interpolated_tween_binding_at(frame)
                    .and_then(|(start_b, interp_b)| {
                        (start_b.xform.determinant().abs() > 1e-9)
                            .then(|| interp_b.xform * start_b.xform.inverse())
                    });
                for s in &cel.shapes {
                    let mut cloned = s.clone();
                    if let Some(dx) = delta_xform {
                        cloned.transform(dx);
                    }
                    cloned.layer_id = 1;
                    out.push(cloned);
                }
            }
        }
        out
    }

    /// インスタンスの再生設定（`binding.loop_mode`, `binding.first_frame`）とキーフレーム開始フレーム `kf_start_frame`、
    /// 現在の親フレーム `current_frame` から、評価すべきシンボル内部フレーム番号（`0..total_frames`）を算出する
    pub(crate) fn resolve_internal_frame(
        &self,
        binding: SymbolInstanceBinding,
        kf_start_frame: usize,
        current_frame: usize,
    ) -> usize {
        let fc = self.total_frames.max(1);
        let base = binding.first_frame.min(fc - 1);
        let elapsed = current_frame.saturating_sub(kf_start_frame);
        match binding.loop_mode {
            SymbolLoopMode::Loop => (base + elapsed) % fc,
            SymbolLoopMode::PlayOnce => (base + elapsed).min(fc - 1),
            SymbolLoopMode::SingleFrame => base,
        }
    }

    /// インスタンスの再生設定（ループ再生 / 1回再生 / 任意のフレーム）に基づいて、
    /// 指定フレーム `current_frame`（開始キーフレーム `kf_start_frame`）でのローカル図形列を返す
    pub(crate) fn evaluated_local_shapes_for_instance(
        &self,
        binding: SymbolInstanceBinding,
        kf_start_frame: usize,
        current_frame: usize,
        allow_empty: bool,
    ) -> Vec<FillShape> {
        let internal_frame = self.resolve_internal_frame(binding, kf_start_frame, current_frame);
        self.evaluated_local_shapes_at(internal_frame, allow_empty)
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

    /// 指定フレーム `frame`（開始キーフレーム `kf_start_frame`）において、各シンボルインスタンスの内部タイムラインを
    /// インスタンスごとの再生モード（ループ再生 / 1回再生 / 任意のフレーム）で評価して展開した図形列を返す
    pub(crate) fn resolve_shapes_with_symbols(
        &self,
        raw_shapes: &[FillShape],
        kf_start_frame: usize,
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
            let local_shapes =
                sym.evaluated_local_shapes_for_instance(b, kf_start_frame, frame, allow_empty);
            out.extend(b.instantiate_shapes(&local_shapes, shape.layer_id, parent_scope));
        }
        out
    }

    /// 指定レイヤー `layer_idx` の指定フレーム `frame` における図形列を、クラシックトゥイーン補間およびシンボル内タイムライン評価込みで返す
    pub(crate) fn evaluate_layer_shapes_at(
        &self,
        layer_idx: usize,
        frame: usize,
        allow_empty: bool,
    ) -> Vec<FillShape> {
        let Some(layer) = self.layers.get(layer_idx) else {
            return Vec::new();
        };
        let Some(kf_idx) = layer.keyframe_index_at(frame) else {
            return Vec::new();
        };
        let kf = &layer.keyframes[kf_idx];
        let cel = &kf.cel;
        let kf_start_frame = kf.start_frame;
        let lid = layer.id;

        if let Some((start_b, interp_b)) = layer.interpolated_tween_binding_at(frame) {
            if let Some(sym) = self.library.iter().find(|s| s.id == interp_b.symbol_id) {
                let local_shapes = sym.evaluated_local_shapes_for_instance(
                    interp_b,
                    kf_start_frame,
                    frame,
                    allow_empty,
                );
                return interp_b.instantiate_shapes(&local_shapes, lid, &[]);
            }
            let delta = if start_b.xform.determinant().abs() > 1e-9 {
                interp_b.xform * start_b.xform.inverse()
            } else {
                Affine::IDENTITY
            };
            return cel
                .shapes
                .iter()
                .map(|s| {
                    let mut c = s.clone().with_layer_id(lid);
                    c.transform(delta);
                    c.symbol_instance = Some(interp_b);
                    c
                })
                .collect();
        }

        self.resolve_shapes_with_symbols(&cel.shapes, kf_start_frame, frame, allow_empty)
            .into_iter()
            .map(|s| s.with_layer_id(lid))
            .collect()
    }

    /// 指定レイヤー・フレームの結合済みワールド座標メッシュを（トゥイーン補間・複数フレームシンボル評価込みで）構築する
    fn build_layer_resolved_world_batch_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
    ) -> Option<Mesh> {
        let resolved = self.evaluate_layer_shapes_at(layer_idx, frame, true);
        if resolved.is_empty() {
            return None;
        }
        let mut batch = Mesh::default();
        for s in &resolved {
            append_mesh(&mut batch, &s.fill_mesh);
        }
        Some(batch)
    }

    /// 指定レイヤー・フレームの描画用スクリーン `Mesh` を返す（トゥイーン中間フレームまたは複数フレームシンボルを含む場合はフレーム評価を行い、それ以外は高速キャッシュを返す）
    pub(crate) fn resolved_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
    ) -> Option<Mesh> {
        let layer = self.layers.get(layer_idx)?;
        let cel = layer.cel_at(frame)?;
        if cel.is_empty() {
            return None;
        }
        if layer.interpolated_tween_binding_at(frame).is_none()
            && !self.has_multiframe_symbol(&cel.shapes)
        {
            return Some(cel.cached_screen_mesh(zoom, pan, dim_backdrop));
        }
        let batch = self.build_layer_resolved_world_batch_mesh(layer_idx, frame)?;
        Some(transform_mesh_to_screen(&batch, zoom, pan, dim_backdrop))
    }

    /// 指定レイヤー・フレームのオニオンスキン描画用スクリーン `Mesh` を返す（トゥイーン補間・複数フレームシンボル対応）
    pub(crate) fn resolved_cel_onion_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        tint_rgb: [u8; 3],
        alpha: u8,
    ) -> Option<Mesh> {
        let layer = self.layers.get(layer_idx)?;
        let cel = layer.cel_at(frame)?;
        if cel.is_empty() {
            return None;
        }
        if layer.interpolated_tween_binding_at(frame).is_none()
            && !self.has_multiframe_symbol(&cel.shapes)
        {
            return Some(cel.cached_onion_mesh(zoom, pan, tint_rgb, alpha));
        }
        let batch = self.build_layer_resolved_world_batch_mesh(layer_idx, frame)?;
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

        let target_lid = self.topmost_layer_id_for_indices(&valid_indices);
        let binding = SymbolInstanceBinding::new(
            inst_gid,
            sym_id,
            Affine::translate(KVec2::new(center.x, center.y)),
        );
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
            .map(|s| s.evaluated_local_shapes_at(0, false))
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
        let stage_center = self.stage_center_world();
        let binding = SymbolInstanceBinding::new(
            inst_gid,
            symbol_id,
            Affine::translate(KVec2::new(stage_center.x, stage_center.y)),
        );
        let target_lid = self.active_scope_layer_id();
        let placed = binding.instantiate_shapes(&sym_shapes, target_lid, &self.editing_group_path);
        self.insert_and_select_shapes_in_scope(placed, false);
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
                if kf.tween && kf.cel.single_symbol_instance().is_none() {
                    kf.tween = false;
                }
            }
        }
        self.sanitize_editing_group_path();
        true
    }

    /// 現在のフレーム・選択レイヤーにおいてクラシックトゥイーンの切替が可能か `(切替可能か, 現在トゥイーンONか)` を返す
    pub(crate) fn can_toggle_tween_at_current_frame(&self) -> (bool, bool) {
        let targets = self.timeline.target_layers_for_frame_op();
        if targets.is_empty() {
            return (false, false);
        }
        let curr_f = self.timeline.current_frame;
        let mut any_tweened = false;
        let mut any_can_enable = false;
        for &l_idx in &targets {
            let layer = &self.timeline.layers[l_idx];
            if let Some(kf_idx) = layer.keyframe_index_at(curr_f) {
                let kf = &layer.keyframes[kf_idx];
                if kf.tween && kf.cel.single_symbol_instance().is_some() {
                    any_tweened = true;
                } else if kf.cel.single_symbol_instance().is_some() {
                    any_can_enable = true;
                }
            }
        }
        (any_tweened || any_can_enable, any_tweened)
    }

    /// 現在のキーフレーム（または保持フレームが属する開始キーフレーム）に対してクラシックトゥイーンを ON / OFF トグルする（`T` キー）。
    /// トゥイーンを ON にするための厳格条件：開始キーフレームに「シンボルインスタンスが1つだけ」存在すること。
    pub(crate) fn timeline_toggle_tween(&mut self) -> bool {
        let targets = self.timeline.target_layers_for_frame_op();
        if targets.is_empty() {
            return false;
        }
        let prev_selected = self.selected_shapes.clone();
        self.flush_canvas_before_timeline_op();

        let (can_toggle, is_currently_tweened) = self.can_toggle_tween_at_current_frame();
        if !can_toggle {
            self.selected_shapes = prev_selected
                .into_iter()
                .filter(|&idx| idx < self.shapes.len())
                .collect();
            self.notify_warning(
                "トゥイーンを設定するには、キーフレームに単一のシンボルインスタンス（F8）のみを配置してください",
            );
            return false;
        }

        self.push_undo();
        let curr_f = self.timeline.current_frame;
        let next_tween = !is_currently_tweened;
        let mut changed = false;

        for &l_idx in &targets {
            let layer = &mut self.timeline.layers[l_idx];
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
            self.load_shapes_from_timeline();
            self.select_shapes_for_timeline_selection();
        }
        changed
    }

    /// 現在選択されている図形が単一のシンボルインスタンスである場合、その `(SymbolInstanceBinding, シンボル名, シンボル総フレーム数)` を返す
    pub(crate) fn selected_single_symbol_info(
        &self,
    ) -> Option<(SymbolInstanceBinding, String, usize)> {
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
        let sym = self
            .timeline
            .library
            .iter()
            .find(|sym| sym.id == first_binding.symbol_id)?;
        Some((first_binding, sym.name.clone(), sym.total_frames.max(1)))
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

    /// 現在選択されている単一シンボルインスタンスの再生モード（ループ再生 / 1回再生 / 任意のフレーム）および
    /// 開始/指定フレーム番号（0始まり）を更新し、現在フレームのキャンバス表示と選択状態を即時反映する
    pub(crate) fn set_selected_symbol_playback(
        &mut self,
        loop_mode: SymbolLoopMode,
        first_frame: usize,
    ) -> bool {
        let Some((binding, _, total_frames)) = self.selected_single_symbol_info() else {
            return false;
        };
        let clamped_first = first_frame.min(total_frames.saturating_sub(1));
        if binding.loop_mode == loop_mode && binding.first_frame == clamped_first {
            return false;
        }

        let inst_gid = binding.instance_gid;
        let sym_id = binding.symbol_id;
        self.clear_selection();
        self.sync_shapes_to_timeline();
        self.push_undo();

        let Some(sym_def) = self
            .timeline
            .library
            .iter()
            .find(|s| s.id == sym_id)
            .cloned()
        else {
            return false;
        };

        let curr_f = self.timeline.current_frame;
        let mut updated = false;
        for layer in &mut self.timeline.layers {
            if !layer.visible || layer.locked {
                continue;
            }
            let Some(kf_idx) = layer.keyframe_index_at(curr_f) else {
                continue;
            };
            let kf = &mut layer.keyframes[kf_idx];
            if kf
                .cel
                .rebuild_symbol_instances_with(kf.start_frame, &sym_def, |mut b| {
                    (b.instance_gid == inst_gid).then(|| {
                        b.loop_mode = loop_mode;
                        b.first_frame = clamped_first;
                        b
                    })
                })
            {
                updated = true;
            }
        }

        if updated {
            self.sync_active_symbol_edit();
            self.load_shapes_from_timeline();
            self.select_symbol_instance_by_gid(inst_gid);
        }
        updated
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
        for layer_idx in 0..self.timeline.layers.len() {
            if !self.timeline.layers[layer_idx].visible {
                continue;
            }
            let resolved = self.timeline.evaluate_layer_shapes_at(layer_idx, curr_f, false);
            backdrop_shapes.extend(resolved.into_iter().filter(|s| {
                s.symbol_instance
                    .is_none_or(|b| b.instance_gid != binding.instance_gid)
            }));
        }

        // シンボル定義のローカルレイヤー群をインスタンスのステージ変換 `binding.xform` で展開して作業タイムラインを構築
        let mut sym_layers = sym_def.layers.clone();
        transform_layers_shapes(&mut sym_layers, binding.xform);
        let sym_timeline = self.timeline.new_for_symbol_edit(&sym_def, sym_layers);

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
        self.select_symbol_instance_by_gid(session.instance_gid);
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
                0,
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
                kf.cel
                    .rebuild_symbol_instances_with(kf.start_frame, &sym_def, |b| {
                        (b.symbol_id == symbol_id).then_some(b)
                    });
            }
        }
    }

    // ============================================================================
    // 4. シンボルプロパティ・ライブラリパネルUI描画
    // ============================================================================

    /// 単一のシンボルインスタンスが選択されているときにプロパティパネルに表示する再生オプション（1回再生 / ループ再生 / 任意のフレーム）
    pub(crate) fn draw_selected_symbol_properties(&mut self, ui: &mut egui::Ui) {
        let Some((binding, sym_name, sym_total_frames)) = self.selected_single_symbol_info() else {
            return;
        };

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{} {sym_name}", icons::SYMBOL))
                    .size(11.5)
                    .strong()
                    .color(Color32::from_rgb(0, 120, 215)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("全 {sym_total_frames} f"))
                        .size(10.5)
                        .color(Color32::from_rgb(110, 118, 130)),
                );
            });
        });

        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("シンボル再生オプション")
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);

        let mut next_mode = binding.loop_mode;
        let max_first_1based = sym_total_frames.max(1);
        let current_first_1based = (binding.first_frame + 1).clamp(1, max_first_1based);
        let mut next_first_1based = current_first_1based;

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let half_w = (ui.available_width() - 6.0) * 0.5;
            if ui
                .add_sized(
                    Vec2::new(half_w, 26.0),
                    egui::Button::new("ループ再生")
                        .selected(binding.loop_mode == SymbolLoopMode::Loop),
                )
                .on_hover_text("キーフレーム開始位置からシンボル内部フレームをループ再生します")
                .clicked()
            {
                next_mode = SymbolLoopMode::Loop;
            }
            if ui
                .add_sized(
                    Vec2::new(half_w, 26.0),
                    egui::Button::new("1回再生")
                        .selected(binding.loop_mode == SymbolLoopMode::PlayOnce),
                )
                .on_hover_text(
                    "キーフレーム開始位置からシンボル内部の最終フレームまで1回再生して停止します",
                )
                .clicked()
            {
                next_mode = SymbolLoopMode::PlayOnce;
            }
        });

        ui.add_space(4.0);

        if ui
            .add_sized(
                Vec2::new(ui.available_width(), 26.0),
                egui::Button::new("任意のフレーム")
                    .selected(binding.loop_mode == SymbolLoopMode::SingleFrame),
            )
            .on_hover_text("指定したシンボル内部フレーム（1コマ）を固定表示します")
            .clicked()
        {
            next_mode = SymbolLoopMode::SingleFrame;
        }

        ui.add_space(6.0);

        let frame_label = match next_mode {
            SymbolLoopMode::SingleFrame => "表示フレーム:",
            SymbolLoopMode::Loop | SymbolLoopMode::PlayOnce => "開始フレーム:",
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                egui::RichText::new(frame_label)
                    .size(11.0)
                    .color(Color32::from_rgb(55, 62, 74)),
            );
            if ui
                .add_enabled(
                    next_first_1based > 1,
                    egui::Button::new(icons::STEP_PREV).min_size(Vec2::new(22.0, 22.0)),
                )
                .on_hover_text("前のシンボル内フレーム")
                .clicked()
            {
                next_first_1based = next_first_1based.saturating_sub(1).max(1);
            }
            ui.add_sized(
                Vec2::new(56.0, 22.0),
                egui::DragValue::new(&mut next_first_1based)
                    .range(1..=max_first_1based)
                    .speed(0.15)
                    .suffix(" f"),
            );
            if ui
                .add_enabled(
                    next_first_1based < max_first_1based,
                    egui::Button::new(icons::STEP_NEXT).min_size(Vec2::new(22.0, 22.0)),
                )
                .on_hover_text("次のシンボル内フレーム")
                .clicked()
            {
                next_first_1based = (next_first_1based + 1).min(max_first_1based);
            }
        });

        if next_mode != binding.loop_mode || next_first_1based != current_first_1based {
            self.set_selected_symbol_playback(next_mode, next_first_1based.saturating_sub(1));
        }

        ui.add_space(6.0);
        if ui
            .add_sized(
                Vec2::new(ui.available_width(), 26.0),
                egui::Button::new(format!("{} シンボル編集に入る", icons::SYMBOL)),
            )
            .on_hover_text("ダブルクリックと同様にシンボル内部のタイムライン編集モードへ入ります")
            .clicked()
        {
            self.enter_symbol_timeline(binding);
        }
    }

    /// 「ライブラリ」タブの描画（シンボルのプレビュー・一覧・名前変更・ステージ配置・削除）
    pub(crate) fn draw_library_tab(&mut self, ui: &mut egui::Ui) {
        let has_sel = !self.selected_shapes.is_empty();
        if ui
            .add_enabled(
                has_sel,
                egui::Button::new(format!("{} 選択図形をシンボル化 (F8)", icons::ADD))
                    .min_size(Vec2::new(ui.available_width(), 28.0)),
            )
            .on_hover_text("キャンバスで選択中の図形を新しいシンボルとして登録 (F8)")
            .clicked()
        {
            self.convert_selection_to_symbol();
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        if self.timeline.library.is_empty() {
            ui.label(
                egui::RichText::new(
                    "シンボルがありません。\n図形を選択して F8 キーを押すとシンボルを作成できます。",
                )
                .size(11.5)
                .color(Color32::from_rgb(115, 122, 135)),
            );
            return;
        }

        if self.timeline.selected_library_symbol.is_none()
            || !self
                .timeline
                .library
                .iter()
                .any(|s| Some(s.id) == self.timeline.selected_library_symbol)
        {
            self.timeline.selected_library_symbol = self.timeline.library.first().map(|s| s.id);
        }

        // 選択中のシンボルのミニプレビュー描画領域
        if let Some(sel_id) = self.timeline.selected_library_symbol {
            if let Some(sym) = self.timeline.library.iter().find(|s| s.id == sel_id) {
                let preview_shapes = sym.evaluated_local_shapes_at(sym.current_frame, false);
                self.draw_symbol_mini_preview(ui, &preview_shapes);
                ui.add_space(6.0);
            }

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let place_w = ui.available_width() - 66.0;
                if ui
                    .add_sized(
                        Vec2::new(place_w.max(80.0), 28.0),
                        egui::Button::new(format!("{} ステージに配置", icons::ADD)),
                    )
                    .on_hover_text("選択中のシンボルを現在のフレーム・レイヤーに配置します")
                    .clicked()
                {
                    self.place_symbol_instance_from_library(sel_id);
                }
                if ui
                    .add_sized(
                        Vec2::new(60.0, 28.0),
                        egui::Button::new(format!("{} 削除", icons::DELETE)),
                    )
                    .on_hover_text("選択中のシンボルをライブラリから削除します")
                    .clicked()
                {
                    self.delete_symbol_from_library(sel_id);
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
        }

        ui.label(
            egui::RichText::new(format!("登録シンボル一覧 ({})", self.timeline.library.len()))
                .size(11.5)
                .strong()
                .color(Color32::from_rgb(55, 62, 74)),
        );
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for sym in &mut self.timeline.library {
                    let is_sel = self.timeline.selected_library_symbol == Some(sym.id);
                    let row_fill = if is_sel {
                        Color32::from_rgb(220, 234, 252)
                    } else {
                        Color32::from_rgb(252, 253, 255)
                    };
                    egui::Frame::none()
                        .fill(row_fill)
                        .stroke(Stroke::new(
                            1.0,
                            if is_sel {
                                Color32::from_rgb(0, 125, 230)
                            } else {
                                Color32::from_rgb(218, 222, 230)
                            },
                        ))
                        .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .selectable_label(
                                        is_sel,
                                        egui::RichText::new(icons::SYMBOL)
                                            .size(15.0)
                                            .color(Color32::from_rgb(0, 130, 220)),
                                    )
                                    .on_hover_text("クリックして選択")
                                    .clicked()
                                {
                                    self.timeline.selected_library_symbol = Some(sym.id);
                                }
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut sym.name)
                                        .desired_width(ui.available_width()),
                                );
                                if resp.clicked() || resp.gained_focus() {
                                    self.timeline.selected_library_symbol = Some(sym.id);
                                }
                            });
                        });
                    ui.add_space(3.0);
                }
            });
    }

    /// ライブラリパネル上部にシンボルの縮小ベクタープレビューを描画する
    fn draw_symbol_mini_preview(&self, ui: &mut egui::Ui, shapes: &[FillShape]) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(252, 252, 254));
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_rgb(206, 212, 222)));

        if shapes.is_empty() {
            return;
        }
        let mut bbox = shapes[0].bounding_box();
        for s in &shapes[1..] {
            bbox = bbox.union(s.bounding_box());
        }
        let w = (bbox.x1 - bbox.x0).max(1.0) as f32;
        let h = (bbox.y1 - bbox.y0).max(1.0) as f32;
        let pad = 14.0;
        let avail_w = (rect.width() - pad * 2.0).max(10.0);
        let avail_h = (rect.height() - pad * 2.0).max(10.0);
        let scale = (avail_w / w).min(avail_h / h).clamp(0.05, 4.0);
        let cx = ((bbox.x0 + bbox.x1) * 0.5) as f32;
        let cy = ((bbox.y0 + bbox.y1) * 0.5) as f32;
        let pan = rect.center().to_vec2() - Vec2::new(cx * scale, cy * scale);

        for s in shapes {
            let mesh = transform_mesh_to_screen(&s.fill_mesh, scale, pan, false);
            painter.add(egui::Shape::mesh(mesh));
        }

        // 中心基準点 `+` マーク
        let origin_scr = Pos2::new(pan.x, pan.y);
        if rect.contains(origin_scr) {
            draw_registration_cross(&painter, origin_scr, 4.0, false);
        }
    }
}
