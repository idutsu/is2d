use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, Mesh, Vec2};
use kurbo::{Affine, Point};

#[cfg(test)]
pub(crate) use crate::timeline::interpolate_affine_with_pivot;
pub(crate) use crate::timeline::{ResolvedCelSig, SymbolInstance, TweenRotate, TweenState};
use crate::timeline::{lru_get, lru_insert, Cel, CelMeshCache, Layer, Timeline};
use crate::vector_engine::{
    append_mesh, next_shape_id, transform_mesh_to_screen, transform_mesh_to_screen_onion,
    union_bounding_boxes, Contour, FillShape, SyncCache,
};
pub(crate) use crate::vector_engine::{SymbolInstanceBinding, SymbolLoopMode};

// ============================================================================
// 1. シンボル定義（SymbolDef）・インスタンス紐付け・トゥイーン状態・編集セッション
// ============================================================================

impl SymbolInstanceBinding {
    /// このインスタンスの現在のワールド座標における変形中心点（ピボット）を返す
    #[inline]
    pub(crate) fn world_pivot(self) -> Point {
        self.xform * self.pivot
    }

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
            let _ = s.fill_mesh();
            let mut inst_s = s.clone();
            inst_s.shape_id = next_shape_id();
            let mut full_path =
                Vec::with_capacity(scope_prefix.len() + 1 + inst_s.group_path.len());
            full_path.extend_from_slice(scope_prefix);
            full_path.push(self.instance_gid);
            full_path.extend(inst_s.group_path.iter().copied());
            inst_s.transform_with_cached_mesh(self.xform);
            inst_s.layer_id = layer_id;
            inst_s.group_path = full_path;
            inst_s.symbol_instance = Some(self);
            out.push(inst_s);
        }
        out
    }
}

impl FillShape {
    /// 現在フレームで内部図形が空白（0シェイプ）のシンボルインスタンスを、
    /// `CanvasState::shapes` 内で他の図形と同一の一元配列上で管理するための非表示マーカー要素を生成する
    pub(crate) fn new_empty_symbol_marker(
        binding: SymbolInstanceBinding,
        layer_id: u64,
        parent_scope: &[u64],
    ) -> Self {
        let mut group_path = Vec::with_capacity(parent_scope.len() + 1);
        group_path.extend_from_slice(parent_scope);
        group_path.push(binding.instance_gid);
        let mut shape = Self::new(Color32::TRANSPARENT, Contour::new(Vec::new()), Vec::new())
            .with_scope(layer_id, &group_path);
        shape.symbol_instance = Some(binding);
        shape
    }
}

/// シンボル定義のプレビュー用ローカルキャッシュ（評価済み外接矩形・ワールド座標結合メッシュ・スクリーン変換済みメッシュ）
#[derive(Clone, Debug, Default)]
pub(crate) struct SymbolPreviewCache {
    key: (u64, u64, usize),
    local_bounds_and_mesh: Option<(kurbo::Rect, egui::Mesh)>,
    screen_mesh: Option<([u32; 3], Arc<egui::Mesh>)>,
}

/// ライブラリに登録されたシンボル定義（独自の `Timeline` とローカル基準点 `(0, 0)` を持つ）
#[derive(Clone, Debug)]
pub(crate) struct SymbolDef {
    pub(crate) id: u64,
    pub(crate) name: String,
    /// シンボル内部の独自タイムライン（各シェイプは基準点 `(0, 0)` 中心のローカル座標系）
    pub(crate) timeline: Timeline,
    /// シンボル定義の変更世代番号（解決済みセルメッシュキャッシュの署名計算用）
    pub(crate) revision: u64,
    mini_preview_cache: SyncCache<Option<SymbolPreviewCache>>,
    dynamic_anim_cache: SyncCache<Option<(u64, u64, bool)>>,
}

pub(crate) const MAX_SYMBOL_NEST_DEPTH: usize = 8;
pub(crate) const MAX_EXPANDED_SYMBOL_SHAPES: usize = 10_000;
pub(crate) const MAX_SYMBOL_EXPANSION_STEPS: usize = 10_000;

type SymbolEvalMemoKey = (u64, usize, usize, usize);

#[derive(Default)]
struct SymbolEvalState {
    steps_used: usize,
    truncated: bool,
    memo: HashMap<SymbolEvalMemoKey, Arc<Vec<FillShape>>>,
}

/// シンボル再帰評価時の設定・メモ化・展開上限コンテキスト
#[derive(Clone)]
pub(crate) struct SymbolEvalCtx<'a> {
    pub(crate) library: &'a [SymbolDef],
    pub(crate) depth: usize,
    state: std::rc::Rc<std::cell::RefCell<SymbolEvalState>>,
}

impl<'a> SymbolEvalCtx<'a> {
    pub(crate) fn new(library: &'a [SymbolDef]) -> Self {
        Self {
            library,
            depth: 0,
            state: std::rc::Rc::new(std::cell::RefCell::new(SymbolEvalState::default())),
        }
    }

    pub(crate) fn nested(&self) -> Self {
        Self {
            library: self.library,
            depth: self.depth + 1,
            state: std::rc::Rc::clone(&self.state),
        }
    }

    #[inline]
    pub(crate) fn has_remaining_steps(&self) -> bool {
        self.state.borrow().steps_used < MAX_SYMBOL_EXPANSION_STEPS
    }

    #[inline]
    pub(crate) fn is_truncated(&self) -> bool {
        self.state.borrow().truncated
    }

    #[inline]
    fn mark_truncated(&self) {
        self.state.borrow_mut().truncated = true;
    }

    #[inline]
    fn try_consume_step(&self) -> bool {
        let mut st = self.state.borrow_mut();
        if st.steps_used >= MAX_SYMBOL_EXPANSION_STEPS {
            st.truncated = true;
            return false;
        }
        st.steps_used += 1;
        true
    }

    #[inline]
    fn get_memo(&self, key: &SymbolEvalMemoKey) -> Option<Arc<Vec<FillShape>>> {
        self.state.borrow().memo.get(key).cloned()
    }

    #[inline]
    fn insert_memo(&self, key: SymbolEvalMemoKey, shapes: Arc<Vec<FillShape>>) {
        self.state.borrow_mut().memo.insert(key, shapes);
    }
}

