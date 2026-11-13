//! 同じレイヤー・同じ階層の生の図形を、境目の線（辺）で持つ地図。セル画の中身そのもので、描く・消す・塗る・選ぶ・
//! 動かす・貼る・輪郭の変形など、生の図形を変える編集はみな、この地図を受け取って地図を返す。
//! Adobe Animate のマージ描画と同じく、辺が「曲線・左の色・右の色」を持つ。境目は1本で、重ね順は無い。
//! 塗りの形（`FillShape`）は、辺から作って描画・当たり判定に使う。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use eframe::egui::Color32;
use kurbo::{Affine, ParamCurve, Point, QuadBez, Rect as KRect, Shape as KurboShape};

use crate::stored_map::StoredMap;
use crate::vector_engine::{nearest_on_quad, rects_overlap, Contour, FillShape, SyncCache};
use crate::vector_engine::{LayerId, ShapeId};

mod grid;
mod overlay;
pub(crate) use self::grid::*;
pub(crate) use self::overlay::*;

/// 辺の（左の色, 右の色）
type Sides = (Option<Color32>, Option<Color32>);

/// 面の印：色と、その面の図形の番号（重ね合わせで、色と一緒に伝える）
type Label = (Option<Color32>, Option<ShapeId>);

/// 辺。a → b に進んだときの左（面積の式で正の側）の色と、右の色を持つ。`None` は何も無い所。
/// `left_src`・`right_src` は、その側の面の図形の番号（セル画の地図は面ごとに1つずつ別の番号を記してしまう：
/// `CheckedMap::uniquely_labeled`。重ね合わせでは、どの図形から来たかを運ぶ）
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MapEdge {
    pub(crate) a: usize,
    pub(crate) b: usize,
    pub(crate) ctrl: Point,
    pub(crate) left: Option<Color32>,
    pub(crate) right: Option<Color32>,
    pub(crate) left_src: Option<ShapeId>,
    pub(crate) right_src: Option<ShapeId>,
}

impl MapEdge {
    /// `start` なら頂点 `v` から始まる向き、そうでなければ `v` で終わる向きにした写し（逆にするときは左右も入れ替える）
    fn oriented(self, start: bool, v: usize) -> MapEdge {
        let ok = if start { self.a == v } else { self.b == v };
        if ok {
            self
        } else {
            MapEdge {
                a: self.b,
                b: self.a,
                left: self.right,
                right: self.left,
                left_src: self.right_src,
                right_src: self.left_src,
                ..self
            }
        }
    }
}

/// 同じレイヤー・同じ階層の生の図形を、頂点と辺で持つ（Adobe Animate と同じ持ち方）
#[derive(Clone, Debug, Default)]
pub(crate) struct PlanarMap {
    pub(crate) nodes: Vec<Point>,
    pub(crate) edges: Vec<MapEdge>,
}

/// 面。外周の輪郭と穴の輪郭（向きのある辺の輪）を持つ。`srcs` は、この面の辺の側に記された元の図形
#[derive(Clone, Debug)]
pub(crate) struct MapFace {
    pub(crate) color: Color32,
    pub(crate) cycles: Vec<Vec<(usize, bool)>>,
    pub(crate) srcs: Vec<ShapeId>,
}

/// 整数（頂点・辺・格子の番号）を鍵にする表のハッシュ。標準の SipHash は、頂点の格子のように何十万回も引くと重いので、
/// 掛け算1回で混ぜる軽いものを使う（鍵は外から来ない値なので、偏りを狙われる心配は無い）
#[derive(Default, Clone, Copy)]
struct IntHasher(u64);

impl std::hash::Hasher for IntHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64);
    }
    fn write_i32(&mut self, i: i32) {
        self.write_u64(i as u32 as u64);
    }
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
}

type IntMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<IntHasher>>;

#[cfg(test)]
struct Seg {
    a: usize,
    b: usize,
    q: QuadBez,
    color: Color32,
    src: ShapeId,
}

impl PlanarMap {
    /// 面の番号が `keep` にある面だけを残した地図（ほかの面の側は何も無いにし、両側とも何も無い辺と、辺の無い頂点を外す）。
    /// 正しい地図の面の一部はそのまま正しい地図で、残した面の輪郭も変わらない（頂点で次の辺を選ぶとき、残した面の
    /// 角の中に辺が増えることは無い）
    pub(crate) fn restricted(&self, keep: &HashSet<ShapeId>) -> PlanarMap {
        self.restricted_by(|id| keep.contains(&id))
    }

    /// 面の番号が `ids` にある面を外した地図（`restricted` の逆）
    pub(crate) fn without(&self, ids: &HashSet<ShapeId>) -> PlanarMap {
        self.restricted_by(|id| !ids.contains(&id))
    }

    /// 面の番号（辺の側に記したもの）がみな `keep` に合うか（`restricted_by` しても変わらないか）
    pub(crate) fn labels_within(&self, keep: impl Fn(ShapeId) -> bool) -> bool {
        self.labels().all(keep)
    }

