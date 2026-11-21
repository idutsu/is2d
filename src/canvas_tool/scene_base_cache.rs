//! 画面の土台（今のコマの、レイヤーごとの図形と計算に使う地図の写し）と、セル画を1枚の結合メッシュで描くときの写し
//! （オニオンスキン・ロック中や再生中のレイヤー・シンボル編集中の親の背景）

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Vec2};
use kurbo::Affine;

use crate::planar_map::LayerBase;
use crate::symbol::{
    compute_resolved_cel_signature, Library, ResolvedCelSig, SymbolEvalCtx, SymbolLoopMode,
};
use crate::timeline::{Layer, Timeline};
use crate::vector_engine::{tinted_screen_mesh, CelTint, FillShape};
use crate::vector_engine::{LayerId, ShapeId};

struct CachedLayer {
    layer_id: LayerId,
    visible: bool,
    locked: bool,
    cel_sig: Option<ResolvedCelSig>,
    eval_frame: usize,
    world_xform_bits: [u64; 6],
    range: Range<usize>,
    /// 画面に出しているキーフレームのセル画（文書の形。地図の編集・カーソル合わせ・浮いた片が、地図をここから読む）
    cel: Option<crate::timeline::Cel>,
    /// そのセル画（と、中のシンボルのセル画）の地図の、計算に使う写し。画面に出している間、写しの持ち主はここ
    /// （`LayerBase::of_stored` は、ここが持っている写しを返す）
    bases: Vec<Arc<LayerBase>>,
}

/// `Document`（編集中タイムライン＋シンボルライブラリ＋`world_xform`）から構築されるレイヤー別の読み取り専用土台キャッシュ
/// （このキャッシュを書き換えてよいのは `rebuild` 処理のみ）
#[derive(Default)]
pub(crate) struct SceneBaseCache {
    layers: Vec<CachedLayer>,
    flat_shapes: Vec<FillShape>,
    layer_order_ids: Vec<LayerId>,
}

fn layer_eval_frame(
    layer: &Layer,
    kf_idx: usize,
    frame: usize,
    library: &Library,
    excluded: Option<u64>,
) -> usize {
    let kf = &layer.keyframes[kf_idx];
    let keep = |inst: &crate::timeline::SymbolInstance| excluded != Some(inst.binding.instance_gid);
    let has_tween = layer
        .interpolated_tween_binding_at(frame)
        .is_some_and(|b| excluded != Some(b.instance_gid));
    let all_single_frame = !has_tween
        && kf
            .cel
            .instances
            .iter()
            .filter(|i| keep(i))
            .all(|inst| inst.binding.loop_mode == SymbolLoopMode::SingleFrame);
    let has_dyn = !all_single_frame
        && kf.cel.instances.iter().filter(|i| keep(i)).any(|inst| {
            library
                .iter()
                .any(|sym| sym.id == inst.binding.symbol_id && sym.has_dynamic_animation(library))
        });
    if !all_single_frame && (has_tween || has_dyn) {
        frame
    } else {
        layer.start_of(kf_idx)
    }
}

/// 表示中・ロックなしのレイヤーが `frame` で絵を持つとき、その（キーフレームの位置、評価するコマ）を返す
fn shown_keyframe_and_eval_frame(
    layer: &Layer,
    frame: usize,
    library: &Library,
) -> Option<(usize, usize)> {
    let kf_idx = (layer.visible && !layer.locked)
        .then(|| layer.keyframe_index_at(frame))
        .flatten()?;
    Some((
        kf_idx,
        layer_eval_frame(layer, kf_idx, frame, library, None),
    ))
}

impl CachedLayer {
    /// 前に作ったこのレイヤーを、作り直さずにそのまま使えるか。
    /// `shown` は、表示中・ロックなしで絵があるときの（キーフレームの位置、評価するコマ）
    fn matches(
        &self,
        layer: &Layer,
        shown: Option<(usize, usize)>,
        library: &Library,
        xform_bits: [u64; 6],
    ) -> bool {
        self.layer_id == layer.id
            && self.visible == layer.visible
            && self.locked == layer.locked
            && self.world_xform_bits == xform_bits
            && match shown {
                Some((kf_idx, ef)) => {
                    self.eval_frame == ef
                        && self
                            .cel_sig
                            .as_ref()
                            .is_some_and(|sig| sig.matches_layer_keyframe(layer, kf_idx, library))
                }
                None => self.cel_sig.is_none(),
            }
    }
}

