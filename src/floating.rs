//! 浮いた片（矩形で切った片・つかんで動かす片・貼り付けた片）。設計：claude/浮いた片を地図で持つ設計_2-2c_6c167ae.md
//!
//! 本当の形はレイヤーごとの地図（文書の座標）：残りの地図（持ち上げたレイヤーの、持ち上げた所を除いた地図）と、片の組
//! （片の地図と、それにかける変換）。画面・選択・当たり判定の図形は、地図から作る見た目（`FloatingView::of`）。

use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::Color32;
use kurbo::Affine;

use crate::canvas_tool::{Placement, SceneBaseCache};
use crate::planar_map::{
    cut_out, parallel_map, realized, region_map, shape_ids_of, stacked, CheckedMap, FaceKeep, LayerBase,
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
/// （`FloatingView::of`。Undo の控えに入るのは地図と変換だけ：レビュー 6ee325a の N-2005）
#[derive(Clone, Debug)]
pub(crate) struct PieceGroup {
    map: StoredMap,
    /// 作ったばかりの計算に使う写し（預かり物：`Floating::drop_fresh` の説明）
    fresh: Option<Arc<LayerBase>>,
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
            fresh: Some(base),
            xform,
        }
    }

    /// 片の地図の、計算に使う写し
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        LayerBase::of_stored(&self.map)
    }

    /// 片の番号：面の番号と線の番号（`shape_ids_of`）。同じ番号をくり返す。前は面の番号だけだったので、線だけの片の組は、番号の無い組として、一部を残すとき
    /// （`restricted`）に丸ごと残り、残りに戻す片（`ids_not_in`）にも数えられず、同じ線が片と残りの両方に入った
    fn ids(&self) -> Vec<ShapeId> {
        shape_ids_of(&self.map).collect()
    }

    /// 片の番号のどれか1つ（画面の図形の並びで、この組の片を探す手がかり。線だけの組も。番号の決まりは `ids` と同じ
    /// `shape_ids_of` で、最初の1つで止める）
    pub(crate) fn first_id(&self) -> Option<ShapeId> {
        shape_ids_of(&self.map).next()
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

    /// 番号が `keep` にある片だけの組（編集中の階層 `scope`。`None`：1つも無い）。正しい地図の面の一部なので、確かめ
    /// 直さない（`LayerBase::checked_part`）
    fn restricted(&self, keep: &HashSet<ShapeId>, scope: &[u64]) -> Option<Self> {
        let ids = self.ids();
        if ids.iter().all(|id| keep.contains(id)) {
            return Some(self.clone());
        }
        if !ids.iter().any(|id| keep.contains(id)) {
            return None;
        }
        Some(Self::new(
            self.base().checked_part(Some(keep), scope),
            self.xform,
        ))
    }
}

/// 浮いた片 `fl` のすべての片の組と、まだ持ち上げていない生の図形 `raw`（レイヤーごとの面の番号。持ち上げると、中身の
/// 地図のその面だけの片になる：`cut_layer`）の、確定のときに変換をかける地図と、その地図に先にかかっている変換（片の組の
/// 変換。持ち上げていない図形は無し）。置けるかの判定は `canvas_tool::TransformFit::fits`
pub(crate) fn piece_maps(
    fl: Option<&Floating>,
    stage: Stage<'_>,
    raw: &[(LayerId, HashSet<ShapeId>)],
) -> Vec<(FitMap, Affine)> {
    let layers = fl.map_or(&[][..], |fl| &fl.layers[..]);
    let pieces = layers
        .iter()
        .flat_map(|l| &l.groups)
        .map(|g| (FitMap::Held(g.base()), g.xform));
    let unlifted = raw.iter().filter_map(|(layer_id, ids)| {
        let cel = stage.cel(*layer_id)?;
        let content = match layers.iter().find(|l| l.layer_id == *layer_id) {
            Some(l) => l.content(&cel),
            None => cel,
        };
        Some((
            FitMap::Owned(content.only_faces_map(ids, stage.scope)),
            Affine::IDENTITY,
        ))
    });
    pieces.chain(unlifted).collect()
}

