//! 画面の土台（今のコマの、レイヤーごとの図形と計算に使う地図の写し）・オニオンスキン・シンボル編集中の親の背景のキャッシュ

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Vec2};
use kurbo::Affine;

use super::{Document, EditPath};
use crate::planar_map::LayerBase;
use crate::symbol::{
    compute_resolved_cel_signature, Library, ResolvedCelSig, SymbolEvalCtx, SymbolLoopMode,
};
use crate::timeline::{Layer, Timeline};
use crate::vector_engine::{
    append_mesh, dim_backdrop_color, onion_tint_color, transform_mesh_to_screen, FillShape,
};
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
        kf.start_frame
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
    /// （`Cel::edit_map` が返したもの。作り直すと、セル画の地図の写しとして持ち直す）
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
    pub(crate) fn rebuild(
        &mut self,
        tl: &Timeline,
        library: &Library,
        world_xform: Affine,
    ) -> bool {
        let frame = tl.current_frame;
        let xform_bits = world_xform.as_coeffs().map(f64::to_bits);

        let same_layer_list = self.layers.len() == tl.layers.len()
            && self.layers.iter().zip(&tl.layers).all(|(c, l)| {
                c.matches(
                    l,
                    shown_keyframe_and_eval_frame(l, frame, library),
                    library,
                    xform_bits,
                )
            });
        if same_layer_list {
            return false;
        }

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
            let ctx = SymbolEvalCtx::new(library);
            let mut evaluated = layer.evaluate_shapes_at((frame, frame), layer.id, ctx.clone());
            let bases = ctx.into_state().take_held();
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

type BackdropViewKey = (u32, u32, u32);
type CombinedBackdropMeshCache = Option<(BackdropViewKey, Option<Arc<egui::Mesh>>)>;
/// 親シーンの背景のレイヤー1枚分の指定 `(階層, 親のシンボル ID, 除くインスタンス, 親のワールド変換, その bits, 親のコマ, レイヤーの位置, キーフレームの位置, 評価するコマ)`
type BackdropLayerSpec = (
    usize,
    Option<u64>,
    u64,
    Affine,
    [u64; 6],
    usize,
    usize,
    usize,
    usize,
);

struct CachedBackdropLayer {
    level_idx: usize,
    parent_symbol_id: Option<u64>,
    excluded_instance_gid: u64,
    world_xform_bits: [u64; 6],
    layer_id: LayerId,
    cel_sig: ResolvedCelSig,
    eval_frame: usize,
    shapes: Vec<FillShape>,
    world_mesh: egui::Mesh,
    screen_cache: Option<(BackdropViewKey, egui::Mesh)>,
}

/// シンボルインプレイス編集時の親シーン背景のレイヤー単位キャッシュ
/// （キー：階層・レイヤーID・セルのリビジョン・評価フレーム・参照しているシンボルのリビジョン）
#[derive(Default)]
pub(crate) struct BackdropCache {
    layers: Vec<CachedBackdropLayer>,
    combined_screen_cache: CombinedBackdropMeshCache,
}

impl BackdropCache {
    #[cfg(test)]
    pub(crate) fn layer_count(&self) -> usize {
        self.layers.len()
    }

    pub(crate) fn cached_screen_mesh(
        &mut self,
        doc: &Document,
        edit_path: &EditPath,
        zoom: f32,
        pan: Vec2,
    ) -> Option<Arc<egui::Mesh>> {
        let view_key = (zoom.to_bits(), pan.x.to_bits(), pan.y.to_bits());
        let mut active_specs = Vec::new();
        for lvl_idx in 0..edit_path.levels.len() {
            let lvl = &edit_path.levels[lvl_idx];
            let (parent_sym_id, parent_tl) = if lvl_idx == 0 {
                (None, &doc.root)
            } else {
                let prev_sym_id = edit_path.levels[lvl_idx - 1].symbol_id;
                let Some(sym) = doc.find_symbol(prev_sym_id) else {
                    continue;
                };
                (Some(prev_sym_id), &sym.timeline)
            };
            let parent_world_xform = edit_path.levels[..lvl_idx]
                .iter()
                .fold(Affine::IDENTITY, |acc, l| acc * l.xform_in_parent);
            let xform_bits = parent_world_xform.as_coeffs().map(f64::to_bits);
            let curr_f = lvl
                .parent_frame
                .min(parent_tl.total_frames.saturating_sub(1));
            for layer_idx in 0..parent_tl.layers.len() {
                let layer = &parent_tl.layers[layer_idx];
                if !layer.visible {
                    continue;
                }
                let Some(kf_idx) = layer.keyframe_index_at(curr_f) else {
                    continue;
                };
                let ef =
                    layer_eval_frame(layer, kf_idx, curr_f, &doc.library, Some(lvl.instance_gid));
                active_specs.push((
                    lvl_idx,
                    parent_sym_id,
                    lvl.instance_gid,
                    parent_world_xform,
                    xform_bits,
                    curr_f,
                    layer_idx,
                    kf_idx,
                    ef,
                ));
            }
        }
        self.resolve_and_build(doc, &active_specs, view_key, zoom, pan)
    }

    fn resolve_and_build(
        &mut self,
        doc: &Document,
        active_specs: &[BackdropLayerSpec],
        view_key: (u32, u32, u32),
        zoom: f32,
        pan: Vec2,
    ) -> Option<Arc<egui::Mesh>> {
        let get_tl = |sym_id: Option<u64>| -> &Timeline {
            match sym_id {
                None => &doc.root,
                Some(id) => &doc.find_symbol(id).expect("valid parent symbol").timeline,
            }
        };
        let matches_spec = |cl: &CachedBackdropLayer, spec: &BackdropLayerSpec| -> bool {
            let &(lvl_idx, parent_sym_id, excl_gid, _, xform_bits, _, layer_idx, kf_idx, ef) = spec;
            let layer = &get_tl(parent_sym_id).layers[layer_idx];
            cl.level_idx == lvl_idx
                && cl.parent_symbol_id == parent_sym_id
                && cl.excluded_instance_gid == excl_gid
                && cl.world_xform_bits == xform_bits
                && cl.layer_id == layer.id
                && cl.eval_frame == ef
                && cl
                    .cel_sig
                    .matches_layer_keyframe(layer, kf_idx, &doc.library)
        };

        let all_unchanged = self.layers.len() == active_specs.len()
            && self
                .layers
                .iter()
                .zip(active_specs)
                .all(|(cl, spec)| matches_spec(cl, spec));

        if all_unchanged {
            if let Some((cached_vk, ref cached_res)) = self.combined_screen_cache {
                if cached_vk == view_key {
                    return cached_res.clone();
                }
            }
        } else {
            let mut old_by_key: HashMap<(usize, Option<u64>, LayerId), CachedBackdropLayer> =
                HashMap::with_capacity(self.layers.len());
            for cl in self.layers.drain(..) {
                old_by_key.insert((cl.level_idx, cl.parent_symbol_id, cl.layer_id), cl);
            }

            let mut new_layers = Vec::with_capacity(active_specs.len());
            for spec in active_specs {
                let &(
                    lvl_idx,
                    parent_sym_id,
                    excl_gid,
                    parent_world_xform,
                    xform_bits,
                    curr_f,
                    layer_idx,
                    kf_idx,
                    ef,
                ) = spec;
                let parent_tl = get_tl(parent_sym_id);
                let layer = &parent_tl.layers[layer_idx];
                let old_cl = match old_by_key.remove(&(lvl_idx, parent_sym_id, layer.id)) {
                    Some(old_cl) if matches_spec(&old_cl, spec) => {
                        new_layers.push(old_cl);
                        continue;
                    }
                    old_cl => old_cl,
                };
                let reusable_shapes = old_cl
                    .as_ref()
                    .filter(|old_cl| old_cl.world_xform_bits == xform_bits)
                    .map(|old_cl| old_cl.shapes.as_slice());
                new_layers.push(Self::build_layer(
                    doc,
                    parent_tl,
                    (
                        lvl_idx,
                        parent_sym_id,
                        excl_gid,
                        parent_world_xform,
                        xform_bits,
                    ),
                    (curr_f, layer_idx, kf_idx, ef),
                    reusable_shapes,
                ));
            }
            self.layers = new_layers;
            self.combined_screen_cache = None;
        }

        let mut combined = egui::Mesh::default();
        for cl in &mut self.layers {
            if cl.world_mesh.is_empty() {
                continue;
            }
            let need_transform = cl
                .screen_cache
                .as_ref()
                .is_none_or(|(vk, _)| *vk != view_key);
            if need_transform {
                let screen_m = transform_mesh_to_screen(
                    cl.world_mesh.clone(),
                    zoom,
                    pan,
                    Some(&dim_backdrop_color),
                );
                cl.screen_cache = Some((view_key, screen_m));
            }
            if let Some((_, ref screen_m)) = cl.screen_cache {
                append_mesh(&mut combined, screen_m);
            }
        }
        let res = (!combined.is_empty()).then(|| Arc::new(combined));
        self.combined_screen_cache = Some((view_key, res.clone()));
        res
    }

    fn build_layer(
        doc: &Document,
        parent_tl: &Timeline,
        (lvl_idx, parent_sym_id, excl_gid, parent_world_xform, xform_bits): (
            usize,
            Option<u64>,
            u64,
            Affine,
            [u64; 6],
        ),
        (curr_f, layer_idx, kf_idx, ef): (usize, usize, usize, usize),
        donor_shapes: Option<&[FillShape]>,
    ) -> CachedBackdropLayer {
        let layer = &parent_tl.layers[layer_idx];
        let resolved = parent_tl.evaluate_layer_shapes_at(layer_idx, curr_f, &doc.library);
        let mut shapes = Vec::with_capacity(resolved.len());
        for mut s in resolved {
            if s.symbol_instance
                .is_some_and(|b| b.instance_gid == excl_gid)
            {
                continue;
            }
            if parent_world_xform != Affine::IDENTITY {
                let _ = s.fill_mesh();
                s.transform_with_cached_mesh(parent_world_xform);
            }
            shapes.push(s);
        }
        if let Some(donors) = donor_shapes {
            reuse_caches_by_key(&mut shapes, donors);
        }
        let mut world_mesh = egui::Mesh::default();
        for s in &shapes {
            append_mesh(&mut world_mesh, s.fill_mesh());
        }
        let cel_sig = compute_resolved_cel_signature(layer, kf_idx, Some(excl_gid), &doc.library);
        CachedBackdropLayer {
            level_idx: lvl_idx,
            parent_symbol_id: parent_sym_id,
            excluded_instance_gid: excl_gid,
            world_xform_bits: xform_bits,
            layer_id: layer.id,
            cel_sig,
            eval_frame: ef,
            shapes,
            world_mesh,
            screen_cache: None,
        }
    }
}

struct CachedOnionCel {
    layer_id: LayerId,
    eval_frame: usize,
    world_xform_bits: [u64; 6],
    cel_sig: ResolvedCelSig,
    world_mesh: Option<egui::Mesh>,
}

/// 表示中の前後オニオンスキンフレームのワールド座標結合メッシュキャッシュ
#[derive(Default)]
pub(crate) struct OnionSkinCache {
    entries: Vec<CachedOnionCel>,
}

impl OnionSkinCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(crate) fn retain_active_frames(
        &mut self,
        tl: &Timeline,
        library: &Library,
        world_xform: Affine,
        prev_frames: usize,
        next_frames: usize,
    ) {
        if tl.total_frames == 0 || (prev_frames == 0 && next_frames == 0) {
            self.entries.clear();
            return;
        }
        let xform_bits = world_xform.as_coeffs().map(f64::to_bits);
        let curr_f = tl.current_frame;
        let min_f = curr_f.saturating_sub(prev_frames);
        let max_f = (curr_f + next_frames).min(tl.total_frames.saturating_sub(1));
        self.entries.retain(|e| {
            e.world_xform_bits == xform_bits
                && tl.layers.iter().any(|layer| {
                    layer.visible
                        && layer.id == e.layer_id
                        && (min_f..=max_f).any(|f| {
                            layer.keyframe_index_at(f).is_some_and(|kf_idx| {
                                layer_eval_frame(layer, kf_idx, f, library, None) == e.eval_frame
                            })
                        })
                })
        });
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cached_screen_mesh(
        &mut self,
        tl: &Timeline,
        ctx: SymbolEvalCtx<'_>,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
        zoom: f32,
        pan: Vec2,
        (tint_rgb, alpha): ([u8; 3], u8),
    ) -> Option<Arc<egui::Mesh>> {
        let layer = tl.layers.get(layer_idx)?;
        let kf_idx = layer.keyframe_index_at(frame)?;
        if layer.keyframes[kf_idx].cel.is_empty() {
            return None;
        }
        let library = ctx.library;
        let ef = layer_eval_frame(layer, kf_idx, frame, library, None);
        let xform_bits = world_xform.as_coeffs().map(f64::to_bits);
        let hit_idx = self.entries.iter().position(|e| {
            e.layer_id == layer.id
                && e.eval_frame == ef
                && e.world_xform_bits == xform_bits
                && e.cel_sig.matches_layer_keyframe(layer, kf_idx, library)
        });
        let idx = if let Some(i) = hit_idx {
            i
        } else {
            self.entries
                .retain(|e| !(e.layer_id == layer.id && e.eval_frame == ef));
            let cel_sig = compute_resolved_cel_signature(layer, kf_idx, None, library);
            let world_mesh = tl.build_cel_world_batch_mesh(layer_idx, frame, world_xform, ctx);
            self.entries.push(CachedOnionCel {
                layer_id: layer.id,
                eval_frame: ef,
                world_xform_bits: xform_bits,
                cel_sig,
                world_mesh,
            });
            self.entries.len() - 1
        };
        let world_batch = self.entries[idx].world_mesh.as_ref()?;
        Some(Arc::new(transform_mesh_to_screen(
            world_batch.clone(),
            zoom,
            pan,
            Some(&|c| onion_tint_color(c, tint_rgb, alpha)),
        )))
    }
}