fn drain_cached_layer_range(
    iter: &mut std::vec::IntoIter<FillShape>,
    cursor: &mut usize,
    range: Range<usize>,
    out: &mut Vec<FillShape>,
) {
    for _ in 0..range.start.saturating_sub(*cursor) {
        let _ = iter.next();
    }
    *cursor = range.end;
    out.extend(iter.take(range.len()));
}

impl SceneBaseCache {
    #[inline]
    pub(crate) fn shapes(&self) -> &[FillShape] {
        &self.flat_shapes
    }

    #[inline]
    pub(crate) fn layer_order_ids(&self) -> &[LayerId] {
        &self.layer_order_ids
    }

    /// レイヤー `layer_id` の、画面に出しているキーフレームのセル画（表示中・ロックなしで絵があるときだけ）
    pub(crate) fn cel(&self, layer_id: LayerId) -> Option<&crate::timeline::Cel> {
        self.layers
            .iter()
            .find(|l| l.layer_id == layer_id)
            .and_then(|l| l.cel.as_ref())
    }

    /// レイヤー `layer_id` の、書いたばかりの地図の計算に使う写し `base` を、次にそのレイヤーを作り直すまで持つ
    /// （`Cel::edit_local`・`Cel::put_local` が返したもの。作り直すと、セル画の地図の写しとして持ち直す）
    pub(crate) fn hold(&mut self, layer_id: LayerId, base: Arc<LayerBase>) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.layer_id == layer_id) {
            l.bases.push(base);
        }
    }

    /// レイヤー `layer_id` を、次の `rebuild` で作り直す（セル画を書き換えたあと）
    pub(crate) fn invalidate_layer(&mut self, layer_id: LayerId) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.layer_id == layer_id) {
            l.cel_sig = None;
        }
    }

    /// キー `(layer_id, cel_revision, eval_frame, world_xform, 参照シンボルの revision)` が変わったレイヤーだけ作り直し、
    /// `shape_id` と `mesh_revision` が同じ図形は描画キャッシュを `HashMap` で引き継ぐ
    /// タイムライン `tl` の今のコマ（変換 `world_xform`）を、作り直さずにそのまま見せてよいか（どのレイヤーの鍵も同じ）
    pub(crate) fn is_fresh(&self, tl: &Timeline, library: &Library, world_xform: Affine) -> bool {
        let frame = tl.current_frame;
        let xform_bits = world_xform.as_coeffs().map(f64::to_bits);
        self.layers.len() == tl.layers.len()
            && self.layers.iter().zip(&tl.layers).all(|(c, l)| {
                c.matches(
                    l,
                    shown_keyframe_and_eval_frame(l, frame, library),
                    library,
                    xform_bits,
                )
            })
    }

    pub(crate) fn rebuild(
        &mut self,
        tl: &Timeline,
        library: &Library,
        world_xform: Affine,
    ) -> bool {
        if self.is_fresh(tl, library, world_xform) {
            return false;
        }
        let frame = tl.current_frame;
        let xform_bits = world_xform.as_coeffs().map(f64::to_bits);

        let old_layers = std::mem::take(&mut self.layers);
        let old_flat = std::mem::take(&mut self.flat_shapes);
        let mut old_by_id: HashMap<LayerId, (CachedLayer, Vec<FillShape>)> =
            HashMap::with_capacity(old_layers.len());
        let mut old_flat_iter = old_flat.into_iter();
        let mut cursor = 0usize;
        for cl in old_layers {
            let mut vec = Vec::with_capacity(cl.range.len());
            drain_cached_layer_range(&mut old_flat_iter, &mut cursor, cl.range.clone(), &mut vec);
            old_by_id.insert(cl.layer_id, (cl, vec));
        }

        let mut new_layers = Vec::with_capacity(tl.layers.len());
        let mut new_flat = Vec::new();
        let mut new_order_ids = Vec::with_capacity(tl.layers.len());

        for layer in &tl.layers {
            new_order_ids.push(layer.id);
            let Some((kf_idx, ef)) = shown_keyframe_and_eval_frame(layer, frame, library) else {
                let start = new_flat.len();
                new_layers.push(CachedLayer {
                    layer_id: layer.id,
                    visible: layer.visible,
                    locked: layer.locked,
                    cel_sig: None,
                    eval_frame: 0,
                    world_xform_bits: xform_bits,
                    range: start..start,
                    cel: None,
                    bases: Vec::new(),
                });
                continue;
            };
            let prev = old_by_id.remove(&layer.id);
            if let Some((old_cl, _)) = &prev {
                if old_cl.matches(layer, Some((kf_idx, ef)), library, xform_bits) {
                    let (old_cl, old_shapes) = prev.unwrap();
                    let start = new_flat.len();
                    new_flat.extend(old_shapes);
                    let end = new_flat.len();
                    new_layers.push(CachedLayer {
                        layer_id: layer.id,
                        visible: layer.visible,
                        locked: layer.locked,
                        cel_sig: old_cl.cel_sig,
                        eval_frame: ef,
                        world_xform_bits: xform_bits,
                        range: start..end,
                        cel: Some(layer.keyframes[kf_idx].cel.clone()),
                        bases: old_cl.bases,
                    });
                    continue;
                }
            }
            // セル画の地図の写しの升目の索引と面の表は、直前まで画面に出していた写し（Undo・やり直しの前の地図など、塊を
            // 共有する地図の写し）から、変わった所だけ作る（図形を取り出す前に。取り出す所は、生きている写しを使う）
            let warm: Vec<Arc<LayerBase>> = match &prev {
                Some((old_cl, _)) => layer.keyframes[kf_idx]
                    .cel
                    .maps
                    .iter()
                    .map(|m| {
                        let b = m.base();
                        b.adopt_index(&old_cl.bases);
                        b.adopt_faces(&old_cl.bases, &m.group_path);
                        b
                    })
                    .collect(),
                None => Vec::new(),
            };
            let ctx = SymbolEvalCtx::new(library);
            let mut evaluated = layer.evaluate_shapes_at(frame, layer.id, ctx.clone());
            let bases = ctx.into_state().take_held();
            drop(warm);
            for s in &mut evaluated {
                s.place(world_xform);
            }
            if let Some((old_cl, old_shapes)) = prev {
                if old_cl.world_xform_bits == xform_bits {
                    reuse_caches_by_key(&mut evaluated, &old_shapes);
                }
            }
            let sig = compute_resolved_cel_signature(layer, kf_idx, None, library);
            let start = new_flat.len();
            new_flat.extend(evaluated);
            let end = new_flat.len();
            new_layers.push(CachedLayer {
                layer_id: layer.id,
                visible: layer.visible,
                locked: layer.locked,
                cel_sig: Some(sig),
                eval_frame: ef,
                world_xform_bits: xform_bits,
                range: start..end,
                cel: Some(layer.keyframes[kf_idx].cel.clone()),
                bases,
            });
        }

        self.layers = new_layers;
        self.flat_shapes = new_flat;
        self.layer_order_ids = new_order_ids;
        true
    }
}