    /// 面の番号（辺の側に記したもの）
    pub(crate) fn labels(&self) -> impl Iterator<Item = ShapeId> + '_ {
        self.edges
            .iter()
            .flat_map(|e| [e.left_src, e.right_src])
            .flatten()
    }

    /// 辺の側に記した図形の番号を、`f` が返す番号に付け替える（`None` を返す番号はそのまま。番号の付け替えはこれ1つ）
    pub(crate) fn relabel(&mut self, mut f: impl FnMut(ShapeId) -> Option<ShapeId>) {
        for e in &mut self.edges {
            for src in [&mut e.left_src, &mut e.right_src].into_iter().flatten() {
                if let Some(n) = f(*src) {
                    *src = n;
                }
            }
        }
    }

    /// 辺の側に記した図形の番号を、番号ごとにみな新しい番号にする（`relabel`。同じ番号は同じ新しい番号になる）
    pub(crate) fn renew_labels(&mut self) {
        let mut fresh: HashMap<ShapeId, ShapeId> = HashMap::new();
        self.relabel(|id| {
            Some(
                *fresh
                    .entry(id)
                    .or_insert_with(crate::vector_engine::next_shape_id),
            )
        });
    }

    /// 辺の曲線の外接矩形（面の図形の外接矩形を合わせたものと同じ）。辺が無ければ `None`
    pub(crate) fn bounding_box(&self) -> Option<KRect> {
        self.edges
            .iter()
            .map(|e| QuadBez::new(self.nodes[e.a], e.ctrl, self.nodes[e.b]).bounding_box())
            .reduce(|acc, r| acc.union(r))
    }

    /// 輪郭 `cycles` の辺の側に記された元の図形（小さい順、重なりなし）
    fn face_srcs(&self, cycles: &[Vec<(usize, bool)>]) -> Vec<ShapeId> {
        let mut srcs: Vec<ShapeId> = cycles
            .iter()
            .flatten()
            .filter_map(|h| self.side_src(*h))
            .collect();
        srcs.sort_unstable();
        srcs.dedup();
        srcs
    }

    /// 座標の変換 `xform` をかけた地図が、地図の格子に置けるか：変換が潰れず（`invertible`）、頂点と制御点がみな
    /// ±65,536px の内（`on_grid`）。地図を変換して置けるかの判定はこれ1つ（変形・貼り付けの事前の確かめ、`realized`）
    pub(crate) fn fits_grid(&self, xform: Affine) -> bool {
        crate::vector_engine::invertible(xform)
            && on_grid(
                self.nodes
                    .iter()
                    .chain(self.edges.iter().map(|e| &e.ctrl))
                    .map(|&p| xform * p),
            )
    }

    /// 面の番号が `keep` に合う面だけを残した地図（`restricted`）
    pub(crate) fn restricted_by(&self, keep: impl Fn(ShapeId) -> bool) -> PlanarMap {
        let mut index: Vec<usize> = vec![usize::MAX; self.nodes.len()];
        let mut nodes: Vec<Point> = Vec::new();
        let mut node = |v: usize| {
            if index[v] == usize::MAX {
                index[v] = nodes.len();
                nodes.push(self.nodes[v]);
            }
            index[v]
        };
        let mut edges = Vec::new();
        for e in &self.edges {
            let left_src = e.left_src.filter(|&id| keep(id));
            let right_src = e.right_src.filter(|&id| keep(id));
            if left_src.is_none() && right_src.is_none() {
                continue;
            }
            edges.push(MapEdge {
                a: node(e.a),
                b: node(e.b),
                ctrl: e.ctrl,
                left: e.left.filter(|_| left_src.is_some()),
                right: e.right.filter(|_| right_src.is_some()),
                left_src,
                right_src,
            });
        }
        PlanarMap { nodes, edges }
    }

    pub(crate) fn quad(&self, e: usize) -> QuadBez {
        let ed = &self.edges[e];
        QuadBez::new(self.nodes[ed.a], ed.ctrl, self.nodes[ed.b])
    }

    /// 生の図形の並びから作る。境目の2本の線を1本の辺にする。地図の面から書き出した図形は、境目の線を両側の図形で
    /// ビットまで同じに持つ（頂点も制御点も同じ点。変換をかけても両側に同じ計算がかかる）ので、頂点は座標がビットまで
    /// 同じ点を1つにし、境目は「両端が逆で制御点がビットまで同じ」2本の線を1本にする。そろわない図形（ずれた図形・重なった
    /// 図形）は、正しい地図にならないので見つかり（`is_valid`）、塗り重ねて作る（`LayerBase::of`）。本番は図形から地図を
    /// 作らないので、テストの下ごしらえだけで使う
    #[cfg(test)]
    pub(crate) fn from_faces<'a>(faces: impl IntoIterator<Item = &'a FillShape>) -> Self {
        let mut nodes: Vec<Point> = Vec::new();
        let mut index: HashMap<[u64; 2], usize> = HashMap::new();
        let mut node = |p: Point| {
            *index
                .entry([p.x.to_bits(), p.y.to_bits()])
                .or_insert_with(|| {
                    nodes.push(p);
                    nodes.len() - 1
                })
        };
        // 1. 向きのある区間（左が面の色）。端を頂点にまとめる
        let mut segs: Vec<Seg> = Vec::new();
        for f in faces {
            for c in f.contours() {
                for q in c.segments.iter() {
                    let (a, b) = (node(q.p0), node(q.p2));
                    if a != b {
                        segs.push(Seg {
                            a,
                            b,
                            q: *q,
                            color: f.color,
                            src: f.shape_id,
                        });
                    }
                }
            }
        }
        // 区間の途中にある頂点では切らない（地図から書き出した図形では、頂点はいつも区間の端にある。前の版の図形で、
        // 区間の途中に頂点があれば、判定の形で辺が頂点に触れるので正しい地図にならず、塗り重ねて作り直す）
        let split = segs;
        // 2. 同じ2頂点を結び、向きが逆で制御点が同じ区間を、1本の辺にまとめる
        let mut groups: IntMap<(usize, usize), Vec<usize>> = IntMap::default();
        for (i, s) in split.iter().enumerate() {
            groups
                .entry((s.a.min(s.b), s.a.max(s.b)))
                .or_default()
                .push(i);
        }
        let mut used = vec![false; split.len()];
        let mut edges = Vec::new();
        for i in 0..split.len() {
            if used[i] {
                continue;
            }
            used[i] = true;
            let s = &split[i];
            let ctrl = [s.q.p1.x.to_bits(), s.q.p1.y.to_bits()];
            let twin = groups[&(s.a.min(s.b), s.a.max(s.b))]
                .iter()
                .copied()
                .find(|&j| {
                    !used[j]
                        && split[j].a == s.b
                        && split[j].b == s.a
                        && [split[j].q.p1.x.to_bits(), split[j].q.p1.y.to_bits()] == ctrl
                });
            let (right, right_src) = match twin {
                Some(j) => {
                    used[j] = true;
                    (Some(split[j].color), Some(split[j].src))
                }
                None => (None, None),
            };
            if right == Some(s.color) {
                continue;
            }
            edges.push(MapEdge {
                a: s.a,
                b: s.b,
                ctrl: s.q.p1,
                left: Some(s.color),
                right,
                left_src: Some(s.src),
                right_src,
            });
        }
        Self { nodes, edges }
    }

    /// 向きのある辺の左側の色
    fn side_color(&self, (i, forward): (usize, bool)) -> Option<Color32> {
        if forward {
            self.edges[i].left
        } else {
            self.edges[i].right
        }
    }

    /// 向きのある辺の左側の元の図形
    fn side_src(&self, (i, forward): (usize, bool)) -> Option<ShapeId> {
        if forward {
            self.edges[i].left_src
        } else {
            self.edges[i].right_src
        }
    }

    /// 正しい地図か（判定の形＝格子の上の折れ線で、整数で決める。設計 段階2 の 2-2）：
    /// 折れ線どうしが共有する頂点のほかで触れず、1本の折れ線が自分と触れず、別々の頂点が別々の格子の点に丸まり、
    /// 面の輪郭が閉じて色がそろい、色のある穴に外周があり（`faces` の壊れた数が 0）、何も無い側の輪郭が色のある面の
    /// 中に入り込んでいない（重なった図形をそのまま地図にしたものでない）。テスト用：本番は `checked`・`valid_faces`
    #[cfg(test)]
    pub(crate) fn is_valid(&self) -> bool {
        self.valid_faces().is_some()
    }

    /// 正しい地図なら（`is_valid`）、その面（`faces`）。正しい地図かの判定はどこでもこれ1つ（書き戻し・読み込み・変換
    /// した地図・テスト）。`None`：正しい地図でない
    pub(crate) fn valid_faces(&self) -> Option<Vec<MapFace>> {
        // 頂点と制御点が格子に置ける（±65,536px の内で有限：`fits_grid`）こと、両側が同じ色の辺が無いことを先に確かめる
        // （格子の外の座標で、判定の形を作らない）
        if !self.fits_grid(Affine::IDENTITY) || self.edges.iter().any(|e| e.left == e.right) {
            return None;
        }
        let polys = self.polys();
        if !self.polys_apart(&polys) {
            return None;
        }
        let cycles = self.face_cycles_with(&polys);
        let regions = self.regions(&polys, &cycles, false);
        if regions.broken > 0 {
            return None;
        }
        // 何も無い側の輪郭：その最初の区間の真ん中の点が、その辺を輪郭に持たない色のある面の中（外周の中で、どの穴の外：
        // `Regions::contains`）にあるか、その面の輪郭の上にあれば、入り込んでいる
        let mut edge_faces: Vec<[usize; 2]> = vec![[usize::MAX; 2]; self.edges.len()];
        for (g, (_, ids)) in regions.groups.iter().enumerate() {
            for h in ids.iter().flat_map(|&k| &regions.cycles[k]) {
                let slot = &mut edge_faces[h.0];
                if slot[0] == usize::MAX {
                    slot[0] = g;
                } else {
                    slot[1] = g;
                }
            }
        }
        // 面の外周の外接矩形を、高さの帯に分けた索引（2倍の座標。大きい面でも、入れる帯の数は帯の数まで）
        let (fy0, fband_h, fbands) = y_bands(regions.groups.iter().map(|(_, ids)| {
            let r = regions.rings[ids[0]].rect;
            (2 * r.y0, 2 * r.y1)
        }));
        for c in &cycles {
            if c.is_empty() || self.side_color(c[0]).is_some() {
                continue;
            }
            let h = c[0];
            let probe = half_mid2(&polys, h);
            for &g in band_at(fy0, fband_h, &fbands, probe.1) {
                let g = g as usize;
                if !edge_faces[h.0].contains(&g) && regions.contains(g, probe) != Some(false) {
                    return None;
                }
            }
        }
        Some(regions.into_faces(self))
    }

    /// テスト用：正しくないとき、どこがだめか
    #[cfg(test)]
    pub(crate) fn validity_report(&self) -> String {
        let polys = self.polys();
        let mut out = String::new();
        for (e, p) in polys.iter().enumerate() {
            if p.len() < 2 {
                out += &format!("degenerate edge {e}; ");
            }
        }
        let segs: Vec<(usize, usize)> = polys
            .iter()
            .enumerate()
            .flat_map(|(e, p)| (0..p.len().saturating_sub(1)).map(move |k| (e, k)))
            .collect();
        let mut n = 0;
        for i in 0..segs.len() {
            for j in i + 1..segs.len() {
                if n < 6 && self.segs_clash(&polys, segs[i], segs[j]) {
                    let (e, k) = segs[i];
                    let (f, m) = segs[j];
                    out += &format!(
                        "clash e{e}[{k}] {:?}-{:?} (nodes {} {}, len {}) vs e{f}[{m}] {:?}-{:?} (nodes {} {}, len {}); ",
                        polys[e][k], polys[e][k + 1], self.edges[e].a, self.edges[e].b, polys[e].len(),
                        polys[f][m], polys[f][m + 1], self.edges[f].a, self.edges[f].b, polys[f].len()
                    );
                    n += 1;
                }
            }
        }
        let cycles = self.face_cycles_with(&polys);
        let (faces, broken) = self.faces_of(&polys, &cycles);
        out += &format!("broken {broken}");
        // 入れ先の無い穴（色のある穴で、同じ色の外周に入らなかったもの）
        for c in &cycles {
            if self.is_broken_cycle(c) || self.side_color(c[0]).is_none() {
                continue;
            }
            let pts = cycle_points(&polys, c);
            if area2_of(&pts) < 0 && !faces.iter().any(|f| f.cycles.contains(c)) {
                out += &format!(
                    "\n  hole without exterior: color {:?}, len {}, first e{}{} at {:?}",
                    self.side_color(c[0]).map(|c| format!(
                        "{:02x}{:02x}{:02x}",
                        c.r(),
                        c.g(),
                        c.b()
                    )),
                    c.len(),
                    c[0].0,
                    if c[0].1 { "+" } else { "-" },
                    pts.first()
                );
            }
        }
        for c in &cycles {
            if self.is_broken_cycle(c) {
                out += &format!("\n  broken cycle len {}: ", c.len());
                for h in c.iter().take(30) {
                    if h.0 == usize::MAX {
                        out += "OPEN ";
                        continue;
                    }
                    let ed = self.edges[h.0];
                    let straight = ed.ctrl == self.nodes[ed.a].midpoint(self.nodes[ed.b]);
                    out += &format!(
                        "e{}{}({}->{}, {:?}, {}, {}) ",
                        h.0,
                        if h.1 { "+" } else { "-" },
                        if h.1 { ed.a } else { ed.b },
                        if h.1 { ed.b } else { ed.a },
                        self.side_color(*h).map(|c| format!(
                            "{:02x}{:02x}{:02x}",
                            c.r(),
                            c.g(),
                            c.b()
                        )),
                        if straight { "S" } else { "C" },
                        polys[h.0].len()
                    );
                }
            }
        }
        out
    }

    /// 全部の辺の判定の形（格子の上の折れ線）
    fn polys(&self) -> Vec<Vec<IPt>> {
        (0..self.edges.len())
            .map(|e| grid_poly(&self.quad(e)))
            .collect()
    }

    /// 判定の形の 1〜3（設計 2-2）：折れ線どうしが共有する頂点のほかで触れない・1本の折れ線が自分と触れない・
    /// 別々の頂点が別々の格子の点に丸まる
    fn polys_apart(&self, polys: &[Vec<IPt>]) -> bool {
        let mut deg = vec![0usize; self.nodes.len()];
        for (e, ed) in self.edges.iter().enumerate() {
            if polys[e].len() < 2 {
                return false;
            }
            deg[ed.a] += 1;
            deg[ed.b] += 1;
        }
        let mut node_at: IntMap<IPt, usize> = IntMap::default();
        for (v, p) in self.nodes.iter().enumerate() {
            if deg[v] > 0 && node_at.insert(gpt(*p), v).is_some_and(|w| w != v) {
                return false;
            }
        }
        let segs: Vec<(usize, usize)> = polys
            .iter()
            .enumerate()
            .flat_map(|(e, p)| (0..p.len() - 1).map(move |k| (e, k)))
            .collect();
        let rects: Vec<GridRect> = segs
            .iter()
            .map(|&(e, k)| GridRect::of(&polys[e][k..k + 2]))
            .collect();
        // 区間は、通るマスだけに入れる（`seg_cells`。近さを調べる格子は `CellGrid` 1つ）。触れる区間どうしは必ず同じマスを
        // 持つので、前に入れた区間のうち、同じマスにある区間とだけ比べる
        let mut grid = CellGrid::new();
        let mut near: Vec<usize> = Vec::new();
        for (i, &(e, k)) in segs.iter().enumerate() {
            let cells: Vec<(i64, i64)> = seg_cells((polys[e][k], polys[e][k + 1]), 0).collect();
            grid.query(cells.iter().copied(), &mut near);
            for &j in &near {
                if rects[i].overlaps(&rects[j]) && self.segs_clash(polys, segs[j], segs[i]) {
                    return false;
                }
            }
            grid.insert(i, cells);
        }
        true
    }

    /// 辺 `e` の区間 `k` と、辺 `f` の区間 `m`（`(e, k)` が前）が、許されない所で触れるか（`seg_pair_clash`。1本の辺の
    /// となり合う区間は、そのつなぎ目を共有してよい）
    fn segs_clash(
        &self,
        polys: &[Vec<IPt>],
        (e, k): (usize, usize),
        (f, m): (usize, usize),
    ) -> bool {
        let (pe, pf) = (&polys[e], &polys[f]);
        let (ee, ef) = (self.edges[e], self.edges[f]);
        seg_pair_clash(
            (pe[k], pe[k + 1]),
            seg_tags(pe, k, ee.a, ee.b),
            (pf[m], pf[m + 1]),
            seg_tags(pf, m, ef.a, ef.b),
            (e == f && m == k + 1).then_some(pe[k + 1]),
        )
    }

    /// 面を取り出す。外周（面積が正）に、それを含むいちばん小さい同じ色の外周の穴（面積が負）を入れる。
    /// 戻り値の2つ目は、閉じなかった輪郭・色がまざった輪郭・入れ先の無い穴の数（壊れていれば 0 より大きい）
    pub(crate) fn faces(&self) -> (Vec<MapFace>, usize) {
        let polys = self.polys();
        self.faces_of(&polys, &self.face_cycles_with(&polys))
    }

    /// `faces` と同じ。判定の形 `polys` と面の輪郭 `cycles`（`face_cycles_with` の結果）を先に求めてあるときに使う
    /// （`regions` の色のある組）
    fn faces_of(&self, polys: &[Vec<IPt>], cycles: &[Vec<(usize, bool)>]) -> (Vec<MapFace>, usize) {
        let regions = self.regions(polys, cycles, false);
        let broken = regions.broken;
        (regions.into_faces(self), broken)
    }

    /// 面の輪郭 `cycles`（`face_cycles_with`。判定の形 `polys`）を、外周（面積が正）と、その中の穴（面積が負）の組に分ける。
    /// 穴は、それを含む同じ色（色の無い所も1つの色とみなす）の外周のうち、いちばん小さいものに入れる。外周か穴かは判定の
    /// 形の面積の符号、穴の入れ先は判定の形での点の内外（`Ring::contains`）で、どちらも整数で決める。`unpainted`：色の無い
    /// 所（何も塗っていない、囲まれた所）の組も作るか。面の取り出し・正しい地図かの判定・バケツの、面の分け方はこれ1つ
    fn regions(
        &self,
        polys: &[Vec<IPt>],
        cycles: &[Vec<(usize, bool)>],
        unpainted: bool,
    ) -> Regions {
        let mut out = Regions {
            cycles: Vec::new(),
            rings: Vec::new(),
            groups: Vec::new(),
            broken: 0,
        };
        let mut holes: Vec<(Option<Color32>, usize)> = Vec::new();
        for cycle in cycles {
            if self.is_broken_cycle(cycle) {
                out.broken += 1;
                continue;
            }
            let color = self.side_color(cycle[0]);
            if color.is_none() && !unpainted {
                continue;
            }
            let k = out.rings.len();
            let ring = Ring::new(polys, cycle);
            if ring.area2 > 0 {
                out.groups.push((color, vec![k]));
            } else {
                holes.push((color, k));
            }
            out.rings.push(ring);
            out.cycles.push(cycle.clone());
        }
        // 小さい外周から順に調べ、最初に含むものを選ぶ。調べる点は、穴の最初の区間の真ん中（正しい地図では、ほかの輪郭の
        // 上に無い）
        let mut by_area: Vec<usize> = (0..out.groups.len()).collect();
        by_area.sort_by_key(|&g| out.rings[out.groups[g].1[0]].area2);
        for (color, k) in holes {
            let probe = half_mid2(polys, out.cycles[k][0]);
            let owner = by_area.iter().copied().find(|&g| {
                out.groups[g].0 == color
                    && out.rings[out.groups[g].1[0]].contains(probe) == Some(true)
            });
            match owner {
                Some(g) => out.groups[g].1.push(k),
                // 色のある穴に外周が無ければ壊れている。色の無い穴に外周が無いのは、いちばん外（囲まれていない所）の穴
                None if color.is_some() => out.broken += 1,
                None => {}
            }
        }
        out
    }

    /// 面 `f` の今の形を、塗りの形にする。輪郭は座標のいちばん小さい点から始め、穴はその点の順に並べる（形は地図の面
    /// だけで決まり、辺の並びや作り方（差分から作る・しまった地図から作り直す）によらない）
    pub(crate) fn face_shape(&self, f: &MapFace, layer_id: u64, group_path: &[u64]) -> FillShape {
        self.face_shape_with(f, layer_id, group_path, |_| None)
    }

    /// `face_shape` の、形の同じ輪郭を `reuse` が返す輪郭（前の図形の輪郭。同じ決まりで作ったもの）にするもの
    fn face_shape_with(
        &self,
        f: &MapFace,
        layer_id: u64,
        group_path: &[u64],
        reuse: impl Fn(&[(usize, bool)]) -> Option<Contour>,
    ) -> FillShape {
        let mut contours = f.cycles.iter().map(|c| {
            reuse(c).unwrap_or_else(|| {
                Contour::new(from_lowest_point(
                    c.iter().map(|h| self.half_quad(*h)).collect(),
                ))
            })
        });
        let exterior = contours.next().unwrap_or_else(|| Contour::new(Vec::new()));
        let mut holes: Vec<Contour> = contours.collect();
        holes.sort_by(|a, b| starts_order(&a.segments, &b.segments));
        FillShape::new(f.color, exterior, holes).with_scope(layer_id, group_path)
    }

    /// 辺から塗りの形を作る。戻り値の2つ目は、壊れた輪郭の数（`faces` と同じ）
    #[cfg(test)]
    pub(crate) fn to_faces_checked(
        &self,
        layer_id: u64,
        group_path: &[u64],
    ) -> (Vec<FillShape>, usize) {
        let (faces, broken) = self.faces();
        (
            faces
                .iter()
                .map(|f| self.face_shape(f, layer_id, group_path))
                .collect(),
            broken,
        )
    }

    /// 頂点 `v` につながる辺
    pub(crate) fn edges_at(&self, v: usize) -> Vec<usize> {
        (0..self.edges.len())
            .filter(|&e| self.edges[e].a == v || self.edges[e].b == v)
            .collect()
    }

    /// 頂点 `v` を `target` へ動かして確定した地図（重ね合わせ `MapRule::Sweep` で。`None`：格子に置けない座標がある）。
    /// つながる辺の制御点は、動いた量の半分だけ動かす（今の頂点の移動と同じ決まり）。`target` がほかの頂点と同じ座標なら
    /// （吸着）、重ね合わせが格子の同じ点を1つの頂点にするので、2つの頂点は1つになる
    pub(crate) fn commit_move_vertex(&self, v: usize, target: Point) -> Option<PlanarMap> {
        let d = target - self.nodes[v];
        let pos = |w: usize| if w == v { target } else { self.nodes[w] };
        let moved: Vec<MovedEdge> = self
            .edges_at(v)
            .into_iter()
            .map(|e| {
                let ed = self.edges[e];
                let k = (ed.a == v) as u8 + (ed.b == v) as u8;
                (
                    e,
                    QuadBez::new(pos(ed.a), ed.ctrl + d * (0.5 * k as f64), pos(ed.b)),
                )
            })
            .collect();
        self.overlay(MapInput::Sweep(&moved), MapRule::Sweep)
    }

    /// 辺 `e` の `grab_t` の所を `target` へ引っぱって確定した地図（重ね合わせ `MapRule::Sweep` で。`None`：格子に置けない
    /// 座標がある）。引っぱった曲線は、今の辺のつまみと同じ決まり（両端は動かない）
    pub(crate) fn commit_pull_edge(
        &self,
        e: usize,
        grab_t: f64,
        target: Point,
    ) -> Option<PlanarMap> {
        let q = self.quad(e);
        let ctrl = q.p1 + (target - q.eval(grab_t)) / (2.0 * grab_t * (1.0 - grab_t));
        self.overlay(
            MapInput::Sweep(&[(e, QuadBez::new(q.p0, ctrl, q.p2))]),
            MapRule::Sweep,
        )
    }

    /// 地図を座標の変換 `xform` で置いたとき、`p` から `r` 以内でいちばん近い頂点（距離は置いた座標で測る）
    pub(crate) fn hit_node(&self, p: Point, r: f64, xform: Affine) -> Option<usize> {
        (0..self.nodes.len())
            .map(|v| (v, (xform * self.nodes[v] - p).hypot()))
            .filter(|(_, d)| *d <= r)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(v, _)| v)
    }

    /// 地図を座標の変換 `xform` で置いたとき、`p` から `r` 以内でいちばん近い辺と、その上のパラメータ（距離は置いた座標で
    /// 測る。パラメータはアフィン変換で変わらない）
    pub(crate) fn hit_edge(&self, p: Point, r: f64, xform: Affine) -> Option<(usize, f64)> {
        let near = KRect::from_center_size(p, (2.0 * r, 2.0 * r));
        (0..self.edges.len())
            .map(|e| (e, xform * self.quad(e)))
            .filter(|(_, q)| rects_overlap(q.bounding_box(), near))
            .map(|(e, q)| {
                let (t, d) = nearest_on_quad(&q, p);
                (e, t, d)
            })
            .filter(|x| x.2 <= r)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(e, t, _)| (e, t))
    }
}

