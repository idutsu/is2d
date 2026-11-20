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
    cut_out, parallel_map, realized, region_map, stacked, CheckedMap, FaceKeep, LayerBase,
    LocalEdit, LocalWrite, MapInput, MapRule, PlanarMap,
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
/// 回す・拡大を重ねる。確定するまで地図にはかけない）。片の図形（画面の写し）は持たず、計算に使う写しの面から作る
/// （`Floating::refresh`。Undo の控えに入るのは地図と変換だけ：レビュー 6ee325a の N-2005）
#[derive(Clone, Debug)]
pub(crate) struct PieceGroup {
    map: StoredMap,
    /// 計算に使う写し（浮いている間だけ持つ。Undo の控えでは外す：`Floating::strip_transient_caches`）
    held: Option<Arc<LayerBase>>,
    pub(crate) xform: Affine,
}

impl PieceGroup {
    /// 変換をかけて置いた地図（その座標の格子で正しい地図にしたもの：`realized`）。片の地図は作ったときに確かめてある
    /// （`new` は確かめた地図だけを受け取る）ので、変換が無ければ、そのまま使う。`None`：格子に置けない
    fn placed(&self) -> Option<PlanarMap> {
        let base = self.base();
        if self.xform == Affine::IDENTITY {
            return Some(base.map().clone());
        }
        Some(realized(base.map(), self.xform)?.into_map())
    }

    /// 確かめた地図 `map`（文書の座標）の片の組。面ごとに1つずつ別の番号を記す（番号が1つに決まらない面には、新しい
    /// 番号：`uniquely_labeled`）。片の地図を確かめるのは、それを作った所の1回だけ（レビュー 6ee325a の N-2004）
    fn new(map: CheckedMap, xform: Affine) -> Self {
        let base = LayerBase::of_stored(&StoredMap::from_dense(map.uniquely_labeled().map(), None));
        Self {
            map: base.stored.clone(),
            held: Some(base),
            xform,
        }
    }

    /// 片の地図の、計算に使う写し
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        LayerBase::of_stored(&self.map)
    }

    /// 計算に使う写しを持って返す（Undo の控えから戻したときは持っていないので、作って持つ）
    fn hold(&mut self) -> Arc<LayerBase> {
        self.held
            .get_or_insert_with(|| LayerBase::of_stored(&self.map))
            .clone()
    }

    /// 片の番号（面の辺の側に記した番号。同じ番号をくり返す）
    fn ids(&self) -> Vec<ShapeId> {
        self.base().map().labels().collect()
    }

    /// 片の番号のどれか1つ（画面の図形の並びで、この組の片を探す手がかり）
    pub(crate) fn first_id(&self) -> Option<ShapeId> {
        self.base().map().labels().next()
    }

    /// 置いた地図（変換をかけたもの）から、地図の並び `cover`（文書の座標）の色のある所を消した組（変換は恒等）。重なら
    /// なければ、この組のまま。`Ok(None)`：片が残らない。`Err`：格子に置けない・正しい地図にならない
    fn erased(&self, cover: &[PlanarMap]) -> Result<Option<Self>, Unplaceable> {
        let overlaps = |r: kurbo::Rect| {
            cover.iter().filter_map(PlanarMap::bounding_box).any(|c| {
                let i = r.intersect(c);
                i.width() > 0.0 && i.height() > 0.0
            })
        };
        match self.base().map().bounding_box() {
            Some(r) if overlaps(self.xform.transform_rect_bbox(r)) => {}
            _ => return Ok(Some(self.clone())),
        }
        let mut map = self.placed().ok_or(Unplaceable)?;
        for c in cover {
            map = map
                .overlay(MapInput::Map(c), MapRule::Paint(None))
                .ok_or(Unplaceable)?;
        }
        if map.edges.is_empty() {
            return Ok(None);
        }
        let map = map.checked().ok_or(Unplaceable)?;
        Ok(Some(Self::new(map, Affine::IDENTITY)))
    }

    /// 番号が `keep` にある片だけの組（`Ok(None)`：1つも無い。`Err`：正しい地図にならない）
    fn restricted(&self, keep: &HashSet<ShapeId>) -> Result<Option<Self>, Unplaceable> {
        let ids = self.ids();
        if ids.iter().all(|id| keep.contains(id)) {
            return Ok(Some(self.clone()));
        }
        if !ids.iter().any(|id| keep.contains(id)) {
            return Ok(None);
        }
        let map = self
            .base()
            .map()
            .restricted(keep)
            .checked()
            .ok_or(Unplaceable)?;
        Ok(Some(Self::new(map, self.xform)))
    }
}

