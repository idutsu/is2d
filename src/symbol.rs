use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use kurbo::Affine;

use crate::timeline::{Cel, Layer, Timeline};
pub(crate) use crate::vector_engine::{SymbolInstanceBinding, SymbolLoopMode, TweenRotate};
use crate::vector_engine::{
    append_mesh, transform_mesh_to_screen, union_bounding_boxes, Contour, FillShape, SyncCache,
};

// ============================================================================
// 1. シンボル定義（SymbolDef）・インスタンス紐付け・トゥイーン状態・編集セッション
// ============================================================================

/// セル（`Cel`）内に正規化して保持されるシンボルインスタンスエンティティ
/// （構成図形が0個の空白フレームや空シンボルであってもインスタンスの存在・配置情報を独立して保持し、`Cel::shapes` には展開図形を重複保持しない）
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolInstance {
    pub(crate) binding: SymbolInstanceBinding,
    pub(crate) layer_id: u64,
    /// このインスタンスが属する外側の親グループ階層パス（ルート配置なら空 `[]`）
    pub(crate) parent_scope: Vec<u64>,
    /// `Cel::shapes`（非シンボル図形列）内での背面→前面の挿入インデックス
    pub(crate) shape_order_index: usize,
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
        rotate_mode: TweenRotate,
    },
}

/// シンボルインプレイス編集時に未変形図形を逆引きする複合キャッシュキー（外周 `contour_id`, 色 RGBA, 穴輪郭数, 穴 `contour_id` 合成ハッシュ）
pub(crate) type StageShapeCacheKey = (u64, [u8; 4], usize, u64);

#[inline]
fn stage_shape_cache_key(s: &FillShape) -> StageShapeCacheKey {
    let holes_hash = s
        .holes
        .iter()
        .fold(0u64, |acc, h| acc.rotate_left(13) ^ h.contour_id);
    (
        s.exterior.contour_id,
        s.color.to_array(),
        s.holes.len(),
        holes_hash,
    )
}

/// シンボルインプレイス編集時に、ステージ変換後の輪郭 `Arc` から元の無劣化ローカル図形を O(1) 逆引きするキャッシュエントリ
#[derive(Clone, Debug)]
pub(crate) struct StageToLocalEntry {
    pub(crate) stage_exterior: Contour,
    pub(crate) stage_holes: Vec<Contour>,
    pub(crate) local_shape: FillShape,
}

/// シンボルインプレイス編集時に、ステージ変換後のセル `revision` から元のローカル `Cel` を O(1) 逆引きするキャッシュエントリ
#[derive(Clone, Debug)]
pub(crate) struct StageCelToLocalEntry {
    pub(crate) stage_revision: u64,
    pub(crate) stage_shapes: Arc<Vec<FillShape>>,
    pub(crate) local_cel: Cel,
}

/// シンボルインプレイス編集時に、未変更キーフレーム（`Cel::revision` 一致）および
/// 未変形図形（`StageShapeCacheKey` 一致）の2階層で無劣化ローカルデータを O(1) 復元するキャッシュ
#[derive(Clone, Debug, Default)]
pub(crate) struct StageToLocalCache {
    pub(crate) cels: HashMap<u64, StageCelToLocalEntry>,
    pub(crate) shapes: HashMap<StageShapeCacheKey, StageToLocalEntry>,
}

impl StageToLocalCache {
    /// Undo 履歴保存時などに、キャッシュ内のローカルセルおよびローカル図形の一時メッシュを破棄する
    pub(crate) fn strip_transient_caches(&mut self) {
        for entry in self.cels.values_mut() {
            entry.local_cel.strip_transient_caches();
        }
        for entry in self.shapes.values_mut() {
            entry.local_shape.strip_transient_caches();
        }
    }
}