impl PlanarMap {
    /// 向きのある辺（辺, 前向きか）の曲線
    pub(crate) fn half_quad(&self, (i, forward): (usize, bool)) -> QuadBez {
        let q = self.quad(i);
        if forward {
            q
        } else {
            QuadBez::new(q.p2, q.p1, q.p0)
        }
    }

    /// 面の輪郭（左側が同じ面になる、向きのある辺の輪）をすべて取り出す。どの向きのある辺も、ちょうど1つの輪に入る。
    /// `polys` は辺の判定の形。頂点のまわりの辺の順番は、判定の形の、頂点から出る最初の区間の整数の向きで決める
    /// （正しい地図では、同じ向きの2本は無い）
    fn face_cycles_with(&self, polys: &[Vec<IPt>]) -> Vec<Vec<(usize, bool)>> {
        let idx = |h: (usize, bool)| 2 * h.0 + h.1 as usize;
        let dir = |h: (usize, bool)| -> IPt {
            let mut it = half_points(polys, h);
            let a = it.next().unwrap_or((0, 0));
            let b = it.next().unwrap_or(a);
            (b.0 - a.0, b.1 - a.1)
        };
        let mut out_of: Vec<Vec<(usize, bool)>> = vec![Vec::new(); self.nodes.len()];
        for (i, e) in self.edges.iter().enumerate() {
            out_of[e.a].push((i, true));
            out_of[e.b].push((i, false));
        }
        // 頂点ごとに、出ていく向きのある辺を角度の順に並べ、並びの中の位置を覚える
        let mut pos = vec![0usize; 2 * self.edges.len()];
        for list in &mut out_of {
            list.sort_by(|&x, &y| cmp_dir(wide(dir(x)), wide(dir(y))).then(idx(x).cmp(&idx(y))));
            for (k, &h) in list.iter().enumerate() {
                pos[idx(h)] = k;
            }
        }
        let head = |h: (usize, bool)| {
            if h.1 {
                self.edges[h.0].b
            } else {
                self.edges[h.0].a
            }
        };
        let mut used = vec![false; 2 * self.edges.len()];
        let mut cycles = Vec::new();
        for i in 0..self.edges.len() {
            for start in [(i, true), (i, false)] {
                if used[idx(start)] {
                    continue;
                }
                let mut cycle = Vec::new();
                let mut cur = start;
                loop {
                    used[idx(cur)] = true;
                    cycle.push(cur);
                    // 次は、入ってきた辺を引き返す向き（同じ辺の逆向き）から時計回りに最初の辺。
                    // 角度の順の並びで、引き返す向きの1つ前（ほかに辺が無いときは引き返す）
                    let list = &out_of[head(cur)];
                    let k = pos[idx((cur.0, !cur.1))];
                    let next = list[(k + list.len() - 1) % list.len()];
                    if next == start {
                        break;
                    }
                    if used[idx(next)] {
                        cycle.push((usize::MAX, false));
                        break;
                    }
                    cur = next;
                }
                cycles.push(cycle);
            }
        }
        cycles
    }
}

