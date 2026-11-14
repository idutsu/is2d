//! 格子の幾何：判定の形（曲線を 1/4096px の格子に丸めた折れ線）の交わり・近さ・巻き数・面の取り出しを整数で決める

use super::*;

/// 格子の点（1/4096px を 1 とする整数の座標）
pub(super) type IPt = (i64, i64);

/// 点を格子の点に丸める（丸めの式はこれ1つ。座標の範囲は見ない：範囲も見るのは `to_grid`）
pub(super) fn gpt(p: Point) -> IPt {
    (
        (p.x * OVERLAY_GRID).round() as i64,
        (p.y * OVERLAY_GRID).round() as i64,
    )
}

/// 曲線の判定の形：重ねるときと同じ数に等分した折れ線の点を、格子に丸めたもの（続けて同じ点は1つにする）
pub(super) fn grid_poly(q: &QuadBez) -> Vec<IPt> {
    grid_line(flat_points(*q, FLAT_TOL).map(|(p, _)| p))
}

/// 曲線の判定の形（`grid_poly`）を `out` に入れる（前の中身は消す。入れ物を使い回して、確保を減らす）
pub(super) fn grid_poly_into(q: &QuadBez, out: &mut Vec<IPt>) {
    grid_line_into(flat_points(*q, FLAT_TOL).map(|(p, _)| p), out);
}

/// 曲線の判定の形（`grid_poly`）の点を1つずつ（並びを作らない）
pub(super) fn grid_poly_points(q: &QuadBez) -> impl Iterator<Item = IPt> {
    grid_points_of(flat_points(*q, FLAT_TOL).map(|(p, _)| p))
}

/// 折れ線の点 `pts` を格子の点に丸めたもの（続けて同じ点は1つにする）
pub(super) fn grid_line(pts: impl Iterator<Item = Point>) -> Vec<IPt> {
    let mut out: Vec<IPt> = Vec::with_capacity(pts.size_hint().0);
    out.extend(grid_points_of(pts));
    out
}

/// 折れ線の点 `pts` を格子の点に丸めたもの（`grid_line`）を `out` に入れる（前の中身は消す）
pub(super) fn grid_line_into(pts: impl Iterator<Item = Point>, out: &mut Vec<IPt>) {
    out.clear();
    out.extend(grid_points_of(pts));
}

/// 折れ線の点 `pts` を格子の点に丸めたものを1つずつ（続けて同じ点は1つにする。折れ線を格子に丸めるのはこれ1つ）
pub(super) fn grid_points_of(pts: impl Iterator<Item = Point>) -> impl Iterator<Item = IPt> {
    let mut last: Option<IPt> = None;
    pts.filter_map(move |p| {
        let ip = gpt(p);
        (last != Some(ip)).then(|| {
            last = Some(ip);
            ip
        })
    })
}

/// `c` が、`a` から `b` へ向かう線の左（正）・右（負）・上（0）のどちらにあるか（外積）
pub(super) fn orient(a: IPt, b: IPt, c: IPt) -> i128 {
    (b.0 - a.0) as i128 * (c.1 - a.1) as i128 - (b.1 - a.1) as i128 * (c.0 - a.0) as i128
}

/// `p` が、区間 `a`–`b` の上（端を含む）にあるか
pub(super) fn on_seg(a: IPt, b: IPt, p: IPt) -> bool {
    orient(a, b, p) == 0
        && p.0 >= a.0.min(b.0)
        && p.0 <= a.0.max(b.0)
        && p.1 >= a.1.min(b.1)
        && p.1 <= a.1.max(b.1)
}

/// 区間 `a`–`b` と `c`–`d` が、どこかで触れるか（端・重なりを含む）
pub(super) fn segs_touch(a: IPt, b: IPt, c: IPt, d: IPt) -> bool {
    // 外接矩形が離れていれば触れない（向きの計算をしないで済ませる）
    if a.0.max(b.0) < c.0.min(d.0)
        || c.0.max(d.0) < a.0.min(b.0)
        || a.1.max(b.1) < c.1.min(d.1)
        || c.1.max(d.1) < a.1.min(b.1)
    {
        return false;
    }
    let (o1, o2) = (orient(a, b, c).signum(), orient(a, b, d).signum());
    let (o3, o4) = (orient(c, d, a).signum(), orient(c, d, b).signum());
    if o1 != o2 && o3 != o4 && o1 != 0 && o2 != 0 && o3 != 0 && o4 != 0 {
        return true;
    }
    on_seg(a, b, c) || on_seg(a, b, d) || on_seg(c, d, a) || on_seg(c, d, b)
}

/// 共有する端 `v` から出る2つの区間 `v`–`p` と `v`–`q` が、`v` のほかでも触れるか（同じ向きに重なるときだけ）
pub(super) fn fan_overlap(v: IPt, p: IPt, q: IPt) -> bool {
    orient(v, p, q) == 0
        && (p.0 - v.0) as i128 * (q.0 - v.0) as i128 + (p.1 - v.1) as i128 * (q.1 - v.1) as i128 > 0
}

/// 区間 `a`–`b` と `c`–`d` の距離が、格子の目の対角線の半分（`GRID_REACH`）以下か（2乗で 1/2 以下か）
pub(super) fn segs_within_reach(a: IPt, b: IPt, c: IPt, d: IPt) -> bool {
    if segs_touch(a, b, c, d) {
        return true;
    }
    // 交わらない2つの区間の距離は、どちらかの端から、もう一方の区間までの距離のいちばん小さいもの
    let pt_seg_le_half = |p: IPt, a: IPt, b: IPt| -> bool {
        let (dx, dy) = ((b.0 - a.0) as i128, (b.1 - a.1) as i128);
        let (px, py) = ((p.0 - a.0) as i128, (p.1 - a.1) as i128);
        let len2 = dx * dx + dy * dy;
        let dot = px * dx + py * dy;
        if len2 == 0 || dot <= 0 {
            return 2 * (px * px + py * py) <= 1;
        }
        if dot >= len2 {
            let (qx, qy) = ((p.0 - b.0) as i128, (p.1 - b.1) as i128);
            return 2 * (qx * qx + qy * qy) <= 1;
        }
        // 線までの距離の2乗 = cross² / len2
        let cross = dx * py - dy * px;
        2 * cross * cross <= len2
    };
    pt_seg_le_half(a, c, d)
        || pt_seg_le_half(b, c, d)
        || pt_seg_le_half(c, a, b)
        || pt_seg_le_half(d, a, b)
}