/// ステージ上の図形 `s` が未変形（外周・内周穴の輪郭 `Contour` が完全一致）であれば、
/// 対応するローカル図形キャッシュエントリを `HashMap` から O(1) で検索して返す
fn lookup_untouched_local_shape<'a>(
    s: &FillShape,
    primary_cache: &'a HashMap<StageShapeCacheKey, StageToLocalEntry>,
    fallback_cache: &'a HashMap<StageShapeCacheKey, StageToLocalEntry>,
) -> Option<&'a StageToLocalEntry> {
    let key = stage_shape_cache_key(s);
    primary_cache
        .get(&key)
        .or_else(|| fallback_cache.get(&key))
        .filter(|e| s.has_same_contour_arcs(&e.stage_exterior, &e.stage_holes))
}

/// シンボル定義のローカルレイヤー配列を `xform` でステージ座標へ変換し、同時に2階層 O(1) 無劣化逆引きキャッシュ（`StageToLocalCache`）を構築する
pub(crate) fn transform_layers_with_local_cache(
    local_layers: &[Layer],
    xform: Affine,
) -> (Vec<Layer>, StageToLocalCache) {
    let mut stage_layers = local_layers.to_vec();
    let mut cache = StageToLocalCache::default();
    for layer in &mut stage_layers {
        for kf in &mut layer.keyframes {
            let local_cel = kf.cel.clone();
            for s in kf.cel.shapes_mut() {
                let local_shape = s.clone();
                s.transform(xform);
                cache.shapes.insert(
                    stage_shape_cache_key(s),
                    StageToLocalEntry {
                        stage_exterior: s.exterior.clone(),
                        stage_holes: s.holes.clone(),
                        local_shape,
                    },
                );
            }
            for inst in kf.cel.instances_mut() {
                inst.binding.xform = xform * inst.binding.xform;
            }
            cache.cels.insert(
                kf.cel.revision,
                StageCelToLocalEntry {
                    stage_revision: kf.cel.revision,
                    stage_shapes: Arc::clone(&kf.cel.shapes),
                    local_cel,
                },
            );
        }
    }
    (stage_layers, cache)
}

/// ライブラリパネルのミニプレビュー用バッチメッシュキャッシュのキー（`(sym_revision, library_revision, current_frame, rect_bits)`）と描画データ（`Option<(Arc<Mesh>, Pos2)>`）
pub(crate) type SymbolMiniPreviewCacheEntry = (
    (u64, u64, usize, [u32; 4]),
    Option<(Arc<egui::Mesh>, egui::Pos2)>,
);

/// ライブラリに登録されたシンボル定義（独自の `Timeline` とローカル基準点 `(0, 0)` を持つ）
#[derive(Clone, Debug)]
pub(crate) struct SymbolDef {
    pub(crate) id: u64,
    pub(crate) name: String,
    /// シンボル内部の独自タイムライン（各シェイプは基準点 `(0, 0)` 中心のローカル座標系）
    pub(crate) timeline: Timeline,
    /// シンボル定義の変更世代番号（解決済みセルメッシュキャッシュの署名計算用）
    pub(crate) revision: u64,
    mini_preview_cache: SyncCache<Option<SymbolMiniPreviewCacheEntry>>,
}

pub(crate) const MAX_SYMBOL_NEST_DEPTH: usize = 8;

/// シンボル再帰評価時の設定コンテキスト
#[derive(Clone, Copy)]
pub(crate) struct SymbolEvalCtx<'a> {
    pub(crate) library: &'a [SymbolDef],
    pub(crate) depth: usize,
}

impl<'a> SymbolEvalCtx<'a> {
    pub(crate) fn new(library: &'a [SymbolDef]) -> Self {
        Self { library, depth: 0 }
    }

