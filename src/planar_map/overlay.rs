//! 重ね合わせ：地図に形・地図・動かした辺を重ね、交わる所で切って、決まり（`MapRule`）で色を決め直した正しい地図を作る
//! （i_overlay で格子の上で切り、同じ曲線から来た区間を元の曲線に戻す）

use super::*;

// ============================================================================
// 形・地図を地図に重ねる（ブラシ・消しゴム・矩形で選んだ片の取り出し・持ち上げた片を置く・貼り付け）
//
// 重ねる線と前からある辺を細かい折れ線にして、i_overlay（vendor/i_overlay）の `EdgeOverlay` で交わる所で切る。
// 交わりは整数の格子（1/4096px）で計算するので、ほぼ重なった線でも食い違わない。切った区間ごとに、
// 左右が重ねる形の中か（i_overlay が数える）と、出どころ（どの曲線の t のどこからどこまでか）が分かる。
// 区間の左右の色は、区間をまたぐたびに出どころの辺の左右の色で隣の面へ伝えて決める（`propagate`）。境目の曲線は、
// 出どころの曲線から当て直す。
// ============================================================================

/// 重ねるときの格子（1px をいくつに分けるか）。地図の頂点のうち重ねる所にかかるものは、この格子の上に置き直す
pub(super) const OVERLAY_GRID: f64 = 4096.0;
/// 曲線を折れ線にするときの、曲線と折れ線の離れ具合の上限（px）。地図の精度で、同じ線・同じ点・2本のまとめの許容もこれから
/// 決まる。地図は保存され、編集のたびに切っては戻すので、描画の許容（その場限り）より細かくする：傾けた矩形で切って戻す
/// のをくり返したとき、0.002 では 300 回で線のずれ 0.0004px 以下・辺 +1 以下だが、0.02 では 10 回で 5.8px ずれて辺が
/// 溜まった（`repeated_cut_and_return_keeps_the_curves`。レビュー f01d518 の N-1001 の①は採らない）
pub(super) const FLAT_TOL: f64 = 0.002;
/// i_overlay が交点を格子の点に丸めるときに、点が動く距離の上限（格子の目の対角線の半分、px）
pub(super) const GRID_REACH: f64 = std::f64::consts::FRAC_1_SQRT_2 / OVERLAY_GRID;
/// i_overlay が切るときに、切れ目の点を、折れ線どうしの本当の交点から動かす距離の上限（px）：交点を格子の点に丸める分
/// （`GRID_REACH`）と、丸めた点から 1 目以内にある区間の端へ寄せる分（i_overlay の `Precision::HIGH` のはじめの半径：
/// 二乗で 1）。丸めで新しく交わった所を切る2回目からは半径を広げるので、そこで切れた点はこれより遠いことがある
pub(super) const SPLIT_REACH: f64 = GRID_REACH + 1.0 / OVERLAY_GRID;

/// 重ねるときの、中と外の色の決め方
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MapRule {
    /// 中をこの色にする（`None` は何も無い。消しゴム・面を空ける）。外は前のまま
    Paint(Option<Color32>),
    /// 中は前のまま、外を何も無いにする（矩形で選んだ中の片を取り出す）
    KeepInside,
    /// 重ねる地図の色のある所を、その色にする（何も無い所は前のまま）。持ち上げた片を置く・貼り付け
    Overlay,
    /// 輪郭の変形（重ねるものは `MapInput::Sweep`）。前の色と、動かしたことによる色ごとの巻き数の変わり方で決める
    /// （`swept_color`）
    Sweep,
}

/// 区間の片側（または中の辺）での、重ねるものの値
pub(super) enum Over {
    /// 重ねる形の中か（`MapInput::Shapes`）
    Inside(bool),
    /// 重ねる地図の色（`MapInput::Map`。色があれば中）か、下から上へ並べた形の、そこでいちばん上の形の色（`MapInput::Stack`）
    Color(Option<Color32>),
    /// 動かしたことによる、色ごとの巻き数の変わり方（`MapInput::Sweep`。並びは `swept_color`）
    Swept(Vec<(Option<Color32>, i32)>),
}

impl MapRule {
    /// `apply` の新しい色に、その側の図形の番号を付けたもの：重ねるものの色を上に置いたなら（重ねる地図・下から上へ並べた
    /// 形）重ねるものの番号 `over_src`、前の色のままなら前の番号、ほかは無し（新しい図形）
    pub(super) fn apply_label(self, over: &Over, over_src: Option<ShapeId>, old: Label) -> Label {
        let color = self.apply(over, old.0);
        let src = match (self, over) {
            (MapRule::Overlay, Over::Color(Some(_))) => over_src,
            _ if color == old.0 => old.1,
            _ => None,
        };
        (color, src)
    }

    /// 重ねるものの値 `over` と前の色 `old` から、新しい色
    pub(super) fn apply(self, over: &Over, old: Option<Color32>) -> Option<Color32> {
        match (self, over) {
            (MapRule::Paint(c), Over::Inside(true) | Over::Color(Some(_))) => c,
            (MapRule::KeepInside, Over::Inside(false) | Over::Color(None)) => None,
            (MapRule::Overlay, Over::Color(Some(p))) => Some(*p),
            (MapRule::Sweep, Over::Swept(w)) => swept_color(old, w),
            _ => old,
        }
    }
}

/// 輪郭の変形での新しい色（設計：claude/輪郭の変形を重ね合わせで確定する設計_08c15dd段階3_cf66a87.md の 2-1）。
/// `w` は、動かしたことによる色ごとの巻き数の変わり方（動かした辺の並びで、左・右の順）。前の色を足した巻き数が正の色の
/// うち、広がってきた色（前の色でない色）があれば、並びで先のもの。無ければ、前の色の巻き数が正なら前の色、でなければ何も無い
pub(super) fn swept_color(old: Option<Color32>, w: &[(Option<Color32>, i32)]) -> Option<Color32> {
    match w.iter().find(|x| x.0 != old && x.1 > 0) {
        Some(x) => x.0,
        None if 1 + w.iter().filter(|x| x.0 == old).map(|x| x.1).sum::<i32>() > 0 => old,
        None => None,
    }
}

/// 輪郭の変形で動かした辺：（辺の番号, 後の曲線）
pub(crate) type MovedEdge = (usize, QuadBez);

/// 重ねるもの：塗りの形（中は巻き数が奇数の所。i_overlay が数える）か、地図（中は色のある面。その座標で正しい地図で
/// あること：前の地図の線に触れない辺は、そのまま写す）か、輪郭の変形で動かした辺（`MapRule::Sweep` で重ねる。後の曲線を
/// 重ね、前の曲線＝元の地図の辺を引く分として数える）か、下から上へ並べた塗りの形（色は、巻き数が奇数の形のうち並びで
/// いちばん上の形の色。重なる所は上の形が見える、という絵の決まりそのもの。`MapRule::Overlay` で重ねる）
#[derive(Clone, Copy)]
pub(crate) enum MapInput<'a> {
    Shapes(&'a [FillShape]),
    Map(&'a PlanarMap),
    Sweep(&'a [MovedEdge]),
    Stack(&'a [&'a FillShape]),
}

impl MapInput<'_> {
    /// 重ね合わせで、前の地図の辺の後ろに並べる辺の数（重ねる地図の辺。`PlanarMap::joined`）
    pub(super) fn joined_edges(&self) -> usize {
        match self {
            MapInput::Map(p) => p.edges.len(),
            _ => 0,
        }
    }

    /// 辺ごとの、輪郭の変形で動かした辺か
    pub(super) fn moved_flags(&self, n: usize) -> Vec<bool> {
        let mut flags = vec![false; n];
        if let MapInput::Sweep(moved) = self {
            for &(e, _) in *moved {
                flags[e] = true;
            }
        }
        flags
    }
}

/// 区間の出どころ：（出どころの曲線の番号, 区間の始点の t, 終点の t, 区間の向き（始点→終点）が出どころの曲線の t の増える
/// 向きか）。向きは t の大小から求めない：i_overlay が交点を格子へ丸めて区間の端の外へ寄せると、切った区間の t の幅が 0 に
/// なることがある（`split` は端で止める）。向きは区間を作るときに決め、切っても変わらず、区間を裏返すときだけ変わる
#[derive(Clone, Copy, Debug)]
pub(super) struct Prov {
    src: u32,
    t0: f64,
    t1: f64,
    fwd: bool,
}

/// 区間の出どころの一覧をしまう所（i_overlay が区間を切る・まとめる・向きを変えるたびに増える）。一覧はみな1つの並びに
/// 入れ、一覧ごとの範囲を持つ（一覧ごとに並びを持つと、区間ごとに小さな確保が起きる。`CellGrid` と同じ理由）
#[derive(Default)]
pub(super) struct ProvStore {
    provs: Vec<Prov>,
    /// 一覧ごとの、`provs` の範囲
    lists: Vec<(u32, u32)>,
}

impl ProvStore {
    /// 出どころ `items` の一覧を足す（番号を返す）
    pub(super) fn push(&mut self, items: impl IntoIterator<Item = Prov>) -> ProvId {
        let start = self.provs.len() as u32;
        self.provs.extend(items);
        self.lists.push((start, self.provs.len() as u32));
        ProvId(self.lists.len() as u32 - 1)
    }

    /// 一覧 `ids` をこの順につなぎ、出どころごとに `f` をかけた一覧を足す（番号を返す）
    fn push_mapped(&mut self, ids: &[ProvId], f: impl Fn(Prov) -> Prov) -> ProvId {
        let start = self.provs.len() as u32;
        for id in ids {
            let (a, b) = self.lists[id.0 as usize];
            for i in a..b {
                let p = f(self.provs[i as usize]);
                self.provs.push(p);
            }
        }
        self.lists.push((start, self.provs.len() as u32));
        ProvId(self.lists.len() as u32 - 1)
    }

    /// 一覧 `id`
    pub(super) fn get(&self, id: ProvId) -> &[Prov] {
        let (a, b) = self.lists[id.0 as usize];
        &self.provs[a as usize..b as usize]
    }
}

/// 区間の出どころの一覧の番号（`ProvStore` の中）。i_overlay の区間に付ける印
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct ProvId(u32);

impl i_overlay9::core::edge_data::OverlayEdgeData for ProvId {
    type Store = ProvStore;

    /// 重なった区間の数が打ち消し合っても、区間は残す（交点を格子に丸めて向きが変わった2本の区間が重なると、i_overlay は
    /// 数の符号を変えて足すので 0 になる。vendor/i_overlay/PATCH.md）
    fn keep_when_count_cancels(&self) -> bool {
        true
    }

    fn reversed(self, store: &mut ProvStore) -> Self {
        store.push_mapped(&[self], |p| Prov {
            src: p.src,
            t0: p.t1,
            t1: p.t0,
            fwd: !p.fwd,
        })
    }

