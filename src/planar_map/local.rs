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

    /// 向きのある辺の、番号によらない鍵（形の鍵 `edge_key` と、辺の決まった向きに進むか）
    pub(crate) fn half_key(&self, (e, forward): (usize, bool)) -> ([u64; 6], bool) {
        let q = self.quad(e);
        let canonical = point_order(q.p0, q.p2).is_le();
        (quad_key(&q), forward == canonical)
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

/// 輪郭を、番号によらない形にしたもの（向きのある辺の鍵の並び。いちばん小さい鍵から始める）。お手本と比べるため
#[cfg(test)]
pub(crate) fn cycle_shape(keys: impl Iterator<Item = ([u64; 6], bool)>) -> Vec<([u64; 6], bool)> {
    let mut c: Vec<([u64; 6], bool)> = keys.collect();
    if let Some(k) = (0..c.len()).min_by_key(|&k| c[k]) {
        c.rotate_left(k);
    }
    c
}