/// 置けるかを調べる地図（`piece_maps`・`Session::transform_fit`）：写しが持つ地図（片の組・グループの階層。写しを生かして
/// おくだけで地図を写さない）か、面だけを残して作った地図（まだ持ち上げていない生の図形）
#[derive(Debug)]
pub(crate) enum FitMap {
    Held(Arc<LayerBase>),
    Owned(PlanarMap),
}

impl FitMap {
    pub(crate) fn map(&self) -> &PlanarMap {
        match self {
            FitMap::Held(base) => base.map(),
            FitMap::Owned(m) => m,
        }
    }
}

/// 片の地図が、格子の上で正しい地図にならない（片の操作を断る）
#[derive(Debug)]
pub(crate) struct Unplaceable;

/// 片の組 `groups` のうち、番号が `keep` にある片だけの組（編集中の階層 `scope`。片の無くなった組は外す）
fn restricted_groups(
    groups: &[PieceGroup],
    keep: &HashSet<ShapeId>,
    scope: &[u64],
) -> Vec<PieceGroup> {
    groups
        .iter()
        .filter_map(|g| g.restricted(keep, scope))
        .collect()
}

/// 持ち上げたレイヤーの残り
#[derive(Clone, Debug)]
pub(crate) struct Origin {
    /// 残りの地図（しまう形。セル画の地図から、持ち上げた所を除いた地図。文書の座標。変わらない塊はセル画の地図と共有する）
    rest: StoredMap,
    /// 作ったばかりの残りの地図の計算に使う写し（預かり物：`Floating::drop_fresh` の説明）
    fresh: Option<Arc<LayerBase>>,
    /// 隠すセル画の図形（セル画の地図の面のうち、残りの地図で変わったもの）
    hidden: Vec<ShapeId>,
    /// 残りの図形の番号（残りの地図の面のうち、セル画に無いもの。図形は計算に使う写しの面から作る：`FloatingView::of`）
    remainder: Vec<ShapeId>,
    /// 出どころ：持ち上げたときのセル画の地図（その階層の）。確定するときに、今のセル画の地図がこれと同じでなければ断る
    /// （`FloatLayer::still_on`。持って生かしておくので、番地が別の地図に使い回されない）。前は出どころを持たず、確定は今の
    /// 画面のセル画に残りを書いたので、確定を通らずにセル画が変わる（確定の呼び忘れ）と、変わった絵が残りで黙って置き換わった
    /// （設計審査 de92ceb の D-3）
    from: StoredMap,
}

