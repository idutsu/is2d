//! 新しい地図を「前の地図＋差分」として見て、触った所だけを計算する道具（設計：
//! claude/触った所だけを計算する地図の設計_22fbbbe.md の「段2b の設計」4.）。
//!
//! 窓で重ねた結果（`overlay_windowed`）の差分（消えた前の地図の辺・増えた結果の辺）を、前の地図に当てた地図
//! （`LocalMap`）。前の地図全体の写しを作らずに、頂点のまわりの辺の順と輪郭を引く：まわりの辺が変わった頂点では差分を
//! 当てた辺の順、ほかの頂点では前の地図の辺の順（升目の索引で頂点の辺を引き、判定の形は塊から引く）。
//!
//! 辺の番号：前の地図の辺は前の地図の番号（`0..n_base`）、増えた辺は `n_base + i`。頂点の番号：前の地図の頂点は前の
//! 地図の番号、増えた頂点は `n_base_nodes + j`。

use super::*;

/// 前の地図に差分を当てた地図
pub(crate) struct LocalMap<'a> {
    base: &'a PlanarMap,
    index: MapIndex<'a>,
    /// 消えた前の地図の辺
    removed: std::collections::HashSet<usize>,
    /// 増えた辺（頂点はこの地図の番号）と、その判定の形
    added: Vec<MapEdge>,
    added_polys: Vec<Poly>,
    /// 増えた頂点の位置（番号は `base.nodes.len() + j`）
    new_nodes: Vec<Point>,
    /// 頂点ごとの、増えた辺（辺の番号の順）
    added_at: HashMap<usize, Vec<usize>>,
}

