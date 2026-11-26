//! 画面の土台（今のコマの、レイヤーごとの図形と計算に使う地図の写し）と、セル画を1枚の結合メッシュで描くときの写し
//! （オニオンスキン・ロック中や再生中のレイヤー・シンボル編集中の親の背景）

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Color32, Vec2};
use kurbo::{Affine, Point};

use crate::planar_map::LayerBase;
use crate::symbol::{
    compute_resolved_cel_signature, CelShape, Library, ResolvedCelSig, SymbolEvalCtx, SymbolLoopMode,
};
use crate::timeline::{Layer, Timeline};
use crate::vector_engine::{tinted_screen_mesh, CelTint, FillShape};
use crate::vector_engine::{LayerId, ShapeId};

/// 使い回してよい画面の写し1つ：（レイヤー, 元の地図の図形, 画面の写し）（`SceneBaseCache::offer`）
pub(crate) type PlacedCopy = (LayerId, Arc<FillShape>, Arc<FillShape>);

/// 次に作り直すときに使い回してよい画面の写し（`SceneBaseCache::offer`）
struct Offered {
    /// 画面への変換のビット
    world: [u64; 6],
    placed: Vec<PlacedCopy>,
    /// その地図の図形を持つ地図の写し（書き戻しが同じ写しを使い：`LayerBase::of_local`、作り直すまで生かす）
    _bases: Vec<Arc<LayerBase>>,
}

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
    /// 図形（`range`）ごとの、元の地図の図形（地図の図形の表にある図形そのもの。インスタンスを展開した図形は `None`）。
    /// 作り直すとき、元の地図の図形が前と同じ物（番地）なら、前の画面の写しを使い回す（持っているので、番地が別の図形に
    /// 使い回されない）
    sources: Vec<Option<Arc<FillShape>>>,
}