    fn split<I: i_overlay9::i_float::int::number::int::IntNumber>(
        self,
        ctx: i_overlay9::core::edge_data::EdgeDataSplit<I>,
        store: &mut ProvStore,
    ) -> (Self, Self) {
        let (ax, ay) = (ctx.a.x.to_f64(), ctx.a.y.to_f64());
        let (dx, dy) = (ctx.b.x.to_f64() - ax, ctx.b.y.to_f64() - ay);
        let len2 = dx * dx + dy * dy;
        let f = if len2 > 0.0 {
            (((ctx.p.x.to_f64() - ax) * dx + (ctx.p.y.to_f64() - ay) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let mid = move |p: &Prov| p.t0 + (p.t1 - p.t0) * f;
        let first = store.push_mapped(&[self], |p| Prov { t1: mid(&p), ..p });
        let second = store.push_mapped(&[self], |p| Prov { t0: mid(&p), ..p });
        (first, second)
    }

    fn merge(
        ctx: i_overlay9::core::edge_data::EdgeDataMerge<
            i_overlay9::segm::boolean::ShapeCountBoolean,
            Self,
        >,
        store: &mut ProvStore,
    ) -> Self {
        store.push_mapped(&[ctx.lhs_data, ctx.rhs_data], |p| p)
    }
}

/// 重ねるときの曲線の出どころ
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum OverlaySrc {
    /// 前からある辺（番号）
    Old(usize),
    /// 重ねる形の区間・重ねる地図の辺（番号）
    New(usize),
}

/// 曲線を、折れ線とのずれが `tol` ほどになるように等分した点と t（等分の点の式はこれ1つ）
pub(super) fn flat_points(q: QuadBez, tol: f64) -> impl Iterator<Item = (Point, f64)> {
    let n = flat_count(&q, tol);
    (0..=n).map(move |i| {
        let t = i as f64 / n as f64;
        (q.eval(t), t)
    })
}

/// 2次ベジェを n 等分した折れ線と曲線のずれは、およそ（多くとも）`bend / (4 n²)`。その `bend`＝|p0 − 2·p1 + p2|
pub(super) fn bend(q: &QuadBez) -> f64 {
    (q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2()).hypot()
}

/// 曲線を n 等分した折れ線と曲線のずれ（`bend`）
pub(super) fn flat_dev(q: &QuadBez, n: usize) -> f64 {
    bend(q) / (4.0 * (n * n) as f64)
}

/// 曲線を、折れ線とのずれ（`flat_dev`）が `tol` ほどになるように等分する数（上限は置かない：座標は格子の範囲
/// ±65,536px に収まるので、地図の精度 `FLAT_TOL` で約 8,100 以下。どの大きさの曲線でも同じ精度になる）
pub(super) fn flat_count(q: &QuadBez, tol: f64) -> usize {
    ((bend(q) / (4.0 * tol)).sqrt().ceil() as usize).max(1)
}

/// 整数の向きを、角度の順（x 軸の正の向きから、面積の式で正の回り方）に比べる（向きは格子の点の差か、その積。
/// 外積は i128 に収まる）
pub(super) fn cmp_dir(a: (i128, i128), b: (i128, i128)) -> std::cmp::Ordering {
    let half = |d: (i128, i128)| {
        if d.1 > 0 || (d.1 == 0 && d.0 > 0) {
            0u8
        } else {
            1
        }
    };
    half(a)
        .cmp(&half(b))
        .then_with(|| 0i128.cmp(&(a.0 * b.1 - a.1 * b.0)))
}

/// 格子の点の差を、`cmp_dir` で比べる幅にする
pub(super) fn wide(d: IPt) -> (i128, i128) {
    (d.0 as i128, d.1 as i128)
}

impl PlanarMap {
    /// 点 `p`（格子の点に丸める）の色：判定の形で、点のある所の色（`LabeledSegs::at`）
    #[cfg(test)]
    pub(crate) fn color_at(&self, p: Point) -> Option<Color32> {
        self.color_locator()(p)
    }

    /// テスト用：点の色を何度も調べるときの `color_at`（判定の形を1回だけ作る）
    #[cfg(test)]
    pub(crate) fn color_locator(&self) -> impl Fn(Point) -> Option<Color32> {
        let segs = LabeledSegs::new((0..self.edges.len()).flat_map(|e| {
            let (l, r) = (self.edges[e].left, self.edges[e].right);
            let pl = grid_poly(&self.quad(e));
            (0..pl.len().saturating_sub(1))
                .map(|k| (pl[k], pl[k + 1], l, r))
                .collect::<Vec<_>>()
        }));
        move |p| {
            let g = gpt(p);
            segs.at((2 * g.0, 2 * g.1)).flatten()
        }
    }

    /// バケツ：点 `p` を含む面（辺で囲まれた所。何も塗っていない所も）を `color` にした地図。`None`：変わらない（どの
    /// 輪郭にも囲まれていない所・輪郭の上・すでに `color` の面）。テスト用（本番は計算に使う写しの判定の形で：
    /// `LayerBase::with_face_filled`）
    #[cfg(test)]
    pub(crate) fn with_face_filled(&self, p: Point, color: Color32) -> Option<PlanarMap> {
        let polys = self.polys();
        self.face_filled_with(&polys, &self.face_cycles_with(&polys), p, color)
    }

    #[cfg(test)]
    /// `with_face_filled` を、辺ごとの判定の形 `polys` と面の輪郭 `cycles` で（計算に使う写しが持つもの：
    /// `LayerBase::with_face_filled`）
    pub(super) fn face_filled_with(
        &self,
        polys: &[Poly],
        cycles: &[Vec<(usize, bool)>],
        p: Point,
        color: Color32,
    ) -> Option<PlanarMap> {
        let regions = self.regions(polys, cycles, true);
        // 点を含む組（正しい地図では、組どうしは重ならないので1つまで）。点が輪郭の上にあれば、どの面か決まらないので
        // 何も変えない
        let g = gpt(p);
        let p2 = (2 * g.0, 2 * g.1);
        let mut found = None;
        for gi in regions.groups_at(p2) {
            match regions.contains(gi, p2) {
                None => return None,
                Some(true) => {
                    found = Some(gi);
                    break;
                }
                Some(false) => {}
            }
        }
        let (group_color, ids) = &regions.groups[found?];
        if *group_color == Some(color) {
            return None;
        }
        // 組の面の輪郭の、面の側（向きどおりなら左）
        let sides = ids.iter().flat_map(|&k| regions.cycles[k].iter().copied());
        Some(self.with_sides_colored(sides, color))
    }

    /// 形か地図か輪郭の変形で動かした辺 `input` を、`rule` の決め方で重ねた地図
    /// （設計：claude/曲線の当て直しを決まりで行う設計_7215b88.md、claude/格子の折れ線で判定する設計_段階2_d90cabe.md、
    /// claude/輪郭の変形を重ね合わせで確定する設計_08c15dd段階3_cf66a87.md）。
    ///
    /// 1. i_overlay に入れる前からある辺を、交わりで決める（`fed_edges`。輪郭の変形で動かした辺はいつも入れる）。重ねる線と一緒に細かい折れ線にして格子に置き、
    ///    i_overlay で交わる所で切る。前からある辺は Subject、重ねる線は Clip（偶奇の規則。重ねる形は重なりの無い形にしておくこと）
    /// 2. 入れなかった辺の判定の形が、切った区間に触れていたら、その辺も入れて切り直す（i_overlay は交点を格子の点に丸め、
    ///    区間の端に近い交点は端へ寄せ、寄せる距離をくり返しのたびに広げるので、区間は折れ線から半目より遠くへ動くことが
    ///    ある。入れる辺は増える一方なので、必ず終わる）
    /// 3. 切った区間ごとに、左右の前の色・重ねるものの値（中か・重ねる地図の色・動かしたことによる巻き数の変わり方）を決め、
    ///    `rule` で新しい色にする。左右が同じ区間は捨てる。区間の頂点のまわりで、となり合う線（区間か、入れなかった辺）の間の
    ///    面の色が食い違えば、その入れなかった辺も入れて切り直す（入れなかった辺の色は自分の所で決めるので、i_overlay が
    ///    区間を動かすと、面ごとに決めた区間の色と食い違いうる）
    /// 4. 区間をすべて直線の辺にした地図から始め、同じ曲線から来た区間の鎖ごとに、決まりを満たすときだけ元の曲線の一部に
    ///    置き換える（`ChainRestorer`）
    ///
    /// 格子に置けない座標（±65,536px の外）があるときは `None`
    pub(crate) fn overlay(&self, input: MapInput<'_>, rule: MapRule) -> Option<PlanarMap> {
        let [m] = self.overlay_rules(input, [rule])?;
        Some(m)
    }

    /// `overlay` を、この地図の辺の計算に使う形 `known`（辺の順。計算に使う写しが持つもの：`LayerBase::overlay`）で
    pub(super) fn overlay_known(
        &self,
        known: &[Poly],
        index: Option<MapIndex<'_>>,
        input: MapInput<'_>,
        rule: MapRule,
    ) -> Option<PlanarMap> {
        let [m] = self.overlay_rules_known(known, index, input, [rule])?;
        Some(m)
    }

    /// 同じ重ねるもの `input` を、決まりごとに重ねた地図（`rules` の順。手順は `overlay`）。決まりによらない所（1・2 と、
    /// 3 の区間の左右の色と重ねるものの値）は1回だけ行うので、どの地図も同じ区間・同じ頂点から作る（矩形で一部を選ぶときの
    /// 中と外：`cut_out`。設計：claude/N-2002の残り・R5・★4・★5の設計_80fe785.md）。3 の頂点のまわりの確かめは決まりごとに
    /// 行い、どれかで食い違えば、その辺を入れてみな切り直す
    pub(crate) fn overlay_rules<const N: usize>(
        &self,
        input: MapInput<'_>,
        rules: [MapRule; N],
    ) -> Option<[PlanarMap; N]> {
        self.overlay_rules_known(&[], None, input, rules)
    }

    /// `overlay_rules` を、この地図の前から `known.len()` 本の辺の計算に使う形 `known` で（近くの辺を探す外接矩形と
    /// マスを作り直さない：`fed_edges`）
    pub(super) fn overlay_rules_known<const N: usize>(
        &self,
        known: &[Poly],
        index: Option<MapIndex<'_>>,
        input: MapInput<'_>,
        rules: [MapRule; N],
    ) -> Option<[PlanarMap; N]> {
        let mut new_quads: Vec<QuadBez> = Vec::new();
        let mut new_cols: Vec<(Option<Color32>, Option<Color32>)> = Vec::new();
        match input {
            MapInput::Shapes(shapes) => {
                new_quads.extend(
                    shapes
                        .iter()
                        .flat_map(|s| s.contours())
                        .flat_map(|c| c.segments.iter().copied()),
                );
                // 形の線の端を、前からある頂点から `FLAT_TOL` 以内なら、その頂点（いちばん近いもの）に寄せる。地図の精度
                // では同じ点なので、寄せないと2つの頂点が格子の別々の点に残り、同じ線として扱う所（`Span`）の端で2本の
                // 折れ線が分かれて、見えない欠片の面ができる（`stitched` は線の端にいつも自分の端の点を置く。レビュー
                // 4cb53aa の N-1002）。続く線の端は同じ点なので、輪郭は閉じたまま。形は偶奇の規則で数えるので、寄せて
                // 輪郭が自分に触れても重ね合わせは成り立つ。重ねる地図の頂点は寄せない：重ねる地図は正しい地図であることを
                // 前提に、辺どうしを調べずに入れるので（`fed_edges`）、地図の中の 0.002px 以内の2つの頂点が1つに寄って
                // 重なると、結果が正しい地図でなくなる
                if let Some(area) = new_quads
                    .iter()
                    .flat_map(|q| [q.p0, q.p2])
                    .map(|p| KRect::from_points(p, p))
                    .reduce(|a, b| a.union(b))
                {
                    let near = self.node_finder_in(FLAT_TOL, Affine::IDENTITY, area, index);
                    let snap = |p: Point| near(p, None).map_or(p, |v| self.nodes[v]);
                    for q in &mut new_quads {
                        q.p0 = snap(q.p0);
                        q.p2 = snap(q.p2);
                    }
                }
            }
            MapInput::Stack(shapes) => {
                new_quads.extend(
                    shapes
                        .iter()
                        .flat_map(|s| s.contours())
                        .flat_map(|c| c.segments.iter().copied()),
                );
            }
            MapInput::Map(p) => {
                for e in 0..p.edges.len() {
                    new_quads.push(p.quad(e));
                    new_cols.push((p.edges[e].left, p.edges[e].right));
                }
            }
            // 後の曲線を、元の辺の左右の色で重ねる。元の辺（前の曲線）はいつも入れる
            MapInput::Sweep(moved) => {
                for &(e, after) in moved {
                    new_quads.push(after);
                    new_cols.push((self.edges[e].left, self.edges[e].right));
                }
            }
        }
        if new_quads.is_empty() {
            // 重ねるものが無い：外は前のまま（中だけ残すなら、何も残らない）
            return Some(rules.map(|rule| match rule {
                MapRule::KeepInside => PlanarMap::default(),
                _ => self.clone(),
            }));
        }
        // 計算に使う写しの升目の索引があれば、形・輪郭の変形は窓で行う（結果は前の地図全体で行ったときと同じ）
        if let (Some(ix), MapInput::Shapes(_) | MapInput::Sweep(_) | MapInput::Map(_)) =
            (index, input)
        {
            let got = self.overlay_windowed(known, ix, input, rules, &new_quads, &new_cols);
            // お手本（設計 段2b の 0.）：テストでは毎回、前の地図全体で重ねた結果と比べる
            #[cfg(test)]
            if super::test_checks() {
                let same = super::uncounted(|| {
                    let full = self.overlay_rules_known(known, None, input, rules);
                    super::same_results(got.as_ref().map(|m| &m[..]), full.as_ref().map(|m| &m[..]))
                });
                assert!(
                    same,
                    "窓で重ねた結果が、前の地図全体で重ねた結果（お手本）と違う"
                );
            }
            return got;
        }
        // 重ねる地図の辺も、前からある辺と同じ決まりで入れるかを決める（入れない辺はそのまま写し、色だけ決める）。そのために
        // 2つの地図の辺を1つに並べる（前からある辺が 0..n_old、重ねる地図の辺がその後ろ）。形と輪郭の変形の線はみな入れる
        let joined;
        let mut partner: Vec<Option<(usize, bool)>> = Vec::new();
        let (cands, seeds): (&PlanarMap, &[QuadBez]) = match input {
            MapInput::Map(p) => {
                let (pairs, alias) = self.same_edges(p);
                joined = self.joined(p, &alias);
                partner = vec![None; joined.edges.len()];
                for &(e, f, same_dir) in &pairs {
                    let f = self.edges.len() + f;
                    partner[e] = Some((f, same_dir));
                    partner[f] = Some((e, same_dir));
                }
                (&joined, &[])
            }
            _ => (self, &new_quads),
        };
        let check = self.edges.len()..cands.edges.len();
        let moved = input.moved_flags(cands.edges.len());
        let mut fed = cands.fed_edges(
            known,
            index,
            seeds,
            &moved,
            moved.clone(),
            check.clone(),
            &partner,
        )?;
        loop {
            match cands.overlay_with(input, &rules, &new_quads, &new_cols, &mut fed) {
                Ok(maps) => return Some(maps.try_into().expect("決まりごとに1つの地図")),
                Err(OverlayStop::Unfed(more)) => {
                    let mut set = std::mem::take(&mut fed.fed);
                    for e in more {
                        set[e] = true;
                    }
                    fed = cands.fed_edges(
                        known,
                        index,
                        seeds,
                        &moved,
                        set,
                        check.clone(),
                        &partner,
                    )?;
                }
                Err(OverlayStop::Refused) => return None,
            }
        }
    }

    /// `overlay_rules_known` を、窓で行う（形・輪郭の変形のとき。設計：claude/触った所だけを計算する地図の設計_22fbbbe.md
    /// の 3.4）。
    ///
    /// 1. 入れる辺を、前の地図全体で決める（`fed_edges`。升目の索引を引くので、手間は重ねるものの近くの分）
    /// 2. 窓：入れる辺と重ねるものの外接矩形（制御点まで）を広げた範囲 `region` に外接矩形がかかる辺と、それらの端の頂点に
    ///    つながる辺。前の地図の辺の番号の順に並べ、頂点も前の地図の番号の順にする（窓の中の番号の順は、前の地図の番号の
    ///    順と同じ）
    /// 3. 窓で重ね合わせる（`overlay_with`）。前の地図全体で行ったときと比べて違いうるのは、窓に無い辺を見る所だけで、
    ///    それは次の3つ。どれも結果を同じにする：
    ///    - 点のある所の前の地図の色を数える所は、窓の外の辺も前の地図の番号の順に数える（`Outside`）
    ///    - 中の辺（重ねるものの外接矩形にかかる入れない辺）・入れる辺の端の頂点につながる辺・2本をまとめる候補の頂点に
    ///      つながる辺は、みな窓にある
    ///    - 曲線に戻す所と2本をまとめる所がほかの辺を調べた範囲（`FedEdges::used`）が `region` に入っていなければ、
    ///      `region` を広げてやり直す（`region` に外接矩形がかかる辺はみな窓にあるので、入っていれば調べた相手は同じ）
    /// 4. 窓の外の辺（外が前のままの決まりなら）と、窓の結果を合わせて、決まりごとの地図にする
    ///
    /// 入れなかった辺が切った区間と交わったとき（`Unfed`）は、前の地図全体の入れる辺を足して 1. からやり直す（前の地図
    /// 全体で行うときと同じ）
    fn overlay_windowed<const N: usize>(
        &self,
        _known: &[Poly],
        ix: MapIndex<'_>,
        input: MapInput<'_>,
        rules: [MapRule; N],
        new_quads: &[QuadBez],
        new_cols: &[(Option<Color32>, Option<Color32>)],
    ) -> Option<[PlanarMap; N]> {
        let (maps, wedges) = Self::window_overlay(ix, input, &rules, new_quads, new_cols)?;
        // 窓の頂点の、前の地図での番号（窓の結果を前の地図全体の結果に合わせるため：`with_window_result`）
        let wnodes: Vec<usize> = {
            let mut v: Vec<usize> = wedges
                .iter()
                .flat_map(|&e| [self.edges[e].a, self.edges[e].b])
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let to_window: IntMap<usize, usize> =
            wedges.iter().enumerate().map(|(i, &e)| (e, i)).collect();
        let mut out = maps.into_iter();
        Some(rules.map(|rule| {
                        let m = out.next().expect("決まりごとに1つの地図");
                        let copy = rule != MapRule::KeepInside;
                        // お手本（設計 段2b の 4. の 2b-2）：前のしまう形に窓の差分を当てた形が、窓の結果を合わせた地図の
                        // しまう形と同じ
                        #[cfg(test)]
                        let diffed = (copy && super::test_checks()).then(|| {
                            super::uncounted(|| {
                                let (removed, added) =
                                    crate::stored_map::window_diff(self, &wedges, &m);
                                let local =
                                    LocalMap::new(ix, &m, &removed, &added);
                                let touched = local.touched_cycles();
                                let edges_ok = local.added_edges_ok();
                                let mut memo = CycleMemo::default();
                                let touched_ok = local.touched_cycles_ok(&mut memo);
                                let cycles_ok = touched_ok.is_some();
                                // 確かめ全体の答え（設計 段2b の 6. の 5.）
                                let prev = LayerBase::new(self);
                                let table = prev.faces_table();
                                let lookup = |id: ShapeId| table.get(&id).cloned().unwrap_or_default();
                                let local_ok = edges_ok
                                    && touched_ok.as_ref().is_some_and(|t| {
                                        local.regions_ok(t, &lookup, &mut memo) == Some(true)
                                    });
                                // 触った面（設計 段2b の 6. の 5.）：番号によらない形にして、前の地図に無いものの集まり
                                let local_faces = local_ok.then(|| {
                                    let faces = local
                                        .touched_faces(touched_ok.as_ref().expect("閉じた"), &lookup, &mut memo)
                                        .expect("正しい地図なら面が作れる");
                                    let prev_faces = prev.faces(&[]);
                                    let orig_shape = |id: ShapeId| {
                                        prev_faces
                                            .binary_search_by_key(&id, |s| s.shape_id)
                                            .ok()
                                            .map(|i| Arc::clone(&prev_faces[i]))
                                    };
                                    let change =
                                        local.rewrite(&faces, &lookup, &orig_shape, &memo, 0, &[]);
                                    // 触った面ごとに1つずつ別の番号
                                    let mut ids = change.ids.clone();
                                    ids.sort_unstable();
                                    ids.dedup();
                                    assert_eq!(ids.len(), faces.len(), "触った面の番号がみな別");
                                    let base_keys: std::collections::HashSet<FaceShape> =
                                        faces_shape_keys(self).into_iter().collect();
                                    let mut keys: Vec<FaceShape> = faces
                                        .iter()
                                        .map(|f| local.face_shape_key(f, &memo))
                                        .filter(|k| !base_keys.contains(k))
                                        .collect();
                                    keys.sort();
                                    (keys, base_keys, change)
                                });
                                (ix.stored.with_diff(self, &removed, &m, &added), (edges_ok, cycles_ok, local_ok, local_faces), touched.map(|t| {
                                    let mut shapes: Vec<_> = t
                                        .iter()
                                        .map(|c| cycle_shape(c.iter().map(|&h| local.half_key(h))))
                                        .collect();
                                    shapes.sort();
                                    shapes
                                }))
                            })
                        });
                        let joined = self.with_window_result(m, &wnodes, &to_window, copy);
                        #[cfg(test)]
                        if let Some((d, (edges_ok, cycles_ok, local_ok, local_faces), touched)) = diffed {
                            // お手本：触った面のうち前の地図に無いものの集まりが、新しい地図全体の面のうち前の地図に
                            // 無いものの集まりと同じ
                            if let Some((keys, base_keys, change)) = local_faces {
                                // お手本（設計 段2b の 4. の 2b-6）：書き戻しの差分が、新しい地図全体から出した差分と同じ
                                // （新しく振る番号は回ごとに違うので、元の図形の番号を引き継いだかだけを比べる）
                                let same = super::uncounted(|| {
                                    let prev = LayerBase::new(self);
                                    let next = joined.clone().written(&prev).expect("正しい地図");
                                    let own_faces = prev.faces(&[]);
                                    let own: Vec<&FillShape> = own_faces.iter().map(|s| &**s).collect();
                                    let full = prev.diff(next, &own, 0, &[]);
                                    let orig: std::collections::HashSet<ShapeId> =
                                        own.iter().map(|s| s.shape_id).collect();
                                    let norm = |shapes: &[FillShape]| {
                                        let mut v: Vec<_> = shapes
                                            .iter()
                                            .map(|s| {
                                                let bits = |c: &Contour| -> Vec<[u64; 6]> {
                                                    c.segments
                                                        .iter()
                                                        .map(|q| {
                                                            [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y]
                                                                .map(f64::to_bits)
                                                        })
                                                        .collect()
                                                };
                                                (
                                                    orig.contains(&s.shape_id).then_some(s.shape_id),
                                                    s.color.to_array(),
                                                    s.contours().map(bits).collect::<Vec<_>>(),
                                                )
                                            })
                                            .collect();
                                        v.sort();
                                        v
                                    };
                                    // 前の地図がセル画の地図（書き戻したもの：書き戻しても面が変わらない）のときだけ比べる（面に
                                    // 番号の無い地図は、書き戻すと触らない面にも番号を振るので、触った所だけの答えとは違う）
                                    let settled = self
                                        .clone()
                                        .written(&prev)
                                        .is_some_and(|n| prev.diff(n, &own, 0, &[]).removed.is_empty());
                                    let mut a = change.removed.clone();
                                    a.sort_unstable();
                                    let mut b = full.removed.clone();
                                    b.sort_unstable();
                                    !settled || (a == b && norm(&change.added) == norm(&full.added))
                                });
                                assert!(
                                    same,
                                    "触った面だけで決めた書き戻しの差分が、新しい地図全体から出した差分（お手本）と違う"
                                );
                                let full: Vec<FaceShape> = super::uncounted(|| {
                                    faces_shape_keys(&joined)
                                        .into_iter()
                                        .filter(|k| !base_keys.contains(k))
                                        .collect()
                                });
                                assert!(
                                    keys == full,
                                    "触った所だけで作った面が、新しい地図全体の面（お手本）と違う：触った所 {} 面、全体 {} 面",
                                    keys.len(),
                                    full.len()
                                );
                            }
                            // お手本：確かめ全体の答えが、新しい地図全体を確かめた答え（`written` と同じ判定）と同じ
                            let full_ok =
                                super::uncounted(|| LayerBase::new(self).written_ok(&joined));
                            assert_eq!(
                                local_ok, full_ok,
                                "触った所だけで確かめた答えが、新しい地図全体を確かめた答え（お手本）と違う"
                            );
                            // お手本（設計 段2b の 6. の 2.）：触った輪郭の決まりの答えが、新しい地図全体の輪郭で調べた答えと同じ
                            let full_cycles_ok =
                                super::uncounted(|| LayerBase::cycles_ok_full(&joined));
                            assert_eq!(
                                cycles_ok, full_cycles_ok,
                                "触った輪郭の決まりの答えが、新しい地図全体の輪郭で調べた答え（お手本）と違う"
                            );
                            // お手本（設計 段2b の 6. の 1.）：増えた辺の決まりの答えが、新しい地図全体で調べた答えと同じ
                            let full_ok = super::uncounted(|| {
                                let prev = LayerBase::new(self);
                                let (polys, origin) = joined.polys_since(Some(&prev));
                                let fresh: Vec<bool> =
                                    origin.iter().map(|o| matches!(o, EdgeOrigin::New)).collect();
                                joined.fits_grid(Affine::IDENTITY)
                                    && joined.edges.iter().all(|e| e.left != e.right)
                                    && joined.polys_apart(&polys, &fresh)
                            });
                            assert_eq!(
                                edges_ok, full_ok,
                                "増えた辺の決まりの答えが、新しい地図全体で調べた答え（お手本）と違う"
                            );
                            let same = super::uncounted(|| {
                                d.same_content(&StoredMap::from_dense(&joined, None))
                            });
                            assert!(
                                same,
                                "前のしまう形に窓の差分を当てた形が、窓の結果を合わせた地図のしまう形（お手本）と違う"
                            );
                            // お手本（設計 段2b の 4. の 2b-4）：触った輪郭の集まりが、新しい地図全体の輪郭のうち触った
                            // 向きのある辺を含むものと同じ。ほかの輪郭は、どれも前の地図にそのままある
                            if let Some(touched) = touched {
                                let ok = super::uncounted(|| {
                                    window_cycles_match(self, &joined, &touched)
                                });
                                assert!(
                                    ok,
                                    "前の地図＋差分でたどった触った輪郭が、新しい地図全体の輪郭（お手本）と違う"
                                );
                            }
                        }
                        joined
                    }))
    }

    pub(super) fn window_overlay(
        ix: MapIndex<'_>,
        input: MapInput<'_>,
        rules: &[MapRule],
        new_quads: &[QuadBez],
        new_cols: &[(Option<Color32>, Option<Color32>)],
    ) -> Option<(Vec<PlanarMap>, Vec<usize>)> {
        // 窓の範囲を、入れる辺と重ねるものの外接矩形から広げる幅（px）。広いほどやり直しが減り、窓は大きくなる
        const WINDOW_PAD: f64 = 2.0;
        let stored = ix.stored;
        let hull = |q: QuadBez| KRect::from_points(q.p0, q.p2).union_pt(q.p1);
        let quad_of = |e: usize| stored.edge_at(e).quad();
        // 範囲に外接矩形（`reach_box`）がかかる前の地図の辺（升目の索引で引く。番号の順）
        let region_edges = |r: KRect| -> Vec<usize> {
            let mut found: Vec<usize> = Vec::new();
            for t in index_tiles_over(r) {
                ix.index.edges_in(stored, t, &mut found);
            }
            found.sort_unstable();
            found.dedup();
            found.retain(|&e| rects_overlap(reach_box(quad_of(e)), r));
            found
        };
        let inside =
            |u: KRect, r: KRect| u.x0 >= r.x0 && u.y0 >= r.y0 && u.x1 <= r.x1 && u.y1 <= r.y1;
        let moved_list: Vec<usize> = match input {
            MapInput::Sweep(m) => m.iter().map(|&(e, _)| e).collect(),
            _ => Vec::new(),
        };
        // 重ねるもの（輪郭の変形なら前の曲線も）の範囲。`overlay_fed` は、これが無ければ重ねない
        let area = new_quads
            .iter()
            .copied()
            .chain(moved_list.iter().map(|&e| quad_of(e)))
            .map(hull)
            .reduce(|a, b| a.union(b))?;
        // 入れる辺（前の地図の番号）。輪郭の変形で動かした辺は、はじめから入れる
        let mut fed_init: std::collections::BTreeSet<usize> = moved_list.iter().copied().collect();
        // 重ねる地図（`MapInput::Map`）の辺のうち、はじめから入れるもの（重ねる地図の番号。入れなかった辺が切った区間と
        // 交わったときに足す）。重ねる地図の辺は、前の地図全体で行うときと同じく、前からある辺の後ろに並べる（`joined`）
        let piece: Option<&PlanarMap> = match input {
            MapInput::Map(p) => Some(p),
            _ => None,
        };
        let np = input.joined_edges();
        let mut fed_init_new: std::collections::BTreeSet<usize> = Default::default();
        // 窓（前の地図の辺の一部を並べた地図 `m`）に重ねる地図の辺を並べた地図と、同じ線の組の相手（`same_edges`。前の
        // 地図全体で行うときと同じ組：組になる前からある辺は、両端が重ねる地図の頂点の外形の中にあるので、外接矩形が
        // 重ねるものの範囲にかかり、みな窓にある）
        let with_piece = |m: PlanarMap| -> (PlanarMap, Vec<Option<(usize, bool)>>) {
            let Some(p) = piece else {
                return (m, Vec::new());
            };
            let (pairs, alias) = m.same_edges(p);
            let n = m.edges.len();
            let joined = m.joined(p, &alias);
            let mut partner = vec![None; joined.edges.len()];
            for &(e, f, same_dir) in &pairs {
                partner[e] = Some((n + f, same_dir));
                partner[n + f] = Some((e, same_dir));
            }
            (joined, partner)
        };
        // 重ねるときの種（重ねる地図は、辺として並べるので種は無い）
        let seeds: &[QuadBez] = if piece.is_some() { &[] } else { new_quads };
        let mut extra: Option<KRect> = None;
        loop {
            // 1. 入れる辺を、重ねるもののまわりの下窓で決める（設計 段2b の 7. の 2b-7b）。入れる辺の近くの辺（外接矩形を
            //    広げた所）は、みな下窓にある。入れる辺の外接矩形を広げた所が下窓の範囲から出たら、範囲を広げてやり直す
            // 下窓のはじめの範囲は、重ねるものと、それに外接矩形がかかる辺（入れる辺になりやすい）の外形まで。たいていは
            //  やり直さずに済む
            let mut r0 = fed_init
                .iter()
                .copied()
                .chain(region_edges(area))
                .map(|e| hull(quad_of(e)))
                .fold(area, |a, b| a.union(b))
                .inflate(WINDOW_PAD, WINDOW_PAD);
            let fed_dense = loop {
                let mut sub_edges = region_edges(r0);
                sub_edges.extend(fed_init.iter().copied());
                sub_edges.sort_unstable();
                sub_edges.dedup();
                let (sub, sub_known) = tile_window(stored, &sub_edges);
                let n0 = sub.edges.len();
                let (sub, sub_partner) = with_piece(sub);
                let mut moved0: Vec<bool> =
                    sub_edges.iter().map(|e| moved_list.contains(e)).collect();
                moved0.resize(n0 + np, false);
                let mut init0: Vec<bool> = sub_edges.iter().map(|e| fed_init.contains(e)).collect();
                init0.extend((0..np).map(|i| fed_init_new.contains(&i)));
                let f = sub.fed_edges(
                    &sub_known,
                    None,
                    seeds,
                    &moved0,
                    init0,
                    n0..n0 + np,
                    &sub_partner,
                )?;
                let fed_now: Vec<usize> = (0..n0).filter(|&e| f.fed[e]).collect();
                // 重ねる地図の辺は、みな調べる（入れるかを決める）ので、どれも近くの辺が下窓にあるかを見る
                let reach = fed_now
                    .iter()
                    .copied()
                    .chain(n0..n0 + np)
                    .map(|e| reach_box(sub.quad(e)).inflate(4.0 * GRID_REACH, 4.0 * GRID_REACH))
                    .fold(area, |a, b| a.union(b));
                if !inside(reach, r0) {
                    r0 = r0.union(reach.inflate(WINDOW_PAD, WINDOW_PAD));
                    continue;
                }
                // 線の番号を、窓によらない形にする（前からある辺は前の地図の番号、重ねる地図の辺はその番号）
                let spans: Vec<(SpanLine, Vec<Span>)> = f
                    .spans
                    .iter()
                    .map(|(l, v)| {
                        let l = match *l {
                            Line::Edge(e) if e < n0 => SpanLine::Old(sub_edges[e]),
                            Line::Edge(e) => SpanLine::New(e - n0),
                            Line::Seed(i) => SpanLine::Seed(i),
                        };
                        (l, v.clone())
                    })
                    .collect();
                let fed_new: Vec<bool> = (0..np).map(|i| f.fed[n0 + i]).collect();
                break (
                    fed_now
                        .into_iter()
                        .map(|e| sub_edges[e])
                        .collect::<Vec<usize>>(),
                    spans,
                    fed_new,
                );
            };
            let (fed_dense, spans_dense, fed_new) = fed_dense;
            // 2. 窓：入れる辺と重ねるものの外接矩形を広げた範囲に外接矩形がかかる辺と、入れる辺と、それらの端の頂点に
            //    つながる辺。辺は塊から引き、頂点は座標で見分ける
            let mut region = fed_dense
                .iter()
                .map(|&e| hull(quad_of(e)))
                .fold(area, |a, b| a.union(b));
            if let Some(x) = extra {
                region = region.union(x);
            }
            let region = region.inflate(WINDOW_PAD, WINDOW_PAD);
            let mut in_window: std::collections::BTreeSet<usize> =
                region_edges(region).into_iter().collect();
            in_window.extend(fed_dense.iter().copied());
            let mut ends: Vec<Point> = Vec::new();
            for &e in &in_window {
                let te = stored.edge_at(e);
                ends.push(te.a);
                ends.push(te.b);
            }
            ends.sort_by(|a, b| point_order(*a, *b));
            ends.dedup();
            for p in ends {
                let mut at: Vec<usize> = Vec::new();
                ix.ends.edges_in(stored, end_key(p), &mut at);
                for e in at {
                    let te = stored.edge_at(e);
                    if te.a == p || te.b == p {
                        in_window.insert(e);
                    }
                }
            }
            let wedges: Vec<usize> = in_window.into_iter().collect();
            let (wmap, wknown) = tile_window(stored, &wedges);
            let nw = wmap.edges.len();
            let (wmap, wpartner) = with_piece(wmap);
            let to_window: IntMap<usize, usize> =
                wedges.iter().enumerate().map(|(i, &e)| (e, i)).collect();
            let spans: IntMap<Line, Vec<Span>> = spans_dense
                .iter()
                .map(|(l, v)| {
                    let l = match *l {
                        SpanLine::Old(e) => Line::Edge(to_window[&e]),
                        SpanLine::New(i) => Line::Edge(nw + i),
                        SpanLine::Seed(i) => Line::Seed(i),
                    };
                    (l, v.clone())
                })
                .collect();
            let wmoved: Vec<(usize, QuadBez)> = match input {
                MapInput::Sweep(m) => m.iter().map(|&(e, q)| (to_window[&e], q)).collect(),
                _ => Vec::new(),
            };
            let winput = match input {
                MapInput::Sweep(_) => MapInput::Sweep(&wmoved),
                other => other,
            };
            let fed_set: std::collections::HashSet<usize> = fed_dense.iter().copied().collect();
            let mut wset = FedEdges {
                fed: wedges
                    .iter()
                    .map(|e| fed_set.contains(e))
                    .chain(fed_new.iter().copied())
                    .collect(),
                known: &wknown,
                spans,
                near: Some(NearEdges::new(&wmap, &wknown, None)),
                partner: wpartner,
                outside: Some(Outside {
                    index: ix,
                    from_window: wedges.clone(),
                }),
                used: None,
            };
            match wmap.overlay_with(winput, rules, new_quads, new_cols, &mut wset) {
                Ok(maps) => {
                    if let Some(u) = wset.used {
                        let u = KRect::new(
                            u.x0 as f64 / OVERLAY_GRID,
                            u.y0 as f64 / OVERLAY_GRID,
                            u.x1 as f64 / OVERLAY_GRID,
                            u.y1 as f64 / OVERLAY_GRID,
                        );
                        if !(u.x0 >= region.x0
                            && u.y0 >= region.y0
                            && u.x1 <= region.x1
                            && u.y1 <= region.y1)
                        {
                            extra = Some(extra.map_or(u, |x| x.union(u)));
                            continue;
                        }
                    }
                    return Some((maps, wedges));
                }
                Err(OverlayStop::Unfed(more)) => {
                    // 入れなかった辺が区間と交わった：その辺も入れて、1. からやり直す（前の地図全体で行うときと同じ）
                    fed_init.extend(fed_dense.iter().copied());
                    fed_init_new.extend((0..np).filter(|&i| fed_new[i]));
                    for e in more {
                        if e < nw {
                            fed_init.insert(wedges[e]);
                        } else {
                            fed_init_new.insert(e - nw);
                        }
                    }
                }
                Err(OverlayStop::Refused) => return None,
            }
        }
    }

    /// 窓で重ね合わせた結果 `result`（窓の頂点 `wnodes`・窓の辺 `to_window` から作った地図の結果）を、前の地図全体の結果に
    /// する：外が前のままの決まり（`copy`）なら、窓に無い前の地図の辺をそのまま足す。窓の結果の頂点は、前の地図の窓の
    /// 頂点と同じ座標ならその頂点にする（窓の外の辺が、端を共有している）
    fn with_window_result(
        &self,
        result: PlanarMap,
        wnodes: &[usize],
        to_window: &IntMap<usize, usize>,
        copy: bool,
    ) -> PlanarMap {
        if !copy {
            return result;
        }
        let at: IntMap<[u64; 2], usize> = wnodes
            .iter()
            .map(|&v| ([self.nodes[v].x.to_bits(), self.nodes[v].y.to_bits()], v))
            .collect();
        let mut out = PlanarMap {
            nodes: self.nodes.clone(),
            edges: (0..self.edges.len())
                .filter(|e| !to_window.contains_key(e))
                .map(|e| self.edges[e])
                .collect(),
        };
        let node: Vec<usize> = result
            .nodes
            .iter()
            .map(|p| match at.get(&[p.x.to_bits(), p.y.to_bits()]) {
                Some(&v) => v,
                None => {
                    out.nodes.push(*p);
                    out.nodes.len() - 1
                }
            })
            .collect();
        out.edges.extend(result.edges.iter().map(|ed| MapEdge {
            a: node[ed.a],
            b: node[ed.b],
            ..*ed
        }));
        out.compact_nodes();
        out
    }

    /// 2つの地図の辺と頂点を1つに並べたもの（`other` の頂点の番号は後ろへずらす。ただし `alias` にある頂点は、その前から
    /// ある頂点にする：同じ線の組の端（`same_edges`））。正しい地図とは限らない（重ね合わせで入れる辺を決めるときと、片を
    /// 置く前の形を表すときの入れ物）
    pub(super) fn joined(&self, other: &PlanarMap, alias: &IntMap<usize, usize>) -> PlanarMap {
        let n = self.nodes.len();
        let at = |v: usize| alias.get(&v).copied().unwrap_or(v + n);
        let nodes: Vec<Point> = self.nodes.iter().chain(&other.nodes).copied().collect();
        let edges = self
            .edges
            .iter()
            .copied()
            .chain(other.edges.iter().map(|e| MapEdge {
                a: at(e.a),
                b: at(e.b),
                ..*e
            }))
            .collect();
        PlanarMap { nodes, edges }
    }

    /// 同じ線の組：前からある辺 e と、重ねる地図 `other` の辺 f で、形がビットまで同じもの（`edge_key`。向きは問わない）。
    /// （e, f, 同じ向きか）の並びと、組の端の `other` の頂点 → 前からある頂点。
    /// 前の地図も重ねる地図も正しい地図なので、e（＝f）は、どちらの地図のどの辺とも、共有する頂点のほかで交わらない
    /// （前からある辺とは e が、重ねる地図の辺とは f が、同じ地図の中にあるので）。だから組は切らずに1本で残し、両側の
    /// 色だけを決まりで決めればよい（`overlay_fed`）
    pub(super) fn same_edges(&self, other: &PlanarMap) -> SameEdges {
        let mut pairs = Vec::new();
        let mut alias: IntMap<usize, usize> = IntMap::default();
        let Some(area) = other
            .nodes
            .iter()
            .map(|&p| KRect::from_points(p, p))
            .reduce(|a, b| a.union(b))
        else {
            return (pairs, alias);
        };
        let inside =
            |p: Point| p.x >= area.x0 && p.x <= area.x1 && p.y >= area.y0 && p.y <= area.y1;
        let mut by_key: std::collections::HashMap<[u64; 6], usize> = Default::default();
        for f in 0..other.edges.len() {
            by_key.insert(super::edge_key(other, f), f);
        }
        for (e, ed) in self.edges.iter().enumerate() {
            if !inside(self.nodes[ed.a]) || !inside(self.nodes[ed.b]) {
                continue;
            }
            if let Some(&f) = by_key.get(&super::edge_key(self, e)) {
                let fd = other.edges[f];
                let same_dir = other.nodes[fd.a] == self.nodes[ed.a];
                let (fa, fb) = if same_dir { (fd.a, fd.b) } else { (fd.b, fd.a) };
                alias.insert(fa, ed.a);
                alias.insert(fb, ed.b);
                pairs.push((e, f, same_dir));
            }
        }
        (pairs, alias)
    }

    /// i_overlay に入れる辺（設計 2-1）。`fed` の辺と重ねる線 `seeds` の折れ線の近くを通る辺を足し、
    /// 足した辺の折れ線の近くを通る辺を足し…と、増えなくなるまで続ける。「近く」は格子の目の対角線の半分
    /// （`GRID_REACH`）以内：i_overlay は交点を格子の点に丸めるので、切った区間は折れ線からその距離まで動く（端へ寄せて
    /// それより遠くへ動いた所は、切ったあとに確かめて切り直す）。
    /// ただし、入れた辺と頂点を共有する辺は、その頂点でいつも触れているので、共有する頂点のほかで交わるかで見る。
    /// 入れた辺の端の頂点は格子に置き直すので、その頂点につながる入れない辺は、置き直した形で調べ直す
    /// （ほかの入れない辺と交わらないかも見る）。`check` の辺（重ねる地図の辺）も、はじめに同じく調べる：入れない辺と
    /// 触れれば入れる。`moved` は輪郭の変形で動かした辺（前の形は同じ線の比べ先にしない）。`known`：前から `known.len()`
    /// 本の辺の計算に使う形（外接矩形とマスを使い回す：`EdgeGeom::reach`）。`None`：格子に置けない座標がある
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fed_edges<'a>(
        &'a self,
        known: &'a [Poly],
        index: Option<MapIndex<'a>>,
        seeds: &[QuadBez],
        moved: &[bool],
        mut fed: Vec<bool>,
        check: std::ops::Range<usize>,
        partner: &[Option<(usize, bool)>],
    ) -> Option<FedEdges<'a>> {
        #[cfg(test)]
        super::FED_RUNS.with(|n| n.set(n.get() + 1));
        let mut snapped = vec![false; self.nodes.len()];
        // 近くの辺と、頂点ごとの辺を探す道具（前からある辺は、計算に使う写しの升目の索引を引く）
        let mut near = NearEdges::new(self, known, index);
        let mut cells: Vec<(i64, i64)> = Vec::new();
        let mut adj_buf: Vec<usize> = Vec::new();
        // 頂点 `v` を格子に置き直すと動くか
        let moves = |v: usize| grid_round(self.nodes[v]) != self.nodes[v];
        // 入れない辺 `e` の判定の形（`out` に入れる）。端の頂点が格子に置き直されていれば、置き直した形で
        let poly_of = |e: usize, snapped: &[bool], out: &mut Vec<IPt>| {
            let ed = self.edges[e];
            let pos = |v: usize| {
                if snapped[v] {
                    grid_round(self.nodes[v])
                } else {
                    self.nodes[v]
                }
            };
            known_or_made(known, self, e, (pos(ed.a), pos(ed.b)), out);
        };
        // 判定の形の入れ物（使い回す）
        let (mut pe, mut pf): (Vec<IPt>, Vec<IPt>) = (Vec::new(), Vec::new());
        // 折れ線の区間：（始点, 終点, 始点の頂点, 終点の頂点）。頂点は、前からある辺の折れ線の両端だけ
        type GridSeg = (IPt, IPt, Option<usize>, Option<usize>);
        // 折れ線：（外接矩形, 区間の範囲）
        type GridLine = (KRect, std::ops::Range<usize>);
        // 折れ線の区間がかかるマス（続く区間はたいてい同じマスにかかるので、続けて同じマスは1つにする。探す順は
        // 変わらない）
        let line_cells = |segs: &[GridSeg], cells: &mut Vec<(i64, i64)>| {
            cells.clear();
            cells.extend(segs.iter().flat_map(|s| seg_cells((s.0, s.1), 0)));
            cells.dedup();
        };
        let seg_rect =
            |a: IPt, b: IPt| KRect::from_points(from_grid(a.0, a.1), from_grid(b.0, b.1));
        let add_polyline = |line: &[IPt],
                            ends: (Option<usize>, Option<usize>),
                            segs: &mut Vec<GridSeg>,
                            seg_grid: &mut CellGrid,
                            lines: &mut Vec<GridLine>| {
            let start = segs.len();
            let mut bbox: Option<KRect> = None;
            for k in 0..line.len().saturating_sub(1) {
                let (a, b) = (line[k], line[k + 1]);
                let r = seg_rect(a, b);
                bbox = Some(bbox.map_or(r, |x| x.union(r)));
                seg_grid.insert(segs.len(), seg_cells((a, b), 0));
                let (ta, tb) = seg_tags(line, k, usize::MAX, usize::MAX);
                segs.push((a, b, ta.and(ends.0), tb.and(ends.1)));
            }
            if let Some(r) = bbox {
                lines.push((r, start..segs.len()));
            }
        };
        // 入れない辺 `e`（判定の形 `pe`）が、区間 `s` の近くを通るか：共有する頂点から出る区間どうしは、そこのほかで
        // 触れるか。ほかは、`GRID_REACH` 以内に来るか
        let near_seg = |near: &NearEdges, e: usize, pe: &[IPt], s: &GridSeg| -> bool {
            if !rects_overlap(near.bbox(e), seg_rect(s.0, s.1)) {
                return false;
            }
            let ed = self.edges[e];
            (0..pe.len().saturating_sub(1)).any(|k| {
                let (a, b) = (pe[k], pe[k + 1]);
                let tags = seg_tags(pe, k, ed.a, ed.b);
                let shares = [(tags.0, a), (tags.1, b)]
                    .into_iter()
                    .any(|(t, p)| t.is_some() && [(s.2, s.0), (s.3, s.1)].contains(&(t, p)));
                if shares {
                    seg_pair_clash((a, b), tags, (s.0, s.1), (s.2, s.3), None)
                } else {
                    segs_within_reach(a, b, s.0, s.1)
                }
            })
        };
        let mut segs: Vec<GridSeg> = Vec::new();
        let mut seg_grid = CellGrid::new();
        let mut found = Vec::new();
        let mut lines: Vec<GridLine> = Vec::new();
        // 重ねる地図の辺は、はじめに入れない辺と触れないかを調べる。同じ線の組の辺は調べない：相手のほかの前からある辺と
        // 触れれば、前の地図で相手がその辺と触れている
        let mut edge_queue: Vec<usize> = check
            .clone()
            .filter(|&c| partner.get(c).copied().flatten().is_none())
            .collect();
        let mut to_feed: Vec<usize> = (0..self.edges.len()).filter(|&e| fed[e]).collect();
        // 同じ線として扱う所（`Span`）：重ねる線（形の線・輪郭の変形の線と、重ねる地図の辺）と、前からある辺（輪郭の変形で
        // 動かした辺は除く）が同じ線になっている所。その2本は入れ、その所は2本とも同じ点の折れ線にする。1本の線の上で
        // 重なる所どうしは重ならないようにする（先に見つけたもの）
        let flat_pad = (FLAT_TOL * OVERLAY_GRID).ceil() as i64;
        let mut spans: IntMap<Line, Vec<Span>> = IntMap::default();
        let new_lines: Vec<(Line, QuadBez)> = seeds
            .iter()
            .enumerate()
            .map(|(i, q)| (Line::Seed(i), *q))
            .chain(check.clone().map(|c| (Line::Edge(c), self.quad(c))))
            .collect();
        for &(line, q) in &new_lines {
            // 格子に置けない座標（頂点と制御点：`fits_grid` と同じ）は、ここで断る
            on_grid([q.p0, q.p1, q.p2].into_iter()).then_some(())?;
            // 同じ線の組（`same_edges`）は入れない（あとで片方を入れるときに、重なる所を記す）
            if matches!(line, Line::Edge(c) if partner.get(c).copied().flatten().is_some()) {
                continue;
            }
            near.query(quad_cells(q, flat_pad), &mut found);
            let mut pairs: Vec<(usize, OverlapPts)> = found
                .iter()
                .copied()
                .filter(|&e| e < check.start && !moved[e])
                .filter_map(|e| Some((e, self.overlap(&q, e)?)))
                .collect();
            pairs.sort_by(|a, b| a.1[0].1.total_cmp(&b.1[0].1));
            for (e, pts) in pairs {
                if add_span(&mut spans, line, e, &pts) {
                    to_feed.push(e);
                    if let Line::Edge(c) = line {
                        to_feed.push(c);
                    }
                }
            }
        }
        let mut fed_set = FedEdges {
            fed: Vec::new(),
            known,
            spans,
            near: None,
            partner: Vec::new(),
            outside: None,
            used: None,
        };
        let mut pbuf = PointsBuf::default();
        let mut gline: Vec<IPt> = Vec::new();
        for (i, q) in seeds.iter().enumerate() {
            fed_set.grid_points(Line::Seed(i), q, &mut pbuf, &mut gline);
            add_polyline(&gline, (None, None), &mut segs, &mut seg_grid, &mut lines);
        }
        for e in &to_feed {
            fed[*e] = false;
        }
        loop {
            // 入れる辺：端の頂点を格子に置き直し、その頂点につながる入れない辺を調べ直す。辺の折れ線を足す
            while let Some(e) = to_feed.pop() {
                if fed[e] {
                    continue;
                }
                fed[e] = true;
                // 同じ線の組は、片方を入れたら、もう片方も入れる。2本が同じ点の折れ線になるよう、重なる所を記す
                if let Some((f, _)) = partner.get(e).copied().flatten() {
                    if !fed[f] {
                        let (c, o) = if e >= check.start { (e, f) } else { (f, e) };
                        if let Some(pts) = self.overlap(&self.quad(c), o) {
                            add_span(&mut fed_set.spans, Line::Edge(c), o, &pts);
                        }
                        to_feed.push(f);
                    }
                }
                let ed = self.edges[e];
                for v in [ed.a, ed.b] {
                    if !snapped[v] {
                        snapped[v] = true;
                        // 置き直して頂点が動いたときだけ、つながる入れない辺の形が変わる
                        if moves(v) {
                            near.edges_at(v, &mut adj_buf);
                            edge_queue.extend(adj_buf.iter().copied().filter(|&f| !fed[f]));
                        }
                    }
                }
                let q = self.quad(e);
                // 格子に置けない座標（頂点と制御点：`fits_grid` と同じ）は、ここで断る
                on_grid([q.p0, q.p1, q.p2].into_iter()).then_some(())?;
                fed_set.grid_points(Line::Edge(e), &q, &mut pbuf, &mut gline);
                add_polyline(
                    &gline,
                    (Some(ed.a), Some(ed.b)),
                    &mut segs,
                    &mut seg_grid,
                    &mut lines,
                );
            }
            if let Some((r, range)) = lines.pop() {
                // 折れ線の区間のマスにかかる辺だけを、折れ線の区間のうち辺の外接矩形にかかるものと比べる
                line_cells(&segs[range.clone()], &mut cells);
                near.query(cells.iter().copied(), &mut found);
                for &e in &found {
                    if fed[e] || !rects_overlap(near.bbox(e), r) {
                        continue;
                    }
                    poly_of(e, &snapped, &mut pe);
                    if segs[range.clone()]
                        .iter()
                        .any(|s| near_seg(&near, e, &pe, s))
                    {
                        to_feed.push(e);
                    }
                }
            } else if let Some(e) = edge_queue.pop() {
                if fed[e] {
                    continue;
                }
                // 重ねる地図の辺どうしは、置き直して動いた端が無ければ触れない（重ねる地図は、その座標で正しい地図）
                let as_given = |f: usize| {
                    let fd = self.edges[f];
                    let moved = |v: usize| snapped[v] && moves(v);
                    check.contains(&f) && !moved(fd.a) && !moved(fd.b)
                };
                let ed = self.edges[e];
                // `e` の判定の形は、要るときに1度だけ作る
                let mut pe_made = false;
                seg_grid.query(near.cells(e).iter().copied(), &mut found);
                let mut hit = false;
                for &i in &found {
                    if !pe_made {
                        poly_of(e, &snapped, &mut pe);
                        pe_made = true;
                    }
                    if near_seg(&near, e, &pe, &segs[i]) {
                        hit = true;
                        break;
                    }
                }
                if !hit {
                    // 端を置き直した形が、ほかの入れない辺と触れないか（共有する頂点は除く）
                    let own_cells = near.cells(e).to_vec();
                    near.query(own_cells, &mut found);
                    let given = as_given(e);
                    let mate = partner.get(e).copied().flatten().map(|(m, _)| m);
                    for &f in &found {
                        if f == e || fed[f] || (given && as_given(f)) || Some(f) == mate {
                            continue;
                        }
                        let fd = self.edges[f];
                        poly_of(f, &snapped, &mut pf);
                        if !pe_made {
                            poly_of(e, &snapped, &mut pe);
                            pe_made = true;
                        }
                        if polys_clash(&pe, (ed.a, ed.b), &pf, (fd.a, fd.b)) {
                            hit = true;
                            break;
                        }
                    }
                }
                if hit {
                    to_feed.push(e);
                }
            } else {
                return Some(FedEdges {
                    fed,
                    near: Some(near),
                    partner: partner.to_vec(),
                    ..fed_set
                });
            }
        }
    }

    /// `overlay_rules` の1回分（入れる辺 `fed` を決めたあと）。決まりごとの地図（`rules` の順）。入れなかった辺が切った
    /// 区間と交わったら `Unfed`（その辺）。`self` は前の地図（重ねる地図なら、その辺を後ろに並べたもの：`joined`）
    pub(super) fn overlay_with(
        &self,
        input: MapInput<'_>,
        rules: &[MapRule],
        new_quads: &[QuadBez],
        new_cols: &[(Option<Color32>, Option<Color32>)],
        fed_set: &mut FedEdges<'_>,
    ) -> Result<Vec<PlanarMap>, OverlayStop> {
        let fed = std::mem::take(&mut fed_set.fed);
        let out = self.overlay_fed(input, rules, new_quads, new_cols, &fed, fed_set);
        fed_set.fed = fed;
        out
    }

    pub(super) fn overlay_fed(
        &self,
        input: MapInput<'_>,
        rules: &[MapRule],
        new_quads: &[QuadBez],
        new_cols: &[(Option<Color32>, Option<Color32>)],
        fed: &[bool],
        fed_set: &mut FedEdges<'_>,
    ) -> Result<Vec<PlanarMap>, OverlayStop> {
        use i_overlay9::core::edge_overlay::{EdgeOverlay, InputEdge};
        use i_overlay9::core::fill_rule::FillRule as IFill;
        use i_overlay9::core::overlay::ShapeType;
        use i_overlay9::vector::edge::{CLIP_LEFT, CLIP_RIGHT};
        // 外が前のままの決まりのときだけ、重ねるものから離れた辺をそのまま残す
        let copies_untouched = |rule: MapRule| rule != MapRule::KeepInside;
        let copy_any = rules.iter().any(|&r| copies_untouched(r));
        let moved = input.moved_flags(self.edges.len());
        // 辺 0..n_old が前からある辺、その後ろが重ねる地図の辺（`joined`）
        let n_old = self.edges.len() - input.joined_edges();
        // 重ねるもの（輪郭の変形なら前の曲線も）の外接矩形。これにかからない入れない辺は、重ねるものの外にある
        let area = new_quads
            .iter()
            .copied()
            .chain(
                (0..self.edges.len())
                    .filter(|&e| moved[e])
                    .map(|e| self.quad(e)),
            )
            .map(|q| q.bounding_box())
            .reduce(|a, b| a.union(b))
            .ok_or(OverlayStop::Refused)?;

        // 0. 出どころの曲線：入れる辺（前からある辺と重ねる地図の辺）と、重ねる形・線。入れない辺のうち、重ねる地図の辺と、
        //    重ねるものの外接矩形にかかる前からある辺は「中の辺」（色を決め直す）
        let mut srcs: Vec<(QuadBez, OverlaySrc)> = Vec::new();
        let mut interior = vec![false; self.edges.len()];
        // 同じ線の組（`same_edges`）のうち入れなかったもの：前からある辺を1本で残し、重ねる地図の辺は出さない（`dropped`）
        let partner = fed_set.partner.clone();
        let mate = |e: usize| partner.get(e).copied().flatten().filter(|_| !fed[e]);
        let dropped = |e: usize| e >= n_old && mate(e).is_some();
        for e in 0..self.edges.len() {
            if fed[e] {
                let src = if e < n_old {
                    OverlaySrc::Old(e)
                } else {
                    OverlaySrc::New(e - n_old)
                };
                srcs.push((self.quad(e), src));
            } else {
                interior[e] = e >= n_old || rects_overlap(fed_set.near().bbox(e), area);
            }
        }
        if !matches!(input, MapInput::Map(_)) {
            for (i, q) in new_quads.iter().enumerate() {
                srcs.push((*q, OverlaySrc::New(i)));
            }
        }

        // 1. 折れ線にして格子に置き、i_overlay で切る。格子は原点から（何回重ねても、格子の上の点は動かない）。
        //    切れ目の頂点は、どちらの曲線からも `FLAT_TOL + SPLIT_REACH` 以内にある（折れ線は曲線から `FLAT_TOL` 以内で、
        //    i_overlay は切れ目の点を折れ線どうしの交点から `SPLIT_REACH` 以内に置く）。切って戻した2本は、その許容で
        //    1本にまとまる（`merge_degree2`）
        let lines: Vec<Line> = srcs
            .iter()
            .map(|(_, kind)| match *kind {
                OverlaySrc::Old(e) => Line::Edge(e),
                OverlaySrc::New(i) if matches!(input, MapInput::Map(_)) => Line::Edge(n_old + i),
                OverlaySrc::New(i) => Line::Seed(i),
            })
            .collect();
        // 出どころの折れ線を、i_overlay で切る（1回だけ）
        #[cfg(test)]
        super::CUTS.with(|n| n.set(n.get() + 1));
        let mut ov: EdgeOverlay<i32, ProvId> = EdgeOverlay::new(srcs.len() * 8);
        let mut buf = PointsBuf::default();
        for (si, (q, kind)) in srcs.iter().enumerate() {
            let st = if matches!(kind, OverlaySrc::Old(_)) {
                ShapeType::Subject
            } else {
                ShapeType::Clip
            };
            // 同じ線として扱う所は、2本とも同じ点の折れ線にする（`Span`）
            let pts = fed_set.points(lines[si], q, &mut buf);
            let mut prev = (to_grid(pts[0].0).ok_or(OverlayStop::Refused)?, pts[0].1);
            for &(p, t) in &pts[1..] {
                let ip = to_grid(p).ok_or(OverlayStop::Refused)?;
                if ip == prev.0 {
                    continue;
                }
                // 点は t の順なので、区間（前の点→この点）は出どころの t の増える向き
                let id = ov.data_store_mut().push([Prov {
                    src: si as u32,
                    t0: prev.1,
                    t1: t,
                    fwd: true,
                }]);
                ov.add_edge(
                    InputEdge {
                        a: prev.0,
                        b: ip,
                        data: id,
                    },
                    st,
                );
                prev = (ip, t);
            }
        }
        // 切ったあとのすべての区間（塗りの規則でふるい落とさない。vendor/i_overlay/PATCH.md）
        let out = ov.build_all_vectors(IFill::EvenOdd);
        let store = ov.into_data_store();

        // 2. 頂点：入れた辺の端の頂点は、その番号のまま格子の上に置く。格子の同じ点にある頂点は1つにする（前の地図の頂点を
        //    先に見るので、重ねる地図の頂点が前の地図の頂点に重なれば、前の地図の頂点になる）。新しい頂点は後ろに足す
        let mut nodes: Vec<Point> = self.nodes.clone();
        let mut node_of: IntMap<(i32, i32), usize> = IntMap::default();
        // 1つにした頂点 → 結果の地図での番号
        let mut merged: IntMap<usize, usize> = IntMap::default();
        for (e, ed) in self.edges.iter().enumerate() {
            if fed[e] {
                for v in [ed.a, ed.b] {
                    let ip = to_grid(self.nodes[v]).ok_or(OverlayStop::Refused)?;
                    nodes[v] = from_grid(ip.x as i64, ip.y as i64);
                    let r = *node_of.entry((ip.x, ip.y)).or_insert(v);
                    if r != v {
                        merged.insert(v, r);
                    }
                }
            }
        }
        let node_at = |v: usize| merged.get(&v).copied().unwrap_or(v);
        struct Piece {
            a: usize,
            b: usize,
            ia: (i64, i64),
            ib: (i64, i64),
            fill: u8,
            /// 出どころの一覧（`piece_provs` の範囲）
            prov: std::ops::Range<usize>,
        }
        let mut pieces: Vec<Piece> = Vec::with_capacity(out.len());
        // 区間ごとの出どころの一覧を、みな1つの並びに入れる（区間ごとに並びを持たない）
        let mut piece_provs: Vec<Prov> = Vec::with_capacity(out.len());
        let mut prov: Vec<Prov> = Vec::new();
        for v in &out {
            let (ka, kb) = ((v.a.x, v.a.y), (v.b.x, v.b.y));
            // 同じ曲線が自分の上で折り返した所（同じ出どころで向きが逆の組）は打ち消す。何も残らなければ区間ごと捨てる
            prov.clear();
            prov.extend_from_slice(store.get(v.data));
            let mut k = 0;
            while k < prov.len() {
                let fwd = prov[k].fwd;
                let twin =
                    (k + 1..prov.len()).find(|&j| prov[j].src == prov[k].src && prov[j].fwd != fwd);
                if let Some(j) = twin {
                    prov.remove(j);
                    prov.remove(k);
                } else {
                    k += 1;
                }
            }
            if prov.is_empty() {
                continue;
            }
            let mut id = |k: (i32, i32), nodes: &mut Vec<Point>| -> usize {
                *node_of.entry(k).or_insert_with(|| {
                    nodes.push(from_grid(k.0 as i64, k.1 as i64));
                    nodes.len() - 1
                })
            };
            let a = id(ka, &mut nodes);
            let b = id(kb, &mut nodes);
            let start = piece_provs.len();
            piece_provs.extend_from_slice(&prov);
            pieces.push(Piece {
                a,
                b,
                ia: (ka.0 as i64, ka.1 as i64),
                ib: (kb.0 as i64, kb.1 as i64),
                fill: v.fill,
                prov: start..piece_provs.len(),
            });
        }
        // 入れなかった辺のうち結果に残るもの（中の辺と、外が前のままの決まりがあるときの離れた辺）が、切った区間と交わって
        // いないか
        // （`fed_edges` は入れた辺の折れ線で調べたが、i_overlay は交点を格子に丸めるので、区間は折れ線から最大で半目ずれる）
        {
            let mut found = Vec::new();
            let mut unfed: Vec<usize> = Vec::new();
            let mut flagged = vec![false; self.edges.len()];
            let mut pe: Vec<IPt> = Vec::new();
            for p in &pieces {
                let (a, b) = (nodes[p.a], nodes[p.b]);
                let r = KRect::from_points(a, b);
                fed_set
                    .near_mut()
                    .query(seg_cells((p.ia, p.ib), 0), &mut found);
                for &e in &found {
                    if fed[e] || flagged[e] || !(interior[e] || copy_any) {
                        continue;
                    }
                    if !rects_overlap(fed_set.near().bbox(e), r) {
                        continue;
                    }
                    let ed = self.edges[e];
                    known_or_made(fed_set.known, self, e, (nodes[ed.a], nodes[ed.b]), &mut pe);
                    let ends = (node_at(ed.a), node_at(ed.b));
                    if polys_clash(&[p.ia, p.ib], (p.a, p.b), &pe, ends) {
                        flagged[e] = true;
                        unfed.push(e);
                    }
                }
            }
            if !unfed.is_empty() {
                return Err(OverlayStop::Unfed(unfed));
            }
        }
        // 3. 頂点ごとの、出ていく区間：（区間, 区間の向きに出ていくか）。区間の頂点でない頂点は空
        let around: Grouped<(usize, bool)> = Grouped::new(
            nodes.len(),
            pieces
                .iter()
                .enumerate()
                .flat_map(|(k, p)| [(p.a, (k, true)), (p.b, (k, false))]),
        );
        let dir_of = |&(k, fwd): &(usize, bool)| -> (i64, i64) {
            let p = &pieces[k];
            if fwd {
                (p.ib.0 - p.ia.0, p.ib.1 - p.ia.1)
            } else {
                (p.ia.0 - p.ib.0, p.ia.1 - p.ib.1)
            }
        };
        // 色をたどるときは、その地図の入れなかった辺のうち、区間の頂点につながるものも並べる（その辺で色が変わるので、たどる
        // のはそこで止める）。入れなかった辺の向きは、その判定の形の、頂点から出る最初の区間の向き（区間と同じく整数）
        let at_node: Grouped<usize> =
            Grouped::new(nodes.len(), merged.iter().map(|(&v, &r)| (r, v)));
        // 区間の頂点ごとの、そこで止める入れなかった辺：（辺, 頂点がその辺の始点か, 向き）
        let mut stop_list: Vec<(usize, (usize, bool, IPt))> = Vec::new();
        let mut pe: Vec<IPt> = Vec::new();
        let mut adj_buf: Vec<usize> = Vec::new();
        for v in (0..around.len()).filter(|&v| !around.get(v).is_empty()) {
            // 区間の頂点 `v` にある、元の頂点（`v` そのものと、`v` に1つにした頂点）
            let own = (v < self.nodes.len()).then_some(v);
            for u in own.into_iter().chain(at_node.get(v).iter().copied()) {
                fed_set.near().edges_at(u, &mut adj_buf);
                for &e in &adj_buf {
                    if fed[e] {
                        continue;
                    }
                    let ed = self.edges[e];
                    known_or_made(fed_set.known, self, e, (nodes[ed.a], nodes[ed.b]), &mut pe);
                    for (end, at_start) in [(ed.a, true), (ed.b, false)] {
                        if end != u {
                            continue;
                        }
                        // 頂点から出る最初の区間（判定の形は少なくとも1点）
                        let n = pe.len();
                        let (p0, p1) = if at_start {
                            (pe[0], pe[1.min(n - 1)])
                        } else {
                            (pe[n - 1], pe[n.saturating_sub(2)])
                        };
                        stop_list.push((v, (e, at_start, (p1.0 - p0.0, p1.1 - p0.1))));
                    }
                }
            }
        }
        let stops: Grouped<(usize, bool, IPt)> = Grouped::new(nodes.len(), stop_list.into_iter());
        // 向きのある区間の左の面を、次にたどる先（角度の順で、来た向きの逆のすぐ前）。番号は 2 × 区間 + 向き。
        // `old`：前の地図の色をたどるか（重ねる地図の色なら、重ねる地図の入れなかった辺で止める）
        let next_turns = |old: bool| {
            let mut next = vec![Turn::Piece(0, false); 2 * pieces.len()];
            let mut turns: Vec<(Turn, IPt)> = Vec::new();
            for v in 0..around.len() {
                turns.clear();
                turns.extend(
                    around
                        .get(v)
                        .iter()
                        .map(|&(k, fwd)| (Turn::Piece(k, fwd), dir_of(&(k, fwd)))),
                );
                turns.extend(
                    stops
                        .get(v)
                        .iter()
                        .filter(|s| (s.0 < n_old) == old)
                        .map(|&(_, _, d)| (Turn::Stop, d)),
                );
                turns.sort_by(|x, y| cmp_dir(wide(x.1), wide(y.1)));
                for (i, &(t, _)) in turns.iter().enumerate() {
                    if let Turn::Piece(k, fwd) = t {
                        next[2 * k + (!fwd) as usize] =
                            turns[(i + turns.len() - 1) % turns.len()].0;
                    }
                }
            }
            next
        };
        let next_old_of = next_turns(true);
        // 重ねる地図の色は、重ねる地図のときだけたどる
        let next_new_of = match input {
            MapInput::Map(_) => next_turns(false),
            _ => Vec::new(),
        };
        let next_old = |h: (usize, bool)| next_old_of[2 * h.0 + h.1 as usize];
        let next_new = |h: (usize, bool)| next_new_of[2 * h.0 + h.1 as usize];
        // 4. 区間の左右の、その地図の色：面ごとの色ごとの巻き数を、区間をまたぐたびに、その区間の出どころの辺の左右の色で
        //    足し引きして、となりの面へ伝える（`propagate`。設計：claude/重ね合わせの面の色と番号を伝えて決める設計_N-1003_N-2001_c467cbf.md）。
        //    足し引きはまたぐ順によらないので、i_overlay が交点を丸めて区間がほかの辺の向こうへ移っても、面の色は食い違わない
        let dbl = |p: IPt| (2 * p.0, 2 * p.1);
        let piece_ends: Vec<(IPt, IPt)> = pieces.iter().map(|p| (p.ia, p.ib)).collect();
        // 辺 `e`（前からある辺か、重ねる地図の辺）の左右の印
        let edge_labels = |e: usize| -> (Label, Label) {
            let ed = self.edges[e];
            ((ed.left, ed.left_src), (ed.right, ed.right_src))
        };
        // 区間の出どころごとの（`piece_provs` と同じ並び）、その地図（`old`：前の地図か）の出どころの辺の左右の印（区間の向きに
        // そろえる。その地図の辺でない出どころは `None`）。区間 `k` の分は `pieces[k].prov` の範囲
        let deltas = |old: bool| -> Vec<Option<(Label, Label)>> {
            piece_provs
                .iter()
                .map(|pv| {
                    let lr = match srcs[pv.src as usize].1 {
                        OverlaySrc::Old(e) if old => edge_labels(e),
                        OverlaySrc::New(i) if !old => edge_labels(n_old + i),
                        _ => return None,
                    };
                    Some(if pv.fwd { lr } else { (lr.1, lr.0) })
                })
                .collect()
        };
        let old_deltas = deltas(true);
        let new_deltas = match input {
            MapInput::Map(_) => deltas(false),
            _ => Vec::new(),
        };
        // その地図の入れなかった辺
        let unfed_of = |old: bool| {
            let range = if old {
                0..n_old
            } else {
                n_old..self.edges.len()
            };
            range.filter(|&e| !fed[e])
        };
        // その地図の入れなかった辺（端を置き直した曲線と、その格子の点の外接矩形）を、高さの帯に分けた索引。点のある所の色を
        // はじめて調べるときに作る
        let unfed_rows = |old: bool| {
            let items: Vec<(usize, QuadBez, GridRect)> = unfed_of(old)
                .map(|e| {
                    let ed = self.edges[e];
                    let q = QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]);
                    (e, q, reach_of(&q))
                })
                .collect();
            let bands = YBands::new(items.iter().map(|it| (2 * it.2.y0, 2 * it.2.y1)));
            (items, bands)
        };
        let rows = [std::cell::OnceCell::new(), std::cell::OnceCell::new()];
        // 窓で行うときの、窓の前からある辺（入れた辺も）の元の形の外接矩形を、高さの帯に分けた索引（`winding_at`）
        let orig_rows: std::cell::OnceCell<OrigRows> = std::cell::OnceCell::new();
        // 区間と、その出どころの辺の左右の色（足し引きの分）。巻き数は色だけで数える（図形の番号は、面の輪郭の辺ごとに
        // 違うことがある（2つの面がまとまった面など）ので、足し引きには入れない）
        // （重ねる地図でないときの重ねる地図の分 `new_deltas` は空で、区間は無い）
        let segs_of = |d: &[Option<(Label, Label)>]| {
            LabeledSegs::new(pieces.iter().flat_map(|p| {
                d.get(p.prov.clone())
                    .unwrap_or(&[])
                    .iter()
                    .flatten()
                    .map(move |&(l, r)| (p.ia, p.ib, l.0, r.0))
            }))
        };
        let old_segs = segs_of(&old_deltas);
        let new_segs = segs_of(&new_deltas);
        // その地図の、点 `p2`（2倍の座標）での色ごとの巻き数：区間の出どころの辺の色と、半直線の高さにかかるその地図の
        // 入れなかった辺の判定の形で、整数で数える（点の状態を数えるのはこれ1つ：面の種と、中の辺のもう片方の地図の色）
        // 判定の形の入れ物（使い回す）
        let pe_buf: std::cell::RefCell<Vec<IPt>> = Default::default();
        let winding_at = |segs: &LabeledSegs<Option<Color32>>, old: bool, p2: IPt| {
            let mut w: Vec<(Option<Color32>, i32)> = Vec::new();
            segs.winding_into(p2, &mut w);
            let mut pe = pe_buf.borrow_mut();
            // 窓で行うときは、前の地図の辺のうち半直線が通る升目にかかるもの（窓の外の辺も）を、前の地図の辺の番号の順に
            // 数える（窓の辺は窓の中と同じ形で。窓の外の辺は端を置き直さないので、前の地図の形のまま）。窓の辺の番号の順は
            // 前の地図の番号の順と同じなので、数える辺と順番は、前の地図全体で行ったときと同じ
            // 窓で行うとき：前の地図全体の辺の交わりの和は、前の地図が正しいので、点を含む前の地図の面の色で決まる（点から
            // 右へ最初に横切る辺の点の側の色 c：c に +1、何も無い所に −1。何も無い所なら 0：`first_crossing`）。窓の辺はその和に
            // 元の形で入っているので、元の形の交わりを引き、入れなかった辺は窓の形の交わりを足す（入れた辺は区間で足してある）。
            // 窓の外の辺は、前の地図全体の和にそのまま入っている。手間は窓の分と、点から最初に横切る辺までの分
            if let (true, Some(out)) = (old, fed_set.outside.as_ref()) {
                let (items, bands) = rows[old as usize].get_or_init(|| unfed_rows(old));
                for &i in bands.at(p2.1) {
                    let (e, q, r) = &items[i as usize];
                    if ray_meets_row(*r, p2) {
                        let ed = self.edges[*e];
                        known_or_made(fed_set.known, self, *e, (q.p0, q.p2), &mut pe);
                        for s in pe.windows(2) {
                            wind_into(dbl(s[0]), dbl(s[1]), ed.left, ed.right, p2, &mut w);
                        }
                    }
                }
                let (items, bands) = orig_rows.get_or_init(|| {
                    let items: Vec<(usize, usize, GridRect)> = (0..n_old)
                        .map(|e| {
                            let be = out.from_window[e];
                            (e, be, reach_of(&out.index.stored.edge_at(be).quad()))
                        })
                        .collect();
                    let bands = YBands::new(items.iter().map(|it| (2 * it.2.y0, 2 * it.2.y1)));
                    (items, bands)
                });
                for &i in bands.at(p2.1) {
                    let (e, be, r) = items[i as usize];
                    if ray_meets_row(r, p2) {
                        let ed = self.edges[e];
                        for s in out.index.stored.poly(be).windows(2) {
                            // 引く：左右の印を入れ替えて足す
                            wind_into(dbl(s[0]), dbl(s[1]), ed.right, ed.left, p2, &mut w);
                        }
                    }
                }
                #[cfg(test)]
                super::OUTSIDE_WINDS.with(|n| n.set(n.get() + 1));
                if let Some((be, left)) = first_crossing(out.index, p2) {
                    let te = out.index.stored.edge_at(be);
                    let c = if left { te.left } else { te.right };
                    if c.is_some() {
                        add_winding(&mut w, c, 1);
                        add_winding(&mut w, None, -1);
                    }
                }
                return w;
            }
            let (items, bands) = rows[old as usize].get_or_init(|| unfed_rows(old));
            for &i in bands.at(p2.1) {
                let (e, q, r) = &items[i as usize];
                if ray_meets_row(*r, p2) {
                    let ed = self.edges[*e];
                    known_or_made(fed_set.known, self, *e, (q.p0, q.p2), &mut pe);
                    for s in pe.windows(2) {
                        wind_into(dbl(s[0]), dbl(s[1]), ed.left, ed.right, p2, &mut w);
                    }
                }
            }
            w
        };
        // その地図の、区間ごとの（左の印, 右の印）。色は、伝えた巻き数で巻き数が正の色（`top_label`。無ければ何も無い所）。
        // 番号は面ごとに1つ：面の輪郭の区間の出どころの辺のうち、面の側の色が面の色と同じものの番号で、いちばん多いもの
        // （同じ数なら小さい番号。番号は図形と面を対応づけるだけで、地図の形と色には効かない）
        let map_sides = |segs: &LabeledSegs<Option<Color32>>,
                         d: &[Option<(Label, Label)>],
                         old: bool,
                         next: &dyn Fn((usize, bool)) -> Turn| {
            let (face, states) = propagate(
                &piece_ends,
                next,
                &|p2| winding_at(segs, old, p2),
                &|w, k, from_left| {
                    let colors = d[pieces[k].prov.clone()]
                        .iter()
                        .flatten()
                        .map(|&(l, r)| (l.0, r.0));
                    crossed(colors, w, from_left)
                },
            );
            let colors: Vec<Option<Color32>> =
                states.iter().map(|w| top_label(w).flatten()).collect();
            let mut votes: Vec<Vec<(ShapeId, usize)>> = vec![Vec::new(); states.len()];
            for (h, &f) in face.iter().enumerate() {
                let f = f as usize;
                for &(l, r) in d[pieces[h / 2].prov.clone()].iter().flatten() {
                    let side = if h % 2 == 1 { l } else { r };
                    if let (true, Some(id)) = (side.0 == colors[f], side.1) {
                        add_vote(&mut votes[f], id);
                    }
                }
            }
            let srcs: Vec<Option<ShapeId>> =
                votes.iter().map(|v| top_vote(v).map(|t| t.0)).collect();
            let label = |h: usize| {
                let f = face[h] as usize;
                (colors[f], srcs[f])
            };
            (0..pieces.len())
                .map(|k| (label(2 * k + 1), label(2 * k)))
                .collect::<Vec<(Label, Label)>>()
        };
        let old_sides = map_sides(&old_segs, &old_deltas, true, &next_old);
        let new_sides = match input {
            MapInput::Map(_) => map_sides(&new_segs, &new_deltas, false, &next_new),
            _ => Vec::new(),
        };
        let old_at = |p2: IPt| top_label(&winding_at(&old_segs, true, p2)).flatten();
        let new_at = |p2: IPt| top_label(&winding_at(&new_segs, false, p2)).flatten();
        // 重ねる形の中か：区間ごとに i_overlay が数えた、左右が重ねる形の中か（中の辺を調べる相手）
        let fill_at = match input {
            MapInput::Shapes(_) => Some(LabeledSegs::new(pieces.iter().map(|p| {
                (
                    p.ia,
                    p.ib,
                    p.fill & CLIP_LEFT != 0,
                    p.fill & CLIP_RIGHT != 0,
                )
            }))),
            _ => None,
        };
        // 輪郭の変形：区間ごとの、巻き数の変わり方を数える印（区間の向きにそろえる）。後の曲線は元の辺の左右の色で、前の
        // 曲線（元の地図の動かした辺）は左右を入れかえて（引く分）。区間を切ったあとの形で数えるので、i_overlay が交点を
        // 丸めた結果と食い違わない
        // 出どころごと（`piece_provs` と同じ並び。数えない出どころは `None`）
        let swept_labels: Vec<Option<Sides>> = match input {
            MapInput::Sweep(_) => piece_provs
                .iter()
                .map(|pv| {
                    let lr = match srcs[pv.src as usize].1 {
                        OverlaySrc::New(i) => new_cols[i],
                        OverlaySrc::Old(e) if moved[e] => (self.edges[e].right, self.edges[e].left),
                        OverlaySrc::Old(_) => return None,
                    };
                    Some(if pv.fwd { lr } else { (lr.1, lr.0) })
                })
                .collect(),
            _ => Vec::new(),
        };
        let swept_of = |k: usize| -> &[Option<Sides>] {
            swept_labels.get(pieces[k].prov.clone()).unwrap_or(&[])
        };
        let swept_segs = LabeledSegs::new(pieces.iter().enumerate().flat_map(|(k, p)| {
            swept_of(k)
                .iter()
                .flatten()
                .map(move |&(l, r)| (p.ia, p.ib, l, r))
        }));
        // 巻き数の並び：動かした辺の並びで、左・右の順（広がってきた色が2つ以上のときは、先の色にする）
        let mut swept_order: Vec<(Option<Color32>, i32)> = Vec::new();
        if let MapInput::Sweep(moved_edges) = input {
            for &(e, _) in moved_edges {
                add_winding(&mut swept_order, self.edges[e].left, 0);
                add_winding(&mut swept_order, self.edges[e].right, 0);
            }
        }
        // 点 `p2`（2倍の座標）での巻き数の変わり方
        let swept_winding = |p2: IPt| {
            let mut w = swept_order.clone();
            swept_segs.winding_into(p2, &mut w);
            w
        };
        // 面ごとの巻き数の変わり方：前の地図の色と同じく、区間をまたぐたびに足し引きして伝える（`propagate`）
        let swept_faces = match input {
            MapInput::Sweep(_) => Some(propagate(
                &piece_ends,
                &next_old,
                &|p2| swept_winding(p2),
                &|w, k, from_left| crossed(swept_of(k).iter().flatten().copied(), w, from_left),
            )),
            _ => None,
        };
        // 区間 `k` の左（`left`）か右での巻き数の変わり方
        let swept_side = |k: usize, left: bool| {
            let (face, states) = swept_faces.as_ref().expect("輪郭の変形の面");
            states[face[2 * k + left as usize] as usize].clone()
        };
        // 下から上へ並べた形：区間ごとに、出どころの形の並びの番号を印にする（区間を切ったあとの形で数えるので、i_overlay が
        // 交点を丸めた結果と食い違わない）。点のある所の色は、巻き数が奇数の形のうち並びでいちばん上の形の色
        let stack = match input {
            MapInput::Stack(shapes) => {
                let shape_of: Vec<u32> = shapes
                    .iter()
                    .enumerate()
                    .flat_map(|(i, s)| {
                        s.contours()
                            .flat_map(move |c| c.segments.iter().map(move |_| i as u32))
                    })
                    .collect();
                let labels: Vec<Vec<u32>> = pieces
                    .iter()
                    .map(|p| {
                        piece_provs[p.prov.clone()]
                            .iter()
                            .filter_map(|pv| match srcs[pv.src as usize].1 {
                                OverlaySrc::New(i) => Some(shape_of[i]),
                                OverlaySrc::Old(_) => None,
                            })
                            .collect()
                    })
                    .collect();
                Some(StackColors::new(shapes, &piece_ends, labels))
            }
            _ => None,
        };
        // 面ごとの、巻き数が奇数の形の集まり：区間をまたぐと、その区間の出どころの形の偶奇だけが変わる（`propagate`）
        let stack_sides: Vec<(Label, Label)> = stack.as_ref().map_or_else(Vec::new, |st| {
            let (face, states) =
                propagate(&piece_ends, &next_old, &|p2| st.odd_at(p2), &|set, k, _| {
                    st.cross(set, k)
                });
            let label = |h: usize| st.label(&states[face[h] as usize]);
            (0..pieces.len())
                .map(|k| (label(2 * k + 1), label(2 * k)))
                .collect()
        });
        // 中の辺（入れなかった辺のうち、重ねるものの範囲の中にある前からある辺と、重ねる地図の辺）：その所での、もう片方の
        // 地図の値（前からある辺なら重ねるものの値、重ねる地図の辺なら前の色）を、切った区間の印で整数で決めて（決まりに
        // よらない）、両側に決まりごとに `rule` を当てる。左右が同じ色になれば消える。中の辺は区間にも、もう片方の地図の入れなかった辺にも触れないので、
        // 値は辺のどこでも同じで、区間の頂点でない頂点でつながる同じ地図の中の辺（組）でも同じ（組ごとに1回だけ調べる）
        let inner: Vec<usize> = (0..self.edges.len()).filter(|&e| interior[e]).collect();
        let mut group: Vec<u32> = (0..inner.len() as u32).collect();
        let mut first_at: IntMap<(bool, usize), usize> = IntMap::default();
        // 同じ線の組の端の頂点：組の両側でもう片方の地図の値が違うので、そこでは組にしない
        let mut pair_node = vec![false; nodes.len()];
        for &e in &inner {
            if mate(e).is_some() {
                let ed = self.edges[e];
                pair_node[node_at(ed.a)] = true;
                pair_node[node_at(ed.b)] = true;
            }
        }
        for (i, &e) in inner.iter().enumerate() {
            let ed = self.edges[e];
            for v in [node_at(ed.a), node_at(ed.b)] {
                if around.get(v).is_empty() && !pair_node[v] {
                    let j = *first_at.entry((e < n_old, v)).or_insert(i);
                    dsu_union(&mut group, i, j);
                }
            }
        }
        // 調べる点は、判定の形の真ん中の区間の真ん中（2倍の座標で、区間の両端の和。両端の頂点は区間と共有することがある
        // ので使わない）
        let probe = |e: usize| {
            let ed = self.edges[e];
            let pe = grid_poly(&QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]));
            let k = pe.len().saturating_sub(2) / 2;
            let (a, b) = (pe[k], pe[(k + 1).min(pe.len() - 1)]);
            (a.0 + b.0, a.1 + b.1)
        };
        let over_at = |p2: IPt| match input {
            MapInput::Shapes(_) => {
                Over::Inside(fill_at.as_ref().and_then(|f| f.at(p2)).unwrap_or(false))
            }
            MapInput::Map(_) => Over::Color(new_at(p2)),
            MapInput::Sweep(_) => Over::Swept(swept_winding(p2)),
            MapInput::Stack(_) => {
                Over::Color(stack.as_ref().and_then(|st| st.label(&st.odd_at(p2)).0))
            }
        };
        let mut over_memo: IntMap<u32, Over> = IntMap::default();
        let mut old_color_memo: IntMap<u32, Option<Color32>> = IntMap::default();
        // 決まりごとの、残す区間（`kept`）・区間の新しい色・残す中の辺・中の辺の新しい色
        struct Ruled {
            kept: Vec<(usize, usize, (Label, Label), usize)>,
            piece_cols: Vec<Sides>,
            interior_kept: Vec<MapEdge>,
            inner_cols: IntMap<usize, Sides>,
        }
        let mut ruled: Vec<Ruled> = Vec::with_capacity(rules.len());
        let mut refeed: Vec<usize> = Vec::new();
        let mut turns: Vec<(IPt, Sides, Option<usize>)> = Vec::new();
        for &rule in rules {
            let copy_untouched = copies_untouched(rule);
            // 5. 区間の新しい印（色と図形の番号）。左右の色が違う区間だけ残す：（始点, 終点, (左の印, 右の印), 区間の番号）
            let mut kept: Vec<(usize, usize, (Label, Label), usize)> = Vec::new();
            // 区間ごとの新しい色（残さない区間も）
            let mut piece_cols: Vec<Sides> = Vec::with_capacity(pieces.len());
            for (k, piece) in pieces.iter().enumerate() {
                let side = |fwd: bool| {
                    let pick = |s: (Label, Label)| if fwd { s.0 } else { s.1 };
                    let old = pick(old_sides[k]);
                    let (over, over_src) = match input {
                        MapInput::Shapes(_) => (
                            Over::Inside(
                                piece.fill & if fwd { CLIP_LEFT } else { CLIP_RIGHT } != 0,
                            ),
                            None,
                        ),
                        MapInput::Map(_) => {
                            let l = pick(new_sides[k]);
                            (Over::Color(l.0), l.1)
                        }
                        MapInput::Sweep(_) => (Over::Swept(swept_side(k, fwd)), None),
                        MapInput::Stack(_) => {
                            let l = pick(stack_sides[k]);
                            (Over::Color(l.0), l.1)
                        }
                    };
                    rule.apply_label(&over, over_src, old)
                };
                let (left, right) = (side(true), side(false));
                piece_cols.push((left.0, right.0));
                if left.0 != right.0 {
                    kept.push((piece.a, piece.b, (left, right), k));
                }
            }
            let mut interior_kept: Vec<MapEdge> = Vec::new();
            // 中の辺ごとの新しい色（残さない辺も）
            let mut inner_cols: IntMap<usize, Sides> = IntMap::default();
            for (i, &e) in inner.iter().enumerate() {
                let ed = self.edges[e];
                if dropped(e) {
                    continue;
                }
                // 同じ線の組：両側の値は、相手（重ねる地図の辺）のその側の印
                if let Some((f, same_dir)) = mate(e) {
                    let fd = self.edges[f];
                    let (fl, fr) = if same_dir {
                        ((fd.left, fd.left_src), (fd.right, fd.right_src))
                    } else {
                        ((fd.right, fd.right_src), (fd.left, fd.left_src))
                    };
                    let l = rule.apply_label(&Over::Color(fl.0), fl.1, (ed.left, ed.left_src));
                    let r = rule.apply_label(&Over::Color(fr.0), fr.1, (ed.right, ed.right_src));
                    inner_cols.insert(e, (l.0, r.0));
                    if l.0 != r.0 {
                        interior_kept.push(MapEdge {
                            a: node_at(ed.a),
                            b: node_at(ed.b),
                            left: l.0,
                            right: r.0,
                            left_src: l.1,
                            right_src: r.1,
                            ..ed
                        });
                    }
                    continue;
                }
                let g = dsu_find(&mut group, i as u32);
                let (l, r) = if e < n_old {
                    let over = over_memo.entry(g).or_insert_with(|| over_at(probe(e)));
                    (rule.apply(over, ed.left), rule.apply(over, ed.right))
                } else {
                    let old = *old_color_memo.entry(g).or_insert_with(|| old_at(probe(e)));
                    (
                        rule.apply(&Over::Color(ed.left), old),
                        rule.apply(&Over::Color(ed.right), old),
                    )
                };
                inner_cols.insert(e, (l, r));
                if l != r {
                    // 元の図形の印は、色が変わらない側だけ残す
                    interior_kept.push(MapEdge {
                        a: node_at(ed.a),
                        b: node_at(ed.b),
                        left: l,
                        right: r,
                        left_src: if l == ed.left { ed.left_src } else { None },
                        right_src: if r == ed.right { ed.right_src } else { None },
                        ..ed
                    });
                }
            }
            // 区間の頂点のまわりで、となり合う向き（区間か、入れなかった辺）の間の面の色がそろうかを確かめる。入れなかった辺の
            // 色は自分の所で、区間の色は面ごとに決めるので、i_overlay が区間を動かして、入れなかった辺がどの面に接するかが
            // 変わると食い違いうる。食い違う入れなかった辺は、入れて切り直す（入れる辺は増える一方なので、くり返しは必ず終わる）。
            // 区間の頂点でない所では、中の辺どうしは同じ値でそろい、区間どうしは面ごとの色でそろう
            let unfed_cols = |e: usize| match inner_cols.get(&e) {
                Some(&c) => c,
                None if copy_untouched => (self.edges[e].left, self.edges[e].right),
                None => (None, None),
            };
            for v in (0..stops.len()).filter(|&v| !stops.get(v).is_empty()) {
                // 頂点から出ていく向きと、その左右の色（と、入れなかった辺ならその番号）
                turns.clear();
                turns.extend(around.get(v).iter().map(|&(k, fwd)| {
                    let (l, r) = piece_cols[k];
                    (dir_of(&(k, fwd)), if fwd { (l, r) } else { (r, l) }, None)
                }));
                for &(e, at_start, d) in stops.get(v).iter().filter(|s| !dropped(s.0)) {
                    let (l, r) = unfed_cols(e);
                    turns.push((d, if at_start { (l, r) } else { (r, l) }, Some(e)));
                }
                turns.sort_by(|x, y| cmp_dir(wide(x.0), wide(y.0)));
                // 角度の順でとなり合う2つの間の面は、前の左で、次の右
                for i in 0..turns.len() {
                    let (a, b) = (&turns[i], &turns[(i + 1) % turns.len()]);
                    if a.1 .0 != b.1 .1 {
                        refeed.extend(a.2);
                        refeed.extend(b.2);
                    }
                }
            }
            ruled.push(Ruled {
                kept,
                piece_cols,
                interior_kept,
                inner_cols,
            });
        }
        if !refeed.is_empty() {
            refeed.sort_unstable();
            refeed.dedup();
            return Err(OverlayStop::Unfed(refeed));
        }
        let mut maps = Vec::with_capacity(rules.len());
        for (ri, (&rule, r)) in rules.iter().zip(ruled).enumerate() {
            let copy_untouched = copies_untouched(rule);
            let Ruled {
                kept,
                piece_cols,
                interior_kept,
                inner_cols,
            } = r;
            // 6. 区間をつないで鎖にする。頂点の次数が 2 で、出どころと左右の色が同じで、t が続いていれば、つなげる
            let at: Grouped<usize> = Grouped::new(
                nodes.len(),
                kept.iter()
                    .enumerate()
                    .flat_map(|(ki, &(a, b, ..))| [(a, ki), (b, ki)]),
            );
            let mut untouched_deg: Vec<usize> = vec![0; nodes.len()];
            for (e, ed) in self.edges.iter().enumerate() {
                if !fed[e] && !interior[e] && copy_untouched {
                    untouched_deg[ed.a] += 1;
                    untouched_deg[ed.b] += 1;
                }
            }
            for ed in &interior_kept {
                untouched_deg[ed.a] += 1;
                untouched_deg[ed.b] += 1;
            }
            // 区間の主な出どころ：前からある辺、重ねる線、輪郭の変形で動かした辺（後の曲線に置き換わる）の順。区間の向き
            // （a→b）での t
            let main_src = |ki: usize| -> Prov {
                let rank = |p: &&Prov| match srcs[p.src as usize].1 {
                    OverlaySrc::Old(e) if !moved[e] => 0,
                    OverlaySrc::New(_) => 1,
                    OverlaySrc::Old(_) => 2,
                };
                *piece_provs[pieces[kept[ki].3].prov.clone()]
                    .iter()
                    .min_by_key(rank)
                    .expect("区間には出どころがある")
            };
            // 鎖の向きでの（始点, 終点, t の始め, t の終わり）
            let piece_ends = |&(ki, fwd): &(usize, bool)| {
                let (a, b, ..) = kept[ki];
                let m = main_src(ki);
                if fwd {
                    (a, b, m.t0, m.t1)
                } else {
                    (b, a, m.t1, m.t0)
                }
            };
            let mut used = vec![false; kept.len()];
            let mut chains: Vec<Chain> = Vec::new();
            let mut chain: std::collections::VecDeque<(usize, bool)> =
                std::collections::VecDeque::new();
            for start in 0..kept.len() {
                if used[start] {
                    continue;
                }
                chain.clear();
                chain.push_back((start, true));
                used[start] = true;
                // 鎖は左右の色が同じ区間をつなぐ。図形の番号は、鎖の左右それぞれで最初に見つかったもの（鎖の左右は、それぞれ
                // 結果の1つの面。番号は面と図形を対応づけるだけなので、区間ごとの番号の違いでは鎖を分けない）
                let (_, _, (mut sl, mut sr), _) = kept[start];
                let s_src = main_src(start).src;
                for forward in [true, false] {
                    loop {
                        let cur = if forward {
                            *chain.back().unwrap()
                        } else {
                            *chain.front().unwrap()
                        };
                        let (pa, pb, ta, tb) = piece_ends(&cur);
                        let (end, t_end) = if forward { (pb, tb) } else { (pa, ta) };
                        let deg = at.get(end).len() + untouched_deg[end];
                        if deg != 2 {
                            break;
                        }
                        let Some(&nk) = at.get(end).iter().find(|&&x| x != cur.0) else {
                            break;
                        };
                        if used[nk] || main_src(nk).src != s_src {
                            break;
                        }
                        let (na, nb, (nl, nr), _) = kept[nk];
                        let n_same = if forward { na == end } else { nb == end };
                        let (l, r) = if n_same { (nl, nr) } else { (nr, nl) };
                        let (_, _, nta, ntb) = piece_ends(&(nk, n_same));
                        let t_join = if forward { nta } else { ntb };
                        // 同じ出どころの続いた区間は、つなぎ目の t がビットまで同じ（入れるときは前の区間の終わりの t を
                        // 次の区間の始めにし、i_overlay が切るときは2つに同じ t を渡す：`ProvId::split`）
                        if (l.0, r.0) != (sl.0, sr.0) || t_join != t_end {
                            break;
                        }
                        sl.1 = sl.1.or(l.1);
                        sr.1 = sr.1.or(r.1);
                        used[nk] = true;
                        if forward {
                            chain.push_back((nk, n_same));
                        } else {
                            chain.push_front((nk, n_same));
                        }
                    }
                }
                let steps: Vec<(usize, usize, f64, f64)> = chain.iter().map(piece_ends).collect();
                let (n0, n1, t0, t1) = (
                    steps[0].0,
                    steps[steps.len() - 1].1,
                    steps[0].2,
                    steps[steps.len() - 1].3,
                );
                // 入れた辺（前からある辺か重ねる地図の辺）が丸ごと残ったときは、元の曲線をそのまま使う（図形の番号は、ほかの
                // 区間と同じく、面ごとに伝えた印のもの）
                let mut whole = None;
                let mut whole_poly = None;
                let fed_edge = match srcs[s_src as usize].1 {
                    OverlaySrc::Old(e) => Some(e),
                    OverlaySrc::New(i) if matches!(input, MapInput::Map(_)) => Some(n_old + i),
                    OverlaySrc::New(_) => None,
                };
                if let Some(e) = fed_edge {
                    let ed = self.edges[e];
                    // 辺の端の t はちょうど 0 と 1（`flatten_with_t`。切っても外側の t は変わらない）
                    let full = t0.min(t1) == 0.0 && t0.max(t1) == 1.0;
                    let (ea, eb) = (node_at(ed.a), node_at(ed.b));
                    let same_dir = (n0, n1) == (ea, eb);
                    if full && n0 != n1 && (same_dir || (n0, n1) == (eb, ea)) {
                        whole = Some(ed.ctrl);
                        let same_ends =
                            (nodes[n0], nodes[n1]) == (self.nodes[ed.a], self.nodes[ed.b]);
                        if same_dir && same_ends {
                            whole_poly = fed_set.known.get(e).cloned();
                        }
                    }
                }
                chains.push(Chain {
                    steps,
                    left: sl,
                    right: sr,
                    curve: srcs[s_src as usize].0,
                    whole,
                    whole_poly,
                });
            }
            // 7. 区間をすべて直線の辺にした地図（いつも正しい）から始めて、鎖ごとに曲線へ置き換える
            //    重ねるものから離れた辺は、そのまま（元の図形の印も残す）
            let mut edges: Vec<MapEdge> = if copy_untouched {
                (0..self.edges.len())
                    .filter(|&e| !fed[e] && !interior[e])
                    .map(|e| self.edges[e])
                    .collect()
            } else {
                Vec::new()
            };
            edges.extend(interior_kept);
            let mut chain_edges: Vec<Vec<usize>> = Vec::with_capacity(chains.len());
            for c in &chains {
                chain_edges.push(
                    c.steps
                        .iter()
                        .map(|&(a, b, ..)| {
                            edges.push(MapEdge {
                                a,
                                b,
                                ctrl: nodes[a].midpoint(nodes[b]),
                                left: c.left.0,
                                right: c.right.0,
                                left_src: c.left.1,
                                right_src: c.right.1,
                            });
                            edges.len() - 1
                        })
                        .collect(),
                );
            }
            // 置き換えで調べる相手になりうるのは、置き換える曲線と帯の外接矩形にかかる辺と頂点だけ。曲線の端は区間の頂点、
            // 制御点は出どころの曲線の一部の制御点なので出どころの曲線の制御多角形の中にある。帯は区間と曲線で囲まれる
            let active = chains
                .iter()
                .map(|c| {
                    let q = c.curve;
                    c.steps.iter().fold(
                        KRect::from_points(q.p0, q.p1).union_pt(q.p2),
                        |r, &(a, b, ..)| r.union_pt(nodes[a]).union_pt(nodes[b]),
                    )
                })
                .reduce(|x, y| x.union(y))
                .map(|r| r.inflate(GRID_REACH, GRID_REACH));
            // この重ね合わせで消えた線（左右が同じ色になって捨てた区間と中の辺）の端の頂点が、2本だけがつながる頂点を
            // まとめる候補（`merge_degree2`。切り口の点が、相手の線が消えた後に角として残らない）
            let dropped_ends: Vec<usize> = pieces
                .iter()
                .zip(&piece_cols)
                .filter(|(_, c)| c.0 == c.1)
                .flat_map(|(p, _)| [p.a, p.b])
                .chain(
                    inner
                        .iter()
                        .filter(|e| inner_cols.get(e).is_some_and(|c| c.0 == c.1))
                        .flat_map(|&e| [node_at(self.edges[e].a), node_at(self.edges[e].b)]),
                )
                .collect();
            // 頂点はどの地図も同じ並びから始める（使わない頂点は最後に詰める）
            let nodes = if ri + 1 == rules.len() {
                std::mem::take(&mut nodes)
            } else {
                nodes.clone()
            };
            let mut restorer = ChainRestorer::new(PlanarMap { nodes, edges }, active);
            for (c, ids) in chains.iter().zip(&chain_edges) {
                restorer.restore(c, ids);
            }
            let (mut m, index) = restorer.finish();
            let used = m.merge_degree2_indexed(dropped_ends, index);
            fed_set.used = match (fed_set.used, used) {
                (Some(a), Some(b)) => Some(a.union(&b)),
                (a, b) => a.or(b),
            };
            m.compact_nodes();
            maps.push(m);
        }
        Ok(maps)
    }
}

