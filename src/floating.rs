//! 浮いた片（矩形で切った片・つかんで動かす片・貼り付けた片）。設計：claude/浮いた片を地図で持つ設計_2-2c_6c167ae.md
//!
//! 本当の形はレイヤーごとの地図（文書の座標）：残りの地図（持ち上げたレイヤーの、持ち上げた所を除いた地図）と、片の組
//! （片の地図と、それにかける変換）。画面・選択・当たり判定の図形は、地図から作る見た目（`FloatingView::of`）。

use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::Color32;
use kurbo::Affine;

use crate::canvas_tool::Placement;
use crate::timeline::{Cel, KeyframeId, Timeline};
use crate::planar_map::{
    cut_out, is_line_id, parallel_map, realized, region_map, shape_ids_of, stacked, CheckedMap, FaceKeep, LayerBase,
    LocalEdit, LocalWrite, MapInput, MapRule, PlanarMap,
};
use crate::stored_map::StoredMap;
use crate::vector_engine::FillShape;
use crate::vector_engine::{LayerId, ShapeId};

/// 浮いた片の操作に要る土台：編集中のタイムラインのレイヤーの重ね順（全レイヤー）と、今のコマに見えているセル画（絵を
/// 扱えるレイヤーだけ：`Layer::is_open`。中身は文書と分け合う写し）とその場所、編集中の階層。文書から作る（`Stage::of`：
/// 画面の土台 `SceneBaseCache` に頼らない。前は画面の土台から読んだので、入口の外から浮いた片を扱う道は、土台を先に作り
/// 直す約束を覚えている必要があった：設計審査 b7208d9 の 8.）。地図の計算に使う写しは、画面の土台が持っていれば同じ物を
/// 分け合う（`LayerBase::of_stored`）ので、作り直さない
#[derive(Clone, Debug)]
pub(crate) struct Stage {
    order: Vec<LayerId>,
    cels: Vec<(LayerId, KeyframeId, Cel)>,
    /// 編集中のシンボルの経路（シンボルの番号の並び。ルートは空）
    path: Vec<u64>,
    scope: Vec<u64>,
}

/// セル画の場所：編集中のシンボルの経路・レイヤー・キーフレーム・階層（浮いた片の見張り `FloatLayer::still_on`）
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CelPlace {
    path: Vec<u64>,
    layer: LayerId,
    keyframe: KeyframeId,
    scope: Vec<u64>,
}

impl Stage {
    /// 編集中のタイムライン `tl`（シンボルの経路 `path`）の今のコマの土台（階層 `scope`）
    pub(crate) fn of(tl: &Timeline, path: Vec<u64>, scope: &[u64]) -> Self {
        let frame = tl.current_frame;
        let order = tl.layers.iter().map(|l| l.id).collect();
        let cels = tl
            .layers
            .iter()
            .filter(|l| l.is_open())
            .filter_map(|l| l.shown_keyframe(frame).map(|(id, kf)| (l.id, id, kf.cel.clone())))
            .collect();
        Self { order, cels, path, scope: scope.to_vec() }
    }

    /// 編集中の階層
    pub(crate) fn scope(&self) -> &[u64] {
        &self.scope
    }

    /// レイヤーの重ね順（編集中のタイムラインの全レイヤー）
    pub(crate) fn order(&self) -> &[LayerId] {
        &self.order
    }

    /// 今のコマに見えているセル画（レイヤーの重ね順）
    pub(crate) fn cels(&self) -> impl Iterator<Item = (LayerId, &Cel)> {
        self.cels.iter().map(|(id, _, cel)| (*id, cel))
    }

    /// レイヤー `layer_id` の、階層 `group_path` の地図（セル画の地図。生の図形はその面：`LayerBase::faces`）。`None`：今の
    /// コマにセル画の無いレイヤー
    pub(crate) fn base_at(&self, layer_id: LayerId, group_path: &[u64]) -> Option<Arc<LayerBase>> {
        let (_, _, cel) = self.cels.iter().find(|(id, _, _)| *id == layer_id)?;
        Some(cel.scope_base(group_path))
    }

    /// レイヤー `layer_id` の、編集中の階層の地図（`base_at`）
    pub(crate) fn cel(&self, layer_id: LayerId) -> Option<Arc<LayerBase>> {
        self.base_at(layer_id, &self.scope)
    }