/// `Document`（編集中タイムライン＋シンボルライブラリ＋`world_xform`）から構築されるレイヤー別の読み取り専用土台キャッシュ
/// （このキャッシュを書き換えてよいのは `rebuild` 処理のみ）
#[derive(Default)]
pub(crate) struct SceneBaseCache {
    layers: Vec<CachedLayer>,
    /// 今のコマの全レイヤーの図形（画面の写し。レイヤーの順。変わらない図形は、作り直しても同じ物を使い回す）
    flat_shapes: Vec<Arc<FillShape>>,
    layer_order_ids: Vec<LayerId>,
    /// 次に作り直すときに使い回してよい、画面の写し（`offer`）
    offered: Option<Offered>,
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
            .instances()
            .iter()
            .filter(|i| keep(i))
            .all(|inst| inst.binding.loop_mode == SymbolLoopMode::SingleFrame);
    let has_dyn = !all_single_frame
        && kf.cel.instances().iter().filter(|i| keep(i)).any(|inst| {
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
    iter: &mut std::vec::IntoIter<Arc<FillShape>>,
    cursor: &mut usize,
    range: Range<usize>,
    out: &mut Vec<Arc<FillShape>>,
) {
    for _ in 0..range.start.saturating_sub(*cursor) {
        let _ = iter.next();
    }
    *cursor = range.end;
    out.extend(iter.take(range.len()));
}

impl SceneBaseCache {
    #[inline]
    pub(crate) fn shapes(&self) -> &[Arc<FillShape>] {
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

    /// 次に作り直すとき（`rebuild`）に使い回してよい画面の写しを渡す：画面への変換 `world` で置いた、（レイヤー, 元の地図の
    /// 図形, 画面の写し）の `placed` と、その地図の図形を持つ地図の写し `bases`（書き戻しが同じ写しを使うまで生かす）。
    /// 複製のドラッグ中に、離したときと同じ計算で作った確定した絵（`Session::take_ahead_commit`）。1回の作り直しで捨てる
    pub(crate) fn offer(
        &mut self,
        world: Affine,
        bases: Vec<Arc<LayerBase>>,
        placed: Vec<PlacedCopy>,
    ) {
        self.offered = Some(Offered {
            world: world.as_coeffs().map(f64::to_bits),
            placed,
            _bases: bases,
        });
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
        // 渡された使い回しの写し（`offer`）は、この1回の作り直しで使うか、作り直さないなら捨てる（前は作り直さずに早く戻ると
        // 残った：約束の棚卸し 5616fcc の B の C8）
        let offered = self.offered.take();
        if self.is_fresh(tl, library, world_xform) {
            return false;
        }
        let frame = tl.current_frame;
        let xform_bits = world_xform.as_coeffs().map(f64::to_bits);

        let old_layers = std::mem::take(&mut self.layers);
        let old_flat = std::mem::take(&mut self.flat_shapes);
        let mut old_by_id: HashMap<LayerId, (CachedLayer, Vec<Arc<FillShape>>)> =
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
                    sources: Vec::new(),
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
                        sources: old_cl.sources,
                    });
                    continue;
                }
            }
            let ctx = SymbolEvalCtx::new(library);
            let cel_shapes = layer.evaluate_cel_shapes_at(frame, ctx.clone());
            let bases = ctx.into_state().take_held();
            // 地図の図形の画面の写しは、元の地図の図形が前と同じ物（番地）で、画面への変換も前と同じなら、前の写しを使い
            // 回す（地図の書き戻しは、変わった所の図形だけを新しくし、ほかは前と同じ物を分け合う：`LayerBase` の面の表・
            // 線の表）。新しい図形だけを写して置く（前は、そのレイヤーの図形を全部写して置き直したので、1か所描き足すだけで、
            // 今のコマのそのレイヤーの図形の数に比例して重くなった：レビュー de92ceb の N-6001。設計：claude/画面の図形の
            // 一覧を触った所だけで作る設計_f8b299c.md）
            let mut reused: crate::planar_map::IntMap<usize, Arc<FillShape>> = Default::default();
            let mut donors: Vec<Arc<FillShape>> = Vec::new();
            if let Some((old_cl, old_shapes)) = prev {
                if old_cl.world_xform_bits == xform_bits {
                    reused.reserve(old_shapes.len());
                    for (src, placed) in old_cl.sources.iter().zip(&old_shapes) {
                        if let Some(src) = src {
                            reused.insert(Arc::as_ptr(src) as usize, placed.clone());
                        }
                    }
                    donors = old_shapes;
                }
            }
            if let Some(offered) = &offered {
                if offered.world == xform_bits {
                    for (lid, src, f) in &offered.placed {
                        if *lid == layer.id {
                            reused.insert(Arc::as_ptr(src) as usize, f.clone());
                        }
                    }
                }
            }
            let mut evaluated: Vec<Arc<FillShape>> = Vec::with_capacity(cel_shapes.len());
            let mut sources: Vec<Option<Arc<FillShape>>> = Vec::with_capacity(cel_shapes.len());
            let mut made: Vec<usize> = Vec::new();
            for s in cel_shapes {
                let (src, mut f) = match s {
                    CelShape::Map(src) => {
                        if let Some(placed) = reused.get(&(Arc::as_ptr(&src) as usize)) {
                            evaluated.push(placed.clone());
                            sources.push(Some(src));
                            continue;
                        }
                        let f = (*src).clone();
                        (Some(src), f)
                    }
                    CelShape::Made(f) => (None, *f),
                };
                f.layer_id = layer.id;
                f.place(world_xform);
                made.push(evaluated.len());
                evaluated.push(Arc::new(f));
                sources.push(src);
            }
            reuse_caches_by_key(&mut evaluated, &made, &donors);
            // お手本：使い回して作った図形の並びが、そのレイヤーの図形をみな写して置いた並び（前の作り方）と同じ
            #[cfg(test)]
            if crate::planar_map::test_checks() {
                let mut whole = layer.evaluate_shapes_at(frame, layer.id, SymbolEvalCtx::new(library));
                for s in &mut whole {
                    s.place(world_xform);
                }
                assert!(
                    same_screen_shapes(&evaluated, &sources, &whole),
                    "画面の土台の図形を使い回して作った並びが、みな写して作った並びと違う"
                );
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
                sources,
            });
        }

        self.layers = new_layers;
        self.flat_shapes = new_flat;
        self.layer_order_ids = new_order_ids;
        true
    }
}

