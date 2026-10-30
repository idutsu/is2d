use crate::{
    symbol::{
        interpolate_affine_with_pivot, resolve_cel_with_library, ResolvedCelSig, ResolvedSigQuery,
        SymbolDef, SymbolEvalCtx, SymbolInstance, TweenRotate, TweenState, MAX_SYMBOL_NEST_DEPTH,
    },
    vector_engine::{
        append_mesh, transform_mesh_to_screen, transform_mesh_to_screen_onion, FillShape,
        OnionMeshCacheEntry, ScreenMeshCacheEntry, SymbolInstanceBinding, SymbolLoopMode,
        SyncCache,
    },
};
use eframe::egui::{Color32, Mesh, Vec2};
use std::sync::Arc;

/// タイムラインおよびシンボル内タイムラインの最大許容フレーム数（破損JSONによる巨大ループ防止）
pub(crate) const MAX_TIMELINE_FRAMES: usize = 100_000;

// ============================================================================
// 1. タイムラインのデータモデル（Cel / Keyframe / Selection / Layer / Timeline）
// ============================================================================

const MAX_RESOLVED_CEL_CACHE_ENTRIES: usize = 256;
type ResolvedScreenCacheKey = (usize, u32, u32, u32, bool);
type ResolvedOnionCacheKey = (usize, u32, u32, u32, [u8; 3], u8);
type LruEntry<K, V> = (K, V, u64);

fn lru_get<K: PartialEq, V: Clone>(entries: &mut [LruEntry<K, V>], key: &K) -> Option<V> {
    let (_, val, last_used_tick) = entries.iter_mut().find(|(k, _, _)| k == key)?;
    *last_used_tick = crate::vector_engine::next_mesh_revision();
    Some(val.clone())
}

fn lru_insert<K, V>(entries: &mut Vec<LruEntry<K, V>>, key: K, val: V) {
    let tick = crate::vector_engine::next_mesh_revision();
    if entries.len() < MAX_RESOLVED_CEL_CACHE_ENTRIES {
        entries.push((key, val, tick));
    } else if let Some(oldest) = entries.iter_mut().min_by_key(|(_, _, t)| *t) {
        *oldest = (key, val, tick);
    }
}

/// セル内の全図形を1つに統合したワールド座標バッチメッシュと、直近の `(zoom, pan)` スクリーン座標・オニオンスキン変換済みキャッシュ
#[derive(Debug, Default)]
struct CelMeshCache {
    shape_revisions: Vec<(u64, Color32)>,
    world_batch_mesh: Arc<Mesh>,
    screen_cache: Option<ScreenMeshCacheEntry>,
    onion_cache: Option<OnionMeshCacheEntry>,
    /// シンボルインスタンスやトゥイーンを含むセルの解決済みメッシュキャッシュ署名
    resolved_sig: Option<ResolvedCelSig>,
    /// 解決済みスクリーン・オニオンメッシュキャッシュが対象としている直近のビューポート `(zoom_bits, pan_x_bits, pan_y_bits)`
    resolved_view_key: Option<(u32, u32, u32)>,
    resolved_world_batches: Vec<LruEntry<usize, Option<Arc<Mesh>>>>,
    resolved_screen_caches: Vec<LruEntry<ResolvedScreenCacheKey, Arc<Mesh>>>,
    resolved_onion_caches: Vec<LruEntry<ResolvedOnionCacheKey, Arc<Mesh>>>,
}

impl Clone for CelMeshCache {
    fn clone(&self) -> Self {
        Self {
            shape_revisions: self.shape_revisions.clone(),
            world_batch_mesh: Arc::clone(&self.world_batch_mesh),
            screen_cache: None,
            onion_cache: None,
            resolved_sig: None,
            resolved_view_key: None,
            resolved_world_batches: Vec::new(),
            resolved_screen_caches: Vec::new(),
            resolved_onion_caches: Vec::new(),
        }
    }
}

/// 1枚のセル画（非シンボル図形群 `Arc<Vec<FillShape>>` ＋ 正規化シンボルインスタンス配列 `Arc<Vec<SymbolInstance>>` ＋ 高速再生・表示用の結合メッシュキャッシュ）
#[derive(Clone, Debug)]
pub(crate) struct Cel {
    /// このセル内の非シンボル図形群（`symbol_instance.is_none()` の生シェイプおよび通常グループ図形のみを正規化保持、CoW `Arc` 共有）
    pub(crate) shapes: Arc<Vec<FillShape>>,
    /// このセル内に配置されているシンボルインスタンスの正規化リスト（`instance_gid` 単位で重複なし、CoW `Arc` 共有）
    pub(crate) instances: Arc<Vec<SymbolInstance>>,
    /// セル内容（`shapes` / `instances`）が更新されるたびにインクリメントされるリビジョン番号（Undo履歴の $O(1)$ CoW 共有用）
    pub(crate) revision: u64,
    mesh_cache: SyncCache<CelMeshCache>,
}

impl Default for Cel {
    fn default() -> Self {
        Self {
            shapes: Arc::new(Vec::new()),
            instances: Arc::new(Vec::new()),
            revision: crate::vector_engine::next_mesh_revision(),
            mesh_cache: SyncCache::new(CelMeshCache::default()),
        }
    }
}

impl Cel {
    pub(crate) fn new(all_shapes: Vec<FillShape>) -> Self {
        let (shapes, instances) = Self::split_shapes_and_instances(all_shapes);
        Self {
            shapes: Arc::new(shapes),
            instances: Arc::new(instances),
            revision: crate::vector_engine::next_mesh_revision(),
            mesh_cache: SyncCache::new(CelMeshCache::default()),
        }
    }

    pub(crate) fn new_with_instances(
        mut shapes: Vec<FillShape>,
        instances: Vec<SymbolInstance>,
    ) -> Self {
        shapes.retain(|s| s.symbol_instance.is_none() && !s.exterior.segments.is_empty());
        Self {
            shapes: Arc::new(shapes),
            instances: Arc::new(instances),
            revision: crate::vector_engine::next_mesh_revision(),
            mesh_cache: SyncCache::new(CelMeshCache::default()),
        }
    }

    #[inline]
    pub(crate) fn shapes_mut(&mut self) -> &mut Vec<FillShape> {
        self.revision = crate::vector_engine::next_mesh_revision();
        Arc::make_mut(&mut self.shapes)
    }