impl PlanarMap {
    /// 面の輪郭 `c`（`face_cycles_with` の1つ）が壊れている：閉じない、または左の色がそろわない。
    /// 壊れた輪郭の面は図形に書き戻せない
    fn is_broken_cycle(&self, c: &[(usize, bool)]) -> bool {
        c.iter().any(|h| h.0 == usize::MAX)
            || c.windows(2)
                .any(|w| self.side_color(w[0]) != self.side_color(w[1]))
    }
}

impl PlanarMap {
    /// 候補の頂点 `candidates` のうち、2本の辺だけがつながり、両側の色が同じで、2本を1本の曲線で表せるものを消す。
    /// 候補は、その操作で消えた線（左右が同じ色になって捨てた区間・辺）の端の頂点（`overlay`・`with_face_filled`）：
    /// 操作が作った切れ目（交わりで切った点・消えた境目の端）だけを消し、操作の前から2本だけだった頂点（足した角）は
    /// どの操作でも候補にならないので残る。切れ目の後始末の決まりはこれ1つ。
    /// 頂点は番号の小さい順に調べ、まとめたら、つながりの変わった両端の頂点だけを調べ直す（まとめた結果は、
    /// まとめるたびに最初から調べ直すのと同じ）。辺の並びは、まとめた辺を消すたびに最後の辺をそこへ移すのと同じ順にする
    pub(crate) fn merge_degree2(&mut self, candidates: impl IntoIterator<Item = usize>) {
        let mut is_cand = vec![false; self.nodes.len()];
        for v in candidates {
            is_cand[v] = true;
        }
        let n = self.edges.len();
        // 辺は元の番号のまま持ち、消した辺は印だけ付ける。`order` は今の並び（消すたびに最後の辺をそこへ移す）、
        // `pos` はその中の位置
        let mut edges = std::mem::take(&mut self.edges);
        let mut alive = vec![true; n];
        let mut order: Vec<usize> = (0..n).collect();
        let mut pos: Vec<usize> = (0..n).collect();
        let mut at: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        for (i, e) in edges.iter().enumerate() {
            at[e.a].push(i);
            if e.b != e.a {
                at[e.b].push(i);
            }
        }
        let candidate = |v: usize, at: &[Vec<usize>]| at[v].len() == 2 && is_cand[v];
        let mut pending: std::collections::BTreeSet<usize> = (0..self.nodes.len())
            .filter(|&v| candidate(v, &at))
            .collect();
        // 判定の形の索引（初めて要るときに作る）と、辺の番号 → 索引の番号。索引に入れる範囲は、まとめる候補の頂点に
        // つながる辺の外接矩形（まとめた辺が範囲から出るときは、広げて作り直す）
        let mut index: Option<PolyIndex> = None;
        let mut ix_id: Vec<usize> = (0..n).collect();
        let mut ix_area = pending
            .iter()
            .flat_map(|&v| at[v].iter())
            .map(|&i| {
                let (a, c, b) = (
                    self.nodes[edges[i].a],
                    edges[i].ctrl,
                    self.nodes[edges[i].b],
                );
                KRect::from_points(a, b).union_pt(c)
            })
            .reduce(|a, b| a.union(b))
            .unwrap_or(KRect::ZERO);
        // まとめると、判定の形でつながりが変わってしまう頂点（まとめない）
        let mut rejected = vec![false; self.nodes.len()];
        while let Some(v) = pending.pop_first() {
            if rejected[v] || !candidate(v, &at) {
                continue;
            }
            // 今の並びで先にある辺が i（v で終わる向き）、後ろが j（v から始まる向き）
            let (i, j) = if pos[at[v][0]] < pos[at[v][1]] {
                (at[v][0], at[v][1])
            } else {
                (at[v][1], at[v][0])
            };
            let (e1, e2) = (edges[i].oriented(false, v), edges[j].oriented(true, v));
            if e1.a == e2.b || e1.left != e2.left || e1.right != e2.right {
                continue;
            }
            let (q1, q2) = (
                QuadBez::new(self.nodes[e1.a], e1.ctrl, self.nodes[v]),
                QuadBez::new(self.nodes[v], e2.ctrl, self.nodes[e2.b]),
            );
            // 2本が、1本の2次ベジェ q を t で切ったものか（de Casteljau：q1.p1・v・q2.p1 が一直線に並び、v がその間を
            // t:(1−t) に分け、q の制御点 c は q1.p1 = (1−t)q1.p0 + t·c、q2.p1 = (1−t)c + t·q2.p2 を満たす）。頂点は
            // 地図の精度（`FLAT_TOL`）でしか曲線に乗っていないので、q を t で切った2本が q1・q2 と同じ曲線（`same_curve`）の
            // ときだけまとめる（絵の形は地図の精度の中でしか変わらない）。直線も同じ決まりでまとまる
            let (p0, a, w, b, p2) = (
                q1.p0.to_vec2(),
                q1.p1.to_vec2(),
                q1.p2.to_vec2(),
                q2.p1.to_vec2(),
                q2.p2.to_vec2(),
            );
            let ab = b - a;
            let t = (w - a).dot(ab) / ab.hypot2();
            if !(t > 0.0 && t < 1.0) {
                continue;
            }
            // 2つの式に最もよく合う制御点
            let c = (t * (a - (1.0 - t) * p0) + (1.0 - t) * (b - t * p2))
                / (t * t + (1.0 - t) * (1.0 - t));
            let q = QuadBez::new(q1.p0, c.to_point(), q2.p2);
            if !(same_curve(&q.subsegment(0.0..t), &q1) && same_curve(&q.subsegment(t..1.0), &q2)) {
                continue;
            }
            // 判定の形で、2本を1本に置き換えてよいか（設計 2-4 の3つの決まり。曲線が端を越えて折り返すときも、
            // 頂点のまわりの順番か自分との重なりで見つかる）
            let pc = grid_poly(&q);
            let ix = index.get_or_insert_with(|| {
                PolyIndex::build(&self.nodes, &edges, |f| alive[f], Some(ix_area))
            });
            let path_of = |ix: &PolyIndex, ix_id: &[usize]| -> Vec<IPt> {
                let (p1, p2) = (&ix.polys[ix_id[i]], &ix.polys[ix_id[j]]);
                let mut path: Vec<IPt> = if edges[i].b == v {
                    p1.clone()
                } else {
                    p1.iter().rev().copied().collect()
                };
                if edges[j].a == v {
                    path.extend(p2.iter().skip(1));
                } else {
                    path.extend(p2.iter().rev().skip(1));
                }
                path
            };
            let mut path = path_of(ix, &ix_id);
            let reach = GridRect::of(&path).union(&GridRect::of(&pc));
            if !ix.covers(&reach) {
                ix_area = ix_area.union(KRect::new(
                    reach.x0 as f64 / OVERLAY_GRID,
                    reach.y0 as f64 / OVERLAY_GRID,
                    reach.x1 as f64 / OVERLAY_GRID,
                    reach.y1 as f64 / OVERLAY_GRID,
                ));
                *ix = PolyIndex::build(&self.nodes, &edges, |f| alive[f], Some(ix_area));
                ix_id = (0..n).collect();
                path = path_of(ix, &ix_id);
            }
            if !ix.can_replace(&[ix_id[i], ix_id[j]], &path, &[v], &pc, e1.a, e2.b) {
                rejected[v] = true;
                continue;
            }
            ix.kill(ix_id[i]);
            ix.kill(ix_id[j]);
            ix_id[i] = ix.add_edge(e1.a, e2.b, pc);
            // まとめる：i を e1.a から e2.b への辺にし、j を消す
            edges[i] = MapEdge {
                a: e1.a,
                b: e2.b,
                ctrl: q.p1,
                ..e1
            };
            alive[j] = false;
            at[v].clear();
            for x in at[e2.b].iter_mut().filter(|x| **x == j) {
                *x = i;
            }
            let p = pos[j];
            let last = *order.last().expect("辺がある");
            order.swap_remove(p);
            if p < order.len() {
                pos[last] = p;
            }
            for w in [e1.a, e2.b] {
                if candidate(w, &at) && !rejected[w] {
                    pending.insert(w);
                }
            }
        }
        self.edges = order.iter().map(|&i| edges[i]).collect();
    }

    /// どの辺にもつながらない頂点を消して、番号を詰める
    fn compact_nodes(&mut self) {
        let mut used = vec![false; self.nodes.len()];
        for e in &self.edges {
            used[e.a] = true;
            used[e.b] = true;
        }
        let mut remap = vec![usize::MAX; self.nodes.len()];
        let mut nodes = Vec::new();
        for (v, p) in self.nodes.iter().enumerate() {
            if used[v] {
                remap[v] = nodes.len();
                nodes.push(*p);
            }
        }
        for e in &mut self.edges {
            e.a = remap[e.a];
            e.b = remap[e.b];
        }
        self.nodes = nodes;
    }
}

/// 辺の形が同じかを比べるための鍵（向きはそろえる）。色は入れない：辺のその側が変わっていないかは、側の元の図形の印で
/// 分かる（重ね合わせ・バケツは、色を変えた側の印を消す）
fn edge_key(m: &PlanarMap, e: usize) -> [u64; 6] {
    let ed = &m.edges[e];
    let (p0, p2) = (m.nodes[ed.a], m.nodes[ed.b]);
    let (p0, p2) = if point_order(p0, p2).is_le() {
        (p0, p2)
    } else {
        (p2, p0)
    };
    [
        p0.x.to_bits(),
        p0.y.to_bits(),
        ed.ctrl.x.to_bits(),
        ed.ctrl.y.to_bits(),
        p2.x.to_bits(),
        p2.y.to_bits(),
    ]
}