/// お手本との比べ（設計 段2b の 4. の 2b-4）：前の地図 `base` と新しい地図 `next` の輪郭を、番号によらない形
/// （`cycle_shape`）にして、`touched`（前の地図＋差分でたどった触った輪郭の形。並べたもの）が、`next` の輪郭のうち
/// `touched` の向きのある辺を含むものの集まりと同じで、`next` のほかの輪郭がどれも `base` にあるか。`next` が正しい地図で
/// なければ比べない（`true`）
#[cfg(test)]
fn window_cycles_match(
    base: &PlanarMap,
    next: &PlanarMap,
    touched: &[Vec<([u64; 6], bool)>],
) -> bool {
    if !next.is_valid() {
        return true;
    }
    let shapes = |m: &PlanarMap| -> Vec<Vec<([u64; 6], bool)>> {
        let polys = m.polys();
        m.face_cycles_with(&polys)
            .iter()
            .map(|c| {
                cycle_shape(c.iter().map(|&(e, forward)| {
                    let q = m.quad(e);
                    (quad_key(&q), forward == point_order(q.p0, q.p2).is_le())
                }))
            })
            .collect()
    };
    let touched_halves: std::collections::HashSet<([u64; 6], bool)> =
        touched.iter().flatten().copied().collect();
    let (mut hit, mut rest): (Vec<_>, Vec<_>) = shapes(next)
        .into_iter()
        .partition(|c| c.iter().any(|h| touched_halves.contains(h)));
    hit.sort();
    if hit != touched {
        return false;
    }
    let base_shapes: std::collections::HashSet<Vec<([u64; 6], bool)>> =
        shapes(base).into_iter().collect();
    rest.retain(|c| !base_shapes.contains(c));
    rest.is_empty()
}