    pub(crate) fn nested(self) -> Self {
        Self {
            library: self.library,
            depth: self.depth + 1,
        }
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
    if ctx.depth >= MAX_SYMBOL_NEST_DEPTH || cel.instances.is_empty() {
        return (*cel.shapes).clone();
    }

    let mut out = Vec::with_capacity(cel.shapes.len() + cel.instances.len());
    let mut next_shape_idx = 0;

    for inst in cel.instances.iter() {
        let target_idx = inst.shape_order_index.min(cel.shapes.len());
        while next_shape_idx < target_idx {
            out.push(cel.shapes[next_shape_idx].clone());
            next_shape_idx += 1;
        }
        let b = inst.binding;
        let Some(sym) = ctx.library.iter().find(|sym| sym.id == b.symbol_id) else {
            continue;
        };
        let local_shapes = sym.evaluated_local_shapes_for_instance(
            b,
            kf_start_frame,
            frame,
            ctx.nested(),
        );
        if local_shapes.is_empty() && ctx.depth == 0 {
            out.push(FillShape::new_empty_symbol_marker(
                b,
                inst.layer_id,
                &inst.parent_scope,
            ));
        } else {
            out.extend(b.instantiate_shapes(&local_shapes, inst.layer_id, &inst.parent_scope));
        }
    }

    while next_shape_idx < cel.shapes.len() {
        out.push(cel.shapes[next_shape_idx].clone());
        next_shape_idx += 1;
    }

    out
}

impl SymbolDef {
    pub(crate) fn new(id: u64, name: String, mut local_shapes: Vec<FillShape>) -> Self {
        for s in &mut local_shapes {
            s.layer_id = 1;
        }
        let mut timeline = Timeline::default();
        timeline.layers[0].keyframes[0].cel = Cel::new(local_shapes);
        Self {
            id,
            name,
            timeline,
            revision: 0,
            mini_preview_cache: SyncCache::new(None),
        }
    }

    /// Undo 履歴保存時などに、シンボル定義内のタイムラインおよびミニプレビューの一時メッシュキャッシュを破棄する
    pub(crate) fn strip_transient_caches(&mut self) {
        self.timeline.strip_transient_caches();
        *self.mini_preview_cache.lock() = None;
    }

    /// ライブラリパネルのミニプレビュー矩形 `rect` に対応するスクリーン変換済み結合 `Arc<Mesh>` と基準点スクリーン座標 `Pos2` をキャッシュ経由で返す
    pub(crate) fn cached_mini_preview(
        &self,
        rect: egui::Rect,
        library_revision: u64,
        library: &[SymbolDef],
    ) -> Option<(Arc<egui::Mesh>, egui::Pos2)> {
        let key = (
            self.revision,
            library_revision,
            self.timeline.current_frame,
            [
                rect.min.x.to_bits(),
                rect.min.y.to_bits(),
                rect.max.x.to_bits(),
                rect.max.y.to_bits(),
            ],
        );
        if let Some((k, cached)) = &*self.mini_preview_cache.lock() {
            if *k == key {
                return cached.clone();
            }
        }
        let mut shapes = self.evaluated_local_shapes_at(self.timeline.current_frame, library);
        if shapes.is_empty() {
            shapes = self.first_non_empty_local_shapes(library);
        }
        let result = if shapes.is_empty() {
            None
        } else {
            let bbox = union_bounding_boxes(&shapes);
            let w = (bbox.x1 - bbox.x0).max(1.0) as f32;
            let h = (bbox.y1 - bbox.y0).max(1.0) as f32;
            let pad = 14.0;
            let avail_w = (rect.width() - pad * 2.0).max(10.0);
            let avail_h = (rect.height() - pad * 2.0).max(10.0);
            let scale = (avail_w / w).min(avail_h / h).clamp(0.05, 4.0);
            let cx = ((bbox.x0 + bbox.x1) * 0.5) as f32;
            let cy = ((bbox.y0 + bbox.y1) * 0.5) as f32;
            let pan = rect.center().to_vec2() - egui::Vec2::new(cx * scale, cy * scale);
            let mut batch = egui::Mesh::default();
            for s in &shapes {
                let scr = transform_mesh_to_screen(s.fill_mesh(), scale, pan, false);
                append_mesh(&mut batch, &scr);
            }
            Some((Arc::new(batch), egui::Pos2::new(pan.x, pan.y)))
        };
        *self.mini_preview_cache.lock() = Some((key, result.clone()));
        result
    }

