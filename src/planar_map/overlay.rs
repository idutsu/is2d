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
/// 曲線を折れ線にするときの、曲線と折れ線の離れ具合の上限（px）
pub(super) const FLAT_TOL: f64 = 0.002;
/// i_overlay が交点を格子の点に丸めるときに、点が動く距離の上限（格子の目の対角線の半分、px）
pub(super) const GRID_REACH: f64 = std::f64::consts::FRAC_1_SQRT_2 / OVERLAY_GRID;

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

/// 区間の出どころの一覧をしまう所（i_overlay が区間を切る・まとめる・向きを変えるたびに増える）
#[derive(Default)]
pub(super) struct ProvStore(Vec<Vec<Prov>>);

impl ProvStore {
    pub(super) fn push(&mut self, v: Vec<Prov>) -> ProvId {
        self.0.push(v);
        ProvId(self.0.len() as u32 - 1)
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
        let v = store.0[self.0 as usize]
            .iter()
            .map(|p| Prov {
                src: p.src,
                t0: p.t1,
                t1: p.t0,
                fwd: !p.fwd,
            })
            .collect();
        store.push(v)
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
        let src = store.0[self.0 as usize].clone();
        let mid = |p: &Prov| p.t0 + (p.t1 - p.t0) * f;
        let first = src.iter().map(|p| Prov { t1: mid(p), ..*p }).collect();
        let second = src.iter().map(|p| Prov { t0: mid(p), ..*p }).collect();
        (store.push(first), store.push(second))
    }