/// `(shape_id, mesh_revision)` が同じ図形から描画・当たり判定のキャッシュを引き継ぐ
fn reuse_caches_by_key(shapes: &mut [FillShape], donors: &[FillShape]) {
    if donors.is_empty() {
        return;
    }
    let donor_map: HashMap<(ShapeId, u64), &FillShape> = donors
        .iter()
        .map(|d| ((d.shape_id, d.mesh_revision), d))
        .collect();
    for s in shapes {
        if let Some(donor) = donor_map.get(&(s.shape_id, s.mesh_revision)) {
            s.reuse_caches_from(donor);
        }
    }
}

/// 画面の写しの鍵：ズーム・パン（ビット）と色の変え方
type CelScreenKey = (u32, u32, u32, CelTint);

/// 描くセル画のある所：タイムライン（ルートは `None`、シンボルの中はシンボルの番号）と、描かないインスタンス（シンボルの
/// 中を編集しているときの親の背景では、編集しているインスタンス。その中身は、編集している所として描く）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CelScope {
    pub(crate) timeline: Option<u64>,
    pub(crate) excluded_instance: Option<u64>,
}

struct CachedCel {
    scope: CelScope,
    layer_id: LayerId,
    eval_frame: usize,
    world_xform_bits: [u64; 6],
    cel_sig: ResolvedCelSig,
    world_mesh: Option<egui::Mesh>,
    /// 画面座標に写したもの（止まっている間は写し直さない）と、直前の描画で使ったか
    screens: Vec<(CelScreenKey, Arc<egui::Mesh>, bool)>,
    /// 直前の描画で使ったか
    used: bool,
}