impl<'a> LocalMap<'a> {
    /// 前の地図 `base`（升目の索引 `index`）の窓の頂点 `wnodes`（前の地図の番号）で重ねた結果 `result` の差分
    /// （`removed`：消えた前の地図の辺、`added`：増えた結果の辺の番号）を当てた地図。結果の頂点は、窓の頂点と同じ座標なら
    /// その頂点（`with_window_result` と同じ決まり）
    pub(crate) fn new(
        base: &'a PlanarMap,
        index: MapIndex<'a>,
        wnodes: &[usize],
        result: &PlanarMap,
        removed: &[usize],
        added: &[usize],
    ) -> Self {
        let at: IntMap<[u64; 2], usize> = wnodes
            .iter()
            .map(|&v| ([base.nodes[v].x.to_bits(), base.nodes[v].y.to_bits()], v))
            .collect();
        let mut new_nodes: Vec<Point> = Vec::new();
        let mut node_of: HashMap<usize, usize> = HashMap::new();
        let mut node = |r: usize, new_nodes: &mut Vec<Point>| -> usize {
            *node_of.entry(r).or_insert_with(|| {
                let p = result.nodes[r];
                match at.get(&[p.x.to_bits(), p.y.to_bits()]) {
                    Some(&v) => v,
                    None => {
                        new_nodes.push(p);
                        base.nodes.len() + new_nodes.len() - 1
                    }
                }
            })
        };
        let n_base = base.edges.len();
        let mut list: Vec<MapEdge> = Vec::with_capacity(added.len());
        let mut polys: Vec<Poly> = Vec::with_capacity(added.len());
        let mut added_at: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, &r) in added.iter().enumerate() {
            let ed = result.edges[r];
            let (a, b) = (node(ed.a, &mut new_nodes), node(ed.b, &mut new_nodes));
            list.push(MapEdge { a, b, ..ed });
            polys.push(EdgeGeom::of(&result.quad(r)));
            added_at.entry(a).or_default().push(n_base + i);
            if b != a {
                added_at.entry(b).or_default().push(n_base + i);
            }
        }
        Self {
            base,
            index,
            removed: removed.iter().copied().collect(),
            added: list,
            added_polys: polys,
            new_nodes,
            added_at,
        }
    }

    fn n_base(&self) -> usize {
        self.base.edges.len()
    }

    /// 辺 `e`
    pub(crate) fn edge(&self, e: usize) -> MapEdge {
        match e.checked_sub(self.n_base()) {
            Some(i) => self.added[i],
            None => self.base.edges[e],
        }
    }

    /// 頂点 `v` の位置
    pub(crate) fn node(&self, v: usize) -> Point {
        match v.checked_sub(self.base.nodes.len()) {
            Some(j) => self.new_nodes[j],
            None => self.base.nodes[v],
        }
    }

    /// 辺 `e` の判定の形
    pub(crate) fn poly(&self, e: usize) -> &Poly {
        match e.checked_sub(self.n_base()) {
            Some(i) => &self.added_polys[i],
            None => self.index.stored.poly(e),
        }
    }

    /// 辺 `e` の曲線
    pub(crate) fn quad(&self, e: usize) -> QuadBez {
        let ed = self.edge(e);
        QuadBez::new(self.node(ed.a), ed.ctrl, self.node(ed.b))
    }

    /// 増えた辺の番号（`n_base + i`）
    pub(crate) fn added_edges(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.added.len()).map(|i| self.n_base() + i)
    }

    /// まわりの辺が変わった頂点：消えた辺と増えた辺の端
    pub(crate) fn dirty_nodes(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .removed
            .iter()
            .flat_map(|&e| [self.base.edges[e].a, self.base.edges[e].b])
            .chain(self.added.iter().flat_map(|ed| [ed.a, ed.b]))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 頂点 `v` につながる辺（番号の順）
    pub(crate) fn edges_at(&self, v: usize) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        if v < self.base.nodes.len() {
            out = self.base.edges_at_in(v, Some(self.index));
            out.retain(|e| !self.removed.contains(e));
        }
        if let Some(list) = self.added_at.get(&v) {
            out.extend(list.iter().copied());
        }
        out
    }

    /// 頂点 `v` から出ていく向きのある辺を、角度の順に並べたもの（`face_cycles_with` と同じ並べ方）
    pub(crate) fn out_sorted(&self, v: usize) -> Vec<(usize, bool)> {
        let idx = |h: (usize, bool)| 2 * h.0 + h.1 as usize;
        let dir = |h: (usize, bool)| -> IPt {
            let p = self.poly(h.0);
            let n = p.len();
            let at = |k: usize| if h.1 { p[k] } else { p[n - 1 - k] };
            let a = if n > 0 { at(0) } else { (0, 0) };
            let b = if n > 1 { at(1) } else { a };
            (b.0 - a.0, b.1 - a.1)
        };
        let mut list: Vec<(usize, bool)> = Vec::new();
        for e in self.edges_at(v) {
            let ed = self.edge(e);
            if ed.a == v {
                list.push((e, true));
            }
            if ed.b == v {
                list.push((e, false));
            }
        }
        list.sort_by(|&x, &y| cmp_dir(wide(dir(x)), wide(dir(y))).then(idx(x).cmp(&idx(y))));
        list
    }

    /// 触った輪郭：増えた辺の向きのある辺と、まわりの辺が変わった頂点から出ていく向きのある辺の、左の面の輪郭（たどる
    /// 決まりは `face_cycles_with` と同じ）。ほかの輪郭は、前の地図の輪郭と同じ（通る頂点のまわりの辺の順が変わらず、
    /// 辺も同じ）。閉じない輪郭は `None`（壊れている）
    pub(crate) fn touched_cycles(&self) -> Option<Vec<Vec<(usize, bool)>>> {
        let mut around: HashMap<usize, Vec<(usize, bool)>> = HashMap::new();
        let mut starts: Vec<(usize, bool)> = self
            .added_edges()
            .flat_map(|e| [(e, true), (e, false)])
            .collect();
        for v in self.dirty_nodes() {
            let list = around.entry(v).or_insert_with(|| self.out_sorted(v));
            starts.extend(list.iter().copied());
        }
        let mut used: std::collections::HashSet<(usize, bool)> = Default::default();
        let mut cycles = Vec::new();
        for start in starts {
            if used.contains(&start) {
                continue;
            }
            let mut cycle = Vec::new();
            let mut cur = start;
            loop {
                used.insert(cur);
                cycle.push(cur);
                let ed = self.edge(cur.0);
                let head = if cur.1 { ed.b } else { ed.a };
                let list = around.entry(head).or_insert_with(|| self.out_sorted(head));
                let back = (cur.0, !cur.1);
                let k = list.iter().position(|&h| h == back)?;
                let next = list[(k + list.len() - 1) % list.len()];
                if next == start {
                    break;
                }
                if used.contains(&next) {
                    return None;
                }
                cur = next;
            }
            cycles.push(cycle);
        }
        Some(cycles)
    }

    /// 点 `p`（文書の座標）を含むマスの升目にかかる辺（消えた辺を除いた前の地図の辺と、増えた辺。番号の順・1つずつ）
    fn edges_near_point(&self, p: Point) -> Vec<usize> {
        self.edges_over(KRect::from_points(p, p))
    }

    /// 矩形 `r`（文書の座標）にかかる升目の辺（消えた辺を除いた前の地図の辺と、増えた辺。番号の順・1つずつ）
    fn edges_over(&self, r: KRect) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for t in index_tiles_over(r) {
            self.index.index.edges_in(self.index.stored, t, &mut out);
        }
        out.retain(|e| !self.removed.contains(e));
        out.extend(self.added_edges());
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 2b-5a：増えた辺の決まり（`checked_since` の、格子に置ける・両側の色が違う・判定の形が2点以上・区間どうしが触れない・
    /// 別々の頂点が別々の格子の点）。前の地図の辺どうしは、前の地図が正しいので調べない。区間どうしは、どちらかが、形が
    /// 前の地図に無い増えた辺のものだけを調べる（消えた辺と形が同じ増えた辺は、判定の形も同じ：`valid_faces_since` と同じ
    /// 考え）
    pub(crate) fn added_edges_ok(&self) -> bool {
        let base_keys: std::collections::HashSet<[u64; 6]> = self
            .removed
            .iter()
            .map(|&e| quad_key(&self.base.quad(e)))
            .collect();
        let mut fresh: std::collections::HashSet<usize> = Default::default();
        for e in self.added_edges() {
            let q = self.quad(e);
            let ed = self.edge(e);
            if !on_grid([q.p0, q.p1, q.p2].into_iter()) || ed.left == ed.right {
                return false;
            }
            if self.poly(e).len() < 2 {
                return false;
            }
            if !base_keys.contains(&quad_key(&q)) {
                fresh.insert(e);
            }
        }
        // 増えた頂点が、ほかの頂点と同じ格子の点に丸まらないか（前の地図の頂点どうしは別々の格子の点）
        let n_nodes = self.base.nodes.len();
        let mut new_nodes: Vec<usize> = self
            .added
            .iter()
            .flat_map(|ed| [ed.a, ed.b])
            .filter(|&v| v >= n_nodes)
            .collect();
        new_nodes.sort_unstable();
        new_nodes.dedup();
        for &v in &new_nodes {
            let p = self.node(v);
            let gp = gpt(p);
            for f in self.edges_near_point(p) {
                let fd = self.edge(f);
                for u in [fd.a, fd.b] {
                    if u != v && gpt(self.node(u)) == gp {
                        return false;
                    }
                }
            }
        }
        // 区間どうし
        let mut fresh_list: Vec<usize> = fresh.into_iter().collect();
        fresh_list.sort_unstable();
        for &e in &fresh_list {
            let pe = self.poly(e).clone();
            let ed = self.edge(e);
            let reach = GridRect::of(&pe);
            let rect = KRect::new(
                reach.x0 as f64 / OVERLAY_GRID,
                reach.y0 as f64 / OVERLAY_GRID,
                reach.x1 as f64 / OVERLAY_GRID,
                reach.y1 as f64 / OVERLAY_GRID,
            )
            .inflate(1.0 / OVERLAY_GRID, 1.0 / OVERLAY_GRID);
            for f in self.edges_over(rect) {
                let pf = self.poly(f).clone();
                let fd = self.edge(f);
                for k in 0..pe.len() - 1 {
                    let rk = GridRect::of(&pe[k..k + 2]);
                    for m in 0..pf.len().saturating_sub(1) {
                        if e == f && m <= k {
                            continue;
                        }
                        if !rk.overlaps(&GridRect::of(&pf[m..m + 2])) {
                            continue;
                        }
                        let joint = (e == f && m == k + 1).then_some(pe[k + 1]);
                        if seg_pair_clash(
                            (pe[k], pe[k + 1]),
                            seg_tags(&pe, k, ed.a, ed.b),
                            (pf[m], pf[m + 1]),
                            seg_tags(&pf, m, fd.a, fd.b),
                            joint,
                        ) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// 向きのある辺の左側の色
    pub(crate) fn side_color(&self, (e, forward): (usize, bool)) -> Option<Color32> {
        let ed = self.edge(e);
        if forward {
            ed.left
        } else {
            ed.right
        }
    }

    /// 2b-5b：触った輪郭が閉じていて、輪郭に沿って左側の色がそろうか（`is_broken_cycle` と同じ決まり）。そろえば触った
    /// 輪郭。ほかの輪郭は前の地図にそのままあり、前の地図は正しいので、閉じて色がそろう
    pub(crate) fn touched_cycles_ok(&self) -> Option<Vec<Vec<(usize, bool)>>> {
        let cycles = self.touched_cycles()?;
        cycles
            .iter()
            .all(|c| {
                c.windows(2)
                    .all(|w| self.side_color(w[0]) == self.side_color(w[1]))
            })
            .then_some(cycles)
    }

    /// 向きのある辺の、番号によらない鍵（形の鍵 `edge_key` と、辺の決まった向きに進むか）
    pub(crate) fn half_key(&self, (e, forward): (usize, bool)) -> ([u64; 6], bool) {
        let q = self.quad(e);
        let canonical = point_order(q.p0, q.p2).is_le();
        (quad_key(&q), forward == canonical)
    }
}

/// たどった輪郭（`LocalMap::cycle_of`）：向きのある辺の並び・左側の色・輪（色のある輪郭だけ：今の `regions` と同じく、
/// 何も無い側の輪郭は外周にも穴にもならない）
pub(crate) struct LocalCycle {
    pub(crate) cycle: Vec<(usize, bool)>,
    pub(crate) color: Option<Color32>,
    ring: Option<Ring>,
}

/// たどった輪郭の控え：輪郭と、向きのある辺ごとの輪郭の番号
#[derive(Default)]
pub(crate) struct CycleMemo {
    pub(crate) cycles: Vec<LocalCycle>,
    of: HashMap<(usize, bool), usize>,
    around: HashMap<usize, Vec<(usize, bool)>>,
}

/// 前の地図の面の表：面の番号（図形の番号）ごとの、その面の輪郭（外周と穴）の向きのある辺のどれか1つ（前の地図の番号）
pub(crate) type BaseFaces<'a> = &'a dyn Fn(ShapeId) -> Vec<(usize, bool)>;

impl LocalMap<'_> {
    /// 向きのある辺 `h` の左の面の輪郭（控え `memo` の番号）。たどる決まりは `face_cycles_with` と同じ。閉じなければ `None`
    pub(crate) fn cycle_of(&self, h: (usize, bool), memo: &mut CycleMemo) -> Option<usize> {
        if let Some(&ci) = memo.of.get(&h) {
            return Some(ci);
        }
        let mut cycle = Vec::new();
        let mut cur = h;
        loop {
            cycle.push(cur);
            let ed = self.edge(cur.0);
            let head = if cur.1 { ed.b } else { ed.a };
            let list = memo
                .around
                .entry(head)
                .or_insert_with(|| self.out_sorted(head));
            let back = (cur.0, !cur.1);
            let k = list.iter().position(|&x| x == back)?;
            let next = list[(k + list.len() - 1) % list.len()];
            if next == h {
                break;
            }
            if cycle.len() > 4 * (self.n_base() + self.added.len()) + 4 {
                return None;
            }
            cur = next;
        }
        let color = self.side_color(h);
        let ring = color.is_some().then(|| self.ring_of(&cycle));
        let ci = memo.cycles.len();
        for &x in &cycle {
            memo.of.insert(x, ci);
        }
        memo.cycles.push(LocalCycle { cycle, color, ring });
        Some(ci)
    }

    /// 輪郭 `cycle` の輪（`Ring::new` と同じ）
    fn ring_of(&self, cycle: &[(usize, bool)]) -> Ring {
        let polys: Vec<Poly> = cycle
            .iter()
            .map(|&(e, _)| Arc::clone(self.poly(e)))
            .collect();
        let local: Vec<(usize, bool)> = cycle
            .iter()
            .enumerate()
            .map(|(i, &(_, f))| (i, f))
            .collect();
        Ring::new(&polys, &local)
    }

    /// 輪郭の調べる点（最初の向きのある辺の、判定の形の最初の区間の真ん中。2倍の座標：`half_mid2` と同じ）
    fn probe(&self, cycle: &[(usize, bool)]) -> IPt {
        let (e, f) = cycle[0];
        let p = self.poly(e);
        let n = p.len();
        let at = |k: usize| if f { p[k] } else { p[n - 1 - k] };
        let a = if n > 0 { at(0) } else { (0, 0) };
        let b = if n > 1 { at(1) } else { a };
        (a.0 + b.0, a.1 + b.1)
    }

    /// 点 `p2`（2倍の座標）から右へ伸ばした半直線が横切る辺（`crossing` で数える区間がある辺）を、横切る所の升目の列の順に
    /// （列, その列で横切る辺）。半直線を横切る区間は、横切る所を含むマスの升目にあるので、升目の索引の行で引ける
    fn ray_columns(&self, p2: IPt) -> Vec<(i64, Vec<usize>)> {
        let col_of = |x2: f64| ((x2 / 2.0) / (GRID_CELL * 4) as f64).floor() as i64;
        let (x, y) = (p2.0.div_euclid(2), p2.1.div_euclid(2));
        let (t0, ty0) = index_tile(cell_of((x - 1, y - 1)));
        let ty1 = index_tile(cell_of((x - 1, y + 2))).1;
        let mut cands: Vec<usize> = Vec::new();
        if let Some(max_x) = self.index.index.max_tile_x() {
            for tx in t0..=max_x {
                for ty in ty0..=ty1 {
                    self.index
                        .index
                        .edges_in(self.index.stored, (tx, ty), &mut cands);
                }
            }
        }
        cands.retain(|e| !self.removed.contains(e));
        cands.extend(self.added_edges());
        cands.sort_unstable();
        cands.dedup();
        let mut cols: std::collections::BTreeMap<i64, Vec<usize>> = Default::default();
        for e in cands {
            let p = self.poly(e);
            for s in p.windows(2) {
                let (a, b) = ((2 * s[0].0, 2 * s[0].1), (2 * s[1].0, 2 * s[1].1));
                if crossing(a, b, p2).is_some() {
                    let t = (p2.1 - a.1) as f64 / (b.1 - a.1) as f64;
                    let x2 = a.0 as f64 + t * (b.0 - a.0) as f64;
                    let list = cols.entry(col_of(x2)).or_default();
                    if list.last() != Some(&e) {
                        list.push(e);
                    }
                }
            }
        }
        cols.into_iter().collect()
    }

    /// 色 `color` の穴（調べる点 `probe`）の入れ先：点を含む同じ色の外周のうち、いちばん小さいもの（今の `regions` と同じ
    /// 決まり）。外周は点を含めば、点から右へ伸ばした半直線と交わる。正しい地図では外周どうしは入れ子か離れているので、
    /// 点を含む外周のうちいちばん内側のものが、半直線と最初に交わる。そこで、横切る所の列の順に輪郭を調べ、見つかった列の
    /// 次の列まで調べていちばん小さいものにする（列の境目での丸めの分）。`None`：輪郭が閉じない。`Some(None)`：入れ先が無い
    fn owner_of(
        &self,
        color: Option<Color32>,
        probe: IPt,
        memo: &mut CycleMemo,
    ) -> Option<Option<usize>> {
        let mut best: Option<(i128, usize)> = None;
        let mut found_col: Option<i64> = None;
        for (col, edges) in self.ray_columns(probe) {
            if found_col.is_some_and(|fc| col > fc + 1) {
                break;
            }
            for e in edges {
                for side in [true, false] {
                    let ci = self.cycle_of((e, side), memo)?;
                    let c = &memo.cycles[ci];
                    let Some(r) = &c.ring else {
                        continue;
                    };
                    if r.area2 > 0
                        && c.color == color
                        && r.contains(probe) == Some(true)
                        && best.is_none_or(|b| r.area2 < b.0)
                    {
                        best = Some((r.area2, ci));
                        found_col.get_or_insert(col);
                    }
                }
            }
        }
        Some(best.map(|b| b.1))
    }

    /// 何も無い側の輪郭 `ci` が、色のある面に入り込んでいないか（今の確かめの決まり：最初の区間の真ん中の点が、その辺を
    /// 輪郭に持たない色のある面の中か、その面の輪郭の上にあれば、入り込んでいる）。点を含みうる面の外周と穴は、点から
    /// 右へ伸ばした半直線と交わるので、横切る輪郭を全部たどって調べる。`None`：輪郭が閉じない
    fn unpainted_ok(&self, ci: usize, memo: &mut CycleMemo) -> Option<bool> {
        let h = memo.cycles[ci].cycle[0];
        let q = self.probe(&memo.cycles[ci].cycle);
        // その辺を輪郭に持つ色のある面（反対側の面）の外周
        let other = self.cycle_of((h.0, !h.1), memo)?;
        let excluded = match &memo.cycles[other].ring {
            Some(r) if r.area2 > 0 => Some(other),
            Some(_) => {
                let (c, p) = (
                    memo.cycles[other].color,
                    self.probe(&memo.cycles[other].cycle),
                );
                self.owner_of(c, p, memo)?
            }
            None => None,
        };
        // 半直線が横切る輪郭
        let mut crossed: Vec<usize> = Vec::new();
        for (_, edges) in self.ray_columns(q) {
            for e in edges {
                for side in [true, false] {
                    let c = self.cycle_of((e, side), memo)?;
                    if !crossed.contains(&c) {
                        crossed.push(c);
                    }
                }
            }
        }
        for &ext in &crossed {
            let Some(er) = &memo.cycles[ext].ring else {
                continue;
            };
            if er.area2 <= 0 || Some(ext) == excluded {
                continue;
            }
            let inside_ext = er.contains(q);
            let ext_color = memo.cycles[ext].color;
            // `Regions::contains` と同じ：外周の上なら入り込み、外周の中で、どの穴の中でもなければ入り込み
            let mut inside = match inside_ext {
                None => return Some(false),
                Some(false) => continue,
                Some(true) => true,
            };
            for &hole in &crossed {
                let hr = match &memo.cycles[hole].ring {
                    Some(r) if r.area2 < 0 && memo.cycles[hole].color == ext_color => r,
                    _ => continue,
                };
                let at = hr.contains(q);
                if at == Some(false) {
                    continue;
                }
                let hp = self.probe(&memo.cycles[hole].cycle);
                if self.owner_of(ext_color, hp, memo)? != Some(ext) {
                    continue;
                }
                match at {
                    None => return Some(false),
                    _ => inside = false,
                }
            }
            if inside {
                return Some(false);
            }
        }
        Some(true)
    }

    /// 2b-5c：触った輪郭の、穴の入れ先と何も無い側の決まり（今の `regions_since` と、何も無い側の輪郭の確かめ）。
    /// - 触った色のある穴：入れ先が無ければ正しくない
    /// - 前の地図の穴で、入れ先の外周が触られたもの（前の地図の面の表 `base_faces` で、触った前の地図の外周の面の穴を
    ///   引く）：入れ先が無ければ正しくない
    /// - 触った何も無い側の輪郭と、新しい輪の外接矩形に辺がかかる前の地図の何も無い側の輪郭：色のある面に入り込めば
    ///   正しくない（調べる輪郭は今の決まりより多いが、どれも正しい地図で成り立つ決まりなので、答えは同じ）
    ///
    /// `touched`：触った輪郭（`touched_cycles_ok`）。`None`：輪郭が閉じない
    pub(crate) fn regions_ok(
        &self,
        touched: &[Vec<(usize, bool)>],
        base_faces: BaseFaces<'_>,
        memo: &mut CycleMemo,
    ) -> Option<bool> {
        let mut touched_ids: Vec<usize> = Vec::new();
        for c in touched {
            touched_ids.push(self.cycle_of(c[0], memo)?);
        }
        let is_touched = |ci: usize, ids: &[usize]| ids.contains(&ci);
        // 触った色のある穴
        for &ci in &touched_ids {
            let (color, hole) = match &memo.cycles[ci].ring {
                Some(r) => (memo.cycles[ci].color, r.area2 < 0),
                None => continue,
            };
            if hole {
                let p = self.probe(&memo.cycles[ci].cycle);
                if self.owner_of(color, p, memo)?.is_none() {
                    return Some(false);
                }
            }
        }
        // 前の地図の穴で、入れ先の外周が触られたもの：触った前の地図の外周（消えた辺と、まわりの辺が変わった前の地図の
        // 頂点の、前の地図の向きのある辺の左の面）の面の、前の地図の輪郭のうち、新しい地図にそのままあるもの
        let mut old_faces: Vec<ShapeId> = Vec::new();
        for &e in &self.removed {
            let ed = self.base.edges[e];
            old_faces.extend(ed.left_src);
            old_faces.extend(ed.right_src);
        }
        for v in self.dirty_nodes() {
            if v < self.base.nodes.len() {
                for e in self.base.edges_at_in(v, Some(self.index)) {
                    let ed = self.base.edges[e];
                    old_faces.extend(ed.left_src);
                    old_faces.extend(ed.right_src);
                }
            }
        }
        old_faces.sort_unstable();
        old_faces.dedup();
        for id in old_faces {
            for h in base_faces(id) {
                if self.removed.contains(&h.0) {
                    continue;
                }
                let ci = self.cycle_of(h, memo)?;
                if is_touched(ci, &touched_ids) {
                    continue;
                }
                let (color, hole) = match &memo.cycles[ci].ring {
                    Some(r) => (memo.cycles[ci].color, r.area2 < 0),
                    None => continue,
                };
                if hole {
                    let p = self.probe(&memo.cycles[ci].cycle);
                    if self.owner_of(color, p, memo)?.is_none() {
                        return Some(false);
                    }
                }
            }
        }
        // 何も無い側の輪郭：触ったものと、新しい輪の外接矩形に辺がかかる前の地図のもの
        let mut to_check: Vec<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| memo.cycles[ci].color.is_none())
            .collect();
        let made: Vec<GridRect> = touched_ids
            .iter()
            .filter_map(|&ci| memo.cycles[ci].ring.as_ref().map(|r| r.rect))
            .collect();
        for r in made {
            let rect = KRect::new(
                r.x0 as f64 / OVERLAY_GRID,
                r.y0 as f64 / OVERLAY_GRID,
                r.x1 as f64 / OVERLAY_GRID,
                r.y1 as f64 / OVERLAY_GRID,
            );
            for e in self.edges_over(rect) {
                for side in [true, false] {
                    if self.side_color((e, side)).is_none() {
                        let ci = self.cycle_of((e, side), memo)?;
                        if !to_check.contains(&ci) {
                            to_check.push(ci);
                        }
                    }
                }
            }
        }
        for ci in to_check {
            if !self.unpainted_ok(ci, memo)? {
                return Some(false);
            }
        }
        Some(true)
    }
}

/// 触った面（`LocalMap::touched_faces`）：色と、輪郭（控えの番号。最初が外周、残りが穴）
pub(crate) struct LocalFace {
    pub(crate) color: Color32,
    pub(crate) cycles: Vec<usize>,
}

impl LocalMap<'_> {
    /// 触った輪郭（`touched_cycles_ok`）にかかわる、形の変わりうる面（設計 段2b の 6. の 5.）：
    /// - 触った外周の面
    /// - 入れ先が決まり直る穴（触った穴・入れ先の外周が触られた前の地図の穴・触った外周の中に入った前の地図の穴）の
    ///   入れ先の面
    /// - 前の地図の面で、外周がそのままで穴が触られたもの
    ///
    /// 面の穴は、決まり直った穴と、外周がそのままの面なら前の地図の面の表 `base_faces` の、そのままの穴。正しい地図
    /// （`regions_ok` が正しいと答えたもの）にだけ使う。`None`：輪郭が閉じないか、穴の入れ先が無い
    pub(crate) fn touched_faces(
        &self,
        touched: &[Vec<(usize, bool)>],
        base_faces: BaseFaces<'_>,
        memo: &mut CycleMemo,
    ) -> Option<Vec<LocalFace>> {
        let mut touched_ids: Vec<usize> = Vec::new();
        for c in touched {
            touched_ids.push(self.cycle_of(c[0], memo)?);
        }
        let painted = |ci: usize, memo: &CycleMemo| memo.cycles[ci].ring.as_ref().map(|r| r.area2);
        // 面に入る外周（触った外周と、穴が変わる外周）
        let mut exteriors: std::collections::BTreeSet<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| painted(ci, memo).is_some_and(|a| a > 0))
            .collect();
        // 入れ先を決め直す穴
        let mut holes: Vec<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| painted(ci, memo).is_some_and(|a| a < 0))
            .collect();
        // 触った前の地図の面：外周がそのままなら穴が変わる面。そのままの穴は入れ先を決め直す
        let mut old_faces: Vec<ShapeId> = Vec::new();
        for &e in &self.removed {
            let ed = self.base.edges[e];
            old_faces.extend(ed.left_src);
            old_faces.extend(ed.right_src);
        }
        for v in self.dirty_nodes() {
            if v < self.base.nodes.len() {
                for e in self.base.edges_at_in(v, Some(self.index)) {
                    let ed = self.base.edges[e];
                    old_faces.extend(ed.left_src);
                    old_faces.extend(ed.right_src);
                }
            }
        }
        old_faces.sort_unstable();
        old_faces.dedup();
        for id in old_faces {
            for h in base_faces(id) {
                if self.removed.contains(&h.0) {
                    continue;
                }
                let ci = self.cycle_of(h, memo)?;
                if touched_ids.contains(&ci) {
                    continue;
                }
                match painted(ci, memo) {
                    Some(a) if a > 0 => {
                        exteriors.insert(ci);
                    }
                    Some(_) => holes.push(ci),
                    None => {}
                }
            }
        }
        // 触った外周の中に入った前の地図の穴（今の `regions_since` は、新しく作った同じ色の外周の外接矩形が調べる点を含む
        // 穴の入れ先を決め直す）
        let new_exteriors: Vec<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| painted(ci, memo).is_some_and(|a| a > 0))
            .collect();
        for ext in new_exteriors {
            let (color, r) = {
                let c = &memo.cycles[ext];
                (c.color, c.ring.as_ref().expect("色のある輪郭").rect)
            };
            let rect = KRect::new(
                r.x0 as f64 / OVERLAY_GRID,
                r.y0 as f64 / OVERLAY_GRID,
                r.x1 as f64 / OVERLAY_GRID,
                r.y1 as f64 / OVERLAY_GRID,
            );
            for e in self.edges_over(rect) {
                for side in [true, false] {
                    if self.side_color((e, side)) != color {
                        continue;
                    }
                    let ci = self.cycle_of((e, side), memo)?;
                    if touched_ids.contains(&ci) || painted(ci, memo).is_none_or(|a| a >= 0) {
                        continue;
                    }
                    if r.contains2(self.probe(&memo.cycles[ci].cycle)) {
                        holes.push(ci);
                    }
                }
            }
        }
        holes.sort_unstable();
        holes.dedup();
        // 穴の入れ先
        let mut placed: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut placed_holes: std::collections::HashSet<usize> = Default::default();
        for h in holes {
            let (color, p) = (memo.cycles[h].color, self.probe(&memo.cycles[h].cycle));
            let owner = self.owner_of(color, p, memo)??;
            exteriors.insert(owner);
            placed.entry(owner).or_default().push(h);
            placed_holes.insert(h);
        }
        // 面にする：外周がそのままの面は、前の地図の面の表のそのままの穴（入れ先を決め直した穴は除く）も入れる
        let mut faces = Vec::new();
        for ext in exteriors {
            let mut cycles = vec![ext];
            cycles.extend(placed.get(&ext).into_iter().flatten().copied());
            if !touched_ids.contains(&ext) {
                let h0 = memo.cycles[ext].cycle[0];
                let id = if h0.1 {
                    self.edge(h0.0).left_src
                } else {
                    self.edge(h0.0).right_src
                };
                if let Some(id) = id {
                    for h in base_faces(id) {
                        if self.removed.contains(&h.0) {
                            continue;
                        }
                        let ci = self.cycle_of(h, memo)?;
                        if ci != ext && !placed_holes.contains(&ci) && !touched_ids.contains(&ci) {
                            cycles.push(ci);
                        }
                    }
                }
            }
            let color = memo.cycles[ext].color.expect("色のある外周");
            faces.push(LocalFace { color, cycles });
        }
        Some(faces)
    }
}