impl Origin {
    /// セル画の地図 `cel` を残りの地図にする1本の道の答え `w`（`LayerBase::replace_local`）から：消した図形が隠す図形、
    /// 入れた図形が残りの図形。線の図形は、セル画の地図から変わった辺の線が隠す図形、増えた辺の線が残りの図形
    /// （`LayerBase::line_changes`）。升目の索引は、セル画の写しから変わった所だけ
    /// 作る。答えが変わらない（セル画の地図のまま）ときは、使う側が残りを持たない
    fn of_local(w: LocalWrite, cel: &LayerBase, scope: &[u64]) -> Self {
        let (base, hidden, remainder) = cel.written_view(w, scope);
        Self {
            rest: base.stored.clone(),
            fresh: Some(base),
            hidden,
            remainder,
            from: cel.stored.clone(),
        }
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

    /// 今のセル画の地図 `cel` に確定してよいか：持ち上げた残りがあれば、その出どころの地図が `cel` そのもの（残りの無い
    /// レイヤー（貼り付け・複製の片だけ）は、今のセル画の地図に片を重ねるだけなので、いつもよい）
    pub(crate) fn still_on(&self, cel: &LayerBase) -> bool {
        self.origin
            .as_ref()
            .is_none_or(|o| o.from.id() == cel.stored.id())
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
        let groups = restricted_groups(&self.groups, ids, stage.scope);
        stacked(start, groups.iter().map(PieceGroup::placed))?.checked()
    }

    /// 確定でセル画の地図 `cel`（階層 `scope`）に書くもの（1本の道の答え：`LayerBase::replace_local`）。確定の計算はこれ1つ
    /// （確定の計画 `plan_commit` と、複製のドラッグ中の止まった絵）。`None`：持ち上げた絵と違うセル画（`still_on`）か、
    /// 格子に置けない座標がある
    pub(crate) fn commit_edit(&self, cel: &Arc<LayerBase>, scope: &[u64]) -> Option<LocalEdit> {
        if !self.still_on(cel) {
            return None;
        }
        let next = self.committed_local(cel)?;
        Some(cel.replace_local(&next, scope))
    }

    /// 確定した地図のしまう形：中身の地図に、片の組を下から順に重ねる（組ごとに変換をかけて、その座標の格子で正しい地図に
    /// する：`realized`。1枚ずつ窓で重ねる：`LayerBase::stacked_local`。中身の地図と塊を共有する）。`None`：格子に置けない座標がある
    pub(crate) fn committed_local(
        &self,
        cel: &Arc<LayerBase>,
    ) -> Option<crate::stored_map::StoredMap> {
        self.content(cel)
            .stacked_local(self.groups.iter().map(PieceGroup::placed))
    }
}

/// 浮いた片の中身（取り消しで戻すもの：レイヤーごとの残りの地図と片の組の地図・変換）。画面の写しと、計算に使う写しを
/// 生かしておくのは、セッションの浮いた片の見た目（`FloatingView`）。セッションは中身を分け合える箱で持ち、取り消しの
/// 控えは箱を分け合う（設計：claude/浮いた片の中身と写しを分ける設計_cb00886.md）
#[derive(Clone, Debug)]
pub(crate) struct Floating {
    layers: Vec<FloatLayer>,
    /// 動かした・変形したか、文書に無い片があるか。`true` なら確定のときに重ねる。`false` は動かしていない矩形選択の
    /// 切り口で、捨てれば元に戻る
    pub(crate) modified: bool,
}

/// 浮いた片の見た目：片の図形と残りの図形（ステージの写し）、隠すセル画の図形と、それを作った計算に使う写し（画面に出して
/// いる間、生かしておく：浮いた片の中の計算が `LayerBase::of_stored` で引く）。中身から作る（`of`）
#[derive(Default, Debug)]
pub(crate) struct FloatingView {
    /// 片の図形と、それぞれの片の組の番号（そのレイヤーの、下から何番目の組か：`placed_pieces`）
    pieces: Vec<FillShape>,
    piece_groups: Vec<usize>,
    /// 残りの図形と、その置き場所
    remainder: Vec<(Placement, FillShape)>,
    /// 画面から隠すセル画の図形
    hidden: HashSet<ShapeId>,
    bases: Vec<Arc<LayerBase>>,
}

impl FloatingView {
    /// 浮いた片 `fl` の見た目：隠す図形、残りの図形（残りの地図の面のうちセル画に無いもの。そのレイヤーの最初の隠す
    /// 図形の所に置く）、片の図形（片の地図の面を「ステージへの変換 `world` ∘ 片の変換」で置いた写し）。面の図形は、
    /// 地図の計算に使う写しの面（`LayerBase::faces`）から作る（作ったばかりの写しは中身が預かっているので、それを使う）
    pub(crate) fn of(fl: &Floating, world: Affine, scope: &[u64]) -> Self {
        let mut view = Self::default();
        for l in &fl.layers {
            let layer_id = l.layer_id;
            if let Some(o) = &l.origin {
                view.hidden.extend(o.hidden.iter().copied());
                let at = Placement::RawOf(layer_id, scope.to_vec());
                let added: HashSet<ShapeId> = o.remainder.iter().copied().collect();
                let base = o.base();
                for f in base.drawn_shapes(scope) {
                    if added.contains(&f.shape_id) {
                        let mut s = (*f).clone();
                        s.layer_id = layer_id;
                        s.place(world);
                        view.remainder.push((at.clone(), s));
                    }
                }
                view.bases.push(base);
            }
            for (gi, g) in l.groups.iter().enumerate() {
                let base = g.base();
                for f in base.drawn_shapes(scope) {
                    let mut s = (*f).clone();
                    s.layer_id = layer_id;
                    // 生の絵の線は、拡大しても太さが変わらない（確定して地図の辺に変換をかけたのと同じ形）
                    if g.xform != Affine::IDENTITY {
                        s.transform_keeping_line_width(g.xform, Affine::IDENTITY);
                    }
                    s.place(world);
                    view.pieces.push(s);
                    view.piece_groups.push(gi);
                }
                view.bases.push(base);
            }
        }
        view
    }

