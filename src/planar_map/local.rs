//! 新しい地図を「前の地図＋差分」として見て、触った所だけを計算する道具（設計：
//! claude/触った所だけを計算する地図の設計_22fbbbe.md の「段2b の設計」4.・7.）。
//!
//! 差分（消えた前の地図の辺・増えた辺。道具ごとに作り方が違う：窓で重ねる・バケツ・アンカー・片を置く・矩形で切る・面を
//! 外す）を前の地図に当てた地図（`LocalMap`）。1本の道の差分から先（`LayerBase::apply_diff`）が使う。前の地図全体の写し（通し番号の一覧）を作らずに、辺は塊から引き（`StoredMap::edge_at`）、頂点は座標で
//! 見分ける。頂点のまわりの辺の順と輪郭は、まわりの辺が変わった頂点では差分を当てた辺の順、ほかの頂点では前の地図の辺の順
//! （升目の索引で頂点の辺を引き、判定の形は塊から引く）。
//!
//! 辺の番号：前の地図の辺は前の地図の計算に使う地図での番号（`0..n_base`）、増えた辺は `n_base + i`。頂点の番号：座標ごとに
//! 初めて見たときに振る（この地図の中だけの番号。座標が同じなら同じ頂点）。

use super::*;

/// 頂点の番号の控え：座標ごとに番号を振る
#[derive(Default)]
struct Nodes {
    of: IntMap<[u64; 2], usize>,
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
    removed: IntSet<usize>,
    /// 増えた辺（頂点はこの地図の番号）と、その判定の形
    added: Vec<MapEdge>,
    added_polys: Vec<Poly>,
    /// 頂点ごとの、増えた辺（辺の番号の順）
    added_at: IntMap<usize, Vec<usize>>,
    /// 升目の索引の升目ごとの、増えた辺（辺の番号の順）。外接矩形（`reach_box`）がかかる升目で入れる（マスを作らずに
    /// 済む）。ここを引く所（輪の上と中の辺を探す `edges_in_ring`・半直線を横切る辺を探す `ray_cols`）は、どれも辺の上の
    /// 点を含むマスの升目を引くので、辺の上の点がみな入る外接矩形の升目で足りる。升目の順に並べ、列ごとの範囲で引ける
    /// ようにする（`edges_in_ring`）
    added_tiles: std::collections::BTreeMap<(i64, i64), Vec<usize>>,
    /// 升目の行ごとの、増えた辺（辺の番号の順・1つずつ。`added_tiles` を行でまとめたもの）。右へ伸ばした半直線を横切る
    /// 辺を探す所（`ray_cols`）が、半直線の通る行の分だけ引く
    added_rows: IntMap<i64, Vec<usize>>,
    nodes: std::cell::RefCell<Nodes>,
    /// 半直線をなめる所（`ray_cols`）の、行の帯ごとの前の地図の辺の控え（穴の入れ先を探す点は多いので、索引を1回だけ引く）
    ray_bands: RayBands,
}