/// 区間 `a`–`b`（端の頂点の番号 `ta`・`tb`。折れ線の端で頂点にあたるときだけ `Some`）と、区間 `c`–`d`（`tc`・`td`）が、
/// 許されない所で触れるか。許されるのは、端を1つだけ共有し、そこから同じ向きに重ならず、その端が両方で同じ頂点か、
/// 1本の折れ線のとなり合う区間のつなぎ目（`joint`）のときだけ
pub(super) fn seg_pair_clash(
    (a, b): (IPt, IPt),
    (ta, tb): (Option<usize>, Option<usize>),
    (c, d): (IPt, IPt),
    (tc, td): (Option<usize>, Option<usize>),
    joint: Option<IPt>,
) -> bool {
    if !segs_touch(a, b, c, d) {
        return false;
    }
    let (v, p, q, t1, t2) = if a == c {
        (a, b, d, ta, tc)
    } else if a == d {
        (a, b, c, ta, td)
    } else if b == c {
        (b, a, d, tb, tc)
    } else if b == d {
        (b, a, c, tb, td)
    } else {
        return true;
    };
    if fan_overlap(v, p, q) {
        return true;
    }
    !((t1.is_some() && t1 == t2) || joint == Some(v))
}

/// 2本の辺の判定の形 `pa`（端の頂点 `ea`）と `pb`（`eb`）が、共有する頂点のほかで触れるか
pub(super) fn polys_clash(pa: &[IPt], ea: (usize, usize), pb: &[IPt], eb: (usize, usize)) -> bool {
    let rb = GridRect::of(pb);
    if !GridRect::of(pa).overlaps(&rb) {
        return false;
    }
    (0..pa.len().saturating_sub(1)).any(|k| {
        if !GridRect::of(&pa[k..k + 2]).overlaps(&rb) {
            return false;
        }
        let ta = seg_tags(pa, k, ea.0, ea.1);
        (0..pb.len().saturating_sub(1)).any(|m| {
            seg_pair_clash(
                (pa[k], pa[k + 1]),
                ta,
                (pb[m], pb[m + 1]),
                seg_tags(pb, m, eb.0, eb.1),
                None,
            )
        })
    })
}

/// 折れ線 `pts`（続けて同じ点は無い）の点が、ある向き（横・縦・斜め2つのどれか）に、一方へだけ進むか。そうなら、
/// 折れ線は自分と触れない：となり合わない2つの区間は、その向きに測った範囲が重ならず、となり合う2つの区間は、
/// つなぎ目から反対の側へ出るので、同じ向きに重ならない（`can_replace` の自分との確かめを省く。省いても答えは同じ）
pub(super) fn monotone(pts: &[IPt]) -> bool {
    [(1, 0), (0, 1), (1, 1), (1, -1)]
        .iter()
        .any(|&(ux, uy): &(i64, i64)| {
            let at = |p: IPt| ux as i128 * p.0 as i128 + uy as i128 * p.1 as i128;
            let up = pts.windows(2).all(|w| at(w[0]) < at(w[1]));
            up || pts.windows(2).all(|w| at(w[0]) > at(w[1]))
        })
}

/// 折れ線 `poly` の区間 `k` の両端の頂点の番号（折れ線の端で、頂点 `na`・`nb` にあたるときだけ）
pub(super) fn seg_tags(
    poly: &[IPt],
    k: usize,
    na: usize,
    nb: usize,
) -> (Option<usize>, Option<usize>) {
    ((k == 0).then_some(na), (k + 2 == poly.len()).then_some(nb))
}

/// 判定の形（格子の上の折れ線）の索引。地図の一部の辺を1本の曲線に置き換えてよいかを、整数で調べる
/// （設計 段階2 の 2-4。曲線への置き換え・2本の辺のまとめ・アンカーの追加と削除で使う）
pub(super) struct PolyIndex {
    /// 辺ごとの判定の形の点。みな1つの並びに入れ、辺ごとの範囲を持つ（辺ごとに並びを持つと、辺ごとに小さな確保が
    /// 起きる。`CellGrid` と同じ理由）。辺 `f` は `pts[poly_at[f]]`（`poly`）
    pts: Vec<IPt>,
    poly_at: Vec<(u32, u32)>,
    pub(super) ends: Vec<(usize, usize)>,
    pub(super) alive: Vec<bool>,
    /// 区間の番号 → （辺, 区間の位置）
    pub(super) segs: Vec<(u32, u32)>,
    pub(super) grid: CellGrid,
    pub(super) node_pos: Vec<IPt>,
    pub(super) node_grid: CellGrid,
    /// 頂点ごとの、つながる辺（消した辺も）。頂点 `v` の最初の印は `first_link[v]`、印は（辺, 同じ頂点の次の印）で、
    /// 終わりは `CELL_END`（足した順の逆にたどる）
    first_link: Vec<u32>,
    links: Vec<(u32, u32)>,
    pub(super) deg: Vec<usize>,
    /// 索引に入れる範囲（格子の点。`None` なら全部）
    pub(super) area: Option<GridRect>,
}

impl PolyIndex {
    /// 地図 `map` の辺の索引。`area`（px）に外接矩形がかかる区間と、中にある頂点だけを入れる（調べる相手がその中に
    /// 限られるとき。`None` なら全部）
    pub(super) fn new(map: &PlanarMap, area: Option<KRect>) -> Self {
        Self::build(&map.nodes, &map.edges, |_| true, area)
    }