    /// このシンボル自身、または内部の入れ子シンボルが複数フレーム（または先頭フレーム空白）を持つかを再帰判定する
    pub(crate) fn has_dynamic_animation(&self, library: &[SymbolDef], depth: usize) -> bool {
        let frame_zero_empty = self
            .timeline
            .layers
            .iter()
            .all(|l| !l.visible || l.cel_at(0).is_none_or(|c| c.shapes.is_empty()));
        if self.timeline.total_frames > 1 || frame_zero_empty {
            return true;
        }
        if depth >= MAX_SYMBOL_NEST_DEPTH {
            return false;
        }
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
    }

    /// シンボルの全フレーム（`0..total_frames`）を先頭から走査し、最初に図形が存在するフレームのローカル図形列を返す
    pub(crate) fn first_non_empty_local_shapes(&self, library: &[SymbolDef]) -> Vec<FillShape> {
        for f in 0..self.timeline.total_frames.max(1) {
            let shapes = self.evaluated_local_shapes_at(f, library);
            if !shapes.is_empty() {
                return shapes;
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
            if layer.visible {
                out.extend(layer.evaluate_shapes_at((frame, child_eval_frame), 1, ctx));
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
        ctx: SymbolEvalCtx<'_>,
    ) -> Vec<FillShape> {
        let internal_frame = self.resolve_internal_frame(binding, kf_start_frame, current_frame);
        let child_eval_frame =
            if self.timeline.total_frames <= 1 && binding.loop_mode != SymbolLoopMode::SingleFrame {
                binding.first_frame + current_frame.saturating_sub(kf_start_frame)
            } else {
                internal_frame
            };
        let fc = self.timeline.total_frames.max(1);
        self.collect_visible_local_shapes_at(internal_frame % fc, child_eval_frame, ctx)
    }

    /// 指定されたフレーム番号 `offset_frame` に対応するシンボル内部フレーム（`offset_frame % total_frames` でループ評価）の
    /// 全可視レイヤーのローカル図形列（背面→前面順）を返す
    pub(crate) fn evaluated_local_shapes_at(
        &self,
        offset_frame: usize,
        library: &[SymbolDef],
    ) -> Vec<FillShape> {
        let binding = SymbolInstanceBinding::new(0, self.id, Affine::IDENTITY);
        self.evaluated_local_shapes_for_instance(
            binding,
            0,
            offset_frame,
            SymbolEvalCtx::new(library).nested(),
        )
    }

    /// 作業用タイムライン `working_tl` と逆変換行列 `inv_xform` からシンボル定義の `timeline` を更新する
    /// （未変更キーフレームは `Cel` の `Arc` ポインタ一致により O(1) でローカル `Cel` を共有復元し、
    ///   編集されたキーフレーム内でも `stage_to_local_cache.shapes` に O(1) 一致する未変形図形は逆変換・再テッセレーションをスキップする）
    pub(crate) fn update_from_working_timeline(
        &mut self,
        working_tl: &Timeline,
        inv_xform: Affine,
        stage_to_local_cache: &mut StageToLocalCache,
    ) {
        let mut next_tl = working_tl.clone();
        next_tl.total_frames = next_tl.total_frames.max(1);
        let mut next_cache = StageToLocalCache {
            cels: HashMap::with_capacity(stage_to_local_cache.cels.len()),
            shapes: HashMap::with_capacity(stage_to_local_cache.shapes.len()),
        };
        for layer in &mut next_tl.layers {
            for kf in &mut layer.keyframes {
                let cel_key = kf.cel.revision;
                if let Some(cel_hit) = next_cache
                    .cels
                    .get(&cel_key)
                    .or_else(|| stage_to_local_cache.cels.get(&cel_key))
                    .filter(|e| e.stage_revision == cel_key)
                    .cloned()
                {
                    for stage_s in cel_hit.stage_shapes.iter() {
                        let s_key = stage_shape_cache_key(stage_s);
                        if let std::collections::hash_map::Entry::Vacant(e) =
                            next_cache.shapes.entry(s_key)
                        {
                            if let Some(s_entry) = stage_to_local_cache.shapes.get(&s_key) {
                                e.insert(s_entry.clone());
                            }
                        }
                    }
                    kf.cel = cel_hit.local_cel.clone();
                    next_cache.cels.entry(cel_key).or_insert(cel_hit);
                    continue;
                }

                let stage_shapes = Arc::clone(&kf.cel.shapes);
                for s in kf.cel.shapes_mut() {
                    if let Some(hit) = lookup_untouched_local_shape(
                        s,
                        &next_cache.shapes,
                        &stage_to_local_cache.shapes,
                    )
                    .cloned()
                    {
                        let key = stage_shape_cache_key(s);
                        let restored = s.restore_from_local_template(&hit.local_shape, inv_xform);
                        next_cache.shapes.entry(key).or_insert(hit);
                        *s = restored;
                    } else {
                        let stage_exterior = s.exterior.clone();
                        let stage_holes = s.holes.clone();
                        let key = stage_shape_cache_key(s);
                        s.transform(inv_xform);
                        next_cache.shapes.insert(
                            key,
                            StageToLocalEntry {
                                stage_exterior,
                                stage_holes,
                                local_shape: s.clone(),
                            },
                        );
                    }
                }
                for inst in kf.cel.instances_mut() {
                    inst.binding.xform = inv_xform * inst.binding.xform;
                }
                next_cache.cels.insert(
                    cel_key,
                    StageCelToLocalEntry {
                        stage_revision: cel_key,
                        stage_shapes,
                        local_cel: kf.cel.clone(),
                    },
                );
            }
        }
        *stage_to_local_cache = next_cache;
        self.timeline = next_tl;
        self.revision = self.revision.wrapping_add(1);
    }
}

/// シンボルをダブルクリックしてシンボル独自のタイムライン編集に入った際に退避する親シーンの状態
#[derive(Clone, Debug)]
pub(crate) struct SymbolEditSession {
    pub(crate) symbol_id: u64,
    pub(crate) instance_gid: u64,
    pub(crate) instance_xform: Affine,
    pub(crate) parent_timeline: Timeline,
    pub(crate) parent_editing_group_path: Vec<u64>,
    /// 親シーンの現在フレームにおける周辺図形（対象シンボルインスタンス自身を除いた背景表示用バッチキャッシュ付きセル）
    pub(crate) backdrop_cel: Cel,
    /// 未変更キーフレームの `Cel` 共有および未変形図形の逆行列往復劣化防止を O(1) で行う2階層キャッシュ
    pub(crate) stage_to_local_cache: StageToLocalCache,
}

impl SymbolEditSession {
    /// Undo 履歴保存時などに、退避中の親タイムライン・背景セル・ローカル形状キャッシュの一時メッシュを破棄する
    pub(crate) fn strip_transient_caches(&mut self) {
        self.parent_timeline.strip_transient_caches();
        self.backdrop_cel.strip_transient_caches();
        self.stage_to_local_cache.strip_transient_caches();
    }
}

/// シンボルライブラリ・シンボル編集階層スタック・グループ/シンボルID採番カウンタを保持する構造体
#[derive(Clone, Debug)]
pub(crate) struct SymbolManager {
    pub(crate) library: Vec<SymbolDef>,
    pub(crate) next_symbol_id: u64,
    pub(crate) selected_library_symbol: Option<u64>,
    /// ライブラリ一覧の `TextEdit` で現在名前編集中のシンボルID（連続タイピング時にUndoを1回だけ記録する用）
    pub(crate) actively_renaming_symbol: Option<u64>,
    pub(crate) edit_stack: Vec<SymbolEditSession>,
    pub(crate) next_group_id: u64,
    /// `library` または `edit_stack` の内容変更世代番号（Undo履歴の `Arc` CoW 構造共有用）
    pub(crate) revision: u64,
}

impl Default for SymbolManager {
    fn default() -> Self {
        Self {
            library: Vec::new(),
            next_symbol_id: 1,
            selected_library_symbol: None,
            actively_renaming_symbol: None,
            edit_stack: Vec::new(),
            next_group_id: 1,
            revision: 0,
        }
    }
}

impl SymbolManager {
    /// シンボルライブラリまたは編集スタックの世代番号 `revision` をインクリメントする
    pub(crate) fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// 新しいグループ / シンボルインスタンス用の一意な `group_id` を採番して返す
    pub(crate) fn alloc_group_id(&mut self) -> u64 {
        let id = self.next_group_id;
        self.next_group_id += 1;
        id
    }

    /// 新しいシンボル定義をライブラリへ登録し、採番された `symbol_id` を返す
    pub(crate) fn register_new_symbol(&mut self, local_shapes: Vec<FillShape>) -> u64 {
        let sym_id = self.next_symbol_id;
        self.next_symbol_id += 1;
        let sym_def = SymbolDef::new(sym_id, format!("Symbol {sym_id}"), local_shapes);
        self.library.push(sym_def);
        self.selected_library_symbol = Some(sym_id);
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
        while let Some(cur_id) = stack.pop() {
            if !visited.insert(cur_id) {
                continue;
            }
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
                        if !visited.contains(&child_id) {
                            stack.push(child_id);
                        }
                    }
                }
            }
        }
        false
    }

