//! 浮いた片（矩形で切った片・つかんで動かす片・貼り付けた片）。設計：claude/浮いた片を地図で持つ設計_2-2c_6c167ae.md
//!
//! 本当の形はレイヤーごとの地図（文書の座標）：残りの地図（持ち上げたレイヤーの、持ち上げた所を除いた地図）と、片の組
//! （片の地図と、それにかける変換）。画面・選択・当たり判定の図形は、地図から作る写し（`Floating::refresh`）。

use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::Color32;
use kurbo::Affine;

use crate::canvas_tool::{Placement, SceneBaseCache};
use crate::planar_map::{
    cut_out, faces_with_ids, per_layer_parallel, realized, region_map, stacked, CheckedMap,
    LayerBase, MapChange, MapInput, MapRule, PlanarMap,
};
use crate::stored_map::StoredMap;
use crate::vector_engine::FillShape;
use crate::vector_engine::{LayerId, ShapeId};

/// 浮いた片の操作に要る画面の土台：画面に出しているセル画（`SceneBaseCache`）と編集中の階層
#[derive(Clone, Copy)]
pub(crate) struct Stage<'a> {
    pub(crate) base: &'a SceneBaseCache,
    pub(crate) scope: &'a [u64],
}

impl Stage<'_> {
    /// レイヤー `layer_id` の、編集中の階層の地図（セル画の地図。生の図形はその面：`LayerBase::faces`）。`None`：画面に
    /// 出していないレイヤー
    fn cel(&self, layer_id: LayerId) -> Option<Arc<LayerBase>> {
        Some(self.base.cel(layer_id)?.scope_base(self.scope))
    }
}

/// 片の組：片の地図（しまう形。文書の座標。面の辺の側に片の番号を記してある）と、それにかける変換（文書の座標で。動かす・
/// 回す・拡大を重ねる。確定するまで地図にはかけない）
#[derive(Clone, Debug)]
pub(crate) struct PieceGroup {
    map: StoredMap,
    /// 計算に使う写し（浮いている間だけ持つ。Undo の控えでは外す：`Floating::strip_transient_caches`）
    held: Option<Arc<LayerBase>>,
    /// 片の図形（文書の座標、変換をかける前。番号は面の番号）
    faces: Arc<Vec<FillShape>>,
    pub(crate) xform: Affine,
}

impl PieceGroup {
    /// 変換をかけて置いた地図（その座標の格子で正しい地図にしたもの：`realized`）。`None`：格子に置けない
    fn placed(&self) -> Option<PlanarMap> {
        Some(realized(&self.base().map, self.xform)?.into_map())
    }

    /// 地図 `map`（文書の座標）の片の組。面の番号が1つに決まらない面には、新しい番号を振る（`faces_with_ids`）
    fn new(map: &PlanarMap, layer_id: LayerId, scope: &[u64], xform: Affine) -> Self {
        let (map, faces) = faces_with_ids(map, layer_id, scope);
        let base = LayerBase::of_stored(&StoredMap::from_dense(&map, None));
        Self {
            map: base.stored.clone(),
            held: Some(base),
            faces: Arc::new(faces),
            xform,
        }
    }

    /// 片の地図の、計算に使う写し
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        LayerBase::of_stored(&self.map)
    }

    fn ids(&self) -> impl Iterator<Item = ShapeId> + '_ {
        self.faces.iter().map(|f| f.shape_id)
    }

    /// 最初の片の番号
    pub(crate) fn first_id(&self) -> Option<ShapeId> {
        self.faces.first().map(|f| f.shape_id)
    }

    /// 番号が `keep` にある片だけの組（1つも無ければ `None`）
    fn restricted(
        &self,
        keep: &HashSet<ShapeId>,
        layer_id: LayerId,
        scope: &[u64],
    ) -> Option<Self> {
        if self.ids().all(|id| keep.contains(&id)) {
            return Some(self.clone());
        }
        if !self.ids().any(|id| keep.contains(&id)) {
            return None;
        }
        Some(Self::new(
            &self.base().map.restricted(keep),
            layer_id,
            scope,
            self.xform,
        ))
    }
}