impl<'a> LocalMap<'a> {
    /// 前の地図（しまう形と升目の索引 `index`）に、窓で重ねた結果 `result` の差分（`removed`：消えた前の地図の辺、
    /// `added`：増えた結果の辺の番号）を当てた地図。結果の頂点は、座標が同じなら前の地図の頂点
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
        let mut added_at: IntMap<usize, Vec<usize>> = IntMap::default();
        let mut added_tiles: std::collections::BTreeMap<(i64, i64), Vec<usize>> =
            Default::default();
        for (i, &r) in added.iter().enumerate() {
            let ed = result.edges[r];
            let (a, b) = (nodes.id(result.nodes[ed.a]), nodes.id(result.nodes[ed.b]));
            list.push(MapEdge { a, b, ..ed });
            let q = result.quad(r);
            polys.push(EdgeGeom::of(&q));
            for t in index_tiles_over(reach_box(q)) {
                added_tiles.entry(t).or_default().push(n_base + i);
            }
            added_at.entry(a).or_default().push(n_base + i);
            if b != a {
                added_at.entry(b).or_default().push(n_base + i);
            }
        }
        let mut added_rows: IntMap<i64, Vec<usize>> = IntMap::default();
        for (t, list) in &added_tiles {
            added_rows.entry(t.1).or_default().extend(list);
        }
        for list in added_rows.values_mut() {
            list.sort_unstable();
            list.dedup();
        }
        Self {
            index,
            n_base,
            removed: removed.iter().copied().collect(),
            added: list,
            added_polys: polys,
            added_at,
            added_tiles,
            added_rows,
            ray_bands: RayBands::default(),
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
                    stroke: te.stroke,
                    line_id: te.line_id,
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
            None => self.index.poly(e),
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

    /// 触った前の地図の面（形が変わりうる前の地図の面を集める決まりはこれ1つ：`regions_faces`・`touched_faces`・
    /// `rewrite`）：消えた辺と、まわりの辺が変わった頂点の、前の地図の辺の側の番号（小さい順・重なり無し）
    fn changed_base_faces(&self) -> Vec<ShapeId> {
        let mut out: Vec<ShapeId> = Vec::new();
        let base_edges = self
            .dirty_nodes()
            .into_iter()
            .flat_map(|v| self.base_edges_at(v));
        for e in self.removed.iter().copied().chain(base_edges) {
            let ed = self.base_edge(e);
            out.extend(ed.left.and(ed.left_src));
            out.extend(ed.right.and(ed.right_src));
        }
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

    /// 閉じた折れ線 `pts`（輪郭の点：`Ring::pts`）の上と中にかかりうる辺（消えた辺を除いた前の地図の辺と、増えた辺。
    /// 番号の順・1つずつ）：輪の上と中の升目（`polygon_squares` の列ごとの範囲）にある辺。輪の中に点がある辺は、その点の
    /// マスの升目にあるので、みな入る。範囲ごとに中身のある升目だけを引くので、手間は輪の長さと中身で決まり、輪の外接矩形の
    /// 中の辺の数（レビュー 96fdc26 の N-1003）にも、中の広さ（レビュー 983236e の N-1001）にもよらない
    fn edges_in_ring(&self, pts: &[IPt]) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        let tiles = polygon_squares::<INDEX_TILE>(pts);
        self.index.edges_over_runs(&tiles, &mut out);
        out.retain(|e| !self.removed.contains(e));
        for (x, y0, y1) in tiles.iter() {
            for (_, list) in self.added_tiles.range((x, y0)..=(x, y1)) {
                out.extend(list.iter().copied());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 2b-5a：増えた辺の決まり（地図全体の判定 `check` の、格子に置ける・両側の色が違う・判定の形が2点以上・区間どうしが
    /// 触れない・別々の頂点が別々の格子の点）。前の地図の辺どうしは、前の地図が正しいので調べない。区間どうしは、どちらかが、
    /// 形が前の地図に無い増えた辺のものだけを調べる（消えた辺と形が同じ増えた辺は、判定の形も同じ）。区間どうしは、地図全体の確かめ（`PlanarMap::polys_apart`）と同じく、区間を通るマスの格子（`CellGrid`）に
    /// 入れ、同じマスにある区間とだけ比べる（触れる区間どうしは必ず同じマスを持つ：`seg_cells`）。手間は触る区間の数に
    /// 比例し、近くの辺どうしの区間の組をみな比べることはしない
    pub(crate) fn added_edges_ok(&self) -> bool {
        let base_keys: IntSet<[u64; 6]> = self
            .removed
            .iter()
            .map(|&e| quad_key(&self.base_edge(e).quad()))
            .collect();
        let mut fresh: IntSet<usize> = Default::default();
        // 確かめる所：増えた辺の判定の形の外接矩形（1目広げる）にかかる升目
        let mut tiles: Vec<(i64, i64)> = Vec::new();
        for e in self.added_edges() {
            let q = self.quad(e);
            let ed = self.edge(e);
            if !on_grid([q.p0, q.p1, q.p2].into_iter()) || !ed.needed() {
                return false;
            }
            let pe = self.poly(e);
            if pe.len() < 2 {
                return false;
            }
            if !base_keys.contains(&quad_key(&q)) {
                fresh.insert(e);
            }
            tiles.extend(GridRect::of(pe).grown(1).index_tiles());
        }
        tiles.sort_unstable();
        tiles.dedup();
        // その升目にかかる辺（消えた辺を除いた前の地図の辺と、増えた辺）
        let mut cands: Vec<usize> = Vec::new();
        for &t in &tiles {
            self.index.index.edges_in(self.index.stored, t, &mut cands);
        }
        cands.retain(|e| !self.removed.contains(e));
        for t in &tiles {
            if let Some(list) = self.added_tiles.get(t) {
                cands.extend(list.iter().copied());
            }
        }
        cands.sort_unstable();
        cands.dedup();
        // 増えた辺の端の頂点が、ほかの頂点と同じ格子の点に丸まらないか（前の地図の頂点どうしは別々の格子の点。座標が
        // 同じなら同じ頂点）。増えた辺の端と同じ格子の点に丸まる頂点は、その点のマスの升目にかかる辺の端なので、
        // 確かめる所の辺の端を見ればよい
        let new_nodes: IntSet<usize> = self.added.iter().flat_map(|ed| [ed.a, ed.b]).collect();
        let mut node_at: IntMap<IPt, usize> = IntMap::default();
        for &f in &cands {
            let fd = self.edge(f);
            for u in [fd.a, fd.b] {
                match node_at.entry(gpt(self.node(u))) {
                    std::collections::hash_map::Entry::Occupied(o) => {
                        let w = *o.get();
                        if w != u && (new_nodes.contains(&u) || new_nodes.contains(&w)) {
                            return false;
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(u);
                    }
                }
            }
        }
        // 区間どうし：形が前の地図に無い増えた辺の区間を格子に入れながら、前に入れた区間と比べ、そのあと確かめる所の
        // ほかの辺の区間を、格子の区間とだけ比べる
        let mut fresh_list: Vec<usize> = fresh.into_iter().collect();
        fresh_list.sort_unstable();
        let mut segs: Vec<(usize, usize, GridRect)> = Vec::new();
        let mut grid = CellGrid::new();
        let mut near: Vec<usize> = Vec::new();
        let clash = |(e, k): (usize, usize), (f, m): (usize, usize)| -> bool {
            let (pe, pf) = (self.poly(e), self.poly(f));
            let (ed, fd) = (self.edge(e), self.edge(f));
            seg_pair_clash(
                (pe[k], pe[k + 1]),
                seg_tags(pe, k, ed.a, ed.b),
                (pf[m], pf[m + 1]),
                seg_tags(pf, m, fd.a, fd.b),
                (e == f && m == k + 1).then_some(pe[k + 1]),
            )
        };
        for &e in &fresh_list {
            let pe = self.poly(e);
            for k in 0..pe.len() - 1 {
                let rk = GridRect::of(&pe[k..k + 2]);
                let cells: Vec<(i64, i64)> = seg_cells((pe[k], pe[k + 1]), 0).collect();
                grid.query(cells.iter().copied(), &mut near);
                for &j in &near {
                    let (f, m, rf) = segs[j];
                    // 前に入れた区間が前（同じ辺なら、区間の順が前）
                    if rk.overlaps(&rf) && clash((f, m), (e, k)) {
                        return false;
                    }
                }
                grid.insert(segs.len(), cells);
                segs.push((e, k, rk));
            }
        }
        let fresh_set: IntSet<usize> = fresh_list.iter().copied().collect();
        for &f in cands.iter().filter(|f| !fresh_set.contains(f)) {
            let pf = self.poly(f);
            for m in 0..pf.len().saturating_sub(1) {
                let rm = GridRect::of(&pf[m..m + 2]);
                grid.query(seg_cells((pf[m], pf[m + 1]), 0), &mut near);
                for &j in &near {
                    let (e, k, re) = segs[j];
                    if re.overlaps(&rm) && clash((e, k), (f, m)) {
                        return false;
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
        // `memo` に入れ、確かめ・面作りで使い回す。たどる決まりは `face_cycles_with` と同じ）
        let mut starts: Vec<(usize, bool)> = self
            .added_edges()
            .flat_map(|e| [(e, true), (e, false)])
            .collect();
        for v in self.dirty_nodes() {
            let list = memo.around.entry(v).or_insert_with(|| self.out_sorted(v));
            starts.extend(list.iter().copied());
        }
        let mut ids: Vec<usize> = Vec::new();
        let mut seen: IntSet<usize> = Default::default();
        for h in starts {
            let ci = self.cycle_of(h, memo)?;
            if seen.insert(ci) {
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
    of: IntMap<(usize, bool), usize>,
    around: IntMap<usize, Vec<(usize, bool)>>,
    /// 何も無い側の輪郭の輪（バケツは、色の無い囲まれた所も面にする：`ring_any`）
    unpainted_rings: IntMap<usize, Ring>,
    /// 決めた穴の入れ先（穴の輪郭の番号ごとの、外周の輪郭の番号。無ければ `None`：`hole_owner`）
    owners: IntMap<usize, Option<usize>>,
}

/// 点から右へ伸ばした半直線が横切る辺を、前の地図＋差分で、列の順に少しずつ引く道具（`LocalMap::ray_cols`・
/// `next_ray_col`）：前の地図の辺は `RayCols` で引き、増えた辺は横切る列ごとに並べておく
struct LocalRayCols {
    base: RayCols,
    /// 増えた辺の、横切る列ごとの並び
    added: std::collections::BTreeMap<i64, Vec<usize>>,
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
    /// 少しずつ引く道具（`LocalRayCols`）。前の地図の辺は `RayCols` で引き、増えた辺は、半直線の通る行にかかるものだけ、
    /// 横切る列をはじめに求めておく
    fn ray_cols(&self, p2: IPt) -> LocalRayCols {
        let probe = RayCols::new(self.index, p2, None);
        let (ty0, ty1) = probe.rows();
        let mut near: Vec<usize> = (ty0..=ty1)
            .filter_map(|row| self.added_rows.get(&row))
            .flatten()
            .copied()
            .collect();
        near.sort_unstable();
        near.dedup();
        let mut added: std::collections::BTreeMap<i64, Vec<usize>> = Default::default();
        for e in near {
            for c in crossing_cols(self.poly(e), p2) {
                let list = added.entry(c).or_default();
                if !list.contains(&e) {
                    list.push(e);
                }
            }
        }
        LocalRayCols {
            base: RayCols::new(self.index, p2, added.keys().last().copied()),
            added,
        }
    }

    /// `LocalRayCols` の次の列（列, その列で半直線を横切る辺）。列が尽きたら `None`
    fn next_ray_col(&self, rc: &mut LocalRayCols) -> Option<(i64, Vec<usize>)> {
        loop {
            let stop = rc.added.keys().next().copied();
            let (tx, mut list) = rc.base.next_col(
                self.index,
                &self.ray_bands,
                |e| !self.removed.contains(&e),
                stop,
            )?;
            list.extend(rc.added.remove(&tx).unwrap_or_default());
            if !list.is_empty() {
                return Some((tx, list));
            }
        }
    }

    /// 色 `color`（`None` ならどの色も。何も無い色は `Some(None)`）の、点 `probe` を含む外周（何も無い側の輪郭も数える）の
    /// うち、いちばん小さいもの（今の `regions` と同じ決まり。穴の入れ先・バケツの面の外周を探すのはこれ1つ）。外周は点を
    /// 含めば、点から右へ伸ばした半直線と交わる。正しい地図では外周どうしは入れ子か離れているので、点を含む外周のうち
    /// いちばん内側のものが、半直線と最初に交わる。そこで、横切る所の列（丸めずに決める：`crossing_cols`）の順に輪郭を
    /// 調べ、見つかった列の中でいちばん小さいものにする。`None`：輪郭が閉じない。`Some(None)`：入れ先が無い
    fn owner(
        &self,
        color: Option<Option<Color32>>,
        probe: IPt,
        memo: &mut CycleMemo,
    ) -> Option<Option<usize>> {
        #[cfg(test)]
        super::OWNER_SEARCHES.with(|n| n.set(n.get() + 1));
        let mut best: Option<(i128, usize)> = None;
        let mut found_col: Option<i64> = None;
        let mut rc = self.ray_cols(probe);
        while let Some((col, edges)) = self.next_ray_col(&mut rc) {
            if found_col.is_some_and(|fc| col > fc) {
                break;
            }
            for e in edges {
                for side in [true, false] {
                    let ci = self.cycle_of((e, side), memo)?;
                    if color.is_some_and(|c| memo.cycles[ci].color != c) {
                        continue;
                    }
                    let r = self.ring_any(ci, memo);
                    if r.is_exterior()
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
    ///   右へ伸ばした半直線が最初に横切る前の地図の辺（消えた辺も）の q の側で決め（`first_crossing`。q が前の地図の辺の
    ///   上なら、q を右へごくわずかずらした所の面。その辺は消えた辺なので、その面は形が変わっていて、触った面で調べる）、
    ///   その辺が残っていて、その向きのある辺の輪郭が触った面に入っていなければ、その面はそのままの面なので、色があれば
    ///   入り込んでいる
    ///
    /// `None`：輪郭が閉じない
    fn unpainted_ok_in(
        &self,
        h: (usize, bool),
        faces: &[LocalFace],
        in_touched: &IntSet<usize>,
        face_of: &IntMap<usize, usize>,
        memo: &mut CycleMemo,
    ) -> Option<bool> {
        let q = self.probe(&[h]);
        let other = self.cycle_of((h.0, !h.1), memo)?;
        let other_face = face_of.get(&other).copied();
        for (fi, f) in faces.iter().enumerate() {
            if other_face == Some(fi) {
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
            if let Some(hf) = super::overlay::first_crossing(self.index, q) {
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

    /// 色のある穴 `ci` の入れ先（`owner`。決めたものは控え `memo` に入れ、2回探さない）。前の地図の穴で、前の入れ先の
    /// 外周 `kept`（`kept_exterior`）が新しい地図にそのまま残り、同じ色の新しい外周（触った外周 `made`：（色, 外接矩形））の
    /// 外接矩形が穴の調べる点を含まなければ、前の入れ先のまま（入れ先は、穴を含む同じ色の外周のうちいちばん小さいもの。
    /// 残った外周の中では前と同じものがいちばん小さく、新しい外周はその点を含まない）。`None`：輪郭が閉じない
    fn hole_owner(
        &self,
        ci: usize,
        kept: Option<usize>,
        made: &[(Option<Color32>, GridRect)],
        memo: &mut CycleMemo,
    ) -> Option<Option<usize>> {
        if let Some(&o) = memo.owners.get(&ci) {
            return Some(o);
        }
        let color = memo.cycles[ci].color;
        let p = self.probe(&memo.cycles[ci].cycle);
        let o = match kept.filter(|_| !made.iter().any(|&(c, r)| c == color && r.contains2(p))) {
            Some(k) => Some(k),
            None => self.owner(Some(color), p, memo)?,
        };
        memo.owners.insert(ci, o);
        Some(o)
    }

    /// 前の地図の面 `id`（前の地図の面の表 `base_faces`）の外周が、新しい地図にそのまま残っていれば（消えた辺が無く、触った
    /// 輪郭 `touched` でない）、その輪郭の番号。`None`：輪郭が閉じない
    fn kept_exterior(
        &self,
        id: ShapeId,
        base_faces: BaseFaces<'_>,
        touched: &IntSet<usize>,
        memo: &mut CycleMemo,
    ) -> Option<Option<usize>> {
        for h in base_faces(id) {
            if self.removed.contains(&h.0) {
                continue;
            }
            let ci = self.cycle_of(h, memo)?;
            if memo.cycles[ci].ring.as_ref().is_some_and(|r| r.is_exterior()) {
                return Some((!touched.contains(&ci)).then_some(ci));
            }
        }
        Some(None)
    }

    /// 触った外周（色, 外接矩形）：前の地図の穴の入れ先を決め直すかを決める（`hole_owner`）
    fn made_exteriors(touched_ids: &[usize], memo: &CycleMemo) -> Vec<(Option<Color32>, GridRect)> {
        touched_ids
            .iter()
            .filter_map(|&ci| {
                let c = &memo.cycles[ci];
                c.ring
                    .as_ref()
                    .filter(|r| r.is_exterior())
                    .map(|r| (c.color, r.rect))
            })
            .collect()
    }

    /// 2b-5c：触った輪郭（`touched_cycles_ok`）の、穴の入れ先と何も無い側の決まり（地図全体の判定 `check` の穴の入れ先
    /// `regions` と、何も無い側の輪郭の確かめ）。正しければ触った面（`touched_faces`）。
    /// - 触った色のある穴：入れ先が無ければ正しくない
    /// - 前の地図の穴で、入れ先の外周が触られたもの（前の地図の面の表 `base_faces` で、触った前の地図の外周の面の穴を
    ///   引く）：入れ先が無ければ正しくない
    /// - 触った何も無い側の輪郭と、新しい輪の中に点がある前の地図の何も無い側の輪郭（輪の上と中のマスの辺から探す：
    ///   `edges_in_ring`）：色のある面に入り込めば正しくない（調べる輪郭は今の決まりより多いが、どれも正しい地図で成り立つ
    ///   決まりなので、答えは同じ）
    ///
    /// `Some(None)`：正しくない。`None`：輪郭が閉じないか、穴の入れ先が無い
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
        let touched_set: IntSet<usize> = touched_ids.iter().copied().collect();
        let made = Self::made_exteriors(&touched_ids, memo);
        // 触った色のある穴
        for &ci in &touched_ids {
            let hole = memo.cycles[ci].ring.as_ref().is_some_and(|r| !r.is_exterior());
            if hole && self.hole_owner(ci, None, &made, memo)?.is_none() {
                return Some(None);
            }
        }
        // 前の地図の穴で、入れ先の外周が触られたもの：触った前の地図の外周（消えた辺と、まわりの辺が変わった前の地図の
        // 頂点の、前の地図の向きのある辺の左の面）の面の、前の地図の輪郭のうち、新しい地図にそのままあるもの
        for id in self.changed_base_faces() {
            let kept = self.kept_exterior(id, base_faces, &touched_set, memo)?;
            for h in base_faces(id) {
                if self.removed.contains(&h.0) {
                    continue;
                }
                let ci = self.cycle_of(h, memo)?;
                if touched_set.contains(&ci) {
                    continue;
                }
                let hole = memo.cycles[ci].ring.as_ref().is_some_and(|r| !r.is_exterior());
                if hole && self.hole_owner(ci, kept, &made, memo)?.is_none() {
                    return Some(None);
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
        let mut checking: IntSet<(usize, bool)> = to_check.iter().copied().collect();
        let made_rings: Vec<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| memo.cycles[ci].ring.is_some())
            .collect();
        for ci in made_rings {
            let near =
                self.edges_in_ring(&memo.cycles[ci].ring.as_ref().expect("色のある輪郭").pts);
            for e in near {
                for side in [true, false] {
                    let h = (e, side);
                    if self.side_color(h).is_some()
                        || memo.of.get(&h).is_some_and(|c| touched_set.contains(c))
                    {
                        continue;
                    }
                    let inside = memo.cycles[ci]
                        .ring
                        .as_ref()
                        .is_some_and(|r| r.contains(self.probe(&[h])) == Some(true));
                    if inside && checking.insert(h) {
                        to_check.push(h);
                    }
                }
            }
        }
        let faces = self.touched_faces(touched, base_faces, memo)?;
        // 触った面に入る輪郭と、触った輪郭（そのままの面でない輪郭）
        let in_touched: IntSet<usize> = faces
            .iter()
            .flat_map(|f| f.cycles.iter().copied())
            .chain(touched_ids.iter().copied())
            .collect();
        // 輪郭ごとの、それを持つ触った面
        let face_of: IntMap<usize, usize> = faces
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| f.cycles.iter().map(move |&ci| (ci, fi)))
            .collect();
        for h in to_check {
            if !self.unpainted_ok_in(h, &faces, &in_touched, &face_of, memo)? {
                return Some(None);
            }
        }
        Some(Some(faces))
    }
}

impl LocalMap<'_> {
    /// 輪郭 `ci` の輪（何も無い側の輪郭も。バケツは、色の無い囲まれた所も面にする）
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
        if let Some(list) = self.added_tiles.get(&index_tile(cell_of(g))) {
            cands.extend(list.iter().copied());
        }
        cands.iter().any(|&e| {
            self.poly(e)
                .windows(2)
                .any(|s| on_seg((2 * s[0].0, 2 * s[0].1), (2 * s[1].0, 2 * s[1].1), p2))
        })
    }

    /// バケツの面（点を含む面。何も無い側の囲まれた所も面）：点 `p2`（2倍の座標）を含む
    /// いちばん小さい外周と、その穴（同じ色で、入れ先がその外周の輪郭）の、向きのある辺。色のある面の穴は、前の地図の面の
    /// 表 `base_faces` で引く。何も無い所の穴は、外周の上と中のマスの辺（`edges_in_ring`）の何も無い側の輪郭から探し、
    /// 入れ先はその中の別の外周とだけ比べて決める。点が輪郭の上にあるか、どの外周にも入らなければ `None`。
    /// （面の色, 面の輪郭の向きのある辺）
    pub(crate) fn face_at(
        &self,
        p2: IPt,
        base_faces: BaseFaces<'_>,
        memo: &mut CycleMemo,
    ) -> Option<FaceAt> {
        if self.on_some_edge(p2) {
            return None;
        }
        let ext = self.owner(None, p2, memo)??;
        let color = memo.cycles[ext].color;
        let mut cycles = vec![ext];
        let mut seen: IntSet<usize> = [ext].into_iter().collect();
        match color {
            Some(_) => {
                let h0 = memo.cycles[ext].cycle[0];
                let id = self.side_src(h0)?;
                for h in base_faces(id) {
                    let ci = self.cycle_of(h, memo)?;
                    if seen.insert(ci) {
                        cycles.push(ci);
                    }
                }
            }
            None => {
                // 外周の上と中のマスの辺の、何も無い側の輪郭を、別の外周（面積が正）と穴の候補（面積が負）に分ける
                let near = self.edges_in_ring(&self.ring_any(ext, memo).pts);
                let (mut others, mut holes): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
                for e in near {
                    for side in [true, false] {
                        if self.side_color((e, side)).is_some() {
                            continue;
                        }
                        let ci = self.cycle_of((e, side), memo)?;
                        if seen.insert(ci) {
                            if self.ring_any(ci, memo).is_exterior() {
                                others.push(ci);
                            } else {
                                holes.push(ci);
                            }
                        }
                    }
                }
                // 穴の入れ先は、調べる点を含む何も無い側の外周のうちいちばん小さいもの（`owner` と同じ決まり）。`ext` より
                // 小さくて点を含む外周は `ext` の中にあるので、その辺は `ext` の上と中のマスにあり、`others` に入っている。
                // そこで、点を含みうる外周（外接矩形の高さの帯の索引で引く）とだけ比べ、半直線はなめない
                let rings = &memo.unpainted_rings;
                let bands = YBands::new(
                    others
                        .iter()
                        .map(|ci| (2 * rings[ci].rect.y0, 2 * rings[ci].rect.y1)),
                );
                let ext_ring = &rings[&ext];
                for ci in holes {
                    let hp = self.probe(&memo.cycles[ci].cycle);
                    let inside = |r: &Ring| r.contains(hp) == Some(true);
                    let in_other = bands.at(hp.1).iter().any(|&k| {
                        let r = &rings[&others[k as usize]];
                        r.area2 < ext_ring.area2 && inside(r)
                    });
                    if inside(ext_ring) && !in_other {
                        cycles.push(ci);
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
    /// （`regions_faces` が正しいと答えたもの）にだけ使う。`None`：輪郭が閉じないか、穴の入れ先が無い
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
        let touched_set: IntSet<usize> = touched_ids.iter().copied().collect();
        let made = Self::made_exteriors(&touched_ids, memo);
        // 色のある輪郭なら、外周か（`Ring::is_exterior`）
        let painted = |ci: usize, memo: &CycleMemo| memo.cycles[ci].ring.as_ref().map(Ring::is_exterior);
        // 面に入る外周（触った外周と、穴が変わる外周）
        let mut exteriors: std::collections::BTreeSet<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| painted(ci, memo) == Some(true))
            .collect();
        // 入れ先を決め直す穴（と、前の地図の穴なら前の入れ先の外周がそのまま残っているか：`hole_owner`）
        let mut holes: Vec<(usize, Option<usize>)> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| painted(ci, memo) == Some(false))
            .map(|ci| (ci, None))
            .collect();
        // 触った前の地図の面：外周がそのままなら穴が変わる面。そのままの穴は入れ先を決め直す
        for id in self.changed_base_faces() {
            let kept = self.kept_exterior(id, base_faces, &touched_set, memo)?;
            for h in base_faces(id) {
                if self.removed.contains(&h.0) {
                    continue;
                }
                let ci = self.cycle_of(h, memo)?;
                if touched_set.contains(&ci) {
                    continue;
                }
                match painted(ci, memo) {
                    Some(true) => {
                        exteriors.insert(ci);
                    }
                    Some(false) => holes.push((ci, kept)),
                    None => {}
                }
            }
        }
        // 触った外周の中に入った前の地図の穴（新しく作った同じ色の外周の中か上に調べる点がある穴は、入れ先を決め直す。外の
        // 穴の入れ先は、この外周では変わらない）
        let new_exteriors: Vec<usize> = touched_ids
            .iter()
            .copied()
            .filter(|&ci| painted(ci, memo) == Some(true))
            .collect();
        for ext in new_exteriors {
            let (color, near) = {
                let c = &memo.cycles[ext];
                let ring = c.ring.as_ref().expect("色のある輪郭");
                (c.color, self.edges_in_ring(&ring.pts))
            };
            for e in near {
                for side in [true, false] {
                    if self.side_color((e, side)) != color {
                        continue;
                    }
                    let ci = self.cycle_of((e, side), memo)?;
                    if touched_set.contains(&ci) || painted(ci, memo) != Some(false) {
                        continue;
                    }
                    let hp = self.probe(&memo.cycles[ci].cycle);
                    let ring = memo.cycles[ext].ring.as_ref().expect("色のある輪郭");
                    if ring.contains(hp) != Some(false) {
                        holes.push((ci, None));
                    }
                }
            }
        }
        // 同じ穴は1回（前の入れ先の外周が分かっているものを先に）
        holes.sort_unstable_by_key(|&(ci, kept)| (ci, kept.is_none()));
        holes.dedup_by_key(|h| h.0);
        // 穴の入れ先
        let mut placed: IntMap<usize, Vec<usize>> = IntMap::default();
        let mut placed_holes: IntSet<usize> = Default::default();
        for (h, kept) in holes {
            let owner = self.hole_owner(h, kept, &made, memo)??;
            exteriors.insert(owner);
            placed.entry(owner).or_default().push(h);
            placed_holes.insert(h);
        }
        // 面にする：外周がそのままの面は、前の地図の面の表のそのままの穴（入れ先を決め直した穴は除く）も入れる
        let mut faces = Vec::new();
        for ext in exteriors {
            let mut cycles = vec![ext];
            cycles.extend(placed.get(&ext).into_iter().flatten().copied());
            if !touched_set.contains(&ext) {
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
                        if ci != ext && !placed_holes.contains(&ci) && !touched_set.contains(&ci) {
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

/// 曲線の形の鍵（両端を決まった順にし、制御点と合わせたビット。地図の辺の鍵 `edge_key` もこれを呼ぶ）
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
        // 増えた辺は、色の無い側の線のラベル（`line_label`）だけを持って始める（色のある側は面の番号を振り直す）
        let mut sides: IntMap<usize, (Option<ShapeId>, Option<ShapeId>)> = IntMap::default();
        for e in self.added_edges() {
            let ed = self.edge(e);
            sides.insert(
                e,
                (
                    line_label(ed.left, ed.left_src, ed.stroke),
                    line_label(ed.right, ed.right_src, ed.stroke),
                ),
            );
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
                stroke: ed.stroke,
                line_id: super::written_line_id(ed.stroke, ed.line_id),
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

    /// 2b-6：触った面 `faces`（`touched_faces`）の書き戻しの差分（書き戻しの決まりはこれ1つ）：
    /// - 面の辺がみな前の地図にあり（形がビットまで同じ）、辺の側の印が同じ1つの元の図形で、その図形と色も同じで、その
    ///   図形のどの辺も消えていない面は、元の図形のまま残す（`kept_src`）
    /// - ほかの面は新しい図形。辺の側の印のうち、残した面の図形でないものでいちばん多い元の図形（同じ数なら小さい番号：
    ///   `top_vote`。重ね合わせで面に伝える番号と同じ決まり）を引き継ぐ（同じ元の図形を引き継ぐ面が2つ以上あれば、その印の
    ///   付いた側がいちばん多い面、同じなら形の鍵の小さい面。地図の番号の順で決めない）。面が分かれても、まとまっても
    ///   （動かした片が残りとつながるなど）、長い輪郭の側が番号を保つので、地図の、その面のほかの辺の印も変わらない（しまう
    ///   地図で、変わった場所の塊だけが新しくなる）
    /// - 残した面の元の図形のほかは、みな消す（どの面にも残らない元の図形が、新しい形と重なって残ることは無い。番号を
    ///   引き継いだ形は、消した元の図形と入れ替わる）
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
        let mut old_faces: std::collections::BTreeSet<ShapeId> =
            self.changed_base_faces().into_iter().collect();
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
        let removed_keys: IntMap<[u64; 6], usize> = self
            .removed
            .iter()
            .map(|&e| (quad_key(&self.base_edge(e).quad()), e))
            .collect();
        let added_keys: IntSet<[u64; 6]> = self
            .added_edges()
            .map(|e| quad_key(&self.quad(e)))
            .collect();
        let same =
            |e: usize| e < self.n_base() || removed_keys.contains_key(&quad_key(&self.quad(e)));
        // 向きのある辺 `h` の側の、前の地図の番号（形が同じ前の地図の辺の、同じ側。前からある辺はその辺の側）
        let old_src = |h: (usize, bool)| -> Option<ShapeId> {
            if h.0 < self.n_base() {
                return self.side_src(h);
            }
            let r = *removed_keys.get(&quad_key(&self.quad(h.0)))?;
            let te = self.base_edge(r);
            let ed = self.edge(h.0);
            let from = self.node(if h.1 { ed.a } else { ed.b });
            if te.a == from {
                te.left_src
            } else {
                te.right_src
            }
        };
        // 辺が消えた元の図形
        let mut cut: IntSet<ShapeId> = Default::default();
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
        let mut kept: IntSet<ShapeId> = Default::default();
        let mut changed: Vec<usize> = Vec::new();
        for (i, f) in faces.iter().enumerate() {
            // 残す面の決まり（`kept_src`）。触っていない面が残す元の図形は、触った面は残さない
            let keep = super::kept_src(
                halves(f),
                |h| self.side_src(h),
                old_src,
                f.color,
                orig_color,
                |s| cut.contains(&s),
            );
            match keep {
                Some(s) if !kept_untouched(s) && kept.insert(s) => ids[i] = s,
                _ => changed.push(i),
            }
        }
        let is_kept = |s: ShapeId| kept.contains(&s) || kept_untouched(s);
        // 引き継ぐ元の図形
        let face_key = |f: &LocalFace| hs_key(&halves(f), self);
        let mut heir: IntMap<ShapeId, (usize, usize)> = IntMap::default();
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
        // 形の変わらない輪郭（触っていない穴など）は、元の図形の輪郭をそのまま使い回す（輪郭の
        // 点の並びを共有する）。元の図形ごとに、輪郭の区間の形（ビット）から（輪郭, 区間の位置）を引く表を1回だけ作る
        // 表の鍵は区間の点のビット（-0.0 は 0.0 にそろえる：区間は `==` で比べるので）。輪郭は、外周なら `None`、穴なら
        // その番号
        type SegAt = IntMap<[u64; 6], (Option<usize>, usize)>;
        let seg_bits =
            |q: &QuadBez| [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y].map(|v| (v + 0.0).to_bits());
        let tables: std::cell::RefCell<IntMap<ShapeId, (Arc<FillShape>, SegAt)>> = Default::default();
        let reuse = |cycle: &[(usize, bool)], quads: &[QuadBez]| -> Option<Contour> {
            if !cycle.iter().all(|h| same(h.0)) {
                return None;
            }
            let src = self.side_src(cycle[0])?;
            let mut tables = tables.borrow_mut();
            let (shape, at) = match tables.entry(src) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => {
                    let shape = orig_shape(src)?;
                    let mut at: SegAt = IntMap::default();
                    let all = std::iter::once((None, &shape.exterior))
                        .chain(shape.holes.iter().enumerate().map(|(i, c)| (Some(i), c)));
                    for (ci, c) in all {
                        for (k, q) in c.segments.iter().enumerate() {
                            at.entry(seg_bits(q)).or_insert((ci, k));
                        }
                    }
                    v.insert((shape, at))
                }
            };
            let &(ci, k) = at.get(&seg_bits(&quads[0]))?;
            let c = match ci {
                None => &shape.exterior,
                Some(i) => &shape.holes[i],
            };
            (c.segments.len() == quads.len()
                && (0..quads.len()).all(|i| c.segments[(k + i) % quads.len()] == quads[i]))
            .then(|| c.clone())
        };
        let added: Vec<FillShape> = changed
            .into_iter()
            .map(|i| {
                let f = &faces[i];
                let cycles = f.cycles.iter().map(|&ci| {
                    memo.cycles[ci]
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
                        .collect()
                });
                let reuse =
                    |k: usize, quads: &[QuadBez]| reuse(&memo.cycles[f.cycles[k]].cycle, quads);
                let mut s = cycles_shape(f.color, cycles, reuse, layer_id, group_path);
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