/// 前の地図の辺 `edges`（計算に使う地図の番号の順）を並べた小さな地図と、辺ごとの計算に使う形。辺は塊から引き
/// （`StoredMap::edge_at`。向きはしまう辺の向きで、計算に使う地図の辺と同じ）、頂点は座標ごとに初めて出てきた順に番号を
/// 振る（設計 段2b の 7. の 2b-7a。写しの通し番号の一覧を使わない）
pub(super) fn tile_window(stored: &StoredMap, edges: &[usize]) -> (PlanarMap, Vec<Poly>) {
    let mut node_of: IntMap<[u64; 2], usize> = IntMap::default();
    let mut nodes: Vec<Point> = Vec::new();
    let mut id = |p: Point, nodes: &mut Vec<Point>| -> usize {
        *node_of
            .entry([p.x.to_bits(), p.y.to_bits()])
            .or_insert_with(|| {
                nodes.push(p);
                nodes.len() - 1
            })
    };
    let mut list: Vec<MapEdge> = Vec::with_capacity(edges.len());
    let mut polys: Vec<Poly> = Vec::with_capacity(edges.len());
    for &e in edges {
        let te = stored.edge_at(e);
        let (a, b) = (id(te.a, &mut nodes), id(te.b, &mut nodes));
        list.push(MapEdge {
            a,
            b,
            ctrl: te.ctrl,
            left: te.left,
            right: te.right,
            left_src: te.left_src,
            right_src: te.right_src,
        });
        polys.push(Arc::clone(stored.poly(e)));
    }
    (PlanarMap { nodes, edges: list }, polys)
}