/// 片の地図が、格子の上で正しい地図にならない（片の操作を断る）
#[derive(Debug)]
pub(crate) struct Unplaceable;

/// 片の組 `groups` のうち、番号が `keep` にある片だけの組（片の無くなった組は外す。`Err`：どれかが正しい地図にならない）
fn restricted_groups(
    groups: &[PieceGroup],
    keep: &HashSet<ShapeId>,
) -> Result<Vec<PieceGroup>, Unplaceable> {
    let mut out = Vec::new();
    for g in groups {
        out.extend(g.restricted(keep)?);
    }
    Ok(out)
}

/// テスト用：図形の形の鍵（曲線の座標のビットと色。番号によらない）
#[cfg(test)]
type ShapeKey = (Vec<u64>, [u8; 4]);

/// 持ち上げたレイヤーの残り
#[derive(Clone, Debug)]
pub(crate) struct Origin {
    /// 残りの地図（しまう形。セル画の地図から、持ち上げた所を除いた地図。文書の座標。変わらない塊はセル画の地図と共有する）
    rest: StoredMap,
    /// 残りの地図の計算に使う写し（浮いている間だけ持つ。Undo の控えでは外す）
    held: Option<Arc<LayerBase>>,
    /// 隠すセル画の図形（セル画の地図の面のうち、残りの地図で変わったもの）
    hidden: Vec<ShapeId>,
    /// 残りの図形の番号（残りの地図の面のうち、セル画に無いもの。図形は計算に使う写しの面から作る：`Floating::refresh`）
    remainder: Vec<ShapeId>,
}

impl Origin {
    /// テスト用（お手本の前の道）：セル画の地図 `cel` との差分 `change`（残りの地図。文書の座標）から。差分が無ければ
    /// （何も切れていない）`None`
    #[cfg(test)]
    fn of(change: crate::planar_map::MapChange, cel: &StoredMap) -> Option<Self> {
        if change.removed.is_empty() && change.added.is_empty() {
            return None;
        }
        // 残りの地図はセル画の地図と塊を共有するので、升目の索引も、セル画の写しの索引から変わった所だけ作る
        let stored = StoredMap::from_dense(change.map.map(), Some(cel));
        let base = match LayerBase::live(cel) {
            Some(prev) => LayerBase::of_stored_after(&stored, &prev),
            None => LayerBase::of_stored(&stored),
        };
        Some(Self {
            rest: base.stored.clone(),
            held: Some(base),
            hidden: change.removed,
            remainder: change.added.iter().map(|s| s.shape_id).collect(),
        })
    }

    /// セル画の地図 `cel` を残りの地図にする1本の道の答え `w`（`LayerBase::replace_local`）から：消した図形が隠す図形、
    /// 入れた図形が残りの図形。升目の索引は、セル画の写しから変わった所だけ作る。答えが変わらない（セル画の地図のまま）
    /// ときは、使う側が残りを持たない
    fn of_local(w: LocalWrite, cel: &LayerBase, scope: &[u64]) -> Self {
        let base = LayerBase::of_local(w.stored, scope, w.faces, cel);
        Self {
            rest: base.stored.clone(),
            held: Some(base),
            hidden: w.removed,
            remainder: w.added,
        }
    }