/// 入れ子シンボル対応で正規化セル（`cel.shapes`＝非シンボル図形群、`cel.instances`＝シンボルインスタンス群）を
/// 背面→前面の `shape_order_index` 順に再帰展開して返す共通ヘルパー
pub(crate) fn resolve_cel_with_library(
    cel: &Cel,
    kf_start_frame: usize,
    frame: usize,
    ctx: SymbolEvalCtx<'_>,
) -> Vec<FillShape> {
    if ctx.depth > 0 {
        for s in cel.shapes.iter() {
            let _ = s.fill_mesh();
        }
    }
    if ctx.depth >= MAX_SYMBOL_NEST_DEPTH || cel.instances.is_empty() || !ctx.has_remaining_steps() {
        if !cel.instances.is_empty() && !ctx.has_remaining_steps() {
            ctx.mark_truncated();
        }
        return (*cel.shapes).clone();
    }

    let mut out = Vec::with_capacity(cel.shapes.len() + cel.instances.len());
    let mut next_cel_idx = 0;

    for inst in cel.instances.iter() {
        let target_idx = inst.shape_order_index.min(cel.shapes.len());
        while next_cel_idx < target_idx && out.len() < MAX_EXPANDED_SYMBOL_SHAPES {
            out.push(cel.shapes[next_cel_idx].clone());
            next_cel_idx += 1;
        }
        if out.len() >= MAX_EXPANDED_SYMBOL_SHAPES || !ctx.has_remaining_steps() {
            ctx.mark_truncated();
            break;
        }
        let b = inst.binding;
        let Some(sym) = ctx.library.iter().find(|sym| sym.id == b.symbol_id) else {
            continue;
        };
        let local_shapes =
            sym.evaluated_local_shapes_for_instance(b, kf_start_frame, frame, ctx.nested());
        if local_shapes.is_empty() && ctx.depth == 0 {
            out.push(FillShape::new_empty_symbol_marker(
                b,
                inst.layer_id,
                &inst.parent_scope,
            ));
        } else {
            let remaining = MAX_EXPANDED_SYMBOL_SHAPES.saturating_sub(out.len());
            if local_shapes.len() > remaining {
                ctx.mark_truncated();
            }
            out.extend(
                b.instantiate_shapes(
                    &local_shapes[..local_shapes.len().min(remaining)],
                    inst.layer_id,
                    &inst.parent_scope,
                ),
            );
        }
    }

    while next_cel_idx < cel.shapes.len() && out.len() < MAX_EXPANDED_SYMBOL_SHAPES {
        out.push(cel.shapes[next_cel_idx].clone());
        next_cel_idx += 1;
    }
    if next_cel_idx < cel.shapes.len() {
        ctx.mark_truncated();
    }

    out
}

impl SymbolDef {
    pub(crate) fn new(id: u64, name: String, mut local_shapes: Vec<FillShape>) -> Self {
        for s in &mut local_shapes {
            s.layer_id = 1;
        }
        let mut timeline = Timeline::default();
        timeline.layers[0].keyframe_mut(0).cel = Cel::new(local_shapes);
        Self {
            id,
            name,
            timeline,
            revision: crate::vector_engine::next_mesh_revision(),
            mini_preview_cache: SyncCache::new(None),
            dynamic_anim_cache: SyncCache::new(None),
        }
    }

    /// Undo 履歴保存時などに、シンボル定義内のタイムラインおよびミニプレビューの一時メッシュキャッシュを破棄する
    pub(crate) fn strip_transient_caches(&mut self) {
        self.timeline.strip_transient_caches();
        self.clear_preview_and_anim_caches();
    }

    /// シンボル定義のミニプレビューおよび動的アニメーション判定の一時キャッシュを破棄する
    pub(crate) fn clear_preview_and_anim_caches(&mut self) {
        *self.mini_preview_cache.lock() = None;
        *self.dynamic_anim_cache.lock() = None;
    }

    fn ensure_preview_local_cache(&self, library_revision: u64, library: &[SymbolDef]) {
        let key = (self.revision, library_revision, self.timeline.current_frame);
        if self
            .mini_preview_cache
            .lock()
            .as_ref()
            .is_some_and(|c| c.key == key)
        {
            return;
        }
        let mut shapes = self.evaluated_local_shapes_at(self.timeline.current_frame, library);
        if shapes.is_empty() {
            shapes = self.first_non_empty_local_shapes(library);
        }
        let local_bounds_and_mesh = if shapes.is_empty() {
            None
        } else {
            let bbox = union_bounding_boxes(&shapes);
            let mut batch = egui::Mesh::default();
            for s in &shapes {
                append_mesh(&mut batch, s.fill_mesh());
            }
            Some((bbox, batch))
        };
        *self.mini_preview_cache.lock() = Some(SymbolPreviewCache {
            key,
            local_bounds_and_mesh,
            screen_mesh: None,
        });
    }

    /// シンボルプレビュー用のローカル座標外接矩形をキャッシュ経由で返す
    pub(crate) fn preview_local_bounds(
        &self,
        library_revision: u64,
        library: &[SymbolDef],
    ) -> Option<kurbo::Rect> {
        self.ensure_preview_local_cache(library_revision, library);
        self.mini_preview_cache
            .lock()
            .as_ref()
            .and_then(|c| c.local_bounds_and_mesh.as_ref().map(|(bbox, _)| *bbox))
    }

    /// 指定された `(scale, pan)` でスクリーン座標へ変換したシンボルプレビュー用結合 `Arc<Mesh>` をキャッシュ経由で返す
    pub(crate) fn cached_preview_screen_mesh(
        &self,
        scale: f32,
        pan: Vec2,
        library_revision: u64,
        library: &[SymbolDef],
    ) -> Option<Arc<egui::Mesh>> {
        self.ensure_preview_local_cache(library_revision, library);
        let scr_key = [scale.to_bits(), pan.x.to_bits(), pan.y.to_bits()];
        let mut guard = self.mini_preview_cache.lock();
        let cache = guard.as_mut()?;
        if let Some((k, mesh)) = &cache.screen_mesh {
            if *k == scr_key {
                return Some(Arc::clone(mesh));
            }
        }
        let (_, world_mesh) = cache.local_bounds_and_mesh.as_ref()?;
        let scr = Arc::new(transform_mesh_to_screen(world_mesh, scale, pan, false));
        cache.screen_mesh = Some((scr_key, Arc::clone(&scr)));
        Some(scr)
    }