/// 確定した地図 `next`（面は `faces`）を、元の図形 `orig_shapes`（地図 `orig` を作った図形。`orig_keys` は `orig` の辺の
/// 鍵）に書き戻すときの差分。戻り値は（消す元の図形, 新しく入れる塗りの形）。
/// - 面のすべての辺の形が残り、面のすべての辺の側の印が同じ1つの元の図形で、その図形と色も同じで、その図形のどの辺も
///   消えていないなら、その面は元の図形のまま残す
/// - それ以外の面は、新しい塗りの形にして入れる。新しい形のうち、辺の側の印がみな同じ1つの元の図形（印の無い側は、
///   新しくできた辺）の面は、その元の図形を引き継ぐ形として元の番号を使う（同じ元の図形を引き継ぐ面が2つ以上あれば、
///   その印の付いた側がいちばん多い面。同じなら面の順で前）。形の変わった図形は番号を変えないので、地図の、その面の
///   ほかの辺の印も変わらない（しまう地図で、変わった場所の塊だけが新しくなる）
/// - 残した面の元の図形のほかは、みな消す（どの面にも残らない元の図形が、新しい形と重なって残ることは無い。番号を
///   引き継いだ形は、消した元の図形と入れ替わる）
///
/// 戻り値の3つ目は、面ごとの図形の番号（残した面は元の図形、ほかは新しい形の番号。`faces` の順）。番号はみな別
/// （同じ元の図形を残す・引き継ぐ面はそれぞれ1つだけ、新しい形には新しい番号）なので、面と図形が1対1になる
fn rewrite_shapes(
    orig: &PlanarMap,
    orig_keys: &HashSet<[u64; 6]>,
    next: &PlanarMap,
    faces: &[MapFace],
    orig_shapes: &[&FillShape],
    layer_id: u64,
    group_path: &[u64],
) -> (Vec<ShapeId>, Vec<FillShape>, Vec<ShapeId>) {
    let next_keys: HashSet<[u64; 6]> = (0..next.edges.len()).map(|e| edge_key(next, e)).collect();
    // 辺が消えた元の図形
    let mut cut: HashSet<ShapeId> = HashSet::new();
    for e in 0..orig.edges.len() {
        if !next_keys.contains(&edge_key(orig, e)) {
            cut.extend(orig.edges[e].left_src);
            cut.extend(orig.edges[e].right_src);
        }
    }
    // 元の図形の色（番号ごと）。残す面は、元の図形と番号・色まで同じ面だけ（印が重ねた片や色を変えた片から来ても、元の
    // 図形でなければ新しい図形にする。残す面はいつも元の図形にあるので、`MapChange::into_face_shapes` が図形を引ける）
    let orig_color: HashMap<ShapeId, Color32> =
        orig_shapes.iter().map(|s| (s.shape_id, s.color)).collect();
    let mut changed: Vec<usize> = Vec::new();
    let mut kept: HashSet<ShapeId> = HashSet::new();
    let mut ids: Vec<ShapeId> = vec![0; faces.len()];
    for (i, f) in faces.iter().enumerate() {
        let all_same = f
            .cycles
            .iter()
            .flatten()
            .all(|h| orig_keys.contains(&edge_key(next, h.0)));
        let keep = all_same
            && f.srcs.len() == 1
            && orig_color.get(&f.srcs[0]) == Some(&f.color)
            && !cut.contains(&f.srcs[0])
            && {
                f.cycles
                    .iter()
                    .flatten()
                    .all(|h| next.side_src(*h) == Some(f.srcs[0]))
            };
        // 同じ元の図形を残せるのは1つの面だけ（面の番号がみな別になる）
        if keep && kept.insert(f.srcs[0]) {
            ids[i] = f.srcs[0];
        } else {
            changed.push(i);
        }
    }
    // 形の変わらない輪郭（触っていない穴など）は、元の図形の輪郭をそのまま使い回す
    let reuse = |cycle: &[(usize, bool)]| -> Option<Contour> {
        if !cycle
            .iter()
            .all(|h| orig_keys.contains(&edge_key(next, h.0)))
        {
            return None;
        }
        let quads: Vec<QuadBez> = cycle.iter().map(|h| next.half_quad(*h)).collect();
        let src = next.side_src(cycle[0])?;
        let shape = orig_shapes.iter().find(|s| s.shape_id == src)?;
        shape
            .contours()
            .find(|c| {
                c.segments.len() == quads.len()
                    && c.segments
                        .iter()
                        .position(|q| *q == quads[0])
                        .is_some_and(|k| {
                            (0..quads.len()).all(|i| c.segments[(k + i) % quads.len()] == quads[i])
                        })
            })
            .cloned()
    };
    // 元の図形を引き継ぐ面：（元の図形 → (その印の付いた側の数, 面)）
    let mut heir: HashMap<ShapeId, (usize, usize)> = HashMap::new();
    for &i in &changed {
        let f = &faces[i];
        let [src] = f.srcs[..] else {
            continue;
        };
        if kept.contains(&src) {
            continue;
        }
        let sides = f
            .cycles
            .iter()
            .flatten()
            .filter(|h| next.side_src(**h) == Some(src))
            .count();
        let best = heir.entry(src).or_insert((sides, i));
        if sides > best.0 {
            *best = (sides, i);
        }
    }
    let added = changed
        .into_iter()
        .map(|i| {
            let f = &faces[i];
            let mut s = next.face_shape_with(f, layer_id, group_path, reuse);
            if let [src] = f.srcs[..] {
                if heir.get(&src).is_some_and(|h| h.1 == i) {
                    s.shape_id = src;
                }
            }
            ids[i] = s.shape_id;
            s
        })
        .collect();
    let removed: Vec<ShapeId> = orig_shapes
        .iter()
        .map(|s| s.shape_id)
        .filter(|id| !kept.contains(id))
        .collect();
    (removed, added, ids)
}

/// 地図 `map` の辺の側に、面 `faces` の図形の番号 `ids` を記した地図（面の無い側は無し）。文書にしまう地図は、面の辺の側に
/// いつもその面の図形の番号を持つ（図形と地図の面が1対1に対応する）
fn labeled(mut map: PlanarMap, faces: &[MapFace], ids: &[ShapeId]) -> PlanarMap {
    for ed in &mut map.edges {
        ed.left_src = None;
        ed.right_src = None;
    }
    for (f, &id) in faces.iter().zip(ids) {
        for &(e, forward) in f.cycles.iter().flatten() {
            let ed = &mut map.edges[e];
            if forward {
                ed.left_src = Some(id);
            } else {
                ed.right_src = Some(id);
            }
        }
    }
    map
}

/// テストで、書くたびに地図を確かめるか（速さを測る試しでは、環境変数 `IS2D_NO_CHECKS` で外す）
#[cfg(test)]
pub(crate) fn test_checks() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("IS2D_NO_CHECKS").is_err())
}

/// 図形に書き戻す地図 `next` の面。正しい地図でなければ（`valid_faces`：読み込みと同じ1つの判定）`None` を返し、書き戻さずに
/// 断る（書けた地図は、保存して開いても必ず読める）。テストでは、正しい地図でない結果を作った操作を、そこで見つける
fn written_faces(next: &PlanarMap) -> Option<Vec<MapFace>> {
    let faces = next.valid_faces();
    #[cfg(test)]
    if test_checks() {
        assert!(
            faces.is_some(),
            "書き戻す地図が正しい地図でない: {}",
            next.validity_report()
        );
    }
    faces
}

// ============================================================================
// 計算に使う写しと、地図を書き戻すときの差分（どの編集も使う）
// ============================================================================

/// 1つのレイヤー・階層の地図の、計算に使う写し：ふつうの形（いつもしまう形から決まった順で作ったもの：
/// `StoredMap::to_dense`）と、図形に書き戻すときの下ごしらえ（辺の鍵。書き戻すときに1回だけ作る）と、面の図形（写し。
/// 初めて要るときに作る：`LayerBase::faces`）。写しは、それを使う所が持つ（画面の土台・地図の編集の間・カーソル合わせ）。
/// 文書と Undo の控えは、しまう形（`StoredMap`）だけを持つ
#[derive(Debug)]
pub(crate) struct LayerBase {
    pub(crate) map: PlanarMap,
    pub(crate) stored: StoredMap,
    keys: OnceLock<HashSet<[u64; 6]>>,
    /// 面の図形：（階層, 面ごとの図形。番号は面の印。番号の順）
    faces: SyncCache<Option<FaceShapes>>,
}

/// 地図の面の図形（写し）：（階層, 面の番号の順の図形）
type FaceShapes = (Vec<u64>, Arc<[Arc<FillShape>]>);

impl LayerBase {
    /// 地図 `map` を、しまう形にして決まった順に並べ直したもの（文書に入らない、その場で使う地図）
    pub(crate) fn new(map: &PlanarMap) -> Self {
        Self::from_stored(StoredMap::from_dense(map, None))
    }

    fn from_stored(stored: StoredMap) -> Self {
        Self {
            map: stored.to_dense(),
            stored,
            keys: OnceLock::new(),
            faces: SyncCache::new(None),
        }
    }

    /// しまった地図 `stored` の計算に使う写し：いま誰かが持っていればそれ、無ければ作る（作ったものを生かすのは、
    /// 受け取った所だけ）
    pub(crate) fn of_stored(stored: &StoredMap) -> Arc<Self> {
        let mut slot = stored
            .dense_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(base) = slot.upgrade() {
            return base;
        }
        let base = Arc::new(Self::from_stored(stored.clone()));
        *slot = Arc::downgrade(&base);
        base
    }