    /// 頂点 `nodes` と辺 `edges` の索引（番号は `edges` のまま。`alive` でない辺は消した印を付ける）。
    /// `area` に掛からない辺は、判定の形を作らない（区間が無い辺として持つ）
    pub(super) fn build(
        nodes: &[Point],
        edges: &[MapEdge],
        alive: impl Fn(usize) -> bool,
        area: Option<KRect>,
    ) -> Self {
        #[cfg(test)]
        super::INDEX_BUILDS.with(|n| n.set(n.get() + 1));
        // 格子の点に直すとき、外へ1目広げる（丸めで内側へ寄らないように）
        let area = area.map(|r| GridRect {
            x0: (r.x0 * OVERLAY_GRID).floor() as i64 - 1,
            y0: (r.y0 * OVERLAY_GRID).floor() as i64 - 1,
            x1: (r.x1 * OVERLAY_GRID).ceil() as i64 + 1,
            y1: (r.y1 * OVERLAY_GRID).ceil() as i64 + 1,
        });
        let mut ix = PolyIndex {
            pts: Vec::new(),
            poly_at: Vec::with_capacity(edges.len()),
            ends: Vec::with_capacity(edges.len()),
            alive: Vec::with_capacity(edges.len()),
            segs: Vec::new(),
            grid: CellGrid::new(),
            node_pos: nodes.iter().map(|p| gpt(*p)).collect(),
            node_grid: CellGrid::new(),
            first_link: vec![CELL_END; nodes.len()],
            links: Vec::with_capacity(2 * edges.len()),
            deg: vec![0; nodes.len()],
            area,
        };
        for (e, ed) in edges.iter().enumerate() {
            let q = QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]);
            let reached = area.is_none_or(|a| a.overlaps(&reach_of(&q)));
            ix.add_edge(
                ed.a,
                ed.b,
                reached.then(|| grid_poly_points(&q)).into_iter().flatten(),
            );
            if !alive(e) {
                ix.kill(e);
            }
        }
        for v in 0..nodes.len() {
            let p = ix.node_pos[v];
            if ix.area.is_none_or(|r| r.contains2((2 * p.0, 2 * p.1))) {
                ix.node_grid.insert(v, [cell_of(p)]);
            }
        }
        ix
    }

    /// 判定の形 `poly` の辺を足す（番号を返す）
    pub(super) fn add_edge(
        &mut self,
        a: usize,
        b: usize,
        poly: impl IntoIterator<Item = IPt>,
    ) -> usize {
        let e = self.poly_at.len();
        let start = self.pts.len();
        self.pts.extend(poly);
        for k in 0..(self.pts.len() - start).saturating_sub(1) {
            let sg = (self.pts[start + k], self.pts[start + k + 1]);
            let r = GridRect::of(&[sg.0, sg.1]);
            if self.area.is_some_and(|x| !x.overlaps(&r)) {
                continue;
            }
            self.grid.insert(self.segs.len(), seg_cells(sg, 0));
            self.segs.push((e as u32, k as u32));
        }
        self.poly_at.push((start as u32, self.pts.len() as u32));
        self.ends.push((a, b));
        self.alive.push(true);
        for v in [a, b] {
            self.links.push((e as u32, self.first_link[v]));
            self.first_link[v] = self.links.len() as u32 - 1;
        }
        self.deg[a] += 1;
        self.deg[b] += 1;
        e
    }

    /// 辺 `f` の判定の形
    pub(super) fn poly(&self, f: usize) -> &[IPt] {
        let (a, b) = self.poly_at[f];
        &self.pts[a as usize..b as usize]
    }

    /// 頂点 `v` につながる辺（消した辺も。足した順の逆）
    fn edges_at(&self, v: usize) -> impl Iterator<Item = usize> + '_ {
        let mut k = self.first_link[v];
        std::iter::from_fn(move || {
            (k != CELL_END).then(|| {
                let (f, next) = self.links[k as usize];
                k = next;
                f as usize
            })
        })
    }

    /// 矩形 `r`（格子の点）が、索引に入れた範囲に収まるか（その中の区間と頂点は、みな索引にある）
    pub(super) fn covers(&self, r: &GridRect) -> bool {
        self.area
            .is_none_or(|a| a.x0 <= r.x0 && r.x1 <= a.x1 && a.y0 <= r.y0 && r.y1 <= a.y1)
    }

    /// 辺を消す
    pub(super) fn kill(&mut self, e: usize) {
        if std::mem::replace(&mut self.alive[e], false) {
            let (a, b) = self.ends[e];
            self.deg[a] -= 1;
            self.deg[b] -= 1;
        }
    }

    /// 辺 `part`（頂点 `na` から、途中の頂点 `inner` を通り、`nb` へつながる道。判定の形の点は `path`）を、
    /// `na` から `nb` への折れ線 `pc` の1本の辺に置き換えてよいか（設計 2-4 の3つの決まり）：
    /// 1. `pc` が自分とも、ほかの辺の折れ線とも、共有する端のほかで触れない
    /// 2. `path` と `pc` で囲まれた所（境目の上を含む）に、ほかの頂点が無い
    /// 3. `na`・`nb` につながるほかの辺の最初の区間の真ん中が、囲まれた所（境目の上を含む）に無い
    pub(super) fn can_replace(
        &self,
        part: &[usize],
        path: &[IPt],
        inner: &[usize],
        pc: &[IPt],
        na: usize,
        nb: usize,
    ) -> bool {
        if pc.len() < 2 || na == nb {
            return false;
        }
        let last = pc.len() - 2;
        // 1. 自分と（ある向きに点が一方へだけ進む折れ線は、自分と触れない：`monotone`）
        if !monotone(pc) {
            for k in 0..=last {
                for m in k + 1..=last {
                    let joint = (m == k + 1).then_some(pc[m]);
                    if seg_pair_clash(
                        (pc[k], pc[k + 1]),
                        (None, None),
                        (pc[m], pc[m + 1]),
                        (None, None),
                        joint,
                    ) {
                        return false;
                    }
                }
            }
        }
        // 1. ほかの辺と
        for k in 0..=last {
            let sg = (pc[k], pc[k + 1]);
            let tags = seg_tags(pc, k, na, nb);
            for c in seg_cells(sg, 0) {
                for id in self.grid.at(c) {
                    let (f, m) = self.segs[id as usize];
                    let (f, m) = (f as usize, m as usize);
                    if !self.alive[f] || part.contains(&f) {
                        continue;
                    }
                    let poly = self.poly(f);
                    let (fa, fb) = self.ends[f];
                    if seg_pair_clash(
                        sg,
                        tags,
                        (poly[m], poly[m + 1]),
                        seg_tags(poly, m, fa, fb),
                        None,
                    ) {
                        return false;
                    }
                }
            }
        }
        // 帯：path をたどり、pc を逆にたどって閉じる。帯の多角形は、外接矩形に入る点を調べるときに初めて作る
        let mut band: Vec<IPt> = path.to_vec();
        band.extend(pc.iter().rev().skip(1).take(pc.len().saturating_sub(2)));
        let poly = std::cell::OnceCell::new();
        let rect = GridRect::of(&band);
        debug_assert!(self.covers(&rect), "索引の範囲の外の帯");
        let inside = |p2: IPt| {
            poly.get_or_init(|| GridPolygon::new(&band))
                .winding(p2)
                .is_none_or(|w| w != 0)
        };
        // 2. 帯の中の頂点（帯の中と輪郭がかかるマスにある）
        for c in polygon_cells(&band) {
            for v in self.node_grid.at(c) {
                let v = v as usize;
                if self.deg[v] == 0 || v == na || v == nb || inner.contains(&v) {
                    continue;
                }
                let p = self.node_pos[v];
                if rect.contains2((2 * p.0, 2 * p.1)) && inside((2 * p.0, 2 * p.1)) {
                    return false;
                }
            }
        }
        // 3. 両端につながるほかの辺
        for v in [na, nb] {
            for f in self.edges_at(v) {
                if !self.alive[f] || part.contains(&f) {
                    continue;
                }
                let poly_f = self.poly(f);
                if poly_f.len() < 2 {
                    return false;
                }
                let (fa, _) = self.ends[f];
                let (p0, p1) = if fa == v {
                    (poly_f[0], poly_f[1])
                } else {
                    (poly_f[poly_f.len() - 1], poly_f[poly_f.len() - 2])
                };
                let mid2 = (p0.0 + p1.0, p0.1 + p1.1);
                if rect.contains2(mid2) && inside(mid2) {
                    return false;
                }
            }
        }
        true
    }
}

/// 番号 `0..n` ごとの値の並び。値はみな1つの並びに入れ、番号ごとの範囲を持つ（番号ごとに並びを持つと、作るたびに番号の
/// 数だけ小さな確保が起きる。`CellGrid` と同じ理由）
pub(super) struct Grouped<T> {
    /// 番号 `k` の値は `items[start[k]..start[k + 1]]`
    start: Vec<u32>,
    items: Vec<T>,
}