/// 持ち上げたレイヤーの残り
#[derive(Clone, Debug)]
pub(crate) struct Origin {
    /// 残りの地図（しまう形。セル画の地図から、持ち上げた所を除いた地図。文書の座標。変わらない塊はセル画の地図と共有する）
    rest: StoredMap,
    /// 残りの地図の計算に使う写し（浮いている間だけ持つ。Undo の控えでは外す）
    held: Option<Arc<LayerBase>>,
    /// 隠すセル画の図形（セル画の地図の面のうち、残りの地図で変わったもの）
    hidden: Vec<ShapeId>,
    /// 残りの図形（残りの地図の面のうち、セル画に無いもの。文書の座標）
    remainder: Vec<FillShape>,
}

impl Origin {
    /// セル画の地図 `cel` との差分 `change`（残りの地図。文書の座標）から。差分が無ければ（何も切れていない）`None`
    fn of(change: MapChange, cel: &StoredMap) -> Option<Self> {
        if change.removed.is_empty() && change.added.is_empty() {
            return None;
        }
        let base = LayerBase::of_stored(&StoredMap::from_dense(change.map.map(), Some(cel)));
        Some(Self {
            rest: base.stored.clone(),
            held: Some(base),
            hidden: change.removed,
            remainder: change.added,
        })
    }

    /// 残りの地図の、計算に使う写し
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        LayerBase::of_stored(&self.rest)
    }
}

/// 1つのレイヤーの浮いた片（編集中の階層）
#[derive(Clone, Debug)]
pub(crate) struct FloatLayer {
    pub(crate) layer_id: LayerId,
    pub(crate) origin: Option<Origin>,
    /// 片の組（下から順）
    pub(crate) groups: Vec<PieceGroup>,
}

impl FloatLayer {
    fn is_empty(&self) -> bool {
        self.origin.is_none() && self.groups.is_empty()
    }

    /// 中身の地図：残りの地図か、無ければセル画の地図 `cel`
    fn content(&self, cel: &Arc<LayerBase>) -> Arc<LayerBase> {
        self.origin
            .as_ref()
            .map_or_else(|| cel.clone(), Origin::base)
    }

    /// 片の組を下から順に重ねた地図（組ごとに変換をかけて、その座標の格子で正しい地図にする：`realized`）。片が無ければ
    /// 空の地図。`None`：格子に置けない座標がある
    pub(crate) fn pieces_map(&self) -> Option<PlanarMap> {
        stacked(
            PlanarMap::default(),
            self.groups.iter().map(PieceGroup::placed),
        )
    }

    /// 選んだもの（番号が `ids` の、中身の地図の面と片）の地図：中身の地図の選んだ面に、選んだ片の組を下から順に重ねる
    /// （確定と同じ重ね方。新しい中身。確かめた地図：`checked`）。`None`：画面に出していないレイヤーか、格子に置けない
    /// 座標があるか、正しい地図にならない
    pub(crate) fn selected_map(
        &self,
        stage: Stage<'_>,
        ids: &HashSet<ShapeId>,
    ) -> Option<CheckedMap> {
        let start = self.content(&stage.cel(self.layer_id)?).map.restricted(ids);
        let groups = self
            .groups
            .iter()
            .filter_map(|g| g.restricted(ids, self.layer_id, stage.scope));
        stacked(start, groups.map(|g| g.placed()))?.checked()
    }

    /// 確定した地図：中身の地図に、片の組を下から順に重ねる（組ごとに変換をかけて、その座標の格子で正しい地図にする：
    /// `realized`）。`None`：格子に置けない座標がある
    pub(crate) fn committed(&self, cel: &Arc<LayerBase>) -> Option<PlanarMap> {
        stacked(
            self.content(cel).map.clone(),
            self.groups.iter().map(PieceGroup::placed),
        )
    }
}