/// 曲線の形の鍵（`edge_key` と同じ：両端を決まった順にし、制御点と合わせたビット）
pub(super) fn quad_key(q: &QuadBez) -> [u64; 6] {
    let (p0, p2) = if point_order(q.p0, q.p2).is_le() {
        (q.p0, q.p2)
    } else {
        (q.p2, q.p0)
    };
    [
        p0.x.to_bits(),
        p0.y.to_bits(),
        q.p1.x.to_bits(),
        q.p1.y.to_bits(),
        p2.x.to_bits(),
        p2.y.to_bits(),
    ]
}

/// 触った面の書き戻しの差分（`LocalMap::rewrite`）：消す元の図形、新しく入れる図形、触った面ごとの図形の番号
pub(crate) struct LocalChange {
    pub(crate) removed: Vec<ShapeId>,
    pub(crate) added: Vec<FillShape>,
    pub(crate) ids: Vec<ShapeId>,
}

impl LocalMap<'_> {
    /// 向きのある辺の左側の元の図形
    fn side_src(&self, (e, forward): (usize, bool)) -> Option<ShapeId> {
        let ed = self.edge(e);
        if forward {
            ed.left_src
        } else {
            ed.right_src
        }
    }

    /// 2b-6：触った面 `faces`（`touched_faces`）の書き戻しの差分。決まりは今の `rewrite_shapes` と同じ：
    /// - 面の辺がみな前の地図にあり（形がビットまで同じ）、辺の側の印が同じ1つの元の図形で、その図形と色も同じで、その
    ///   図形のどの辺も消えていない面は、元の図形のまま残す
    /// - ほかの面は新しい図形。辺の側の印のうち、残した面の図形でないものでいちばん多い元の図形を引き継ぐ（同じ元の図形を
    ///   引き継ぐ面が2つ以上あれば、その印の付いた側がいちばん多い面、同じなら形の鍵の小さい面）
    /// - 残した面の元の図形のほかは、みな消す
    ///
    /// 触っていない面は、前の地図の面のまま残る（その元の図形は、触った面では残す図形として数える）。前の地図の面のうち
    /// 形が変わりうるもの（`old_faces`）の元の図形だけが、消す候補になる
    pub(crate) fn rewrite(
        &self,
        faces: &[LocalFace],
        base_faces: BaseFaces<'_>,
        memo: &CycleMemo,
        layer_id: u64,
        group_path: &[u64],
    ) -> LocalChange {
        // 形が変わりうる前の地図の面：消えた辺と、まわりの辺が変わった頂点の、前の地図の辺の側の面と、触った面のうち外周が
        // 前の地図にそのままある面
        let mut old_faces: std::collections::BTreeSet<ShapeId> = Default::default();
        for &e in &self.removed {
            let ed = self.base.edges[e];
            old_faces.extend(ed.left_src);
            old_faces.extend(ed.right_src);
        }
        for v in self.dirty_nodes() {
            if v < self.base.nodes.len() {
                for e in self.base.edges_at_in(v, Some(self.index)) {
                    let ed = self.base.edges[e];
                    old_faces.extend(ed.left_src);
                    old_faces.extend(ed.right_src);
                }
            }
        }
        for f in faces {
            let h0 = memo.cycles[f.cycles[0]].cycle[0];
            if h0.0 < self.n_base() {
                old_faces.extend(self.side_src(h0));
            }
        }
        // 前の地図の図形の色
        let orig_color = |id: ShapeId| -> Option<Color32> {
            let h = *base_faces(id).first()?;
            let ed = self.base.edges[h.0];
            if h.1 {
                ed.left
            } else {
                ed.right
            }
        };
        // 触っていない面が残す元の図形：前の地図の面で、形が変わりえないもの
        let kept_untouched = |id: ShapeId| !old_faces.contains(&id) && orig_color(id).is_some();
        // 形が前の地図にある辺（増えた辺は、消えた辺と形が同じときだけ）
        let removed_keys: HashMap<[u64; 6], usize> = self
            .removed
            .iter()
            .map(|&e| (quad_key(&self.base.quad(e)), e))
            .collect();
        let added_keys: std::collections::HashSet<[u64; 6]> = self
            .added_edges()
            .map(|e| quad_key(&self.quad(e)))
            .collect();
        let same =
            |e: usize| e < self.n_base() || removed_keys.contains_key(&quad_key(&self.quad(e)));
        // 辺が消えた元の図形
        let mut cut: std::collections::HashSet<ShapeId> = Default::default();
        for (k, &e) in &removed_keys {
            if !added_keys.contains(k) {
                let ed = self.base.edges[e];
                cut.extend(ed.left_src);
                cut.extend(ed.right_src);
            }
        }
        let halves = |f: &LocalFace| -> Vec<(usize, bool)> {
            f.cycles
                .iter()
                .flat_map(|&ci| memo.cycles[ci].cycle.iter().copied())
                .collect()
        };
        let mut ids: Vec<ShapeId> = vec![0; faces.len()];
        let mut kept: std::collections::HashSet<ShapeId> = Default::default();
        let mut changed: Vec<usize> = Vec::new();
        for (i, f) in faces.iter().enumerate() {
            let hs = halves(f);
            let mut srcs: Vec<ShapeId> = hs.iter().filter_map(|&h| self.side_src(h)).collect();
            srcs.sort_unstable();
            srcs.dedup();
            let keep = hs.iter().all(|&(e, _)| same(e))
                && srcs.len() == 1
                && orig_color(srcs[0]) == Some(f.color)
                && !cut.contains(&srcs[0])
                && hs.iter().all(|&h| self.side_src(h) == Some(srcs[0]));
            if keep && !kept_untouched(srcs[0]) && kept.insert(srcs[0]) {
                ids[i] = srcs[0];
            } else {
                changed.push(i);
            }
        }
        let is_kept = |s: ShapeId| kept.contains(&s) || kept_untouched(s);
        // 引き継ぐ元の図形
        let face_key = |f: &LocalFace| hs_key(&halves(f), self);
        let mut heir: HashMap<ShapeId, (usize, usize)> = HashMap::new();
        let mut choice: Vec<Option<ShapeId>> = vec![None; faces.len()];
        for &i in &changed {
            let mut votes: Vec<(ShapeId, usize)> = Vec::new();
            for h in halves(&faces[i]) {
                if let Some(src) = self.side_src(h).filter(|&s| !is_kept(s)) {
                    add_vote(&mut votes, src);
                }
            }
            let Some((src, sides)) = top_vote(&votes) else {
                continue;
            };
            choice[i] = Some(src);
            let best = heir.entry(src).or_insert((sides, i));
            if best.1 != i
                && (sides > best.0
                    || (sides == best.0 && face_key(&faces[i]) < face_key(&faces[best.1])))
            {
                *best = (sides, i);
            }
        }
        let added: Vec<FillShape> = changed
            .into_iter()
            .map(|i| {
                let f = &faces[i];
                let mut contours = f.cycles.iter().map(|&ci| {
                    let quads: Vec<QuadBez> = memo.cycles[ci]
                        .cycle
                        .iter()
                        .map(|&(e, forward)| {
                            let q = self.quad(e);
                            if forward {
                                q
                            } else {
                                QuadBez::new(q.p2, q.p1, q.p0)
                            }
                        })
                        .collect();
                    Contour::new(from_lowest_point(quads))
                });
                let exterior = contours.next().unwrap_or_else(|| Contour::new(Vec::new()));
                let mut holes: Vec<Contour> = contours.collect();
                holes.sort_by(|a, b| starts_order(&a.segments, &b.segments));
                let mut s =
                    FillShape::new(f.color, exterior, holes).with_scope(layer_id, group_path);
                if let Some(src) = choice[i].filter(|src| heir.get(src).is_some_and(|h| h.1 == i)) {
                    s.shape_id = src;
                }
                ids[i] = s.shape_id;
                s
            })
            .collect();
        let removed: Vec<ShapeId> = old_faces
            .iter()
            .copied()
            .filter(|&id| orig_color(id).is_some() && !kept.contains(&id))
            .collect();
        LocalChange {
            removed,
            added,
            ids,
        }
    }
}