impl<T: Copy + Default> Grouped<T> {
    /// （番号, 値）の組 `pairs` を番号ごとに分けたもの（同じ番号の値は `pairs` の順）
    pub(super) fn new(n: usize, pairs: impl Iterator<Item = (usize, T)> + Clone) -> Self {
        // 番号ごとの数を `start[k + 1]` に数え、足し上げて範囲の始めにする。値を入れるときは `start[k]` を進めるので、入れ
        // 終わると `start[k]` は番号 `k` の終わり（＝ `k + 1` の始め）になる。それを1つずらして戻す
        let mut start = vec![0u32; n + 1];
        for (k, _) in pairs.clone() {
            start[k + 1] += 1;
        }
        for k in 0..n {
            start[k + 1] += start[k];
        }
        let mut items = vec![T::default(); start[n] as usize];
        for (k, v) in pairs {
            items[start[k] as usize] = v;
            start[k] += 1;
        }
        start.copy_within(0..n, 1);
        start[0] = 0;
        Self { start, items }
    }

    /// 番号 `k` の値（`k` が範囲の外なら空）
    pub(super) fn get(&self, k: usize) -> &[T] {
        match (self.start.get(k), self.start.get(k + 1)) {
            (Some(&a), Some(&b)) => &self.items[a as usize..b as usize],
            _ => &[],
        }
    }

    /// 番号の数
    pub(super) fn len(&self) -> usize {
        self.start.len().saturating_sub(1)
    }
}

/// 区間（2倍の座標の y の範囲）を、横の帯に分けた索引。帯の数は区間の数の平方根（上限 256。速さだけに関わる）。帯ごとの
/// 区間の番号はみな1つの並びに入れ、帯ごとの範囲を持つ（帯ごとに並びを持つと、作るたびに帯の数だけ小さな確保が起きる。
/// `CellGrid` と同じ理由）
pub(super) struct YBands {
    /// いちばん下の y
    y0: i64,
    /// 帯の高さ
    band_h: i64,
    /// 帯ごとの区間の番号（区間の順）
    bands: Grouped<u32>,
}

impl YBands {
    /// 区間の y の範囲 `spans`（小さい方, 大きい方）の索引（区間の番号は `spans` の順）
    pub(super) fn new(spans: impl ExactSizeIterator<Item = (i64, i64)> + Clone) -> Self {
        if spans.len() == 0 {
            return Self {
                y0: 0,
                band_h: 1,
                bands: Grouped::new(1, std::iter::empty()),
            };
        }
        let (y0, y1) = spans
            .clone()
            .fold((i64::MAX, i64::MIN), |(lo, hi), (a, b)| {
                (lo.min(a), hi.max(b))
            });
        let n = ((spans.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
        let band_h = ((y1 - y0) / n as i64 + 1).max(1);
        // 区間がかかる帯ごとに（帯, 区間の番号）
        let pairs = spans.enumerate().flat_map(move |(i, (lo, hi))| {
            let (k0, k1) = (((lo - y0) / band_h) as usize, ((hi - y0) / band_h) as usize);
            (k0.min(n - 1)..k1.min(n - 1) + 1).map(move |k| (k, i as u32))
        });
        Self {
            y0,
            band_h,
            bands: Grouped::new(n, pairs),
        }
    }

    /// 点の y（2倍の座標）の帯にかかる区間の番号
    pub(super) fn at(&self, y: i64) -> &[u32] {
        let k = (y - self.y0).div_euclid(self.band_h);
        if k < 0 {
            &[]
        } else {
            self.bands.get(k as usize)
        }
    }
}

/// 点 `p` から右へ伸ばした半直線が区間 `a`〜`b`（2倍の座標）を横切る向き：上へなら 1、下へなら −1。下の端は含め、上の
/// 端は含めない（半開区間）。点が区間の上にあれば横切らない（点を右上へごくわずかずらした所として数えるのと同じ）
pub(super) fn crossing(a: IPt, b: IPt, p: IPt) -> Option<i32> {
    if a.1 <= p.1 && b.1 > p.1 && orient(a, b, p) > 0 {
        Some(1)
    } else if b.1 <= p.1 && a.1 > p.1 && orient(a, b, p) < 0 {
        Some(-1)
    } else {
        None
    }
}

/// 右へ伸ばした半直線と区間 `a`〜`b`（2倍の座標）の交わり（`crossing`）を、印ごとの巻き数 `w` に足す。上へ横切れば左の
/// 印 `l` に +1・右の印 `r` に −1、下へ横切れば逆
pub(super) fn wind_into<L: Copy + PartialEq>(
    a: IPt,
    b: IPt,
    l: L,
    r: L,
    p: IPt,
    w: &mut Vec<(L, i32)>,
) {
    if let Some(d) = crossing(a, b, p) {
        add_winding(w, l, d);
        add_winding(w, r, -d);
    }
}

/// 印ごとの巻き数 `w` の、印 `lab` に `v` を足す（無ければ足す。`v` が 0 でも印を足す）
pub(super) fn add_winding<L: PartialEq>(w: &mut Vec<(L, i32)>, lab: L, v: i32) {
    match w.iter_mut().find(|x| x.0 == lab) {
        Some(x) => x.1 += v,
        None => w.push((lab, v)),
    }
}

/// 巻き数が正の印（印が閉じた境目を作っていれば、点のある所の印。無ければ `None`＝いちばん外）
pub(super) fn top_label<L: Copy>(w: &[(L, i32)]) -> Option<L> {
    w.iter().find(|x| x.1 > 0).map(|x| x.0)
}

/// 曲線の判定の形が入る、格子の点の矩形（制御点の外接矩形を、切り下げ〜切り上げ）
pub(super) fn reach_of(q: &QuadBez) -> GridRect {
    let lo = |a: f64, b: f64, c: f64| (a.min(b).min(c) * OVERLAY_GRID).floor() as i64;
    let hi = |a: f64, b: f64, c: f64| (a.max(b).max(c) * OVERLAY_GRID).ceil() as i64;
    GridRect {
        x0: lo(q.p0.x, q.p1.x, q.p2.x),
        y0: lo(q.p0.y, q.p1.y, q.p2.y),
        x1: hi(q.p0.x, q.p1.x, q.p2.x),
        y1: hi(q.p0.y, q.p1.y, q.p2.y),
    }
}

/// 点 `p`（2倍の座標）から右へ伸ばした半直線が、格子の点の矩形 `r` にかかるか
pub(super) fn ray_meets_row(r: GridRect, p: IPt) -> bool {
    2 * r.y0 <= p.1 && p.1 <= 2 * r.y1 && 2 * r.x1 >= p.0
}

/// 下から上へ並べた形（`MapInput::Stack`）の、点のある所の色：巻き数が奇数の形のうち、並びでいちばん上の形の色。
/// 区間（格子の点）ごとに、出どころの形の並びの番号を印にする。点での「巻き数が奇数の形」の集まりは、形ごとに1ビットの
/// 並びで持つ
pub(super) struct StackColors<'a> {
    shapes: &'a [&'a FillShape],
    /// 区間ごとの、出どころの形の並びの番号
    labels: Vec<Vec<u32>>,
    segs: LabeledSegs<u32>,
    /// ビットの並びの長さ（64 ビットの数がいくつか）
    words: usize,
}