/// 浮いた片。写し（`pieces`・`remainder`・`hidden`）は、地図が変わるたびに `refresh` で作り直す
#[derive(Clone, Debug)]
pub(crate) struct Floating {
    layers: Vec<FloatLayer>,
    /// 動かした・変形したか、文書に無い片があるか。`true` なら確定のときに重ねる。`false` は動かしていない矩形選択の
    /// 切り口で、捨てれば元に戻る
    pub(crate) modified: bool,
    /// 片の図形（ステージの写し）
    pieces: Vec<FillShape>,
    /// 残りの図形（ステージの写し）と、その置き場所
    remainder: Vec<(Placement, FillShape)>,
    /// 画面から隠すセル画の図形
    hidden: HashSet<ShapeId>,
}

impl Floating {
    pub(crate) fn new(modified: bool) -> Self {
        Self {
            layers: Vec::new(),
            modified,
            pieces: Vec::new(),
            remainder: Vec::new(),
            hidden: HashSet::new(),
        }
    }

    /// 片の図形（ステージの写し）
    pub(crate) fn pieces(&self) -> &[FillShape] {
        &self.pieces
    }

    /// 残りの図形（ステージの写し）
    pub(crate) fn remainder(&self) -> impl Iterator<Item = &FillShape> {
        self.remainder.iter().map(|r| &r.1)
    }

    /// 残りの図形と、その置き場所
    pub(crate) fn placed_remainder(&self) -> &[(Placement, FillShape)] {
        &self.remainder
    }

    /// 画面から隠すセル画の図形
    pub(crate) fn hidden(&self) -> &HashSet<ShapeId> {
        &self.hidden
    }

    pub(crate) fn layers(&self) -> &[FloatLayer] {
        &self.layers
    }