    #[inline]
    pub(crate) fn instances_mut(&mut self) -> &mut Vec<SymbolInstance> {
        self.revision = crate::vector_engine::next_mesh_revision();
        Arc::make_mut(&mut self.instances)
    }

    #[inline]
    pub(crate) fn set_parts(&mut self, shapes: Vec<FillShape>, instances: Vec<SymbolInstance>) {
        self.shapes = Arc::new(shapes);
        self.instances = Arc::new(instances);
        self.revision = crate::vector_engine::next_mesh_revision();
        *self.mesh_cache.lock() = CelMeshCache::default();
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        if !self.shapes.is_empty() || !self.instances.is_empty() {
            self.shapes = Arc::new(Vec::new());
            self.instances = Arc::new(Vec::new());
            self.revision = crate::vector_engine::next_mesh_revision();
        }
        *self.mesh_cache.lock() = CelMeshCache::default();
    }

    /// キャンバス展開図形列 `all_shapes` を「非シンボル図形列（`shapes`）」と「正規化 `SymbolInstance` リスト（`instances`）」に分離する
    pub(crate) fn split_shapes_and_instances(
        all_shapes: Vec<FillShape>,
    ) -> (Vec<FillShape>, Vec<SymbolInstance>) {
        let mut non_symbol_shapes = Vec::with_capacity(all_shapes.len());
        let mut instances: Vec<SymbolInstance> = Vec::new();
        for s in all_shapes {
            if let Some(b) = s.symbol_instance {
                if !instances
                    .iter()
                    .any(|inst| inst.binding.instance_gid == b.instance_gid)
                {
                    instances.push(SymbolInstance {
                        binding: b,
                        layer_id: s.layer_id,
                        parent_scope: b.parent_scope_from_path(&s.group_path).to_vec(),
                        shape_order_index: non_symbol_shapes.len(),
                    });
                }
            } else if !s.exterior.segments.is_empty() {
                non_symbol_shapes.push(s);
            }
        }
        (non_symbol_shapes, instances)
    }

    /// Undo 履歴保存時などに、セル内の全図形および結合メッシュキャッシュの一時データを破棄する
    pub(crate) fn strip_transient_caches(&mut self) {
        if self.shapes.iter().any(FillShape::has_any_transient_cache) {
            for s in Arc::make_mut(&mut self.shapes) {
                s.strip_transient_caches();
            }
        }
        *self.mesh_cache.lock() = CelMeshCache::default();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.shapes.is_empty() && self.instances.is_empty()
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
        for shape in self.shapes.iter() {
            append_mesh(&mut batch, shape.fill_mesh());
        }
        cache.world_batch_mesh = Arc::new(batch);
        cache.screen_cache = None;
        cache.onion_cache = None;
    }

    /// セル内の全図形を1つに結合し、指定された `(zoom, pan, dim_backdrop)` に変換したスクリーン座標 `Arc<Mesh>` を返す
    pub(crate) fn cached_screen_mesh(&self, zoom: f32, pan: Vec2, dim_backdrop: bool) -> Arc<Mesh> {
        let mut cache = self.mesh_cache.lock();
        self.ensure_world_batch_mesh(&mut cache);
        let key = (
            zoom.to_bits(),
            pan.x.to_bits(),
            pan.y.to_bits(),
            dim_backdrop,
        );
        if let Some((k, mesh)) = &cache.screen_cache {
            if *k == key {
                return Arc::clone(mesh);
            }
        }
        let screen_mesh = Arc::new(transform_mesh_to_screen(
            &cache.world_batch_mesh,
            zoom,
            pan,
            dim_backdrop,
        ));
        cache.screen_cache = Some((key, Arc::clone(&screen_mesh)));
        screen_mesh
    }

    /// オニオンスキン表示用に、結合済みワールドメッシュをスクリーン座標＋ティント色へ一括変換してキャッシュ経由で返す
    pub(crate) fn cached_onion_mesh(
        &self,
        zoom: f32,
        pan: Vec2,
        tint_rgb: [u8; 3],
        alpha: u8,
    ) -> Arc<Mesh> {
        let mut cache = self.mesh_cache.lock();
        self.ensure_world_batch_mesh(&mut cache);
        let key = (
            zoom.to_bits(),
            pan.x.to_bits(),
            pan.y.to_bits(),
            tint_rgb,
            alpha,
        );
        if let Some((k, mesh)) = &cache.onion_cache {
            if *k == key {
                return Arc::clone(mesh);
            }
        }
        let onion_mesh = Arc::new(transform_mesh_to_screen_onion(
            &cache.world_batch_mesh,
            zoom,
            pan,
            tint_rgb,
            alpha,
        ));
        cache.onion_cache = Some((key, Arc::clone(&onion_mesh)));
        onion_mesh
    }