impl<'a> StackColors<'a> {
    pub(super) fn new(
        shapes: &'a [&'a FillShape],
        ends: &[(IPt, IPt)],
        labels: Vec<Vec<u32>>,
    ) -> Self {
        let segs = LabeledSegs::new(
            ends.iter()
                .zip(&labels)
                .flat_map(|(&(a, b), ls)| ls.iter().map(move |&i| (a, b, i, i))),
        );
        Self {
            shapes,
            labels,
            segs,
            words: shapes.len().div_ceil(64),
        }
    }

    /// ビットの並び `set` の、区間 `k` の出どころの形の偶奇を変える（区間を横切る）
    pub(super) fn cross(&self, set: &mut [u64], k: usize) {
        for &i in &self.labels[k] {
            set[i as usize / 64] ^= 1 << (i % 64);
        }
    }

    /// 点 `p`（2倍の座標。区間の上なら、点を右上へごくわずかずらした所）で巻き数が奇数の形：右へ伸ばした半直線が
    /// 横切る数の偶奇
    pub(super) fn odd_at(&self, p: IPt) -> Vec<u64> {
        let mut set = vec![0u64; self.words];
        self.segs
            .crossings(p, |i, _, _| set[i as usize / 64] ^= 1 << (i % 64));
        set
    }

    /// 巻き数が奇数の形の集まり `set` の印（並びでいちばん上の形の色と番号。無ければ何も無い所）
    pub(super) fn label(&self, set: &[u64]) -> Label {
        match set.iter().enumerate().rev().find(|(_, w)| **w != 0) {
            Some((j, w)) => {
                let s = self.shapes[64 * j + 63 - w.leading_zeros() as usize];
                (Some(s.color), Some(s.shape_id))
            }
            None => (None, None),
        }
    }
}

/// 区間を左の面から右の面へ（`from_left`。でなければ右から左へ）またいだ巻き数 `w`：区間の出どころの辺ごとの左右の印
/// `sides`（区間の向きにそろえたもの）の、出る側の印を −1、入る側の印を +1（`propagate` の地図・輪郭の変形のまたぎ方）
pub(super) fn crossed<L: Copy + PartialEq>(
    sides: impl IntoIterator<Item = (L, L)>,
    w: &mut Vec<(L, i32)>,
    from_left: bool,
) {
    for (l, r) in sides {
        let (from, to) = if from_left { (l, r) } else { (r, l) };
        add_winding(w, from, -1);
        add_winding(w, to, 1);
    }
}

/// 組を分ける表 `parent` での、`x` の組の代表（いちばん小さい番号）
pub(super) fn dsu_find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let up = parent[parent[x as usize] as usize];
        parent[x as usize] = up;
        x = up;
    }
    x
}

/// 組を分ける表 `parent` で、`x` と `y` の組を1つにする
pub(super) fn dsu_union(parent: &mut [u32], x: usize, y: usize) {
    let (rx, ry) = (dsu_find(parent, x as u32), dsu_find(parent, y as u32));
    if rx != ry {
        parent[rx.max(ry) as usize] = rx.min(ry);
    }
}

/// 切った区間（端は `ends`）の面ごとの状態を、区間をまたぐたびに変えて、となりの面へ伝える。重ね合わせで、片方の地図の
/// 色（前の地図・重ねる地図・輪郭の変形の巻き数の変わり方・下から上へ並べた形の偶奇）を決めるのはこれ1つ（設計：
/// claude/重ね合わせの面の色と番号を伝えて決める設計_N-1003_N-2001_c467cbf.md）。
///
/// 面は、区間をたどってつながる向きのある区間（`next`。面の輪郭）。`cross(state, k, from_left)` は、区間 `k` を左の面から
/// 右の面へ（`from_left` でなければ右から左へ）またいだ状態にする。どの決まった面ともつながらない面だけ、その面の最初の
/// 向きのある区間の真ん中の点（2倍の座標で両端の和）で、点の状態 `state_at` を数える。その点は、点を右上へごくわずか
/// ずらした所の値になる（`crossing`）ので、区間が下向きか、横向きで右向きなら区間の左の面、ほかは右の面の値。
/// 戻り値は（向きのある区間（番号は 2 × 区間 + 左か）ごとの面の番号, 面ごとの状態）
pub(super) fn propagate<S: Clone>(
    ends: &[(IPt, IPt)],
    next: &dyn Fn((usize, bool)) -> Turn,
    state_at: &dyn Fn(IPt) -> S,
    cross: &dyn Fn(&mut S, usize, bool),
) -> (Vec<u32>, Vec<S>) {
    let n = ends.len();
    let mut parent: Vec<u32> = (0..2 * n as u32).collect();
    for h in 0..2 * n {
        if let Turn::Piece(k, fwd) = next((h / 2, h % 2 == 1)) {
            dsu_union(&mut parent, h, 2 * k + fwd as usize);
        }
    }
    // 面の番号と、面ごとの向きのある区間
    let mut face_of: Vec<u32> = vec![u32::MAX; 2 * n];
    let mut faces = 0u32;
    let face: Vec<u32> = (0..2 * n)
        .map(|h| {
            let r = dsu_find(&mut parent, h as u32) as usize;
            if face_of[r] == u32::MAX {
                face_of[r] = faces;
                faces += 1;
            }
            face_of[r]
        })
        .collect();
    let members = Grouped::new(
        faces as usize,
        face.iter()
            .enumerate()
            .map(|(h, &f)| (f as usize, h as u32)),
    );
    let mut states: Vec<Option<S>> = vec![None; faces as usize];
    let mut queue: Vec<u32> = Vec::new();
    for h0 in 0..2 * n {
        let f0 = face[h0] as usize;
        if states[f0].is_some() {
            continue;
        }
        let (k, (a, b)) = (h0 / 2, ends[h0 / 2]);
        let mut s = state_at((a.0 + b.0, a.1 + b.1));
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let mid_on_left = dy < 0 || (dy == 0 && dx > 0);
        if mid_on_left != (h0 % 2 == 1) {
            cross(&mut s, k, mid_on_left);
        }
        states[f0] = Some(s);
        queue.push(f0 as u32);
        while let Some(f) = queue.pop() {
            let f = f as usize;
            for &h in members.get(f) {
                let g = face[h as usize ^ 1] as usize;
                if states[g].is_none() {
                    let mut s = states[f].clone().expect("決まった面");
                    cross(&mut s, h as usize / 2, h % 2 == 1);
                    states[g] = Some(s);
                    queue.push(g as u32);
                }
            }
        }
    }
    (
        face,
        states
            .into_iter()
            .map(|s| s.expect("どの面も決まる"))
            .collect(),
    )
}

/// 左右に印（色・重ねる形の中か）のある格子の区間の集まり。点のある所の印を、整数で決める（`at`）
pub(super) struct LabeledSegs<L> {
    /// （始点, 終点, 左の印, 右の印）。座標は格子の点の2倍
    segs: Vec<(IPt, IPt, L, L)>,
    bands: YBands,
}