    /// レイヤー `layer_id` のセル画の場所
    fn place(&self, layer_id: LayerId) -> Option<CelPlace> {
        let (_, keyframe, _) = self.cels.iter().find(|(id, _, _)| *id == layer_id)?;
        Some(CelPlace { path: self.path.clone(), layer: layer_id, keyframe: *keyframe, scope: self.scope.clone() })
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
    /// 確定で置く組か（動かした・色を変えた・文書に無い）。置かない組（選んで持ち上げただけ）は、確定でセル画を変えない：
    /// 元の所は、持ち上げる前の絵のまま（`Origin::kept`。設計 claude/約束を入口で守る設計_5616fcc.md の 13 の 2.）。置かない組を
    /// 置く組にするのは `Floating::place_all` だけ（書く残りから元の所を切り抜くのと一緒に行う。作り方は、置かない組 `new`・
    /// 出どころの無い置く組 `new_placed`・地図だけ替える `replaced` で、どれも置かない組を置く組にしない：設計審査 01e4283 の
    /// D-2。前は操作ごとに置く組の印を付けて `place_all` を呼び、呼び忘れると元の所の絵と片が二重に書かれえた）
    placed: bool,
    /// 上に重ねる線の形：この組の塗りより上に見えていた、一部の確定で先に書いた線の地図（線だけの地図。文書の座標。無ければ
    /// 空。`Floating::split_off`）。使うときは、その時点の中身から、この形に沿う線を取り出す（`LayerBase::lines_along`）：
    /// 確定ではこの組を置いたあとに、確定で書く残りからその線を取り出してもう一度重ね（`FloatLayer::placed_on`）、持っている
    /// 間はこの組の塗りの上に描き（`FloatingView::of`）、持ち上げた線ならこの組のすぐ上に置く（`cut_layer`）。中身にもう無い
    /// 所（動かした・消した・覆われた）は取り出されないので、幽霊の線は出ない。前は一部を確定すると、同じ組の線が塗りの下に
    /// 入り、あとで塗りを確定すると線が覆われて消えた（設計審査 b7208d9 の 9.）。そのあと番号で覚えたので、線が切られて
    /// 番号が変わると迷子になった（設計審査 01e4283 の D-1）
    lines_over: StoredMap,
}

impl PieceGroup {
    /// 変換をかけて置いた地図（その座標の格子で正しい地図にしたもの：`realized`）。片の地図は作ったときに確かめてある
    /// （`new` は確かめた地図だけを受け取る）ので、変換が無ければ、そのまま使う。`None`：格子に置けない
    fn placed_map(&self) -> Option<PlanarMap> {
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
            placed: false,
            lines_over: StoredMap::from_dense(&PlanarMap::default(), None),
        }
    }

    /// 出どころの無い置く組（貼り付けた・複製した中身：書く残りに元の所が無いので、置く組にしてよい）
    fn new_placed(map: CheckedMap, xform: Affine) -> Self {
        Self { placed: true, ..Self::new(map, xform) }
    }

    /// 片の地図を `map` に替えた組（置く組か・変換・上に重ねる線は引き継ぐ）
    fn replaced(&self, map: CheckedMap) -> Self {
        Self {
            placed: self.placed,
            lines_over: self.lines_over.clone(),
            ..Self::new(map, self.xform)
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
        let mut map = self.placed_map().ok_or(Unplaceable)?;
        for c in cover {
            map = map
                .overlay(MapInput::Map(c), MapRule::Paint(None))
                .ok_or(Unplaceable)?;
        }
        if map.edges.is_empty() {
            return Ok(None);
        }
        let map = map.checked().ok_or(Unplaceable)?;
        Ok(Some(Self {
            placed: self.placed,
            lines_over: self.lines_over.clone(),
            ..Self::new(map, Affine::IDENTITY)
        }))
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
        Some(Self {
            placed: self.placed,
            lines_over: self.lines_over.clone(),
            ..Self::new(self.base().checked_part(Some(keep), scope), self.xform)
        })
    }

    /// 上に重ねる線の形の地図（無ければ `None`）
    fn lines_over_map(&self) -> Option<Arc<LayerBase>> {
        (!self.lines_over.is_empty()).then(|| LayerBase::of_stored(&self.lines_over))
    }

    /// 上に重ねる線の形に、線だけの地図 `lines`（文書の座標）を足す（重ねる。同じ線は1本）。`None`：格子に置けない
    fn add_lines_over(&mut self, lines: &[PlanarMap]) -> Option<()> {
        let mut m = self.lines_over_map().map(|b| b.map().clone()).unwrap_or_default();
        for l in lines {
            m = m.overlay(MapInput::Map(l), MapRule::Overlay)?;
        }
        self.lines_over = StoredMap::from_dense(&m, Some(&self.lines_over));
        Some(())
    }
}

/// 浮いた片 `fl` のすべての片の組と、まだ持ち上げていない生の図形 `raw`（レイヤーごとの面の番号。持ち上げると、中身の
/// 地図のその面だけの片になる：`cut_layer`）の、確定のときに変換をかける地図と、その地図に先にかかっている変換（片の組の
/// 変換。持ち上げていない図形は無し）。置けるかの判定は `canvas_tool::TransformFit::fits`
pub(crate) fn piece_maps(
    fl: Option<&Floating>,
    stage: &Stage,
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
            FitMap::Owned(content.only_faces_map(ids, stage.scope())),
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
    /// 出どころの場所（持ち上げたときのセル画の場所：シンボルの経路・レイヤー・キーフレーム・階層）。確定するときに、今の
    /// セル画の場所がこれと同じでなければ断る（`FloatLayer::still_on`。地図だけを比べると、F6 で写したキーフレームは地図を
    /// 分け合うので、別のコマを同じ所と見た：設計審査 b7208d9 の 3.）
    place: CelPlace,
    /// 出どころ：持ち上げたときのセル画の地図（その階層の）。確定するときに、今のセル画の地図がこれと同じでなければ断る
    /// （`FloatLayer::still_on`。持って生かしておくので、番地が別の地図に使い回されない）。前は出どころを持たず、確定は今の
    /// 画面のセル画に残りを書いたので、確定を通らずにセル画が変わる（確定の呼び忘れ）と、変わった絵が残りで黙って置き換わった
    /// （設計審査 de92ceb の D-3）
    from: StoredMap,
    /// 確定で書く残り：出どころの地図から、確定で置く組（`PieceGroup::placed`）の元の所だけを切り抜いた地図（置かない組の
    /// 元の所は、出どころのまま）。`rest`（画面に見せる、持ち上げた所をみな切り抜いた残り）とは、置かない組があるときだけ
    /// 違う。組をみな置く組にするとき（動かす・色を変える・消す：`Floating::place_all`）に `rest` にそろえる。前は確定で
    /// いつも `rest` に組をみな重ねたので、動かした絵があるときに選び足しただけの所が、切り口のまま書かれた（約束を入口で
    /// 守る設計_5616fcc.md の 13 の 2.）
    kept: StoredMap,
}

/// 残りが変わる出来事（2つの残りを書く決まりの表の行：`Origin::after`）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestChange {
    /// 置かない組の片を持ち上げた・戻した：見せる残りだけが変わる（書く残りは前のまま。初めてなら出どころ）
    Unplaced,
    /// 一部を確定した：書いたあとのセル画が、新しい出どころで書く残り（置かない組は確定でセル画を変えない）
    Committed,
}