    /// しまう地図 `stored`（階層 `group_path`）の計算に使う写しを、面の図形 `shapes`（地図の面ごとに1つ：
    /// `MapChange::into_face_shapes`。変わらない面は前の図形、形の変わった面は番号を引き継いだ新しい図形）で作る（地図の
    /// 編集のあと。面を求め直さない。画面の描画キャッシュは、番号と版が同じ図形から引き継がれる）
    pub(crate) fn with_faces(
        stored: StoredMap,
        group_path: &[u64],
        mut shapes: Vec<Arc<FillShape>>,
    ) -> Arc<Self> {
        let base = Self::from_stored(stored);
        shapes.sort_unstable_by_key(|s| s.shape_id);
        *base.faces.lock() = Some((group_path.to_vec(), shapes.into()));
        let base = Arc::new(base);
        *base
            .stored
            .dense_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&base);
        base
    }

    /// 面の図形（写し。階層 `group_path`）。面の順で、図形の番号は面の印（セル画の地図の面には、いつも1つずつ別の番号が
    /// 記してある）。`layer_id` は付けない（使う側が付ける）
    pub(crate) fn faces(&self, group_path: &[u64]) -> Arc<[Arc<FillShape>]> {
        let mut cached = self.faces.lock();
        if let Some((gp, shapes)) = &*cached {
            if gp == group_path {
                return shapes.clone();
            }
        }
        let shapes = self.face_shapes(group_path);
        *cached = Some((group_path.to_vec(), shapes.clone()));
        shapes
    }

    /// 面の図形を地図から作る（`faces`。番号の順）
    fn face_shapes(&self, group_path: &[u64]) -> Arc<[Arc<FillShape>]> {
        let (_, _, shapes) = numbered_faces(&self.map, 0, group_path);
        let mut shapes: Vec<Arc<FillShape>> = shapes.into_iter().map(Arc::new).collect();
        shapes.sort_unstable_by_key(|s| s.shape_id);
        shapes.into()
    }

    /// テスト用：図形から作る地図：座標の変換 `xform` で置いた図形 `own`（下から上への並び。変換したあとの座標）の地図。
    /// 地図は文書の座標で作る（`with_doc_shapes`）。本番は図形から地図を作らない（新しい中身も地図で運ぶ）ので、テストの
    /// 下ごしらえだけで使う。正しい地図でない図形（重なった図形など）は、並びの順に塗り重ねて作り直す（`painted_map`。絵は
    /// 同じ）。`None`：格子に置けない座標がある
    #[cfg(test)]
    pub(crate) fn of(own: &[&FillShape], xform: Affine) -> Option<Arc<Self>> {
        FROM_SHAPES.with(|n| n.set(n.get() + 1));
        let base = with_doc_shapes(own, xform, |own| {
            if !own.iter().all(|s| shape_on_grid(s)) {
                return None;
            }
            let map = PlanarMap::from_faces(own.iter().copied());
            Some(if map.is_valid() {
                Self::new(&map)
            } else {
                Self::new(&painted_map(own)?)
            })
        })?;
        Some(Arc::new(base))
    }

    /// この地図（文書の座標）の面の図形 `own` に、編集した地図 `next` を書き戻す差分（`MapChange`。新しい形も文書の座標。
    /// 変わらない面は前の図形のまま：`rewrite_shapes`）。`None`：`next` に壊れた輪郭がある（`written_faces`）
    pub(crate) fn diff(
        &self,
        next: &PlanarMap,
        own: &[&FillShape],
        layer_id: LayerId,
        group_path: &[u64],
    ) -> Option<MapChange> {
        let faces = written_faces(next)?;
        let keys = self.keys.get_or_init(|| {
            (0..self.map.edges.len())
                .map(|e| edge_key(&self.map, e))
                .collect()
        });
        let (removed, added, ids) =
            rewrite_shapes(&self.map, keys, next, &faces, own, layer_id, group_path);
        Some(MapChange {
            removed,
            added,
            map: CheckedMap {
                map: next.clone(),
                faces,
            }
            .with_ids(&ids),
            ids,
        })
    }

    /// テスト用：`diff` を、座標の変換 `xform` で置いた図形 `own`（文書の無い、ただの図形の並び。この地図は `LayerBase::of`
    /// で作ったもの）で受け取り、新しい形を `xform` で置いた写しで返すもの。図形がそのまま正しい地図にならず、塗り重ねて
    /// 作った地図なら、地図の面と図形は1対1でないので、図形を全部書き換える
    #[cfg(test)]
    pub(crate) fn diff_placed(
        &self,
        next: &PlanarMap,
        own: &[&FillShape],
        layer_id: LayerId,
        group_path: &[u64],
        xform: Affine,
    ) -> Option<MapChange> {
        let mut change = with_doc_shapes(own, xform, |own| {
            if PlanarMap::from_faces(own.iter().copied()).is_valid() {
                return self.diff(next, own, layer_id, group_path);
            }
            let faces = written_faces(next)?;
            let added: Vec<FillShape> = faces
                .iter()
                .map(|f| next.face_shape(f, layer_id, group_path))
                .collect();
            let ids: Vec<ShapeId> = added.iter().map(|s| s.shape_id).collect();
            Some(MapChange {
                removed: own.iter().map(|s| s.shape_id).collect(),
                map: CheckedMap {
                    map: next.clone(),
                    faces,
                }
                .with_ids(&ids),
                added,
                ids,
            })
        })?;
        change.added = change.added.into_iter().map(|d| placed(d, xform)).collect();
        Some(change)
    }
}

#[cfg(test)]
thread_local! {
    /// テスト用：このスレッドで、図形から地図を作った回数（`LayerBase::of`）
    static FROM_SHAPES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// テスト用：このスレッドで、図形から地図を作った回数
#[cfg(test)]
pub(crate) fn from_shape_builds() -> usize {
    FROM_SHAPES.with(|n| n.get())
}

/// テスト用：図形 `shapes`（座標の変換 `xform` で置いた図形）を文書の座標の形（`FillShape::to_doc`）にして `f` に渡す。
/// 地図はいつも文書の座標で作る：格子の上の判定はアフィン変換で保たれないので、置いた座標の地図は正しい地図とは限らない
#[cfg(test)]
fn with_doc_shapes<R>(
    shapes: &[&FillShape],
    xform: Affine,
    f: impl FnOnce(&[&FillShape]) -> R,
) -> R {
    let docs: Vec<std::borrow::Cow<'_, FillShape>> =
        shapes.iter().map(|s| s.to_doc(xform)).collect();
    let refs: Vec<&FillShape> = docs.iter().map(|d| d.as_ref()).collect();
    f(&refs)
}

/// 文書の座標の図形 `d` を、座標の変換 `xform` で置いた写し（`FillShape::place`）
fn placed(mut d: FillShape, xform: Affine) -> FillShape {
    d.place(xform);
    d
}

/// 図形の並び `shapes` のうち、レイヤー `layer_id`・階層 `group_path` の、地図にする生の図形（並びの順）
#[cfg(test)]
pub(crate) fn layer_own<'a>(
    shapes: &'a [FillShape],
    layer_id: LayerId,
    group_path: &[u64],
) -> Vec<&'a FillShape> {
    shapes
        .iter()
        .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path))
        .collect()
}

/// 地図にする生の図形か：階層 `scope` の生の図形（シンボルの図形でない）で、輪郭がある
pub(crate) fn is_map_shape(s: &FillShape, scope: &[u64]) -> bool {
    s.is_raw_at_scope(scope) && s.symbol_instance.is_none() && !s.exterior.segments.is_empty()
}

/// 輪郭の変形の1回分（どの辺・頂点かは、形と位置で探す）
#[derive(Clone, Copy, Debug)]
pub(crate) enum MapEdit {
    /// 形が `orig_seg` の辺の `grab_t` の所を `target` へ引っぱる
    PullEdge {
        orig_seg: QuadBez,
        grab_t: f64,
        target: Point,
    },
    /// `orig_vertex` にある頂点を `target` へ動かす
    MoveVertex { orig_vertex: Point, target: Point },
    /// 形が `orig_seg` の辺の `t` の所に頂点を足す（境目なら両側の図形に入る）
    AddAnchor { orig_seg: QuadBez, t: f64 },
    /// `orig_vertex` にある頂点を消し、前後の辺を1本の直線にする
    DeleteAnchor { orig_vertex: Point },
}

impl MapEdit {
    /// 地図 `map` にこの編集をかけて確定した地図。`None`：その辺・頂点が無いか、編集できない
    pub(crate) fn apply(self, map: &PlanarMap) -> Option<PlanarMap> {
        let vertex = |p: Point| (0..map.nodes.len()).find(|&v| map.nodes[v] == p);
        match self {
            MapEdit::AddAnchor { orig_seg, t } => {
                let (e, forward) = map.find_edge(&orig_seg)?;
                map.with_anchor_added(e, if forward { t } else { 1.0 - t })
            }
            MapEdit::DeleteAnchor { orig_vertex } => map.with_anchor_deleted(vertex(orig_vertex)?),
            MapEdit::PullEdge {
                orig_seg,
                grab_t,
                target,
            } => {
                let (e, forward) = map.find_edge(&orig_seg)?;
                let t = if forward { grab_t } else { 1.0 - grab_t };
                map.commit_pull_edge(e, t, target)
            }
            MapEdit::MoveVertex {
                orig_vertex,
                target,
            } => map.commit_move_vertex(vertex(orig_vertex)?, target),
        }
    }
}

impl PlanarMap {
    /// 形が `q` の辺（向きが逆でもよい）と、`q` と同じ向きか
    fn find_edge(&self, q: &QuadBez) -> Option<(usize, bool)> {
        (0..self.edges.len()).find_map(|e| {
            let m = self.quad(e);
            if m == *q {
                Some((e, true))
            } else if m.p0 == q.p2 && m.p1 == q.p1 && m.p2 == q.p0 {
                Some((e, false))
            } else {
                None
            }
        })
    }

    /// 辺 `e` を `t` の所で2本に切った形（形は変わらない）。分けた2本の判定の形が別々の折れ線にならない（端に近すぎて、
    /// 新しい頂点が端と同じ格子の点に丸まる）か、つながりが変わるときは `None`
    pub(crate) fn with_anchor_added(&self, e: usize, t: f64) -> Option<PlanarMap> {
        let mut m = self.clone();
        let ed = m.edges[e];
        let q = m.quad(e);
        m.nodes.push(q.eval(t));
        let v = m.nodes.len() - 1;
        m.edges[e] = MapEdge {
            b: v,
            ctrl: q.subsegment(0.0..t).p1,
            ..ed
        };
        m.edges.push(MapEdge {
            a: v,
            ctrl: q.subsegment(t..1.0).p1,
            ..ed
        });
        // 判定の形で、つながりが変わらないこと（分けた2本の折れ線は、元の折れ線と少し違う）
        let (p1, p2) = (grid_poly(&m.quad(e)), grid_poly(&m.quad(m.edges.len() - 1)));
        if p1.len() < 2 || p2.len() < 2 {
            return None;
        }
        let pc: Vec<IPt> = p1.iter().chain(p2.iter().skip(1)).copied().collect();
        self.can_replace_path(&[e], ed.a, ed.b, &pc).then_some(m)
    }

    /// 頂点 `v` を消し、前後の辺を1本の直線にした形。消せないときは `None`：
    /// つながる辺が2本でない（3色以上が接する点など）、前後が同じ頂点（細いレンズ形）、
    /// 判定の形でつながりが変わる（直線がほかの辺に触れる・ほかの頂点や辺をまたぐ）
    pub(crate) fn with_anchor_deleted(&self, v: usize) -> Option<PlanarMap> {
        let at = self.edges_at(v);
        if at.len() != 2 {
            return None;
        }
        let (e1, e2) = (
            self.edges[at[0]].oriented(false, v),
            self.edges[at[1]].oriented(true, v),
        );
        if e1.a == e2.b || e1.a == v || e2.b == v || e1.left != e2.left || e1.right != e2.right {
            return None;
        }
        let (pa, pb) = (self.nodes[e1.a], self.nodes[e2.b]);
        let line = MapEdge {
            a: e1.a,
            b: e2.b,
            ctrl: pa.midpoint(pb),
            ..e1
        };
        // 2本を直線に置き換えても、判定の形でつながりが変わらないこと（設計 2-4 の3つの決まり。囲まれた所に何も無く、
        // 直線がほかに触れなければ、両側の面は潰れも裏返りもしない）
        let pc = grid_poly(&QuadBez::new(pa, line.ctrl, pb));
        if !self.can_replace_path(&[at[0], at[1]], e1.a, e2.b, &pc) {
            return None;
        }
        let mut m = self.clone();
        m.edges[at[0]] = line;
        m.edges.swap_remove(at[1]);
        m.compact_nodes();
        Some(m)
    }

    /// 頂点 `na` から辺 `part` を順にたどって `nb` へつながる道を、`na` から `nb` への折れ線 `pc` の1本の辺に
    /// 置き換えてよいか（判定の形で、設計 2-4 の3つの決まり）
    fn can_replace_path(&self, part: &[usize], na: usize, nb: usize, pc: &[IPt]) -> bool {
        let mut path: Vec<IPt> = vec![gpt(self.nodes[na])];
        let mut inner: Vec<usize> = Vec::new();
        let mut at = na;
        for &f in part {
            let ed = self.edges[f];
            let poly = grid_poly(&self.quad(f));
            if ed.a == at {
                path.extend(poly.iter().skip(1));
                at = ed.b;
            } else {
                path.extend(poly.iter().rev().skip(1));
                at = ed.a;
            }
            inner.push(at);
        }
        inner.pop();
        if at != nb {
            return false;
        }
        let r = GridRect::of(&path).union(&GridRect::of(pc));
        let area = KRect::new(
            r.x0 as f64 / OVERLAY_GRID,
            r.y0 as f64 / OVERLAY_GRID,
            r.x1 as f64 / OVERLAY_GRID,
            r.y1 as f64 / OVERLAY_GRID,
        );
        PolyIndex::new(self, Some(area)).can_replace(part, &path, &inner, pc, na, nb)
    }