impl<L: Copy + PartialEq> LabeledSegs<L> {
    /// 区間（格子の点）と左右の印
    pub(super) fn new(items: impl Iterator<Item = (IPt, IPt, L, L)>) -> Self {
        let segs: Vec<(IPt, IPt, L, L)> = items
            .filter(|(a, b, ..)| a != b)
            .map(|(a, b, l, r)| ((2 * a.0, 2 * a.1), (2 * b.0, 2 * b.1), l, r))
            .collect();
        let bands = YBands::new(segs.iter().map(|(a, b, ..)| (a.1.min(b.1), a.1.max(b.1))));
        Self { segs, bands }
    }

    /// 点 `p`（2倍の座標）から右へ伸ばした半直線が横切る区間ごとに、`f(左の印, 右の印, 横切る向き)` を呼ぶ（`crossing`）
    pub(super) fn crossings(&self, p: IPt, mut f: impl FnMut(L, L, i32)) {
        for &i in self.bands.at(p.1) {
            let (a, b, l, r) = self.segs[i as usize];
            if let Some(d) = crossing(a, b, p) {
                f(l, r, d);
            }
        }
    }

    /// 点 `p`（2倍の座標）での巻き数を `w` に足す（`wind_into`）
    pub(super) fn winding_into(&self, p: IPt, w: &mut Vec<(L, i32)>) {
        self.crossings(p, |l, r, d| {
            add_winding(w, l, d);
            add_winding(w, r, -d);
        });
    }

    /// 点 `p`（2倍の座標）のある所の印（`top_label`）。区間の印が閉じた境目を作っていれば（正しい地図の色、i_overlay が
    /// 数えた塗り）、どの点でも1つに決まる。点が区間の上にあれば、点を右上へごくわずかずらした所の印になる
    pub(super) fn at(&self, p: IPt) -> Option<L> {
        let mut w = Vec::new();
        self.winding_into(p, &mut w);
        top_label(&w)
    }
}

/// 格子の多角形（点は2倍の座標）。帯に分けて、点の巻き数を整数で数える
pub(super) struct GridPolygon {
    /// （区間の始点, 終点）。座標は格子の点の2倍
    segs: Vec<(IPt, IPt)>,
    bands: YBands,
}

impl GridPolygon {
    /// 閉じた折れ線 `pts`（格子の点。最後の点から最初の点へ閉じる）
    pub(super) fn new(pts: &[IPt]) -> Self {
        let dbl = |p: IPt| (2 * p.0, 2 * p.1);
        let segs: Vec<(IPt, IPt)> = (0..pts.len())
            .map(|i| (dbl(pts[i]), dbl(pts[(i + 1) % pts.len()])))
            .filter(|(a, b)| a != b)
            .collect();
        let bands = YBands::new(segs.iter().map(|(a, b)| (a.1.min(b.1), a.1.max(b.1))));
        Self { segs, bands }
    }

    /// 点 `p`（2倍の座標）の巻き数。`p` が輪郭の上にあれば `None`
    pub(super) fn winding(&self, p: IPt) -> Option<i32> {
        let mut w = 0;
        for &i in self.bands.at(p.1) {
            let (a, b) = self.segs[i as usize];
            if on_seg(a, b, p) {
                return None;
            }
            w += crossing(a, b, p).unwrap_or(0);
        }
        Some(w)
    }
}

/// 格子の点の外接矩形
#[derive(Clone, Copy)]
pub(super) struct GridRect {
    pub(super) x0: i64,
    pub(super) y0: i64,
    pub(super) x1: i64,
    pub(super) y1: i64,
}

impl GridRect {
    pub(super) fn of(pts: &[IPt]) -> Self {
        pts.iter().fold(
            GridRect {
                x0: i64::MAX,
                y0: i64::MAX,
                x1: i64::MIN,
                y1: i64::MIN,
            },
            |r, p| GridRect {
                x0: r.x0.min(p.0),
                y0: r.y0.min(p.1),
                x1: r.x1.max(p.0),
                y1: r.y1.max(p.1),
            },
        )
    }