    /// このシンボル自身、または内部の入れ子シンボルが複数フレーム（または先頭フレーム空白）を持つかを再帰判定する
    pub(crate) fn has_dynamic_animation(&self, library: &[SymbolDef], depth: usize) -> bool {
        if depth >= MAX_SYMBOL_NEST_DEPTH {
            return false;
        }
        let self_sig = self.timeline.layers.iter().fold(
            self.revision
                .wrapping_mul(31)
                .wrapping_add(self.timeline.total_frames as u64),
            |acc, l| {
                let c0 = l
                    .cel_at(0)
                    .map_or(0, |c| c.revision.wrapping_add(u64::from(!c.is_empty())));
                acc.wrapping_mul(31)
                    .wrapping_add(u64::from(l.visible))
                    .wrapping_mul(31)
                    .wrapping_add(c0)
            },
        );
        let lib_sig = library.iter().fold(library.len() as u64, |acc, s| {
            acc.wrapping_mul(31)
                .wrapping_add(s.id)
                .wrapping_mul(31)
                .wrapping_add(s.revision)
                .wrapping_mul(31)
                .wrapping_add(s.timeline.total_frames as u64)
        });
        if let Some((rev, sig, cached)) = *self.dynamic_anim_cache.lock() {
            if rev == self_sig && sig == lib_sig {
                return cached;
            }
        }
        let frame_zero_empty = self
            .timeline
            .layers
            .iter()
            .all(|l| !l.visible || l.cel_at(0).is_none_or(|c| c.is_empty()));
        let dynamic = if self.timeline.total_frames > 1 || frame_zero_empty {
            true
        } else {
            self.timeline.layers.iter().any(|layer| {
                layer.keyframes.iter().any(|kf| {
                    kf.cel.instances.iter().any(|inst| {
                        library.iter().any(|child| {
                            child.id == inst.binding.symbol_id
                                && child.has_dynamic_animation(library, depth + 1)
                        })
                    })
                })
            })
        };
        *self.dynamic_anim_cache.lock() = Some((self_sig, lib_sig, dynamic));
        dynamic
    }

    /// シンボルの全フレーム（`0..total_frames`）を先頭から走査し、最初に図形が存在するフレームのローカル図形列を返す
    pub(crate) fn first_non_empty_local_shapes(&self, library: &[SymbolDef]) -> Vec<FillShape> {
        let has_any_content = self
            .timeline
            .layers
            .iter()
            .any(|l| l.visible && l.keyframes.iter().any(|kf| !kf.cel.is_empty()));
        if !has_any_content {
            return Vec::new();
        }
        let ctx = SymbolEvalCtx::new(library).nested();
        let binding = SymbolInstanceBinding::new(0, self.id, Affine::IDENTITY);
        for f in 0..self.timeline.total_frames.max(1) {
            if !ctx.has_remaining_steps() {
                break;
            }
            let shapes = self.evaluated_local_shapes_for_instance(binding, 0, f, ctx.clone());
            if !shapes.is_empty() {
                return Arc::unwrap_or_clone(shapes);
            }
        }
        Vec::new()
    }

    fn collect_visible_local_shapes_at(
        &self,
        frame: usize,
        child_eval_frame: usize,
        ctx: SymbolEvalCtx<'_>,
    ) -> Vec<FillShape> {
        let mut out = Vec::new();
        for layer in &self.timeline.layers {
            if out.len() >= MAX_EXPANDED_SYMBOL_SHAPES || !ctx.has_remaining_steps() {
                ctx.mark_truncated();
                break;
            }
            if layer.visible {
                let eval = layer.evaluate_shapes_at((frame, child_eval_frame), 1, ctx.clone());
                let remaining = MAX_EXPANDED_SYMBOL_SHAPES.saturating_sub(out.len());
                if eval.len() > remaining {
                    ctx.mark_truncated();
                }
                out.extend(eval.into_iter().take(remaining));
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
        let fc = self.timeline.total_frames.max(1);
        let elapsed = current_frame.saturating_sub(kf_start_frame);
        match binding.loop_mode {
            SymbolLoopMode::Loop => (binding.first_frame % fc + elapsed % fc) % fc,
            SymbolLoopMode::PlayOnce => binding
                .first_frame
                .min(fc - 1)
                .saturating_add(elapsed)
                .min(fc - 1),
            SymbolLoopMode::SingleFrame => binding.first_frame.min(fc - 1),
        }
    }

    /// インスタンスの再生設定（ループ再生 / 1回再生 / 任意のフレーム）に基づいて、
    /// 指定フレーム `current_frame`（開始キーフレーム `kf_start_frame`）でのローカル図形列を返す
    pub(crate) fn evaluated_local_shapes_for_instance(
        &self,
        binding: SymbolInstanceBinding,
        kf_start_frame: usize,
        current_frame: usize,
        ctx: SymbolEvalCtx<'_>,
    ) -> Arc<Vec<FillShape>> {
        if !ctx.try_consume_step() {
            return Arc::default();
        }
        let internal_frame = self.resolve_internal_frame(binding, kf_start_frame, current_frame);
        let child_eval_frame = if self.timeline.total_frames <= 1
            && binding.loop_mode != SymbolLoopMode::SingleFrame
        {
            binding
                .first_frame
                .saturating_add(current_frame.saturating_sub(kf_start_frame))
        } else {
            internal_frame
        };
        let fc = self.timeline.total_frames.max(1);
        let frame_idx = internal_frame % fc;
        let eval_ctx = if ctx.depth == 0 { ctx.nested() } else { ctx };
        let memo_key = (self.id, frame_idx, child_eval_frame, eval_ctx.depth);
        if let Some(cached) = eval_ctx.get_memo(&memo_key) {
            return cached;
        }
        let evaluated = Arc::new(self.collect_visible_local_shapes_at(
            frame_idx,
            child_eval_frame,
            eval_ctx.clone(),
        ));
        eval_ctx.insert_memo(memo_key, Arc::clone(&evaluated));
        evaluated
    }

    /// 指定されたフレーム番号 `offset_frame` に対応するシンボル内部フレーム（`offset_frame % total_frames` でループ評価）の
    /// 全可視レイヤーのローカル図形列（背面→前面順）を返す
    pub(crate) fn evaluated_local_shapes_at(
        &self,
        offset_frame: usize,
        library: &[SymbolDef],
    ) -> Vec<FillShape> {
        let binding = SymbolInstanceBinding::new(0, self.id, Affine::IDENTITY);
        Arc::unwrap_or_clone(self.evaluated_local_shapes_for_instance(
            binding,
            0,
            offset_frame,
            SymbolEvalCtx::new(library).nested(),
        ))
    }
}

/// シンボルインプレイス編集の1段分の階層情報
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditLevel {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    /// 入った時点の、親の中での変換（トゥイーン中なら補間後の値）
    pub(crate) xform_in_parent: Affine,
    pub(crate) parent_frame: usize,
    pub(crate) parent_active_layer: usize,
    pub(crate) parent_group_path: Vec<u64>,
}

/// シンボルインプレイス編集の経路（空ならルート）
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EditPath {
    pub(crate) levels: Vec<EditLevel>,
}

impl EditPath {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.levels.len()
    }

    #[inline]
    pub(crate) fn last(&self) -> Option<&EditLevel> {
        self.levels.last()
    }

    /// `levels` の変換を順に掛けたワールド（ステージ）変換行列を返す
    #[inline]
    pub(crate) fn world_xform(&self) -> Affine {
        self.levels
            .iter()
            .fold(Affine::IDENTITY, |acc, lvl| acc * lvl.xform_in_parent)
    }
}

/// 保存対象の文書状態（唯一の正解データ：ルートタイムライン・シンボルライブラリ・ステージサイズ・ID採番・リビジョン）
#[derive(Clone, Debug)]
pub(crate) struct Document {
    pub(crate) root: Timeline,
    pub(crate) library: Vec<SymbolDef>,
    pub(crate) stage_size: Vec2,
    pub(crate) next_symbol_id: u64,
    pub(crate) next_group_id: u64,
    /// `library` の内容変更世代番号（Undo履歴の `Arc` CoW 構造共有用）
    pub(crate) revision: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            root: Timeline::default(),
            library: Vec::new(),
            stage_size: Vec2::new(800.0, 450.0),
            next_symbol_id: 1,
            next_group_id: 1,
            revision: 0,
        }
    }
}