    fn ensure_resolved_world_batch(
        &self,
        cache: &mut CelMeshCache,
        sig_query: ResolvedSigQuery<'_>,
        eval_frame_key: usize,
        build_fn: impl FnOnce() -> Option<Mesh>,
    ) -> Option<Arc<Mesh>> {
        if !cache
            .resolved_sig
            .as_ref()
            .is_some_and(|sig| sig_query.matches(sig))
        {
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
        (sig_query, eval_frame_key): (ResolvedSigQuery<'_>, usize),
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
        build_fn: impl FnOnce() -> Option<Mesh>,
    ) -> Option<Arc<Mesh>> {
        let mut cache = self.mesh_cache.lock();
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
        if cache
            .resolved_sig
            .as_ref()
            .is_some_and(|sig| sig_query.matches(sig))
        {
            if let Some(mesh) = lru_get(&mut cache.resolved_screen_caches, &key) {
                return Some(mesh);
            }
        }
        let world_batch =
            self.ensure_resolved_world_batch(&mut cache, sig_query, eval_frame_key, build_fn)?;
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

    /// シンボルインプレイス編集時の親シーン背景セル（`backdrop_cel`）の半透明スクリーン座標メッシュをキャッシュ経由で返す
    pub(crate) fn cached_backdrop_screen_mesh(
        &self,
        parent_frame: usize,
        zoom: f32,
        pan: Vec2,
        library: &[SymbolDef],
    ) -> Option<Arc<Mesh>> {
        if self.is_empty() {
            return None;
        }
        if self.instances.is_empty() {
            return Some(self.cached_screen_mesh(zoom, pan, true));
        }
        let sig_query = ResolvedSigQuery::SingleCel { cel: self, library };
        self.cached_resolved_screen_mesh((sig_query, parent_frame), zoom, pan, true, || {
            let shapes = crate::symbol::resolve_cel_with_library(
                self,
                parent_frame,
                parent_frame,
                SymbolEvalCtx::new(library),
            );
            let mut batch = Mesh::default();
            for s in &shapes {
                append_mesh(&mut batch, s.fill_mesh());
            }
            (!batch.is_empty()).then_some(batch)
        })
    }

    /// シンボルインスタンスやトゥイーンを含むセルの解決済みオニオンスキン `Arc<Mesh>` をキャッシュ経由で返す
    pub(crate) fn cached_resolved_onion_mesh(
        &self,
        (sig_query, eval_frame_key): (ResolvedSigQuery<'_>, usize),
        zoom: f32,
        pan: Vec2,
        (tint_rgb, alpha): ([u8; 3], u8),
        build_fn: impl FnOnce() -> Option<Mesh>,
    ) -> Option<Arc<Mesh>> {
        let mut cache = self.mesh_cache.lock();
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
        if cache
            .resolved_sig
            .as_ref()
            .is_some_and(|sig| sig_query.matches(sig))
        {
            if let Some(mesh) = lru_get(&mut cache.resolved_onion_caches, &key) {
                return Some(mesh);
            }
        }
        let world_batch =
            self.ensure_resolved_world_batch(&mut cache, sig_query, eval_frame_key, build_fn)?;
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

    /// このセル（キーフレーム）に「ルート直下の単一シンボルインスタンスのみ（1個だけ）」が存在する場合、
    /// その `SymbolInstanceBinding` を返す。
    /// （空セル、生シェイプのみ、通常グループのみ、複数シンボル、シンボル＋生シェイプ、シンボル＋グループ等の場合はすべて `None`）
    pub(crate) fn single_symbol_instance(&self) -> Option<SymbolInstanceBinding> {
        (self.shapes.is_empty()
            && self.instances.len() == 1
            && self.instances[0].parent_scope.is_empty())
        .then(|| self.instances[0].binding)
    }

    /// このセル内に、複数フレームまたは先頭フレーム空白の動的シンボルインスタンス（入れ子含む）が含まれているかを返す
    pub(crate) fn has_dynamic_symbol(&self, library: &[SymbolDef]) -> bool {
        self.instances.iter().any(|inst| {
            library.iter().any(|sym| {
                sym.id == inst.binding.symbol_id && sym.has_dynamic_animation(library, 0)
            })
        })
    }

    /// セル内の正規化シンボルインスタンス一覧 `self.instances` のうち、
    /// `map_binding` が `Some(updated_binding)` を返す対象インスタンスを更新する
    pub(crate) fn update_symbol_instances_with(
        &mut self,
        mut map_binding: impl FnMut(SymbolInstanceBinding) -> Option<SymbolInstanceBinding>,
    ) -> bool {
        let mut any_updated = false;
        for inst in self.instances_mut() {
            if let Some(new_b) = map_binding(inst.binding) {
                inst.binding = new_b;
                any_updated = true;
            }
        }
        any_updated
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
        let mut next_shape_idx = 0;

        for inst in self.instances.iter().cloned() {
            let target_idx = inst.shape_order_index.min(self.shapes.len());
            while next_shape_idx < target_idx {
                out_shapes.push(self.shapes[next_shape_idx].clone());
                next_shape_idx += 1;
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
        while next_shape_idx < self.shapes.len() {
            out_shapes.push(self.shapes[next_shape_idx].clone());
            next_shape_idx += 1;
        }
        self.set_parts(out_shapes, out_instances);
    }
}

/// タイムライン上のキーフレーム（開始フレーム番号 0-indexed、セル画 `Cel`、クラシックトゥイーン有効フラグ `tween`、トゥイーン回転方向 `tween_rotate`）
#[derive(Clone, Debug)]
pub(crate) struct Keyframe {
    pub(crate) start_frame: usize,
    pub(crate) cel: Cel,
    pub(crate) tween: bool,
    pub(crate) tween_rotate: TweenRotate,
}

impl Keyframe {
    pub(crate) fn new_blank(start_frame: usize) -> Self {
        Self {
            start_frame,
            cel: Cel::default(),
            tween: false,
            tween_rotate: TweenRotate::Auto,
        }
    }

    /// キーフレーム内のセル図形とトゥイーン設定をクリアし、変更があったかどうかを返す
    pub(crate) fn clear_cel(&mut self) -> bool {
        if self.cel.is_empty() && !self.tween {
            false
        } else {
            self.cel.clear();
            self.tween = false;
            self.tween_rotate = TweenRotate::Auto;
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
        let prev_kf_start = self
            .keyframe_index_at(frame)
            .map(|i| self.keyframes[i].start_frame)
            .unwrap_or(frame);
        let elapsed = frame.saturating_sub(prev_kf_start);
        let split_tween_binding = if clone_prev {
            self.interpolated_tween_binding_at(frame)
        } else {
            None
        };

        let cloned_cel = if clone_prev {
            self.cel_at(frame)
                .map(|c| {
                    let cloned_shapes: Vec<FillShape> = c
                        .shapes
                        .iter()
                        .cloned()
                        .map(|mut s| {
                            s.layer_id = lid;
                            if let Some((start_b, mut interp_b)) = split_tween_binding {
                                if interp_b.loop_mode != SymbolLoopMode::SingleFrame {
                                    interp_b.first_frame =
                                        interp_b.first_frame.saturating_add(elapsed);
                                }
                                if start_b.xform.determinant().abs() > 1e-9 {
                                    let delta = interp_b.xform * start_b.xform.inverse();
                                    s.transform(delta);
                                    s.symbol_instance = Some(interp_b);
                                }
                            } else if let Some(mut b) = s.symbol_instance {
                                if b.loop_mode != SymbolLoopMode::SingleFrame {
                                    b.first_frame = b.first_frame.saturating_add(elapsed);
                                    s.symbol_instance = Some(b);
                                }
                            }
                            s
                        })
                        .collect();
                    let cloned_instances: Vec<SymbolInstance> = c
                        .instances
                        .iter()
                        .cloned()
                        .map(|mut inst| {
                            inst.layer_id = lid;
                            if let Some((start_b, interp_b)) = split_tween_binding {
                                if inst.binding.instance_gid == start_b.instance_gid {
                                    inst.binding = interp_b;
                                }
                            }
                            if inst.binding.loop_mode != SymbolLoopMode::SingleFrame {
                                inst.binding.first_frame =
                                    inst.binding.first_frame.saturating_add(elapsed);
                            }
                            inst
                        })
                        .collect();
                    Cel::new_with_instances(cloned_shapes, cloned_instances)
                })
                .unwrap_or_default()
        } else {
            Cel::default()
        };

        let inherited_rotate = if split_tween_binding.is_some() {
            self.keyframe_index_at(frame)
                .map(|idx| self.keyframes[idx].tween_rotate)
                .unwrap_or(TweenRotate::Auto)
        } else {
            TweenRotate::Auto
        };
        let insert_idx = self.keyframes.partition_point(|kf| kf.start_frame < frame);
        self.keyframes.insert(
            insert_idx,
            Keyframe {
                start_frame: frame,
                cel: cloned_cel,
                tween: split_tween_binding.is_some(),
                tween_rotate: inherited_rotate,
            },
        );
        insert_idx
    }

    /// 指定フレーム `frame` を含むキーフレームを削除（フレーム0の先頭の場合は空白化）する（`Shift+F6`）
    pub(crate) fn delete_keyframe_at(&mut self, frame: usize) -> bool {
        let Some(idx) = self.keyframe_index_at(frame) else {
            return false;
        };
        if idx == 0 {
            if frame == 0 {
                self.keyframes[0].clear_cel()
            } else {
                false
            }
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
        if self.frame_count == 0 || frame >= self.frame_count {
            return false;
        }
        let Some(kf_idx) = self.keyframe_index_at(frame) else {
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
                tween_rotate: kf.tween_rotate,
            })
            .collect();

        if !kfs.iter().any(|kf| kf.start_frame == 0) {
            let fallback_cel = self.cel_at(min_frame).cloned().unwrap_or_default();
            let (fallback_tween, fallback_rotate) = self
                .keyframe_index_at(min_frame)
                .map(|idx| (self.keyframes[idx].tween, self.keyframes[idx].tween_rotate))
                .unwrap_or((false, TweenRotate::Auto));
            kfs.insert(
                0,
                Keyframe {
                    start_frame: 0,
                    cel: fallback_cel,
                    tween: fallback_tween,
                    tween_rotate: fallback_rotate,
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

    /// 抽出された相対キーフレーム群 `kfs` を `[dst_min_frame..=dst_max_frame]` に配置する（移動先範囲の既存キーフレームは上書き置換し、尺を超える場合は自動延長）
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
        self.keyframes
            .retain(|kf| kf.start_frame < dst_min_frame || kf.start_frame > dst_max_frame);
        for mut kf in kfs {
            if kf.cel.shapes.iter().any(|s| s.layer_id != dst_lid) {
                for shape in kf.cel.shapes_mut() {
                    shape.layer_id = dst_lid;
                }
            }
            if kf.cel.instances.iter().any(|i| i.layer_id != dst_lid) {
                for inst in kf.cel.instances_mut() {
                    inst.layer_id = dst_lid;
                }
            }
            let valid_tween = kf.tween && kf.cel.single_symbol_instance().is_some();
            let rotate_mode = if valid_tween {
                kf.tween_rotate
            } else {
                TweenRotate::Auto
            };
            let target_f = dst_min_frame + kf.start_frame;
            if let Some(existing) = self
                .keyframes
                .iter_mut()
                .find(|existing_kf| existing_kf.start_frame == target_f)
            {
                existing.cel = kf.cel;
                existing.tween = valid_tween;
                existing.tween_rotate = rotate_mode;
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
                        tween_rotate: rotate_mode,
                    },
                );
            }
        }
        if self.keyframes.is_empty() || self.keyframes[0].start_frame > 0 {
            self.keyframes.insert(0, Keyframe::new_blank(0));
        }
    }

    /// `kf_idx` 番目のキーフレームにおけるトゥイーン状態（未設定 / 点線の未完トゥイーン / 実線矢印の完全トゥイーン）を判定する
    pub(crate) fn tween_state_at_keyframe(&self, kf_idx: usize) -> TweenState {
        let Some(kf) = self.keyframes.get(kf_idx) else {
            return TweenState::None;
        };
        if !kf.tween {
            return TweenState::None;
        }
        let Some(start_binding) = kf.cel.single_symbol_instance() else {
            return TweenState::Incomplete;
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
                    rotate_mode: kf.tween_rotate,
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
                rotate_mode,
            } if frame > start_frame && frame < end_frame => {
                let t = (frame - start_frame) as f64 / (end_frame - start_frame) as f64;
                let (interp_xform, interp_pivot) = interpolate_affine_with_pivot(
                    start_binding.xform,
                    start_binding.pivot,
                    end_binding.xform,
                    end_binding.pivot,
                    rotate_mode,
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

    /// レイヤー内の全キーフレームから指定 `symbol_id` のシンボル参照をデタッチし、単一シンボル条件を失ったトゥイーンを解除する
    pub(crate) fn detach_symbol_id(
        &mut self,
        symbol_id: u64,
        library: &[SymbolDef],
        next_group_id: &mut u64,
    ) {
        for kf in &mut self.keyframes {
            kf.cel
                .detach_symbol_id(symbol_id, kf.start_frame, library, next_group_id);
            if kf.tween && kf.cel.single_symbol_instance().is_none() {
                kf.tween = false;
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
                        return interp_b.instantiate_shapes(&local_shapes, out_layer_id, &[]);
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
}

/// `load_shapes_from_timeline` 読み込み時点のシェイプ署名（ハッシュ衝突のない完全比較構造体）
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LoadedShapeSig {
    shape_id: u64,
    layer_id: u64,
    mesh_revision: u64,
    color: Color32,
    group_path: Vec<u64>,
    symbol_instance: Option<SymbolInstanceBinding>,
}

impl LoadedShapeSig {
    #[inline]
    pub(crate) fn from_shape(s: &FillShape) -> Self {
        Self {
            shape_id: s.shape_id,
            layer_id: s.layer_id,
            mesh_revision: s.mesh_revision,
            color: s.color,
            group_path: s.group_path.clone(),
            symbol_instance: s.symbol_instance,
        }
    }

    #[inline]
    pub(crate) fn matches(&self, s: &FillShape) -> bool {
        self.shape_id == s.shape_id
            && self.mesh_revision == s.mesh_revision
            && self.color == s.color
            && self.group_path == s.group_path
            && self.symbol_instance == s.symbol_instance
    }

    #[inline]
    pub(crate) fn matches_with_layer(&self, s: &FillShape) -> bool {
        self.layer_id == s.layer_id && self.matches(s)
    }

    fn shapes_match(a: &FillShape, b: &FillShape) -> bool {
        a.mesh_revision == b.mesh_revision
            && a.color == b.color
            && a.group_path == b.group_path
            && a.symbol_instance == b.symbol_instance
    }
}

/// レイヤー配列・現在のフレーム・FPS・セル範囲選択を管理するタイムライン構造体（永続ドキュメント状態）
#[derive(Clone, Debug)]
pub(crate) struct Timeline {
    pub(crate) layers: Vec<Layer>,
    pub(crate) current_frame: usize,
    pub(crate) active_layer: usize,
    pub(crate) total_frames: usize,
    pub(crate) fps: u32,
    pub(crate) next_layer_number: usize,
    pub(crate) next_layer_id: u64,
    pub(crate) cell_selection: Option<TimelineCellSelection>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            layers: vec![Layer::new_with_id(1, "Layer 1")],
            current_frame: 0,
            active_layer: 0,
            total_frames: 1,
            fps: 24,
            next_layer_number: 2,
            next_layer_id: 2,
            cell_selection: Some(TimelineCellSelection::single(0, 0)),
        }
    }
}

impl Timeline {
    /// シンボル内部タイムライン編集モード用に、シンボル定義の `timeline` をベースに親タイムラインのFPSを引き継いだ作業用 `Timeline` を生成する
    pub(crate) fn new_for_symbol_edit(&self, sym_def: &SymbolDef, sym_layers: Vec<Layer>) -> Self {
        let mut tl = sym_def.timeline.clone();
        let act_l = tl.active_layer.min(sym_layers.len().saturating_sub(1));
        let act_f = tl.current_frame.min(tl.total_frames.saturating_sub(1));
        tl.layers = sym_layers;
        tl.active_layer = act_l;
        tl.current_frame = act_f;
        tl.total_frames = tl.total_frames.max(1);
        tl.fps = self.fps;
        tl.next_layer_number = tl.next_layer_number.max(2);
        tl.next_layer_id = tl.next_layer_id.max(2);
        tl.cell_selection = Some(TimelineCellSelection::single(act_l, act_f));
        tl
    }

    /// 全レイヤーの `frame_count` の最大値からタイムライン全体の `total_frames` を再計算し、レイヤー選択インデックスを範囲内へ補正する
    pub(crate) fn sync_total_frames(&mut self) {
        self.total_frames = self
            .layers
            .iter()
            .map(|l| l.frame_count)
            .max()
            .unwrap_or(1)
            .max(1);
        let max_l = self.layers.len().saturating_sub(1);
        self.active_layer = self.active_layer.min(max_l);
        if let Some(sel) = &mut self.cell_selection {
            sel.min_layer = sel.min_layer.min(max_l);
            sel.max_layer = sel.max_layer.min(max_l);
        }
    }

    /// タイムライン全体の総フレーム数を `next_total` に変更する：
    /// - 延長時：従来 `total_frames` に達していた最長レイヤー群のみを `next_total` へ伸ばし、意図的に短い個別レイヤーの `frame_count` は上書きしない。
    /// - 短縮時：`next_total` を超えるレイヤーの `frame_count` を切り詰め、範囲外（`start_frame >= next_total`）の孤立キーフレームも除去する。
    pub(crate) fn resize_total_frames(&mut self, next_total: usize) {
        let next_total = next_total.max(1);
        let old_max = self
            .layers
            .iter()
            .map(|l| l.frame_count)
            .max()
            .unwrap_or(self.total_frames.max(1));

        if next_total > old_max {
            let mut any_extended = false;
            for layer in &mut self.layers {
                if layer.frame_count >= old_max {
                    layer.frame_count = next_total;
                    any_extended = true;
                }
            }
            if !any_extended {
                if let Some(layer) = self.layers.get_mut(self.active_layer) {
                    layer.frame_count = next_total;
                }
            }
        } else {
            for layer in &mut self.layers {
                if layer.frame_count > next_total {
                    layer.frame_count = next_total;
                    layer.keyframes.retain(|kf| kf.start_frame < next_total);
                    if layer.keyframes.is_empty() || layer.keyframes[0].start_frame > 0 {
                        layer.keyframes.insert(0, Keyframe::new_blank(0));
                    }
                }
            }
        }
        self.sync_total_frames();
        if self.current_frame >= self.total_frames {
            self.current_frame = self.total_frames - 1;
        }
        if let Some(sel) = &mut self.cell_selection {
            sel.min_frame = sel.min_frame.min(self.current_frame);
            sel.max_frame = sel.max_frame.min(self.current_frame);
        }
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
        self.layers
            .get(self.active_layer)
            .map(|l| l.id)
            .unwrap_or(1)
    }

    /// 指定した `layer_id` を持つレイヤーのインデックス（`0..layers.len()`）を返す
    pub(crate) fn layer_index_by_id(&self, layer_id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == layer_id)
    }

    /// 指定した `layer_id` のレイヤーが現在フレームにおいて表示中かつ非ロック（編集可能）であるかを返す
    pub(crate) fn is_layer_editable_by_id(&self, layer_id: u64) -> bool {
        self.layers.iter().any(|l| {
            l.id == layer_id
                && l.visible
                && !l.locked
                && self.current_frame < l.frame_count
                && !l.keyframes.is_empty()
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

    /// 指定レイヤー `lid` のシェイプ群が直近の読み込み時点から未変更であるかを判定する
    fn is_layer_unchanged_since_load(
        loaded_signatures: &[LoadedShapeSig],
        lid: u64,
        shapes: &[FillShape],
    ) -> bool {
        let mut loaded_iter = loaded_signatures.iter().filter(|sig| sig.layer_id == lid);
        shapes
            .iter()
            .filter(|s| s.layer_id == lid)
            .all(|s| loaded_iter.next().is_some_and(|sig| sig.matches(s)))
            && loaded_iter.next().is_none()
    }

    /// 非編集可能レイヤーに紐付いているシェイプの `layer_id` を現在の編集可能レイヤーIDへ補正する
    pub(crate) fn normalize_shape_layer_ids(&self, shapes: &mut [FillShape]) {
        let fallback_lid = if self.is_active_layer_editable() {
            self.active_layer_id()
        } else {
            self.layers
                .iter()
                .find(|l| l.visible && !l.locked && self.current_frame < l.frame_count)
                .map(|l| l.id)
                .unwrap_or_else(|| self.active_layer_id())
        };
        for shape in shapes {
            if !self.is_layer_editable_by_id(shape.layer_id) {
                shape.layer_id = fallback_lid;
            }
        }
    }

    /// Undo 履歴保存時などに、全レイヤー・全キーフレームの一時メッシュキャッシュを破棄する
    pub(crate) fn strip_transient_caches(&mut self) {
        self.strip_transient_caches_with_prev(None);
    }

    /// 直前の履歴スナップショット `prev` と同一リビジョンの `Cel` は既にキャッシュ除去済みの `Arc` を $O(1)$ で共有し、
    /// 変更されたキーフレームのみキャッシュ除去を実行する
    pub(crate) fn strip_transient_caches_with_prev(&mut self, prev: Option<&Timeline>) {
        for layer in &mut self.layers {
            let prev_layer = prev.and_then(|p| p.layers.iter().find(|pl| pl.id == layer.id));
            for kf in &mut layer.keyframes {
                if let Some(prev_kf) = prev_layer.and_then(|pl| {
                    pl.keyframes
                        .iter()
                        .find(|pkf| pkf.cel.revision == kf.cel.revision)
                }) {
                    kf.cel.shapes = Arc::clone(&prev_kf.cel.shapes);
                    kf.cel.instances = Arc::clone(&prev_kf.cel.instances);
                    *kf.cel.mesh_cache.lock() = CelMeshCache::default();
                } else {
                    kf.cel.strip_transient_caches();
                }
            }
        }
    }

    /// 与えられた図形リスト `shapes` を、現在フレームの各「表示中かつ非ロック」レイヤーの `Cel` に `layer_id` ごとに振り分けて書き込む
    /// （変更がないレイヤーはクローンとキャッシュ破棄をスキップし、いずれかの `Cel` が更新された場合のみ `true` を返す）
    pub(crate) fn write_shapes_to_editable_cels(
        &mut self,
        shapes: &[FillShape],
        library: &[SymbolDef],
        loaded_signatures: &[LoadedShapeSig],
    ) -> bool {
        let frame = self.current_frame;
        let mut any_cel_modified = false;
        for l_idx in 0..self.layers.len() {
            if !self.layers[l_idx].visible || self.layers[l_idx].locked {
                continue;
            }
            let lid = self.layers[l_idx].id;
            let is_tween_mid = self.layers[l_idx]
                .interpolated_tween_binding_at(frame)
                .is_some();
            let has_multiframe = self.layers[l_idx]
                .cel_at(frame)
                .is_some_and(|c| c.has_dynamic_symbol(library));

            if (is_tween_mid || has_multiframe)
                && Self::is_layer_unchanged_since_load(loaded_signatures, lid, shapes)
            {
                continue;
            }

            if is_tween_mid {
                let (prev_kf_start, prev_tween_rotate) = self.layers[l_idx]
                    .keyframe_index_at(frame)
                    .map(|i| {
                        let kf = &self.layers[l_idx].keyframes[i];
                        (kf.start_frame, kf.tween_rotate)
                    })
                    .unwrap_or((frame, TweenRotate::Auto));
                let elapsed = frame.saturating_sub(prev_kf_start);
                let new_shapes: Vec<FillShape> = shapes
                    .iter()
                    .filter(|s| s.layer_id == lid)
                    .cloned()
                    .collect();
                let kf_idx = self.layers[l_idx].insert_keyframe(frame, false);
                let (non_symbol_shapes, mut next_instances) =
                    Cel::split_shapes_and_instances(new_shapes);
                for inst in &mut next_instances {
                    if inst.binding.loop_mode != SymbolLoopMode::SingleFrame {
                        inst.binding.first_frame =
                            inst.binding.first_frame.saturating_add(elapsed);
                    }
                }
                self.layers[l_idx].keyframes[kf_idx].cel =
                    Cel::new_with_instances(non_symbol_shapes, next_instances);
                let has_single_sym = self.layers[l_idx].keyframes[kf_idx]
                    .cel
                    .single_symbol_instance()
                    .is_some();
                self.layers[l_idx].keyframes[kf_idx].tween = has_single_sym;
                if has_single_sym {
                    self.layers[l_idx].keyframes[kf_idx].tween_rotate = prev_tween_rotate;
                }
                any_cel_modified = true;
                continue;
            }

            if let Some(kf_idx) = self.layers[l_idx].keyframe_index_at(frame) {
                let kf = &mut self.layers[l_idx].keyframes[kf_idx];
                let updated_cel_shapes: Vec<FillShape> = shapes
                    .iter()
                    .filter(|s| s.layer_id == lid)
                    .cloned()
                    .collect();
                let (non_symbol_shapes, next_instances) =
                    Cel::split_shapes_and_instances(updated_cel_shapes);
                let unchanged = *kf.cel.instances == next_instances
                    && kf.cel.shapes.len() == non_symbol_shapes.len()
                    && kf
                        .cel
                        .shapes
                        .iter()
                        .zip(&non_symbol_shapes)
                        .all(|(a, b)| LoadedShapeSig::shapes_match(a, b));
                if !unchanged {
                    kf.cel.set_parts(non_symbol_shapes, next_instances);
                    if kf.tween && kf.cel.single_symbol_instance().is_none() {
                        kf.tween = false;
                    }
                    any_cel_modified = true;
                }
            }
        }
        any_cel_modified
    }

    /// 現在アクティブなレイヤーが現在フレームにおいて編集可能（表示中・非ロック・フレーム作成済み）であるかを返す
    pub(crate) fn is_active_layer_editable(&self) -> bool {
        self.layers
            .get(self.active_layer)
            .is_some_and(|l| l.visible && !l.locked && self.current_frame < l.frame_count)
    }

    /// 対象レイヤー群 `targets` に対して現在フレーム（または直後フレーム）へキーフレームを挿入する（`F6` / `F7`）
    pub(crate) fn insert_keyframe_on_targets(&mut self, targets: &[usize], clone_prev: bool) {
        let curr_f = self.current_frame;
        let should_advance = if clone_prev {
            targets.iter().all(|&l_idx| {
                self.layers
                    .get(l_idx)
                    .is_some_and(|l| l.keyframes.iter().any(|kf| kf.start_frame == curr_f))
            })
        } else {
            targets.iter().any(|&l_idx| {
                self.layers.get(l_idx).is_some_and(|l| {
                    l.keyframes
                        .iter()
                        .any(|kf| kf.start_frame == curr_f && !kf.cel.is_empty())
                })
            })
        };

        let target_frame = if should_advance { curr_f + 1 } else { curr_f };
        let shift_for_blank = !clone_prev
            && should_advance
            && targets.iter().any(|&l_idx| {
                self.layers.get(l_idx).is_some_and(|l| {
                    l.keyframes
                        .iter()
                        .any(|kf| kf.start_frame == target_frame && !kf.cel.is_empty())
                })
            });

        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                if shift_for_blank {
                    layer.insert_frame_at(curr_f);
                }
                layer.insert_keyframe(target_frame, clone_prev);
            }
        }
        self.sync_total_frames();
        self.current_frame = target_frame;
        if let Some(sel) = &mut self.cell_selection {
            sel.min_frame = target_frame;
            sel.max_frame = target_frame;
        }
    }

    /// 対象レイヤー群 `targets` の現在フレームに削除可能なキーフレームが存在するかを返す
    pub(crate) fn can_delete_keyframe_on_targets(&self, targets: &[usize]) -> bool {
        let curr_f = self.current_frame;
        targets.iter().any(|&l_idx| {
            self.layers.get(l_idx).is_some_and(|layer| {
                layer.keyframe_index_at(curr_f).is_some_and(|idx| {
                    idx > 0
                        || (curr_f == 0
                            && layer
                                .keyframes
                                .first()
                                .is_some_and(|kf| !kf.cel.is_empty() || kf.tween))
                })
            })
        })
    }

    /// 対象レイヤー群 `targets` の現在フレームにあるキーフレームを削除（または空白化）する（`Shift+F6`）
    pub(crate) fn delete_keyframe_on_targets(&mut self, targets: &[usize]) {
        let curr_f = self.current_frame;
        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                layer.delete_keyframe_at(curr_f);
            }
        }
    }

    /// 対象レイヤー群 `targets` の現在フレーム（または選択終端）に保持尺を挿入する（`F5`）
    pub(crate) fn extend_frame_on_targets(&mut self, targets: &[usize]) {
        let target_f = self
            .cell_selection
            .map(|s| s.max_frame.max(self.current_frame))
            .unwrap_or(self.current_frame);
        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                layer.insert_frame_at(target_f);
            }
        }
        self.sync_total_frames();
    }

    /// 対象レイヤー群 `targets` の現在フレームに短縮可能なフレームが存在するかを返す
    pub(crate) fn can_remove_frame_on_targets(&self, targets: &[usize]) -> bool {
        let curr_f = self.current_frame;
        targets.iter().any(|&l_idx| {
            self.layers.get(l_idx).is_some_and(|l| {
                curr_f < l.frame_count
                    && (l.frame_count > 1
                        || l.keyframes
                            .first()
                            .is_some_and(|kf| !kf.cel.is_empty() || kf.tween))
            })
        })
    }

    /// 対象レイヤー群 `targets` の現在フレームの保持尺を1フレーム短縮する（`Shift+F5`）
    pub(crate) fn remove_frame_on_targets(&mut self, targets: &[usize]) -> bool {
        let curr_f = self.current_frame;
        let mut any_changed = false;
        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                if layer.remove_frame_at(curr_f) {
                    any_changed = true;
                }
            }
        }
        self.sync_total_frames();
        self.current_frame = self.current_frame.min(self.total_frames.saturating_sub(1));
        if let Some(sel) = &mut self.cell_selection {
            sel.min_frame = sel.min_frame.min(self.current_frame);
            sel.max_frame = sel.max_frame.min(self.current_frame);
        }
        any_changed
    }

    /// フレーム移動・複製の移動先範囲 `(dst_sel, actual_dl)` を検証・算出する（移動量が0またはロックレイヤーを含む場合は `None`）
    pub(crate) fn resolve_frame_move_bounds(
        &self,
        sel: TimelineCellSelection,
        delta_layer: isize,
        delta_frame: isize,
    ) -> Option<(TimelineCellSelection, isize)> {
        let num_layers = self.layers.len();
        if num_layers == 0 {
            return None;
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
            return None;
        }
        for src_l in sel.min_layer..=sel.max_layer {
            let dst_l = (src_l as isize + actual_dl) as usize;
            let src_layer = self.layers.get(src_l)?;
            let dst_layer = self.layers.get(dst_l)?;
            if src_layer.locked || dst_layer.locked {
                return None;
            }
        }
        Some((
            TimelineCellSelection {
                min_layer: dst_min_layer,
                max_layer: dst_max_layer,
                min_frame: dst_min_frame,
                max_frame: dst_max_frame,
            },
            actual_dl,
        ))
    }

    /// 検証済みの移動先 `dst_sel` へ選択範囲 `sel` のキーフレーム群を移動または複製する
    pub(crate) fn move_or_copy_frames(
        &mut self,
        sel: TimelineCellSelection,
        dst_sel: TimelineCellSelection,
        actual_dl: isize,
        copy_mode: bool,
    ) {
        let extracted: Vec<(usize, Vec<Keyframe>)> = (sel.min_layer..=sel.max_layer)
            .filter_map(|src_l| {
                let dst_l = (src_l as isize + actual_dl) as usize;
                let kfs = self
                    .layers
                    .get(src_l)?
                    .extract_keyframes_in_range(sel.min_frame, sel.max_frame);
                Some((dst_l, kfs))
            })
            .collect();

        if !copy_mode {
            for src_l in sel.min_layer..=sel.max_layer {
                if let Some(layer) = self.layers.get_mut(src_l) {
                    layer.remove_keyframes_in_range(sel.min_frame, sel.max_frame);
                }
            }
        }

        for (dst_l, kfs) in extracted {
            if let Some(layer) = self.layers.get_mut(dst_l) {
                layer.place_keyframes_at(dst_sel.min_frame, dst_sel.max_frame, kfs);
            }
        }

        self.sync_total_frames();
        self.active_layer = dst_sel.max_layer;
        self.current_frame = dst_sel.min_frame;
        self.cell_selection = Some(dst_sel);
    }

    /// 新しいレイヤーを現在のアクティブレイヤーの1つ上（前面）に追加して選択する
    pub(crate) fn add_layer_above_active(&mut self) {
        let name = format!("Layer {}", self.next_layer_number);
        self.next_layer_number = self.next_layer_number.saturating_add(1);
        let lid = self.next_layer_id;
        self.next_layer_id = self.next_layer_id.saturating_add(1);
        let insert_idx = (self.active_layer + 1).min(self.layers.len());
        let fc = self.total_frames.max(1);
        self.layers
            .insert(insert_idx, Layer::new_with_id_and_frames(lid, name, fc));
        self.select_single_cell(insert_idx, self.current_frame);
    }

    /// 現在アクティブなレイヤーを削除し、総フレーム数と選択セルを再同期する（最低1レイヤーは維持する）
    pub(crate) fn delete_active_layer(&mut self) {
        if self.layers.len() <= 1 || self.active_layer >= self.layers.len() {
            return;
        }
        self.layers.remove(self.active_layer);
        self.sync_total_frames();
        self.select_single_cell(self.active_layer, self.current_frame);
    }

    /// 指定レイヤー `from_idx` を `to_idx` の重ね順位置へ移動して選択する
    pub(crate) fn reorder_layer(&mut self, from_idx: usize, to_idx: usize) {
        if from_idx >= self.layers.len() || to_idx >= self.layers.len() {
            return;
        }
        let layer = self.layers.remove(from_idx);
        self.layers.insert(to_idx, layer);
        self.select_single_cell(to_idx, self.current_frame);
    }

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
        library: &[SymbolDef],
    ) -> Option<Mesh> {
        let resolved = self.evaluate_layer_shapes_at(layer_idx, frame, library);
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
        library: &'a [SymbolDef],
    ) -> Option<(&'a Cel, Option<(ResolvedSigQuery<'a>, usize)>)> {
        let layer = self.layers.get(layer_idx)?;
        let kf_idx = layer.keyframe_index_at(frame)?;
        let kf = &layer.keyframes[kf_idx];
        if kf.cel.is_empty() {
            return None;
        }
        let has_tween = layer.interpolated_tween_binding_at(frame).is_some();
        let is_pure_shape_cel = !has_tween && kf.cel.instances.is_empty();
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
    pub(crate) fn resolved_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
        library: &[SymbolDef],
    ) -> Option<Arc<Mesh>> {
        let (cel, resolved_ctx) = self.cel_cache_context_at(layer_idx, frame, library)?;
        let Some((sig, eval_frame_key)) = resolved_ctx else {
            return (!cel.shapes.is_empty())
                .then(|| cel.cached_screen_mesh(zoom, pan, dim_backdrop));
        };
        cel.cached_resolved_screen_mesh((sig, eval_frame_key), zoom, pan, dim_backdrop, || {
            self.build_uncached_world_batch_mesh(layer_idx, frame, library)
        })
    }

    /// 指定レイヤー・フレームのオニオンスキン描画用スクリーン `Arc<Mesh>` を返す（トゥイーン補間・複数フレームシンボルもキャッシュ経由で高速返却）
    pub(crate) fn resolved_cel_onion_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        (tint_rgb, alpha): ([u8; 3], u8),
        library: &[SymbolDef],
    ) -> Option<Arc<Mesh>> {
        let (cel, resolved_ctx) = self.cel_cache_context_at(layer_idx, frame, library)?;
        let Some((sig, eval_frame_key)) = resolved_ctx else {
            return (!cel.shapes.is_empty())
                .then(|| cel.cached_onion_mesh(zoom, pan, tint_rgb, alpha));
        };
        cel.cached_resolved_onion_mesh((sig, eval_frame_key), zoom, pan, (tint_rgb, alpha), || {
            self.build_uncached_world_batch_mesh(layer_idx, frame, library)
        })
    }

    /// 現在のフレーム・選択レイヤーにおいてクラシックトゥイーンの切替が可能か `(切替可能か, 現在トゥイーンONか)` を返す
    pub(crate) fn can_toggle_tween_at_current_frame(&self) -> (bool, bool) {
        let targets = self.target_layers_for_frame_op();
        if targets.is_empty() {
            return (false, false);
        }
        let curr_f = self.current_frame;
        let mut any_tweened = false;
        let mut any_can_enable = false;
        for &l_idx in &targets {
            let layer = &self.layers[l_idx];
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

    /// 現在選択されているアクティブレイヤー・現在フレームが属するキーフレームがクラシックトゥイーン有効である場合、
    /// `(キーフレーム開始フレーム 0-indexed, 現在の回転モード TweenRotate, 完全トゥイーンか)` を返す
    pub(crate) fn active_tween_keyframe_info(&self) -> Option<(usize, TweenRotate, bool)> {
        let layer = self.layers.get(self.active_layer)?;
        let kf_idx = layer.keyframe_index_at(self.current_frame)?;
        let kf = layer.keyframes.get(kf_idx)?;
        if !kf.tween || kf.cel.single_symbol_instance().is_none() {
            return None;
        }
        let is_complete = matches!(
            layer.tween_state_at_keyframe(kf_idx),
            TweenState::Complete { .. }
        );
        Some((kf.start_frame, kf.tween_rotate, is_complete))
    }
}