/// 面の向きのある辺の並び `hs` の、番号によらない鍵（`face_key` と同じ：向きのある辺の鍵のうちいちばん小さいもの）
fn hs_key(hs: &[(usize, bool)], m: &LocalMap<'_>) -> ([u64; 6], bool) {
    hs.iter()
        .map(|&h| m.half_key(h))
        .min()
        .expect("面の輪郭には辺がある")
}

/// 面の、番号によらない形（色と、輪郭の形の並び。並べたもの）。お手本と比べるため
pub(crate) type FaceShape = ([u8; 4], Vec<Vec<([u64; 6], bool)>>);

impl LocalMap<'_> {
    /// 触った面の、番号によらない形
    pub(crate) fn face_shape_key(&self, f: &LocalFace, memo: &CycleMemo) -> FaceShape {
        let mut cycles: Vec<_> = f
            .cycles
            .iter()
            .map(|&ci| cycle_shape(memo.cycles[ci].cycle.iter().map(|&h| self.half_key(h))))
            .collect();
        cycles.sort();
        (f.color.to_array(), cycles)
    }
}

/// 地図 `m` の全部の面の、番号によらない形（`faces` の面。並べたもの）
pub(crate) fn faces_shape_keys(m: &PlanarMap) -> Vec<FaceShape> {
    let (faces, _) = m.faces();
    let mut out: Vec<FaceShape> = faces
        .iter()
        .map(|f| {
            let mut cycles: Vec<_> = f
                .cycles
                .iter()
                .map(|c| {
                    cycle_shape(c.iter().map(|&(e, forward)| {
                        let q = m.quad(e);
                        (quad_key(&q), forward == point_order(q.p0, q.p2).is_le())
                    }))
                })
                .collect();
            cycles.sort();
            (f.color.to_array(), cycles)
        })
        .collect();
    out.sort();
    out
}

/// 輪郭を、番号によらない形にしたもの（向きのある辺の鍵の並び。いちばん小さい鍵から始める）。お手本と比べるため
#[cfg(test)]
pub(crate) fn cycle_shape(keys: impl Iterator<Item = ([u64; 6], bool)>) -> Vec<([u64; 6], bool)> {
    let mut c: Vec<([u64; 6], bool)> = keys.collect();
    if let Some(k) = (0..c.len()).min_by_key(|&k| c[k]) {
        c.rotate_left(k);
    }
    c
}