impl Document {
    /// シンボルライブラリの世代番号 `revision` を一意な値へ更新する
    pub(crate) fn bump_revision(&mut self) {
        self.revision = crate::vector_engine::next_mesh_revision();
    }

    /// 新しいグループ / シンボルインスタンス用の一意な `group_id` を採番して返す
    pub(crate) fn alloc_group_id(&mut self) -> u64 {
        let id = self.next_group_id.max(1);
        self.next_group_id = if id < u64::MAX { id + 1 } else { 1 };
        id
    }

    /// 新しいシンボル定義をライブラリへ登録し、採番された `symbol_id` を返す
    pub(crate) fn register_new_symbol(&mut self, local_shapes: Vec<FillShape>) -> u64 {
        let mut sym_id = self.next_symbol_id.max(1);
        while self.library.iter().any(|s| s.id == sym_id) {
            sym_id = if sym_id < u64::MAX { sym_id + 1 } else { 1 };
        }
        self.next_symbol_id = sym_id.saturating_add(1);
        let mut name_num = sym_id;
        while self
            .library
            .iter()
            .any(|s| s.name == format!("Symbol {name_num}"))
        {
            name_num = if name_num < u64::MAX { name_num + 1 } else { 1 };
        }
        let sym_def = SymbolDef::new(sym_id, format!("Symbol {name_num}"), local_shapes);
        self.library.push(sym_def);
        self.bump_revision();
        sym_id
    }

    /// 指定した `symbol_id` のシンボル定義を参照する
    pub(crate) fn find_symbol(&self, symbol_id: u64) -> Option<&SymbolDef> {
        self.library.iter().find(|s| s.id == symbol_id)
    }

    /// 指定した `symbol_id` のシンボル定義を可変参照する
    pub(crate) fn find_symbol_mut(&mut self, symbol_id: u64) -> Option<&mut SymbolDef> {
        self.library.iter_mut().find(|s| s.id == symbol_id)
    }

    /// 高さが `MAX_SYMBOL_NEST_DEPTH`（8）を超えるシンボルが1つでもあれば `true` を返す
    pub(crate) fn symbol_nest_exceeded(&self) -> bool {
        fn height(
            sym_id: u64,
            depth: usize,
            by_id: &HashMap<u64, &SymbolDef>,
            memo: &mut HashMap<u64, usize>,
        ) -> usize {
            if let Some(&h) = memo.get(&sym_id) {
                return h;
            }
            let Some(sym) = by_id.get(&sym_id) else {
                return 0;
            };
            if depth > MAX_SYMBOL_NEST_DEPTH {
                return MAX_SYMBOL_NEST_DEPTH + 1;
            }
            let mut max_child = 0usize;
            for layer in &sym.timeline.layers {
                for kf in &layer.keyframes {
                    for inst in kf.cel.instances.iter() {
                        let ch = height(inst.binding.symbol_id, depth + 1, by_id, memo);
                        if ch >= MAX_SYMBOL_NEST_DEPTH {
                            return MAX_SYMBOL_NEST_DEPTH + 1;
                        }
                        max_child = max_child.max(ch);
                    }
                }
            }
            let h = 1 + max_child;
            memo.insert(sym_id, h);
            h
        }
        let by_id: HashMap<u64, &SymbolDef> = self.library.iter().map(|s| (s.id, s)).collect();
        let mut memo = HashMap::with_capacity(self.library.len());
        self.library
            .iter()
            .any(|s| height(s.id, 1, &by_id, &mut memo) > MAX_SYMBOL_NEST_DEPTH)
    }

    /// `parent_symbol_id` の内部に `candidate_child_id` を配置した場合に、
    /// 直接または間接的な循環参照（`A -> B -> A` 等）が発生するかを DFS で判定する
    pub(crate) fn would_create_symbol_cycle(
        &self,
        parent_symbol_id: u64,
        candidate_child_id: u64,
    ) -> bool {
        if parent_symbol_id == candidate_child_id {
            return true;
        }
        let mut stack = vec![candidate_child_id];
        let mut visited = std::collections::HashSet::new();
        visited.insert(candidate_child_id);
        while let Some(cur_id) = stack.pop() {
            let Some(sym) = self.find_symbol(cur_id) else {
                continue;
            };
            for layer in &sym.timeline.layers {
                for kf in &layer.keyframes {
                    for inst in kf.cel.instances.iter() {
                        let child_id = inst.binding.symbol_id;
                        if child_id == parent_symbol_id {
                            return true;
                        }
                        if visited.insert(child_id) {
                            stack.push(child_id);
                        }
                    }
                }
            }
        }
        false
    }

    /// 現在の編集コンテキスト（ルートシーンまたはシンボルインプレース編集経路 `edit_path`）に
    /// `candidate_child_id` のシンボルインスタンスを安全に配置可能か（循環参照を起こさないか）を返す
    pub(crate) fn can_place_symbol_in_current_context(
        &self,
        edit_path: &EditPath,
        candidate_child_id: u64,
    ) -> bool {
        self.find_symbol(candidate_child_id).is_some()
            && edit_path
                .levels
                .iter()
                .all(|lvl| !self.would_create_symbol_cycle(lvl.symbol_id, candidate_child_id))
    }