    /// 現在の編集コンテキスト（ルートシーンまたはシンボルインプレース編集スタック）に
    /// `candidate_child_id` のシンボルインスタンスを安全に配置可能か（循環参照を起こさないか）を返す
    pub(crate) fn can_place_symbol_in_current_context(&self, candidate_child_id: u64) -> bool {
        self.find_symbol(candidate_child_id).is_some()
            && !self
                .edit_stack
                .iter()
                .any(|sess| self.would_create_symbol_cycle(sess.symbol_id, candidate_child_id))
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
        sym.revision = sym.revision.wrapping_add(1);
        self.bump_revision();
        true
    }

    /// Undo / Redo スナップショット復元後に、ライブラリ選択状態の整合性を整える
    pub(crate) fn sync_after_restore(&mut self) {
        self.actively_renaming_symbol = None;
        if self
            .selected_library_symbol
            .is_some_and(|id| !self.library.iter().any(|s| s.id == id))
        {
            self.selected_library_symbol = self.library.first().map(|s| s.id);
        }
    }

    /// シンボル内部タイムライン編集中に、現在の `working_tl`（シンボルタイムライン）の状態を
    /// ライブラリ定義（`SymbolDef`）へ同期する（`backdrop_cel` は正規化保持のため再構築不要）
    pub(crate) fn sync_active_edit(&mut self, working_tl: &Timeline) {
        let Some((sym_id, inv_xform)) = self.edit_stack.last().and_then(|session| {
            (session.instance_xform.determinant().abs() > 1e-9)
                .then(|| (session.symbol_id, session.instance_xform.inverse()))
        }) else {
            return;
        };

        let mut cache = self
            .edit_stack
            .last_mut()
            .map(|s| std::mem::take(&mut s.stage_to_local_cache))
            .unwrap_or_default();
        if let Some(sym_def) = self.find_symbol_mut(sym_id) {
            sym_def.update_from_working_timeline(working_tl, inv_xform, &mut cache);
        }
        if let Some(session) = self.edit_stack.last_mut() {
            session.stage_to_local_cache = cache;
        }
        self.bump_revision();
    }