/// 同じ線の組（前からある辺, 重ねる地図の辺, 同じ向きか）の並びと、組の端の重ねる地図の頂点 → 前からある頂点
/// （`PlanarMap::same_edges`）
type SameEdges = (Vec<(usize, usize, bool)>, IntMap<usize, usize>);

/// 線 `line` と前からある辺 `e` が重なる所 `pts`（`PlanarMap::overlap`）を、2本の同じ線として扱う所（`Span`）として記す。
/// 1本の線の上で、前に記した所と重なるなら記さない（`false`）
fn add_span(spans: &mut IntMap<Line, Vec<Span>>, line: Line, e: usize, pts: &OverlapPts) -> bool {
    let free = |list: Option<&Vec<Span>>, lo: f64, hi: f64| {
        list.is_none_or(|v| v.iter().all(|sp| hi <= sp.lo || lo >= sp.hi))
    };
    let (s_lo, s_hi) = (pts[0].1, pts[pts.len() - 1].1);
    let (u_a, u_b) = (pts[0].2, pts[pts.len() - 1].2);
    let (u_lo, u_hi) = (u_a.min(u_b), u_a.max(u_b));
    if !free(spans.get(&line), s_lo, s_hi) || !free(spans.get(&Line::Edge(e)), u_lo, u_hi) {
        return false;
    }
    let mut on_edge: Vec<(Point, f64)> = pts.iter().map(|&(p, _, u)| (p, u)).collect();
    if u_a > u_b {
        on_edge.reverse();
    }
    for (key, span) in [
        (
            line,
            Span {
                lo: s_lo,
                hi: s_hi,
                pts: pts.iter().map(|&(p, s, _)| (p, s)).collect(),
            },
        ),
        (
            Line::Edge(e),
            Span {
                lo: u_lo,
                hi: u_hi,
                pts: on_edge,
            },
        ),
    ] {
        let list = spans.entry(key).or_default();
        let at = list.partition_point(|sp| sp.lo < span.lo);
        list.insert(at, span);
    }
    true
}