    pub(super) fn union(&self, o: &GridRect) -> GridRect {
        GridRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    pub(super) fn overlaps(&self, o: &GridRect) -> bool {
        self.x0 <= o.x1 && o.x0 <= self.x1 && self.y0 <= o.y1 && o.y0 <= self.y1
    }

    /// 2倍の座標の点 `p` が、矩形（端を含む）に入るか
    pub(super) fn contains2(&self, p: IPt) -> bool {
        p.0 >= 2 * self.x0 && p.0 <= 2 * self.x1 && p.1 >= 2 * self.y0 && p.1 <= 2 * self.y1
    }
}

// ---- 近さを調べる格子（08c15dd レビューの段階6。N-1004） ----
//
// 区間・曲線は、通るマスだけに入れ、通るマスだけで探す。手間は長さに比例し、形の大きさの2乗では増えない。マスの大きさは
// 1つ（`GRID_CELL`）で、格子は1つの作り（`CellGrid`）

/// 近さを調べる格子の1マス（8px。格子の点で数える）
pub(super) const GRID_CELL: i64 = 8 * OVERLAY_GRID as i64;

/// 点（格子の点）を含むマス
pub(super) fn cell_of(p: IPt) -> (i64, i64) {
    (p.0.div_euclid(GRID_CELL), p.1.div_euclid(GRID_CELL))
}

/// 区間 `a`–`b`（格子の点）を、どの向きにも `pad` 目だけ太らせたものがかかるマス。列ごとに、その列にかかる区間の y の
/// 範囲のマスをたどる。区間の上（太らせたなら `pad` 以内）のどの点も、その点を含むマスが返るので、触れる区間どうし
/// （`pad` 以内に近づく区間どうし）は、必ず同じマスを持つ
pub(super) fn seg_cells((a, b): (IPt, IPt), pad: i64) -> impl Iterator<Item = (i64, i64)> {
    let (a, b) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    // x（a.0 ≤ x ≤ b.0）での区間の y の切り捨て（`up` なら切り上げ）。格子の範囲（±65,536px）の点なら積は i64 に収まる
    // （i128 の割り算は遅い）。収まらないときだけ i128 で
    let y_at = move |x: i64, up: bool| -> i64 {
        if dx == 0 {
            return if up { a.1.max(b.1) } else { a.1.min(b.1) };
        }
        // dx > 0 なので、div_euclid は切り捨て
        let q = match (x - a.0).checked_mul(dy) {
            Some(n) => n.div_euclid(dx) + (up && n.rem_euclid(dx) != 0) as i64,
            None => {
                let (n, d) = ((x - a.0) as i128 * dy as i128, dx as i128);
                (if up {
                    -(-n).div_euclid(d)
                } else {
                    n.div_euclid(d)
                }) as i64
            }
        };
        a.1 + q
    };
    let c = GRID_CELL;
    ((a.0 - pad).div_euclid(c)..=(b.0 + pad).div_euclid(c)).flat_map(move |cx| {
        let (lo, hi) = ((cx * c - pad).max(a.0), ((cx + 1) * c + pad).min(b.0));
        let ylo = y_at(lo, false).min(y_at(hi, false));
        let yhi = y_at(lo, true).max(y_at(hi, true));
        ((ylo - pad).div_euclid(c)..=(yhi + pad).div_euclid(c)).map(move |cy| (cx, cy))
    })
}

/// 曲線 `q`（px）と、その判定の形を、`pad` 目だけ太らせたものがかかるマス。曲線を、弦とのずれが半マスほどになる数に
/// 等分し（`flat_count`）、弦を、ずれの分と `pad` だけ太らせてたどる
pub(super) fn quad_cells(q: QuadBez, pad: i64) -> impl Iterator<Item = (i64, i64)> {
    let tol = GRID_CELL as f64 / OVERLAY_GRID / 2.0;
    // 太らせる分：弦と曲線のずれ、判定の形（同じ等分の、`FLAT_TOL` の数）と曲線のずれ、両方の点の丸め（それぞれ
    // 対角線の半分まで）
    let dev = flat_dev(&q, flat_count(&q, tol)) + flat_dev(&q, flat_count(&q, FLAT_TOL));
    let pad = pad + (dev * OVERLAY_GRID).ceil() as i64 + 2;
    let mut pts = flat_points(q, tol).map(|(p, _)| gpt(p));
    let first = pts.next();
    pts.scan(first, |prev, b| prev.replace(b).map(|a| (a, b)))
        .flat_map(move |ab| seg_cells(ab, pad))
}

/// 多角形（格子の点）の中と輪郭がかかるマス：輪郭の区間がかかるマスを列ごとに集め、列の中では一番下から一番上までの
/// マス（細い帯なら、輪郭のマスとほとんど同じ）
pub(super) fn polygon_cells(pts: &[IPt]) -> impl Iterator<Item = (i64, i64)> {
    // 列は、点の x の範囲の列（区間がかかるマスの列は、その中にある）。列ごとの（一番下, 一番上）
    let r = GridRect::of(pts);
    let cx0 = r.x0.div_euclid(GRID_CELL);
    let cols = if pts.is_empty() {
        0
    } else {
        (r.x1.div_euclid(GRID_CELL) - cx0 + 1) as usize
    };
    let mut span: Vec<(i64, i64)> = vec![(i64::MAX, i64::MIN); cols];
    for k in 0..pts.len() {
        for (cx, cy) in seg_cells((pts[k], pts[(k + 1) % pts.len()]), 0) {
            let e = &mut span[(cx - cx0) as usize];
            *e = (e.0.min(cy), e.1.max(cy));
        }
    }
    span.into_iter()
        .enumerate()
        .flat_map(move |(i, (y0, y1))| (y0..=y1).map(move |cy| (cx0 + i as i64, cy)))
}

/// 近さを調べる格子：マスごとの番号の一覧。番号はみな1つの並び（`items`）に入れ、マスごとに入れた順につなぐ（マスごとに
/// 並びを持つと、入れるたびに小さな確保と伸ばし直しが起き、それが重ね合わせの手間の大きな部分になる）
pub(super) struct CellGrid {
    /// マスごとの、最初と最後に入れた番号の位置（`items` の）
    cells: IntMap<(i64, i64), (u32, u32)>,
    /// 入れた番号と、同じマスで次に入れた番号の位置（無ければ `CELL_END`）
    items: Vec<(u32, u32)>,
    /// 番号ごとに、最後に返した `query` の回（同じ番号を2度返さないため）
    seen: Vec<u32>,
    stamp: u32,
}

/// `CellGrid::items` の、マスの終わりの印
const CELL_END: u32 = u32::MAX;

impl CellGrid {
    pub(super) fn new() -> Self {
        Self {
            cells: IntMap::default(),
            items: Vec::new(),
            seen: Vec::new(),
            stamp: 0,
        }
    }

    /// 番号 `id` を、マス `cells` に入れる
    pub(super) fn insert(&mut self, id: usize, cells: impl IntoIterator<Item = (i64, i64)>) {
        for c in cells {
            let at = self.items.len() as u32;
            self.items.push((id as u32, CELL_END));
            match self.cells.entry(c) {
                std::collections::hash_map::Entry::Occupied(mut o) => {
                    let last = std::mem::replace(&mut o.get_mut().1, at);
                    self.items[last as usize].1 = at;
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert((at, at));
                }
            }
        }
        if self.seen.len() <= id {
            self.seen.resize(id + 1, 0);
        }
    }

    /// マス `c` の番号（入れた順）
    pub(super) fn at(&self, c: (i64, i64)) -> impl Iterator<Item = u32> + '_ {
        let mut k = self.cells.get(&c).map_or(CELL_END, |&(first, _)| first);
        std::iter::from_fn(move || {
            (k != CELL_END).then(|| {
                let (id, next) = self.items[k as usize];
                k = next;
                id
            })
        })
    }

    /// マス `cells` のどれかに入っている番号（1つずつ）
    pub(super) fn query(
        &mut self,
        cells: impl IntoIterator<Item = (i64, i64)>,
        out: &mut Vec<usize>,
    ) {
        out.clear();
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.fill(0);
            self.stamp = 1;
        }
        for c in cells {
            let mut k = self.cells.get(&c).map_or(CELL_END, |&(first, _)| first);
            while k != CELL_END {
                let (id, next) = self.items[k as usize];
                if self.seen[id as usize] != self.stamp {
                    self.seen[id as usize] = self.stamp;
                    out.push(id as usize);
                }
                k = next;
            }
        }
    }
}

/// 面の輪郭1つ：判定の形の点・面積の2倍（外周は正、穴は負）・外接矩形
pub(super) struct Ring {
    pub(super) pts: Vec<IPt>,
    pub(super) area2: i128,
    pub(super) rect: GridRect,
    /// 点の内外を調べる多角形（要るときに作る）
    pub(super) poly: std::cell::OnceCell<GridPolygon>,
}

impl Ring {
    pub(super) fn new(polys: &[Vec<IPt>], cycle: &[(usize, bool)]) -> Self {
        let pts = cycle_points(polys, cycle);
        Self {
            area2: area2_of(&pts),
            rect: GridRect::of(&pts),
            pts,
            poly: std::cell::OnceCell::new(),
        }
    }

    /// 点 `q`（2倍の座標）を中に含むか（巻き数が 0 でない）。輪郭の上なら `None`。面の輪郭の内外の判定はこれ1つ
    pub(super) fn contains(&self, q: IPt) -> Option<bool> {
        if !self.rect.contains2(q) {
            return Some(false);
        }
        self.poly
            .get_or_init(|| GridPolygon::new(&self.pts))
            .winding(q)
            .map(|w| w != 0)
    }
}