    /// テスト用（お手本との比べ）：隠す図形と、残りの図形の形・残りの地図の面の形（新しく振る番号によらない形）
    #[cfg(test)]
    fn compare_key(&self, scope: &[u64]) -> (Vec<ShapeId>, Vec<ShapeKey>, Vec<ShapeKey>) {
        let key = |s: &FillShape| {
            let bits: Vec<u64> = s
                .contours()
                .flat_map(|c| c.segments.iter())
                .flat_map(|q| [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y].map(f64::to_bits))
                .collect();
            (bits, s.color.to_array())
        };
        let faces = self.base().faces(scope);
        let rem: HashSet<ShapeId> = self.remainder.iter().copied().collect();
        let mut remainder: Vec<_> = faces
            .iter()
            .filter(|f| rem.contains(&f.shape_id))
            .map(|f| key(f))
            .collect();
        remainder.sort();
        let mut all: Vec<_> = faces.iter().map(|f| key(f)).collect();
        all.sort();
        (self.hidden.clone(), remainder, all)
    }

    /// 残りの地図の、計算に使う写し
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        LayerBase::of_stored(&self.rest)
    }

    /// 計算に使う写しを持って返す（Undo の控えから戻したときは持っていないので、作って持つ）
    fn hold(&mut self) -> Arc<LayerBase> {
        self.held
            .get_or_insert_with(|| LayerBase::of_stored(&self.rest))
            .clone()
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
        let start = self
            .content(&stage.cel(self.layer_id)?)
            .map()
            .restricted(ids);
        let groups = restricted_groups(&self.groups, ids).ok()?;
        stacked(start, groups.iter().map(PieceGroup::placed))?.checked()
    }

    #[cfg(test)]
    /// 確定した地図：中身の地図に、片の組を下から順に重ねる（組ごとに変換をかけて、その座標の格子で正しい地図にする：
    /// `realized`）。`None`：格子に置けない座標がある
    pub(crate) fn committed(&self, cel: &Arc<LayerBase>) -> Option<PlanarMap> {
        self.content(cel)
            .stacked(self.groups.iter().map(PieceGroup::placed))
    }

    /// 確定した地図のしまう形（`committed` と同じ答え。片の組を1枚ずつ窓で重ねる：`LayerBase::stacked_local`。中身の
    /// 地図と塊を共有する）。`None`：格子に置けない座標がある
    pub(crate) fn committed_local(
        &self,
        cel: &Arc<LayerBase>,
    ) -> Option<crate::stored_map::StoredMap> {
        self.content(cel)
            .stacked_local(self.groups.iter().map(PieceGroup::placed))
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

    /// 写しを作り直す：隠す図形、残りの図形（残りの地図の面のうちセル画に無いもの。そのレイヤーの最初の隠す図形の所に
    /// 置く）、片の図形（片の地図の面を「ステージへの変換 `world` ∘ 片の変換」で置いた写し）。面の図形は、地図の計算に
    /// 使う写しの面（`LayerBase::faces`）から作る。レイヤーは重ね順 `order` に並べ、空のレイヤーは外す
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
        for l in &mut self.layers {
            let layer_id = l.layer_id;
            if let Some(o) = &mut l.origin {
                self.hidden.extend(o.hidden.iter().copied());
                let at = o.hidden.first().map_or_else(
                    || Placement::TopOfLayer(layer_id, scope.to_vec()),
                    |&id| Placement::ReplaceAt(id),
                );
                let added: HashSet<ShapeId> = o.remainder.iter().copied().collect();
                for f in o.hold().faces(scope).iter() {
                    if added.contains(&f.shape_id) {
                        let mut s = (**f).clone();
                        s.layer_id = layer_id;
                        s.place(world);
                        self.remainder.push((at.clone(), s));
                    }
                }
            }
            for g in &mut l.groups {
                for f in g.hold().faces(scope).iter() {
                    let mut s = (**f).clone();
                    s.layer_id = layer_id;
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
        let results: Vec<Option<FloatLayer>> = parallel_map(layers, |(layer_id, ids)| {
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
            .all(|g| g.base().map().fits_grid(doc * g.xform))
    }

    /// 動かす・回す・拡大：すべての片の組に、文書の座標の変換 `doc` を重ねる（重ねてよいかは先に `fits_transform` で
    /// 確かめる）
    pub(crate) fn transform(&mut self, doc: Affine) {
        for g in self.layers.iter_mut().flat_map(|l| &mut l.groups) {
            g.xform = doc * g.xform;
        }
        self.modified = true;
    }

    /// レイヤー `layers` の片のうち、番号が `ids` の片の色を `color` にする（同じ色でとなり合う片は1つになる）。全部か
    /// 無しか：`Err`：色を変えた片の地図が正しい地図にならない（何も変えない）。`Ok`：変えたか
    pub(crate) fn recolor(
        &mut self,
        ids: &HashSet<ShapeId>,
        color: Color32,
        layers: &HashSet<LayerId>,
    ) -> Result<bool, Unplaceable> {
        let mut next: Vec<(usize, usize, PieceGroup)> = Vec::new();
        for (li, l) in self.layers.iter().enumerate() {
            if !layers.contains(&l.layer_id) {
                continue;
            }
            for (gi, g) in l.groups.iter().enumerate() {
                if let Some(m) = g.base().map().recolored(ids, color) {
                    let m = m.checked().ok_or(Unplaceable)?;
                    next.push((li, gi, PieceGroup::new(m, g.xform)));
                }
            }
        }
        let any = !next.is_empty();
        for (li, gi, g) in next {
            self.layers[li].groups[gi] = g;
        }
        if any {
            self.modified = true;
        }
        Ok(any)
    }

    /// レイヤー `layer_id` の組 `group` の片の地図（文書の座標、変換をかける前）を `edit` で編集する。`false`：その組が
    /// 無いか、編集できない
    pub(crate) fn edit_group(
        &mut self,
        layer_id: LayerId,
        group: usize,
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
        let Some(next) = edit(g.base().map()).and_then(PlanarMap::checked) else {
            return false;
        };
        *g = PieceGroup::new(next, g.xform);
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
            let Ok(kept) = restricted_groups(&l.groups, keep) else {
                return false;
            };
            if kept.is_empty() {
                l.origin = None;
                l.groups.clear();
                continue;
            }
            let Ok(back) = restricted_groups(&l.groups, &drop) else {
                return false;
            };
            if back.is_empty() {
                continue;
            }
            let Some(cel) = stage.cel(layer_id) else {
                return false;
            };
            // 1本の道：外した片を中身に窓で重ね（`stacked_local`）、セル画の地図との違いを差分から先に渡す
            // （`replace_local`）。残りはセル画に対する答え
            let content = l.content(&cel);
            let Some(next) =
                content.stacked_local(back.iter().map(|g| Some(g.base().map().clone())))
            else {
                return false;
            };
            let origin = match cel.replace_local(&next, stage.scope) {
                LocalEdit::Same => None,
                LocalEdit::Changed(w) => Some(Origin::of_local(w, &cel, stage.scope)),
                LocalEdit::Refused(_) => return false,
            };
            // お手本：前の道（中身の地図全体に片を重ね、セル画に対して確かめて書き戻す）の残りと同じ
            #[cfg(test)]
            if crate::planar_map::test_checks() {
                let ok = crate::planar_map::uncounted(|| {
                    let mut rest = content.map().clone();
                    for g in &back {
                        match rest.overlay(MapInput::Map(g.base().map()), MapRule::Overlay) {
                            Some(m) => rest = m,
                            None => return false,
                        }
                    }
                    let faces = cel.faces(stage.scope);
                    let own: Vec<&FillShape> = faces.iter().map(|s| &**s).collect();
                    let full = rest.written(&cel).and_then(|r| {
                        Origin::of(cel.diff(r, &own, layer_id, stage.scope), &cel.stored)
                    });
                    match (&origin, &full) {
                        (None, None) => true,
                        (Some(a), Some(b)) => {
                            a.compare_key(stage.scope) == b.compare_key(stage.scope)
                        }
                        _ => false,
                    }
                });
                assert!(ok, "片を戻した残りが、前の道で戻した残り（お手本）と違う");
            }
            l.origin = origin;
            l.groups = kept;
        }
        true
    }

    /// 番号が `keep` に無い片と、残りを取り出す（確定する分）。この浮いた片には、残す片だけが残る（残りは確定するので
    /// 持たない）。見えている重なりを変えないように、残す片の組からは、それより上の組の取り出す片が覆う所を消す（取り
    /// 出す片は確定してレイヤーの中身になり、浮いた片はいつもレイヤーのいちばん上に描かれるので、消さないと、確定した
    /// 片の上へ繰り上がる。上の片を確定すれば下の片が削られるのと同じ。レビュー f01d518 の N-3002）。残す片は確定しない
    /// ので、その下のレイヤーの中身は変えない。全部か無しか：`Err`：片の地図が正しい地図にならない（何も変えない）
    pub(crate) fn split_off(&mut self, keep: &HashSet<ShapeId>) -> Result<Floating, Unplaceable> {
        let drop: HashSet<ShapeId> = self
            .layers
            .iter()
            .flat_map(|l| l.groups.iter().flat_map(|g| g.ids()))
            .filter(|id| !keep.contains(id))
            .collect();
        let parts = self
            .layers
            .iter()
            .map(|l| {
                // 上の組から順に見て、それまでに取り出す片の置いた地図（覆う所）を集める
                let (mut dropped, mut kept, mut cover) = (Vec::new(), Vec::new(), Vec::new());
                for g in l.groups.iter().rev() {
                    if let Some(k) = g.restricted(keep)? {
                        kept.extend(k.erased(&cover)?);
                    }
                    if let Some(d) = g.restricted(&drop)? {
                        cover.push(d.placed().ok_or(Unplaceable)?);
                        dropped.push(d);
                    }
                }
                dropped.reverse();
                kept.reverse();
                Ok((dropped, kept))
            })
            .collect::<Result<Vec<_>, Unplaceable>>()?;
        let mut out = Floating::new(true);
        for (l, (dropped, kept)) in self.layers.iter_mut().zip(parts) {
            out.layers.push(FloatLayer {
                layer_id: l.layer_id,
                origin: l.origin.take(),
                groups: dropped,
            });
            l.groups = kept;
        }
        self.layers.retain(|l| !l.is_empty());
        out.layers.retain(|l| !l.is_empty());
        Ok(out)
    }

    /// 確かめた地図 `map`（文書の座標。面の番号が片の番号）を、変換 `xform` をかける片の組として、レイヤー `layer_id` の
    /// 一番上に足す（変換は、動かした片と同じく確定のときに地図にかける）
    pub(crate) fn add_map(&mut self, layer_id: LayerId, map: CheckedMap, xform: Affine) {
        let group = PieceGroup::new(map, xform);
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
    // お手本（前の道）が使う、セル画の面の図形
    #[cfg(test)]
    let cel_faces = cel.faces(stage.scope);
    #[cfg(test)]
    let own: Vec<&FillShape> = cel_faces.iter().map(|s| &**s).collect();
    let empty = FloatLayer {
        layer_id,
        origin: None,
        groups: Vec::new(),
    };
    let cur = cur.unwrap_or(&empty);
    let content_base = cur.content(&cel);
    let part = (!ids.is_empty())
        .then(|| content_base.only_faces_map(ids, stage.scope))
        .filter(|m| !m.edges.is_empty());
    let (origin, inside): (Option<Origin>, CheckedMap) = match (part, rect) {
        (None, None) => return Some(cur.clone()),
        // 面を丸ごと持ち上げる：中身から、その面を外す（面の番号はそのまま。片は前と同じ番号の図形）。外す面の輪郭の辺だけ
        // を替えたしまう形（`restricted_local`）を、セル画の地図に対する差分として1本の道に渡す（`replace_local`）
        (Some(part), None) => {
            let rest = content_base
                .restricted_local(FaceKeep::Without(ids), stage.scope)
                .unwrap_or_else(|| StoredMap::from_dense(&PlanarMap::default(), None));
            let origin = match cel.replace_local(&rest, stage.scope) {
                LocalEdit::Same => None,
                LocalEdit::Changed(w) => Some(Origin::of_local(w, &cel, stage.scope)),
                LocalEdit::Refused(_) => return None,
            };
            // お手本：前の道（中身の地図全体から面を外し、セル画に対して確かめて書き戻す）の残りと同じ
            #[cfg(test)]
            if crate::planar_map::test_checks() {
                let ok = crate::planar_map::uncounted(|| {
                    let full = content_base.map().without(ids).written(&cel).and_then(|r| {
                        Origin::of(cel.diff(r, &own, layer_id, stage.scope), &cel.stored)
                    });
                    match (&origin, &full) {
                        (None, None) => true,
                        (Some(a), Some(b)) => {
                            a.compare_key(stage.scope) == b.compare_key(stage.scope)
                        }
                        _ => false,
                    }
                });
                assert!(
                    ok,
                    "面を持ち上げた残りが、前の道で持ち上げた残り（お手本）と違う"
                );
            }
            (origin, part.checked()?)
        }
        // 選ぶ所で切る（1本の道：`cut_out`）。残りは中身に対する答えなので、セル画に対する残りにする
        (part, Some(r)) => {
            let region_owned;
            let region = match part {
                None => MapInput::Shapes(std::slice::from_ref(r)),
                Some(p) => {
                    region_owned = region_map(&p, rect)?;
                    MapInput::Map(&region_owned)
                }
            };
            let (rest, inside) = cut_out(&cel, &content_base, region, stage.scope)?;
            let origin = match rest {
                LocalEdit::Changed(w) => Some(Origin::of_local(w, &cel, stage.scope)),
                _ => None,
            };
            // お手本：前の道（残りもセル画の地図に対して確かめ、書き戻す）の残りと同じ（隠す図形・残りの図形の形・残りの
            // 地図の面の形）
            #[cfg(test)]
            if crate::planar_map::test_checks() {
                let ok = crate::planar_map::uncounted(|| {
                    let full = crate::planar_map::cut_out_full(
                        &cel,
                        &own,
                        &content_base,
                        region,
                        layer_id,
                        stage.scope,
                    )
                    .and_then(|(rest, _)| Origin::of(rest, &cel.stored));
                    match (&origin, &full) {
                        (None, None) => true,
                        (Some(a), Some(b)) => {
                            a.compare_key(stage.scope) == b.compare_key(stage.scope)
                        }
                        _ => false,
                    }
                });
                assert!(ok, "矩形で切った残りが、前の道で切った残り（お手本）と違う");
            }
            (origin, inside)
        }
    };
    let mut groups = cur.groups.clone();
    if !inside.map().edges.is_empty() {
        match groups.first_mut() {
            Some(g) if !modified => {
                let joined = g
                    .base()
                    .map()
                    .overlay(MapInput::Map(inside.map()), MapRule::Overlay)?
                    .checked()?;
                *g = PieceGroup::new(joined, Affine::IDENTITY);
            }
            _ => groups.insert(0, PieceGroup::new(inside, Affine::IDENTITY)),
        }
    }
    Some(FloatLayer {
        layer_id,
        origin,
        groups,
    })
}