/// 窓の前からある辺の（窓の辺の番号, 前の地図での番号, 元の形の外接矩形）と、その高さの帯の索引（`overlay_fed` の
/// `winding_at`）
type OrigRows = (Vec<(usize, usize, GridRect)>, YBands);

/// 半直線が横切る辺（`first_crossing`）：横切る所, 上へ向けた区間の向き, 点の側の向きのある辺
type Crossing = (f64, (i64, i64), (usize, bool));

/// 格子の点を2倍の座標にする
fn dbl(p: IPt) -> IPt {
    (2 * p.0, 2 * p.1)
}

/// 前の地図（升目の索引 `ix` の地図）で、点 `q`（2倍の座標）から右へ伸ばした半直線が最初に横切る辺の、q の側の向きの
/// ある辺（点を右上へごくわずかずらした所として数える：`crossing` と同じ。点を通る区間は横切らない。同じ頂点で横切る辺は、
/// ずらした高さで先に横切るもの）。前の地図が正しければ、ずらした点を含む面は、この向きのある辺の左の面で、前の地図全体の
/// 辺の `crossing` の和は、その面の色で決まる。横切る辺が無ければ `None`（どの面にも入らない）。横切る所の列の順に、列ごとに
/// 升目の索引を引く（手間は、最初に横切る辺までの分）
pub(super) fn first_crossing(ix: MapIndex<'_>, q: IPt) -> Option<(usize, bool)> {
    let col_of = |x2: f64| ((x2 / 2.0) / (GRID_CELL * 4) as f64).floor() as i64;
    let (x, y) = (q.0.div_euclid(2), q.1.div_euclid(2));
    let (t0, ty0) = index_tile(cell_of((x - 1, y - 1)));
    let ty1 = index_tile(cell_of((x - 1, y + 2))).1;
    let last = ix.index.max_tile_x().unwrap_or(t0 - 1);
    let stored = ix.stored;
    let mut cands: Vec<usize> = Vec::new();
    for tx in t0..=last {
        cands.clear();
        for ty in ty0..=ty1 {
            ix.index.edges_in(stored, (tx, ty), &mut cands);
        }
        cands.sort_unstable();
        cands.dedup();
        let mut best: Option<Crossing> = None;
        for &e in &cands {
            for s in stored.poly(e).windows(2) {
                let (a, b) = (dbl(s[0]), dbl(s[1]));
                let Some(d) = crossing(a, b, q) else {
                    continue;
                };
                let t = (q.1 - a.1) as f64 / (b.1 - a.1) as f64;
                let xx = a.0 as f64 + t * (b.0 - a.0) as f64;
                if col_of(xx) != tx {
                    continue;
                }
                let up = if b.1 > a.1 {
                    (b.0 - a.0, b.1 - a.1)
                } else {
                    (a.0 - b.0, a.1 - b.1)
                };
                // 上へ横切る区間は左が、下へ横切る区間は右が q の側
                let side = (e, d > 0);
                let better = best.is_none_or(|(bx, bu, _)| {
                    xx < bx
                        || (xx == bx
                            && (up.0 as i128) * (bu.1 as i128) < (bu.0 as i128) * (up.1 as i128))
                });
                if better {
                    best = Some((xx, up, side));
                }
            }
        }
        if let Some((_, _, side)) = best {
            return Some(side);
        }
    }
    None
}