/// 面の輪郭を、外周と、その中の穴の組に分けたもの（`PlanarMap::regions`）
pub(super) struct Regions {
    /// 使う輪郭（壊れた輪郭は入れない）
    pub(super) cycles: Vec<Vec<(usize, bool)>>,
    /// 輪郭ごとの `Ring`
    pub(super) rings: Vec<Ring>,
    /// 組ごとの（色, 輪郭の番号の並び（外周が先、穴はその後））。組の順は、外周の輪郭の順
    pub(super) groups: Vec<(Option<Color32>, Vec<usize>)>,
    /// 閉じなかった輪郭・色がまざった輪郭・入れ先の無い色のある穴の数（壊れていれば 0 より大きい）
    pub(super) broken: usize,
    /// 組の外周の外接矩形（2倍の座標）を、高さの帯に分けた索引（`groups_at` が引く）
    pub(super) bands: YBands,
}

impl Regions {
    /// 点 `q`（2倍の座標）を含みうる組：外周の外接矩形の高さが `q` にかかる組（組の番号の順）。点を含む組を探すのは
    /// これ1つ（穴の入れ先 `regions`、バケツの点の面 `with_face_filled`、何も無い側の輪郭の確かめ `valid_faces_since`）。
    /// どの配置でも、点の帯にかかる組だけを調べるので、組の数の2乗では増えない（レビュー 6ee325a の N-1001）
    pub(super) fn groups_at(&self, q: IPt) -> impl Iterator<Item = usize> + '_ {
        self.bands.at(q.1).iter().map(|&g| g as usize)
    }

    /// 組 `g` が点 `q`（2倍の座標）を中に含むか：外周の中で、どの穴の外か。組の輪郭の上なら `None`
    pub(super) fn contains(&self, g: usize, q: IPt) -> Option<bool> {
        let ids = &self.groups[g].1;
        if !self.rings[ids[0]].contains(q)? {
            return Some(false);
        }
        let mut inside = true;
        for &k in &ids[1..] {
            if self.rings[k].contains(q)? {
                inside = false;
            }
        }
        Some(inside)
    }

    /// 色のある組を面にする（`srcs` は、面の辺の側に記された元の図形）
    pub(super) fn into_faces(self, map: &PlanarMap) -> Vec<MapFace> {
        let mut cycles: Vec<Option<Vec<(usize, bool)>>> =
            self.cycles.into_iter().map(Some).collect();
        self.groups
            .into_iter()
            .filter_map(|(color, ids)| {
                let color = color?;
                let cycles: Vec<Vec<(usize, bool)>> =
                    ids.iter().filter_map(|&k| cycles[k].take()).collect();
                let srcs = map.face_srcs(&cycles);
                Some(MapFace {
                    color,
                    cycles,
                    srcs,
                })
            })
            .collect()
    }
}

/// 向きのある辺 `h` の判定の形の点（向きどおりの順）
pub(super) fn half_points(
    polys: &[Vec<IPt>],
    (i, forward): (usize, bool),
) -> impl Iterator<Item = IPt> + '_ {
    let p = &polys[i];
    let n = p.len();
    (0..n).map(move |k| if forward { p[k] } else { p[n - 1 - k] })
}

/// 面の輪郭 `cycle` の判定の形の点（閉じた折れ線。最後の点から最初の点へ閉じる）
pub(super) fn cycle_points(polys: &[Vec<IPt>], cycle: &[(usize, bool)]) -> Vec<IPt> {
    let mut out = Vec::new();
    for &h in cycle {
        let mut it = half_points(polys, h).peekable();
        while let Some(p) = it.next() {
            // 終点は、次の向きのある辺の始点と同じ
            if it.peek().is_some() {
                out.push(p);
            }
        }
    }
    out
}

/// 閉じた折れ線の面積の2倍（格子の目の2乗。符号は面積の式の向き）
pub(super) fn area2_of(pts: &[IPt]) -> i128 {
    (0..pts.len())
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
            p.0 as i128 * q.1 as i128 - q.0 as i128 * p.1 as i128
        })
        .sum()
}

/// 輪の区間の並び `segs` を、始点の座標がいちばん小さい区間から始まるように回したもの（同じ点から始まる所が2つ以上
/// あれば、そこからの始点の並びがいちばん小さいもの）
pub(super) fn from_lowest_point(mut segs: Vec<QuadBez>) -> Vec<QuadBez> {
    let n = segs.len();
    let start = (0..n)
        .min_by(|&i, &j| {
            (0..n)
                .map(|k| point_order(segs[(i + k) % n].p0, segs[(j + k) % n].p0))
                .find(|o| o.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    segs.rotate_left(start);
    segs
}

/// 点の、座標の小さい順（x、同じなら y）
pub(crate) fn point_order(p: Point, q: Point) -> std::cmp::Ordering {
    p.x.total_cmp(&q.x).then_with(|| p.y.total_cmp(&q.y))
}

/// 輪の、始点の並びの小さい順
pub(super) fn starts_order(a: &[QuadBez], b: &[QuadBez]) -> std::cmp::Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| point_order(x.p0, y.p0))
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.len().cmp(&b.len()))
}

/// 向きのある辺 `h` の判定の形の最初の区間の真ん中の点（2倍の座標）
pub(super) fn half_mid2(polys: &[Vec<IPt>], h: (usize, bool)) -> IPt {
    let mut it = half_points(polys, h);
    let a = it.next().unwrap_or((0, 0));
    let b = it.next().unwrap_or(a);
    (a.0 + b.0, a.1 + b.1)
}

/// テスト用：図形 `s` の点（端と制御点）がみな格子に置けるか（`on_grid`。図形から地図を作る下ごしらえ）
#[cfg(test)]
pub(super) fn shape_on_grid(s: &FillShape) -> bool {
    on_grid(
        s.contours()
            .flat_map(|c| c.segments.iter().flat_map(|q| [q.p0, q.p1, q.p2])),
    )
}

/// 点 `pts` がみな地図の格子に置けるか（±65,536px の内で、有限：`to_grid`）。地図にできる形かは、どこでもこれで決める
pub(crate) fn on_grid(mut pts: impl Iterator<Item = Point>) -> bool {
    pts.all(|p| to_grid(p).is_some())
}

/// 点を、重ねるときの格子の点にする。格子に置けない（±65,536px の外）ときは `None`
pub(super) fn to_grid(p: Point) -> Option<i_overlay9::i_float::int::point::IntPoint<i32>> {
    pub(super) const LIMIT: u64 = 1 << 28;
    let (x, y) = gpt(p);
    // 有限でない座標は `gpt` で数にならない（NaN は 0 になる）ので、先に断る
    (p.x.is_finite() && p.y.is_finite() && x.unsigned_abs() < LIMIT && y.unsigned_abs() < LIMIT)
        .then(|| i_overlay9::i_float::int::point::IntPoint::new(x as i32, y as i32))
}

/// 格子の点の座標
pub(super) fn from_grid(x: i64, y: i64) -> Point {
    Point::new(x as f64 / OVERLAY_GRID, y as f64 / OVERLAY_GRID)
}

/// 点を格子の点に丸めた座標（格子に置けない点はそのまま。その点を使う重ね合わせは `to_grid` で断る）
pub(super) fn grid_round(p: Point) -> Point {
    to_grid(p).map_or(p, |ip| from_grid(ip.x as i64, ip.y as i64))
}