    /// 指定した `symbol_id` のシンボル名を変更する（変更があった場合のみ `true` を返す）
    pub(crate) fn rename_symbol(&mut self, symbol_id: u64, new_name: String) -> bool {
        let Some(sym) = self.find_symbol_mut(symbol_id) else {
            return false;
        };
        if sym.name == new_name {
            return false;
        }
        sym.name = new_name;
        sym.revision = crate::vector_engine::next_mesh_revision();
        self.bump_revision();
        true
    }

    /// 指定した `symbol_id` をライブラリから削除し、ルートタイムライン・入れ子シンボルから参照をデタッチする
    pub(crate) fn remove_symbol_and_detach(&mut self, symbol_id: u64) -> bool {
        if !self.library.iter().any(|s| s.id == symbol_id) {
            return false;
        }
        for layer in &mut self.root.layers {
            layer.detach_symbol_id(symbol_id, &self.library, &mut self.next_group_id);
        }
        let library_snapshot = self.library.clone();
        for sym_def in &mut self.library {
            if sym_def.id == symbol_id {
                continue;
            }
            for layer in &mut sym_def.timeline.layers {
                layer.detach_symbol_id(symbol_id, &library_snapshot, &mut self.next_group_id);
            }
            sym_def.revision = crate::vector_engine::next_mesh_revision();
        }
        self.library.retain(|s| s.id != symbol_id);
        self.bump_revision();
        true
    }

    /// Undo / Redo 後に `edit_path` を検証し、途中のシンボルまたはインスタンスが消えていたら到達できる段まで戻る
    pub(crate) fn validate_edit_path(
        &mut self,
        edit_path: &mut EditPath,
        editing_group_path: &mut Vec<u64>,
    ) {
        let mut reachable_len = 0usize;
        for i in 0..edit_path.levels.len() {
            let lvl = &edit_path.levels[i];
            if self.find_symbol(lvl.symbol_id).is_none() {
                break;
            }
            if lvl.instance_gid != 0 {
                let parent_tl = if i == 0 {
                    &self.root
                } else {
                    let prev_sym_id = edit_path.levels[i - 1].symbol_id;
                    let Some(sym) = self.find_symbol(prev_sym_id) else {
                        break;
                    };
                    &sym.timeline
                };
                let has_inst = parent_tl.layers.iter().any(|l| {
                    l.keyframes.iter().any(|kf| {
                        kf.cel.instances.iter().any(|inst| {
                            inst.binding.instance_gid == lvl.instance_gid
                                && inst.binding.symbol_id == lvl.symbol_id
                        })
                    })
                });
                if !has_inst {
                    break;
                }
            }
            reachable_len = i + 1;
        }
        if reachable_len < edit_path.levels.len() {
            let popped = edit_path.levels[reachable_len].clone();
            edit_path.levels.truncate(reachable_len);
            *editing_group_path = popped.parent_group_path;
            if let Some(last) = edit_path.last() {
                if let Some(s) = self.find_symbol_mut(last.symbol_id) {
                    s.timeline.active_layer = popped
                        .parent_active_layer
                        .min(s.timeline.layers.len().saturating_sub(1));
                    s.timeline.current_frame = popped
                        .parent_frame
                        .min(s.timeline.total_frames.saturating_sub(1));
                }
            } else {
                self.root.active_layer = popped
                    .parent_active_layer
                    .min(self.root.layers.len().saturating_sub(1));
                self.root.current_frame = popped
                    .parent_frame
                    .min(self.root.total_frames.saturating_sub(1));
            }
        }
    }
}

// ============================================================================
// 2. 解決済みセルメッシュキャッシュ署名計算（衝突ゼロ署名 `ResolvedCelSig`）
// ============================================================================

impl ResolvedCelSig {
    #[inline]
    fn are_referenced_symbols_current(&self, library: &[SymbolDef]) -> bool {
        self.referenced_symbols.iter().all(|&(sym_id, rev)| {
            if rev == u64::MAX {
                !library.iter().any(|s| s.id == sym_id)
            } else {
                library.iter().any(|s| s.id == sym_id && s.revision == rev)
            }
        })
    }

    /// ヒープアロケーションや再帰走査を一切行わずに、既存の署名 `self` が指定キーフレーム・ライブラリに対して有効かを O(参照シンボル数) で判定する
    pub(crate) fn matches_layer_keyframe(
        &self,
        layer: &Layer,
        kf_idx: usize,
        library: &[SymbolDef],
    ) -> bool {
        let kf = &layer.keyframes[kf_idx];
        let next_kf = if kf.tween {
            layer.keyframes.get(kf_idx + 1).and_then(|nkf| {
                nkf.cel
                    .single_symbol_instance()
                    .map(|nb| (nkf.start_frame, nb))
            })
        } else {
            None
        };
        self.cel_revision == kf.cel.revision
            && self.start_frame == kf.start_frame
            && self.tween == kf.tween
            && self.tween_rotate == kf.tween_rotate
            && self.next_kf == next_kf
            && self.are_referenced_symbols_current(library)
    }
}

/// 解決済みセルメッシュキャッシュの署名照合・遅延構築クエリ（`Copy` かつヒープ確保ゼロ）
#[derive(Clone, Copy)]
pub(crate) enum ResolvedSigQuery<'a> {
    LayerKeyframe {
        layer: &'a Layer,
        kf_idx: usize,
        library: &'a [SymbolDef],
    },
}

impl<'a> ResolvedSigQuery<'a> {
    /// 既存のキャッシュ済み署名 `sig` が現在も有効かをヒープアロケーションなしで高速判定する
    #[inline]
    pub(crate) fn matches(self, sig: &ResolvedCelSig) -> bool {
        match self {
            Self::LayerKeyframe {
                layer,
                kf_idx,
                library,
            } => sig.matches_layer_keyframe(layer, kf_idx, library),
        }
    }

    /// キャッシュ署名不一致（初回またはシンボル/セル編集後）の場合にのみ新しい `ResolvedCelSig` を構築する
    pub(crate) fn compute(self) -> ResolvedCelSig {
        match self {
            Self::LayerKeyframe {
                layer,
                kf_idx,
                library,
            } => compute_resolved_cel_signature(layer, kf_idx, library),
        }
    }
}

/// 指定された `sym_id` およびその内部で入れ子参照されている全子シンボルの `(id, revision)` を再帰的に収集する
fn collect_referenced_symbol_revisions(
    sym_id: u64,
    library: &[SymbolDef],
    visited: &mut Vec<u64>,
    depth: usize,
    out: &mut Vec<(u64, u64)>,
) {
    if depth >= MAX_SYMBOL_NEST_DEPTH || visited.contains(&sym_id) {
        return;
    }
    visited.push(sym_id);
    if let Some(sym) = library.iter().find(|s| s.id == sym_id) {
        out.push((sym.id, sym.revision));
        for layer in &sym.timeline.layers {
            for kf in &layer.keyframes {
                for inst in kf.cel.instances.iter() {
                    collect_referenced_symbol_revisions(
                        inst.binding.symbol_id,
                        library,
                        visited,
                        depth + 1,
                        out,
                    );
                }
            }
        }
    } else {
        out.push((sym_id, u64::MAX));
    }
}