/// 窓で重ね合わせるときの、下窓から窓へ渡す線の番号（窓によらない形）：前からある辺（前の地図の番号）・重ねる地図の辺
/// （その番号）・重ねる線（`Line::Seed`）
#[derive(Clone, Copy)]
enum SpanLine {
    Old(usize),
    New(usize),
    Seed(usize),
}

/// 同じ線として扱う所（`Span`）にかかわる線：重ねる線（形・輪郭の変形の線。`fed_edges` の `seeds` の番号）か、辺（前から
/// ある辺と、重ねる地図の辺。`fed_edges` の地図の辺の番号）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Line {
    Seed(usize),
    Edge(usize),
}

/// 線の上の、ほかの線と同じ線として扱う所：自分の上の範囲（小さい方, 大きい方）と、その所の点（自分の上の t の順）。
/// 地図は曲線を `FLAT_TOL` の精度でしか表さないので、前からある辺と重ねる線が、どこでも `FLAT_TOL` 以内に重なる所は
/// 同じ線とし、2本とも同じ点の折れ線にする（同じ線を重ねても、折れ線どうしが格子の数目の幅で交わって、見えない欠片の
/// 面ができたり、境目が細かい直線に割れたりしない。レビュー 6cca852 の N-1001、7f63b04 の N-1003）
#[derive(Clone, Debug)]
pub(super) struct Span {
    lo: f64,
    hi: f64,
    pts: Vec<(Point, f64)>,
}

impl PlanarMap {
    /// 重ねる線 `q` と前からある辺 `e` が同じ線になっている所（どちらがどちらに含まれても、端の一部どうしが重なっても
    /// よい。両端が `FLAT_TOL` より離れている所）：その所の点と、`q` の上の t、辺の上の t（`q` の上の t の順）。点は、辺の曲線のその所を切り出して折れ線に
    /// したもので、両端は重なりの端になっている方の線の端の点。辺の t と `q` の t は、同じ曲線の部分なら1次式で対応する
    /// ので、2本のその所を切り出した曲線が同じ曲線（`same_curve`：両側の制御点で決める）のときだけ
    pub(super) fn overlap(&self, q: &QuadBez, e: usize) -> Option<OverlapPts> {
        use kurbo::ParamCurveNearest;
        let m = self.quad(e);
        // 曲線から確かに `FLAT_TOL` より遠い点は、いちばん近い点を探さない（`nearest_on_quad` の距離は本当の距離以上
        // なので、探しても同じく外れる）。曲線は外接矩形の中にあり、弦から `bend / 4` 以内にある（2次ベジェと弦の差は
        // 2t(1−t)(p1 − (p0 + p2)/2) で、2t(1−t) ≤ 1/2）ので、矩形から遠い点と、弦までの距離から `bend / 4` を引いても
        // 遠い点（丸めの分の余裕をとる）は遠い
        let (qr, mr) = (q.bounding_box(), m.bounding_box());
        let (q_bend, m_bend) = (bend(q) / 4.0, bend(&m) / 4.0);
        let near = |c: &QuadBez, r: KRect, c_bend: f64, p: Point| {
            p.x >= r.x0 - FLAT_TOL
                && p.x <= r.x1 + FLAT_TOL
                && p.y >= r.y0 - FLAT_TOL
                && p.y <= r.y1 + FLAT_TOL
                && kurbo::Line::new(c.p0, c.p2)
                    .nearest(p, 0.0)
                    .distance_sq
                    .sqrt()
                    - c_bend
                    <= FLAT_TOL * 1.001
        };
        // 両方の線の端のうち、もう一方の線の上にあるもの（多くて4つ）
        let mut ends = [(0.0, 0.0, Point::ZERO); 4];
        let mut n_ends = 0;
        for (u, p) in [(0.0, m.p0), (1.0, m.p2)] {
            if !near(q, qr, q_bend, p) {
                continue;
            }
            let (s, d) = nearest_on_quad(q, p);
            if d <= FLAT_TOL {
                ends[n_ends] = (s, u, p);
                n_ends += 1;
            }
        }
        for (s, p) in [(0.0, q.p0), (1.0, q.p2)] {
            if !near(&m, mr, m_bend, p) {
                continue;
            }
            let (u, d) = nearest_on_quad(&m, p);
            if d <= FLAT_TOL {
                ends[n_ends] = (s, u, p);
                n_ends += 1;
            }
        }
        let ends = &ends[..n_ends];
        let lo = *ends.iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
        let hi = *ends.iter().max_by(|a, b| a.0.total_cmp(&b.0))?;
        // 両端が `FLAT_TOL` 以内なら、地図の精度では1つの点（端どうしが触れているだけ）
        if lo.0 >= hi.0 || lo.1 == hi.1 || (hi.2 - lo.2).hypot() <= FLAT_TOL {
            return None;
        }
        let sub = m.subsegment(lo.1.min(hi.1)..lo.1.max(hi.1));
        // 辺のその所を `q` の向きにしたもの
        let along = if lo.1 < hi.1 {
            sub
        } else {
            QuadBez::new(sub.p2, sub.p1, sub.p0)
        };
        if !same_curve(&q.subsegment(lo.0..hi.0), &along, FLAT_TOL) {
            return None;
        }
        // 辺のその所を折れ線にした点と、`q` の向きでの t（はじめは 3 つ目に入れておく）
        let mut pts: OverlapPts = flat_points(sub, FLAT_TOL)
            .map(|(p, t)| (p, t, 0.0))
            .collect();
        if lo.1 > hi.1 {
            pts.reverse();
            for p in &mut pts {
                p.1 = 1.0 - p.1;
            }
        }
        let at = |t: f64| (lo.0 + (hi.0 - lo.0) * t, lo.1 + (hi.1 - lo.1) * t);
        let n = pts.len();
        for (k, pt) in pts.iter_mut().enumerate() {
            *pt = match k {
                0 => (lo.2, lo.0, lo.1),
                _ if k + 1 == n => (hi.2, hi.0, hi.1),
                _ => {
                    let (s, u) = at(pt.1);
                    (pt.0, s, u)
                }
            };
        }
        Some(pts)
    }
}

/// 2本の2次ベジェ `a`・`b`（同じ向きで、t が同じ所どうしを比べる）が、許容 `tol` の中で同じ曲線か：3組の制御点がみな
/// `tol` 以内。2本の差も2次ベジェで、曲線の点は制御点の重みつきの平均（重みの和は1）なので、どこでも2本の離れ具合は
/// 制御点の離れ具合以下になる。両側に同じ決まりがかかり、折れ線の細かさによらない（レビュー 4f4e953 の N-1001）。
/// 同じ曲線かの判定はこれ1つ（同じ線として扱う所は地図の精度 `FLAT_TOL`、2本の辺のまとめは切れ目の頂点のずれも入れた
/// `FLAT_TOL + SPLIT_REACH`）
pub(super) fn same_curve(a: &QuadBez, b: &QuadBez, tol: f64) -> bool {
    [(a.p0, b.p0), (a.p1, b.p1), (a.p2, b.p2)]
        .iter()
        .all(|&(x, y)| (x - y).hypot() <= tol)
}

/// 同じ線になっている所の点：（点, 重ねる線の上の t, 辺の上の t）。重ねる線の上の t の順
pub(super) type OverlapPts = Vec<(Point, f64, f64)>;

/// 曲線を折れ線にした点 `own`（点と t）の、同じ線として扱う所 `spans`（重ならない。範囲の順）を、その所の点に替えたもの
/// （`out` に入れる。前の中身は消す）。両端はいつも自分の端の点で、続く点の t が同じなら後の点を落とす（区間の向きは t の
/// 大きさで決めるので、t の幅が 0 の区間を作らない）
pub(super) fn stitched(own: &[(Point, f64)], spans: &[Span], out: &mut Vec<(Point, f64)>) {
    out.clear();
    if spans.is_empty() {
        out.extend_from_slice(own);
        return;
    }
    let n = own.len();
    out.push(own[0]);
    let push = |out: &mut Vec<(Point, f64)>, p: (Point, f64)| {
        if out.last().is_none_or(|l| l.1 != p.1) {
            out.push(p);
        }
    };
    let mut k = 1;
    for sp in spans {
        while k < n - 1 && own[k].1 < sp.lo {
            push(out, own[k]);
            k += 1;
        }
        for &p in &sp.pts {
            push(out, p);
        }
        while k < n - 1 && own[k].1 <= sp.hi {
            k += 1;
        }
    }
    for &p in &own[k..n - 1] {
        push(out, p);
    }
    let end = own[n - 1];
    match out.last_mut() {
        Some(l) if l.1 == end.1 => *l = end,
        _ => out.push(end),
    }
}

/// 折れ線の点を作る入れ物（使い回して、確保を減らす：`FedEdges::points`）
#[derive(Default)]
pub(super) struct PointsBuf {
    own: Vec<(Point, f64)>,
    out: Vec<(Point, f64)>,
}