    fn merge(
        ctx: i_overlay9::core::edge_data::EdgeDataMerge<
            i_overlay9::segm::boolean::ShapeCountBoolean,
            Self,
        >,
        store: &mut ProvStore,
    ) -> Self {
        let mut v = store.0[ctx.lhs_data.0 as usize].clone();
        v.extend(store.0[ctx.rhs_data.0 as usize].iter().copied());
        store.push(v)
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

/// 曲線を折れ線にする（`t` つき）。曲線と折れ線の離れ具合が `tol` 以下になる等分
pub(super) fn flatten_with_t(q: &QuadBez, tol: f64) -> Vec<(Point, f64)> {
    flat_points(*q, tol).collect()
}

/// 曲線を、折れ線とのずれが `tol` ほどになるように等分した点と t（`flatten_with_t`。等分の点の式はこれ1つ）
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
    /// 輪郭にも囲まれていない所・輪郭の上・すでに `color` の面）
    pub(crate) fn with_face_filled(&self, p: Point, color: Color32) -> Option<PlanarMap> {
        let polys = self.polys();
        let regions = self.regions(&polys, &self.face_cycles_with(&polys), true);
        // 点を含む組（正しい地図では、組どうしは重ならないので1つまで）。点が輪郭の上にあれば、どの面か決まらないので
        // 何も変えない
        let g = gpt(p);
        let p2 = (2 * g.0, 2 * g.1);
        let mut found = None;
        for gi in 0..regions.groups.len() {
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
            return Some(match rule {
                MapRule::KeepInside => PlanarMap::default(),
                _ => self.clone(),
            });
        }
        // 重ねる地図の辺も、前からある辺と同じ決まりで入れるかを決める（入れない辺はそのまま写し、色だけ決める）。そのために
        // 2つの地図の辺を1つに並べる（前からある辺が 0..n_old、重ねる地図の辺がその後ろ）。形と輪郭の変形の線はみな入れる
        let joined;
        let (cands, seeds): (&PlanarMap, &[QuadBez]) = match input {
            MapInput::Map(p) => {
                joined = self.joined(p);
                (&joined, &[])
            }
            _ => (self, &new_quads),
        };
        let check = self.edges.len()..cands.edges.len();
        let moved = input.moved_flags(cands.edges.len());
        let mut fed = cands.fed_edges(seeds, &moved, moved.clone(), check.clone())?;
        loop {
            match cands.overlay_with(input, rule, &new_quads, &new_cols, &mut fed) {
                Ok(m) => return Some(m),
                Err(OverlayStop::Unfed(more)) => {
                    let mut set = std::mem::take(&mut fed.fed);
                    for e in more {
                        set[e] = true;
                    }
                    fed = cands.fed_edges(seeds, &moved, set, check.clone())?;
                }
                Err(OverlayStop::Refused) => return None,
            }
        }
    }

    /// 2つの地図の辺と頂点を1つに並べたもの（`other` の頂点の番号は後ろへずらす）。正しい地図とは限らない（重ね合わせで
    /// 入れる辺を決めるときと、片を置く前の形を表すときの入れ物）
    pub(super) fn joined(&self, other: &PlanarMap) -> PlanarMap {
        let n = self.nodes.len();
        let nodes: Vec<Point> = self.nodes.iter().chain(&other.nodes).copied().collect();
        let edges = self
            .edges
            .iter()
            .copied()
            .chain(other.edges.iter().map(|e| MapEdge {
                a: e.a + n,
                b: e.b + n,
                ..*e
            }))
            .collect();
        PlanarMap { nodes, edges }
    }

    /// i_overlay に入れる辺（設計 2-1）。`fed` の辺と重ねる線 `seeds` の折れ線の近くを通る辺を足し、
    /// 足した辺の折れ線の近くを通る辺を足し…と、増えなくなるまで続ける。「近く」は格子の目の対角線の半分
    /// （`GRID_REACH`）以内：i_overlay は交点を格子の点に丸めるので、切った区間は折れ線からその距離まで動く（端へ寄せて
    /// それより遠くへ動いた所は、切ったあとに確かめて切り直す）。
    /// ただし、入れた辺と頂点を共有する辺は、その頂点でいつも触れているので、共有する頂点のほかで交わるかで見る。
    /// 入れた辺の端の頂点は格子に置き直すので、その頂点につながる入れない辺は、置き直した形で調べ直す
    /// （ほかの入れない辺と交わらないかも見る）。`check` の辺（重ねる地図の辺）も、はじめに同じく調べる：入れない辺と
    /// 触れれば入れる。`moved` は輪郭の変形で動かした辺（前の形は同じ線の比べ先にしない）。`None`：格子に置けない座標がある
    pub(super) fn fed_edges(
        &self,
        seeds: &[QuadBez],
        moved: &[bool],
        mut fed: Vec<bool>,
        check: std::ops::Range<usize>,
    ) -> Option<FedEdges> {
        let mut snapped = vec![false; self.nodes.len()];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        // 辺の外接矩形と、辺がかかるマス。端を格子に置き直すと曲線は最大で `GRID_REACH` 動き、判定の形の点は丸めでさらに
        // `GRID_REACH` 動くので、その分と、近くとみなす距離（`GRID_REACH`）の分を広げておく
        let boxes: Vec<KRect> = (0..self.edges.len())
            .map(|e| {
                let r = 3.0 * GRID_REACH;
                self.quad(e).bounding_box().inflate(r, r)
            })
            .collect();
        let reach_pad = (3.0 * GRID_REACH * OVERLAY_GRID).ceil() as i64;
        // 辺 `e` がかかるマスは `all_cells[cell_range[e]]`
        let mut all_cells: Vec<(i64, i64)> = Vec::new();
        let mut cell_range: Vec<std::ops::Range<usize>> = Vec::with_capacity(self.edges.len());
        let mut edge_grid = CellGrid::new();
        for (e, ed) in self.edges.iter().enumerate() {
            adj[ed.a].push(e);
            if ed.b != ed.a {
                adj[ed.b].push(e);
            }
            let start = all_cells.len();
            all_cells.extend(quad_cells(self.quad(e), reach_pad));
            cell_range.push(start..all_cells.len());
            edge_grid.insert(e, all_cells[start..].iter().copied());
        }
        let edge_cells = |e: usize| all_cells[cell_range[e].clone()].iter().copied();
        // 頂点 `v` を格子に置き直すと動くか
        let moves = |v: usize| grid_round(self.nodes[v]) != self.nodes[v];
        // 入れない辺 `e` の判定の形。端の頂点が格子に置き直されていれば、置き直した形で
        let poly_of = |e: usize, snapped: &[bool]| -> Vec<IPt> {
            let ed = self.edges[e];
            let pos = |v: usize| {
                if snapped[v] {
                    grid_round(self.nodes[v])
                } else {
                    self.nodes[v]
                }
            };
            grid_poly(&QuadBez::new(pos(ed.a), ed.ctrl, pos(ed.b)))
        };
        // 折れ線の区間：（始点, 終点, 始点の頂点, 終点の頂点）。頂点は、前からある辺の折れ線の両端だけ
        type GridSeg = (IPt, IPt, Option<usize>, Option<usize>);
        // 折れ線：（外接矩形, 区間の範囲）
        type GridLine = (KRect, std::ops::Range<usize>);
        // 折れ線の区間がかかるマス
        let line_cells = |segs: &[GridSeg]| -> Vec<(i64, i64)> {
            segs.iter().flat_map(|s| seg_cells((s.0, s.1), 0)).collect()
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
        let near_seg = |e: usize, pe: &[IPt], s: &GridSeg| -> bool {
            if !rects_overlap(boxes[e], seg_rect(s.0, s.1)) {
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
        let mut edge_queue: Vec<usize> = check.clone().collect();
        let mut to_feed: Vec<usize> = (0..self.edges.len()).filter(|&e| fed[e]).collect();
        // 同じ線として扱う所（`Span`）：重ねる線（形の線・輪郭の変形の線と、重ねる地図の辺）と、前からある辺（輪郭の変形で
        // 動かした辺は除く）が同じ線になっている所。その2本は入れ、その所は2本とも同じ点の折れ線にする。1本の線の上で
        // 重なる所どうしは重ならないようにする（先に見つけたもの）
        let flat_pad = (FLAT_TOL * OVERLAY_GRID).ceil() as i64;
        let mut spans: HashMap<Line, Vec<Span>> = HashMap::new();
        let new_lines: Vec<(Line, QuadBez)> = seeds
            .iter()
            .enumerate()
            .map(|(i, q)| (Line::Seed(i), *q))
            .chain(check.clone().map(|c| (Line::Edge(c), self.quad(c))))
            .collect();
        let free = |list: Option<&Vec<Span>>, lo: f64, hi: f64| {
            list.is_none_or(|v| v.iter().all(|sp| hi <= sp.lo || lo >= sp.hi))
        };
        for &(line, q) in &new_lines {
            // 格子に置けない座標（頂点と制御点：`fits_grid` と同じ）は、ここで断る
            on_grid([q.p0, q.p1, q.p2].into_iter()).then_some(())?;
            edge_grid.query(quad_cells(q, flat_pad), &mut found);
            let mut pairs: Vec<(usize, OverlapPts)> = found
                .iter()
                .copied()
                .filter(|&e| e < check.start && !moved[e])
                .filter_map(|e| Some((e, self.overlap(&q, e)?)))
                .collect();
            pairs.sort_by(|a, b| a.1[0].1.total_cmp(&b.1[0].1));
            for (e, pts) in pairs {
                let (s_lo, s_hi) = (pts[0].1, pts[pts.len() - 1].1);
                let (u_a, u_b) = (pts[0].2, pts[pts.len() - 1].2);
                let (u_lo, u_hi) = (u_a.min(u_b), u_a.max(u_b));
                if !free(spans.get(&line), s_lo, s_hi)
                    || !free(spans.get(&Line::Edge(e)), u_lo, u_hi)
                {
                    continue;
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
                to_feed.push(e);
                if let Line::Edge(c) = line {
                    to_feed.push(c);
                }
            }
        }
        let fed_set = FedEdges {
            fed: Vec::new(),
            spans,
            boxes: Vec::new(),
            grid: CellGrid::new(),
            adj: Vec::new(),
        };
        for (i, q) in seeds.iter().enumerate() {
            let line = fed_set.grid_points(Line::Seed(i), q);
            add_polyline(&line, (None, None), &mut segs, &mut seg_grid, &mut lines);
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
                let ed = self.edges[e];
                for v in [ed.a, ed.b] {
                    if !snapped[v] {
                        snapped[v] = true;
                        // 置き直して頂点が動いたときだけ、つながる入れない辺の形が変わる
                        if moves(v) {
                            edge_queue.extend(adj[v].iter().copied().filter(|&f| !fed[f]));
                        }
                    }
                }
                let q = self.quad(e);
                // 格子に置けない座標（頂点と制御点：`fits_grid` と同じ）は、ここで断る
                on_grid([q.p0, q.p1, q.p2].into_iter()).then_some(())?;
                add_polyline(
                    &fed_set.grid_points(Line::Edge(e), &q),
                    (Some(ed.a), Some(ed.b)),
                    &mut segs,
                    &mut seg_grid,
                    &mut lines,
                );
            }
            if let Some((r, range)) = lines.pop() {
                // 折れ線の区間のマスにかかる辺だけを、折れ線の区間のうち辺の外接矩形にかかるものと比べる
                edge_grid.query(line_cells(&segs[range.clone()]), &mut found);
                for &e in &found {
                    if fed[e] || !rects_overlap(boxes[e], r) {
                        continue;
                    }
                    let pe = poly_of(e, &snapped);
                    if segs[range.clone()].iter().any(|s| near_seg(e, &pe, s)) {
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
                let mut pe: Option<Vec<IPt>> = None;
                seg_grid.query(edge_cells(e), &mut found);
                let mut hit = found.iter().any(|&i| {
                    near_seg(e, pe.get_or_insert_with(|| poly_of(e, &snapped)), &segs[i])
                });
                if !hit {
                    // 端を置き直した形が、ほかの入れない辺と触れないか（共有する頂点は除く）
                    edge_grid.query(edge_cells(e), &mut found);
                    let given = as_given(e);
                    hit = found.iter().any(|&f| {
                        if f == e || fed[f] || (given && as_given(f)) {
                            return false;
                        }
                        let fd = self.edges[f];
                        let pf = poly_of(f, &snapped);
                        let pe = pe.get_or_insert_with(|| poly_of(e, &snapped));
                        polys_clash(pe, (ed.a, ed.b), &pf, (fd.a, fd.b))
                    });
                }
                if hit {
                    to_feed.push(e);
                }
            } else {
                return Some(FedEdges {
                    fed,
                    boxes,
                    grid: edge_grid,
                    adj,
                    ..fed_set
                });
            }
        }
    }

    /// `overlay` の1回分（入れる辺 `fed` を決めたあと）。入れなかった辺が切った区間と交わったら `Unfed`（その辺）
    /// `self` は前の地図（重ねる地図なら、その辺を後ろに並べたもの：`joined`）
    pub(super) fn overlay_with(
        &self,
        input: MapInput<'_>,
        rule: MapRule,
        new_quads: &[QuadBez],
        new_cols: &[(Option<Color32>, Option<Color32>)],
        fed_set: &mut FedEdges,
    ) -> Result<PlanarMap, OverlayStop> {
        let fed = std::mem::take(&mut fed_set.fed);
        let out = self.overlay_fed(input, rule, new_quads, new_cols, &fed, fed_set);
        fed_set.fed = fed;
        out
    }

    pub(super) fn overlay_fed(
        &self,
        input: MapInput<'_>,
        rule: MapRule,
        new_quads: &[QuadBez],
        new_cols: &[(Option<Color32>, Option<Color32>)],
        fed: &[bool],
        fed_set: &mut FedEdges,
    ) -> Result<PlanarMap, OverlayStop> {
        use i_overlay9::core::edge_overlay::{EdgeOverlay, InputEdge};
        use i_overlay9::core::fill_rule::FillRule as IFill;
        use i_overlay9::core::overlay::ShapeType;
        use i_overlay9::vector::edge::{CLIP_LEFT, CLIP_RIGHT};
        // 外が前のままのときだけ、重ねるものから離れた辺をそのまま残す
        let copy_untouched = rule != MapRule::KeepInside;
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
        for e in 0..self.edges.len() {
            if fed[e] {
                let src = if e < n_old {
                    OverlaySrc::Old(e)
                } else {
                    OverlaySrc::New(e - n_old)
                };
                srcs.push((self.quad(e), src));
            } else {
                interior[e] = e >= n_old || rects_overlap(fed_set.boxes[e], area);
            }
        }
        if !matches!(input, MapInput::Map(_)) {
            for (i, q) in new_quads.iter().enumerate() {
                srcs.push((*q, OverlaySrc::New(i)));
            }
        }

        // 1. 折れ線にして格子に置き、i_overlay で切る。格子は原点から（何回重ねても、格子の上の点は動かない）
        let mut ov: EdgeOverlay<i32, ProvId> = EdgeOverlay::new(srcs.len() * 8);
        for (si, (q, kind)) in srcs.iter().enumerate() {
            let st = if matches!(kind, OverlaySrc::Old(_)) {
                ShapeType::Subject
            } else {
                ShapeType::Clip
            };
            // 同じ線として扱う所は、2本とも同じ点の折れ線にする（`Span`）
            let line = match *kind {
                OverlaySrc::Old(e) => Line::Edge(e),
                OverlaySrc::New(i) if matches!(input, MapInput::Map(_)) => Line::Edge(n_old + i),
                OverlaySrc::New(i) => Line::Seed(i),
            };
            let pts = fed_set.points(line, q);
            let mut prev = (to_grid(pts[0].0).ok_or(OverlayStop::Refused)?, pts[0].1);
            for &(p, t) in &pts[1..] {
                let ip = to_grid(p).ok_or(OverlayStop::Refused)?;
                if ip == prev.0 {
                    continue;
                }
                // 点は t の順なので、区間（前の点→この点）は出どころの t の増える向き
                let id = ov.data_store_mut().push(vec![Prov {
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
            prov: Vec<Prov>,
        }
        let mut pieces: Vec<Piece> = Vec::with_capacity(out.len());
        for v in &out {
            let (ka, kb) = ((v.a.x, v.a.y), (v.b.x, v.b.y));
            // 同じ曲線が自分の上で折り返した所（同じ出どころで向きが逆の組）は打ち消す。何も残らなければ区間ごと捨てる
            let mut prov = store.0[v.data.0 as usize].clone();
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
            pieces.push(Piece {
                a,
                b,
                ia: (ka.0 as i64, ka.1 as i64),
                ib: (kb.0 as i64, kb.1 as i64),
                fill: v.fill,
                prov,
            });
        }
        // 入れなかった辺のうち結果に残るもの（中の辺と、外が前のままのときの離れた辺）が、切った区間と交わっていないか
        // （`fed_edges` は入れた辺の折れ線で調べたが、i_overlay は交点を格子に丸めるので、区間は折れ線から最大で半目ずれる）
        {
            let mut found = Vec::new();
            let mut unfed: Vec<usize> = Vec::new();
            let mut flagged = vec![false; self.edges.len()];
            for p in &pieces {
                let (a, b) = (nodes[p.a], nodes[p.b]);
                let r = KRect::from_points(a, b);
                fed_set.grid.query(seg_cells((p.ia, p.ib), 0), &mut found);
                for &e in &found {
                    if fed[e] || flagged[e] || !(interior[e] || copy_untouched) {
                        continue;
                    }
                    if !rects_overlap(fed_set.boxes[e], r) {
                        continue;
                    }
                    let ed = self.edges[e];
                    let pe = grid_poly(&QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]));
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
        // 3. 頂点ごとの、出ていく区間：（区間, 区間の向きに出ていくか）
        let mut around: IntMap<usize, Vec<(usize, bool)>> = IntMap::default();
        for (k, p) in pieces.iter().enumerate() {
            around.entry(p.a).or_default().push((k, true));
            around.entry(p.b).or_default().push((k, false));
        }
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
        let mut at_node: IntMap<usize, Vec<usize>> = IntMap::default();
        for (&v, &r) in &merged {
            at_node.entry(r).or_default().push(v);
        }
        let mut stops: IntMap<usize, Vec<(Turn, IPt)>> = IntMap::default();
        for &v in around.keys() {
            // 区間の頂点 `v` にある、元の頂点（`v` そのものと、`v` に1つにした頂点）
            let own = (v < self.nodes.len()).then_some(v);
            for u in own
                .into_iter()
                .chain(at_node.get(&v).into_iter().flatten().copied())
            {
                for &e in &fed_set.adj[u] {
                    if fed[e] {
                        continue;
                    }
                    let ed = self.edges[e];
                    let pe = grid_poly(&QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]));
                    for (end, at_start) in [(ed.a, true), (ed.b, false)] {
                        if end != u {
                            continue;
                        }
                        let mut it = half_points(std::slice::from_ref(&pe), (0, at_start));
                        let p0 = it.next().unwrap_or((0, 0));
                        let p1 = it.next().unwrap_or(p0);
                        stops
                            .entry(v)
                            .or_default()
                            .push((Turn::Stop(e, at_start), (p1.0 - p0.0, p1.1 - p0.1)));
                    }
                }
            }
        }
        // 向きのある区間の左の面を、次にたどる先（角度の順で、来た向きの逆のすぐ前）。番号は 2 × 区間 + 向き。
        // `old`：前の地図の色をたどるか（重ねる地図の色なら、重ねる地図の入れなかった辺で止める）
        let next_turns = |old: bool| {
            let mut next = vec![Turn::Piece(0, false); 2 * pieces.len()];
            for (&v, list) in &around {
                let mut turns: Vec<(Turn, IPt)> = list
                    .iter()
                    .map(|&(k, fwd)| (Turn::Piece(k, fwd), dir_of(&(k, fwd))))
                    .collect();
                if let Some(extra) = stops.get(&v) {
                    turns.extend(
                        extra
                            .iter()
                            .filter(|t| matches!(t.0, Turn::Stop(e, _) if (e < n_old) == old)),
                    );
                }
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
        // 区間ごとの、その地図（`old`：前の地図か）の出どころの辺の左右の印（区間の向きにそろえる）
        let deltas = |old: bool| -> Vec<Vec<(Label, Label)>> {
            pieces
                .iter()
                .map(|p| {
                    p.prov
                        .iter()
                        .filter_map(|pv| {
                            let lr = match srcs[pv.src as usize].1 {
                                OverlaySrc::Old(e) if old => edge_labels(e),
                                OverlaySrc::New(i) if !old => edge_labels(n_old + i),
                                _ => return None,
                            };
                            Some(if pv.fwd { lr } else { (lr.1, lr.0) })
                        })
                        .collect()
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
            let bands = y_bands(items.iter().map(|it| (2 * it.2.y0, 2 * it.2.y1)));
            (items, bands)
        };
        let rows = [std::cell::OnceCell::new(), std::cell::OnceCell::new()];
        // 区間と、その出どころの辺の左右の色（足し引きの分）。巻き数は色だけで数える（図形の番号は、面の輪郭の辺ごとに
        // 違うことがある（2つの面がまとまった面など）ので、足し引きには入れない）
        let segs_of = |d: &[Vec<(Label, Label)>]| {
            LabeledSegs::new(
                pieces
                    .iter()
                    .zip(d)
                    .flat_map(|(p, ls)| ls.iter().map(move |&(l, r)| (p.ia, p.ib, l.0, r.0))),
            )
        };
        let old_segs = segs_of(&old_deltas);
        let new_segs = segs_of(&new_deltas);
        // その地図の、点 `p2`（2倍の座標）での色ごとの巻き数：区間の出どころの辺の色と、半直線の高さにかかるその地図の
        // 入れなかった辺の判定の形で、整数で数える（点の状態を数えるのはこれ1つ：面の種と、中の辺のもう片方の地図の色）
        let winding_at = |segs: &LabeledSegs<Option<Color32>>, old: bool, p2: IPt| {
            let mut w: Vec<(Option<Color32>, i32)> = Vec::new();
            segs.winding_into(p2, &mut w);
            let (items, (y0, band_h, bands)) = rows[old as usize].get_or_init(|| unfed_rows(old));
            for &i in band_at(*y0, *band_h, bands, p2.1) {
                let (e, q, r) = &items[i as usize];
                if ray_meets_row(*r, p2) {
                    let ed = self.edges[*e];
                    for s in grid_poly(q).windows(2) {
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
                         d: &[Vec<(Label, Label)>],
                         old: bool,
                         next: &dyn Fn((usize, bool)) -> Turn| {
            let (face, states) = propagate(
                &piece_ends,
                next,
                &|p2| winding_at(segs, old, p2),
                &|w, k, from_left| {
                    let colors: Vec<Sides> = d[k].iter().map(|&(l, r)| (l.0, r.0)).collect();
                    crossed(&colors, w, from_left)
                },
            );
            let colors: Vec<Option<Color32>> =
                states.iter().map(|w| top_label(w).flatten()).collect();
            let mut votes: Vec<Vec<(ShapeId, usize)>> = vec![Vec::new(); states.len()];
            for (h, &f) in face.iter().enumerate() {
                let f = f as usize;
                for &(l, r) in &d[h / 2] {
                    let side = if h % 2 == 1 { l } else { r };
                    if let (true, Some(id)) = (side.0 == colors[f], side.1) {
                        match votes[f].iter_mut().find(|v| v.0 == id) {
                            Some(v) => v.1 += 1,
                            None => votes[f].push((id, 1)),
                        }
                    }
                }
            }
            let srcs: Vec<Option<ShapeId>> = votes
                .iter()
                .map(|v| {
                    v.iter()
                        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                        .map(|v| v.0)
                })
                .collect();
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
        let swept_labels: Vec<Vec<Sides>> = match input {
            MapInput::Sweep(_) => pieces
                .iter()
                .map(|p| {
                    p.prov
                        .iter()
                        .filter_map(|pv| {
                            let lr = match srcs[pv.src as usize].1 {
                                OverlaySrc::New(i) => new_cols[i],
                                OverlaySrc::Old(e) if moved[e] => {
                                    (self.edges[e].right, self.edges[e].left)
                                }
                                OverlaySrc::Old(_) => return None,
                            };
                            Some(if pv.fwd { lr } else { (lr.1, lr.0) })
                        })
                        .collect()
                })
                .collect(),
            _ => Vec::new(),
        };
        let swept_segs = LabeledSegs::new(
            pieces
                .iter()
                .zip(&swept_labels)
                .flat_map(|(p, ls)| ls.iter().map(move |&(l, r)| (p.ia, p.ib, l, r))),
        );
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
                &|w, k, from_left| crossed(&swept_labels[k], w, from_left),
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
                        p.prov
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
                        Over::Inside(piece.fill & if fwd { CLIP_LEFT } else { CLIP_RIGHT } != 0),
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
        // 6. 区間をつないで鎖にする。頂点の次数が 2 で、出どころと左右の色が同じで、t が続いていれば、つなげる
        let mut at: IntMap<usize, Vec<usize>> = IntMap::default();
        for (ki, &(a, b, ..)) in kept.iter().enumerate() {
            at.entry(a).or_default().push(ki);
            at.entry(b).or_default().push(ki);
        }
        // 中の辺（入れなかった辺のうち、重ねるものの範囲の中にある前からある辺と、重ねる地図の辺）：その所での、もう片方の
        // 地図の値（前からある辺なら重ねるものの値、重ねる地図の辺なら前の色）を、切った区間の印で整数で決めて、両側に
        // `rule` を当てる。左右が同じ色になれば消える。中の辺は区間にも、もう片方の地図の入れなかった辺にも触れないので、
        // 値は辺のどこでも同じで、区間の頂点でない頂点でつながる同じ地図の中の辺（組）でも同じ（組ごとに1回だけ調べる）
        let inner: Vec<usize> = (0..self.edges.len()).filter(|&e| interior[e]).collect();
        let mut group: Vec<u32> = (0..inner.len() as u32).collect();
        let mut first_at: HashMap<(bool, usize), usize> = HashMap::new();
        for (i, &e) in inner.iter().enumerate() {
            let ed = self.edges[e];
            for v in [node_at(ed.a), node_at(ed.b)] {
                if !around.contains_key(&v) {
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
        let mut interior_kept: Vec<MapEdge> = Vec::new();
        // 中の辺ごとの新しい色（残さない辺も）
        let mut inner_cols: IntMap<usize, Sides> = IntMap::default();
        for (i, &e) in inner.iter().enumerate() {
            let ed = self.edges[e];
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
        let mut refeed: Vec<usize> = Vec::new();
        for (&v, extra) in &stops {
            // 頂点から出ていく向きと、その左右の色（と、入れなかった辺ならその番号）
            let mut turns: Vec<(IPt, Sides, Option<usize>)> = around[&v]
                .iter()
                .map(|&(k, fwd)| {
                    let (l, r) = piece_cols[k];
                    (dir_of(&(k, fwd)), if fwd { (l, r) } else { (r, l) }, None)
                })
                .collect();
            for &(t, d) in extra {
                if let Turn::Stop(e, at_start) = t {
                    let (l, r) = unfed_cols(e);
                    turns.push((d, if at_start { (l, r) } else { (r, l) }, Some(e)));
                }
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
        if !refeed.is_empty() {
            refeed.sort_unstable();
            refeed.dedup();
            return Err(OverlayStop::Unfed(refeed));
        }
        let mut untouched_deg: IntMap<usize, usize> = IntMap::default();
        for (e, ed) in self.edges.iter().enumerate() {
            if !fed[e] && !interior[e] && copy_untouched {
                *untouched_deg.entry(ed.a).or_insert(0) += 1;
                *untouched_deg.entry(ed.b).or_insert(0) += 1;
            }
        }
        for ed in &interior_kept {
            *untouched_deg.entry(ed.a).or_insert(0) += 1;
            *untouched_deg.entry(ed.b).or_insert(0) += 1;
        }
        // 区間の主な出どころ：前からある辺、重ねる線、輪郭の変形で動かした辺（後の曲線に置き換わる）の順。区間の向き
        // （a→b）での t
        let main_src = |ki: usize| -> Prov {
            let rank = |p: &&Prov| match srcs[p.src as usize].1 {
                OverlaySrc::Old(e) if !moved[e] => 0,
                OverlaySrc::New(_) => 1,
                OverlaySrc::Old(_) => 2,
            };
            *pieces[kept[ki].3]
                .prov
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
        for start in 0..kept.len() {
            if used[start] {
                continue;
            }
            let mut chain: std::collections::VecDeque<(usize, bool)> =
                std::collections::VecDeque::from([(start, true)]);
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
                    let deg = at.get(&end).map_or(0, |v| v.len())
                        + untouched_deg.get(&end).copied().unwrap_or(0);
                    if deg != 2 {
                        break;
                    }
                    let Some(&nk) = at[&end].iter().find(|&&x| x != cur.0) else {
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
                }
            }
            chains.push(Chain {
                steps,
                left: sl,
                right: sr,
                curve: srcs[s_src as usize].0,
                whole,
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
        let map = PlanarMap { nodes, edges };
        let mut restorer = ChainRestorer::new(map, active);
        for (c, ids) in chains.iter().zip(&chain_edges) {
            restorer.restore(c, ids);
        }
        let mut m = restorer.finish();
        m.merge_degree2(dropped_ends);
        m.compact_nodes();
        Ok(m)
    }
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
        let m = self.quad(e);
        let mut ends: Vec<(f64, f64, Point)> = Vec::new();
        for (u, p) in [(0.0, m.p0), (1.0, m.p2)] {
            let (s, d) = nearest_on_quad(q, p);
            if d <= FLAT_TOL {
                ends.push((s, u, p));
            }
        }
        for (s, p) in [(0.0, q.p0), (1.0, q.p2)] {
            let (u, d) = nearest_on_quad(&m, p);
            if d <= FLAT_TOL {
                ends.push((s, u, p));
            }
        }
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
        if !same_curve(&q.subsegment(lo.0..hi.0), &along) {
            return None;
        }
        let mut pts = flatten_with_t(&sub, FLAT_TOL);
        if lo.1 > hi.1 {
            pts.reverse();
            for p in &mut pts {
                p.1 = 1.0 - p.1;
            }
        }
        let at = |t: f64| (lo.0 + (hi.0 - lo.0) * t, lo.1 + (hi.1 - lo.1) * t);
        let n = pts.len();
        Some(
            pts.into_iter()
                .enumerate()
                .map(|(k, (p, t))| match k {
                    0 => (lo.2, lo.0, lo.1),
                    _ if k + 1 == n => (hi.2, hi.0, hi.1),
                    _ => {
                        let (s, u) = at(t);
                        (p, s, u)
                    }
                })
                .collect(),
        )
    }
}

/// 2本の2次ベジェ `a`・`b`（同じ向きで、t が同じ所どうしを比べる）が、地図の精度で同じ曲線か：3組の制御点がみな
/// `FLAT_TOL` 以内。2本の差も2次ベジェで、曲線の点は制御点の重みつきの平均（重みの和は1）なので、どこでも2本の離れ具合は
/// 制御点の離れ具合以下になる。両側に同じ決まりがかかり、折れ線の細かさによらない（レビュー 4f4e953 の N-1001）。
/// 同じ曲線かの判定はこれ1つ（同じ線として扱う所・2本の辺のまとめ）
pub(super) fn same_curve(a: &QuadBez, b: &QuadBez) -> bool {
    [(a.p0, b.p0), (a.p1, b.p1), (a.p2, b.p2)]
        .iter()
        .all(|&(x, y)| (x - y).hypot() <= FLAT_TOL)
}

/// 同じ線になっている所の点：（点, 重ねる線の上の t, 辺の上の t）。重ねる線の上の t の順
pub(super) type OverlapPts = Vec<(Point, f64, f64)>;

/// 曲線を折れ線にした点 `own`（点と t）の、同じ線として扱う所 `spans`（重ならない。範囲の順）を、その所の点に替えたもの。
/// 両端はいつも自分の端の点で、続く点の t が同じなら後の点を落とす（区間の向きは t の大きさで決めるので、t の幅が 0 の
/// 区間を作らない）
pub(super) fn stitched(own: Vec<(Point, f64)>, spans: &[Span]) -> Vec<(Point, f64)> {
    if spans.is_empty() {
        return own;
    }
    let n = own.len();
    let mut out = vec![own[0]];
    let push = |out: &mut Vec<(Point, f64)>, p: (Point, f64)| {
        if out.last().is_none_or(|l| l.1 != p.1) {
            out.push(p);
        }
    };
    let mut k = 1;
    for sp in spans {
        while k < n - 1 && own[k].1 < sp.lo {
            push(&mut out, own[k]);
            k += 1;
        }
        for &p in &sp.pts {
            push(&mut out, p);
        }
        while k < n - 1 && own[k].1 <= sp.hi {
            k += 1;
        }
    }
    for &p in &own[k..n - 1] {
        push(&mut out, p);
    }
    let end = own[n - 1];
    match out.last_mut() {
        Some(l) if l.1 == end.1 => *l = end,
        _ => out.push(end),
    }
    out
}

/// i_overlay に入れる前からある辺（`fed`）と、そのときに作った辺の外接矩形・格子・頂点ごとの辺
pub(super) struct FedEdges {
    fed: Vec<bool>,
    /// 線ごとの、同じ線として扱う所（範囲の順）
    spans: HashMap<Line, Vec<Span>>,
    /// 辺の外接矩形（格子に置き直す分と、近くとみなす距離の分を広げたもの）
    boxes: Vec<KRect>,
    /// 辺がかかるマス（同じだけ広げたもの）
    grid: CellGrid,
    adj: Vec<Vec<usize>>,
}

impl FedEdges {
    /// 線 `line`（曲線 `q`）を折れ線にした点と t。同じ線として扱う所は、その所の点（`stitched`）
    pub(super) fn points(&self, line: Line, q: &QuadBez) -> Vec<(Point, f64)> {
        stitched(
            flatten_with_t(q, FLAT_TOL),
            self.spans.get(&line).map_or(&[][..], |v| v.as_slice()),
        )
    }

    /// 線 `line` の判定の形（格子に置いた折れ線）
    pub(super) fn grid_points(&self, line: Line, q: &QuadBez) -> Vec<IPt> {
        if !self.spans.contains_key(&line) {
            return grid_poly(q);
        }
        grid_line(self.points(line, q).into_iter().map(|(p, _)| p))
    }
}

/// 区間の色をたどるときに、頂点で次に進む先
#[derive(Clone, Copy, Debug)]
pub(super) enum Turn {
    /// 区間（番号, 区間の向きに出ていくか）
    Piece(usize, bool),
    /// 入れなかった前からある辺（番号, 頂点がその辺の始点か）。たどるのはここで止める
    Stop(usize, bool),
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
    /// 判定の形の索引（置き換える鎖が無ければ `None`）
    index: Option<PolyIndex>,
}

impl ChainRestorer {
    /// `active`：置き換える曲線と帯がすべて入る矩形（`None` なら鎖が無い）。調べる相手（索引に入れる区間・頂点）は、
    /// これにかかるものだけでよい
    pub(super) fn new(map: PlanarMap, active: Option<KRect>) -> Self {
        let index = active.map(|area| PolyIndex::new(&map, Some(area)));
        Self { map, index }
    }

    /// 鎖 `c`（区間の辺の番号 `ids`）を、決まりを満たす所だけ曲線にする
    pub(super) fn restore(&mut self, c: &Chain, ids: &[usize]) {
        let mut spans = vec![(0, ids.len())];
        while let Some((i, j)) = spans.pop() {
            if self.try_replace(c, ids, i, j) || j - i == 1 {
                continue;
            }
            let mid = (i + j) / 2;
            spans.push((mid, j));
            spans.push((i, mid));
        }
    }

    /// 鎖 `c` の区間 `i..j` を1本の曲線に置き換えられれば置き換える（判定の形で、設計 2-4 の3つの決まりを調べる）
    pub(super) fn try_replace(&mut self, c: &Chain, ids: &[usize], i: usize, j: usize) -> bool {
        let (na, nb) = (c.steps[i].0, c.steps[j - 1].1);
        let (ta, tb) = (c.steps[i].2, c.steps[j - 1].3);
        if na == nb || ta == tb {
            return false;
        }
        let Some(index) = self.index.as_mut() else {
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
        let inner: Vec<usize> = c.steps[i + 1..j].iter().map(|s| s.0).collect();
        let path: Vec<IPt> = std::iter::once(gpt(nodes[na]))
            .chain(c.steps[i..j].iter().map(|s| gpt(nodes[s.1])))
            .collect();
        let pc = grid_poly(&q);
        if !index.can_replace(part, &path, &inner, &pc, na, nb) {
            return false;
        }
        for &f in part {
            index.kill(f);
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
        index.add_edge(na, nb, pc);
        true
    }

    /// 置き換え終わった地図（使わなくなった辺を除く）
    pub(super) fn finish(self) -> PlanarMap {
        let mut map = self.map;
        if let Some(index) = self.index {
            let mut k = 0;
            map.edges.retain(|_| {
                k += 1;
                index.alive[k - 1]
            });
        }
        map
    }
}

// ---- 判定の形：格子の上の折れ線（設計 段階2） ----
//
// 地図のつながり（頂点のまわりの辺の順番）・外周か穴か・辺の交わりは、各辺の曲線を重ね合わせと同じ折れ線にし、
// 1/4096px の格子に丸めたもので、整数で決める。曲線は絵のためだけに使う。