pub(crate) fn compute_resolved_cel_signature(
    layer: &Layer,
    kf_idx: usize,
    library: &[SymbolDef],
) -> ResolvedCelSig {
    let kf = &layer.keyframes[kf_idx];
    let mut visited_syms: Vec<u64> = Vec::new();
    let mut referenced_symbols: Vec<(u64, u64)> = Vec::new();
    for inst in kf.cel.instances.iter() {
        collect_referenced_symbol_revisions(
            inst.binding.symbol_id,
            library,
            &mut visited_syms,
            0,
            &mut referenced_symbols,
        );
    }
    let mut next_kf = None;
    if kf.tween {
        if let Some(nkf) = layer.keyframes.get(kf_idx + 1) {
            if let Some(nb) = nkf.cel.single_symbol_instance() {
                next_kf = Some((nkf.start_frame, nb));
                collect_referenced_symbol_revisions(
                    nb.symbol_id,
                    library,
                    &mut visited_syms,
                    0,
                    &mut referenced_symbols,
                );
            }
        }
    }
    ResolvedCelSig {
        cel_revision: kf.cel.revision,
        start_frame: kf.start_frame,
        tween: kf.tween,
        tween_rotate: kf.tween_rotate,
        next_kf,
        referenced_symbols,
    }
}

/// シンボルインプレイス編集の背景レイヤー用：除外対象の `excluded_instance_gid` 以外のインスタンスが参照するシンボル群の世代番号から解決済みキャッシュ署名を計算する
pub(crate) fn compute_backdrop_layer_cel_signature(
    layer: &Layer,
    kf_idx: usize,
    excluded_instance_gid: u64,
    library: &[SymbolDef],
) -> ResolvedCelSig {
    let kf = &layer.keyframes[kf_idx];
    let mut visited_syms: Vec<u64> = Vec::new();
    let mut referenced_symbols: Vec<(u64, u64)> = Vec::new();
    for inst in kf.cel.instances.iter() {
        if inst.binding.instance_gid == excluded_instance_gid {
            continue;
        }
        collect_referenced_symbol_revisions(
            inst.binding.symbol_id,
            library,
            &mut visited_syms,
            0,
            &mut referenced_symbols,
        );
    }
    let mut next_kf = None;
    if kf.tween {
        if let Some(nkf) = layer.keyframes.get(kf_idx + 1) {
            if let Some(nb) = nkf.cel.single_symbol_instance() {
                next_kf = Some((nkf.start_frame, nb));
                if kf
                    .cel
                    .single_symbol_instance()
                    .is_none_or(|sb| sb.instance_gid != excluded_instance_gid)
                {
                    collect_referenced_symbol_revisions(
                        nb.symbol_id,
                        library,
                        &mut visited_syms,
                        0,
                        &mut referenced_symbols,
                    );
                }
            }
        }
    }
    ResolvedCelSig {
        cel_revision: kf.cel.revision,
        start_frame: kf.start_frame,
        tween: kf.tween,
        tween_rotate: kf.tween_rotate,
        next_kf,
        referenced_symbols,
    }
}

// ============================================================================
// 3. Cel / Layer / Timeline のシンボル展開・解決済みメッシュ評価メソッド
// ============================================================================

impl Cel {
    fn ensure_resolved_world_batch(
        &self,
        cache: &mut CelMeshCache,
        sig_query: ResolvedSigQuery<'_>,
        eval_frame_key: usize,
        xform_key: [u64; 6],
        build_fn: impl FnOnce() -> Option<Mesh>,
    ) -> Option<Arc<Mesh>> {
        if cache.resolved_xform_key != Some(xform_key)
            || !cache
                .resolved_sig
                .as_ref()
                .is_some_and(|sig| sig_query.matches(sig))
        {
            cache.resolved_xform_key = Some(xform_key);
            cache.resolved_sig = Some(sig_query.compute());
            cache.resolved_world_batches.clear();
            cache.resolved_screen_caches.clear();
            cache.resolved_onion_caches.clear();
        }
        if let Some(opt_mesh) = lru_get(&mut cache.resolved_world_batches, &eval_frame_key) {
            return opt_mesh;
        }
        let built = build_fn().map(Arc::new);
        lru_insert(
            &mut cache.resolved_world_batches,
            eval_frame_key,
            built.clone(),
        );
        built
    }

