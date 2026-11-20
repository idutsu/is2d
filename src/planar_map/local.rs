//! 新しい地図を「前の地図＋差分」として見て、触った所だけを計算する道具（設計：
//! claude/触った所だけを計算する地図の設計_22fbbbe.md の「段2b の設計」4.・7.）。
//!
//! 窓で重ねた結果（`overlay_windowed`）の差分（消えた前の地図の辺・増えた結果の辺）を、前の地図に当てた地図
//! （`LocalMap`）。前の地図全体の写し（通し番号の一覧）を作らずに、辺は塊から引き（`StoredMap::edge_at`）、頂点は座標で
//! 見分ける。頂点のまわりの辺の順と輪郭は、まわりの辺が変わった頂点では差分を当てた辺の順、ほかの頂点では前の地図の辺の順
//! （升目の索引で頂点の辺を引き、判定の形は塊から引く）。
//!
//! 辺の番号：前の地図の辺は前の地図の計算に使う地図での番号（`0..n_base`）、増えた辺は `n_base + i`。頂点の番号：座標ごとに
//! 初めて見たときに振る（この地図の中だけの番号。座標が同じなら同じ頂点）。

use super::*;

/// 頂点の番号の控え：座標ごとに番号を振る
#[derive(Default)]
struct Nodes {
    of: HashMap<[u64; 2], usize>,
    at: Vec<Point>,
}

impl Nodes {
    fn id(&mut self, p: Point) -> usize {
        let at = &mut self.at;
        *self
            .of
            .entry([p.x.to_bits(), p.y.to_bits()])
            .or_insert_with(|| {
                at.push(p);
                at.len() - 1
            })
    }
}

/// 前の地図に差分を当てた地図
pub(crate) struct LocalMap<'a> {
    index: MapIndex<'a>,
    /// 前の地図の辺の数
    n_base: usize,
    /// 消えた前の地図の辺
    removed: std::collections::HashSet<usize>,
    /// 増えた辺（頂点はこの地図の番号）と、その判定の形
    added: Vec<MapEdge>,
    added_polys: Vec<Poly>,
    /// 頂点ごとの、増えた辺（辺の番号の順）
    added_at: HashMap<usize, Vec<usize>>,
    nodes: std::cell::RefCell<Nodes>,
}