    /// 指定した `symbol_id` をライブラリから削除し、渡されたタイムライン・キャンバス図形・入れ子シンボル・編集スタックから参照をデタッチする
    pub(crate) fn remove_symbol_and_detach(
        &mut self,
        symbol_id: u64,
        timeline: &mut Timeline,
        canvas_shapes: &mut Vec<FillShape>,
    ) -> bool {
        if !self.library.iter().any(|s| s.id == symbol_id) {
            return false;
        }
        for s in canvas_shapes.iter_mut() {
            if s.symbol_instance.is_some_and(|b| b.symbol_id == symbol_id) {
                s.symbol_instance = None;
            }
        }
        canvas_shapes.retain(|s| !s.exterior.segments.is_empty() || s.symbol_instance.is_some());
        for layer in &mut timeline.layers {
            layer.detach_symbol_id(symbol_id, &self.library);
        }
        for i in 0..self.library.len() {
            if self.library[i].id == symbol_id {
                continue;
            }
            let mut layers = std::mem::take(&mut self.library[i].timeline.layers);
            for layer in &mut layers {
                layer.detach_symbol_id(symbol_id, &self.library);
            }
            self.library[i].timeline.layers = layers;
            self.library[i].revision = self.library[i].revision.wrapping_add(1);
        }
        for sess in &mut self.edit_stack {
            for layer in &mut sess.parent_timeline.layers {
                layer.detach_symbol_id(symbol_id, &self.library);
            }
            sess.backdrop_cel.detach_symbol_id(
                symbol_id,
                sess.parent_timeline.current_frame,
                &self.library,
            );
        }
        self.library.retain(|s| s.id != symbol_id);
        if self.selected_library_symbol == Some(symbol_id) {
            self.selected_library_symbol = self.library.first().map(|s| s.id);
        }
        self.bump_revision();
        true
    }
}

// ============================================================================
// 2. 解決済みセルメッシュキャッシュ署名計算（衝突ゼロ署名 `ResolvedCelSig`）
// ============================================================================

/// シンボル展開やクラシックトゥイーン補間を含む解決済みセルメッシュキャッシュの衝突ゼロ署名
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedCelSig {
    pub(crate) cel_revision: u64,
    pub(crate) start_frame: usize,
    pub(crate) tween: bool,
    pub(crate) tween_rotate: TweenRotate,
    pub(crate) next_kf: Option<(usize, SymbolInstanceBinding)>,
    pub(crate) referenced_symbols: Vec<(u64, u64)>,
}

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
            layer
                .keyframes
                .get(kf_idx + 1)
                .and_then(|nkf| nkf.cel.single_symbol_instance().map(|nb| (nkf.start_frame, nb)))
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