    /// 地図を座標の変換 `xform` で置いたとき、頂点 `v` を `p` へ動かすときに吸い付く、ほかの頂点（`radius` 以内でいちばん
    /// 近いもの。距離は置いた座標で測る）の位置（地図の座標）
    pub(crate) fn snap_target(
        &self,
        v: usize,
        p: Point,
        radius: f64,
        xform: Affine,
    ) -> Option<Point> {
        (0..self.nodes.len())
            .filter(|&w| w != v)
            .map(|w| (self.nodes[w], (xform * self.nodes[w] - p).hypot()))
            .filter(|(_, d)| *d <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(q, _)| q)
    }
}

/// 輪郭の変形のドラッグ中の表示：地図 `base.map` に編集 `edit`（文書の座標）をかけて確定した結果を、図形の差分で返す
/// （離したときの確定 `Cel::edit_map` と同じ計算）。`own` は地図の面の文書の図形。新しい形は、座標の変換 `xform` で置いた写し
pub(crate) fn reshape_result(
    base: &LayerBase,
    edit: MapEdit,
    own: &[&FillShape],
    layer_id: LayerId,
    group_path: &[u64],
    xform: Affine,
) -> Option<(Vec<ShapeId>, Vec<FillShape>)> {
    let next = edit.apply(&base.map)?;
    let change = base.diff(&next, own, layer_id, group_path)?;
    Some((
        change.removed,
        change.added.into_iter().map(|d| placed(d, xform)).collect(),
    ))
}

/// `items` のそれぞれに `f` をかけた結果（`items` の順）。2つ以上あれば、別々のスレッドで同時に計算する
/// （レイヤーごとの地図の計算は、ほかのレイヤーを見ないので、同時に計算しても結果は同じ）
pub(crate) fn per_layer_parallel<T: Sync, R: Send>(
    items: &[T],
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    if items.len() < 2 {
        return items.iter().map(&f).collect();
    }
    std::thread::scope(|scope| {
        let f = &f;
        let handles: Vec<_> = items.iter().map(|it| scope.spawn(move || f(it))).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect()
    })
}

/// 差分を図形の並びに入れる。新しい形は、消した図形のうち最初のものの位置に入れる。
/// 何も消さないとき（何も無い所へのブラシなど）は、レイヤー `layer_id`・階層 `group_path` の生の図形の置き場
/// （グループ・シンボルより下）に入れる
#[cfg(test)]
pub(crate) fn apply_shape_diff(
    shapes: &mut Vec<FillShape>,
    removed: &[ShapeId],
    added: Vec<FillShape>,
    layer_id: LayerId,
    group_path: &[u64],
) {
    let at = shapes.iter().position(|s| removed.contains(&s.shape_id));
    shapes.retain(|s| !removed.contains(&s.shape_id));
    let at = at
        .unwrap_or_else(|| raw_scope_insert_index(shapes, layer_id, group_path))
        .min(shapes.len());
    shapes.splice(at..at, added);
}

/// レイヤー `target_layer_id`・階層 `target_group_path` の生の図形の置き場（その階層の生の図形の後ろ。グループ・シンボルより下）
#[cfg(test)]
fn raw_scope_insert_index<S: std::borrow::Borrow<FillShape>>(
    canvas: &[S],
    target_layer_id: u64,
    target_group_path: &[u64],
) -> usize {
    let at = |i: usize| canvas[i].borrow();
    let n = canvas.len();
    (0..n)
        .rev()
        .find(|&i| {
            let s = at(i);
            s.layer_id == target_layer_id
                && s.group_path == target_group_path
                && s.symbol_instance.is_none()
        })
        .map(|pos| pos + 1)
        .or_else(|| {
            (0..n).find(|&i| {
                let s = at(i);
                s.layer_id == target_layer_id
                    && s.is_in_editing_scope(target_group_path)
                    && !s.is_raw_at_scope(target_group_path)
            })
        })
        .or_else(|| {
            (0..n)
                .rev()
                .find(|&i| {
                    let s = at(i);
                    s.layer_id == target_layer_id && s.is_in_editing_scope(target_group_path)
                })
                .map(|pos| pos + 1)
        })
        .or_else(|| {
            (0..n)
                .rev()
                .find(|&i| at(i).layer_id == target_layer_id)
                .map(|pos| pos + 1)
        })
        .unwrap_or(n)
}

/// 地図で編集した図形の差分
#[derive(Debug)]
pub(crate) struct MapChange {
    /// 消す元の図形
    pub(crate) removed: Vec<ShapeId>,
    /// 新しく入れる塗りの形（座標の変換で置いた写し）
    pub(crate) added: Vec<FillShape>,
    /// 結果の地図（文書の座標。確かめた地図）。面の辺の側に、その面の図形の番号を記してある（`labeled`）
    pub(crate) map: CheckedMap,
    /// 結果の地図の面ごとの図形の番号（面の順。みな別の番号）
    pub(crate) ids: Vec<ShapeId>,
}

/// 地図で編集した差分 `change` を、図形の並び `shapes` のレイヤー `layer_id`・階層 `group_path` に入れる
/// （`apply_shape_diff`。テスト用：文書の無い図形の並びで、地図の編集を試す）
#[cfg(test)]
pub(crate) fn apply_map_change(
    shapes: &mut Vec<FillShape>,
    change: MapChange,
    layer_id: LayerId,
    group_path: &[u64],
) {
    apply_shape_diff(shapes, &change.removed, change.added, layer_id, group_path);
}

// ============================================================================
// 浮いた片の地図（設計：claude/浮いた片を地図で持つ設計_2-2c_6c167ae.md）
// ============================================================================

impl PlanarMap {
    /// 変換 `a` をかけた地図（頂点と制御点を写す。裏返る変換では、辺の左右を入れ替える）。格子の上で正しい地図とは
    /// 限らない（`realized`）
    pub(crate) fn transformed(&self, a: Affine) -> PlanarMap {
        let flip = a.determinant() < 0.0;
        PlanarMap {
            nodes: self.nodes.iter().map(|&p| a * p).collect(),
            edges: self
                .edges
                .iter()
                .map(|e| {
                    let e = MapEdge {
                        ctrl: a * e.ctrl,
                        ..*e
                    };
                    if flip {
                        MapEdge {
                            left: e.right,
                            right: e.left,
                            left_src: e.right_src,
                            right_src: e.left_src,
                            ..e
                        }
                    } else {
                        e
                    }
                })
                .collect(),
        }
    }

    /// 面の番号が `ids` にある面の色を `color` にした地図（同じ色でとなり合う面の間の辺は消え、面は1つになる）
    pub(crate) fn recolored(&self, ids: &HashSet<ShapeId>, color: Color32) -> PlanarMap {
        let mut m = self.clone();
        for e in &mut m.edges {
            if e.left_src.is_some_and(|id| ids.contains(&id)) {
                e.left = Some(color);
            }
            if e.right_src.is_some_and(|id| ids.contains(&id)) {
                e.right = Some(color);
            }
        }
        m.edges.retain(|e| e.left != e.right);
        m.compact_nodes();
        m
    }
}

/// 地図 `map`（文書の座標）に変換 `xform` をかけ、その座標の格子で正しい地図にしたもの。変換した地図が正しくなければ
/// （縮小で頂点が近づくなど）、変換した面を塗り重ねて作る（面は重ならないので、絵は同じ。面の番号は残る）。
/// 「変形した形を、その座標の格子で地図にする」決まり。結果は確かめた地図（`checked`）。`None`：格子に置けない座標が
/// あるか、正しい地図にならない
pub(crate) fn realized(map: &PlanarMap, xform: Affine) -> Option<CheckedMap> {
    if xform == Affine::IDENTITY {
        return map.clone().checked();
    }
    if !map.fits_grid(xform) {
        return None;
    }
    let t = map.transformed(xform);
    if let Some(checked) = t.checked() {
        return Some(checked);
    }
    // 面の形には、面の番号（辺の側に記された番号が1つなら、その番号）を付けて塗る。塗り重ねは、いちばん上の形の番号を
    // 面の印として運ぶので、面の番号が残る（レビュー 4f4e953 の N-2001）
    let (faces, _) = map.faces();
    let shapes: Vec<FillShape> = faces
        .iter()
        .map(|f| {
            let mut s = map.face_shape(f, 0, &[]);
            if let [id] = f.srcs[..] {
                s.shape_id = id;
            }
            s.transform(xform);
            s
        })
        .collect();
    let refs: Vec<&FillShape> = shapes.iter().collect();
    painted_map(&refs)?.checked()
}

/// 地図 `map`（文書の座標）の面の図形と、面の辺の側にその番号を記した地図。面の番号は、辺の側に記された番号が1つの面は
/// その番号（同じ番号の面が2つ以上あれば、面の順で前の面だけ）、ほかは新しい番号
pub(crate) fn faces_with_ids(
    map: &PlanarMap,
    layer_id: LayerId,
    group_path: &[u64],
) -> (PlanarMap, Vec<FillShape>) {
    let (faces, ids, shapes) = numbered_faces(map, layer_id, group_path);
    (labeled(map.clone(), &faces, &ids), shapes)
}

/// 地図 `map` の面と、面ごとの番号（`face_ids` の `Keep`）と、その番号を付けた面の図形（どれも面の順）。面から番号つきの
/// 図形を作るのはこれ1つ（画面の土台の面の図形 `LayerBase::face_shapes`、片の面の図形 `faces_with_ids`）
fn numbered_faces(
    map: &PlanarMap,
    layer_id: LayerId,
    group_path: &[u64],
) -> (Vec<MapFace>, Vec<ShapeId>, Vec<FillShape>) {
    let (faces, _) = map.faces();
    let ids = face_ids(&faces, FaceIds::Keep);
    let shapes = faces
        .iter()
        .zip(&ids)
        .map(|(f, &id)| {
            let mut s = map.face_shape(f, layer_id, group_path);
            s.shape_id = id;
            s
        })
        .collect();
    (faces, ids, shapes)
}

/// 地図 `start` に、地図 `maps` を下から順に重ねた地図（`MapRule::Overlay`。重ねる地図の色が上）。`start` が空なら、
/// 最初の地図をそのまま使う（空の地図に重ねても同じ地図。重ねる計算を省く）。地図を下から順に重ねるのはこれ1つ（浮いた
/// 片の組・選んだものの地図・確定・セル画を1枚に重ねる）。`None`：地図が作れない（格子に置けない座標がある）
pub(crate) fn stacked(
    start: PlanarMap,
    maps: impl IntoIterator<Item = Option<PlanarMap>>,
) -> Option<PlanarMap> {
    maps.into_iter().try_fold(start, |map, m| {
        let m = m?;
        if map.edges.is_empty() {
            Some(m)
        } else {
            map.overlay(MapInput::Map(&m), MapRule::Overlay)
        }
    })
}