impl<'a> LocalMap<'a> {
    /// 前の地図（しまう形と升目の索引 `index`）に、窓で重ねた結果 `result` の差分（`removed`：消えた前の地図の辺、
    /// `added`：増えた結果の辺の番号）を当てた地図。結果の頂点は、座標が同じなら前の地図の頂点（`with_window_result` と
    /// 同じ決まり）
    pub(crate) fn new(
        index: MapIndex<'a>,
        result: &PlanarMap,
        removed: &[usize],
        added: &[usize],
    ) -> Self {
        let n_base = index.stored.edge_count();
        let mut nodes = Nodes::default();
        let mut list: Vec<MapEdge> = Vec::with_capacity(added.len());
        let mut polys: Vec<Poly> = Vec::with_capacity(added.len());
        let mut added_at: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, &r) in added.iter().enumerate() {
            let ed = result.edges[r];
            let (a, b) = (nodes.id(result.nodes[ed.a]), nodes.id(result.nodes[ed.b]));
            list.push(MapEdge { a, b, ..ed });
            polys.push(EdgeGeom::of(&result.quad(r)));
            added_at.entry(a).or_default().push(n_base + i);
            if b != a {
                added_at.entry(b).or_default().push(n_base + i);
            }
        }
        Self {
            index,
            n_base,
            removed: removed.iter().copied().collect(),
            added: list,
            added_polys: polys,
            added_at,
            nodes: std::cell::RefCell::new(nodes),
        }
    }

    fn n_base(&self) -> usize {
        self.n_base
    }

    /// 前の地図の辺 `e`（塊から引く）
    fn base_edge(&self, e: usize) -> crate::stored_map::TileEdge {
        self.index.stored.edge_at(e)
    }

    /// 点 `p` の頂点の番号
    fn node_id(&self, p: Point) -> usize {
        self.nodes.borrow_mut().id(p)
    }

    /// 辺 `e`
    pub(crate) fn edge(&self, e: usize) -> MapEdge {
        match e.checked_sub(self.n_base()) {
            Some(i) => self.added[i],
            None => {
                let te = self.base_edge(e);
                MapEdge {
                    a: self.node_id(te.a),
                    b: self.node_id(te.b),
                    ctrl: te.ctrl,
                    left: te.left,
                    right: te.right,
                    left_src: te.left_src,
                    right_src: te.right_src,
                }
            }
        }
    }

    /// 頂点 `v` の位置
    pub(crate) fn node(&self, v: usize) -> Point {
        self.nodes.borrow().at[v]
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
        match e.checked_sub(self.n_base()) {
            Some(_) => {
                let ed = self.edge(e);
                QuadBez::new(self.node(ed.a), ed.ctrl, self.node(ed.b))
            }
            None => self.base_edge(e).quad(),
        }
    }

    /// 増えた辺の番号（`n_base + i`）
    pub(crate) fn added_edges(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.added.len()).map(|i| self.n_base() + i)
    }

    /// まわりの辺が変わった頂点：消えた辺と増えた辺の端
    pub(crate) fn dirty_nodes(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for &e in &self.removed {
            let te = self.base_edge(e);
            out.push(self.node_id(te.a));
            out.push(self.node_id(te.b));
        }
        out.extend(self.added.iter().flat_map(|ed| [ed.a, ed.b]));
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 頂点 `v` を端に持つ前の地図の辺（消えた辺も。番号の順）
    fn base_edges_at(&self, v: usize) -> Vec<usize> {
        let p = self.node(v);
        let mut out: Vec<usize> = Vec::new();
        self.index
            .ends
            .edges_in(self.index.stored, end_key(p), &mut out);
        out.retain(|&e| {
            let te = self.base_edge(e);
            te.a == p || te.b == p
        });
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 頂点 `v` につながる辺（番号の順）
    pub(crate) fn edges_at(&self, v: usize) -> Vec<usize> {
        let mut out = self.base_edges_at(v);
        out.retain(|e| !self.removed.contains(e));
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

    #[cfg(test)]
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
            .map(|&e| quad_key(&self.base_edge(e).quad()))
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
        // 増えた辺の端の頂点が、ほかの頂点と同じ格子の点に丸まらないか（前の地図の頂点どうしは別々の格子の点。座標が
        // 同じなら同じ頂点）
        let mut new_nodes: Vec<usize> = self.added.iter().flat_map(|ed| [ed.a, ed.b]).collect();
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
    pub(crate) fn touched_cycles_ok(
        &self,
        memo: &mut CycleMemo,
    ) -> Option<Vec<Vec<(usize, bool)>>> {
        // 触った輪郭：増えた辺の向きのある辺と、まわりの辺が変わった頂点から出ていく向きのある辺の、左の面の輪郭（控え
        // `memo` に入れ、確かめ・面作りで使い回す。たどる決まりは `touched_cycles` と同じ）
        let mut starts: Vec<(usize, bool)> = self
            .added_edges()
            .flat_map(|e| [(e, true), (e, false)])
            .collect();
        for v in self.dirty_nodes() {
            let list = memo.around.entry(v).or_insert_with(|| self.out_sorted(v));
            starts.extend(list.iter().copied());
        }
        let mut ids: Vec<usize> = Vec::new();
        for h in starts {
            let ci = self.cycle_of(h, memo)?;
            if !ids.contains(&ci) {
                ids.push(ci);
            }
        }
        let cycles: Vec<Vec<(usize, bool)>> = ids
            .iter()
            .map(|&ci| memo.cycles[ci].cycle.clone())
            .collect();
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
    /// 何も無い側の輪郭の輪（バケツは、色の無い囲まれた所も面にする：`ring_any`）
    unpainted_rings: HashMap<usize, Ring>,
}

/// 点から右へ伸ばした半直線が横切る辺を、列の順に少しずつ引く道具（`LocalMap::ray_cols`・`next_ray_col`）
struct RayCols {
    p2: IPt,
    /// 次に調べる列と、最後の列
    next: i64,
    last: i64,
    /// 調べる升目の行（下, 上）
    rows: (i64, i64),
    /// 増えた辺の、横切る列ごとの並び
    added: std::collections::BTreeMap<i64, Vec<usize>>,
}

/// 半直線が横切る辺（`LocalMap::base_facing`）：横切る所, 上へ向けた区間の向き, 点の側の向きのある辺
type Facing = (f64, (i64, i64), (usize, bool));

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
    /// 少しずつ引く道具（`RayCols`）。半直線を横切る区間は、横切る所を含むマスの升目にあるので、列ごとに升目の索引の行を
    /// 引けばよい
    fn ray_cols(&self, p2: IPt) -> RayCols {
        let col_of = |x2: f64| ((x2 / 2.0) / (GRID_CELL * 4) as f64).floor() as i64;
        let (x, y) = (p2.0.div_euclid(2), p2.1.div_euclid(2));
        let (t0, ty0) = index_tile(cell_of((x - 1, y - 1)));
        let ty1 = index_tile(cell_of((x - 1, y + 2))).1;
        // 増えた辺は少ないので、横切る列をはじめに求めておく
        let mut added: std::collections::BTreeMap<i64, Vec<usize>> = Default::default();
        for e in self.added_edges() {
            for c in self.crossing_cols(e, p2, col_of) {
                let list = added.entry(c).or_default();
                if !list.contains(&e) {
                    list.push(e);
                }
            }
        }
        let last = self
            .index
            .index
            .max_tile_x()
            .into_iter()
            .chain(added.keys().last().copied())
            .max()
            .unwrap_or(t0 - 1);
        RayCols {
            p2,
            next: t0,
            last,
            rows: (ty0, ty1),
            added,
        }
    }

    /// 辺 `e` の区間が、点 `p2` から右へ伸ばした半直線を横切る所の列
    fn crossing_cols(&self, e: usize, p2: IPt, col_of: impl Fn(f64) -> i64) -> Vec<i64> {
        let p = self.poly(e);
        let mut out = Vec::new();
        for s in p.windows(2) {
            let (a, b) = ((2 * s[0].0, 2 * s[0].1), (2 * s[1].0, 2 * s[1].1));
            if crossing(a, b, p2).is_some() {
                let t = (p2.1 - a.1) as f64 / (b.1 - a.1) as f64;
                out.push(col_of(a.0 as f64 + t * (b.0 - a.0) as f64));
            }
        }
        out
    }

    /// `RayCols` の次の列（列, その列で半直線を横切る辺）。列が尽きたら `None`
    fn next_ray_col(&self, rc: &mut RayCols) -> Option<(i64, Vec<usize>)> {
        let col_of = |x2: f64| ((x2 / 2.0) / (GRID_CELL * 4) as f64).floor() as i64;
        while rc.next <= rc.last {
            let tx = rc.next;
            rc.next += 1;
            let mut cands: Vec<usize> = Vec::new();
            for ty in rc.rows.0..=rc.rows.1 {
                self.index
                    .index
                    .edges_in(self.index.stored, (tx, ty), &mut cands);
            }
            cands.retain(|e| !self.removed.contains(e));
            cands.sort_unstable();
            cands.dedup();
            let mut list: Vec<usize> = cands
                .into_iter()
                .filter(|&e| self.crossing_cols(e, rc.p2, col_of).contains(&tx))
                .collect();
            list.extend(rc.added.remove(&tx).unwrap_or_default());
            if !list.is_empty() {
                return Some((tx, list));
            }
        }
        None
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
        let mut rc = self.ray_cols(probe);
        while let Some((col, edges)) = self.next_ray_col(&mut rc) {
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

    /// 何も無い側の向きのある辺 `h` の左の何も無い所が、色のある面に入り込んでいないか（今の確かめの決まり：最初の区間の
    /// 真ん中の点 q が、その辺を輪郭に持たない色のある面の中か、その面の輪郭の上にあれば、入り込んでいる。設計 段2b の
    /// 12.）。新しい地図の面は、触った面 `faces` と、前の地図にそのままある面に分かれる（`in_touched`：触った面に入る輪郭と
    /// 触った輪郭）。
    /// - 触った面：反対側の面を除く色のある面の中（外周の中で、どの穴の中でもない）か、外周・穴の上なら入り込んでいる
    /// - そのままの面：h が前の地図の辺なら、前の地図が正しいので入らない。h が増えた辺なら、q を含む前の地図の面を、q から
    ///   右へ伸ばした半直線が最初に横切る前の地図の辺の q の側で決め（`base_facing`）、その向きのある辺の輪郭がそのままで
    ///   触った面に入っていなければ、その面はそのままの面なので、色があれば入り込んでいる
    ///
    /// `None`：輪郭が閉じない
    fn unpainted_ok_in(
        &self,
        h: (usize, bool),
        faces: &[LocalFace],
        in_touched: &std::collections::HashSet<usize>,
        memo: &mut CycleMemo,
    ) -> Option<bool> {
        let q = self.probe(&[h]);
        let other = self.cycle_of((h.0, !h.1), memo)?;
        for f in faces {
            if f.cycles.contains(&other) {
                continue;
            }
            let outer = memo.cycles[f.cycles[0]].ring.as_ref()?;
            match outer.contains(q) {
                None => return Some(false),
                Some(false) => continue,
                Some(true) => {}
            }
            let mut inside = true;
            for &hc in &f.cycles[1..] {
                match memo.cycles[hc].ring.as_ref()?.contains(q) {
                    None => return Some(false),
                    Some(true) => inside = false,
                    Some(false) => {}
                }
            }
            if inside {
                return Some(false);
            }
        }
        if h.0 >= self.n_base() {
            if let Ok(Some(hf)) = self.base_facing(q) {
                if !self.removed.contains(&hf.0) {
                    let c = self.cycle_of(hf, memo)?;
                    if !in_touched.contains(&c) && self.side_color(hf).is_some() {
                        return Some(false);
                    }
                }
            }
        }
        Some(true)
    }

    /// 前の地図（消えた辺も）で、点 `q`（2倍の座標）から右へ伸ばした半直線が最初に横切る辺の、q の側の向きのある辺（点を
    /// 右上へごくわずかずらした所として数える：`crossing` と同じ。同じ頂点で横切る辺は、ずらした高さで先に横切るもの）。
    /// 前の地図は正しいので、q を含む前の地図の面は、この向きのある辺の左の面。横切る辺が無ければ `Ok(None)`（どの面にも
    /// 入らない）。q が前の地図の辺の上にあれば `Err(())`。横切る所の列の順に、列ごとに升目の索引を引く（手間は、最初に
    /// 横切る辺までの分）
    fn base_facing(&self, q: IPt) -> Result<Option<(usize, bool)>, ()> {
        let col_of = |x2: f64| ((x2 / 2.0) / (GRID_CELL * 4) as f64).floor() as i64;
        let (x, y) = (q.0.div_euclid(2), q.1.div_euclid(2));
        let (t0, ty0) = index_tile(cell_of((x - 1, y - 1)));
        let ty1 = index_tile(cell_of((x - 1, y + 2))).1;
        let last = self.index.index.max_tile_x().unwrap_or(t0 - 1);
        let stored = self.index.stored;
        let mut cands: Vec<usize> = Vec::new();
        for tx in t0..=last {
            cands.clear();
            for ty in ty0..=ty1 {
                self.index.index.edges_in(stored, (tx, ty), &mut cands);
            }
            cands.sort_unstable();
            cands.dedup();
            let mut best: Option<Facing> = None;
            for &e in &cands {
                for s in stored.poly(e).windows(2) {
                    let (a, b) = ((2 * s[0].0, 2 * s[0].1), (2 * s[1].0, 2 * s[1].1));
                    if on_seg(a, b, q) {
                        return Err(());
                    }
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
                                && (up.0 as i128) * (bu.1 as i128)
                                    < (bu.0 as i128) * (up.1 as i128))
                    });
                    if better {
                        best = Some((xx, up, side));
                    }
                }
            }
            if let Some((_, _, side)) = best {
                return Ok(Some(side));
            }
        }
        Ok(None)
    }

    /// 2b-5c：触った輪郭の、穴の入れ先と何も無い側の決まり（今の `regions_since` と、何も無い側の輪郭の確かめ）。
    /// - 触った色のある穴：入れ先が無ければ正しくない
    /// - 前の地図の穴で、入れ先の外周が触られたもの（前の地図の面の表 `base_faces` で、触った前の地図の外周の面の穴を
    ///   引く）：入れ先が無ければ正しくない
    /// - 触った何も無い側の輪郭と、新しい輪の外接矩形に辺がかかる前の地図の何も無い側の輪郭：色のある面に入り込めば
    ///   正しくない（調べる輪郭は今の決まりより多いが、どれも正しい地図で成り立つ決まりなので、答えは同じ）
    ///
    /// `touched`：触った輪郭（`touched_cycles_ok`）。`None`：輪郭が閉じない（テスト用。本番は面も受け取る `regions_faces`）
    #[cfg(test)]
    pub(crate) fn regions_ok(
        &self,
        touched: &[Vec<(usize, bool)>],
        base_faces: BaseFaces<'_>,
        memo: &mut CycleMemo,
    ) -> Option<bool> {
        Some(self.regions_faces(touched, base_faces, memo)?.is_some())
    }

    /// `regions_ok` と、正しければ触った面（`touched_faces`）。`Some(None)`：正しくない。`None`：輪郭が閉じないか、穴の
    /// 入れ先が無い
    pub(crate) fn regions_faces(
        &self,
        touched: &[Vec<(usize, bool)>],
        base_faces: BaseFaces<'_>,
        memo: &mut CycleMemo,
    ) -> Option<Option<Vec<LocalFace>>> {
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
                    return Some(None);
                }
            }
        }
        // 前の地図の穴で、入れ先の外周が触られたもの：触った前の地図の外周（消えた辺と、まわりの辺が変わった前の地図の
        // 頂点の、前の地図の向きのある辺の左の面）の面の、前の地図の輪郭のうち、新しい地図にそのままあるもの
        let mut old_faces: Vec<ShapeId> = Vec::new();
        for &e in &self.removed {
            let ed = self.base_edge(e);
            old_faces.extend(ed.left_src);
            old_faces.extend(ed.right_src);
        }
        for v in self.dirty_nodes() {
            for e in self.base_edges_at(v) {
                let ed = self.base_edge(e);
                old_faces.extend(ed.left_src);
                old_faces.extend(ed.right_src);
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
                        return Some(None);
                    }
                }
            }
        }
        // 何も無い側の決まり：触った何も無い側の輪郭（最初の辺の所で）と、新しい色のある輪の中に点がある、前の地図の何も
        // 無い側の向きのある辺（何も無い所が新しい色のある面の中に入ったなら、その所の辺は新しい輪の中にある）
        let mut to_check: Vec<(usize, bool)> = touched_ids
            .iter()
            .filter(|&&ci| memo.cycles[ci].color.is_none())
            .map(|&ci| memo.cycles[ci].cycle[0])
            .collect();
        let made: Vec<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| memo.cycles[ci].ring.is_some())
            .collect();
        for ci in made {
            let r = memo.cycles[ci].ring.as_ref().expect("色のある輪郭").rect;
            let rect = KRect::new(
                r.x0 as f64 / OVERLAY_GRID,
                r.y0 as f64 / OVERLAY_GRID,
                r.x1 as f64 / OVERLAY_GRID,
                r.y1 as f64 / OVERLAY_GRID,
            );
            for e in self.edges_over(rect) {
                for side in [true, false] {
                    let h = (e, side);
                    if self.side_color(h).is_some()
                        || memo.of.get(&h).is_some_and(|c| touched_ids.contains(c))
                    {
                        continue;
                    }
                    let inside = memo.cycles[ci]
                        .ring
                        .as_ref()
                        .is_some_and(|r| r.contains(self.probe(&[h])) == Some(true));
                    if inside && !to_check.contains(&h) {
                        to_check.push(h);
                    }
                }
            }
        }
        let faces = self.touched_faces(touched, base_faces, memo)?;
        // 触った面に入る輪郭と、触った輪郭（そのままの面でない輪郭）
        let in_touched: std::collections::HashSet<usize> = faces
            .iter()
            .flat_map(|f| f.cycles.iter().copied())
            .chain(touched_ids.iter().copied())
            .collect();
        for h in to_check {
            if !self.unpainted_ok_in(h, &faces, &in_touched, memo)? {
                return Some(None);
            }
        }
        Some(Some(faces))
    }
}