    /// ヒープアロケーションを一切行わずに、既存の署名 `self` が単一セル（`backdrop_cel` 等）とライブラリに対して有効かを判定する
    pub(crate) fn matches_cel(&self, cel: &Cel, library: &[SymbolDef]) -> bool {
        self.cel_revision == cel.revision
            && self.start_frame == 0
            && !self.tween
            && self.tween_rotate == TweenRotate::Auto
            && self.next_kf.is_none()
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
    SingleCel {
        cel: &'a Cel,
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
            Self::SingleCel { cel, library } => sig.matches_cel(cel, library),
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
            Self::SingleCel { cel, library } => compute_cel_signature(cel, library),
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

/// 単一セル（`backdrop_cel` 等）とそれが参照するシンボル群の世代番号から解決済みキャッシュ署名を計算する
pub(crate) fn compute_cel_signature(cel: &Cel, library: &[SymbolDef]) -> ResolvedCelSig {
    let mut visited_syms: Vec<u64> = Vec::new();
    let mut referenced_symbols: Vec<(u64, u64)> = Vec::new();
    for inst in cel.instances.iter() {
        collect_referenced_symbol_revisions(
            inst.binding.symbol_id,
            library,
            &mut visited_syms,
            0,
            &mut referenced_symbols,
        );
    }
    ResolvedCelSig {
        cel_revision: cel.revision,
        start_frame: 0,
        tween: false,
        tween_rotate: TweenRotate::Auto,
        next_kf: None,
        referenced_symbols,
    }
}