/// 面ごとの番号の決め方（`face_ids`）
#[derive(Clone, Copy)]
enum FaceIds {
    /// 辺の側に記された番号が1つの面はその番号（同じ番号の面が2つ以上あれば、面の順で前の面だけ）、ほかは新しい番号
    Keep,
    /// みな新しい番号（新しい中身を置くとき。元の面と番号が重ならない）
    Fresh,
}

/// 面 `faces` の番号（面ごとに1つずつ別の番号）。面の番号の決め方はこれ1つ
fn face_ids(faces: &[MapFace], how: FaceIds) -> Vec<ShapeId> {
    let mut used: HashSet<ShapeId> = HashSet::new();
    faces
        .iter()
        .map(|f| match (how, &f.srcs[..]) {
            (FaceIds::Keep, [id]) if used.insert(*id) => *id,
            _ => crate::vector_engine::next_shape_id(),
        })
        .collect()
}

/// 正しい地図（`valid_faces` で確かめたもの）と、その面。作れるのは `PlanarMap::checked` と、正しい地図を正しいまま
/// 変える、このあとのメソッドだけなので、これを受け取る所（セル画にしまう地図：`ScopeMap::new`）は確かめ直さない
#[derive(Clone, Debug)]
pub(crate) struct CheckedMap {
    map: PlanarMap,
    faces: Vec<MapFace>,
}

impl PlanarMap {
    /// 正しい地図なら（`valid_faces`。受け入れの判定はこれ1つ）、確かめた地図にする。`None`：正しい地図でない
    pub(crate) fn checked(self) -> Option<CheckedMap> {
        let faces = self.valid_faces()?;
        Some(CheckedMap { map: self, faces })
    }
}

impl CheckedMap {
    pub(crate) fn map(&self) -> &PlanarMap {
        &self.map
    }

    pub(crate) fn into_map(self) -> PlanarMap {
        self.map
    }

    /// 面の辺の側に、面ごとに1つずつ別の番号（`face_ids` の `Keep`）を記したもの（セル画にしまう地図）。辺の側の番号は
    /// 面の形を変えないので、正しい地図のまま
    pub(crate) fn uniquely_labeled(self) -> CheckedMap {
        self.labeled(FaceIds::Keep)
    }

    /// 面の辺の側に、面ごとにみな新しい番号（`face_ids` の `Fresh`）を記したもの
    pub(crate) fn freshly_labeled(self) -> CheckedMap {
        self.labeled(FaceIds::Fresh)
    }

    fn labeled(self, how: FaceIds) -> CheckedMap {
        let ids = face_ids(&self.faces, how);
        self.with_ids(&ids)
    }

    /// 面の辺の側に、面ごとの番号 `ids`（面の順）を記したもの
    fn with_ids(self, ids: &[ShapeId]) -> CheckedMap {
        let map = labeled(self.map, &self.faces, ids);
        let faces = self
            .faces
            .into_iter()
            .zip(ids)
            .map(|(mut f, &id)| {
                f.srcs = vec![id];
                f
            })
            .collect();
        CheckedMap { map, faces }
    }

    /// 面に番号の無い地図（読み込んだ地図）の辺のうち、前の地図 `prev` に形と色が同じ辺があるものに、その辺の側の番号を
    /// 記したもの（`StoredMap::labels_of`。面の番号も）。変わらない面は前の面の番号を持つので、`uniquely_labeled` のあとも
    /// 前の地図と塊を共有する
    pub(crate) fn inheriting(mut self, prev: &StoredMap) -> CheckedMap {
        let labels = prev.labels_of(&self.map);
        for (e, (left, right)) in self.map.edges.iter_mut().zip(labels) {
            e.left_src = left;
            e.right_src = right;
        }
        for f in &mut self.faces {
            f.srcs = self.map.face_srcs(&f.cycles);
        }
        self
    }

    /// 辺の側に記した図形の番号を、`f` が返す番号に付け替える（`PlanarMap::relabel`。面の番号も）
    fn relabel(&mut self, mut f: impl FnMut(ShapeId) -> Option<ShapeId>) {
        self.map.relabel(&mut f);
        for face in &mut self.faces {
            for id in &mut face.srcs {
                if let Some(n) = f(*id) {
                    *id = n;
                }
            }
            face.srcs.sort_unstable();
            face.srcs.dedup();
        }
    }
}

/// 選ぶ所の地図：地図 `part`（中身の地図から選んだ面だけを残したもの）に、形 `rect` を足した所（色のある所が中）。
/// `None`：格子に置けない座標がある
pub(crate) fn region_map(part: &PlanarMap, rect: Option<&FillShape>) -> Option<PlanarMap> {
    let mut m = part.clone();
    if let Some(r) = rect {
        m = m.overlay(
            MapInput::Shapes(std::slice::from_ref(r)),
            MapRule::Paint(Some(r.color)),
        )?;
    }
    Some(m)
}

/// 浮いた片を切り出す：セル画の地図 `cel`（文書の図形 `own` の地図）の、中身 `content`
/// （`cel` の地図そのものか、前に切り出した残り）を、選ぶ所 `region` で1回切る。（残りの差分, 中の差分）：残りの差分の
/// 消す図形が隠す図形、新しく入れる形が残りの図形、地図が残りの地図。中の差分の地図が片の地図（丸ごと中にあるセル画の
/// 面は、その番号のまま）。片の新しい形は、残りの形と同じ元の図形を引き継ぐことがあるので、番号を新しく振る（地図の
/// 印も）。残りの形は、セル画の地図の書き換えと同じく、元の図形の番号を引き継ぐ（隠す元の図形と入れ替わる。残りの地図の
/// 印が変わらないので、残りの地図は、切り口から離れた所の塊をセル画の地図と共有する）。中と外は同じ地図を同じ所で
/// 切るので、切り口は両側で同じ線になる（地図の精度 `FLAT_TOL` の中で。中と外は別々に重ね合わせるので、切り口の折れ線を
/// 曲線に戻すかどうかが両側で違い、点の並びがビットまでは同じにならないことがある。重ね直すと同じ線として1本になる：
/// `same_curve`）。`None`：格子に置けない座標がある
pub(crate) fn cut_out(
    cel: &LayerBase,
    own: &[&FillShape],
    content: &PlanarMap,
    region: MapInput<'_>,
    layer_id: LayerId,
    group_path: &[u64],
) -> Option<(MapChange, MapChange)> {
    let inside = content.overlay(region, MapRule::KeepInside)?;
    let rest = content.overlay(region, MapRule::Paint(None))?;
    let rest_change = cel.diff(&rest, own, layer_id, group_path)?;
    let mut inside_change = cel.diff(&inside, own, layer_id, group_path)?;
    inside_change.renew_ids();
    Some((rest_change, inside_change))
}

impl MapChange {
    /// 新しく入れる形に新しい番号を振る（地図の、その面の辺の側の印も付け替える）。元の図形の番号を引き継いだ形も新しくする
    pub(crate) fn renew_ids(&mut self) {
        let mut renamed: HashMap<ShapeId, ShapeId> = HashMap::new();
        for s in &mut self.added {
            let id = crate::vector_engine::next_shape_id();
            renamed.insert(s.shape_id, id);
            s.shape_id = id;
        }
        self.map.relabel(|id| renamed.get(&id).copied());
        for id in &mut self.ids {
            if let Some(n) = renamed.get(id) {
                *id = *n;
            }
        }
    }

    /// 結果の地図と、その面ごとの図形（面の順）：新しく入れる形か、前の面の図形 `old` のうち番号が同じもの。どちらにも
    /// 無い番号があるか、面にならない新しい形があれば `None`（面と図形が1対1にならない。書かない）
    pub(crate) fn into_face_shapes(
        self,
        old: &[Arc<FillShape>],
    ) -> Option<(CheckedMap, Vec<Arc<FillShape>>)> {
        let old: HashMap<ShapeId, &Arc<FillShape>> = old.iter().map(|s| (s.shape_id, s)).collect();
        let mut added: HashMap<ShapeId, FillShape> =
            self.added.into_iter().map(|s| (s.shape_id, s)).collect();
        let shapes = self
            .ids
            .iter()
            .map(|id| match added.remove(id) {
                Some(mut s) => {
                    s.strip_transient_caches();
                    Some(Arc::new(s))
                }
                None => old.get(id).map(|s| Arc::clone(s)),
            })
            .collect::<Option<Vec<_>>>()?;
        added.is_empty().then_some((self.map, shapes))
    }
}

/// 地図で編集した結果
#[derive(Debug)]
#[cfg(test)]
pub(crate) enum MapEditOutcome {
    /// 形が変わった
    Changed(MapChange),
    /// 形が変わらない
    Unchanged,
    /// 地図にできない（格子に置けない座標がある：±65,536px の外）か、結果に壊れた輪郭がある（`written_faces`）
    Refused,
}

/// 図形の並び `shapes`（座標の変換 `xform` で置いた図形）のうち、レイヤー `layer_id`・階層 `group_path` の生の図形を地図に
/// して（`LayerBase::of`。文書の座標の地図）、`edit` で編集した差分。`edit` に渡す形・点も文書の座標にしておく。地図を
/// 塗り重ねて作り直したとき（壊れた地図）は、レイヤーの生の図形を全部書き直す
#[cfg(test)]
pub(crate) fn map_edit_diff(
    shapes: &[FillShape],
    layer_id: LayerId,
    group_path: &[u64],
    xform: Affine,
    edit: impl FnOnce(&PlanarMap) -> Option<PlanarMap>,
) -> MapEditOutcome {
    let own = layer_own(shapes, layer_id, group_path);
    let Some(base) = LayerBase::of(&own, xform) else {
        return MapEditOutcome::Refused;
    };
    let Some(change) =
        edit(&base.map).and_then(|next| base.diff_placed(&next, &own, layer_id, group_path, xform))
    else {
        return MapEditOutcome::Refused;
    };
    if change.removed.is_empty() && change.added.is_empty() {
        MapEditOutcome::Unchanged
    } else {
        MapEditOutcome::Changed(change)
    }
}

/// 図形 `shapes`（下から上への並び）を、並びの順に空の地図へ塗り重ねた地図（重なる所は上の図形が見える。絵の決まりそのもの）。
/// 全部の図形を1回で重ねる（`MapInput::Stack`）。図形ごとに偶奇の規則なので、輪郭が自分と交わる図形もそのまま扱える。
/// `None`：格子に置けない座標がある
fn painted_map(shapes: &[&FillShape]) -> Option<PlanarMap> {
    PlanarMap::default().overlay(MapInput::Stack(shapes), MapRule::Overlay)
}

/// テスト用：図形 `shapes`（下から上への並び）を塗り重ねた地図（`painted_map`）
#[cfg(test)]
pub(crate) fn painted_map_pub(shapes: &[FillShape]) -> Option<PlanarMap> {
    let refs: Vec<&FillShape> = shapes.iter().collect();
    painted_map(&refs)
}