impl LocalMap<'_> {
    /// 輪郭 `ci` の輪（何も無い側の輪郭も。今の `regions` の `unpainted` と同じ）
    fn ring_any<'m>(&self, ci: usize, memo: &'m mut CycleMemo) -> &'m Ring {
        if memo.cycles[ci].ring.is_some() {
            return memo.cycles[ci].ring.as_ref().expect("色のある輪郭");
        }
        if !memo.unpainted_rings.contains_key(&ci) {
            let r = self.ring_of(&memo.cycles[ci].cycle);
            memo.unpainted_rings.insert(ci, r);
        }
        &memo.unpainted_rings[&ci]
    }

    /// 点 `p2`（2倍の座標）が辺の判定の形の上にあるか（輪郭の上の点は、どの面か決まらない）。点の上を通る辺は、点を含む
    /// マスの升目にある
    fn on_some_edge(&self, p2: IPt) -> bool {
        let g = (p2.0.div_euclid(2), p2.1.div_euclid(2));
        let mut cands: Vec<usize> = Vec::new();
        self.index
            .index
            .edges_in(self.index.stored, index_tile(cell_of(g)), &mut cands);
        cands.retain(|e| !self.removed.contains(e));
        cands.extend(self.added_edges());
        cands.iter().any(|&e| {
            self.poly(e)
                .windows(2)
                .any(|s| on_seg((2 * s[0].0, 2 * s[0].1), (2 * s[1].0, 2 * s[1].1), p2))
        })
    }

    /// 色 `color`（何も無い色も）の、点 `probe` を含むいちばん小さい外周（何も無い側の輪郭も数える。`owner_of` と同じ
    /// 決め方）。`None`：輪郭が閉じない
    fn owner_any(
        &self,
        color: Option<Option<Color32>>,
        probe: IPt,
        memo: &mut CycleMemo,
    ) -> Option<Option<usize>> {
        let mut best: Option<(i128, usize)> = None;
        let mut found_col: Option<i64> = None;
        let mut rc = self.ray_cols(probe);
        while let Some((col, edges)) = self.next_ray_col(&mut rc) {
            if found_col.is_some_and(|fc| col > fc + 1) {
                break;
            }
            for e in edges {
                for side in [true, false] {
                    let ci = self.cycle_of((e, side), memo)?;
                    if color.is_some_and(|c| memo.cycles[ci].color != c) {
                        continue;
                    }
                    let r = self.ring_any(ci, memo);
                    if r.area2 > 0
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

    /// バケツの面（今の `face_filled_with` の、点を含む組。何も無い側の囲まれた所も組）：点 `p2`（2倍の座標）を含む
    /// いちばん小さい外周と、その穴（同じ色で、入れ先がその外周の輪郭）の、向きのある辺。色のある面の穴は、前の地図の面の
    /// 表 `base_faces` で引く。何も無い所の穴は、外周の外接矩形にかかる何も無い側の輪郭から探す。点が輪郭の上にあるか、
    /// どの外周にも入らなければ `None`。（面の色, 面の輪郭の向きのある辺）
    pub(crate) fn face_at(
        &self,
        p2: IPt,
        base_faces: BaseFaces<'_>,
        memo: &mut CycleMemo,
    ) -> Option<FaceAt> {
        if self.on_some_edge(p2) {
            return None;
        }
        let ext = self.owner_any(None, p2, memo)??;
        let color = memo.cycles[ext].color;
        let mut cycles = vec![ext];
        match color {
            Some(_) => {
                let h0 = memo.cycles[ext].cycle[0];
                let id = self.side_src(h0)?;
                for h in base_faces(id) {
                    let ci = self.cycle_of(h, memo)?;
                    if !cycles.contains(&ci) {
                        cycles.push(ci);
                    }
                }
            }
            None => {
                let r = self.ring_any(ext, memo).rect;
                let rect = KRect::new(
                    r.x0 as f64 / OVERLAY_GRID,
                    r.y0 as f64 / OVERLAY_GRID,
                    r.x1 as f64 / OVERLAY_GRID,
                    r.y1 as f64 / OVERLAY_GRID,
                );
                for e in self.edges_over(rect) {
                    for side in [true, false] {
                        if self.side_color((e, side)).is_some() {
                            continue;
                        }
                        let ci = self.cycle_of((e, side), memo)?;
                        if cycles.contains(&ci) || self.ring_any(ci, memo).area2 >= 0 {
                            continue;
                        }
                        let hp = self.probe(&memo.cycles[ci].cycle);
                        if self.owner_any(Some(None), hp, memo)? == Some(ext) {
                            cycles.push(ci);
                        }
                    }
                }
            }
        }
        // 点が穴の中なら、その面ではない（正しい地図では、穴の中の所は、より内側の外周に入る）
        for &h in &cycles[1..] {
            if self.ring_any(h, memo).contains(p2) != Some(false) {
                return None;
            }
        }
        let halves = cycles
            .iter()
            .flat_map(|&ci| memo.cycles[ci].cycle.iter().copied())
            .collect();
        Some((color, halves))
    }
}

/// バケツの面（`LocalMap::face_at`）：面の色と、面の輪郭の向きのある辺
pub(crate) type FaceAt = (Option<Color32>, Vec<(usize, bool)>);

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
            let ed = self.base_edge(e);
            old_faces.extend(ed.left_src);
            old_faces.extend(ed.right_src);
        }
        for v in self.dirty_nodes() {
            for e in self.base_edges_at(v) {
                let ed = self.base_edge(e);
                old_faces.extend(ed.left_src);
                old_faces.extend(ed.right_src);
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

impl LocalMap<'_> {
    /// 触った面 `faces` の辺の側に、面ごとの図形の番号 `ids` を記した辺（今の `labeled` と同じ：面の辺の側にはその面の
    /// 番号、面の無い側には無し）。（番号が変わる前の地図の辺, 足す辺）：前の地図の辺で側の番号が変わるものは、消して、
    /// 番号を付け替えた辺を足す。増えた辺は、みな番号を付け替えて足す
    pub(crate) fn relabeled_edges(
        &self,
        faces: &[LocalFace],
        ids: &[ShapeId],
        memo: &CycleMemo,
    ) -> (Vec<usize>, Vec<crate::stored_map::TileEdge>) {
        let mut sides: HashMap<usize, (Option<ShapeId>, Option<ShapeId>)> = HashMap::new();
        for e in self.added_edges() {
            sides.insert(e, (None, None));
        }
        for (f, &id) in faces.iter().zip(ids) {
            for &ci in &f.cycles {
                for &(e, forward) in &memo.cycles[ci].cycle {
                    let ed = self.edge(e);
                    let s = sides.entry(e).or_insert((ed.left_src, ed.right_src));
                    if forward {
                        s.0 = Some(id);
                    } else {
                        s.1 = Some(id);
                    }
                }
            }
        }
        let mut changed_base: Vec<usize> = Vec::new();
        let mut add: Vec<crate::stored_map::TileEdge> = Vec::new();
        let mut keys: Vec<usize> = sides.keys().copied().collect();
        keys.sort_unstable();
        for e in keys {
            let (l, r) = sides[&e];
            let ed = self.edge(e);
            if e < self.n_base() {
                if (ed.left_src, ed.right_src) == (l, r) {
                    continue;
                }
                changed_base.push(e);
            }
            let q = self.quad(e);
            add.push(crate::stored_map::TileEdge {
                a: q.p0,
                b: q.p2,
                ctrl: q.p1,
                left: ed.left,
                right: ed.right,
                left_src: l,
                right_src: r,
            });
        }
        (changed_base, add)
    }
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
        orig_shape: &dyn Fn(ShapeId) -> Option<Arc<FillShape>>,
        memo: &CycleMemo,
        layer_id: u64,
        group_path: &[u64],
    ) -> LocalChange {
        // 形が変わりうる前の地図の面：消えた辺と、まわりの辺が変わった頂点の、前の地図の辺の側の面と、触った面のうち外周が
        // 前の地図にそのままある面
        let mut old_faces: std::collections::BTreeSet<ShapeId> = Default::default();
        for &e in &self.removed {
            let ed = self.base_edge(e);
            old_faces.extend(ed.left_src);
            old_faces.extend(ed.right_src);
        }
        for v in self.dirty_nodes() {
            for e in self.base_edges_at(v) {
                let ed = self.base_edge(e);
                old_faces.extend(ed.left_src);
                old_faces.extend(ed.right_src);
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
            let ed = self.base_edge(h.0);
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
            .map(|&e| (quad_key(&self.base_edge(e).quad()), e))
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
                let ed = self.base_edge(e);
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
        // 形の変わらない輪郭（触っていない穴など）は、元の図形の輪郭をそのまま使い回す（`rewrite_shapes` と同じ：輪郭の
        // 点の並びを共有する）
        let reuse = |cycle: &[(usize, bool)], quads: &[QuadBez]| -> Option<Contour> {
            if !cycle.iter().all(|h| same(h.0)) {
                return None;
            }
            let shape = orig_shape(self.side_src(cycle[0])?)?;
            let found = shape
                .contours()
                .find(|c| {
                    c.segments.len() == quads.len()
                        && c.segments
                            .iter()
                            .position(|q| *q == quads[0])
                            .is_some_and(|k| {
                                (0..quads.len())
                                    .all(|i| c.segments[(k + i) % quads.len()] == quads[i])
                            })
                })
                .cloned();
            found
        };
        let added: Vec<FillShape> = changed
            .into_iter()
            .map(|i| {
                let f = &faces[i];
                let mut contours = f.cycles.iter().map(|&ci| {
                    let cycle = &memo.cycles[ci].cycle;
                    let quads: Vec<QuadBez> = cycle
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
                    reuse(cycle, &quads).unwrap_or_else(|| Contour::new(from_lowest_point(quads)))
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
#[cfg(test)]
pub(crate) type FaceShape = ([u8; 4], Vec<Vec<([u64; 6], bool)>>);

#[cfg(test)]
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
#[cfg(test)]
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