    /// 片の図形（ステージの写し）
    pub(crate) fn pieces(&self) -> &[FillShape] {
        &self.pieces
    }

    /// 片の図形と、その置き場（そのレイヤー・階層 `scope` の生の図形のすぐ上の、片の組ごとの置き場：`Placement::AboveRaw`）
    pub(crate) fn placed_pieces<'a>(
        &'a self,
        scope: &'a [u64],
    ) -> impl Iterator<Item = (Placement, &'a FillShape)> + 'a {
        self.pieces.iter().zip(&self.piece_groups).map(move |(p, &g)| {
            (Placement::AboveRaw(p.layer_id, scope.to_vec(), g), p)
        })
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

    /// 片の番号
    pub(crate) fn piece_ids(&self) -> HashSet<ShapeId> {
        self.pieces.iter().map(|p| p.shape_id).collect()
    }
}

impl Floating {
    pub(crate) fn new(modified: bool) -> Self {
        Self {
            layers: Vec::new(),
            modified,
        }
    }

    pub(crate) fn layers(&self) -> &[FloatLayer] {
        &self.layers
    }

    /// 片の組の片のうち、番号が `keep` に無い片の番号
    fn ids_not_in(&self, keep: &HashSet<ShapeId>) -> HashSet<ShapeId> {
        self.layers
            .iter()
            .flat_map(|l| l.groups.iter().flat_map(|g| g.ids()))
            .filter(|id| !keep.contains(id))
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// 重ね順 `order` に並べ、空のレイヤーを外す（並べ方は1つに決まる：同じ重ね順のレイヤーはもとの順のまま）
    pub(crate) fn tidy(&mut self, order: &[LayerId]) {
        self.layers.retain(|l| !l.is_empty());
        self.layers.sort_by_key(|l| {
            order
                .iter()
                .position(|&id| id == l.layer_id)
                .unwrap_or(usize::MAX)
        });
    }

    /// 預かり物を外す。浮いた片の操作は、計算の途中で新しい計算に使う写し（残りの地図の面の表など）を作る。中身はそれを、
    /// 浮いた片を書き換える入口が見た目を作るまでの間だけ預かる（見た目が受け取る前に捨てると、見た目を作るときに地図の面を
    /// 一から数え直す）。入口は、見た目を作ったあと預かり物を外してから箱に入れるので、セッションの箱と取り消しの控えは
    /// 預かり物を持たない（`Session::put_floating`）
    pub(crate) fn drop_fresh(&mut self) {
        for l in &mut self.layers {
            if let Some(o) = &mut l.origin {
                o.fresh = None;
            }
            for g in &mut l.groups {
                g.fresh = None;
            }
        }
    }

    /// 確定でレイヤーごとに書くもの（`FloatLayer::commit_edit` と、それを作ったセル画の地図の写し。レイヤーをまたがないので、
    /// 同時に計算する：待ち時間はいちばん重いレイヤー1つ分）。確定の計算はこれ1つ（確定の計画 `plan_commit` と、複製の
    /// ドラッグ中の確定した絵）。`None` のレイヤー：画面に出していないか、確定できない
    #[allow(clippy::type_complexity)]
    pub(crate) fn commit_edits(&self, stage: Stage<'_>) -> Vec<Option<(LocalEdit, Arc<LayerBase>)>> {
        parallel_map(&self.layers, |l| {
            let base = stage.cel(l.layer_id)?;
            let r = l.commit_edit(&base, stage.scope)?;
            Some((r, base))
        })
    }

    /// 同じ中身か（地図は同じしまう形、変換は同じ値。預かり物は見ない）
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.modified == other.modified
            && self.layers.len() == other.layers.len()
            && self.layers.iter().zip(&other.layers).all(|(a, b)| {
                a.layer_id == b.layer_id
                    && match (&a.origin, &b.origin) {
                        (None, None) => true,
                        (Some(x), Some(y)) => x.rest.id() == y.rest.id() && x.from.id() == y.from.id(),
                        _ => false,
                    }
                    && a.groups.len() == b.groups.len()
                    && a.groups
                        .iter()
                        .zip(&b.groups)
                        .all(|(g, h)| g.map.id() == h.map.id() && g.xform == h.xform)
            })
    }

    /// テスト用：レイヤーごとの（レイヤー, 残りの地図と預かり物を持つか, 片の組の地図と預かり物を持つか）
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
                    l.origin.as_ref().map(|o| (&o.rest, o.fresh.is_some())),
                    l.groups
                        .iter()
                        .map(|g| (&g.map, g.fresh.is_some()))
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

    /// 動かす・回す・拡大：すべての片の組に、文書の座標の変換 `doc` を重ねる（重ねてよいかは先に `TransformFit::fits` で
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
        let drop = self.ids_not_in(keep);
        for l in &mut self.layers {
            let layer_id = l.layer_id;
            let kept = restricted_groups(&l.groups, keep, stage.scope);
            if kept.is_empty() {
                l.origin = None;
                l.groups.clear();
                continue;
            }
            let back = restricted_groups(&l.groups, &drop, stage.scope);
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
            l.origin = origin;
            l.groups = kept;
        }
        true
    }

    /// 番号が `keep` に無い片と、残りを取り出す（確定する分）。この浮いた片には、残す片だけが残る（残りは確定するので
    /// 持たない）。見えている重なりを変えないように、残す片の組からは、それより上の組の取り出す片が覆う所を消す（取り
    /// 出す片は確定してレイヤーの中身になり、浮いた片はいつもレイヤーの生の図形のいちばん上に描かれるので、消さないと、確定した
    /// 片の上へ繰り上がる。上の片を確定すれば下の片が削られるのと同じ。レビュー f01d518 の N-3002）。残す片は確定しない
    /// ので、その下のレイヤーの中身は変えない。全部か無しか：`Err`：片の地図が正しい地図にならない（何も変えない）
    pub(crate) fn split_off(
        &mut self,
        keep: &HashSet<ShapeId>,
        scope: &[u64],
    ) -> Result<Floating, Unplaceable> {
        let drop = self.ids_not_in(keep);
        let parts = self
            .layers
            .iter()
            .map(|l| {
                // 上の組から順に見て、それまでに取り出す片の置いた地図（覆う所）を集める
                let (mut dropped, mut kept, mut cover) = (Vec::new(), Vec::new(), Vec::new());
                for g in l.groups.iter().rev() {
                    if let Some(k) = g.restricted(keep, scope) {
                        kept.extend(k.erased(&cover)?);
                    }
                    if let Some(d) = g.restricted(&drop, scope) {
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
    let empty = FloatLayer {
        layer_id,
        origin: None,
        groups: Vec::new(),
    };
    let cur = cur.unwrap_or(&empty);
    let content_base = cur.content(&cel);
    let (origin, inside): (Option<Origin>, CheckedMap) = match rect {
        // 面を丸ごと持ち上げる：中身から、その面を外す（面の番号はそのまま。片は前と同じ番号の図形。正しい地図の面の一部
        // なので確かめ直さない：`checked_part`）。外す面の輪郭の辺だけを替えたしまう形（`restricted_local`）を、セル画の
        // 地図に対する差分として1本の道に渡す（`replace_local`）
        None => {
            let part = (!ids.is_empty())
                .then(|| content_base.checked_part(Some(ids), stage.scope))
                .filter(|m| !m.map().edges.is_empty());
            let Some(part) = part else {
                return Some(cur.clone());
            };
            let rest = content_base
                .restricted_local(FaceKeep::Without(ids), stage.scope)
                .unwrap_or_else(|| StoredMap::from_dense(&PlanarMap::default(), None));
            let origin = match cel.replace_local(&rest, stage.scope) {
                LocalEdit::Same => None,
                LocalEdit::Changed(w) => Some(Origin::of_local(w, &cel, stage.scope)),
                LocalEdit::Refused(_) => return None,
            };
            (origin, part)
        }
        // 選ぶ所で切る（1本の道：`cut_out`）。残りは中身に対する答えなので、セル画に対する残りにする
        Some(r) => {
            let part = (!ids.is_empty())
                .then(|| content_base.only_faces_map(ids, stage.scope))
                .filter(|m| !m.edges.is_empty());
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