    /// 片の番号
    pub(crate) fn piece_ids(&self) -> HashSet<ShapeId> {
        self.pieces.iter().map(|p| p.shape_id).collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// 写しを作り直す：隠す図形、残りの図形（そのレイヤーの最初の隠す図形の所に置く）、片の図形（片の地図の面を
    /// 「ステージへの変換 `world` ∘ 片の変換」で置いた写し）。レイヤーは重ね順 `order` に並べ、空のレイヤーは外す
    pub(crate) fn refresh(&mut self, world: Affine, scope: &[u64], order: &[LayerId]) {
        self.layers.retain(|l| !l.is_empty());
        self.layers.sort_by_key(|l| {
            order
                .iter()
                .position(|&id| id == l.layer_id)
                .unwrap_or(usize::MAX)
        });
        self.hidden.clear();
        self.remainder.clear();
        self.pieces.clear();
        for l in &self.layers {
            if let Some(o) = &l.origin {
                self.hidden.extend(o.hidden.iter().copied());
                let at = o.hidden.first().map_or_else(
                    || Placement::TopOfLayer(l.layer_id, scope.to_vec()),
                    |&id| Placement::ReplaceAt(id),
                );
                for r in &o.remainder {
                    let mut s = r.clone();
                    s.place(world);
                    self.remainder.push((at.clone(), s));
                }
            }
            for g in &l.groups {
                for f in g.faces.iter() {
                    let mut s = f.clone();
                    if g.xform != Affine::IDENTITY {
                        s.transform(g.xform);
                    }
                    s.place(world);
                    self.pieces.push(s);
                }
            }
        }
    }

    /// Undo の控えにする形にする：控えが持つのは地図（しまう形）と変換だけ。計算に使う写しと、画面の写し（片・残り・
    /// 隠す図形）は外す（戻すときは `Session::set_floating` が `refresh` で作り直す）
    pub(crate) fn strip_transient_caches(&mut self) {
        for l in &mut self.layers {
            if let Some(o) = &mut l.origin {
                o.held = None;
            }
            for g in &mut l.groups {
                g.held = None;
            }
        }
        self.pieces = Vec::new();
        self.remainder = Vec::new();
        self.hidden = HashSet::new();
    }

    /// 直前の履歴スナップショットと同じ中身か（地図は同じ `Arc`、変換は同じ値）
    pub(crate) fn matches_undo_snapshot(&self, other: &Self) -> bool {
        self.modified == other.modified
            && self.layers.len() == other.layers.len()
            && self.layers.iter().zip(&other.layers).all(|(a, b)| {
                a.layer_id == b.layer_id
                    && match (&a.origin, &b.origin) {
                        (None, None) => true,
                        (Some(x), Some(y)) => x.rest.id() == y.rest.id(),
                        _ => false,
                    }
                    && a.groups.len() == b.groups.len()
                    && a.groups
                        .iter()
                        .zip(&b.groups)
                        .all(|(g, h)| g.map.id() == h.map.id() && g.xform == h.xform)
            })
    }

    /// テスト用：レイヤーごとの（レイヤー, 残りの地図と写しを持つか, 片の組の地図と写しを持つか）
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub(crate) fn stored_maps(
        &self,
    ) -> Vec<(LayerId, Option<(&StoredMap, bool)>, Vec<(&StoredMap, bool)>)> {
        self.layers
            .iter()
            .map(|l| {
                (
                    l.layer_id,
                    l.origin.as_ref().map(|o| (&o.rest, o.held.is_some())),
                    l.groups
                        .iter()
                        .map(|g| (&g.map, g.held.is_some()))
                        .collect(),
                )
            })
            .collect()
    }

    fn layer_mut(&mut self, layer_id: LayerId) -> &mut FloatLayer {
        let i = match self.layers.iter().position(|l| l.layer_id == layer_id) {
            Some(i) => i,
            None => {
                self.layers.push(FloatLayer {
                    layer_id,
                    origin: None,
                    groups: Vec::new(),
                });
                self.layers.len() - 1
            }
        };
        &mut self.layers[i]
    }

    /// 矩形で選ぶ・持ち上げる：レイヤーごと（レイヤーと、その中身のうち選ぶ面の番号）に、選ぶ所で中身を切り、中を片の
    /// 組にする（`cut_layer`）。レイヤーをまたがないので、同時に計算する。戻り値は、地図にできなかった（格子に置けない
    /// 座標がある）レイヤー
    pub(crate) fn cut(
        &mut self,
        stage: Stage<'_>,
        layers: &[(LayerId, HashSet<ShapeId>)],
        rect: Option<&FillShape>,
    ) -> Vec<LayerId> {
        let modified = self.modified;
        let current = &self.layers;
        let results: Vec<Option<FloatLayer>> = per_layer_parallel(layers, |(layer_id, ids)| {
            let cur = current.iter().find(|l| l.layer_id == *layer_id);
            cut_layer(cur, modified, stage, *layer_id, ids, rect)
        });
        let mut failed = Vec::new();
        for ((layer_id, _), r) in layers.iter().zip(results) {
            match r {
                Some(l) => *self.layer_mut(*layer_id) = l,
                None => failed.push(*layer_id),
            }
        }
        failed
    }

    /// すべての片の組に文書の座標の変換 `doc` を重ねても、確定で地図にできるか（確定と同じ式：片の組の地図に、重ねた
    /// 変換をかけた点がみな格子に置ける：`PlanarMap::fits_grid`）
    pub(crate) fn fits_transform(&self, doc: Affine) -> bool {
        self.layers
            .iter()
            .flat_map(|l| &l.groups)
            .all(|g| g.base().map.fits_grid(doc * g.xform))
    }

    /// 動かす・回す・拡大：すべての片の組に、文書の座標の変換 `doc` を重ねる（重ねてよいかは先に `fits_transform` で
    /// 確かめる）
    pub(crate) fn transform(&mut self, doc: Affine) {
        for g in self.layers.iter_mut().flat_map(|l| &mut l.groups) {
            g.xform = doc * g.xform;
        }
        self.modified = true;
    }

    /// レイヤー `layers` の片のうち、番号が `ids` の片の色を `color` にする（同じ色でとなり合う片は1つになる）
    pub(crate) fn recolor(
        &mut self,
        ids: &HashSet<ShapeId>,
        color: Color32,
        layers: &HashSet<LayerId>,
        scope: &[u64],
    ) -> bool {
        let mut any = false;
        for l in self
            .layers
            .iter_mut()
            .filter(|l| layers.contains(&l.layer_id))
        {
            let layer_id = l.layer_id;
            for g in &mut l.groups {
                if g.faces
                    .iter()
                    .any(|f| ids.contains(&f.shape_id) && f.color != color)
                {
                    *g = PieceGroup::new(
                        &g.base().map.recolored(ids, color),
                        layer_id,
                        scope,
                        g.xform,
                    );
                    any = true;
                }
            }
        }
        if any {
            self.modified = true;
        }
        any
    }

    /// レイヤー `layer_id` の組 `group` の片の地図（文書の座標、変換をかける前）を `edit` で編集する。`false`：その組が
    /// 無いか、編集できない
    pub(crate) fn edit_group(
        &mut self,
        layer_id: LayerId,
        group: usize,
        scope: &[u64],
        edit: impl FnOnce(&PlanarMap) -> Option<PlanarMap>,
    ) -> bool {
        let Some(g) = self
            .layers
            .iter_mut()
            .find(|l| l.layer_id == layer_id)
            .and_then(|l| l.groups.get_mut(group))
        else {
            return false;
        };
        let Some(next) = edit(&g.base().map) else {
            return false;
        };
        *g = PieceGroup::new(&next, layer_id, scope, g.xform);
        true
    }

    /// 動かしていない片のうち、番号が `keep` に無い片を残りに戻す：外した片を残りの地図に重ねる（切り口は両側で同じ線
    /// なので、同じ色でつながる所は1つになる：`cut_out`）。片が無くなったレイヤーは外す（何も変わらない）。`false`：地図にできない
    pub(crate) fn return_pieces(&mut self, stage: Stage<'_>, keep: &HashSet<ShapeId>) -> bool {
        debug_assert!(!self.modified);
        let drop: HashSet<ShapeId> = self
            .layers
            .iter()
            .flat_map(|l| l.groups.iter().flat_map(|g| g.ids()))
            .filter(|id| !keep.contains(id))
            .collect();
        for l in &mut self.layers {
            let layer_id = l.layer_id;
            let kept: Vec<PieceGroup> = l
                .groups
                .iter()
                .filter_map(|g| g.restricted(keep, layer_id, stage.scope))
                .collect();
            if kept.is_empty() {
                l.origin = None;
                l.groups.clear();
                continue;
            }
            let back: Vec<PieceGroup> = l
                .groups
                .iter()
                .filter_map(|g| g.restricted(&drop, layer_id, stage.scope))
                .collect();
            if back.is_empty() {
                continue;
            }
            let Some(cel) = stage.cel(layer_id) else {
                return false;
            };
            let mut rest = l.content(&cel).map.clone();
            for g in &back {
                let Some(m) = rest.overlay(MapInput::Map(&g.base().map), MapRule::Overlay) else {
                    return false;
                };
                rest = m;
            }
            let faces = cel.faces(stage.scope);
            let own: Vec<&FillShape> = faces.iter().map(|s| &**s).collect();
            let Some(change) = cel.diff(&rest, &own, layer_id, stage.scope) else {
                return false;
            };
            l.origin = Origin::of(change, &cel.stored);
            l.groups = kept;
        }
        true
    }

    /// 番号が `keep` に無い片と、残りを取り出す（確定する分）。この浮いた片には、残す片だけが残る（残りは確定するので
    /// 持たない）
    pub(crate) fn split_off(&mut self, keep: &HashSet<ShapeId>, scope: &[u64]) -> Floating {
        let drop: HashSet<ShapeId> = self
            .layers
            .iter()
            .flat_map(|l| l.groups.iter().flat_map(|g| g.ids()))
            .filter(|id| !keep.contains(id))
            .collect();
        let mut out = Floating::new(true);
        for l in &mut self.layers {
            let layer_id = l.layer_id;
            out.layers.push(FloatLayer {
                layer_id,
                origin: l.origin.take(),
                groups: l
                    .groups
                    .iter()
                    .filter_map(|g| g.restricted(&drop, layer_id, scope))
                    .collect(),
            });
            l.groups = l
                .groups
                .iter()
                .filter_map(|g| g.restricted(keep, layer_id, scope))
                .collect();
        }
        self.layers.retain(|l| !l.is_empty());
        out.layers.retain(|l| !l.is_empty());
        out
    }

    /// 地図 `map`（文書の座標。面の番号が片の番号）を、変換 `xform` をかける片の組として、レイヤー `layer_id` の一番上に
    /// 足す（変換は、動かした片と同じく確定のときに地図にかける）
    pub(crate) fn add_map(
        &mut self,
        stage: Stage<'_>,
        layer_id: LayerId,
        map: &PlanarMap,
        xform: Affine,
    ) {
        let group = PieceGroup::new(map, layer_id, stage.scope, xform);
        self.layer_mut(layer_id).groups.push(group);
        self.modified = true;
    }

    /// 片を捨てる（削除：残りだけを確定する）
    pub(crate) fn drop_pieces(&mut self) {
        for l in &mut self.layers {
            l.groups.clear();
        }
    }
}

/// 1つのレイヤーの切り出し：中身（残りの地図か、セル画の地図）を、選ぶ所（番号が `ids` の中身の面の所と、形 `rect` の
/// 所）で1回切り、中を片の組にする（`cut_out`。中と外は同じ地図を同じ所で切るので、切り口は両側で同じ線）。矩形が無い
/// とき（選んだ面を丸ごと持ち上げる）は切らずに、中身からその面を外す（番号はそのまま）。動かして
/// いない片（`modified` でないとき）は元の所にあるので、新しい片を前からの片に重ねて1つの組にする（切り口は同じ線
/// なので、同じ色でつながる片は1つになる）。動かした片があれば、新しい片は別の組として一番下に足す。`None`：地図に
/// できない
fn cut_layer(
    cur: Option<&FloatLayer>,
    modified: bool,
    stage: Stage<'_>,
    layer_id: LayerId,
    ids: &HashSet<ShapeId>,
    rect: Option<&FillShape>,
) -> Option<FloatLayer> {
    let cel = stage.cel(layer_id)?;
    let faces = cel.faces(stage.scope);
    let own: Vec<&FillShape> = faces.iter().map(|s| &**s).collect();
    let empty = FloatLayer {
        layer_id,
        origin: None,
        groups: Vec::new(),
    };
    let cur = cur.unwrap_or(&empty);
    let content_base = cur.content(&cel);
    let content = &content_base.map;
    let part = (!ids.is_empty())
        .then(|| content.restricted(ids))
        .filter(|m| !m.edges.is_empty());
    let (rest, inside) = match (part, rect) {
        (None, None) => return Some(cur.clone()),
        // 面を丸ごと持ち上げる：中身から、その面を外す（面の番号はそのまま。片は前と同じ番号の図形）
        (Some(part), None) => (
            cel.diff(&content.without(ids), &own, layer_id, stage.scope)?,
            part,
        ),
        (part, Some(r)) => {
            let region_owned;
            let region = match part {
                None => MapInput::Shapes(std::slice::from_ref(r)),
                Some(p) => {
                    region_owned = region_map(&p, rect)?;
                    MapInput::Map(&region_owned)
                }
            };
            let (rest, inside) = cut_out(&cel, &own, content, region, layer_id, stage.scope)?;
            (rest, inside.map.into_map())
        }
    };
    let mut groups = cur.groups.clone();
    if !inside.edges.is_empty() {
        match groups.first_mut() {
            Some(g) if !modified => {
                let joined = g
                    .base()
                    .map
                    .overlay(MapInput::Map(&inside), MapRule::Overlay)?;
                *g = PieceGroup::new(&joined, layer_id, stage.scope, Affine::IDENTITY);
            }
            _ => groups.insert(
                0,
                PieceGroup::new(&inside, layer_id, stage.scope, Affine::IDENTITY),
            ),
        }
    }
    Some(FloatLayer {
        layer_id,
        origin: Origin::of(rest, &cel.stored),
        groups,
    })
}