impl Origin {
    /// 出来事 `change` のあとの残り（見せる残りと書く残りを書くのは、ここと `place_all` だけ。決まりの表は `RestChange`：
    /// この表から「見せる残り＋置かない組＝書く残り」が必ず成り立つ。前は呼ぶ側が書く残りを渡したので、2つの残りを別々の
    /// 関数が書いた：設計 claude/審査b7208d9の直しの設計.md の 9.）。前の残り `prev`、セル画の地図 `cel` を新しい見せる残りに
    /// する1本の道の答え `w`（`LayerBase::replace_local`）から：消した図形が隠す図形、入れた図形が残りの図形。線の図形は、
    /// セル画の地図から変わった辺の線が隠す図形、増えた辺の線が残りの図形（`LayerBase::line_changes`）。升目の索引は、セル画の
    /// 写しから変わった所だけ作る。答えが変わらない（セル画の地図のまま）ときは、使う側が残りを持たない
    fn after(
        prev: Option<&Origin>,
        change: RestChange,
        w: LocalWrite,
        cel: &LayerBase,
        place: CelPlace,
        scope: &[u64],
    ) -> Self {
        let kept = match change {
            RestChange::Unplaced => prev.map_or_else(|| cel.stored.clone(), |o| o.kept.clone()),
            RestChange::Committed => cel.stored.clone(),
        };
        let (base, hidden, remainder) = cel.written_view(w, scope);
        Self {
            place,
            rest: base.stored.clone(),
            fresh: Some(base),
            hidden,
            remainder,
            from: cel.stored.clone(),
            kept,
        }
    }

    /// 残りの地図の、計算に使う写し
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        LayerBase::of_stored(&self.rest)
    }

    /// 組をみな置く組にした：書く残りを見せる残りにそろえる（決まりの表の1行。`Floating::place_all`）
    fn place_all(&mut self) {
        self.kept = self.rest.clone();
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

    /// 今のセル画（場所 `place`・地図 `cel`）に確定してよいか：持ち上げた残りがあれば、その出どころの場所と地図が今の
    /// セル画と同じ（残りの無いレイヤー（貼り付け・複製の片だけ）は、今のセル画の地図に片を重ねるだけなので、いつもよい）
    pub(crate) fn still_on(&self, place: &CelPlace, cel: &LayerBase) -> bool {
        self.origin
            .as_ref()
            .is_none_or(|o| o.place == *place && o.from.id() == cel.stored.id())
    }

    /// 中身の地図：残りの地図か、無ければセル画の地図 `cel`
    fn content(&self, cel: &Arc<LayerBase>) -> Arc<LayerBase> {
        self.origin
            .as_ref()
            .map_or_else(|| cel.clone(), Origin::base)
    }

    /// 確定で書く残り（`Origin::kept`）か、無ければセル画の地図 `cel`
    fn kept_content(&self, cel: &Arc<LayerBase>) -> Arc<LayerBase> {
        self.origin
            .as_ref()
            .map_or_else(|| cel.clone(), |o| LayerBase::of_stored(&o.kept))
    }

    /// 片の組を下から順に重ねた地図（組ごとに変換をかけて、その座標の格子で正しい地図にする：`realized`）。片が無ければ
    /// 空の地図。`None`：格子に置けない座標がある
    pub(crate) fn pieces_map(&self) -> Option<PlanarMap> {
        stacked(
            PlanarMap::default(),
            self.groups.iter().map(PieceGroup::placed_map),
        )
    }

    /// 選んだもの（番号が `ids` の、中身の地図の面と片）の地図：中身の地図の選んだ面に、選んだ片の組を下から順に重ねる
    /// （確定と同じ重ね方。新しい中身。確かめた地図：`checked`）。`None`：画面に出していないレイヤーか、格子に置けない
    /// 座標があるか、正しい地図にならない
    pub(crate) fn selected_map(
        &self,
        stage: &Stage,
        ids: &HashSet<ShapeId>,
    ) -> Option<CheckedMap> {
        // 選んだ面だけで作る（手間は選んだ面の大きさ。前はセル画の地図全体を作ってから選んだ面を残した：全体の点検 2026-11-27）
        let start = self.content(&stage.cel(self.layer_id)?).only_faces_map(ids, stage.scope());
        let groups = restricted_groups(&self.groups, ids, stage.scope());
        stacked(start, groups.iter().map(PieceGroup::placed_map))?.checked()
    }

    /// 確定でセル画の地図 `cel`（階層 `scope`）に書くもの（1本の道の答え：`LayerBase::replace_local`）。確定の計算はこれ1つ
    /// （確定の計画 `plan_commit` と、複製のドラッグ中の止まった絵）。`None`：持ち上げた絵と違うセル画（`still_on`）か、
    /// 格子に置けない座標がある
    pub(crate) fn commit_edit(&self, cel: &Arc<LayerBase>, place: &CelPlace, scope: &[u64]) -> Option<LocalEdit> {
        if !self.still_on(place, cel) {
            return None;
        }
        let next = self.committed_local(cel)?;
        Some(cel.replace_local(&next, scope))
    }

    /// 確定した地図のしまう形：確定で書く残り（`Origin::kept`）に、置く組（`PieceGroup::placed`）を下から順に重ねる（組ごとに
    /// 変換をかけて、その座標の格子で正しい地図にする：`realized`。1枚ずつ窓で重ねる：`LayerBase::stacked_local`。残りの地図と
    /// 塊を共有する）。置かない組の所は、持ち上げる前の絵のまま。`None`：格子に置けない座標がある
    pub(crate) fn committed_local(
        &self,
        cel: &Arc<LayerBase>,
    ) -> Option<crate::stored_map::StoredMap> {
        self.placed_on(&self.kept_content(cel))
    }

    /// 地図 `base` に、置く組を下から順に重ねたしまう形（確定と、一部を確定したあとの残りの作り直しが使う重ね方はこれ1つ）。
    /// 組ごとに、組を置いたあとに、その組の上に重ねる線の形（`PieceGroup::lines_over`）のうち、`base` に線がまだある所を
    /// 重ね直す（`LayerBase::lines_along` の形：線だけの地図なので、色は両側とも前のまま、線の区間に線を乗せる。`base` に無い
    /// 線（持ち上げて動かした・消した・覆われた）は重ねない。手間はその線の大きさの分）
    fn placed_on(&self, base: &Arc<LayerBase>) -> Option<crate::stored_map::StoredMap> {
        base.stacked_local(self.groups.iter().filter(|g| g.placed).flat_map(|g| {
            let lines = g
                .lines_over_map()
                .map(|over| base.lines_along(over.map()).shape)
                .filter(|m| !m.edges.is_empty())
                .map(Some);
            std::iter::once(g.placed_map()).chain(lines)
        }), MapRule::Overlay)
    }
}