/// テスト用（お手本との比べ）：画面の図形の並び `a`（元の地図の図形 `sources`）と `b` が、同じ図形（番号・色・レイヤー・
/// 階層・線・輪郭）の同じ並びか。インスタンスを展開した図形（元の地図の図形が無い）は、展開するたびに新しい番号が付く
/// ので、番号は比べない
#[cfg(test)]
fn same_screen_shapes(a: &[Arc<FillShape>], sources: &[Option<Arc<FillShape>>], b: &[FillShape]) -> bool {
    let same_contour = |x: &crate::vector_engine::Contour, y: &crate::vector_engine::Contour| {
        x.segments[..] == y.segments[..]
    };
    a.len() == b.len()
        && a.iter().zip(sources).zip(b).all(|((x, src), y)| {
            (src.is_none() || x.shape_id == y.shape_id)
                && x.color() == y.color()
                && x.layer_id == y.layer_id
                && x.group_path == y.group_path
                && x.line == y.line
                && x.symbol_instance.is_some() == y.symbol_instance.is_some()
                && same_contour(x.exterior(), y.exterior())
                && x.holes().len() == y.holes().len()
                && x.holes().iter().zip(y.holes()).all(|(h, k)| same_contour(h, k))
        })
}

/// 新しく写して置いた図形（`shapes` の番号 `made`。まだ誰とも分け合っていない）に、`(shape_id, mesh_revision)` が同じ
/// 前の図形 `donors` から描画・当たり判定のキャッシュを引き継ぐ（形が同じで色だけ変わった面など）
fn reuse_caches_by_key(shapes: &mut [Arc<FillShape>], made: &[usize], donors: &[Arc<FillShape>]) {
    if donors.is_empty() || made.is_empty() {
        return;
    }
    let donor_map: HashMap<(ShapeId, u64), &FillShape> = donors
        .iter()
        .map(|d| ((d.shape_id, d.mesh_revision), &**d))
        .collect();
    for &i in made {
        let s = Arc::get_mut(&mut shapes[i]).expect("写したばかりの図形は分け合っていない");
        if let Some(donor) = donor_map.get(&(s.shape_id, s.mesh_revision)) {
            s.reuse_caches_from(donor);
        }
    }
}

/// 画面の写しの鍵：ズーム・パン（ビット）と、画面に見えている範囲（ビット。見えている図形だけを描く段のときだけ：
/// `CelMeshCache::screen_mesh`）と、色の変え方
type CelScreenKey = (u32, u32, u32, Option<[u64; 4]>, CelTint);