/// i_overlay に入れる前からある辺（`fed`）と、そのときに作った辺の外接矩形・格子・頂点ごとの辺
pub(super) struct FedEdges<'a> {
    fed: Vec<bool>,
    /// 前から `known.len()` 本の辺の判定の形（計算に使う写しが持つもの：`fed_edges` の `known`）
    known: &'a [Poly],
    /// 線ごとの、同じ線として扱う所（範囲の順）
    spans: IntMap<Line, Vec<Span>>,
    /// 近くの辺と、頂点ごとの辺を探す道具（`fed_edges` が作って渡す）
    near: Option<NearEdges<'a>>,
    /// 辺ごとの、同じ線の組の相手と、同じ向きか（`same_edges`。重ねる地図でなければ空）
    partner: Vec<Option<(usize, bool)>>,
    /// 重ね合わせを窓で行うときの、窓の外（`overlay_rules_windowed`。窓で行わなければ `None`）
    outside: Option<Outside<'a>>,
    /// ほかの辺を判定の形で調べた範囲（2本をまとめる所の索引の範囲：`merge_degree2_indexed`。窓で行うとき、窓がこれを
    /// 覆うかを確かめる）
    used: Option<GridRect>,
}

/// 重ね合わせを窓（前の地図の辺の一部を、番号の順のまま並べた小さな地図）で行うときの、窓の外：前の地図と、その辺の
/// 計算に使う形と升目の索引、前の地図の辺の番号から窓の辺の番号への対応。点のある所の前の地図の色を数えるとき
/// （`overlay_fed` の `winding_at`）、窓の外の辺も、前の地図の辺の番号の順に数える
pub(super) struct Outside<'a> {
    index: MapIndex<'a>,
    /// 窓の辺の、前の地図での番号
    from_window: Vec<usize>,
}

impl<'a> FedEdges<'a> {
    fn near(&self) -> &NearEdges<'a> {
        self.near.as_ref().expect("fed_edges が作る")
    }

    fn near_mut(&mut self) -> &mut NearEdges<'a> {
        self.near.as_mut().expect("fed_edges が作る")
    }
}

/// 計算に使う写しの升目の索引（`LayerBase::index`）と、その地図のしまう形。重ね合わせ（`overlay_rules_known`）が、
/// 前からある辺の近くを、毎回全部の辺を格子に入れ直さずに探すのに使う
#[derive(Clone, Copy)]
pub(crate) struct MapIndex<'a> {
    pub(crate) index: &'a crate::stored_map::TileIndex,
    /// 端の升目の索引（`LayerBase::ends_index`）
    pub(crate) ends: &'a crate::stored_map::TileIndex,
    pub(crate) stored: &'a StoredMap,
}

/// 重ね合わせで、近くの辺と、頂点ごとの辺を探す道具。地図 `map` の辺のうち、前から `n_index` 本（計算に使う写しの
/// 地図の辺。升目の索引がある）は索引を引いて、要る升目の辺だけを近さを調べる格子に入れ、残りの辺（重ねる地図の辺）は
/// はじめに格子に入れる。索引が無ければ、全部の辺をはじめに格子に入れる（前と同じ）。どちらでも、マスで探した辺の並び
/// （マスの順、マスの中は辺の番号の順）と、頂点ごとの辺の並び（辺の番号の順）は、全部の辺を格子に入れたときと同じ
pub(super) struct NearEdges<'a> {
    map: &'a PlanarMap,
    known: &'a [Poly],
    index: Option<MapIndex<'a>>,
    /// 索引を引く辺の数（`0..n_index`）
    n_index: usize,
    /// 索引を引かない辺（`n_index..`）の外接矩形とマス（`known` に無い辺の分）
    made: Vec<EdgeReach>,
    /// 索引を引かない辺の格子（番号は `n_index` を引いたもの）と、頂点ごとの辺（辺の番号の順）
    local: CellGrid,
    local_adj: IntMap<usize, Vec<usize>>,
    /// 索引から読んだ升目と、読んだ辺、マスごとの読んだ辺（辺の番号の順）
    loaded_tiles: std::collections::HashSet<(i64, i64)>,
    loaded_edges: std::collections::HashSet<usize>,
    cell_edges: IntMap<(i64, i64), Vec<usize>>,
    /// 探すときに同じ辺を2度返さないための印
    seen: std::collections::HashSet<usize>,
    buf: Vec<usize>,
}

impl<'a> NearEdges<'a> {
    pub(super) fn new(map: &'a PlanarMap, known: &'a [Poly], index: Option<MapIndex<'a>>) -> Self {
        let n_index = match index {
            Some(_) => known.len().min(map.edges.len()),
            None => 0,
        };
        let from = known.len().min(map.edges.len());
        let made: Vec<EdgeReach> = (from..map.edges.len())
            .map(|e| edge_reach(map.quad(e)))
            .collect();
        let mut near = Self {
            map,
            known,
            index,
            n_index,
            made,
            local: CellGrid::new(),
            local_adj: IntMap::default(),
            loaded_tiles: Default::default(),
            loaded_edges: Default::default(),
            cell_edges: IntMap::default(),
            seen: Default::default(),
            buf: Vec::new(),
        };
        for e in n_index..map.edges.len() {
            let cells: Vec<(i64, i64)> = near.reach(e).1.to_vec();
            near.local.insert(e - n_index, cells);
            let ed = map.edges[e];
            near.local_adj.entry(ed.a).or_default().push(e);
            if ed.b != ed.a {
                near.local_adj.entry(ed.b).or_default().push(e);
            }
        }
        near
    }

    /// 辺 `e` の外接矩形とマス（`edge_reach`。前から `known.len()` 本は、計算に使う形が持つもの）
    fn reach(&self, e: usize) -> &EdgeReach {
        match self.known.get(e) {
            Some(g) => g.reach(self.map.quad(e)),
            None => &self.made[e - self.known.len()],
        }
    }

    /// 辺 `e` の外接矩形（格子に置き直す分と、近くとみなす距離の分を広げたもの：`reach_box`。マスは作らない）
    pub(super) fn bbox(&self, e: usize) -> KRect {
        reach_box(self.map.quad(e))
    }

    /// 辺 `e` がかかるマス（同じだけ広げたもの）
    pub(super) fn cells(&self, e: usize) -> &[(i64, i64)] {
        &self.reach(e).1
    }

    /// 索引の升目 `tile` の辺を読み、その辺のマスに入れる（マスの中は辺の番号の順にそろえる）
    fn load(&mut self, tile: (i64, i64)) {
        let Some(ix) = self.index else {
            return;
        };
        if !self.loaded_tiles.insert(tile) {
            return;
        }
        let mut found = std::mem::take(&mut self.buf);
        found.clear();
        ix.index.edges_in(ix.stored, tile, &mut found);
        let mut touched: Vec<(i64, i64)> = Vec::new();
        for &e in &found {
            if e >= self.n_index || !self.loaded_edges.insert(e) {
                continue;
            }
            // 索引を引く辺は、計算に使う形を持つ（`n_index` は `known.len()` 以下）
            let (known, map) = (self.known, self.map);
            for &c in known[e].reach(map.quad(e)).1.iter() {
                self.cell_edges.entry(c).or_default().push(e);
                touched.push(c);
            }
        }
        touched.sort_unstable();
        touched.dedup();
        for c in touched {
            if let Some(v) = self.cell_edges.get_mut(&c) {
                v.sort_unstable();
            }
        }
        self.buf = found;
    }

    /// マス `cells` のどれかにかかる辺（1つずつ。マスの順、マスの中は辺の番号の順：`CellGrid::query` と同じ）
    pub(super) fn query(
        &mut self,
        cells: impl IntoIterator<Item = (i64, i64)>,
        out: &mut Vec<usize>,
    ) {
        out.clear();
        self.seen.clear();
        let mut local_found: Vec<usize> = Vec::new();
        for c in cells {
            if self.index.is_some() {
                self.load(index_tile(c));
                if let Some(list) = self.cell_edges.get(&c) {
                    for &e in list {
                        if self.seen.insert(e) {
                            out.push(e);
                        }
                    }
                }
            }
            local_found.clear();
            local_found.extend(self.local.at(c).map(|i| i as usize + self.n_index));
            for &e in &local_found {
                if self.seen.insert(e) {
                    out.push(e);
                }
            }
        }
    }

    /// 頂点 `v` につながる辺（辺の番号の順）を `out` に入れる（前の中身は消す）。前からある辺は、端の升目の索引で頂点の
    /// 座標の所の辺を引き、端が `v` のものだけ残す（重ねる地図の頂点なら、前からある辺の端ではないので残らない）
    pub(super) fn edges_at(&self, v: usize, out: &mut Vec<usize>) {
        out.clear();
        if let Some(ix) = self.index {
            let p = self.map.nodes[v];
            ix.ends.edges_in(ix.stored, end_key(p), out);
            out.retain(|&e| {
                e < self.n_index && {
                    let ed = self.map.edges[e];
                    ed.a == v || ed.b == v
                }
            });
            out.sort_unstable();
            out.dedup();
        }
        if let Some(list) = self.local_adj.get(&v) {
            out.extend(list.iter().copied());
        }
    }
}

/// 地図 `m` の辺 `e` を、端の頂点を `ends` に置いた曲線の判定の形（`out` に入れる。前の中身は消す）。端が動いて
/// いなければ（`m` の頂点と同じ点）、判定の形は辺の曲線だけで決まるので、前から `known.len()` 本の辺は、計算に使う写しが
/// 持つもの（`grid_poly` で作ったもの）を写す
fn known_or_made(
    known: &[Poly],
    m: &PlanarMap,
    e: usize,
    ends: (Point, Point),
    out: &mut Vec<IPt>,
) {
    let ed = m.edges[e];
    match known.get(e) {
        Some(g) if ends == (m.nodes[ed.a], m.nodes[ed.b]) => {
            out.clear();
            out.extend_from_slice(g);
        }
        _ => grid_poly_into(&QuadBez::new(ends.0, ed.ctrl, ends.1), out),
    }
}

impl FedEdges<'_> {
    /// 線 `line`（曲線 `q`）を折れ線にした点と t（入れ物 `buf` の中）。同じ線として扱う所は、その所の点（`stitched`）
    pub(super) fn points<'a>(
        &self,
        line: Line,
        q: &QuadBez,
        buf: &'a mut PointsBuf,
    ) -> &'a mut Vec<(Point, f64)> {
        match self.spans.get(&line) {
            Some(spans) => {
                buf.own.clear();
                buf.own.extend(flat_points(*q, FLAT_TOL));
                stitched(&buf.own, spans, &mut buf.out);
            }
            None => {
                buf.out.clear();
                buf.out.extend(flat_points(*q, FLAT_TOL));
            }
        }
        &mut buf.out
    }

    /// 線 `line` の判定の形（格子に置いた折れ線）を `out` に入れる（前の中身は消す。`buf` は折れ線の点の入れ物）
    pub(super) fn grid_points(
        &self,
        line: Line,
        q: &QuadBez,
        buf: &mut PointsBuf,
        out: &mut Vec<IPt>,
    ) {
        if !self.spans.contains_key(&line) {
            grid_poly_into(q, out);
            return;
        }
        let pts = self.points(line, q, buf);
        grid_line_into(pts.iter().map(|&(p, _)| p), out);
    }
}

/// 区間の色をたどるときに、頂点で次に進む先
#[derive(Clone, Copy, Debug)]
pub(super) enum Turn {
    /// 区間（番号, 区間の向きに出ていくか）
    Piece(usize, bool),
    /// 入れなかった辺。たどるのはここで止める
    Stop,
}

/// `overlay_with` が止まった理由
pub(super) enum OverlayStop {
    /// 入れなかった辺（番号）が、切った区間と交わった。入れて切り直す
    Unfed(Vec<usize>),
    /// 格子に置けない座標がある
    Refused,
}

/// 同じ曲線から来た、左右の色が同じ区間の鎖
pub(super) struct Chain {
    /// 鎖の向きでの区間：（始点, 終点, 出どころの曲線の t の始め, t の終わり）
    steps: Vec<(usize, usize, f64, f64)>,
    /// 左右の印（色と図形の番号）
    left: Label,
    right: Label,
    /// 出どころの曲線
    curve: QuadBez,
    /// 入れた辺が丸ごと残ったとき：元の制御点
    whole: Option<Point>,
    /// 丸ごと残った辺が、同じ向き・同じ端のまま（曲線がビットまで同じ）で、計算に使う写しがその判定の形を持つとき：その形
    whole_poly: Option<Poly>,
}

/// 直線の区間の鎖を、元の曲線の一部へ置き換える。置き換えてよいのは、今の地図で、曲線 C の判定の形が次の3つを
/// 満たすときだけ（`PolyIndex::can_replace`。設計 段階2 の 2-4）：
/// 1. C が、自分とも、ほかの辺とも、共有する端のほかで触れない
/// 2. 区間の列 P と C で囲まれた所（帯。境目の上を含む）に、ほかの頂点が無い
/// 3. 帯の両端の頂点につながるほかの辺が、帯の中に無い（その辺は P とも C とも触れないので、最初の区間の真ん中の点1つで
///    決まる）
///
/// 満たさなければ鎖を真ん中で2つに分け、区間1つでも満たさなければ直線のまま残す
pub(super) struct ChainRestorer {
    map: PlanarMap,
    /// 置き換える曲線と帯がすべて入る矩形（`None` なら鎖が無い）。索引に入れる区間・頂点は、これにかかるものだけでよい
    active: Option<KRect>,
    /// 判定の形の索引。初めて要るとき（判定の形が変わる置き換えを調べるとき）に、その時の地図から作る
    index: Option<PolyIndex>,
    /// 辺ごとの、使っているか（`map.edges` の順。索引を作る前の置き換えの分も持つ）
    alive: Vec<bool>,
    /// 使い回す入れ物（調べる鎖の範囲・途中の頂点・道の点・曲線の判定の形）
    spans: Vec<(usize, usize)>,
    inner: Vec<usize>,
    path: Vec<IPt>,
    pc: Vec<IPt>,
}

impl ChainRestorer {
    /// `active`：置き換える曲線と帯がすべて入る矩形（`None` なら鎖が無い）。調べる相手（索引に入れる区間・頂点）は、
    /// これにかかるものだけでよい
    pub(super) fn new(map: PlanarMap, active: Option<KRect>) -> Self {
        Self {
            alive: vec![true; map.edges.len()],
            map,
            active,
            index: None,
            spans: Vec::new(),
            inner: Vec::new(),
            path: Vec::new(),
            pc: Vec::new(),
        }
    }

    /// 鎖 `c`（区間の辺の番号 `ids`）を、決まりを満たす所だけ曲線にする
    pub(super) fn restore(&mut self, c: &Chain, ids: &[usize]) {
        let mut spans = std::mem::take(&mut self.spans);
        spans.clear();
        spans.push((0, ids.len()));
        while let Some((i, j)) = spans.pop() {
            if self.try_replace(c, ids, i, j) || j - i == 1 {
                continue;
            }
            let mid = (i + j) / 2;
            spans.push((mid, j));
            spans.push((i, mid));
        }
        self.spans = spans;
    }

    /// 鎖 `c` の区間 `i..j` を1本の曲線に置き換えられれば置き換える（判定の形で、設計 2-4 の3つの決まりを調べる）
    pub(super) fn try_replace(&mut self, c: &Chain, ids: &[usize], i: usize, j: usize) -> bool {
        let (na, nb) = (c.steps[i].0, c.steps[j - 1].1);
        let (ta, tb) = (c.steps[i].2, c.steps[j - 1].3);
        if na == nb || ta == tb {
            return false;
        }
        let Some(area) = self.active else {
            return false;
        };
        let whole = (i == 0 && j == ids.len()).then_some(c.whole).flatten();
        let ctrl = match whole {
            Some(ctrl) => ctrl,
            None if ta < tb => c.curve.subsegment(ta..tb).p1,
            None => c.curve.subsegment(tb..ta).p1,
        };
        let nodes = &self.map.nodes;
        let q = QuadBez::new(nodes[na], ctrl, nodes[nb]);
        let part = &ids[i..j];
        self.inner.clear();
        self.inner.extend(c.steps[i + 1..j].iter().map(|s| s.0));
        self.path.clear();
        self.path.push(gpt(nodes[na]));
        self.path
            .extend(c.steps[i..j].iter().map(|s| gpt(nodes[s.1])));
        // 丸ごと残った辺がビットまで同じ曲線なら、その判定の形（計算に使う写しが持つもの）を使う
        match (whole, &c.whole_poly) {
            (Some(_), Some(g)) => {
                self.pc.clear();
                self.pc.extend_from_slice(g);
            }
            _ => grid_poly_into(&q, &mut self.pc),
        }
        // 曲線の判定の形が鎖の道（区間の頂点の並び）とまったく同じなら、置き換えても判定の形は何も変わらない。鎖の区間は
        // i_overlay が切った区間で、共有する端のほかで触れ合わず、途中の頂点はほかの辺につながらない（次数 2 でつなげた）
        // ので、3つの決まりはもう満たされている。調べるのは、判定の形が変わる置き換えだけ（索引もそのときに作る）
        if self.pc != self.path {
            let (nodes, edges, alive) = (&self.map.nodes, &self.map.edges, &self.alive);
            let index = self
                .index
                .get_or_insert_with(|| PolyIndex::build(nodes, edges, |f| alive[f], Some(area)));
            if !index.can_replace(part, &self.path, &self.inner, &self.pc, na, nb) {
                return false;
            }
        }
        for &f in part {
            self.alive[f] = false;
            if let Some(index) = self.index.as_mut() {
                index.kill(f);
            }
        }
        self.map.edges.push(MapEdge {
            a: na,
            b: nb,
            ctrl,
            left: c.left.0,
            right: c.right.0,
            left_src: c.left.1,
            right_src: c.right.1,
        });
        self.alive.push(true);
        if let Some(index) = self.index.as_mut() {
            index.add_edge(na, nb, self.pc.iter().copied());
        }
        true
    }

    /// 置き換え終わった地図（使わなくなった辺を除く）と、その辺の判定の形の索引・辺の番号 → 索引の番号（索引を作らな
    /// かったら `None`。続く `merge_degree2_indexed` が使い、無ければ要るときに作る：答えは索引の範囲によらない）
    pub(super) fn finish(self) -> (PlanarMap, Option<(PolyIndex, Vec<usize>)>) {
        let mut map = self.map;
        // 地図の辺と、`alive`・索引の辺は、同じ番号で1つずつ足してきた
        let mut k = 0;
        map.edges.retain(|_| {
            k += 1;
            self.alive[k - 1]
        });
        let index = self.index.map(|index| {
            let ids = (0..self.alive.len()).filter(|&f| self.alive[f]).collect();
            (index, ids)
        });
        (map, index)
    }
}

// ---- 判定の形：格子の上の折れ線（設計 段階2） ----
//
// 地図のつながり（頂点のまわりの辺の順番）・外周か穴か・辺の交わりは、各辺の曲線を重ね合わせと同じ折れ線にし、
// 1/4096px の格子に丸めたもので、整数で決める。曲線は絵のためだけに使う。