/// 浮いた片の中身（取り消しで戻すもの：レイヤーごとの残りの地図と片の組の地図・変換）。画面の写しと、計算に使う写しを
/// 生かしておくのは、セッションの浮いた片の見た目（`FloatingView`）。セッションは中身を分け合える箱で持ち、取り消しの
/// 控えは箱を分け合う（設計：claude/浮いた片の中身と写しを分ける設計_cb00886.md）
#[derive(Clone, Debug)]
pub(crate) struct Floating {
    layers: Vec<FloatLayer>,
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
    /// 上に重ねる線の図形（`PieceGroup::lines_over`。見せる中身にあるもの）と、その置き場所（守る組の置き場：線は組の
    /// 塗りより上に描く）。動かさない
    lines_over: Vec<(Placement, FillShape)>,
    /// 画面から隠すセル画の図形
    hidden: HashSet<ShapeId>,
    bases: Vec<Arc<LayerBase>>,
}

impl FloatingView {
    /// 浮いた片 `fl` の見た目：隠す図形、残りの図形（残りの地図の面のうちセル画に無いもの。そのレイヤーの最初の隠す
    /// 図形の所に置く）、片の図形（片の地図の面を「ステージへの変換 `world` ∘ 片の変換」で置いた写し）、上に重ねる線の
    /// 図形（見せる中身（残りか、無ければ土台 `stage` のセル画）にある線の写し。中身に無い線（持ち上げた・消した）は
    /// 描かない）。面の図形は、地図の計算に使う写しの面（`LayerBase::faces`）から作る（作ったばかりの写しは中身が預かって
    /// いるので、それを使う）
    pub(crate) fn of(fl: &Floating, world: Affine, stage: &Stage) -> Self {
        let scope = stage.scope();
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
                if let Some(over) = g.lines_over_map() {
                    let Some(content) = stage.cel(layer_id).map(|cel| l.content(&cel)) else {
                        continue;
                    };
                    // 見せる中身から取り出した上に重ねる線の形を、重なっている中身の線の番号で描く（確定で重ねる形と同じ）
                    let along = content.lines_along(over.map());
                    let mut m = along.shape;
                    for (ed, &id) in m.edges.iter_mut().zip(&along.shape_ids) {
                        ed.line_id = Some(id);
                    }
                    let base = LayerBase::of_stored(&StoredMap::from_dense(&m, None));
                    let at = Placement::AboveRaw(layer_id, scope.to_vec(), gi);
                    for f in base.drawn_shapes(scope).iter().filter(|f| f.line.is_some()) {
                        let mut s = (**f).clone();
                        s.layer_id = layer_id;
                        s.place(world);
                        view.lines_over.push((at.clone(), s));
                    }
                    view.bases.push(base);
                }
            }
        }
        view
    }

    /// 上に重ねる線の図形と、その置き場所（動かさない）
    pub(crate) fn placed_lines_over(&self) -> &[(Placement, FillShape)] {
        &self.lines_over
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
    pub(crate) fn new() -> Self {
        Self { layers: Vec::new() }
    }

    /// 確定でセル画を変えるか：置く組（動かした・色を変えた・文書に無い）があるか、元の所を切り抜いたまま書く残りがある
    /// （消した・取り出した）。`false` は選んで持ち上げただけで、捨てれば元に戻る
    pub(crate) fn modified(&self) -> bool {
        self.layers.iter().any(|l| {
            l.groups.iter().any(|g| g.placed)
                || l.origin.as_ref().is_some_and(|o| o.kept.id() != o.from.id())
        })
    }

    /// 持っている組をみな置く組にする（動かす・色を変える・消す・グループやシンボルへ取り出す前）：組の元の所はみな切り
    /// 抜いたまま書く（確定で書く残りを、画面に見せる残りにそろえる）
    fn place_all(&mut self) {
        for l in &mut self.layers {
            if let Some(o) = &mut l.origin {
                o.place_all();
            }
            for g in &mut l.groups {
                g.placed = true;
            }
        }
    }

    pub(crate) fn layers(&self) -> &[FloatLayer] {
        &self.layers
    }

    /// 番号が `keep` に無い片があるか
    pub(crate) fn has_pieces_outside(&self, keep: &HashSet<ShapeId>) -> bool {
        !self.ids_not_in(keep).is_empty()
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
    pub(crate) fn commit_edits(&self, stage: &Stage) -> Vec<Option<(LocalEdit, Arc<LayerBase>)>> {
        parallel_map(&self.layers, |l| {
            let base = stage.cel(l.layer_id)?;
            let r = l.commit_edit(&base, &stage.place(l.layer_id)?, stage.scope())?;
            Some((r, base))
        })
    }

    /// 同じ中身か（地図は同じしまう形、変換は同じ値。預かり物は見ない）
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        // 項目はみな分解で並べる（項目を足すと、比べるか比べないかを決めるまでコンパイラが止める）。預かり物（`fresh`）は
        // 計算に使う写し、隠す図形・残りの図形は残りの地図から決まるので比べない
        let same_origin = |x: &Origin, y: &Origin| {
            let Origin { place, rest, fresh: _, hidden: _, remainder: _, from, kept } = x;
            *place == y.place && rest.id() == y.rest.id() && from.id() == y.from.id() && kept.id() == y.kept.id()
        };
        let same_group = |g: &PieceGroup, h: &PieceGroup| {
            let PieceGroup { map, fresh: _, xform, placed, lines_over } = g;
            map.id() == h.map.id() && *xform == h.xform && *placed == h.placed && lines_over.id() == h.lines_over.id()
        };
        let same_layer = |a: &FloatLayer, b: &FloatLayer| {
            let FloatLayer { layer_id, origin, groups } = a;
            *layer_id == b.layer_id
                && match (origin, &b.origin) {
                    (None, None) => true,
                    (Some(x), Some(y)) => same_origin(x, y),
                    _ => false,
                }
                && groups.len() == b.groups.len()
                && groups.iter().zip(&b.groups).all(|(g, h)| same_group(g, h))
        };
        let Floating { layers } = self;
        layers.len() == other.layers.len() && layers.iter().zip(&other.layers).all(|(a, b)| same_layer(a, b))
    }

    /// テスト用：見せる残りと確定で書く残りの関係の確かめ（浮いた片を書き換える入口が、テストでは毎回呼ぶ：
    /// `Session::put_floating`）：見せる残りに、置かない組を元の所に戻すと、書く残りと同じ塗りになる（2つの残りは別々の
    /// 関数が書くので、どの操作で関係が崩れても、ここで止める：設計審査 4f8ef46 の D-1・D-2）。線は、色と太さごとの線の長さの
    /// 合計と外枠で比べる（戻すと切り口で辺が分かれたままになる（同じ線が2本の辺になる）ので、線の図形の数や面積では比べない。
    /// 前は線を比べなかったので、線だけが二重になる・消える食い違いを見逃しえた：設計審査 01e4283 の D-2）。塗りは、色ごとの
    /// 塗った所（面積と外枠）で比べる（線で分かれた
    /// 同じ色の塗りは、戻すと1つにつながることがあるので、塗りの数では比べない。切り口の曲線は、戻すと地図の精度 `FLAT_TOL`
    /// の中でずれることがあるので、外枠は 1/20px、面積は 1px² まで同じとみなす）。`Err`：違う所
    #[cfg(test)]
    pub(crate) fn check_views(&self, scope: &[u64]) -> Result<(), String> {
        // 塗り（色）と線（色と太さ）ごとの（面積か長さの合計, 外枠）
        type Painted = std::collections::BTreeMap<([u8; 4], u8), (f64, [f64; 4])>;
        let painted = |m: &StoredMap| -> Painted {
            use kurbo::ParamCurveArclen;
            let mut out = Painted::new();
            let mut add = |key: ([u8; 4], u8), amount: f64, b: kurbo::Rect| {
                let e = out.entry(key).or_insert((0.0, [f64::MAX, f64::MAX, f64::MIN, f64::MIN]));
                e.0 += amount;
                e.1 = [e.1[0].min(b.x0), e.1[1].min(b.y0), e.1[2].max(b.x1), e.1[3].max(b.y1)];
            };
            let base = LayerBase::of_stored(m);
            for f in base.drawn_shapes(scope).iter().filter(|f| f.line.is_none()) {
                let area = f.exterior().signed_area().abs() - f.holes().iter().map(|h| h.signed_area().abs()).sum::<f64>();
                add((f.color().to_array(), 0), area, f.bounding_box());
            }
            let map = base.map();
            for (e, ed) in map.edges.iter().enumerate() {
                if let Some(stroke) = ed.stroke {
                    let q = map.quad(e);
                    add((stroke.color.to_array(), stroke.width), q.arclen(1e-6), kurbo::Shape::bounding_box(&q));
                }
            }
            out
        };
        // 塗りの面積は 1px²、線の長さと外枠は 1/20px まで同じとみなす
        let same = |a: &Painted, b: &Painted| -> bool {
            a.len() == b.len()
                && a.iter().zip(b).all(|((ka, (aa, ba)), (kb, (ab, bb)))| {
                    let tol = if ka.1 == 0 { 1.0 } else { 0.05 };
                    ka == kb && (aa - ab).abs() <= tol && ba.iter().zip(bb).all(|(x, y)| (x - y).abs() <= 0.05)
                })
        };
        for l in &self.layers {
            let Some(o) = &l.origin else {
                continue;
            };
            let back = crate::planar_map::uncounted(|| {
                o.base()
                    .stacked_local(l.groups.iter().filter(|g| !g.placed).map(|g| Some(g.base().map().clone())), MapRule::Restore)
            })
            .ok_or("置かない組を戻せない")?;
            let (shown, kept) = crate::planar_map::uncounted(|| (painted(&back), painted(&o.kept)));
            if !same(&shown, &kept) {
                return Err(format!("レイヤー {}：見せる残り＋置かない組 {shown:?} と書く残り {kept:?} の塗りか線が違う", l.layer_id));
            }
        }
        Ok(())
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
        stage: &Stage,
        layers: &[(LayerId, HashSet<ShapeId>)],
        rect: Option<&FillShape>,
    ) -> Vec<LayerId> {
        let current = &self.layers;
        let results: Vec<Option<FloatLayer>> = parallel_map(layers, |(layer_id, ids)| {
            let cur = current.iter().find(|l| l.layer_id == *layer_id);
            cut_layer(cur, stage, *layer_id, ids, rect)
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
        self.place_all();
        for g in self.layers.iter_mut().flat_map(|l| &mut l.groups) {
            g.xform = doc * g.xform;
        }
    }

    /// レイヤー `layers` の片のうち、番号が `ids` の片の色を `color` にする（同じ色でとなり合う片は1つになる）。全部か
    /// 無しか：`Err`：色を変えた片の地図が正しい地図にならない（何も変えない）。`Ok`：変えたか
    pub(crate) fn recolor(
        &mut self,
        ids: &HashSet<ShapeId>,
        color: Color32,
        layers: &HashSet<LayerId>,
    ) -> Result<bool, Unplaceable> {
        let mut next: Vec<(usize, usize, CheckedMap)> = Vec::new();
        for (li, l) in self.layers.iter().enumerate() {
            if !layers.contains(&l.layer_id) {
                continue;
            }
            for (gi, g) in l.groups.iter().enumerate() {
                if let Some(m) = g.base().map().recolored(ids, color) {
                    next.push((li, gi, m.checked().ok_or(Unplaceable)?));
                }
            }
        }
        let any = !next.is_empty();
        if any {
            self.place_all();
        }
        for (li, gi, m) in next {
            let g = &mut self.layers[li].groups[gi];
            *g = g.replaced(m);
        }
        Ok(any)
    }

    /// レイヤー `layer_id` の置く組 `group` の片の地図（文書の座標、変換をかける前）を `edit` で編集する。`false`：その組が
    /// 無いか、置く組でない（置かない組は持ち上げる前の絵と同じなので、絵の方を編集する）か、編集できない
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
            .filter(|g| g.placed)
        else {
            return false;
        };
        let Some(next) = edit(g.base().map()).and_then(PlanarMap::checked) else {
            return false;
        };
        *g = g.replaced(next);
        true
    }

    /// 動かしていない片（置かない組の片）のうち、番号が `keep` に無い片を残りに戻す：外した片を残りの地図に重ねる（切り口は
    /// 両側で同じ線なので、同じ色でつながる所は1つになる：`cut_out`）。動かしていない片は書く残り（`Origin::kept`）に元の
    /// まま入っているので、書く残りは変えない（文書も書かない）。動かした片（置く組）は戻さない（外した動かした片は確定する
    /// もの：`commit_floating_except`）。片が無くなったレイヤーは外す（何も変わらない）。`false`：地図にできない
    pub(crate) fn return_pieces(&mut self, stage: &Stage, keep: &HashSet<ShapeId>) -> bool {
        let drop = self.ids_not_in(keep);
        for l in &mut self.layers {
            let layer_id = l.layer_id;
            let kept: Vec<PieceGroup> = l
                .groups
                .iter()
                .filter_map(|g| if g.placed { Some(g.clone()) } else { g.restricted(keep, stage.scope()) })
                .collect();
            if kept.is_empty() {
                l.origin = None;
                l.groups.clear();
                continue;
            }
            let unplaced: Vec<PieceGroup> = l.groups.iter().filter(|g| !g.placed).cloned().collect();
            let back = restricted_groups(&unplaced, &drop, stage.scope());
            if back.is_empty() {
                continue;
            }
            let (Some(cel), Some(place)) = (stage.cel(layer_id), stage.place(layer_id)) else {
                return false;
            };
            // 1本の道：外した片を中身に窓で重ね（`stacked_local`）、セル画の地図との違いを差分から先に渡す
            // （`replace_local`）。残りはセル画に対する答え
            let content = l.content(&cel);
            let Some(next) =
                content.stacked_local(back.iter().map(|g| Some(g.base().map().clone())), MapRule::Restore)
            else {
                return false;
            };
            let origin = match cel.replace_local(&next, stage.scope()) {
                LocalEdit::Same => None,
                LocalEdit::Changed(w) => Some(Origin::after(l.origin.as_ref(), RestChange::Unplaced, w, &cel, place.clone(), stage.scope())),
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
                // 上の組から順に見て、それまでに取り出す片の置いた地図（覆う所）を集める。取り出す片の線は、同じ組と下の組の
                // 残す塗りより上に見えているので、その下のいちばん近い残す組（同じ組の残す片か、無ければ下の組）に「上に
                // 重ねる線」として覚える（`PieceGroup::lines_over`。上の組の残す塗りは、今までどおり線を覆う）
                let (mut dropped, mut kept, mut cover) = (Vec::new(), Vec::new(), Vec::new());
                let mut lines_above: Vec<PlanarMap> = Vec::new();
                for g in l.groups.iter().rev() {
                    let k = match g.restricted(keep, scope) {
                        Some(k) => k.erased(&cover)?,
                        None => None,
                    };
                    if let Some(d) = g.restricted(&drop, scope) {
                        cover.push(d.placed_map().ok_or(Unplaceable)?);
                        let line_ids: HashSet<ShapeId> = d.ids().into_iter().filter(|&id| is_line_id(id)).collect();
                        if let Some(lines) = d.restricted(&line_ids, scope) {
                            lines_above.push(lines.placed_map().ok_or(Unplaceable)?);
                        }
                        dropped.push(d);
                    }
                    if let Some(mut k) = k {
                        if !lines_above.is_empty() {
                            k.add_lines_over(&lines_above).ok_or(Unplaceable)?;
                            lines_above.clear();
                        }
                        kept.push(k);
                    }
                }
                dropped.reverse();
                kept.reverse();
                Ok((dropped, kept))
            })
            .collect::<Result<Vec<_>, Unplaceable>>()?;
        let mut out = Floating::new();
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

    /// 一部を確定したあと（`split_off` で取り出した `committed` を書いたあと）、持ったまま残る置かない組のために、残りを作り直す。
    /// 確定する側には置く組しか無い（外した置かない組は、確定の前に `return_pieces` で残りに戻してある：`commit_floating_except`。
    /// 前は置かない組も確定する側に入れたので、ここで作る残りからその片が消えた：設計審査 4f8ef46 の D-1）：
    /// 出どころは書いたあとのセル画の地図、残りは、書く前の画面に見せる残りに、確定した置く組を重ねたもの（書いたセル画
    /// から、残る置かない組の元の所だけを切り抜いた地図と同じ。確定と同じ重ね方 `FloatLayer::placed_on`）、確定で書く残りは
    /// 出どころのまま（置かない組は確定でセル画を変えない）。置かない組が無いレイヤーは残りを持たない（置く組の元の所は、
    /// 書いた地図でもう切り抜いてある）。`false`：地図にできない
    pub(crate) fn rebase_after_commit(&mut self, stage: &Stage, committed: &Floating) -> bool {
        debug_assert!(
            committed.layers.iter().all(|l| l.groups.iter().all(|g| g.placed)),
            "一部の確定に、動かしていない片が入っている"
        );
        for l in &mut self.layers {
            if l.groups.iter().all(|g| g.placed) {
                continue;
            }
            let Some(done) = committed.layers.iter().find(|c| c.layer_id == l.layer_id) else {
                continue;
            };
            let Some(o) = &done.origin else {
                continue;
            };
            let (Some(cel), Some(place)) = (stage.cel(l.layer_id), stage.place(l.layer_id)) else {
                return false;
            };
            let Some(rest) = done.placed_on(&o.base()) else {
                return false;
            };
            l.origin = match cel.replace_local(&rest, stage.scope()) {
                LocalEdit::Same => None,
                LocalEdit::Changed(w) => Some(Origin::after(None, RestChange::Committed, w, &cel, place.clone(), stage.scope())),
                LocalEdit::Refused(_) => return false,
            };
        }
        true
    }

    /// 確かめた地図 `map`（文書の座標。面の番号が片の番号）を、変換 `xform` をかける片の組として、レイヤー `layer_id` の
    /// 一番上に足す（変換は、動かした片と同じく確定のときに地図にかける）
    pub(crate) fn add_map(&mut self, layer_id: LayerId, map: CheckedMap, xform: Affine) {
        let group = PieceGroup::new_placed(map, xform);
        self.layer_mut(layer_id).groups.push(group);
    }

    /// 番号が `keep` の片を、元の所から取り出す（シンボルの中身に入れる：元の所は切り抜いたまま書く。組をみな置く組にして
    /// から `split_off`）。この浮いた片には取り出した片が残り、確定する分（残りと、ほかの片）を返す。`Err`：片の地図が
    /// 正しい地図にならない（何も変えない：組を置く組にするのも、分けられると分かってから）
    pub(crate) fn take_out(&mut self, keep: &HashSet<ShapeId>, scope: &[u64]) -> Result<Floating, Unplaceable> {
        let mut next = self.clone();
        next.place_all();
        let rest = next.split_off(keep, scope)?;
        *self = next;
        Ok(rest)
    }

    /// 片を捨てる（削除：残りだけを確定する。持ち上げた所はみな切り抜いたまま書く）
    pub(crate) fn drop_pieces(&mut self) {
        self.place_all();
        for l in &mut self.layers {
            l.groups.clear();
        }
    }
}

/// 1つのレイヤーの切り出し：中身（残りの地図か、セル画の地図）を、選ぶ所（番号が `ids` の中身の面の所と、形 `rect` の
/// 所）で1回切り、中を片の組にする（`cut_out`。中と外は同じ地図を同じ所で切るので、切り口は両側で同じ線）。矩形が無い
/// とき（選んだ面を丸ごと持ち上げる）は切らずに、中身からその面を外す（番号はそのまま）。動かして
/// いない片（`modified` でないとき）は元の所にあるので、新しい片を前からの片に重ねて1つの組にする（切り口は同じ線
/// なので、同じ色でつながる片は1つになる）。動かした片があれば、新しい片は別の組として一番下に足す。ただし、組の上に
/// 重ねる線（`PieceGroup::lines_over`）だった線は、その組の塗りより上に見えているので、その組のすぐ上の別の組にする（見えて
/// いる重なりを変えない。一番下に入れると、確定で塗りに覆われて消える）。`None`：地図にできない
fn cut_layer(
    cur: Option<&FloatLayer>,
    stage: &Stage,
    layer_id: LayerId,
    ids: &HashSet<ShapeId>,
    rect: Option<&FillShape>,
) -> Option<FloatLayer> {
    let cel = stage.cel(layer_id)?;
    let place = stage.place(layer_id)?;
    let empty = FloatLayer {
        layer_id,
        origin: None,
        groups: Vec::new(),
    };
    let cur = cur.unwrap_or(&empty);
    let content_base = cur.content(&cel);
    // 切り出しても、確定で書く残りは変わらない（切り出した組は置かない組：決まりの表 `RestChange::Unplaced`）
    let (origin, inside): (Option<Origin>, CheckedMap) = match rect {
        // 面を丸ごと持ち上げる：中身から、その面を外す（面の番号はそのまま。片は前と同じ番号の図形。正しい地図の面の一部
        // なので確かめ直さない：`checked_part`）。外す面の輪郭の辺だけを替えたしまう形（`restricted_local`）を、セル画の
        // 地図に対する差分として1本の道に渡す（`replace_local`）
        None => {
            let part = (!ids.is_empty())
                .then(|| content_base.checked_part(Some(ids), stage.scope()))
                .filter(|m| !m.map().edges.is_empty());
            let Some(part) = part else {
                return Some(cur.clone());
            };
            let rest = content_base
                .restricted_local(FaceKeep::Without(ids), stage.scope())
                .unwrap_or_else(|| StoredMap::from_dense(&PlanarMap::default(), None));
            let origin = match cel.replace_local(&rest, stage.scope()) {
                LocalEdit::Same => None,
                LocalEdit::Changed(w) => Some(Origin::after(cur.origin.as_ref(), RestChange::Unplaced, w, &cel, place.clone(), stage.scope())),
                LocalEdit::Refused(_) => return None,
            };
            (origin, part)
        }
        // 選ぶ所で切る（1本の道：`cut_out`）。残りは中身に対する答えなので、セル画に対する残りにする
        Some(r) => {
            let part = (!ids.is_empty())
                .then(|| content_base.only_faces_map(ids, stage.scope()))
                .filter(|m| !m.edges.is_empty());
            let region_owned;
            let region = match part {
                None => MapInput::Shapes(std::slice::from_ref(r)),
                Some(p) => {
                    region_owned = region_map(&p, rect)?;
                    MapInput::Map(&region_owned)
                }
            };
            let (rest, inside) = cut_out(&cel, &content_base, region, stage.scope())?;
            let origin = match rest {
                LocalEdit::Changed(w) => Some(Origin::after(cur.origin.as_ref(), RestChange::Unplaced, w, &cel, place.clone(), stage.scope())),
                _ => None,
            };
            (origin, inside)
        }
    };
    let mut groups = cur.groups.clone();
    if inside.map().edges.is_empty() {
        return Some(FloatLayer { layer_id, origin, groups });
    }
    // 上の組から順に、その組の上に重ねる線だった線を、その組のすぐ上に入れる（入れても下の組の番号は変わらない）
    let mut lifted = PieceGroup::new(inside, Affine::IDENTITY);
    for gi in (0..groups.len()).rev() {
        let Some(shape) = groups[gi].lines_over_map() else {
            continue;
        };
        // 組の上に重ねる線（確定・見た目と同じ形：`LayerBase::lines_along`）を、その組のすぐ上の組にし、持ち上げた片から除く
        let over = lifted.base().lines_along(shape.map()).shape;
        if over.edges.is_empty() {
            continue;
        }
        let rest = lifted.base().map().overlay(MapInput::Map(&over), MapRule::Paint(None))?;
        groups.insert(gi + 1, PieceGroup::new(over.checked()?, Affine::IDENTITY));
        if rest.edges.is_empty() {
            return Some(FloatLayer { layer_id, origin, groups });
        }
        lifted = PieceGroup::new(rest.checked()?, Affine::IDENTITY);
    }
    match groups.first_mut() {
        Some(g) if !g.placed => {
            let joined = g
                .base()
                .map()
                .overlay(MapInput::Map(lifted.base().map()), MapRule::Restore)?
                .checked()?;
            *g = PieceGroup {
                lines_over: g.lines_over.clone(),
                ..PieceGroup::new(joined, Affine::IDENTITY)
            };
        }
        _ => groups.insert(0, lifted),
    }
    Some(FloatLayer {
        layer_id,
        origin,
        groups,
    })
}