    /// シンボルインスタンスやトゥイーンを含むセルの解決済みスクリーン座標 `Arc<Mesh>` をキャッシュ経由で返す
    pub(crate) fn cached_resolved_screen_mesh(
        &self,
        (sig_query, eval_frame_key, world_xform): (ResolvedSigQuery<'_>, usize, Affine),
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
        build_fn: impl FnOnce() -> Option<Mesh>,
    ) -> Option<Arc<Mesh>> {
        let mut cache = self.mesh_cache.lock();
        let xform_key = world_xform.as_coeffs().map(f64::to_bits);
        let view_key = (zoom.to_bits(), pan.x.to_bits(), pan.y.to_bits());
        if cache.resolved_view_key != Some(view_key) {
            cache.resolved_view_key = Some(view_key);
            cache.resolved_screen_caches.clear();
            cache.resolved_onion_caches.clear();
        }
        let key = (
            eval_frame_key,
            view_key.0,
            view_key.1,
            view_key.2,
            dim_backdrop,
        );
        if cache.resolved_xform_key == Some(xform_key)
            && cache
                .resolved_sig
                .as_ref()
                .is_some_and(|sig| sig_query.matches(sig))
        {
            if let Some(mesh) = lru_get(&mut cache.resolved_screen_caches, &key) {
                return Some(mesh);
            }
        }
        let world_batch = self.ensure_resolved_world_batch(
            &mut cache,
            sig_query,
            eval_frame_key,
            xform_key,
            build_fn,
        )?;
        let screen_mesh = Arc::new(transform_mesh_to_screen(
            &world_batch,
            zoom,
            pan,
            dim_backdrop,
        ));
        lru_insert(
            &mut cache.resolved_screen_caches,
            key,
            Arc::clone(&screen_mesh),
        );
        Some(screen_mesh)
    }

    /// シンボルインスタンスやトゥイーンを含むセルの解決済みオニオンスキン `Arc<Mesh>` をキャッシュ経由で返す
    pub(crate) fn cached_resolved_onion_mesh(
        &self,
        (sig_query, eval_frame_key, world_xform): (ResolvedSigQuery<'_>, usize, Affine),
        zoom: f32,
        pan: Vec2,
        (tint_rgb, alpha): ([u8; 3], u8),
        build_fn: impl FnOnce() -> Option<Mesh>,
    ) -> Option<Arc<Mesh>> {
        let mut cache = self.mesh_cache.lock();
        let xform_key = world_xform.as_coeffs().map(f64::to_bits);
        let view_key = (zoom.to_bits(), pan.x.to_bits(), pan.y.to_bits());
        if cache.resolved_view_key != Some(view_key) {
            cache.resolved_view_key = Some(view_key);
            cache.resolved_screen_caches.clear();
            cache.resolved_onion_caches.clear();
        }
        let key = (
            eval_frame_key,
            view_key.0,
            view_key.1,
            view_key.2,
            tint_rgb,
            alpha,
        );
        if cache.resolved_xform_key == Some(xform_key)
            && cache
                .resolved_sig
                .as_ref()
                .is_some_and(|sig| sig_query.matches(sig))
        {
            if let Some(mesh) = lru_get(&mut cache.resolved_onion_caches, &key) {
                return Some(mesh);
            }
        }
        let world_batch = self.ensure_resolved_world_batch(
            &mut cache,
            sig_query,
            eval_frame_key,
            xform_key,
            build_fn,
        )?;
        let onion_mesh = Arc::new(transform_mesh_to_screen_onion(
            &world_batch,
            zoom,
            pan,
            tint_rgb,
            alpha,
        ));
        lru_insert(
            &mut cache.resolved_onion_caches,
            key,
            Arc::clone(&onion_mesh),
        );
        Some(onion_mesh)
    }

    /// このセル内に、複数フレームまたは先頭フレーム空白の動的シンボルインスタンス（入れ子含む）が含まれているかを返す
    pub(crate) fn has_dynamic_symbol(&self, library: &[SymbolDef]) -> bool {
        self.instances.iter().any(|inst| {
            library.iter().any(|sym| {
                sym.id == inst.binding.symbol_id && sym.has_dynamic_animation(library, 0)
            })
        })
    }

    /// 指定された `symbol_id` を参照するシンボルインスタンスバインディングを解除（通常のグループ図形へ展開して `shapes` に格納）する
    pub(crate) fn detach_symbol_id(
        &mut self,
        symbol_id: u64,
        kf_start_frame: usize,
        library: &[SymbolDef],
        next_group_id: &mut u64,
    ) {
        if !self
            .instances
            .iter()
            .any(|inst| inst.binding.symbol_id == symbol_id)
        {
            return;
        }
        let ctx = SymbolEvalCtx::new(library);
        let mut out_shapes = Vec::with_capacity(self.shapes.len() + self.instances.len());
        let mut out_instances = Vec::with_capacity(self.instances.len());
        let mut next_cel_idx = 0;

        for inst in self.instances.iter().cloned() {
            let target_idx = inst.shape_order_index.min(self.shapes.len());
            while next_cel_idx < target_idx {
                out_shapes.push(self.shapes[next_cel_idx].clone());
                next_cel_idx += 1;
            }
            if inst.binding.symbol_id == symbol_id {
                if let Some(sym) = library.iter().find(|s| s.id == symbol_id) {
                    let local_shapes = sym.evaluated_local_shapes_for_instance(
                        inst.binding,
                        kf_start_frame,
                        kf_start_frame,
                        ctx.nested(),
                    );
                    let mut inner_gid_map: Vec<(u64, u64)> = Vec::new();
                    let inner_start = inst.parent_scope.len() + 1;
                    for mut s in inst.binding.instantiate_shapes(
                        &local_shapes,
                        inst.layer_id,
                        &inst.parent_scope,
                    ) {
                        s.symbol_instance = None;
                        if s.group_path.len() > inner_start {
                            for gid in &mut s.group_path[inner_start..] {
                                let mapped = if let Some(&(_, m)) =
                                    inner_gid_map.iter().find(|&&(old, _)| old == *gid)
                                {
                                    m
                                } else {
                                    let id = *next_group_id;
                                    *next_group_id = next_group_id.saturating_add(1);
                                    inner_gid_map.push((*gid, id));
                                    id
                                };
                                *gid = mapped;
                            }
                        }
                        out_shapes.push(s);
                    }
                }
            } else {
                let mut kept = inst;
                kept.shape_order_index = out_shapes.len();
                out_instances.push(kept);
            }
        }
        while next_cel_idx < self.shapes.len() {
            out_shapes.push(self.shapes[next_cel_idx].clone());
            next_cel_idx += 1;
        }
        self.set_parts(out_shapes, out_instances);
    }
}

impl Layer {
    /// レイヤー内の全キーフレームから指定 `symbol_id` のシンボル参照をデタッチし、単一シンボル条件を失ったトゥイーンを解除する
    pub(crate) fn detach_symbol_id(
        &mut self,
        symbol_id: u64,
        library: &[SymbolDef],
        next_group_id: &mut u64,
    ) {
        for i in 0..self.keyframes.len() {
            if self.keyframes[i]
                .cel
                .instances
                .iter()
                .any(|inst| inst.binding.symbol_id == symbol_id)
                || (self.keyframes[i].tween
                    && self.keyframes[i].cel.single_symbol_instance().is_none())
            {
                let kf = self.keyframe_mut(i);
                kf.cel
                    .detach_symbol_id(symbol_id, kf.start_frame, library, next_group_id);
                if kf.tween && kf.cel.single_symbol_instance().is_none() {
                    kf.tween = false;
                }
            }
        }
    }

    /// このレイヤーの指定フレーム `frame` における図形列を、クラシックトゥイーン補間およびシンボル内タイムライン評価込みで返す
    pub(crate) fn evaluate_shapes_at(
        &self,
        (frame, child_eval_frame): (usize, usize),
        out_layer_id: u64,
        ctx: SymbolEvalCtx<'_>,
    ) -> Vec<FillShape> {
        let Some(kf_idx) = self.keyframe_index_at(frame) else {
            return Vec::new();
        };
        let kf = &self.keyframes[kf_idx];
        if let Some((_, interp_b)) = self.interpolated_tween_binding_at(frame) {
            if ctx.depth < MAX_SYMBOL_NEST_DEPTH {
                if let Some(sym) = ctx.library.iter().find(|s| s.id == interp_b.symbol_id) {
                    let local_shapes = sym.evaluated_local_shapes_for_instance(
                        interp_b,
                        kf.start_frame,
                        child_eval_frame,
                        ctx.nested(),
                    );
                    if !local_shapes.is_empty() {
                        if local_shapes.len() > MAX_EXPANDED_SYMBOL_SHAPES {
                            ctx.mark_truncated();
                        }
                        return interp_b.instantiate_shapes(
                            &local_shapes[..local_shapes.len().min(MAX_EXPANDED_SYMBOL_SHAPES)],
                            out_layer_id,
                            &[],
                        );
                    }
                }
            }
            if ctx.depth == 0 {
                return vec![FillShape::new_empty_symbol_marker(
                    interp_b,
                    out_layer_id,
                    &[],
                )];
            }
            return Vec::new();
        }

        resolve_cel_with_library(&kf.cel, kf.start_frame, child_eval_frame, ctx)
            .into_iter()
            .map(|s| s.with_layer_id(out_layer_id))
            .collect()
    }

    /// このレイヤーの指定フレーム `frame` における単一シンボルインスタンス `inst` のみを展開して返す
    pub(crate) fn evaluate_single_symbol_instance_at(
        &self,
        frame: usize,
        inst: &SymbolInstance,
        library: &[SymbolDef],
    ) -> Vec<FillShape> {
        let Some(kf_idx) = self.keyframe_index_at(frame) else {
            return Vec::new();
        };
        let kf = &self.keyframes[kf_idx];
        let b = inst.binding;
        let Some(sym) = library.iter().find(|sym| sym.id == b.symbol_id) else {
            return Vec::new();
        };
        let ctx = SymbolEvalCtx::new(library);
        let local_shapes =
            sym.evaluated_local_shapes_for_instance(b, kf.start_frame, frame, ctx.nested());
        if local_shapes.is_empty() {
            vec![FillShape::new_empty_symbol_marker(
                b,
                self.id,
                &inst.parent_scope,
            )]
        } else {
            if local_shapes.len() > MAX_EXPANDED_SYMBOL_SHAPES {
                ctx.mark_truncated();
            }
            b.instantiate_shapes(
                &local_shapes[..local_shapes.len().min(MAX_EXPANDED_SYMBOL_SHAPES)],
                self.id,
                &inst.parent_scope,
            )
        }
    }
}

impl Timeline {
    /// 指定レイヤー `layer_idx` の指定フレーム `frame` における図形列を、クラシックトゥイーン補間およびシンボル内タイムライン評価込みで返す
    pub(crate) fn evaluate_layer_shapes_at(
        &self,
        layer_idx: usize,
        frame: usize,
        library: &[SymbolDef],
    ) -> Vec<FillShape> {
        let Some(layer) = self.layers.get(layer_idx) else {
            return Vec::new();
        };
        layer.evaluate_shapes_at((frame, frame), layer.id, SymbolEvalCtx::new(library))
    }

    fn build_uncached_world_batch_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
        library: &[SymbolDef],
    ) -> Option<Mesh> {
        let mut resolved = self.evaluate_layer_shapes_at(layer_idx, frame, library);
        if world_xform != Affine::IDENTITY {
            for s in &mut resolved {
                let _ = s.fill_mesh();
                s.transform_with_cached_mesh(world_xform);
            }
        }
        let mut batch = Mesh::default();
        for s in &resolved {
            append_mesh(&mut batch, s.fill_mesh());
        }
        (!batch.is_empty()).then_some(batch)
    }

    fn cel_cache_context_at<'a>(
        &'a self,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
        library: &'a [SymbolDef],
    ) -> Option<(&'a Cel, Option<(ResolvedSigQuery<'a>, usize)>)> {
        let layer = self.layers.get(layer_idx)?;
        let kf_idx = layer.keyframe_index_at(frame)?;
        let kf = &layer.keyframes[kf_idx];
        if kf.cel.is_empty() {
            return None;
        }
        let has_tween = layer.interpolated_tween_binding_at(frame).is_some();
        let is_pure_shape_cel =
            !has_tween && kf.cel.instances.is_empty() && world_xform == Affine::IDENTITY;
        if is_pure_shape_cel {
            return Some((&kf.cel, None));
        }
        let all_single_frame = !has_tween
            && kf
                .cel
                .instances
                .iter()
                .all(|inst| inst.binding.loop_mode == SymbolLoopMode::SingleFrame);
        let is_dynamic = !all_single_frame && (has_tween || kf.cel.has_dynamic_symbol(library));
        let eval_frame_key = if is_dynamic { frame } else { kf.start_frame };
        let sig_query = ResolvedSigQuery::LayerKeyframe {
            layer,
            kf_idx,
            library,
        };
        Some((&kf.cel, Some((sig_query, eval_frame_key))))
    }

    /// 指定レイヤー・フレームの描画用スクリーン `Arc<Mesh>` を返す（純粋シェイプセルもシンボル・トゥイーンを含むセルも `Cel` キャッシュを再利用する）
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolved_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
        world_xform: Affine,
        library: &[SymbolDef],
    ) -> Option<Arc<Mesh>> {
        let (cel, resolved_ctx) = self.cel_cache_context_at(layer_idx, frame, world_xform, library)?;
        let Some((sig, eval_frame_key)) = resolved_ctx else {
            return (!cel.shapes.is_empty())
                .then(|| cel.cached_screen_mesh(zoom, pan, dim_backdrop));
        };
        cel.cached_resolved_screen_mesh(
            (sig, eval_frame_key, world_xform),
            zoom,
            pan,
            dim_backdrop,
            || self.build_uncached_world_batch_mesh(layer_idx, frame, world_xform, library),
        )
    }

    /// 指定レイヤー・フレームのオニオンスキン描画用スクリーン `Arc<Mesh>` を返す（トゥイーン補間・複数フレームシンボルもキャッシュ経由で高速返却）
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolved_cel_onion_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        (tint_rgb, alpha): ([u8; 3], u8),
        world_xform: Affine,
        library: &[SymbolDef],
    ) -> Option<Arc<Mesh>> {
        let (cel, resolved_ctx) = self.cel_cache_context_at(layer_idx, frame, world_xform, library)?;
        let Some((sig, eval_frame_key)) = resolved_ctx else {
            return (!cel.shapes.is_empty())
                .then(|| cel.cached_onion_mesh(zoom, pan, tint_rgb, alpha));
        };
        cel.cached_resolved_onion_mesh(
            (sig, eval_frame_key, world_xform),
            zoom,
            pan,
            (tint_rgb, alpha),
            || self.build_uncached_world_batch_mesh(layer_idx, frame, world_xform, library),
        )
    }
}