/// 組み立てたセル画：ワールド座標の結合メッシュとその図形（何も描かないなら `None`）と、地図の計算に使う写し
type Built = (Option<(egui::Mesh, Vec<Arc<FillShape>>)>, Vec<Arc<crate::planar_map::LayerBase>>);

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
    /// `world_mesh` にした図形（描く順。下から。メッシュの写しは持たない：`Timeline::build_cel_world`）
    shapes: Vec<Arc<FillShape>>,
    /// セル画の地図の計算に使う写し（面と線の図形の表を持つ）。写しを持つ間は生かしておくので、同じセル画を今のコマとして
    /// 描くとき（画面の土台）や、オニオンスキンの反対側に移ったときに、面と線の図形を作り直さない（しまう地図が生きている
    /// 写しを返す：`LayerBase::of_stored`）。持つだけで読まない
    _bases: Vec<Arc<crate::planar_map::LayerBase>>,
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
    /// パン `pan`・色の変え方 `tint` で描く画面座標の結合メッシュ（`Timeline::build_cel_world` のメッシュを画面に写したもの）。
    /// 倍率が高く、図形が持つメッシュでは角が見える段（`mesh_level` が 1 以上）では、画面に見えている範囲 `visible`（ステージの
    /// 座標）にかかる図形だけを、その倍率で描くメッシュ（`FillShape::view_mesh`。編集している図形と同じもの）で結合して写す。
    /// セル画が変わらなければ作り直さず、ズーム・パン・見えている範囲・色の変え方も同じなら写し直さない
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
        visible: kurbo::Rect,
        tint: CelTint,
    ) -> Option<Arc<egui::Mesh>> {
        let idx = self.entry_index(tl, scope, ctx, layer_idx, frame, world_xform)?;
        let entry = &mut self.entries[idx];
        let world_batch = entry.world_mesh.as_ref()?;
        let level = crate::vector_engine::mesh_level(zoom);
        let view_bits = (level > 0).then(|| [visible.x0, visible.y0, visible.x1, visible.y1].map(f64::to_bits));
        let key = (zoom.to_bits(), pan.x.to_bits(), pan.y.to_bits(), view_bits, tint);
        if let Some(s) = entry.screens.iter_mut().find(|s| s.0 == key) {
            s.2 = true;
            return Some(Arc::clone(&s.1));
        }
        let mesh = if level == 0 {
            tinted_screen_mesh(world_batch, zoom, pan, tint)
        } else {
            let mut world = egui::Mesh::default();
            for s in entry
                .shapes
                .iter()
                .filter(|s| crate::vector_engine::rects_overlap(s.bounding_box(), visible))
            {
                crate::vector_engine::append_mesh(&mut world, &s.view_mesh(zoom));
            }
            tinted_screen_mesh(&world, zoom, pan, tint)
        };
        let mesh = Arc::new(mesh);
        // ズーム・パンが変わった古い写しは、同じ色の変え方なら置き換える（パン・ズームの間に写しを溜めない）
        entry.screens.retain(|s| s.0 .4 != tint);
        entry.screens.push((key, Arc::clone(&mesh), true));
        Some(mesh)
    }

    /// 所 `scope` のタイムライン `tl` の、レイヤー `layer_idx` のコマ `frame` のセル画を変換 `world_xform` で置いたとき、点
    /// `pt`（ステージの座標）で見えているもの（色と、線ならその線）：描くのと同じ写し（`screen_mesh`。無ければ作る）の
    /// 図形を、上に描くものから順に、点の内外の判定 `FillShape::contains_point` でたどる
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn seen_at(
        &mut self,
        tl: &Timeline,
        scope: CelScope,
        ctx: SymbolEvalCtx<'_>,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
        pt: Point,
    ) -> Option<(Color32, Option<crate::planar_map::EdgeStroke>)> {
        let idx = self.entry_index(tl, scope, ctx, layer_idx, frame, world_xform)?;
        self.entries[idx]
            .shapes
            .iter()
            .rev()
            .find(|s| s.contains_point(pt))
            .map(|s| (s.color(), s.line))
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
                let built = key.build(tl, scope, world_xform, ctx);
                self.insert(tl, scope, library, &key, built)
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
        for (key, built) in missing.iter().zip(built) {
            self.insert(tl, scope, library, key, built);
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

    /// 鍵 `key` の写しを、作った写し `built`（ワールド座標の結合メッシュと、その図形）で入れて、その番号を返す（同じ所・
    /// レイヤー・評価するコマの古い写しは捨てる）
    fn insert(
        &mut self,
        tl: &Timeline,
        scope: CelScope,
        library: &Library,
        key: &CelKey,
        (built, bases): Built,
    ) -> usize {
        let (world_mesh, shapes) = built.map_or((None, Vec::new()), |(m, s)| (Some(m), s));
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
            shapes,
            _bases: bases,
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

    /// 写し（ワールド座標の結合メッシュと、その図形）を作る
    fn build(
        &self,
        tl: &Timeline,
        scope: CelScope,
        world_xform: Affine,
        ctx: SymbolEvalCtx<'_>,
    ) -> Built {
        // 計算に使う写しを先に手に入れて持つ（組み立てはこの写しを使い、面と線の図形の表がこの写しに残る）
        let bases = tl.layers[self.layer_idx].keyframes[self.kf_idx]
            .cel
            .maps()
            .iter()
            .map(|m| m.base())
            .collect();
        let built = tl.build_cel_world(
            self.layer_idx,
            self.frame,
            world_xform,
            scope.excluded_instance,
            ctx,
        );
        (built, bases)
    }
}