/// セル画を1枚の結合メッシュで描くときの写し（所・レイヤー・評価するコマ・変換・セル画の署名ごと）。ワールド座標の結合メッシュと、
/// 画面座標に写したものを持つ。持つのは、直前の1回の描画で使ったものだけ（`start_pass`）。オニオンスキンと、ロック中・
/// 再生中のレイヤーと、シンボル編集中の親の背景が、それぞれ1つずつ持つ（持ち主が無いと、描くたびに地図の組み立て・面・
/// メッシュを作り直す：レビュー「軽量化・高速化」の★1。親の背景も同じ写しで描く：レビュー f01d518 の N-3005）
#[derive(Default)]
pub(crate) struct CelMeshCache {
    entries: Vec<CachedCel>,
}

impl CelMeshCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// 1回の描画の始めに呼ぶ：直前の描画で使わなかった写しを捨てる
    pub(crate) fn start_pass(&mut self) {
        self.entries.retain(|e| e.used);
        for e in &mut self.entries {
            e.used = false;
            e.screens.retain(|s| s.2);
            for s in &mut e.screens {
                s.2 = false;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// 所 `scope` のタイムライン `tl` の、レイヤー `layer_idx` のコマ `frame` のセル画を、変換 `world_xform`・ズーム `zoom`・
    /// パン `pan`・色の変え方 `tint` で描く画面座標の結合メッシュ（`Timeline::build_cel_world_batch_mesh` を画面に写したもの）。
    /// セル画が変わらなければ作り直さず、ズーム・パン・色の変え方も同じなら写し直さない
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn screen_mesh(
        &mut self,
        tl: &Timeline,
        scope: CelScope,
        ctx: SymbolEvalCtx<'_>,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
        zoom: f32,
        pan: Vec2,
        tint: CelTint,
    ) -> Option<Arc<egui::Mesh>> {
        let idx = self.entry_index(tl, scope, ctx, layer_idx, frame, world_xform)?;
        let entry = &mut self.entries[idx];
        let world_batch = entry.world_mesh.as_ref()?;
        let key = (zoom.to_bits(), pan.x.to_bits(), pan.y.to_bits(), tint);
        if let Some(s) = entry.screens.iter_mut().find(|s| s.0 == key) {
            s.2 = true;
            return Some(Arc::clone(&s.1));
        }
        let mesh = Arc::new(tinted_screen_mesh(world_batch, zoom, pan, tint));
        // ズーム・パンが変わった古い写しは、同じ色の変え方なら置き換える（パン・ズームの間に写しを溜めない）
        entry.screens.retain(|s| s.0 .3 != tint);
        entry.screens.push((key, Arc::clone(&mesh), true));
        Some(mesh)
    }

    /// 所 `scope` のタイムライン `tl` の、レイヤー `layer_idx` のコマ `frame` のセル画の、変換 `world_xform` のワールド座標の
    /// 結合メッシュ（描くのと同じ写し：`screen_mesh`。無ければ作る）
    pub(crate) fn world_mesh(
        &mut self,
        tl: &Timeline,
        scope: CelScope,
        ctx: SymbolEvalCtx<'_>,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
    ) -> Option<&egui::Mesh> {
        let idx = self.entry_index(tl, scope, ctx, layer_idx, frame, world_xform)?;
        self.entries[idx].world_mesh.as_ref()
    }

    /// 写しの番号（無ければワールド座標の結合メッシュを作って入れる）。この描画で使った印を付ける
    fn entry_index(
        &mut self,
        tl: &Timeline,
        scope: CelScope,
        ctx: SymbolEvalCtx<'_>,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
    ) -> Option<usize> {
        let library = ctx.library;
        let key = CelKey::of(tl, scope, library, layer_idx, frame, world_xform)?;
        let idx = match self.find(tl, scope, library, &key) {
            Some(i) => i,
            None => {
                let world_mesh = key.build(tl, scope, world_xform, ctx);
                self.insert(tl, scope, library, &key, world_mesh)
            }
        };
        self.entries[idx].used = true;
        Some(idx)
    }

    /// 所 `scope` のタイムライン `tl` の、セル画 `cels`（レイヤーの番号、コマ）のうち写しの無いものの、変換 `world_xform` の
    /// ワールド座標の結合メッシュを、同時に作って写しに入れる（`parallel_map`）。スクラブで窓に入るコマや、遠くのコマへ
    /// 飛んだときの窓の全部を、いちばん重いもの1つ分ほどの待ち時間で作る（レビュー f01d518 の N-5004）
    pub(crate) fn prepare(
        &mut self,
        tl: &Timeline,
        scope: CelScope,
        ctx: SymbolEvalCtx<'_>,
        cels: &[(usize, usize)],
        world_xform: Affine,
    ) {
        let library = ctx.library;
        let mut missing: Vec<CelKey> = Vec::new();
        for &(layer_idx, frame) in cels {
            let Some(key) = CelKey::of(tl, scope, library, layer_idx, frame, world_xform) else {
                continue;
            };
            let listed = missing
                .iter()
                .any(|k| k.layer_idx == key.layer_idx && k.eval_frame == key.eval_frame);
            if !listed && self.find(tl, scope, library, &key).is_none() {
                missing.push(key);
            }
        }
        let built = crate::planar_map::parallel_map(&missing, |key| {
            key.build(tl, scope, world_xform, ctx.clone())
        });
        for (key, world_mesh) in missing.iter().zip(built) {
            self.insert(tl, scope, library, key, world_mesh);
        }
    }

    /// 鍵 `key` の写しの番号（セル画が変わっていれば無い）
    fn find(
        &self,
        tl: &Timeline,
        scope: CelScope,
        library: &Library,
        key: &CelKey,
    ) -> Option<usize> {
        let layer = &tl.layers[key.layer_idx];
        self.entries.iter().position(|e| {
            e.scope == scope
                && e.layer_id == layer.id
                && e.eval_frame == key.eval_frame
                && e.world_xform_bits == key.world_xform_bits
                && e.cel_sig.matches_layer_keyframe(layer, key.kf_idx, library)
        })
    }

    /// 鍵 `key` の写しを、ワールド座標の結合メッシュ `world_mesh` で入れて、その番号を返す（同じ所・レイヤー・評価する
    /// コマの古い写しは捨てる）
    fn insert(
        &mut self,
        tl: &Timeline,
        scope: CelScope,
        library: &Library,
        key: &CelKey,
        world_mesh: Option<egui::Mesh>,
    ) -> usize {
        let layer = &tl.layers[key.layer_idx];
        self.entries.retain(|e| {
            !(e.scope == scope && e.layer_id == layer.id && e.eval_frame == key.eval_frame)
        });
        self.entries.push(CachedCel {
            scope,
            layer_id: layer.id,
            eval_frame: key.eval_frame,
            world_xform_bits: key.world_xform_bits,
            cel_sig: compute_resolved_cel_signature(
                layer,
                key.kf_idx,
                scope.excluded_instance,
                library,
            ),
            world_mesh,
            screens: Vec::new(),
            used: false,
        });
        self.entries.len() - 1
    }
}

/// 写しを引くための、セル画の場所（レイヤーの番号・コマ・キーフレームの番号・評価するコマ・変換のビット）
struct CelKey {
    layer_idx: usize,
    frame: usize,
    kf_idx: usize,
    eval_frame: usize,
    world_xform_bits: [u64; 6],
}

impl CelKey {
    /// タイムライン `tl` のレイヤー `layer_idx` のコマ `frame` に絵があれば、その場所
    fn of(
        tl: &Timeline,
        scope: CelScope,
        library: &Library,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
    ) -> Option<Self> {
        let layer = tl.layers.get(layer_idx)?;
        let kf_idx = layer.keyframe_index_at(frame)?;
        if layer.keyframes[kf_idx].cel.is_empty() {
            return None;
        }
        Some(Self {
            layer_idx,
            frame,
            kf_idx,
            eval_frame: layer_eval_frame(layer, kf_idx, frame, library, scope.excluded_instance),
            world_xform_bits: world_xform.as_coeffs().map(f64::to_bits),
        })
    }

    /// ワールド座標の結合メッシュを作る
    fn build(
        &self,
        tl: &Timeline,
        scope: CelScope,
        world_xform: Affine,
        ctx: SymbolEvalCtx<'_>,
    ) -> Option<egui::Mesh> {
        tl.build_cel_world_batch_mesh(
            self.layer_idx,
            self.frame,
            world_xform,
            scope.excluded_instance,
            ctx,
        )
    }
}
