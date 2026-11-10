//! 同じレイヤー・同じ階層の生の図形を、境目の線（辺）で持つ地図。選択ツールの輪郭の変形とカーソル合わせで使う。
//! Adobe Animate のマージ描画と同じく、辺が「曲線・左の色・右の色」を持つ。境目は1本で、重ね順は無い。
//! 塗りの形（`FillShape`）は、辺から作って描画・当たり判定に使う。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui::Color32;
use kurbo::{
    BezPath, ParamCurve, Point, QuadBez, Rect as KRect, Shape as KurboShape, Vec2 as KVec2,
};

use crate::canvas_tool::{HoverTarget, LayerId, Scene, Session, ShapeId};
use crate::vector_engine::{rects_overlap, Contour, FillShape};

/// 図形から地図を作るとき（`from_faces`）に、同じ点・同じ線とみなす距離。生の図形はどれも地図から書き出したもので、
/// 境目は両側の図形でビットまで同じ曲線なので、許容はほぼ 0 でよい（0.05 にすると、地図の上で 0.05 より近くに
/// できた2つの頂点がまとめられて、地図が壊れることがある）
pub(crate) const FACE_TOL: f64 = 1e-6;

/// 辺の（左の色, 右の色）
type Sides = (Option<Color32>, Option<Color32>);

/// 辺。a → b に進んだときの左（面積の式で正の側）の色と、右の色を持つ。`None` は何も無い所。
/// `left_src`・`right_src` は、その側の面がどの図形から来たか（図形から作ったときだけ意味がある）
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
    /// 頂点ごとに、その位置にある元の図形の頂点のひとつ（図形, 輪郭, 区間）。カーソル合わせの表示に使う
    pub(crate) node_src: Vec<Option<(ShapeId, usize, usize)>>,
}

/// 面。外周の輪郭と穴の輪郭（向きのある辺の輪）を持つ。`srcs` は、この面の辺の側に記された元の図形
#[derive(Clone, Debug)]
pub(crate) struct MapFace {
    pub(crate) color: Color32,
    pub(crate) cycles: Vec<Vec<(usize, bool)>>,
    pub(crate) srcs: Vec<ShapeId>,
}

/// 曲線 `q` の上で点 `p` にいちばん近い所の（パラメータ, 距離）
pub(crate) fn nearest_on_quad(q: &QuadBez, p: Point) -> (f64, f64) {
    let dist_sq = |t: f64| (q.eval(t) - p).hypot2();
    let mut t = (0..=16)
        .map(|k| k as f64 / 16.0)
        .min_by(|a, b| dist_sq(*a).total_cmp(&dist_sq(*b)))
        .unwrap_or(0.0);
    let second = (q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2()) * 2.0;
    for _ in 0..6 {
        let b = q.eval(t) - p;
        let first = (q.p1 - q.p0) * (2.0 * (1.0 - t)) + (q.p2 - q.p1) * (2.0 * t);
        let slope = first.dot(first) + b.dot(second);
        if slope.abs() < 1e-14 {
            break;
        }
        t = (t - b.dot(first) / slope).clamp(0.0, 1.0);
    }
    (t, dist_sq(t).sqrt())
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

/// 頂点を探すための格子
struct NodeGrid {
    cell: f64,
    cells: IntMap<(i64, i64), Vec<usize>>,
}

impl NodeGrid {
    fn new(cell: f64) -> Self {
        Self {
            cell,
            cells: IntMap::default(),
        }
    }
    fn key(&self, p: Point) -> (i64, i64) {
        (
            (p.x / self.cell).floor() as i64,
            (p.y / self.cell).floor() as i64,
        )
    }
    fn insert(&mut self, p: Point, id: usize) {
        self.cells.entry(self.key(p)).or_default().push(id);
    }
    fn nearest(&self, nodes: &[Point], p: Point, tol: f64) -> Option<usize> {
        // `tol` より近い頂点がありうるマス（ふつうは自分のマスだけ）
        let (x0, y0) = self.key(Point::new(p.x - tol, p.y - tol));
        let (x1, y1) = self.key(Point::new(p.x + tol, p.y + tol));
        let mut best = None;
        let mut best_d = tol;
        for kx in x0..=x1 {
            for ky in y0..=y1 {
                for &id in self.cells.get(&(kx, ky)).into_iter().flatten() {
                    let d = (nodes[id] - p).hypot();
                    if d <= best_d {
                        best_d = d;
                        best = Some(id);
                    }
                }
            }
        }
        best
    }
}

fn snap_node(nodes: &mut Vec<Point>, grid: &mut NodeGrid, p: Point) -> usize {
    if let Some(id) = grid.nearest(nodes, p, FACE_TOL) {
        return id;
    }
    nodes.push(p);
    grid.insert(p, nodes.len() - 1);
    nodes.len() - 1
}

fn quad_deriv(q: &QuadBez, t: f64) -> KVec2 {
    (q.p1 - q.p0) * (2.0 * (1.0 - t)) + (q.p2 - q.p1) * (2.0 * t)
}

/// 2本の曲線の交わる所の（t, u）。曲線を半分ずつに割って、外接矩形が重なる所だけを細かく見ていき、
/// 小さくなったらニュートン法で交点に詰める。`shared` の点（共有する頂点）そのものは除く
fn quad_intersections(qa: &QuadBez, qb: &QuadBez, shared: &[Point]) -> Vec<(f64, f64)> {
    /// 共有する頂点が、両方の曲線のどの端か：（qa の t, qb の t）
    type Ends = [(f64, f64); 2];
    /// 頂点 `v` を端に持つ曲線の一部 `x`・`y`（制御多角形の三角形）が、`v` から見て角度で離れているか
    /// （三角形はそれぞれ `v` から出る2本の向きの間に入るので、向きの範囲が重ならなければ `v` のほかで交わらない）
    fn apart_at(v: Point, x: &QuadBez, y: &QuadBez) -> bool {
        let rays = |q: &QuadBez| -> Option<(KVec2, KVec2)> {
            let (r1, r2) = if q.p0 == v {
                (q.p1 - v, q.p2 - v)
            } else if q.p2 == v {
                (q.p1 - v, q.p0 - v)
            } else {
                return None;
            };
            let (l1, l2) = (r1.hypot(), r2.hypot());
            if l1 < 1e-12 || l2 < 1e-12 {
                return None;
            }
            let (r1, r2) = (r1 / l1, r2 / l2);
            Some(if r1.cross(r2) >= 0.0 {
                (r1, r2)
            } else {
                (r2, r1)
            })
        };
        let (Some((a1, b1)), Some((a2, b2))) = (rays(x), rays(y)) else {
            return false;
        };
        // 向きの範囲（半周より狭い）に `w` が入るか。境目の近くは入るとみなす（離れていると言わない）
        let inside = |a: KVec2, b: KVec2, w: KVec2| a.cross(w) > -1e-9 && w.cross(b) > -1e-9;
        !(inside(a1, b1, a2) || inside(a1, b1, b2) || inside(a2, b2, a1) || inside(a2, b2, b1))
    }
    #[allow(clippy::too_many_arguments)]
    fn rec(
        qa: &QuadBez,
        (a0, a1): (f64, f64),
        qb: &QuadBez,
        (b0, b1): (f64, f64),
        ends: &Ends,
        depth: u32,
        out: &mut Vec<(f64, f64)>,
    ) {
        let (sa, sb) = (qa.subsegment(a0..a1), qb.subsegment(b0..b1));
        let (ra, rb) = (sa.bounding_box(), sb.bounding_box());
        if !rects_overlap(ra.inflate(1e-9, 1e-9), rb) {
            return;
        }
        // 共有する頂点を両方とも端に持つ組：頂点から見て離れていれば、そこで触れるだけ（交わりに数えない点）
        for &(ta, tb) in ends {
            if (ta == a0 || ta == a1) && (tb == b0 || tb == b1) {
                let v = if ta == 0.0 { qa.p0 } else { qa.p2 };
                if apart_at(v, &sa, &sb) {
                    return;
                }
            }
        }
        // 太い線で分ける：片方の曲線は、両端を結ぶ線から、制御点までの距離の半分の帯に入る。
        // もう片方の制御多角形がその帯の外にあれば交わらない（ほぼ平行に並ぶ2本を、外接矩形より早く見分ける）
        let apart = |x: &QuadBez, y: &QuadBez| -> bool {
            let d = x.p2 - x.p0;
            let len = d.hypot();
            if len < 1e-12 {
                return false;
            }
            let n = KVec2::new(-d.y, d.x) / len;
            let h = n.dot(x.p1 - x.p0) * 0.5;
            let (lo, hi) = (h.min(0.0) - 1e-9, h.max(0.0) + 1e-9);
            let ds = [n.dot(y.p0 - x.p0), n.dot(y.p1 - x.p0), n.dot(y.p2 - x.p0)];
            ds.iter().all(|&v| v < lo) || ds.iter().all(|&v| v > hi)
        };
        if apart(&sa, &sb) || apart(&sb, &sa) {
            return;
        }
        let small = |r: KRect| r.width().max(r.height()) < 0.02;
        if (small(ra) && small(rb)) || depth >= 48 {
            let (mut t, mut u) = ((a0 + a1) * 0.5, (b0 + b1) * 0.5);
            for _ in 0..12 {
                let f = qa.eval(t) - qb.eval(u);
                let (da, db) = (quad_deriv(qa, t), quad_deriv(qb, u));
                let det = db.x * da.y - da.x * db.y;
                if det.abs() < 1e-18 {
                    return;
                }
                t = (t + (f.x * db.y - db.x * f.y) / det).clamp(0.0, 1.0);
                u = (u + (da.y * f.x - da.x * f.y) / det).clamp(0.0, 1.0);
            }
            if (qa.eval(t) - qb.eval(u)).hypot() < 1e-7 {
                out.push((t, u));
            }
            return;
        }
        let (am, bm) = ((a0 + a1) * 0.5, (b0 + b1) * 0.5);
        for ra in [(a0, am), (am, a1)] {
            for rb in [(b0, bm), (bm, b1)] {
                rec(qa, ra, qb, rb, ends, depth + 1, out);
            }
        }
    }
    // 共有する頂点（ビットまで同じ点）が、それぞれの曲線のどの端か。無い所は t = 2（どの範囲の端にもならない）
    let mut ends: Ends = [(2.0, 2.0); 2];
    let mut k = 0;
    for v in shared {
        for ta in [0.0, 1.0] {
            for tb in [0.0, 1.0] {
                let end = |q: &QuadBez, t: f64| if t == 0.0 { q.p0 } else { q.p2 };
                if k < 2 && end(qa, ta) == *v && end(qb, tb) == *v {
                    ends[k] = (ta, tb);
                    k += 1;
                }
            }
        }
    }
    let mut raw = Vec::new();
    rec(qa, (0.0, 1.0), qb, (0.0, 1.0), &ends, 0, &mut raw);
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (t, u) in raw {
        let p = qa.eval(t);
        if shared.iter().any(|v| (p - *v).hypot() < 1e-6) {
            continue;
        }
        if !out
            .iter()
            .any(|(t2, u2)| (qa.eval(*t2) - p).hypot() < 1e-6 && (u2 - u).abs() < 1e-3)
        {
            out.push((t, u));
        }
    }
    out
}

struct Seg {
    a: usize,
    b: usize,
    q: QuadBez,
    color: Color32,
    order: usize,
    src: ShapeId,
}

impl PlanarMap {
    pub(crate) fn quad(&self, e: usize) -> QuadBez {
        let ed = &self.edges[e];
        QuadBez::new(self.nodes[ed.a], ed.ctrl, self.nodes[ed.b])
    }

    /// 生の図形の並びから作る。境目の2本の線をそろえて1本の辺にする。
    /// 2本の線の曲線が少し違うときは、並びの前の図形（古い図形）の曲線を使う
    pub(crate) fn from_faces<'a>(faces: impl IntoIterator<Item = &'a FillShape>) -> Self {
        let mut nodes: Vec<Point> = Vec::new();
        let mut node_src: Vec<Option<(ShapeId, usize, usize)>> = Vec::new();
        let mut grid = NodeGrid::new(4.0);
        // 1. 向きのある区間（左が面の色）。端を頂点にまとめる
        let mut segs: Vec<Seg> = Vec::new();
        for (order, f) in faces.into_iter().enumerate() {
            for (ci, c) in f.contours().enumerate() {
                for (si, q) in c.segments.iter().enumerate() {
                    let a = snap_node(&mut nodes, &mut grid, q.p0);
                    if node_src.len() < nodes.len() {
                        node_src.push(Some((f.shape_id, ci, si)));
                    }
                    let b = snap_node(&mut nodes, &mut grid, q.p2);
                    if node_src.len() < nodes.len() {
                        node_src.push(Some((f.shape_id, ci, (si + 1) % c.segments.len())));
                    }
                    if a != b {
                        segs.push(Seg {
                            a,
                            b,
                            q: QuadBez::new(nodes[a], q.p1, nodes[b]),
                            color: f.color,
                            order,
                            src: f.shape_id,
                        });
                    }
                }
            }
        }
        // 区間の途中にある頂点では切らない（地図から書き出した図形では、頂点はいつも区間の端にある。前の版の図形で、
        // 区間の途中に頂点があれば、判定の形で辺が頂点に触れるので正しい地図にならず、塗り重ねて作り直す）
        let split = segs;
        // 2. 同じ2頂点を結び、向きが逆で形の近い区間を、1本の辺にまとめる
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
            let mid = s.q.eval(0.5);
            let twin = groups[&(s.a.min(s.b), s.a.max(s.b))]
                .iter()
                .copied()
                .find(|&j| {
                    !used[j]
                        && split[j].a == s.b
                        && split[j].b == s.a
                        && (split[j].q.eval(0.5) - mid).hypot() <= FACE_TOL
                });
            let (ctrl, right, right_src) = match twin {
                Some(j) => {
                    used[j] = true;
                    let t = &split[j];
                    let ctrl = if t.order < s.order { t.q.p1 } else { s.q.p1 };
                    (ctrl, Some(t.color), Some(t.src))
                }
                None => (s.q.p1, None, None),
            };
            if right == Some(s.color) {
                continue;
            }
            edges.push(MapEdge {
                a: s.a,
                b: s.b,
                ctrl,
                left: Some(s.color),
                right,
                left_src: Some(s.src),
                right_src,
            });
        }
        Self {
            nodes,
            edges,
            node_src,
        }
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
    /// 中に入り込んでいない（重なった図形をそのまま地図にしたものでない）
    pub(crate) fn is_valid(&self) -> bool {
        let polys = self.polys();
        if !self.polys_apart(&polys) {
            return false;
        }
        let cycles = self.face_cycles_with(&polys);
        let (faces, broken) = self.faces_of(&polys, &cycles);
        if broken > 0 {
            return false;
        }
        // 何も無い側の輪郭：その最初の区間の真ん中の点が、その辺を輪郭に持たない色のある面の中（外周の中で、どの穴の外）
        // にあれば、入り込んでいる
        let mut edge_faces: Vec<[usize; 2]> = vec![[usize::MAX; 2]; self.edges.len()];
        let mut regions: Vec<Vec<(GridRect, Option<GridPolygon>)>> =
            Vec::with_capacity(faces.len());
        for (fi, f) in faces.iter().enumerate() {
            for h in f.cycles.iter().flatten() {
                let slot = &mut edge_faces[h.0];
                if slot[0] == usize::MAX {
                    slot[0] = fi;
                } else {
                    slot[1] = fi;
                }
            }
            regions.push(
                f.cycles
                    .iter()
                    .map(|c| (GridRect::of(&cycle_points(&polys, c)), None))
                    .collect(),
            );
        }
        // 面の外周の外接矩形の格子（px で引く）
        let px = |v: i64| v as f64 / OVERLAY_GRID;
        let mut face_grid = BoxGrid::new(64.0);
        for (fi, reg) in regions.iter().enumerate() {
            let r = reg[0].0;
            face_grid.insert(fi, KRect::new(px(r.x0), px(r.y0), px(r.x1), px(r.y1)));
        }
        let mut found = Vec::new();
        for c in &cycles {
            if c.is_empty() || self.side_color(c[0]).is_some() {
                continue;
            }
            let h = c[0];
            let probe = half_mid2(&polys, h);
            let at = Point::new(px(probe.0) * 0.5, px(probe.1) * 0.5);
            face_grid.query(KRect::from_points(at, at), &mut found);
            for &fi in &found {
                let f = &faces[fi];
                if edge_faces[h.0].contains(&fi) || !regions[fi][0].0.contains2(probe) {
                    continue;
                }
                let mut wind = |k: usize| {
                    let (bbox, poly) = &mut regions[fi][k];
                    if !bbox.contains2(probe) {
                        return Some(0);
                    }
                    poly.get_or_insert_with(|| {
                        GridPolygon::new(&cycle_points(&polys, &f.cycles[k]))
                    })
                    .winding(probe)
                };
                let inside = |w: Option<i32>| w.map(|w| w != 0);
                match inside(wind(0)) {
                    None => return false,
                    Some(false) => continue,
                    Some(true) => {}
                }
                let mut in_hole = false;
                for k in 1..f.cycles.len() {
                    match inside(wind(k)) {
                        None => return false,
                        Some(true) => in_hole = true,
                        Some(false) => {}
                    }
                }
                if !in_hole {
                    return false;
                }
            }
        }
        true
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
        let (_, broken) = self.faces_of(&polys, &cycles);
        out += &format!("broken {broken}");
        for c in &cycles {
            if self.is_broken_cycle(c) {
                out += &format!("\n  broken cycle len {}: ", c.len());
                for h in c.iter().take(30) {
                    if h.0 == usize::MAX {
                        out += "OPEN ";
                        continue;
                    }
                    let ed = self.edges[h.0];
                    let straight =
                        (ed.ctrl - self.nodes[ed.a].midpoint(self.nodes[ed.b])).hypot() < 1e-12;
                    out += &format!(
                        "e{}{}({}->{}, {:?}, {}, {}) ",
                        h.0,
                        if h.1 { "+" } else { "-" },
                        if h.1 { ed.a } else { ed.b },
                        if h.1 { ed.b } else { ed.a },
                        self.side_color(*h).map(|c| c.r()),
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
        // （マス, 区間の左端の x, 区間）を並べ、マスごとに左端の順に見る。区間の右端より左端が右にある区間から先は、
        // そのマスでは重ならない。並べる鍵は、マスの x・y と区間の左端の x を1つの数にしたもの（座標は ±2^28）
        let key = |c: (i64, i64), x0: i64| -> u128 {
            ((c.0 + (1 << 31)) as u128) << 96
                | ((c.1 + (1 << 31)) as u128) << 64
                | (x0 + (1 << 62)) as u128
        };
        let mut ent: Vec<(u128, u32)> = Vec::with_capacity(segs.len() * 2);
        for (i, r) in rects.iter().enumerate() {
            for c in cells_of(((r.x0, r.y0), (r.x1, r.y1)), GRID_CELL) {
                ent.push((key(c, r.x0), i as u32));
            }
        }
        ent.sort_unstable();
        let cell0 = |r: &GridRect| (r.x0.div_euclid(GRID_CELL), r.y0.div_euclid(GRID_CELL));
        let cell_of_key = |k: u128| {
            (
                (k >> 96) as i64 - (1 << 31),
                ((k >> 64) as u32) as i64 - (1 << 31),
            )
        };
        let mut start = 0;
        while start < ent.len() {
            let ck = ent[start].0 >> 64;
            let c = cell_of_key(ent[start].0);
            let mut end = start + 1;
            while end < ent.len() && ent[end].0 >> 64 == ck {
                end += 1;
            }
            for x in start..end {
                let i = ent[x].1 as usize;
                let ri = rects[i];
                for &(_, j) in &ent[x + 1..end] {
                    let j = j as usize;
                    let rj = rects[j];
                    if rj.x0 > ri.x1 {
                        break;
                    }
                    // 重ならない組と、両方が入る最初のマス（左下）でないマスの組は飛ばす（同じ組を2度調べない）
                    if !ri.overlaps(&rj) {
                        continue;
                    }
                    let (ci, cj) = (cell0(&ri), cell0(&rj));
                    if (ci.0.max(cj.0), ci.1.max(cj.1)) != c {
                        continue;
                    }
                    let (i, j) = (i.min(j), i.max(j));
                    if self.segs_clash(polys, segs[i], segs[j]) {
                        return false;
                    }
                }
            }
            start = end;
        }
        true
    }

    /// 辺 `e` の区間 `k` と、辺 `f` の区間 `m`（`(e, k)` が前）が、許されない所で触れるか。
    /// 許されるのは、端を1つだけ共有し、そこから同じ向きに重ならず、その端が「2つの辺が共有する頂点」か
    /// 「1本の辺のとなり合う区間のつなぎ目」のときだけ
    fn segs_clash(
        &self,
        polys: &[Vec<IPt>],
        (e, k): (usize, usize),
        (f, m): (usize, usize),
    ) -> bool {
        let (a, b) = (polys[e][k], polys[e][k + 1]);
        let (c, d) = (polys[f][m], polys[f][m + 1]);
        if !segs_touch(a, b, c, d) {
            return false;
        }
        // 共有する端と、それぞれのもう一方の端
        let common = if a == c {
            (a, b, d)
        } else if a == d {
            (a, b, c)
        } else if b == c {
            (b, a, d)
        } else if b == d {
            (b, a, c)
        } else {
            return true;
        };
        let (v, p, q) = common;
        if fan_overlap(v, p, q) {
            return true;
        }
        // 区間が辺の端の頂点 `v` から出ているなら、その頂点の番号
        let node_at = |e: usize, k: usize| -> Option<usize> {
            let poly = &polys[e];
            let ed = self.edges[e];
            if k == 0 && v == poly[0] {
                Some(ed.a)
            } else if k + 2 == poly.len() && v == poly[poly.len() - 1] {
                Some(ed.b)
            } else {
                None
            }
        };
        let same_node = matches!((node_at(e, k), node_at(f, m)), (Some(x), Some(y)) if x == y);
        let permitted = if e == f {
            (m == k + 1 && v == b && v == c) || (m != k && same_node)
        } else {
            same_node
        };
        !permitted
    }

    /// 面を取り出す。外周（面積が正）に、それを含むいちばん小さい同じ色の外周の穴（面積が負）を入れる。
    /// 戻り値の2つ目は、閉じなかった輪郭・色がまざった輪郭・入れ先の無い穴の数（壊れていれば 0 より大きい）
    pub(crate) fn faces(&self) -> (Vec<MapFace>, usize) {
        let polys = self.polys();
        self.faces_of(&polys, &self.face_cycles_with(&polys))
    }

    /// `faces` と同じ。判定の形 `polys` と面の輪郭 `cycles`（`face_cycles_with` の結果）を先に求めてあるときに使う。
    /// 外周か穴かは判定の形の面積の符号、穴の入れ先は判定の形での点の内外で、どちらも整数で決める
    fn faces_of(&self, polys: &[Vec<IPt>], cycles: &[Vec<(usize, bool)>]) -> (Vec<MapFace>, usize) {
        let mut broken = 0;
        // （面, 面積の2倍, 内外を調べる多角形（要るときに作る）, 外接矩形）
        let mut exteriors: Vec<(MapFace, i128, Option<GridPolygon>, GridRect)> = Vec::new();
        let mut holes: Vec<(Color32, Vec<(usize, bool)>)> = Vec::new();
        for cycle in cycles {
            if self.is_broken_cycle(cycle) {
                broken += 1;
                continue;
            }
            let Some(color) = self.side_color(cycle[0]) else {
                continue;
            };
            let pts = cycle_points(polys, cycle);
            let area2 = area2_of(&pts);
            if area2 > 0 {
                exteriors.push((
                    MapFace {
                        color,
                        cycles: vec![cycle.clone()],
                        srcs: Vec::new(),
                    },
                    area2,
                    None,
                    GridRect::of(&pts),
                ));
            } else {
                holes.push((color, cycle.clone()));
            }
        }
        // 穴の入れ先は、穴を含む同じ色の外周のうち、いちばん小さいもの。小さい外周から順に調べ、最初に含むものを選ぶ。
        // 調べる点は、穴の最初の区間の真ん中（正しい地図では、ほかの輪郭の上に無い）
        let mut by_area: Vec<usize> = (0..exteriors.len()).collect();
        by_area.sort_by(|&a, &b| exteriors[a].1.cmp(&exteriors[b].1));
        for (color, cycle) in holes {
            let probe = half_mid2(polys, cycle[0]);
            let owner = by_area.iter().copied().find(|&i| {
                let e = &mut exteriors[i];
                if e.0.color != color || !e.3.contains2(probe) {
                    return false;
                }
                let poly = e
                    .2
                    .get_or_insert_with(|| GridPolygon::new(&cycle_points(polys, &e.0.cycles[0])));
                poly.winding(probe).is_some_and(|w| w != 0)
            });
            match owner {
                Some(i) => exteriors[i].0.cycles.push(cycle),
                None => broken += 1,
            }
        }
        let faces = exteriors
            .into_iter()
            .map(|(mut f, _, _, _)| {
                let mut srcs: Vec<ShapeId> = f
                    .cycles
                    .iter()
                    .flatten()
                    .filter_map(|h| self.side_src(*h))
                    .collect();
                srcs.sort_unstable();
                srcs.dedup();
                f.srcs = srcs;
                f
            })
            .collect();
        (faces, broken)
    }

    /// 面 `f` の今の形を、塗りの形にする
    pub(crate) fn face_shape(&self, f: &MapFace, layer_id: u64, group_path: &[u64]) -> FillShape {
        let mut contours = f
            .cycles
            .iter()
            .map(|c| Contour::new(c.iter().map(|h| self.half_quad(*h)).collect()));
        let exterior = contours.next().unwrap_or_else(|| Contour::new(Vec::new()));
        FillShape::new(f.color, exterior, contours.collect()).with_scope(layer_id, group_path)
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

    /// 頂点 `v` を `target` へ動かした形。つながる辺の制御点は、動いた量の半分だけ動かす（今の頂点の移動と同じ決まり）
    pub(crate) fn with_node_moved(&self, v: usize, target: Point) -> Self {
        let mut m = self.clone();
        let d = target - m.nodes[v];
        m.nodes[v] = target;
        for e in &mut m.edges {
            let k = (e.a == v) as u8 + (e.b == v) as u8;
            if k > 0 {
                e.ctrl += d * (0.5 * k as f64);
            }
        }
        m
    }

    /// 辺 `e` の `grab_t` の所を `target` へ引っぱった形（今の辺のつまみと同じ決まり。両端は動かない）
    pub(crate) fn with_edge_pulled(&self, e: usize, grab_t: f64, target: Point) -> Self {
        let mut m = self.clone();
        let q = m.quad(e);
        let d = target - q.eval(grab_t);
        m.edges[e].ctrl += d / (2.0 * grab_t * (1.0 - grab_t));
        m
    }

    /// `p` から `r` 以内でいちばん近い頂点
    pub(crate) fn hit_node(&self, p: Point, r: f64) -> Option<usize> {
        (0..self.nodes.len())
            .map(|v| (v, (self.nodes[v] - p).hypot()))
            .filter(|(_, d)| *d <= r)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(v, _)| v)
    }

    /// `p` から `r` 以内でいちばん近い辺と、その上のパラメータ
    pub(crate) fn hit_edge(&self, p: Point, r: f64) -> Option<(usize, f64)> {
        (0..self.edges.len())
            .filter(|&e| {
                rects_overlap(
                    self.quad(e).bounding_box(),
                    KRect::from_center_size(p, (2.0 * r, 2.0 * r)),
                )
            })
            .map(|e| {
                let (t, d) = nearest_on_quad(&self.quad(e), p);
                (e, t, d)
            })
            .filter(|x| x.2 <= r)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(e, t, _)| (e, t))
    }
}

impl PlanarMap {
    /// テスト用：辺 `check` とほかの辺の交わる所（`crossings_grid`。`orig` はすべて `false`）
    #[cfg(test)]
    pub(crate) fn crossings_grid_pub(&self, check: &[usize]) -> Vec<(usize, f64, usize, f64)> {
        self.crossings_grid(check, &vec![false; self.edges.len()])
    }

    /// テスト用：点 `p` の色を、半直線で調べた答えと、色ごとの輪郭で調べた答え
    #[cfg(test)]
    pub(crate) fn ray_color_pub(&self, p: Point) -> (Option<Option<Color32>>, Option<Color32>) {
        (
            Some(self.color_at(p)),
            Self::color_in(&self.color_regions(), p),
        )
    }

    /// `check` の辺と、ほかの辺の交わる所（`crossings` と同じ形）。格子で近くの辺だけを調べる。
    /// `orig` が両方 `true` の組（元の地図のまま残った辺どうし。元の地図で交わっていない）は調べない
    fn crossings_grid(&self, check: &[usize], orig: &[bool]) -> Vec<(usize, f64, usize, f64)> {
        const CELL: f64 = 8.0;
        let boxes: Vec<KRect> = (0..self.edges.len())
            .map(|e| self.quad(e).bounding_box())
            .collect();
        let cells = |r: &KRect| {
            let (x0, x1) = ((r.x0 / CELL).floor() as i64, (r.x1 / CELL).floor() as i64);
            let (y0, y1) = ((r.y0 / CELL).floor() as i64, (r.y1 / CELL).floor() as i64);
            (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
        };
        // 格子には、調べる辺のマスにかかる辺だけを入れる（調べる辺が少ないとき、遠くの辺を入れずに済む）
        let mut grid: IntMap<(i64, i64), Vec<usize>> = IntMap::default();
        if 2 * check.len() < self.edges.len() {
            for &e in check {
                for c in cells(&boxes[e]) {
                    grid.entry(c).or_default();
                }
            }
            for (e, b) in boxes.iter().enumerate() {
                for c in cells(b) {
                    if let Some(list) = grid.get_mut(&c) {
                        list.push(e);
                    }
                }
            }
        } else {
            for (e, b) in boxes.iter().enumerate() {
                for c in cells(b) {
                    grid.entry(c).or_default().push(e);
                }
            }
        }
        let mut pos = vec![usize::MAX; self.edges.len()];
        for (mi, &e) in check.iter().enumerate() {
            pos[e] = pos[e].min(mi);
        }
        let mut out = Vec::new();
        let mut seen = vec![usize::MAX; self.edges.len()];
        for (mi, &e) in check.iter().enumerate() {
            let qe = self.quad(e);
            for c in cells(&boxes[e]) {
                let Some(list) = grid.get(&c) else { continue };
                for &f in list {
                    if f == e || pos[f] < mi || seen[f] == mi || !rects_overlap(boxes[e], boxes[f])
                    {
                        continue;
                    }
                    seen[f] = mi;
                    if orig[e] && orig[f] {
                        continue;
                    }
                    let (ea, eb, fa, fb) = (
                        self.edges[e].a,
                        self.edges[e].b,
                        self.edges[f].a,
                        self.edges[f].b,
                    );
                    let shared: Vec<Point> = [ea, eb]
                        .into_iter()
                        .filter(|v| *v == fa || *v == fb)
                        .map(|v| self.nodes[v])
                        .collect();
                    for (t, u) in quad_intersections(&qe, &self.quad(f), &shared) {
                        out.push((e, t, f, u));
                    }
                }
            }
        }
        out
    }

    /// 交わる所で辺を切る。端に当たっている所は、その端の頂点を使う
    fn split_at_crossings(&mut self, hits: &[(usize, f64, usize, f64)]) {
        const END: f64 = 1e-7;
        let mut cuts: Vec<Vec<(f64, usize)>> = vec![Vec::new(); self.edges.len()];
        for &(e, t, f, u) in hits {
            let end_node = |m: &PlanarMap, edge: usize, t: f64| {
                if t < END {
                    Some(m.edges[edge].a)
                } else if t > 1.0 - END {
                    Some(m.edges[edge].b)
                } else {
                    None
                }
            };
            let node = match (end_node(self, e, t), end_node(self, f, u)) {
                (Some(v), Some(w)) => {
                    // 別々の頂点が同じ位置に重なった：1つにまとめる
                    if v != w {
                        for ed in &mut self.edges {
                            if ed.a == w {
                                ed.a = v;
                            }
                            if ed.b == w {
                                ed.b = v;
                            }
                        }
                    }
                    continue;
                }
                (Some(v), None) | (None, Some(v)) => v,
                (None, None) => {
                    let p = self.quad(e).eval(t).midpoint(self.quad(f).eval(u));
                    self.nodes.push(p);
                    self.nodes.len() - 1
                }
            };
            if (END..=1.0 - END).contains(&t) {
                cuts[e].push((t, node));
            }
            if (END..=1.0 - END).contains(&u) {
                cuts[f].push((u, node));
            }
        }
        let old = std::mem::take(&mut self.edges);
        for (i, ed) in old.into_iter().enumerate() {
            let mut cs = std::mem::take(&mut cuts[i]);
            if cs.is_empty() {
                self.edges.push(ed);
                continue;
            }
            cs.sort_by(|x, y| x.0.total_cmp(&y.0));
            let q = QuadBez::new(self.nodes[ed.a], ed.ctrl, self.nodes[ed.b]);
            let (mut t0, mut n0) = (0.0, ed.a);
            for (t, v) in cs.into_iter().chain(std::iter::once((1.0, ed.b))) {
                if v == n0 || t - t0 < 1e-9 {
                    continue;
                }
                self.edges.push(MapEdge {
                    a: n0,
                    b: v,
                    ctrl: q.subsegment(t0..t).p1,
                    ..ed
                });
                t0 = t;
                n0 = v;
            }
        }
    }
}

/// ドラッグの確定。辺がほかの辺と交わらず、ほかの頂点もまたいでいなければ、そのまま。
/// そうでなければ、交わる所で辺を切り、ドラッグで掃いた所の色を決め直す（広がった側が勝つ。Animate と同じ考え方）。
/// `orig_cycles` は `orig` の面の輪郭（`face_cycles` の結果）。戻り値の2つ目は、色を決め直したか
pub(crate) fn commit_edit(
    orig: &PlanarMap,
    orig_cycles: &[Vec<(usize, bool)>],
    edited: PlanarMap,
    moved: &[usize],
) -> (PlanarMap, bool) {
    let hits = edited.crossings_grid(moved, &vec![false; edited.edges.len()]);
    // ドラッグで掃いた所（前の曲線と後の曲線で囲む所）に、ほかの頂点が入ったか
    let moved_nodes: Vec<usize> = moved
        .iter()
        .flat_map(|&e| [edited.edges[e].a, edited.edges[e].b])
        .collect();
    let swept: Vec<BezPath> = moved
        .iter()
        .map(|&e| {
            let (qo, qn) = (orig.quad(e), edited.quad(e));
            let mut p = BezPath::new();
            p.move_to(qo.p0);
            p.quad_to(qo.p1, qo.p2);
            p.line_to(qn.p2);
            p.quad_to(qn.p1, qn.p0);
            p.close_path();
            p
        })
        .collect();
    // ほかの頂点、またはほかの辺の真ん中が、掃いた所に入ったか（両端を共有する細い面が裏返るときは、頂点は入らない）
    let island = (0..edited.nodes.len())
        .filter(|v| !moved_nodes.contains(v))
        .any(|v| swept.iter().any(|p| p.winding(edited.nodes[v]) != 0))
        || (0..edited.edges.len())
            .filter(|f| !moved.contains(f))
            .any(|f| {
                let mid = edited.quad(f).eval(0.5);
                swept.iter().any(|p| p.winding(mid) != 0)
            });
    // 動かした辺を含む面の輪郭が裏返ったか（面積の正負が変わったか）
    let flipped = !hits.is_empty() || island || {
        let area_of = |m: &PlanarMap, cycle: &[(usize, bool)]| {
            Contour::new(cycle.iter().map(|h| m.half_quad(*h)).collect()).signed_area()
        };
        orig_cycles.iter().any(|cycle| {
            cycle.iter().all(|h| h.0 != usize::MAX)
                && cycle.iter().any(|h| moved.contains(&h.0))
                && area_of(orig, cycle).signum() != area_of(&edited, cycle).signum()
        })
    };
    if !flipped {
        return (edited, false);
    }
    let area = moved
        .iter()
        .map(|&e| {
            orig.quad(e)
                .bounding_box()
                .union(edited.quad(e).bounding_box())
        })
        .reduce(|a, b| a.union(b))
        .unwrap_or(KRect::ZERO)
        .inflate(1.0, 1.0);
    // 色ごとの、前と後の塗りの範囲（その色を左に持つ輪郭すべてを1つの道筋にまとめ、巻き数が正の所を中とする）
    let before = orig.color_regions();
    let after = edited.color_regions();
    let color_at = |p: Point| -> Option<Color32> {
        let mut cands: Vec<Color32> = Vec::new();
        for (c, path) in &after {
            if path.winding(p) > 0 && !cands.contains(c) {
                cands.push(*c);
            }
        }
        if cands.len() <= 1 {
            return cands.first().copied();
        }
        let gained = cands
            .iter()
            .find(|c| !before.iter().any(|(bc, bp)| bc == *c && bp.winding(p) > 0));
        Some(*gained.unwrap_or(&cands[0]))
    };
    // 前の位置の辺も仮に足す（「広がった側」の判定は前の位置で切り替わるので、そこも辺にしておく）
    let mut m = edited;
    let mut checked: Vec<usize> = moved.to_vec();
    for &e in moved {
        let qo = orig.quad(e);
        let node_for = |p: Point, m: &mut PlanarMap| -> usize {
            if let Some(v) = (0..m.nodes.len()).find(|&v| (m.nodes[v] - p).hypot() < 1e-9) {
                return v;
            }
            m.nodes.push(p);
            m.nodes.len() - 1
        };
        let a = node_for(qo.p0, &mut m);
        let b = node_for(qo.p2, &mut m);
        m.edges.push(MapEdge {
            a,
            b,
            ctrl: qo.p1,
            left: None,
            right: None,
            left_src: None,
            right_src: None,
        });
        checked.push(m.edges.len() - 1);
    }
    let hits = m.crossings_grid(&checked, &vec![false; m.edges.len()]);
    m.split_at_crossings(&hits);
    // 面ごとに色を1つ決める（面の輪郭を、辺の向きでたどって取り出す）
    for cycle in m.face_cycles() {
        let cycle: Vec<(usize, bool)> = cycle.into_iter().filter(|h| h.0 != usize::MAX).collect();
        if !cycle
            .iter()
            .any(|&(i, _)| rects_overlap(m.quad(i).bounding_box(), area))
        {
            continue;
        }
        let color = m.cycle_sample_color(&cycle, &color_at);
        for &(i, forward) in &cycle {
            if forward {
                m.edges[i].left = color;
            } else {
                m.edges[i].right = color;
            }
        }
    }
    m.edges.retain(|e| e.left != e.right);
    m.merge_degree2(area);
    m.compact_nodes();
    // 安全網：決め直した結果が壊れていたら（輪郭が閉じない・色がまざる・線が交わったまま）、ドラッグを取り消す
    let near: Vec<usize> = (0..m.edges.len())
        .filter(|&i| rects_overlap(m.quad(i).bounding_box(), area))
        .collect();
    if m.cycle_check() != (0, 0)
        || !m
            .crossings_grid(&near, &vec![false; m.edges.len()])
            .is_empty()
    {
        return (orig.clone(), true);
    }
    (m, true)
}

impl PlanarMap {
    /// 向きのある辺（辺, 前向きか）の曲線
    fn half_quad(&self, (i, forward): (usize, bool)) -> QuadBez {
        let q = self.quad(i);
        if forward {
            q
        } else {
            QuadBez::new(q.p2, q.p1, q.p0)
        }
    }

    /// 面の輪郭（左側が同じ面になる、向きのある辺の輪）をすべて取り出す。どの向きのある辺も、ちょうど1つの輪に入る
    pub(crate) fn face_cycles(&self) -> Vec<Vec<(usize, bool)>> {
        self.face_cycles_with(&self.polys())
    }

    /// `face_cycles` と同じ。判定の形 `polys` を先に求めてあるときに使う。頂点のまわりの辺の順番は、判定の形の、
    /// 頂点から出る最初の区間の整数の向きで決める（正しい地図では、同じ向きの2本は無い）
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

    /// 面の輪郭の左側の点で色を調べる。長い辺から順に、ほかの辺に近すぎない点を選び、多数決で決める
    fn cycle_sample_color(
        &self,
        cycle: &[(usize, bool)],
        color_at: &dyn Fn(Point) -> Option<Color32>,
    ) -> Option<Color32> {
        let mut order: Vec<(usize, bool)> = cycle.to_vec();
        order.sort_by(|x, y| {
            let lx = (self.half_quad(*x).p2 - self.half_quad(*x).p0).hypot();
            let ly = (self.half_quad(*y).p2 - self.half_quad(*y).p0).hypot();
            ly.total_cmp(&lx)
        });
        let mut votes: Vec<(Option<Color32>, usize)> = Vec::new();
        for h in order.into_iter().take(5) {
            let q = self.half_quad(h);
            let tan = quad_deriv(&q, 0.5);
            let len = tan.hypot();
            if len < 1e-12 {
                continue;
            }
            let mid = q.eval(0.5);
            let mut delta = (0.25 * (q.p2 - q.p0).hypot()).clamp(1e-5, 0.02);
            let mut sample = None;
            for _ in 0..4 {
                let p = mid + KVec2::new(-tan.y, tan.x) / len * delta;
                let r = KRect::from_center_size(p, (4.0 * delta, 4.0 * delta));
                let too_close = (0..self.edges.len()).any(|j| {
                    j != h.0
                        && rects_overlap(self.quad(j).bounding_box(), r)
                        && nearest_on_quad(&self.quad(j), p).1 < delta
                });
                if !too_close {
                    sample = Some(p);
                    break;
                }
                delta *= 0.25;
            }
            if let Some(p) = sample {
                let c = color_at(p);
                match votes.iter_mut().find(|v| v.0 == c) {
                    Some(v) => v.1 += 1,
                    None => votes.push((c, 1)),
                }
            }
        }
        votes.into_iter().max_by_key(|v| v.1).and_then(|v| v.0)
    }
}

impl PlanarMap {
    /// 調べる用：（閉じない面の輪郭の数, 色がそろわない面の輪郭の数）
    pub(crate) fn cycle_check(&self) -> (usize, usize) {
        self.cycle_counts(&self.face_cycles())
    }

    /// `cycle_check` と同じ。面の輪郭 `cycles`（`face_cycles` の結果）を先に求めてあるときに使う
    fn cycle_counts(&self, cycles: &[Vec<(usize, bool)>]) -> (usize, usize) {
        let is_open = |c: &Vec<(usize, bool)>| c.iter().any(|h| h.0 == usize::MAX);
        let open = cycles.iter().filter(|c| is_open(c)).count();
        let mixed = cycles
            .iter()
            .filter(|c| !is_open(c) && self.is_broken_cycle(c))
            .count();
        (open, mixed)
    }

    /// 面の輪郭 `c`（`face_cycles` の1つ）が壊れている：閉じない、または左の色がそろわない。
    /// 壊れた輪郭の面は図形に書き戻せない
    fn is_broken_cycle(&self, c: &[(usize, bool)]) -> bool {
        c.iter().any(|h| h.0 == usize::MAX)
            || c.windows(2)
                .any(|w| self.side_color(w[0]) != self.side_color(w[1]))
    }
}

impl PlanarMap {
    /// 範囲 `area` の中で、2本の辺だけがつながる頂点のうち、両側の色が同じで、2本を1本の曲線で表せるものを消す
    /// （交わりで切った点が、相手の辺が消えた後に残らないように）。
    /// 頂点は番号の小さい順に調べ、まとめたら、つながりの変わった両端の頂点だけを調べ直す（まとめた結果は、
    /// まとめるたびに最初から調べ直すのと同じ）。辺の並びは、まとめた辺を消すたびに最後の辺をそこへ移すのと同じ順にする
    pub(crate) fn merge_degree2(&mut self, area: KRect) {
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
        let candidate =
            |v: usize, at: &[Vec<usize>]| at[v].len() == 2 && area.contains(self.nodes[v]);
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
            let (d1, d2) = (q1.p1 - q1.p0, q2.p1 - q2.p2);
            let den = d1.cross(d2);
            if den.abs() < 1e-12 {
                continue;
            }
            let s = (q2.p2 - q1.p0).cross(d2) / den;
            let q = QuadBez::new(q1.p0, q1.p0 + d1 * s, q2.p2);
            // 2本が1本の曲線に近く、1本の曲線も2本に近いときだけ（絵の形が変わらないように）
            let fits = [&q1, &q2]
                .iter()
                .all(|qq| (1..4).all(|k| nearest_on_quad(&q, qq.eval(k as f64 / 4.0)).1 < 0.01))
                && nearest_on_quad(&q, self.nodes[v]).1 < 0.01
                && (1..8).all(|k| {
                    let p = q.eval(k as f64 / 8.0);
                    nearest_on_quad(&q1, p).1 < 0.01 || nearest_on_quad(&q2, p).1 < 0.01
                });
            if !fits {
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
        self.node_src.clear();
    }
}

impl PlanarMap {
    /// 色ごとに、その色を左に持つ辺だけをたどって輪郭を作る（線が交わったままの、確定前の形にも使える）。
    /// 戻り値は（色, 巻き数で中を判定する道筋）
    fn color_regions(&self) -> Vec<(Color32, BezPath)> {
        // 向きのある辺：（始点, 終点, 曲線, 色, 始点から出ていく向き, 終点から引き返す向き）。向きは判定の形の端の区間
        let polys = self.polys();
        let dir = |p: IPt, q: IPt| (q.0 - p.0, q.1 - p.1);
        let mut half: Vec<(usize, usize, QuadBez, Color32, IPt, IPt)> =
            Vec::with_capacity(self.edges.len() * 2);
        for (i, e) in self.edges.iter().enumerate() {
            let q = self.quad(i);
            let pl = &polys[i];
            let (first, last) = (pl[0], pl[pl.len() - 1]);
            let (second, before_last) = (pl[1.min(pl.len() - 1)], pl[pl.len().saturating_sub(2)]);
            if let Some(c) = e.left {
                half.push((e.a, e.b, q, c, dir(first, second), dir(last, before_last)));
            }
            if let Some(c) = e.right {
                let r = QuadBez::new(q.p2, q.p1, q.p0);
                half.push((e.b, e.a, r, c, dir(last, before_last), dir(first, second)));
            }
        }
        let mut out_of: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        for (i, h) in half.iter().enumerate() {
            out_of[h.0].push(i);
        }
        let mut used = vec![false; half.len()];
        let mut paths: Vec<(Color32, BezPath)> = Vec::new();
        for start in 0..half.len() {
            if used[start] {
                continue;
            }
            let color = half[start].3;
            let mut path = BezPath::new();
            path.move_to(half[start].2.p0);
            let mut cur = start;
            let closed = loop {
                used[cur] = true;
                path.quad_to(half[cur].2.p1, half[cur].2.p2);
                let back = half[cur].5;
                let next = out_of[half[cur].1]
                    .iter()
                    .copied()
                    .filter(|&h| half[h].3 == color && (!used[h] || h == start))
                    .min_by(|&x, &y| cmp_cw_from(back, half[x].4, half[y].4));
                match next {
                    Some(h) if h == start => break true,
                    Some(h) => cur = h,
                    None => break false,
                }
            };
            if closed {
                path.close_path();
                match paths.iter_mut().find(|p| p.0 == color) {
                    Some(p) => p.1.extend(path.iter()),
                    None => paths.push((color, path)),
                }
            }
        }
        paths
    }
}

/// 辺の形と両側の色が同じかを比べるための鍵（向きはそろえる）
fn edge_key(m: &PlanarMap, e: usize) -> [u64; 8] {
    let ed = &m.edges[e];
    let (p0, p2) = (m.nodes[ed.a], m.nodes[ed.b]);
    let (p0, p2, l, r) = if (p0.x, p0.y) <= (p2.x, p2.y) {
        (p0, p2, ed.left, ed.right)
    } else {
        (p2, p0, ed.right, ed.left)
    };
    let col = |c: Option<Color32>| c.map_or(u64::MAX, |c| u32::from_le_bytes(c.to_array()) as u64);
    [
        p0.x.to_bits(),
        p0.y.to_bits(),
        ed.ctrl.x.to_bits(),
        ed.ctrl.y.to_bits(),
        p2.x.to_bits(),
        p2.y.to_bits(),
        col(l),
        col(r),
    ]
}

/// 確定した地図 `next` を、元の図形に書き戻すときの差分。図形から作った地図 `orig`（`orig_keys` はその辺の鍵）と比べる。
/// 戻り値は（消す元の図形, 新しく入れる塗りの形）。
/// - 形か色が変わった辺の両側の図形は消す
/// - 面のすべての辺が変わらず、側に記された元の図形が1つで、その図形を消さないなら、その面は元の図形のまま残す
/// - それ以外の面は、新しい塗りの形にして入れ、その面に記された元の図形も消す
pub(crate) fn rewrite_shapes(
    orig: &PlanarMap,
    orig_keys: &HashSet<[u64; 8]>,
    next: &PlanarMap,
    orig_shapes: &[FillShape],
    layer_id: u64,
    group_path: &[u64],
) -> (Vec<ShapeId>, Vec<FillShape>) {
    let next_keys: HashSet<[u64; 8]> = (0..next.edges.len()).map(|e| edge_key(next, e)).collect();
    let mut removed: HashSet<ShapeId> = HashSet::new();
    for e in 0..orig.edges.len() {
        if !next_keys.contains(&edge_key(orig, e)) {
            removed.extend(orig.edges[e].left_src);
            removed.extend(orig.edges[e].right_src);
        }
    }
    let (faces, _) = next.faces();
    let mut changed: Vec<&MapFace> = Vec::new();
    for f in &faces {
        let all_same = f
            .cycles
            .iter()
            .flatten()
            .all(|h| orig_keys.contains(&edge_key(next, h.0)));
        let keep = all_same && f.srcs.len() == 1 && !removed.contains(&f.srcs[0]) && {
            f.cycles
                .iter()
                .flatten()
                .all(|h| next.side_src(*h) == Some(f.srcs[0]))
        };
        if !keep {
            changed.push(f);
        }
    }
    for f in &changed {
        removed.extend(f.srcs.iter().copied());
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
    let added = changed
        .into_iter()
        .map(|f| {
            let mut contours = f.cycles.iter().map(|c| {
                reuse(c)
                    .unwrap_or_else(|| Contour::new(c.iter().map(|h| next.half_quad(*h)).collect()))
            });
            let exterior = contours.next().unwrap_or_else(|| Contour::new(Vec::new()));
            FillShape::new(f.color, exterior, contours.collect()).with_scope(layer_id, group_path)
        })
        .collect();
    let mut removed: Vec<ShapeId> = removed.into_iter().collect();
    removed.sort_unstable();
    (removed, added)
}

// ============================================================================
// 選択ツールの輪郭の変形（頂点の移動・辺のつまみ）で使う
// ============================================================================

/// 1つのレイヤーの、編集中の階層の生の図形から作った地図
pub(crate) struct LayerMap {
    pub(crate) layer_id: LayerId,
    pub(crate) group_path: Vec<u64>,
    pub(crate) map: Arc<PlanarMap>,
    /// 図形から作った地図が壊れていて、塗り重ねて作り直したか（`layer_map`。書き戻すときは図形を全部書き換える）
    pub(crate) repainted: bool,
    /// 場面の並びで、このレイヤーの生の図形が最初に出てくる番号（これより後ろの、ほかの図形が前面）
    first_idx: usize,
    /// このレイヤーの、地図にした図形の鍵（図形が同じなら、地図を作り直さずに使い回す）
    key: u64,
}

/// カーソル合わせ用の地図（場面の生の図形が変わるまで使い回す）
#[derive(Default)]
pub(crate) struct ScopeMaps {
    key: u64,
    pub(crate) maps: Vec<LayerMap>,
}

fn is_map_shape(s: &FillShape, scope: &[u64]) -> bool {
    s.is_raw_at_scope(scope) && s.symbol_instance.is_none() && !s.exterior.segments.is_empty()
}

/// 編集中の階層の生の図形（選択中の図形を除く）から、レイヤーごとに地図を作る。前回と図形が同じレイヤーは作り直さない
/// （1回の編集で変わるのは触ったレイヤーだけなので、作り直すのもそのレイヤーだけになる）
pub(crate) fn scope_maps(scene: Scene<'_>, session: &Session) -> Arc<ScopeMaps> {
    use std::hash::{Hash, Hasher};
    let scope = &session.editing_group_path;
    // 選択中の図形（浮いている図形は、ほかの図形と重なることがある）は入れない
    let in_map = |s: &FillShape| is_map_shape(s, scope) && !session.selection.contains_shape(s);
    let mut by_layer: Vec<(LayerId, usize, Vec<&FillShape>)> = Vec::new();
    for (idx, s) in scene.iter().enumerate().filter(|(_, s)| in_map(s)) {
        match by_layer.iter_mut().find(|x| x.0 == s.layer_id) {
            Some(x) => x.2.push(s),
            None => by_layer.push((s.layer_id, idx, vec![s])),
        }
    }
    let layer_key = |layer_id: LayerId, shapes: &[&FillShape]| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (scope, layer_id).hash(&mut h);
        for s in shapes {
            (s.shape_id, s.mesh_revision).hash(&mut h);
        }
        h.finish()
    };
    let keys: Vec<u64> = by_layer
        .iter()
        .map(|(layer_id, _, shapes)| layer_key(*layer_id, shapes))
        .collect();
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        keys.hash(&mut h);
        h.finish()
    };
    let mut cache = session.planar_maps.lock();
    let prev = match cache.as_ref() {
        Some(c) if c.key == key => return c.clone(),
        Some(c) => c.clone(),
        None => Arc::new(ScopeMaps::default()),
    };
    // 地図にできないレイヤー（格子に置けない座標がある）は入れない（カーソル合わせも変形もしない）
    let maps = by_layer
        .into_iter()
        .zip(keys)
        .filter_map(|((layer_id, first_idx, shapes), key)| {
            let (map, repainted) = match prev.maps.iter().find(|m| m.key == key) {
                Some(old) => (old.map.clone(), old.repainted),
                None => {
                    let (map, repainted) = layer_map(&shapes)?;
                    (Arc::new(map), repainted)
                }
            };
            Some(LayerMap {
                layer_id,
                group_path: scope.clone(),
                map,
                repainted,
                first_idx,
                key,
            })
        })
        .collect();
    let built = Arc::new(ScopeMaps { key, maps });
    *cache = Some(built.clone());
    built
}

/// 点 `p` が、地図 `lm` のレイヤーの生の図形より前面にある、ほかの図形（前面のレイヤー・同じレイヤーのグループ）に覆われているか
fn hidden_by_front(scene: Scene<'_>, session: &Session, lm: &LayerMap, p: Point) -> bool {
    (lm.first_idx + 1..scene.len()).any(|i| {
        scene.get(i).is_some_and(|s| {
            !(s.layer_id == lm.layer_id && s.is_raw_at_scope(&lm.group_path))
                && s.is_in_editing_scope(&session.editing_group_path)
                && s.contains_point(p)
        })
    })
}

/// アンカーの追加（`add`）・削除で、マウスにいちばん近い辺・頂点を探す（半径 `12 / zoom`。同じレイヤーの生の図形どうしに前後は無い）。
/// 追加は辺の上の位置を 0.08〜0.92 に収め、足せる所（`with_anchor_added` が形を返す所）だけ。削除は、消せる頂点
/// （`with_anchor_deleted` が形を返す頂点）だけ
pub(crate) fn anchor_hover(
    scene: Scene<'_>,
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
    add: bool,
) -> HoverTarget {
    let r = 12.0 / (zoom as f64);
    let maps = scope_maps(scene, session);
    if add {
        let mut best: Option<(f64, &LayerMap, usize, f64)> = None;
        for lm in &maps.maps {
            let Some((e, t)) = lm.map.hit_edge(mouse_pt, r) else {
                continue;
            };
            let p = lm.map.quad(e).eval(t);
            let d = (p - mouse_pt).hypot();
            if best.as_ref().is_none_or(|b| d < b.0)
                && !hidden_by_front(scene, session, lm, p)
                && lm.map.with_anchor_added(e, t).is_some()
            {
                best = Some((d, lm, e, t.clamp(0.08, 0.92)));
            }
        }
        return best.map_or(HoverTarget::None, |(_, lm, e, t)| {
            let q = lm.map.quad(e);
            let ed = &lm.map.edges[e];
            HoverTarget::Edge {
                shape_id: ed.left_src.or(ed.right_src).unwrap_or(0),
                contour_idx: 0,
                seg_idx: 0,
                t,
                closest_pos: q.eval(t),
                seg: q,
                map_edge: Some((lm.layer_id, e)),
            }
        });
    }
    let mut best: Option<(f64, &LayerMap, usize)> = None;
    for lm in &maps.maps {
        let Some(v) = lm.map.hit_node(mouse_pt, r) else {
            continue;
        };
        let p = lm.map.nodes[v];
        let d = (p - mouse_pt).hypot();
        if best.as_ref().is_none_or(|b| d < b.0)
            && !hidden_by_front(scene, session, lm, p)
            && lm.map.with_anchor_deleted(v).is_some()
        {
            best = Some((d, lm, v));
        }
    }
    best.map_or(HoverTarget::None, |(_, lm, v)| {
        let (shape_id, contour_idx, seg_idx) = lm
            .map
            .node_src
            .get(v)
            .copied()
            .flatten()
            .unwrap_or((0, 0, 0));
        HoverTarget::Vertex {
            shape_id,
            contour_idx,
            seg_idx,
            pos: lm.map.nodes[v],
            map_node: Some((lm.layer_id, v)),
        }
    })
}

/// 選択ツールの輪郭の変形で、マウスにいちばん近い頂点か辺を探す。
/// 同じレイヤーの生の図形どうしに前後は無い（境目の線は1本）。前面のレイヤーやグループに隠れた所は除く
pub(crate) fn reshape_hover(
    scene: Scene<'_>,
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
) -> HoverTarget {
    let inv_z = 1.0 / (zoom as f64);
    let (vertex_r, inner_r, outer_r) = (9.0 * inv_z, 5.5 * inv_z, 9.5 * inv_z);
    let maps = scope_maps(scene, session);
    let hidden = |lm: &LayerMap, p: Point| hidden_by_front(scene, session, lm, p);
    let mut best_v: Option<(f64, &LayerMap, usize)> = None;
    for lm in &maps.maps {
        if let Some(v) = lm.map.hit_node(mouse_pt, vertex_r) {
            let d = (lm.map.nodes[v] - mouse_pt).hypot();
            if best_v.as_ref().is_none_or(|b| d < b.0) && !hidden(lm, lm.map.nodes[v]) {
                best_v = Some((d, lm, v));
            }
        }
    }
    if let Some((_, lm, v)) = best_v {
        let (shape_id, contour_idx, seg_idx) = lm
            .map
            .node_src
            .get(v)
            .copied()
            .flatten()
            .unwrap_or((0, 0, 0));
        return HoverTarget::Vertex {
            shape_id,
            contour_idx,
            seg_idx,
            pos: lm.map.nodes[v],
            map_node: Some((lm.layer_id, v)),
        };
    }
    let mut best_e: Option<(f64, &LayerMap, usize, f64, ShapeId)> = None;
    for lm in &maps.maps {
        let Some((e, t)) = lm.map.hit_edge(mouse_pt, outer_r) else {
            continue;
        };
        let q = lm.map.quad(e);
        let p = q.eval(t);
        let d = (p - mouse_pt).hypot();
        let tan = quad_deriv(&q, t);
        let on_left = (mouse_pt - p).dot(KVec2::new(-tan.y, tan.x)) >= 0.0;
        let ed = &lm.map.edges[e];
        let (side, side_src) = if on_left {
            (ed.left, ed.left_src)
        } else {
            (ed.right, ed.right_src)
        };
        let max = if side.is_some() { inner_r } else { outer_r };
        let src = side_src.or(ed.left_src).or(ed.right_src).unwrap_or(0);
        if d <= max && best_e.as_ref().is_none_or(|b| d < b.0) && !hidden(lm, p) {
            best_e = Some((d, lm, e, t, src));
        }
    }
    if let Some((_, lm, e, t, shape_id)) = best_e {
        let q = lm.map.quad(e);
        return HoverTarget::Edge {
            shape_id,
            contour_idx: 0,
            seg_idx: 0,
            t,
            closest_pos: q.eval(t),
            seg: q,
            map_edge: Some((lm.layer_id, e)),
        };
    }
    HoverTarget::None
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

    /// 辺 `e` を `t`（0.08〜0.92 に収める）の所で2本に切った形（形は変わらない）
    pub(crate) fn with_anchor_added(&self, e: usize, t: f64) -> Option<PlanarMap> {
        let t = t.clamp(0.08, 0.92);
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

    /// 頂点 `v` を `p` へ動かすときに吸い付く、ほかの頂点（`radius` 以内でいちばん近いもの）
    pub(crate) fn snap_target(&self, v: usize, p: Point, radius: f64) -> Option<Point> {
        (0..self.nodes.len())
            .filter(|&w| w != v)
            .map(|w| (self.nodes[w], (self.nodes[w] - p).hypot()))
            .filter(|(_, d)| *d <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(q, _)| q)
    }
}

/// 輪郭の変形の元になる地図と、ドラッグのあいだ変わらない下ごしらえ（面の輪郭・辺の鍵）。
/// ドラッグを始めるときに1回だけ作り、ドラッグ中の毎回の計算で使い回す
#[derive(Debug)]
pub(crate) struct ReshapeBase {
    pub(crate) map: Arc<PlanarMap>,
    /// 塗り重ねて作り直した地図か（`layer_map`）。書き戻すときは、そのレイヤーの図形を全部書き換える
    repainted: bool,
    cycles: Vec<Vec<(usize, bool)>>,
    keys: HashSet<[u64; 8]>,
}

impl ReshapeBase {
    pub(crate) fn new(map: Arc<PlanarMap>, repainted: bool) -> Self {
        let cycles = map.face_cycles();
        let keys = (0..map.edges.len()).map(|e| edge_key(&map, e)).collect();
        Self {
            map,
            repainted,
            cycles,
            keys,
        }
    }
}

/// 地図 `base.map` に編集 `edit` をかけて確定した結果を、図形の差分で返す（ドラッグ中の表示と確定で同じ計算）。
/// 戻り値は（消す元の図形, 新しく入れる塗りの形）。`orig_shapes` は、形の変わらない輪郭を使い回すために元の図形を探す所
pub(crate) fn reshape_result(
    base: &ReshapeBase,
    edit: MapEdit,
    orig_shapes: &[FillShape],
    layer_id: LayerId,
    group_path: &[u64],
) -> Option<(Vec<ShapeId>, Vec<FillShape>)> {
    let map = &base.map;
    // 編集した地図
    let next = match edit {
        MapEdit::AddAnchor { orig_seg, t } => {
            let (e, forward) = map.find_edge(&orig_seg)?;
            map.with_anchor_added(e, if forward { t } else { 1.0 - t })?
        }
        MapEdit::DeleteAnchor { orig_vertex } => {
            let v = (0..map.nodes.len()).find(|&v| map.nodes[v] == orig_vertex)?;
            map.with_anchor_deleted(v)?
        }
        MapEdit::PullEdge {
            orig_seg,
            grab_t,
            target,
        } => {
            let (e, forward) = map.find_edge(&orig_seg)?;
            let t = if forward { grab_t } else { 1.0 - grab_t };
            commit_edit(map, &base.cycles, map.with_edge_pulled(e, t, target), &[e]).0
        }
        MapEdit::MoveVertex {
            orig_vertex,
            target,
        } => {
            let v = (0..map.nodes.len()).find(|&v| map.nodes[v] == orig_vertex)?;
            commit_edit(
                map,
                &base.cycles,
                map.with_node_moved(v, target),
                &map.edges_at(v),
            )
            .0
        }
    };
    if base.repainted {
        let own: Vec<&FillShape> = orig_shapes
            .iter()
            .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path))
            .collect();
        return Some(all_rewritten(&own, &next, layer_id, group_path));
    }
    Some(rewrite_shapes(
        map,
        &base.keys,
        &next,
        orig_shapes,
        layer_id,
        group_path,
    ))
}

/// 図形の並び `shapes` のうち、レイヤー `layer_id`・階層 `group_path` の生の図形を地図にして、編集 `edit` をかけて確定した差分
/// （消す元の図形, 新しく入れる塗りの形）。形が変わらないときは `None`
pub(crate) fn reshape_diff(
    shapes: &[FillShape],
    layer_id: LayerId,
    group_path: &[u64],
    edit: MapEdit,
) -> Option<(Vec<ShapeId>, Vec<FillShape>)> {
    let own: Vec<&FillShape> = shapes
        .iter()
        .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path))
        .collect();
    let (map, repainted) = layer_map(&own)?;
    let base = ReshapeBase::new(Arc::new(map), repainted);
    reshape_result(&base, edit, shapes, layer_id, group_path)
        .filter(|(removed, added)| !removed.is_empty() || !added.is_empty())
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

/// 矩形選択で切った結果（レイヤー1枚・階層1つ分）
#[derive(Debug, Default)]
pub(crate) struct MarqueeSplit {
    /// 矩形にかかって切れた図形（文書には残し、画面から隠す元の図形）
    pub(crate) hidden: Vec<ShapeId>,
    /// 丸ごと矩形の中にある図形（切らずに、そのまま選ぶ）
    pub(crate) whole: Vec<ShapeId>,
    /// 切れた図形の、矩形の外の部分
    pub(crate) remainder: Vec<FillShape>,
    /// 切れた図形の、矩形の中の部分（持ち上げる片）
    pub(crate) pieces: Vec<FillShape>,
}

/// 矩形選択：図形の並び `shapes` のうち、レイヤー `layer_id`・階層 `group_path` の生の図形（`skip` に当たるものは除く）を地図にして、
/// 矩形の形 `rect` で切る。矩形の中の部分と外の部分は、同じ地図から作るので、境目が両側でビットまで同じになる。
/// 地図で切れないとき（安全網で止まった）は `None`
pub(crate) fn marquee_split(
    shapes: &[FillShape],
    layer_id: LayerId,
    group_path: &[u64],
    skip: impl Fn(&FillShape) -> bool,
    rect: &FillShape,
) -> Option<MarqueeSplit> {
    let own: Vec<FillShape> = shapes
        .iter()
        .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path) && !skip(s))
        .cloned()
        .collect();
    let own_refs: Vec<&FillShape> = own.iter().collect();
    let (map, repainted) = layer_map(&own_refs)?;
    // 矩形は形として重ねる（中か外かは i_overlay が数えるので、地図として重ねるより軽い）
    let rect_input = MapInput::Shapes(std::slice::from_ref(rect));
    let inside = map.overlay(rect_input, MapRule::KeepInside)?;
    let rest = map.overlay(rect_input, MapRule::Paint(None))?;
    if repainted {
        // 地図を塗り重ねて作り直したときは、レイヤーの生の図形を全部隠して、中の片と外の残りに置き換える
        let (hidden, remainder) = all_rewritten(&own_refs, &rest, layer_id, group_path);
        let (_, pieces) = all_rewritten(&own_refs, &inside, layer_id, group_path);
        return Some(MarqueeSplit {
            hidden,
            whole: Vec::new(),
            remainder,
            pieces,
        });
    }
    let keys: HashSet<[u64; 8]> = (0..map.edges.len()).map(|e| edge_key(&map, e)).collect();
    let (removed_rest, remainder) = rewrite_shapes(&map, &keys, &rest, &own, layer_id, group_path);
    let (removed_inside, pieces) = rewrite_shapes(&map, &keys, &inside, &own, layer_id, group_path);
    // 残りから消えて、中では変わらない図形が、丸ごと中にある図形。どちらでも変わった図形が、切れた図形
    let hidden: Vec<ShapeId> = removed_rest
        .iter()
        .copied()
        .filter(|id| removed_inside.contains(id))
        .collect();
    let whole: Vec<ShapeId> = removed_rest
        .iter()
        .copied()
        .filter(|id| !removed_inside.contains(id))
        .collect();
    Some(MarqueeSplit {
        hidden,
        whole,
        remainder,
        pieces,
    })
}

/// 持ち上げた片 `pieces`（同じ地図から作った、重ならない片）を、図形の並び `shapes` のレイヤー `layer_id`・階層 `group_path` の
/// 生の図形の上に置いた差分。片を1つの地図にして、今の地図に重ねる（片どうしの境目は1本の辺のまま）。
/// 片を置く所に今の図形の線が1本も無く、何も塗られていないときは、片をそのまま足す（形も番号も変えない）
pub(crate) fn place_pieces_diff(
    shapes: &[FillShape],
    layer_id: LayerId,
    group_path: &[u64],
    pieces: &[FillShape],
) -> MapEditOutcome {
    // 片の地図：そのまま正しい地図になればそれを、ならなければ（片どうしが重なる：浮いている片の上に別の図形を足したときなど）
    // 並びの順に塗り重ねた地図
    let as_is = PlanarMap::from_faces(pieces.iter());
    let (piece_map, valid) = if as_is.is_valid() {
        (as_is, true)
    } else {
        let refs: Vec<&FillShape> = pieces.iter().collect();
        match painted_map(&refs) {
            Some(m) => (m, false),
            None => return MapEditOutcome::Refused,
        }
    };
    let Some(area) = (0..piece_map.edges.len())
        .map(|e| piece_map.quad(e).bounding_box())
        .reduce(|a, b| a.union(b))
        .map(|r| r.inflate(1.0, 1.0))
    else {
        return MapEditOutcome::Unchanged;
    };
    let own: Vec<&FillShape> = shapes
        .iter()
        .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path))
        .collect();
    let near = own.iter().any(|s| {
        rects_overlap(s.bounding_box(), area)
            && s.contours().any(|c| {
                c.segments
                    .iter()
                    .any(|q| rects_overlap(q.bounding_box(), area))
            })
    });
    // 置く所（外接矩形）に線が1本も無ければ、そこは1つの面の中か、何も無い所。真ん中の点が何も無い所なら、そのまま足す
    if !near && !own.iter().any(|s| s.contains_point(area.center())) {
        let added = if valid {
            pieces
                .iter()
                .map(|p| p.clone().with_scope(layer_id, group_path))
                .collect()
        } else {
            piece_map
                .faces()
                .0
                .iter()
                .map(|f| piece_map.face_shape(f, layer_id, group_path))
                .collect()
        };
        return MapEditOutcome::Changed(Vec::new(), added);
    }
    map_edit_diff(shapes, layer_id, group_path, |m| {
        m.overlay(MapInput::Map(&piece_map), MapRule::Overlay)
    })
}

/// 境目がビットまで同じ図形どうし（同じ地図から作った片）の、同じ色でとなり合うものを1つにまとめる。
/// 図形どうしは重ならないこと。レイヤー・階層ごとにまとめる
pub(crate) fn merge_touching_faces(shapes: Vec<FillShape>) -> Vec<FillShape> {
    let mut groups: Vec<(LayerId, Vec<u64>, Vec<FillShape>)> = Vec::new();
    for s in shapes {
        match groups
            .iter_mut()
            .find(|g| g.0 == s.layer_id && g.1 == s.group_path)
        {
            Some(g) => g.2.push(s),
            None => groups.push((s.layer_id, s.group_path.clone(), vec![s])),
        }
    }
    let mut out = Vec::new();
    for (layer_id, group_path, list) in groups {
        if list.len() <= 1 {
            out.extend(list);
            continue;
        }
        let map = PlanarMap::from_faces(list.iter());
        if !map.is_valid() {
            // 正しい地図にならない（境目がそろっていない・重なる）ときは、まとめずにそのまま
            out.extend(list);
            continue;
        }
        let faces: Vec<FillShape> = map
            .faces()
            .0
            .iter()
            .map(|f| map.face_shape(f, layer_id, &group_path))
            .collect();
        // 形の変わらない片は、元の図形（番号）をそのまま使う
        for f in faces {
            match list.iter().find(|s| {
                s.color == f.color
                    && s.exterior.segments == f.exterior.segments
                    && s.holes.len() == f.holes.len()
            }) {
                Some(orig) => out.push(orig.clone()),
                None => out.push(f),
            }
        }
    }
    out
}

/// 地図で編集した結果
#[derive(Debug)]
pub(crate) enum MapEditOutcome {
    /// 形が変わった：（消す元の図形, 新しく入れる塗りの形）
    Changed(Vec<ShapeId>, Vec<FillShape>),
    /// 形が変わらない
    Unchanged,
    /// 地図にできない（格子に置けない座標がある：±65,536px の外）
    Refused,
}

/// レイヤー `target_layer_id`・階層 `target_group_path` の生の図形の置き場（その階層の生の図形の後ろ。グループ・シンボルより下）
pub(crate) fn raw_scope_insert_index(
    canvas: &[FillShape],
    target_layer_id: u64,
    target_group_path: &[u64],
) -> usize {
    canvas
        .iter()
        .rposition(|s| {
            s.layer_id == target_layer_id
                && s.group_path == target_group_path
                && s.symbol_instance.is_none()
        })
        .map(|pos| pos + 1)
        .or_else(|| {
            canvas.iter().position(|s| {
                s.layer_id == target_layer_id
                    && s.is_in_editing_scope(target_group_path)
                    && !s.is_raw_at_scope(target_group_path)
            })
        })
        .or_else(|| {
            canvas
                .iter()
                .rposition(|s| {
                    s.layer_id == target_layer_id && s.is_in_editing_scope(target_group_path)
                })
                .map(|pos| pos + 1)
        })
        .or_else(|| {
            canvas
                .iter()
                .rposition(|s| s.layer_id == target_layer_id)
                .map(|pos| pos + 1)
        })
        .unwrap_or(canvas.len())
}

/// 図形の並び `shapes` のうち、レイヤー `layer_id`・階層 `group_path` の生の図形を地図にして（`layer_map`）、`edit` で編集した差分。
/// 地図を塗り重ねて作り直したとき（壊れた地図）は、レイヤーの生の図形を全部書き直す
pub(crate) fn map_edit_diff(
    shapes: &[FillShape],
    layer_id: LayerId,
    group_path: &[u64],
    edit: impl FnOnce(&PlanarMap) -> Option<PlanarMap>,
) -> MapEditOutcome {
    let own: Vec<&FillShape> = shapes
        .iter()
        .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path))
        .collect();
    let Some((map, repainted)) = layer_map(&own) else {
        return MapEditOutcome::Refused;
    };
    let Some(next) = edit(&map) else {
        return MapEditOutcome::Refused;
    };
    let (removed, added) = if repainted {
        all_rewritten(&own, &next, layer_id, group_path)
    } else {
        let keys: HashSet<[u64; 8]> = (0..map.edges.len()).map(|e| edge_key(&map, e)).collect();
        rewrite_shapes(&map, &keys, &next, shapes, layer_id, group_path)
    };
    if removed.is_empty() && added.is_empty() {
        MapEditOutcome::Unchanged
    } else {
        MapEditOutcome::Changed(removed, added)
    }
}

/// 生の図形 `own`（下から上への並び）の地図と、塗り重ねて作り直したか。図形から作った地図が正しい地図でなければ
/// （`is_valid`。前の版で描いた、境目がそろっていない図形や重なった図形など）、並びの順に塗り重ねて作り直す
/// （`painted_map`。絵は同じ）。`None`：格子に置けない座標がある
fn layer_map(own: &[&FillShape]) -> Option<(PlanarMap, bool)> {
    let map = PlanarMap::from_faces(own.iter().copied());
    if !map.is_valid() {
        return Some((painted_map(own)?, true));
    }
    Some((map, false))
}

/// 図形 `shapes`（下から上への並び）を、並びの順に空の地図へ塗り重ねた地図（重なる所は上の図形が見える。絵の決まりそのもの）。
/// 重ならずにそのまま正しい地図になる続きの図形は、まとめて1回で重ねる（正しい地図の一部はいつも正しいので、いちばん長い続きを
/// 二分法で探す）。1つだけでも正しい地図にならない図形（輪郭が自分と交わる）は、塗りの形として重ねる（偶奇の規則）。
/// `None`：格子に置けない座標がある
fn painted_map(shapes: &[&FillShape]) -> Option<PlanarMap> {
    let valid = |r: &[&FillShape]| PlanarMap::from_faces(r.iter().copied()).is_valid();
    let mut acc = PlanarMap::default();
    let mut i = 0;
    while i < shapes.len() {
        if !valid(&shapes[i..i + 1]) {
            let one = (*shapes[i]).clone();
            acc = acc.overlay(
                MapInput::Shapes(std::slice::from_ref(&one)),
                MapRule::Paint(Some(one.color)),
            )?;
            i += 1;
            continue;
        }
        let (mut lo, mut hi) = (i + 1, shapes.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if valid(&shapes[i..mid]) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let run = PlanarMap::from_faces(shapes[i..lo].iter().copied());
        acc = acc.overlay(MapInput::Map(&run), MapRule::Overlay)?;
        i = lo;
    }
    Some(acc)
}

/// テスト用：図形 `shapes`（下から上への並び）を塗り重ねた地図（`painted_map`）
#[cfg(test)]
pub(crate) fn painted_map_pub(shapes: &[FillShape]) -> Option<PlanarMap> {
    let refs: Vec<&FillShape> = shapes.iter().collect();
    painted_map(&refs)
}

/// 地図を作り直したときの差分：生の図形 `own` を全部消し、地図 `next` の面を全部入れる
fn all_rewritten(
    own: &[&FillShape],
    next: &PlanarMap,
    layer_id: LayerId,
    group_path: &[u64],
) -> (Vec<ShapeId>, Vec<FillShape>) {
    (
        own.iter().map(|s| s.shape_id).collect(),
        next.faces()
            .0
            .iter()
            .map(|f| next.face_shape(f, layer_id, group_path))
            .collect(),
    )
}

// ============================================================================
// 形・地図を地図に重ねる（ブラシ・消しゴム・矩形で選んだ片の取り出し・持ち上げた片を置く・貼り付け）
//
// 重ねる線と前からある辺を細かい折れ線にして、i_overlay（vendor/i_overlay）の `EdgeOverlay` で交わる所で切る。
// 交わりは整数の格子（1/4096px）で計算するので、ほぼ重なった線でも食い違わない。切った区間ごとに、
// 左右が重ねる形の中か（i_overlay が数える）と、出どころ（どの曲線の t のどこからどこまでか）が分かる。
// 区間の左右の色は、切った区間でできる面ごとに決める（`face_colors`）。境目の曲線は、出どころの曲線から当て直す。
// ============================================================================

/// 重ねるときの格子（1px をいくつに分けるか）。地図の頂点のうち重ねる所にかかるものは、この格子の上に置き直す
const OVERLAY_GRID: f64 = 4096.0;
/// 曲線を折れ線にするときの、曲線と折れ線の離れ具合の上限（px）
const FLAT_TOL: f64 = 0.002;
/// i_overlay が交点を格子の点に丸めるときに、点が動く距離の上限（格子の目の対角線の半分、px）
const GRID_REACH: f64 = std::f64::consts::FRAC_1_SQRT_2 / OVERLAY_GRID;

/// 重ねるときの、中と外の色の決め方
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MapRule {
    /// 中をこの色にする（`None` は何も無い。消しゴム・面を空ける）。外は前のまま
    Paint(Option<Color32>),
    /// 中は前のまま、外を何も無いにする（矩形で選んだ中の片を取り出す）
    KeepInside,
    /// 重ねる地図の色のある所を、その色にする（何も無い所は前のまま）。持ち上げた片を置く・貼り付け
    Overlay,
}

impl MapRule {
    /// `inside`：重ねるもの（地図なら色のある面）の中か。`p`：重ねる地図の色。`old`：前の色
    fn apply(self, inside: bool, p: Option<Color32>, old: Option<Color32>) -> Option<Color32> {
        match self {
            MapRule::Paint(c) => {
                if inside {
                    c
                } else {
                    old
                }
            }
            MapRule::KeepInside => {
                if inside {
                    old
                } else {
                    None
                }
            }
            MapRule::Overlay => {
                if inside {
                    p
                } else {
                    old
                }
            }
        }
    }
}

/// 重ねるもの：塗りの形（中は巻き数が奇数の所。i_overlay が数える）か、地図（中は色のある面）
#[derive(Clone, Copy)]
pub(crate) enum MapInput<'a> {
    Shapes(&'a [FillShape]),
    Map(&'a PlanarMap),
}

/// 区間の出どころ：（出どころの曲線の番号, 区間の始点の t, 終点の t）
#[derive(Clone, Copy, Debug)]
struct Prov {
    src: u32,
    t0: f64,
    t1: f64,
}

/// 区間の出どころの一覧をしまう所（i_overlay が区間を切る・まとめる・向きを変えるたびに増える）
#[derive(Default)]
struct ProvStore(Vec<Vec<Prov>>);

impl ProvStore {
    fn push(&mut self, v: Vec<Prov>) -> ProvId {
        self.0.push(v);
        ProvId(self.0.len() as u32 - 1)
    }
}

/// 区間の出どころの一覧の番号（`ProvStore` の中）。i_overlay の区間に付ける印
#[derive(Clone, Copy, PartialEq, Debug)]
struct ProvId(u32);

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
        let first = src
            .iter()
            .map(|p| Prov {
                src: p.src,
                t0: p.t0,
                t1: mid(p),
            })
            .collect();
        let second = src
            .iter()
            .map(|p| Prov {
                src: p.src,
                t0: mid(p),
                t1: p.t1,
            })
            .collect();
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
enum OverlaySrc {
    /// 前からある辺（番号）
    Old(usize),
    /// 重ねる形の区間・重ねる地図の辺（番号）
    New(usize),
}

/// 曲線を折れ線にする（`t` つき）。曲線と折れ線の離れ具合が `tol` 以下になる等分
fn flatten_with_t(q: &QuadBez, tol: f64) -> Vec<(Point, f64)> {
    let n = flat_count(q, tol);
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            (q.eval(t), t)
        })
        .collect()
}

/// 曲線を、折れ線とのずれが `tol` ほどになるように等分する数
fn flat_count(q: &QuadBez, tol: f64) -> usize {
    let dd = (q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2()).hypot();
    // 2次ベジェを n 等分した折れ線の離れ具合は、およそ |p0 - 2p1 + p2| / (4 n²)
    ((dd / (4.0 * tol)).sqrt().ceil() as usize).clamp(1, 512)
}

/// 整数の向きを、角度の順（x 軸の正の向きから、面積の式で正の回り方）に比べる（向きは格子の点の差か、その積。
/// 外積は i128 に収まる）
fn cmp_dir(a: (i128, i128), b: (i128, i128)) -> std::cmp::Ordering {
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
fn wide(d: IPt) -> (i128, i128) {
    (d.0 as i128, d.1 as i128)
}

/// 格子に丸めて同じ区間になった、ほぼ重なった辺の左右の色（区間の向きにそろえたもの）を1組にまとめる。
/// 辺を実際の位置（区間の真ん中にあたる点が、区間のどちら側にあるか）で並べ、いちばん左の辺の左と、いちばん右の辺の右を使う。
/// `entries`：（左右の色, その辺の上の点）
fn combine_sides(mut entries: Vec<(Sides, Point)>, mid: Point, dir: KVec2) -> Option<Sides> {
    if entries.len() <= 1 {
        return entries.pop().map(|e| e.0);
    }
    entries.sort_by(|x, y| dir.cross(y.1 - mid).total_cmp(&dir.cross(x.1 - mid)));
    Some((entries[0].0 .0, entries[entries.len() - 1].0 .1))
}

impl PlanarMap {
    /// 点 `p`（格子の点に丸める）の色：判定の形で、点のある所の色（`LabeledSegs::at`）
    #[cfg(test)]
    pub(crate) fn color_at(&self, p: Point) -> Option<Color32> {
        let segs = LabeledSegs::new((0..self.edges.len()).flat_map(|e| {
            let (l, r) = (self.edges[e].left, self.edges[e].right);
            let pl = grid_poly(&self.quad(e));
            (0..pl.len().saturating_sub(1))
                .map(|k| (pl[k], pl[k + 1], l, r))
                .collect::<Vec<_>>()
        }));
        let g = gpt(p);
        segs.at((2 * g.0, 2 * g.1)).flatten()
    }

    /// 点 `p` の色（`color_regions` の結果から）
    #[cfg(test)]
    fn color_in(regions: &[(Color32, BezPath)], p: Point) -> Option<Color32> {
        regions
            .iter()
            .find(|(_, path)| path.winding(p) > 0)
            .map(|(c, _)| *c)
    }

    /// 地図全体を `aff` で動かした写し（頂点と制御点を動かす。辺のつながりはそのまま）
    #[cfg(test)]
    pub(crate) fn transformed(&self, aff: kurbo::Affine) -> PlanarMap {
        let mut m = self.clone();
        for p in &mut m.nodes {
            *p = aff * *p;
        }
        for e in &mut m.edges {
            e.ctrl = aff * e.ctrl;
        }
        m
    }

    /// バケツ：点 `p` を含む面（辺で囲まれた所。何も塗っていない所も）を `color` にする。
    /// どの輪郭にも囲まれていない所・すでに `color` の面は、何も変えない（同じ地図を返す）。
    /// 地図に壊れた輪郭があって、囲まれていない所かどうかを決められないときは `None`
    pub(crate) fn with_face_filled(&self, p: Point, color: Color32) -> Option<PlanarMap> {
        let polys = self.polys();
        let all = self.face_cycles_with(&polys);
        let broken = self.cycle_counts(&all) != (0, 0);
        // 閉じない輪郭（壊れた所）は使わない。輪郭ごとに（判定の形の点, 面積の2倍, 外接矩形）。外周か穴か・点の内外は、
        // 判定の形で整数で決める
        let cycles: Vec<Vec<(usize, bool)>> = all
            .into_iter()
            .filter(|c| c.iter().all(|h| h.0 != usize::MAX))
            .collect();
        let info: Vec<(Vec<IPt>, i128, GridRect)> = cycles
            .iter()
            .map(|c| {
                let pts = cycle_points(&polys, c);
                let (area2, rect) = (area2_of(&pts), GridRect::of(&pts));
                (pts, area2, rect)
            })
            .collect();
        let mut shapes: Vec<Option<GridPolygon>> = (0..cycles.len()).map(|_| None).collect();
        // 輪郭 `k` が点 `q`（2倍の座標）を中に含むか（輪郭の上なら `None`）
        let mut contains = |k: usize, q: IPt| -> Option<bool> {
            if !info[k].2.contains2(q) {
                return Some(false);
            }
            let poly = shapes[k].get_or_insert_with(|| GridPolygon::new(&info[k].0));
            poly.winding(q).map(|w| w != 0)
        };
        // 点を含む、いちばん小さい外周（面積が正）。点が輪郭の上にあれば、どの面か決まらないので何も変えない
        let g = gpt(p);
        let p2 = (2 * g.0, 2 * g.1);
        let mut ext: Option<usize> = None;
        for k in (0..cycles.len()).filter(|&k| info[k].1 > 0) {
            match contains(k, p2) {
                None => return Some(self.clone()),
                Some(true) if ext.is_none_or(|e| info[k].1 < info[e].1) => ext = Some(k),
                _ => {}
            }
        }
        let Some(ext) = ext else {
            return (!broken).then(|| self.clone());
        };
        if self.side_color(cycles[ext][0]) == Some(color) {
            return Some(self.clone());
        }
        // その外周の中にある穴（面積が負）のうち、それを含むいちばん小さい外周がこの外周のもの。調べる点は、穴の最初の
        // 区間の真ん中（正しい地図では、ほかの輪郭の上に無い）
        let mut face: Vec<usize> = vec![ext];
        for k in (0..cycles.len()).filter(|&k| info[k].1 < 0) {
            let probe = half_mid2(&polys, cycles[k][0]);
            if contains(ext, probe) != Some(true) {
                continue;
            }
            let smaller_owner = (0..cycles.len()).any(|j| {
                info[j].1 > 0 && info[j].1 < info[ext].1 && contains(j, probe) == Some(true)
            });
            if !smaller_owner {
                face.push(k);
            }
        }
        let mut m = self.clone();
        for &k in &face {
            for &(i, forward) in &cycles[k] {
                if forward {
                    m.edges[i].left = Some(color);
                    m.edges[i].left_src = None;
                } else {
                    m.edges[i].right = Some(color);
                    m.edges[i].right_src = None;
                }
            }
        }
        m.edges.retain(|e| e.left != e.right);
        // 2本だけがつながる頂点をまとめるのは、色を変えた輪郭の上（外周の外接矩形の中。頂点は判定の形の点から半目以内に
        // あるので、1目広げる）
        let r = info[ext].2;
        let g = |v: i64| v as f64 / OVERLAY_GRID;
        m.merge_degree2(KRect::new(
            g(r.x0 - 1),
            g(r.y0 - 1),
            g(r.x1 + 1),
            g(r.y1 + 1),
        ));
        m.compact_nodes();
        Some(m)
    }

    /// 形か地図 `input` を、`rule` の決め方で重ねた地図
    /// （設計：claude/曲線の当て直しを決まりで行う設計_7215b88.md、claude/格子の折れ線で判定する設計_段階2_d90cabe.md）。
    ///
    /// 1. i_overlay に入れる前からある辺を、交わりで決める（`fed_edges`）。重ねる線と一緒に細かい折れ線にして格子に置き、
    ///    i_overlay で交わる所で切る。前からある辺は Subject、重ねる線は Clip（偶奇の規則。重ねる形は重なりの無い形にしておくこと）
    /// 2. 入れなかった辺の判定の形が、切った区間に触れていたら、その辺も入れて切り直す（i_overlay は交点を格子の点に丸め、
    ///    区間の端に近い交点は端へ寄せ、寄せる距離をくり返しのたびに広げるので、区間は折れ線から半目より遠くへ動くことが
    ///    ある。入れる辺は増える一方なので、必ず終わる）
    /// 3. 切った区間ごとに、左右の前の色・重ねるものの中か（重ねる地図の色）を決め、`rule` で新しい色にする。左右が同じ区間は捨てる
    /// 4. 区間をすべて直線の辺にした地図から始め、同じ曲線から来た区間の鎖ごとに、決まりを満たすときだけ元の曲線の一部に
    ///    置き換える（`ChainRestorer`）
    ///
    /// 重ねるものが空のとき、格子に置けない座標（±65,536px の外）があるときは `None`
    pub(crate) fn overlay(&self, input: MapInput<'_>, rule: MapRule) -> Option<PlanarMap> {
        let mut new_quads: Vec<QuadBez> = Vec::new();
        let mut new_cols: Vec<(Option<Color32>, Option<Color32>)> = Vec::new();
        match input {
            MapInput::Shapes(shapes) => {
                for s in shapes {
                    for c in s.contours() {
                        new_quads.extend(c.segments.iter().copied());
                    }
                }
            }
            MapInput::Map(p) => {
                for e in 0..p.edges.len() {
                    new_quads.push(p.quad(e));
                    new_cols.push((p.edges[e].left, p.edges[e].right));
                }
            }
        }
        if new_quads.is_empty() {
            return None;
        }
        let mut fed = self.fed_edges(&new_quads, vec![false; self.edges.len()])?;
        loop {
            match self.overlay_with(input, rule, &new_quads, &new_cols, &mut fed) {
                Ok(m) => return Some(m),
                Err(OverlayStop::Unfed(more)) => {
                    let mut set = std::mem::take(&mut fed.fed);
                    for e in more {
                        set[e] = true;
                    }
                    fed = self.fed_edges(&new_quads, set)?;
                }
                Err(OverlayStop::Refused) => return None,
            }
        }
    }

    /// i_overlay に入れる前からある辺（設計 2-1）。`fed` の辺と重ねる線 `new_quads` の折れ線の近くを通る辺を足し、
    /// 足した辺の折れ線の近くを通る辺を足し…と、増えなくなるまで続ける。「近く」は格子の目の対角線の半分
    /// （`GRID_REACH`）以内：i_overlay は交点を格子の点に丸めるので、切った区間は折れ線からその距離まで動く（端へ寄せて
    /// それより遠くへ動いた所は、切ったあとに確かめて切り直す）。
    /// ただし、入れた辺と頂点を共有する辺は、その頂点でいつも触れているので、共有する頂点のほかで交わるかで見る。
    /// 入れた辺の端の頂点は格子に置き直すので、その頂点につながる入れない辺は、置き直した形で調べ直す
    /// （ほかの入れない辺と交わらないかも見る）。`None`：格子に置けない座標がある
    fn fed_edges(&self, new_quads: &[QuadBez], mut fed: Vec<bool>) -> Option<FedEdges> {
        let mut snapped = vec![false; self.nodes.len()];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        let mut edge_grid = BoxGrid::new(64.0);
        // 辺の外接矩形。端を格子に置き直すと曲線は最大で `GRID_REACH` 動き、判定の形の点は丸めでさらに `GRID_REACH`
        // 動くので、その分と、近くとみなす距離（`GRID_REACH`）の分を広げておく
        let boxes: Vec<KRect> = (0..self.edges.len())
            .map(|e| {
                let r = 3.0 * GRID_REACH;
                self.quad(e).bounding_box().inflate(r, r)
            })
            .collect();
        for (e, ed) in self.edges.iter().enumerate() {
            adj[ed.a].push(e);
            if ed.b != ed.a {
                adj[ed.b].push(e);
            }
            edge_grid.insert(e, boxes[e]);
        }
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
        let seg_rect =
            |a: IPt, b: IPt| KRect::from_points(from_grid(a.0, a.1), from_grid(b.0, b.1));
        let add_polyline = |q: &QuadBez,
                            ends: (Option<usize>, Option<usize>),
                            segs: &mut Vec<GridSeg>,
                            seg_grid: &mut BoxGrid,
                            lines: &mut Vec<GridLine>|
         -> Option<()> {
            // 格子に置けない座標は、ここで断る
            to_grid(q.p0)?;
            to_grid(q.p2)?;
            let line = grid_poly(q);
            let start = segs.len();
            let mut bbox: Option<KRect> = None;
            for k in 0..line.len().saturating_sub(1) {
                let (a, b) = (line[k], line[k + 1]);
                let r = seg_rect(a, b);
                bbox = Some(bbox.map_or(r, |x| x.union(r)));
                seg_grid.insert(segs.len(), r);
                let (ta, tb) = seg_tags(&line, k, usize::MAX, usize::MAX);
                segs.push((a, b, ta.and(ends.0), tb.and(ends.1)));
            }
            if let Some(r) = bbox {
                lines.push((r, start..segs.len()));
            }
            Some(())
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
        let mut seg_grid = BoxGrid::new(16.0);
        let mut lines: Vec<GridLine> = Vec::new();
        let mut edge_queue: Vec<usize> = Vec::new();
        for q in new_quads {
            add_polyline(q, (None, None), &mut segs, &mut seg_grid, &mut lines)?;
        }
        let mut to_feed: Vec<usize> = (0..self.edges.len()).filter(|&e| fed[e]).collect();
        for e in &to_feed {
            fed[*e] = false;
        }
        let mut found = Vec::new();
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
                        edge_queue.extend(adj[v].iter().copied().filter(|&f| !fed[f]));
                    }
                }
                add_polyline(
                    &self.quad(e),
                    (Some(ed.a), Some(ed.b)),
                    &mut segs,
                    &mut seg_grid,
                    &mut lines,
                )?;
            }
            if let Some((r, range)) = lines.pop() {
                // 折れ線の外接矩形にかかる辺だけを、折れ線の区間のうち辺の外接矩形にかかるものと比べる
                edge_grid.query(r, &mut found);
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
                let pe = poly_of(e, &snapped);
                let r = boxes[e];
                seg_grid.query(r, &mut found);
                let mut hit = found.iter().any(|&i| near_seg(e, &pe, &segs[i]));
                if !hit {
                    // 端を置き直した形が、ほかの入れない辺と触れないか（共有する頂点は除く）
                    edge_grid.query(r, &mut found);
                    let ed = self.edges[e];
                    hit = found.iter().any(|&f| {
                        if f == e || fed[f] {
                            return false;
                        }
                        let fd = self.edges[f];
                        let pf = poly_of(f, &snapped);
                        polys_clash(&pe, (ed.a, ed.b), &pf, (fd.a, fd.b))
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
                });
            }
        }
    }

    /// `overlay` の1回分（入れる辺 `fed` を決めたあと）。入れなかった辺が切った区間と交わったら `Unfed`（その辺）
    fn overlay_with(
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

    fn overlay_fed(
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
        // 重ねるものの外接矩形。これにかからない入れない辺は、重ねるものの外にある
        let area = new_quads
            .iter()
            .map(|q| q.bounding_box())
            .reduce(|a, b| a.union(b))
            .ok_or(OverlayStop::Refused)?;

        // 0. 出どころの曲線：入れる前からある辺と、重ねる線。入れない辺のうち、重ねるものの外接矩形にかかるものは「中の辺」
        let mut srcs: Vec<(QuadBez, OverlaySrc)> = Vec::new();
        let mut interior = vec![false; self.edges.len()];
        for e in 0..self.edges.len() {
            if fed[e] {
                srcs.push((self.quad(e), OverlaySrc::Old(e)));
            } else if rects_overlap(fed_set.boxes[e], area) {
                interior[e] = true;
            }
        }
        for (i, q) in new_quads.iter().enumerate() {
            srcs.push((*q, OverlaySrc::New(i)));
        }

        // 1. 折れ線にして格子に置き、i_overlay で切る。格子は原点から（何回重ねても、格子の上の点は動かない）
        let mut ov: EdgeOverlay<i32, ProvId> = EdgeOverlay::new(srcs.len() * 8);
        for (si, (q, kind)) in srcs.iter().enumerate() {
            let st = if matches!(kind, OverlaySrc::Old(_)) {
                ShapeType::Subject
            } else {
                ShapeType::Clip
            };
            let pts = flatten_with_t(q, FLAT_TOL);
            let mut prev = (to_grid(pts[0].0).ok_or(OverlayStop::Refused)?, pts[0].1);
            for &(p, t) in &pts[1..] {
                let ip = to_grid(p).ok_or(OverlayStop::Refused)?;
                if ip == prev.0 {
                    continue;
                }
                let id = ov.data_store_mut().push(vec![Prov {
                    src: si as u32,
                    t0: prev.1,
                    t1: t,
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

        // 2. 頂点：入れた辺の端の頂点（格子に丸めた所が同じ）は、その番号のまま格子の上に置く。新しい頂点は後ろに足す
        let mut nodes: Vec<Point> = self.nodes.clone();
        let mut node_of: IntMap<(i32, i32), usize> = IntMap::default();
        for (e, ed) in self.edges.iter().enumerate() {
            if fed[e] {
                for v in [ed.a, ed.b] {
                    let ip = to_grid(self.nodes[v]).ok_or(OverlayStop::Refused)?;
                    node_of.insert((ip.x, ip.y), v);
                    nodes[v] = from_grid(ip.x as i64, ip.y as i64);
                }
            }
        }
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
                let fwd = prov[k].t1 > prov[k].t0;
                let twin = (k + 1..prov.len())
                    .find(|&j| prov[j].src == prov[k].src && (prov[j].t1 > prov[j].t0) != fwd);
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
                fed_set.grid.query(r, &mut found);
                for &e in &found {
                    if fed[e] || flagged[e] || !(interior[e] || copy_untouched) {
                        continue;
                    }
                    if !rects_overlap(fed_set.boxes[e], r) {
                        continue;
                    }
                    let ed = self.edges[e];
                    let pe = grid_poly(&QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]));
                    if polys_clash(&[p.ia, p.ib], (p.a, p.b), &pe, (ed.a, ed.b)) {
                        flagged[e] = true;
                        unfed.push(e);
                    }
                }
            }
            if !unfed.is_empty() {
                return Err(OverlayStop::Unfed(unfed));
            }
        }
        // 3. 頂点ごとに、出ていく区間を角度の順に並べる：（区間, 区間の向きに出ていくか）
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
        for list in around.values_mut() {
            list.sort_by(|x, y| cmp_dir(wide(dir_of(x)), wide(dir_of(y))));
        }
        // 向きのある区間の左の面を、次にたどる区間（角度の順で、来た向きの逆のすぐ前）。番号は 2 × 区間 + 向き
        let mut next_of = vec![(0, false); 2 * pieces.len()];
        for list in around.values() {
            for (i, &(k, fwd)) in list.iter().enumerate() {
                // 頂点へ入ってくる向き（出ていく (k, fwd) の逆）の次は、角度の順で一つ前
                next_of[2 * k + (!fwd) as usize] = list[(i + list.len() - 1) % list.len()];
            }
        }
        // 前の色をたどるときは、入れなかった前からある辺のうち、区間の頂点につながるものも並べる（その辺で前の色が変わるので、
        // たどるのはそこで止める）。重ねる地図の色は、重ねる地図の辺がみな区間になっているので、区間だけでたどる
        // 入れなかった辺の向きは、その判定の形の、頂点から出る最初の区間の向き（区間と同じく整数）
        let mut stops: IntMap<usize, Vec<(Turn, IPt)>> = IntMap::default();
        for &v in around.keys() {
            // 区間の頂点のうち、前からある頂点（番号が前の地図の中）にだけ、前からある辺がつながる
            if v >= self.nodes.len() {
                continue;
            }
            for &e in &fed_set.adj[v] {
                if fed[e] {
                    continue;
                }
                let ed = self.edges[e];
                let pe = grid_poly(&QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]));
                for (end, at_start) in [(ed.a, true), (ed.b, false)] {
                    if end != v {
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
        let mut next_old_of = vec![Turn::Piece(0, false); 2 * pieces.len()];
        for (&v, list) in &around {
            let mut turns: Vec<(Turn, IPt)> = list
                .iter()
                .map(|&(k, fwd)| (Turn::Piece(k, fwd), dir_of(&(k, fwd))))
                .collect();
            if let Some(extra) = stops.get(&v) {
                turns.extend(extra.iter().copied());
            }
            turns.sort_by(|x, y| cmp_dir(wide(x.1), wide(y.1)));
            for (i, &(t, _)) in turns.iter().enumerate() {
                if let Turn::Piece(k, fwd) = t {
                    next_old_of[2 * k + (!fwd) as usize] =
                        turns[(i + turns.len() - 1) % turns.len()].0;
                }
            }
        }
        let next_new = |h: (usize, bool)| {
            Turn::Piece(
                next_of[2 * h.0 + h.1 as usize].0,
                next_of[2 * h.0 + h.1 as usize].1,
            )
        };
        let next_old = |h: (usize, bool)| next_old_of[2 * h.0 + h.1 as usize];
        // 4. 区間の左右の色の材料
        let side_of = |p: &Piece, want_old: bool| -> Option<(Option<Color32>, Option<Color32>)> {
            let (a, b) = (from_grid(p.ia.0, p.ia.1), from_grid(p.ib.0, p.ib.1));
            let entries = p
                .prov
                .iter()
                .filter_map(|pv| {
                    let lr = match srcs[pv.src as usize].1 {
                        OverlaySrc::Old(e) if want_old => (self.edges[e].left, self.edges[e].right),
                        OverlaySrc::New(i) if !want_old && !new_cols.is_empty() => new_cols[i],
                        _ => return None,
                    };
                    let lr = if pv.t1 > pv.t0 { lr } else { (lr.1, lr.0) };
                    Some((lr, srcs[pv.src as usize].0.eval((pv.t0 + pv.t1) * 0.5)))
                })
                .collect();
            combine_sides(entries, a.midpoint(b), b - a)
        };
        let old_of: Vec<_> = pieces.iter().map(|p| side_of(p, true)).collect();
        let new_of: Vec<_> = pieces.iter().map(|p| side_of(p, false)).collect();
        // 面ごとの色は、材料（出どころの辺の色）で決める（`face_colors`）。材料の無い面と食い違う面の色、中の辺の内外は、
        // 点のある所の印を整数で決める（`LabeledSegs`。設計：claude/重ね合わせの色と内外を整数で決める設計_08c15dd段階2_8702663.md）
        let dbl = |p: IPt| (2 * p.0, 2 * p.1);
        let piece_ends: Vec<(IPt, IPt)> = pieces.iter().map(|p| (p.ia, p.ib)).collect();
        // 奥の点：向きのある区間 `h` の真ん中から、面の側へ垂直に伸ばした半直線が、その地図の面を分ける最初の線（その地図の
        // 色を持つ区間 `material`、前の色なら入れなかった前からある辺の判定の形も）に当たるまでの半分の所。当たらなければ、
        // 面は外へ開いているので `None`。見る所を選ぶだけで、色は整数で決める（点は `FINE` 倍の座標。食い違う面は格子の目
        // より細いことがあるので、格子の点に丸めると面の外に出る）
        let deep_point =
            |h: usize, material: &dyn Fn(usize) -> bool, with_unfed: bool| -> Option<IPt> {
                let k = h / 2;
                let (a, b) = (nodes[pieces[k].a], nodes[pieces[k].b]);
                let (a, b) = if h % 2 == 1 { (a, b) } else { (b, a) };
                let d = (b - a) / (b - a).hypot();
                let (m, nrm) = (a.midpoint(b), KVec2::new(-d.y, d.x));
                let mut best = (0..pieces.len())
                    .filter(|&j| j != k && material(j))
                    .filter_map(|j| ray_seg_dist(m, nrm, nodes[pieces[j].a], nodes[pieces[j].b]))
                    .reduce(f64::min);
                if with_unfed {
                    for e in (0..self.edges.len()).filter(|&e| !fed[e]) {
                        let ed = self.edges[e];
                        let q = QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]);
                        if !ray_meets_rect(m, nrm, best, reach_of(&q)) {
                            continue;
                        }
                        for w in grid_poly(&q).windows(2) {
                            let (pa, pb) = (from_grid(w[0].0, w[0].1), from_grid(w[1].0, w[1].1));
                            if let Some(t) = ray_seg_dist(m, nrm, pa, pb) {
                                best = Some(best.map_or(t, |x| x.min(t)));
                            }
                        }
                    }
                }
                best.map(|s| {
                    let q = (m + nrm * (s * 0.5)).to_vec2() * (OVERLAY_GRID * FINE as f64);
                    (q.x.round() as i64, q.y.round() as i64)
                })
            };
        // 地図の元の形（判定の形）での、点（`FINE` 倍の座標）のある所の色。半直線の高さにかかる辺だけを見る
        let truth_at = |p: IPt, quads: &dyn Fn(usize) -> Option<(QuadBez, Sides)>, n: usize| {
            let fine = |g: IPt| (FINE * g.0, FINE * g.1);
            let mut w = Vec::new();
            for (q, (l, r)) in (0..n).filter_map(quads) {
                let rr = reach_of(&q);
                if FINE * rr.y0 <= p.1 && p.1 <= FINE * rr.y1 && FINE * rr.x1 >= p.0 {
                    for s in grid_poly(&q).windows(2) {
                        wind_into(fine(s[0]), fine(s[1]), l, r, p, &mut w);
                    }
                }
            }
            top_label(&w).flatten()
        };
        let old_quad = |e: usize| Some((self.quad(e), (self.edges[e].left, self.edges[e].right)));
        let new_quad = |i: usize| Some((new_quads[i], new_cols[i]));
        let mut old_faces = face_colors(
            &old_of,
            &next_old,
            &|e| {
                let ed = self.edges[e];
                (ed.left, ed.right)
            },
            &piece_ends,
        );
        for (r, h) in std::mem::take(&mut old_faces.mixed) {
            let c = deep_point(h, &|j| old_of[j].is_some(), true)
                .and_then(|p2| truth_at(p2, &old_quad, self.edges.len()));
            old_faces.color[r as usize] = Some(c);
        }
        let new_faces = match input {
            MapInput::Shapes(_) => None,
            MapInput::Map(_) => {
                let mut nf = face_colors(
                    &new_of,
                    &next_new,
                    &|_| unreachable!("重ねる地図の辺はみな区間になる"),
                    &piece_ends,
                );
                for (r, h) in std::mem::take(&mut nf.mixed) {
                    let c = deep_point(h, &|j| new_of[j].is_some(), false)
                        .and_then(|p2| truth_at(p2, &new_quad, new_quads.len()));
                    nf.color[r as usize] = Some(c);
                }
                Some(nf)
            }
        };
        // 面ごとに決めた色での、区間の左右の色
        let resolved = |faces: &FaceColors, k: usize| -> Sides {
            (
                faces.color[faces.root[2 * k + 1] as usize].flatten(),
                faces.color[faces.root[2 * k] as usize].flatten(),
            )
        };
        // その地図の色を持つ区間（面ごとに決めた色）。材料の無い面の色と、中の辺の重ねる地図の色を調べる相手
        let labeled = |faces: &FaceColors, of: &[Option<Sides>]| {
            LabeledSegs::new((0..pieces.len()).filter(|&k| of[k].is_some()).map(|k| {
                let (l, r) = resolved(faces, k);
                (pieces[k].ia, pieces[k].ib, l, r)
            }))
        };
        let old_pieces: std::cell::OnceCell<LabeledSegs<Option<Color32>>> =
            std::cell::OnceCell::new();
        let new_pieces = new_faces.as_ref().map(|nf| labeled(nf, &new_of));
        // 材料の無い面の前の色：前の色を持つ区間と、半直線の高さにかかる入れなかった前からある辺で
        let old_at = |p2: IPt| {
            let mut w = Vec::new();
            old_pieces
                .get_or_init(|| labeled(&old_faces, &old_of))
                .winding_into(p2, &mut w);
            for e in (0..self.edges.len()).filter(|&e| !fed[e]) {
                let ed = self.edges[e];
                let q = QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]);
                if ray_meets_row(reach_of(&q), p2) {
                    for s in grid_poly(&q).windows(2) {
                        wind_into(dbl(s[0]), dbl(s[1]), ed.left, ed.right, p2, &mut w);
                    }
                }
            }
            top_label(&w).flatten()
        };
        let new_at = |p2: IPt| new_pieces.as_ref().and_then(|ix| ix.at(p2)).flatten();
        // 材料の無い面の色：面の区間の頂点で調べる（面ごとに1回）。その頂点は、その地図の色を持つ区間の上に無い
        // （あれば、頂点のまわりでその区間が同じ面に入り、材料になる）。入れなかった辺は区間に触れない
        let mut old_memo: IntMap<u32, Option<Color32>> = IntMap::default();
        let mut new_memo: IntMap<u32, Option<Color32>> = IntMap::default();
        let color_of = |faces: &FaceColors,
                        memo: &mut IntMap<u32, Option<Color32>>,
                        h: (usize, bool),
                        at: &dyn Fn(IPt) -> Option<Color32>|
         -> Option<Color32> {
            let r = faces.root[2 * h.0 + h.1 as usize];
            if let Some(c) = faces.color[r as usize] {
                return c;
            }
            *memo.entry(r).or_insert_with(|| at(dbl(pieces[h.0].ia)))
        };
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
            MapInput::Map(_) => None,
        };
        // 5. 区間の新しい色。左右が違う区間だけ残す：（始点, 終点, (左の色, 右の色), 区間の番号）
        let mut kept: Vec<(usize, usize, Sides, usize)> = Vec::new();
        for (k, piece) in pieces.iter().enumerate() {
            let mut side = |fwd: bool| {
                let old = color_of(&old_faces, &mut old_memo, (k, fwd), &old_at);
                let (inside, p) = match input {
                    MapInput::Shapes(_) => {
                        let f = piece.fill;
                        (
                            if fwd {
                                f & CLIP_LEFT != 0
                            } else {
                                f & CLIP_RIGHT != 0
                            },
                            None,
                        )
                    }
                    MapInput::Map(_) => {
                        let nf = new_faces.as_ref().expect("重ねる地図の面の色");
                        let c = color_of(nf, &mut new_memo, (k, fwd), &new_at);
                        (c.is_some(), c)
                    }
                };
                rule.apply(inside, p, old)
            };
            let (left, right) = (side(true), side(false));
            if left != right {
                kept.push((piece.a, piece.b, (left, right), k));
            }
        }
        // 6. 区間をつないで鎖にする。頂点の次数が 2 で、出どころと左右の色が同じで、t が続いていれば、つなげる
        let mut at: IntMap<usize, Vec<usize>> = IntMap::default();
        for (ki, &(a, b, ..)) in kept.iter().enumerate() {
            at.entry(a).or_default().push(ki);
            at.entry(b).or_default().push(ki);
        }
        // 中の辺（重ねるものの範囲の中にあって、入れなかった辺）：重ねるものの中か（地図なら色）を、切った区間の印で整数で
        // 決めて、両側に `rule` を当てる。区間に触れないので、辺のどこでも同じ。左右が同じ色になれば消える
        let mut interior_kept: Vec<(usize, MapEdge)> = Vec::new();
        for e in (0..self.edges.len()).filter(|&e| interior[e]) {
            let ed = self.edges[e];
            // 調べる点は、判定の形の真ん中の区間の真ん中（2倍の座標で、区間の両端の和）。中の辺は両端の頂点のほかで区間に
            // 触れない（両端の頂点は区間と共有することがあるので使わない）
            let pe = grid_poly(&QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]));
            let k = pe.len().saturating_sub(2) / 2;
            let (a, b) = (pe[k], pe[(k + 1).min(pe.len() - 1)]);
            let p2 = (a.0 + b.0, a.1 + b.1);
            let (inside, p) = match input {
                MapInput::Shapes(_) => (
                    fill_at.as_ref().and_then(|f| f.at(p2)).unwrap_or(false),
                    None,
                ),
                MapInput::Map(_) => {
                    let c = new_at(p2);
                    (c.is_some(), c)
                }
            };
            let (l, r) = (
                rule.apply(inside, p, ed.left),
                rule.apply(inside, p, ed.right),
            );
            if l != r {
                interior_kept.push((
                    e,
                    MapEdge {
                        left: l,
                        right: r,
                        left_src: if l == ed.left { ed.left_src } else { None },
                        right_src: if r == ed.right { ed.right_src } else { None },
                        ..ed
                    },
                ));
            }
        }
        let mut untouched_deg: IntMap<usize, usize> = IntMap::default();
        for (e, ed) in self.edges.iter().enumerate() {
            if !fed[e] && !interior[e] && copy_untouched {
                *untouched_deg.entry(ed.a).or_insert(0) += 1;
                *untouched_deg.entry(ed.b).or_insert(0) += 1;
            }
        }
        for (_, ed) in &interior_kept {
            *untouched_deg.entry(ed.a).or_insert(0) += 1;
            *untouched_deg.entry(ed.b).or_insert(0) += 1;
        }
        // 区間の主な出どころ（前からある辺を先に）。区間の向き（a→b）での t
        let main_src = |ki: usize| -> Prov {
            let pv = &pieces[kept[ki].3].prov;
            *pv.iter()
                .find(|p| matches!(srcs[p.src as usize].1, OverlaySrc::Old(_)))
                .unwrap_or(&pv[0])
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
            let (_, _, (sl, sr), _) = kept[start];
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
                    let colors = if n_same { (nl, nr) } else { (nr, nl) };
                    let (_, _, nta, ntb) = piece_ends(&(nk, n_same));
                    let t_join = if forward { nta } else { ntb };
                    if colors != (sl, sr) || (t_join - t_end).abs() > 1e-6 {
                        break;
                    }
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
            // 前からある辺が丸ごと残ったときは、元の曲線と元の図形の印（色が変わらない側だけ）をそのまま使う
            let mut whole = None;
            if let OverlaySrc::Old(e) = srcs[s_src as usize].1 {
                let ed = self.edges[e];
                let full = t0.min(t1) < 1e-9 && t0.max(t1) > 1.0 - 1e-9;
                let same_dir = (n0, n1) == (ed.a, ed.b);
                if full && n0 != n1 && (same_dir || (n0, n1) == (ed.b, ed.a)) {
                    let (ol, or, osl, osr) = if same_dir {
                        (ed.left, ed.right, ed.left_src, ed.right_src)
                    } else {
                        (ed.right, ed.left, ed.right_src, ed.left_src)
                    };
                    whole = Some((
                        ed.ctrl,
                        if sl == ol { osl } else { None },
                        if sr == or { osr } else { None },
                    ));
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
        edges.extend(interior_kept.into_iter().map(|(_, ed)| ed));
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
                            left: c.left,
                            right: c.right,
                            left_src: None,
                            right_src: None,
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
        let map = PlanarMap {
            node_src: vec![None; nodes.len()],
            nodes,
            edges,
        };
        let mut restorer = ChainRestorer::new(map, active);
        for (c, ids) in chains.iter().zip(&chain_edges) {
            restorer.restore(c, ids);
        }
        let mut m = restorer.finish();
        m.compact_nodes();
        Ok(m)
    }
}

/// i_overlay に入れる前からある辺（`fed`）と、そのときに作った辺の外接矩形・格子・頂点ごとの辺
struct FedEdges {
    fed: Vec<bool>,
    /// 辺の外接矩形（格子に置き直す分と、近くとみなす距離の分を広げたもの）
    boxes: Vec<KRect>,
    grid: BoxGrid,
    adj: Vec<Vec<usize>>,
}

/// 区間の色をたどるときに、頂点で次に進む先
#[derive(Clone, Copy, Debug)]
enum Turn {
    /// 区間（番号, 区間の向きに出ていくか）
    Piece(usize, bool),
    /// 入れなかった前からある辺（番号, 頂点がその辺の始点か）。たどるのはここで止める
    Stop(usize, bool),
}

/// 重ね合わせで、片方の地図（前の地図か重ねる地図）の色を、切った区間の面ごとに1つに決めたもの
struct FaceColors {
    /// 向きのある区間（番号は 2 × 区間 + 向き）ごとの、左の面の代表
    root: Vec<u32>,
    /// 代表ごとの面の色。材料の無い面（と、食い違いを決める前の面）は `None`
    color: Vec<Option<Option<Color32>>>,
    /// 材料が食い違う面：（代表, 面のいちばん長い向きのある区間）。色は呼び出し側が奥の点で決める
    mixed: Vec<(u32, usize)>,
}

/// 片方の地図の色を、切った区間（端は `segs`）の面ごとに1つに決める（`FaceColors`）。
///
/// 同じ面とみなすのは、面の輪郭をたどってつながる向きのある区間（`next` が区間を返す）と、その地図の色を持たない区間の
/// 両側（その地図の面を分けない）。面の色の材料は、面の中の区間の出どころの辺の色（`of`）と、たどって止まった
/// 入れなかった前からある辺の色（`stop_sides`）。材料がみな同じなら、その色。
///
/// 材料が食い違うのは、その地図の2本の辺の判定の形が、格子の目ほどの所まで近づいている所だけ（i_overlay が交点を
/// 格子の点に丸めたり端へ寄せたりするときに、片方の区間がもう片方の向こう側へ移ると、それぞれの辺が自分の側だと
/// 思っている面が、切った区間の上では別の面になる）。その面は `mixed` に入れる
fn face_colors(
    of: &[Option<Sides>],
    next: &dyn Fn((usize, bool)) -> Turn,
    stop_sides: &dyn Fn(usize) -> Sides,
    segs: &[(IPt, IPt)],
) -> FaceColors {
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            let up = parent[parent[x as usize] as usize];
            parent[x as usize] = up;
            x = up;
        }
        x
    }
    let n = segs.len();
    let half = |h: usize| (h / 2, h % 2 == 1);
    let mut parent: Vec<u32> = (0..2 * n as u32).collect();
    let union = |parent: &mut Vec<u32>, x: usize, y: usize| {
        let (rx, ry) = (find(parent, x as u32), find(parent, y as u32));
        if rx != ry {
            parent[rx.max(ry) as usize] = rx.min(ry);
        }
    };
    for h in 0..2 * n {
        if let Turn::Piece(k, fwd) = next(half(h)) {
            union(&mut parent, h, 2 * k + fwd as usize);
        }
    }
    for (k, o) in of.iter().enumerate() {
        if o.is_none() {
            union(&mut parent, 2 * k, 2 * k + 1);
        }
    }
    let root: Vec<u32> = (0..2 * n as u32).map(|h| find(&mut parent, h)).collect();
    // 代表ごとの材料：まだ無い / 1つの色 / 食い違う
    #[derive(Clone, Copy, PartialEq)]
    enum Claim {
        Empty,
        One(Option<Color32>),
        Mixed,
    }
    let mut claim = vec![Claim::Empty; 2 * n];
    for (h, &r) in root.iter().enumerate() {
        let (k, fwd) = half(h);
        let r = r as usize;
        let mut add = |c: Option<Color32>| {
            claim[r] = match claim[r] {
                Claim::Empty => Claim::One(c),
                Claim::One(x) if x == c => Claim::One(x),
                _ => Claim::Mixed,
            };
        };
        if let Some((l, r)) = of[k] {
            add(if fwd { l } else { r });
        }
        if let Turn::Stop(e, at_start) = next((k, fwd)) {
            let (l, r) = stop_sides(e);
            add(if at_start { l } else { r });
        }
    }
    // 食い違う面：いちばん長い区間
    let mut longest: IntMap<u32, (i128, usize)> = IntMap::default();
    for (h, &r) in root.iter().enumerate() {
        if claim[r as usize] != Claim::Mixed {
            continue;
        }
        let (a, b) = segs[h / 2];
        let (dx, dy) = ((b.0 - a.0) as i128, (b.1 - a.1) as i128);
        let slot = longest.entry(r).or_insert((-1, h));
        if dx * dx + dy * dy > slot.0 {
            *slot = (dx * dx + dy * dy, h);
        }
    }
    let color = claim
        .iter()
        .map(|c| match c {
            Claim::One(c) => Some(*c),
            _ => None,
        })
        .collect();
    let mut mixed: Vec<(u32, usize)> = longest.into_iter().map(|(r, (_, h))| (r, h)).collect();
    mixed.sort_unstable();
    FaceColors { root, color, mixed }
}

/// 点 `m` から向き `dir`（長さ1）へ伸ばした半直線が、区間 `a`〜`b` に最初に当たるまでの距離（当たらなければ `None`）
fn ray_seg_dist(m: Point, dir: KVec2, a: Point, b: Point) -> Option<f64> {
    let e = b - a;
    let den = dir.cross(e);
    if den == 0.0 {
        return None;
    }
    let w = a - m;
    let (s, t) = (w.cross(e) / den, w.cross(dir) / den);
    (s > 0.0 && (0.0..=1.0).contains(&t)).then_some(s)
}

/// 点 `m` から向き `dir` へ伸ばした半直線（`reach` があれば、その長さまで）が、格子の点の矩形 `r` にかかるか
fn ray_meets_rect(m: Point, dir: KVec2, reach: Option<f64>, r: GridRect) -> bool {
    // 浮動小数で比べるので、矩形を1目広げてから比べる（かかる矩形を落とさない）
    let g = 1.0 / OVERLAY_GRID;
    let (lo, hi) = (
        Point::new((r.x0 - 1) as f64 * g, (r.y0 - 1) as f64 * g),
        Point::new((r.x1 + 1) as f64 * g, (r.y1 + 1) as f64 * g),
    );
    // 矩形の左右・上下の帯に入る半直線の長さの範囲（slab）を重ねる
    let (mut t0, mut t1) = (0.0f64, reach.unwrap_or(f64::INFINITY));
    for (o, d, a, b) in [(m.x, dir.x, lo.x, hi.x), (m.y, dir.y, lo.y, hi.y)] {
        if d == 0.0 {
            if o < a || o > b {
                return false;
            }
        } else {
            let (u, v) = ((a - o) / d, (b - o) / d);
            t0 = t0.max(u.min(v));
            t1 = t1.min(u.max(v));
        }
    }
    t0 <= t1
}

/// `overlay_with` が止まった理由
enum OverlayStop {
    /// 入れなかった辺（番号）が、切った区間と交わった。入れて切り直す
    Unfed(Vec<usize>),
    /// 格子に置けない座標がある
    Refused,
}

/// 同じ曲線から来た、左右の色が同じ区間の鎖
struct Chain {
    /// 鎖の向きでの区間：（始点, 終点, 出どころの曲線の t の始め, t の終わり）
    steps: Vec<(usize, usize, f64, f64)>,
    left: Option<Color32>,
    right: Option<Color32>,
    /// 出どころの曲線
    curve: QuadBez,
    /// 前からある辺が丸ごと残ったとき：（元の制御点, 左の図形の印, 右の図形の印）
    whole: Option<(Point, Option<ShapeId>, Option<ShapeId>)>,
}

/// 直線の区間の鎖を、元の曲線の一部へ置き換える。置き換えてよいのは、今の地図で、曲線 C の判定の形が次の3つを
/// 満たすときだけ（`PolyIndex::can_replace`。設計 段階2 の 2-4）：
/// 1. C が、自分とも、ほかの辺とも、共有する端のほかで触れない
/// 2. 区間の列 P と C で囲まれた所（帯。境目の上を含む）に、ほかの頂点が無い
/// 3. 帯の両端の頂点につながるほかの辺が、帯の中に無い（その辺は P とも C とも触れないので、最初の区間の真ん中の点1つで
///    決まる）
///
/// 満たさなければ鎖を真ん中で2つに分け、区間1つでも満たさなければ直線のまま残す
struct ChainRestorer {
    map: PlanarMap,
    /// 判定の形の索引（置き換える鎖が無ければ `None`）
    index: Option<PolyIndex>,
}

impl ChainRestorer {
    /// `active`：置き換える曲線と帯がすべて入る矩形（`None` なら鎖が無い）。調べる相手（索引に入れる区間・頂点）は、
    /// これにかかるものだけでよい
    fn new(map: PlanarMap, active: Option<KRect>) -> Self {
        let index = active.map(|area| PolyIndex::new(&map, Some(area)));
        Self { map, index }
    }

    /// 鎖 `c`（区間の辺の番号 `ids`）を、決まりを満たす所だけ曲線にする
    fn restore(&mut self, c: &Chain, ids: &[usize]) {
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
    fn try_replace(&mut self, c: &Chain, ids: &[usize], i: usize, j: usize) -> bool {
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
            Some((ctrl, ..)) => ctrl,
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
        let (left_src, right_src) = whole.map_or((None, None), |w| (w.1, w.2));
        self.map.edges.push(MapEdge {
            a: na,
            b: nb,
            ctrl,
            left: c.left,
            right: c.right,
            left_src,
            right_src,
        });
        index.add_edge(na, nb, pc);
        true
    }

    /// 置き換え終わった地図（使わなくなった辺を除く）
    fn finish(self) -> PlanarMap {
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

/// 外接矩形で引く格子（`query` は、矩形に重なるマスにある番号を、重なりなく返す）
struct BoxGrid {
    cell: f64,
    cells: IntMap<(i64, i64), Vec<usize>>,
    /// 番号ごとに、最後に返した `query` の回（同じ番号を2度返さないため）
    seen: Vec<u32>,
    stamp: u32,
}

impl BoxGrid {
    fn new(cell: f64) -> Self {
        Self {
            cell,
            cells: IntMap::default(),
            seen: Vec::new(),
            stamp: 0,
        }
    }

    fn range(&self, r: KRect) -> (i64, i64, i64, i64) {
        let c = self.cell;
        (
            (r.x0 / c).floor() as i64,
            (r.x1 / c).floor() as i64,
            (r.y0 / c).floor() as i64,
            (r.y1 / c).floor() as i64,
        )
    }

    fn insert(&mut self, id: usize, r: KRect) {
        let (x0, x1, y0, y1) = self.range(r);
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
        if self.seen.len() <= id {
            self.seen.resize(id + 1, 0);
        }
    }

    fn query(&mut self, r: KRect, out: &mut Vec<usize>) {
        out.clear();
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == 0 {
            self.seen.fill(0);
            self.stamp = 1;
        }
        let (x0, x1, y0, y1) = self.range(r);
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(list) = self.cells.get(&(x, y)) {
                    for &id in list {
                        if self.seen[id] != self.stamp {
                            self.seen[id] = self.stamp;
                            out.push(id);
                        }
                    }
                }
            }
        }
    }
}

// ---- 判定の形：格子の上の折れ線（設計 段階2） ----
//
// 地図のつながり（頂点のまわりの辺の順番）・外周か穴か・辺の交わりは、各辺の曲線を重ね合わせと同じ折れ線にし、
// 1/4096px の格子に丸めたもので、整数で決める。曲線は絵のためだけに使う。

/// 格子の点（1/4096px を 1 とする整数の座標）
type IPt = (i64, i64);

/// 点を格子の点に丸める（`to_grid` と同じ丸め。座標の範囲は見ない）
fn gpt(p: Point) -> IPt {
    (
        (p.x * OVERLAY_GRID).round() as i64,
        (p.y * OVERLAY_GRID).round() as i64,
    )
}

/// 曲線の判定の形：重ねるときと同じ数に等分した折れ線の点を、格子に丸めたもの（続けて同じ点は1つにする）
fn grid_poly(q: &QuadBez) -> Vec<IPt> {
    // 点は `flatten_with_t` と同じ（同じ t で同じ式）
    let n = flat_count(q, FLAT_TOL);
    let mut out: Vec<IPt> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let ip = gpt(q.eval(i as f64 / n as f64));
        if out.last() != Some(&ip) {
            out.push(ip);
        }
    }
    out
}

/// `c` が、`a` から `b` へ向かう線の左（正）・右（負）・上（0）のどちらにあるか（外積）
fn orient(a: IPt, b: IPt, c: IPt) -> i128 {
    (b.0 - a.0) as i128 * (c.1 - a.1) as i128 - (b.1 - a.1) as i128 * (c.0 - a.0) as i128
}

/// `p` が、区間 `a`–`b` の上（端を含む）にあるか
fn on_seg(a: IPt, b: IPt, p: IPt) -> bool {
    orient(a, b, p) == 0
        && p.0 >= a.0.min(b.0)
        && p.0 <= a.0.max(b.0)
        && p.1 >= a.1.min(b.1)
        && p.1 <= a.1.max(b.1)
}

/// 区間 `a`–`b` と `c`–`d` が、どこかで触れるか（端・重なりを含む）
fn segs_touch(a: IPt, b: IPt, c: IPt, d: IPt) -> bool {
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
fn fan_overlap(v: IPt, p: IPt, q: IPt) -> bool {
    orient(v, p, q) == 0
        && (p.0 - v.0) as i128 * (q.0 - v.0) as i128 + (p.1 - v.1) as i128 * (q.1 - v.1) as i128 > 0
}

/// 区間 `a`–`b` と `c`–`d` の距離が、格子の目の対角線の半分（`GRID_REACH`）以下か（2乗で 1/2 以下か）
fn segs_within_reach(a: IPt, b: IPt, c: IPt, d: IPt) -> bool {
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

/// 頂点で、`back`（入ってきた辺を引き返す向き）から時計回りに見たとき、`x` が `y` より先に来るか
/// （面の輪郭をたどるとき、次に選ぶのは時計回りに最初の辺。`back` と同じ向きはいちばん後）
fn cmp_cw_from(back: IPt, x: IPt, y: IPt) -> std::cmp::Ordering {
    // `back` を +x の向きに回した座標での、反時計回りの角度の大きい順
    let rot = |d: IPt| {
        let (bx, by, dx, dy) = (back.0 as i128, back.1 as i128, d.0 as i128, d.1 as i128);
        (bx * dx + by * dy, bx * dy - by * dx)
    };
    let (rx, ry) = (rot(x), rot(y));
    let zero = |r: (i128, i128)| r.1 == 0 && r.0 > 0;
    match (zero(rx), zero(ry)) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => cmp_dir(ry, rx),
    }
}

/// 区間 `a`–`b`（端の頂点の番号 `ta`・`tb`。折れ線の端で頂点にあたるときだけ `Some`）と、区間 `c`–`d`（`tc`・`td`）が、
/// 許されない所で触れるか。許されるのは、端を1つだけ共有し、そこから同じ向きに重ならず、その端が両方で同じ頂点か、
/// 1本の折れ線のとなり合う区間のつなぎ目（`joint`）のときだけ
fn seg_pair_clash(
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
fn polys_clash(pa: &[IPt], ea: (usize, usize), pb: &[IPt], eb: (usize, usize)) -> bool {
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

/// 折れ線 `poly` の区間 `k` の両端の頂点の番号（折れ線の端で、頂点 `na`・`nb` にあたるときだけ）
fn seg_tags(poly: &[IPt], k: usize, na: usize, nb: usize) -> (Option<usize>, Option<usize>) {
    ((k == 0).then_some(na), (k + 2 == poly.len()).then_some(nb))
}

/// 判定の形（格子の上の折れ線）の索引。地図の一部の辺を1本の曲線に置き換えてよいかを、整数で調べる
/// （設計 段階2 の 2-4。曲線への置き換え・2本の辺のまとめ・アンカーの追加と削除で使う）
struct PolyIndex {
    polys: Vec<Vec<IPt>>,
    ends: Vec<(usize, usize)>,
    alive: Vec<bool>,
    /// 区間の番号 → （辺, 区間の位置）
    segs: Vec<(u32, u32)>,
    grid: IntMap<(i64, i64), Vec<u32>>,
    node_pos: Vec<IPt>,
    node_grid: IntMap<(i64, i64), Vec<usize>>,
    adj: Vec<Vec<usize>>,
    deg: Vec<usize>,
    /// 索引に入れる範囲（格子の点。`None` なら全部）
    area: Option<GridRect>,
}

impl PolyIndex {
    /// 地図 `map` の辺の索引。`area`（px）に外接矩形がかかる区間と、中にある頂点だけを入れる（調べる相手がその中に
    /// 限られるとき。`None` なら全部）
    fn new(map: &PlanarMap, area: Option<KRect>) -> Self {
        Self::build(&map.nodes, &map.edges, |_| true, area)
    }

    /// 頂点 `nodes` と辺 `edges` の索引（番号は `edges` のまま。`alive` でない辺は消した印を付ける）。
    /// `area` に掛からない辺は、判定の形を作らない（区間が無い辺として持つ）
    fn build(
        nodes: &[Point],
        edges: &[MapEdge],
        alive: impl Fn(usize) -> bool,
        area: Option<KRect>,
    ) -> Self {
        // 格子の点に直すとき、外へ1目広げる（丸めで内側へ寄らないように）
        let area = area.map(|r| GridRect {
            x0: (r.x0 * OVERLAY_GRID).floor() as i64 - 1,
            y0: (r.y0 * OVERLAY_GRID).floor() as i64 - 1,
            x1: (r.x1 * OVERLAY_GRID).ceil() as i64 + 1,
            y1: (r.y1 * OVERLAY_GRID).ceil() as i64 + 1,
        });
        let mut ix = PolyIndex {
            polys: Vec::with_capacity(edges.len()),
            ends: Vec::with_capacity(edges.len()),
            alive: Vec::with_capacity(edges.len()),
            segs: Vec::new(),
            grid: IntMap::default(),
            node_pos: nodes.iter().map(|p| gpt(*p)).collect(),
            node_grid: IntMap::default(),
            adj: vec![Vec::new(); nodes.len()],
            deg: vec![0; nodes.len()],
            area,
        };
        for (e, ed) in edges.iter().enumerate() {
            let q = QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]);
            let poly = if area.is_none_or(|a| a.overlaps(&reach_of(&q))) {
                grid_poly(&q)
            } else {
                Vec::new()
            };
            ix.add_edge(ed.a, ed.b, poly);
            if !alive(e) {
                ix.kill(e);
            }
        }
        for v in 0..nodes.len() {
            let p = ix.node_pos[v];
            if ix.area.is_none_or(|r| r.contains2((2 * p.0, 2 * p.1))) {
                ix.node_grid.entry(node_cell(p)).or_default().push(v);
            }
        }
        ix
    }

    /// 辺を足す（番号を返す）
    fn add_edge(&mut self, a: usize, b: usize, poly: Vec<IPt>) -> usize {
        let e = self.polys.len();
        for k in 0..poly.len().saturating_sub(1) {
            let sg = (poly[k], poly[k + 1]);
            let r = GridRect::of(&[sg.0, sg.1]);
            if self.area.is_some_and(|x| !x.overlaps(&r)) {
                continue;
            }
            let id = self.segs.len() as u32;
            self.segs.push((e as u32, k as u32));
            for c in grid_cells(sg) {
                self.grid.entry(c).or_default().push(id);
            }
        }
        self.polys.push(poly);
        self.ends.push((a, b));
        self.alive.push(true);
        self.adj[a].push(e);
        self.adj[b].push(e);
        self.deg[a] += 1;
        self.deg[b] += 1;
        e
    }

    /// 矩形 `r`（格子の点）が、索引に入れた範囲に収まるか（その中の区間と頂点は、みな索引にある）
    fn covers(&self, r: &GridRect) -> bool {
        self.area
            .is_none_or(|a| a.x0 <= r.x0 && r.x1 <= a.x1 && a.y0 <= r.y0 && r.y1 <= a.y1)
    }

    /// 辺を消す
    fn kill(&mut self, e: usize) {
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
    fn can_replace(
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
        // 1. 自分と
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
        // 1. ほかの辺と
        for k in 0..=last {
            let sg = (pc[k], pc[k + 1]);
            let tags = seg_tags(pc, k, na, nb);
            for c in grid_cells(sg) {
                let Some(list) = self.grid.get(&c) else {
                    continue;
                };
                for &id in list {
                    let (f, m) = self.segs[id as usize];
                    let (f, m) = (f as usize, m as usize);
                    if !self.alive[f] || part.contains(&f) {
                        continue;
                    }
                    let poly = &self.polys[f];
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
        // 帯：path をたどり、pc を逆にたどって閉じる
        let mut band: Vec<IPt> = path.to_vec();
        band.extend(pc.iter().rev().skip(1).take(pc.len().saturating_sub(2)));
        let poly = GridPolygon::new(&band);
        let rect = GridRect::of(&band);
        debug_assert!(self.covers(&rect), "索引の範囲の外の帯");
        let inside = |p2: IPt| poly.winding(p2).is_none_or(|w| w != 0);
        // 2. 帯の中の頂点
        let (c0, c1) = (node_cell((rect.x0, rect.y0)), node_cell((rect.x1, rect.y1)));
        for cx in c0.0..=c1.0 {
            for cy in c0.1..=c1.1 {
                let Some(list) = self.node_grid.get(&(cx, cy)) else {
                    continue;
                };
                for &v in list {
                    if self.deg[v] == 0 || v == na || v == nb || inner.contains(&v) {
                        continue;
                    }
                    let p = self.node_pos[v];
                    if rect.contains2((2 * p.0, 2 * p.1)) && inside((2 * p.0, 2 * p.1)) {
                        return false;
                    }
                }
            }
        }
        // 3. 両端につながるほかの辺
        for v in [na, nb] {
            for &f in &self.adj[v] {
                if !self.alive[f] || part.contains(&f) {
                    continue;
                }
                let poly_f = &self.polys[f];
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

/// 頂点を入れる格子のマス（1マスは 16px）
fn node_cell(p: IPt) -> (i64, i64) {
    const CELL: i64 = 16 * OVERLAY_GRID as i64;
    (p.0.div_euclid(CELL), p.1.div_euclid(CELL))
}

/// 区間（2倍の座標の y の範囲 `spans`）を、横の帯に分けた索引：（いちばん下の y, 帯の高さ, 帯ごとの区間の番号）。
/// 帯の数は区間の数の平方根（上限 256。速さだけに関わる）
fn y_bands(spans: impl ExactSizeIterator<Item = (i64, i64)> + Clone) -> (i64, i64, Vec<Vec<u32>>) {
    if spans.len() == 0 {
        return (0, 1, vec![Vec::new()]);
    }
    let (y0, y1) = spans
        .clone()
        .fold((i64::MAX, i64::MIN), |(lo, hi), (a, b)| {
            (lo.min(a), hi.max(b))
        });
    let n = ((spans.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
    let band_h = ((y1 - y0) / n as i64 + 1).max(1);
    let mut bands = vec![Vec::new(); n];
    for (i, (lo, hi)) in spans.enumerate() {
        let (k0, k1) = (((lo - y0) / band_h) as usize, ((hi - y0) / band_h) as usize);
        for band in bands.iter_mut().take(k1.min(n - 1) + 1).skip(k0.min(n - 1)) {
            band.push(i as u32);
        }
    }
    (y0, band_h, bands)
}

/// 帯の索引で、点の y（2倍の座標）の帯にかかる区間の番号
fn band_at(y0: i64, band_h: i64, bands: &[Vec<u32>], y: i64) -> &[u32] {
    let k = (y - y0).div_euclid(band_h);
    if k < 0 || k as usize >= bands.len() {
        &[]
    } else {
        &bands[k as usize]
    }
}

/// 右へ伸ばした半直線と区間 `a`〜`b`（2倍の座標）の交わりを、印ごとの巻き数 `w` に足す。上へ横切れば左の印 `l` に +1・
/// 右の印 `r` に −1、下へ横切れば逆。下の端は含め、上の端は含めない（半開区間）。点が区間の上にあれば数えない（点を
/// 右上へごくわずかずらした所として数えるのと同じ）
fn wind_into<L: Copy + PartialEq>(a: IPt, b: IPt, l: L, r: L, p: IPt, w: &mut Vec<(L, i32)>) {
    let d = if a.1 <= p.1 && b.1 > p.1 && orient(a, b, p) > 0 {
        1
    } else if b.1 <= p.1 && a.1 > p.1 && orient(a, b, p) < 0 {
        -1
    } else {
        return;
    };
    for (lab, v) in [(l, d), (r, -d)] {
        match w.iter_mut().find(|x| x.0 == lab) {
            Some(x) => x.1 += v,
            None => w.push((lab, v)),
        }
    }
}

/// 巻き数が正の印（印が閉じた境目を作っていれば、点のある所の印。無ければ `None`＝いちばん外）
fn top_label<L: Copy>(w: &[(L, i32)]) -> Option<L> {
    w.iter().find(|x| x.1 > 0).map(|x| x.0)
}

/// 曲線の判定の形が入る、格子の点の矩形（制御点の外接矩形を、切り下げ〜切り上げ）
fn reach_of(q: &QuadBez) -> GridRect {
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
fn ray_meets_row(r: GridRect, p: IPt) -> bool {
    2 * r.y0 <= p.1 && p.1 <= 2 * r.y1 && 2 * r.x1 >= p.0
}

/// 左右に印（色・重ねる形の中か）のある格子の区間の集まり。点のある所の印を、整数で決める（`at`）
struct LabeledSegs<L> {
    /// （始点, 終点, 左の印, 右の印）。座標は格子の点の2倍
    segs: Vec<(IPt, IPt, L, L)>,
    y0: i64,
    band_h: i64,
    bands: Vec<Vec<u32>>,
}

impl<L: Copy + PartialEq> LabeledSegs<L> {
    /// 区間（格子の点）と左右の印
    fn new(items: impl Iterator<Item = (IPt, IPt, L, L)>) -> Self {
        let segs: Vec<(IPt, IPt, L, L)> = items
            .filter(|(a, b, ..)| a != b)
            .map(|(a, b, l, r)| ((2 * a.0, 2 * a.1), (2 * b.0, 2 * b.1), l, r))
            .collect();
        let (y0, band_h, bands) =
            y_bands(segs.iter().map(|(a, b, ..)| (a.1.min(b.1), a.1.max(b.1))));
        Self {
            segs,
            y0,
            band_h,
            bands,
        }
    }

    /// 点 `p`（2倍の座標）での巻き数を `w` に足す（`wind_into`）
    fn winding_into(&self, p: IPt, w: &mut Vec<(L, i32)>) {
        for &i in band_at(self.y0, self.band_h, &self.bands, p.1) {
            let (a, b, l, r) = self.segs[i as usize];
            wind_into(a, b, l, r, p, w);
        }
    }

    /// 点 `p`（2倍の座標）のある所の印。区間の印が閉じた境目を作っていれば（正しい地図の色、i_overlay が数えた塗り）、
    /// どの点でも1つに決まる。点が区間の上にあれば、点を右上へごくわずかずらした所の印になる
    fn at(&self, p: IPt) -> Option<L> {
        let mut w = Vec::new();
        self.winding_into(p, &mut w);
        top_label(&w)
    }
}

/// 格子の多角形（点は2倍の座標）。帯に分けて、点の巻き数を整数で数える
struct GridPolygon {
    /// （区間の始点, 終点）。座標は格子の点の2倍
    segs: Vec<(IPt, IPt)>,
    y0: i64,
    band_h: i64,
    bands: Vec<Vec<u32>>,
}

impl GridPolygon {
    /// 閉じた折れ線 `pts`（格子の点。最後の点から最初の点へ閉じる）
    fn new(pts: &[IPt]) -> Self {
        let dbl = |p: IPt| (2 * p.0, 2 * p.1);
        let segs: Vec<(IPt, IPt)> = (0..pts.len())
            .map(|i| (dbl(pts[i]), dbl(pts[(i + 1) % pts.len()])))
            .filter(|(a, b)| a != b)
            .collect();
        let (y0, band_h, bands) = y_bands(segs.iter().map(|(a, b)| (a.1.min(b.1), a.1.max(b.1))));
        Self {
            segs,
            y0,
            band_h,
            bands,
        }
    }

    /// 点 `p`（2倍の座標）の巻き数。`p` が輪郭の上にあれば `None`
    fn winding(&self, p: IPt) -> Option<i32> {
        let mut w = 0;
        for &i in band_at(self.y0, self.band_h, &self.bands, p.1) {
            let (a, b) = self.segs[i as usize];
            if on_seg(a, b, p) {
                return None;
            }
            // 下の端は含め、上の端は含めない（半開区間）
            if a.1 <= p.1 {
                if b.1 > p.1 && orient(a, b, p) > 0 {
                    w += 1;
                }
            } else if b.1 <= p.1 && orient(a, b, p) < 0 {
                w -= 1;
            }
        }
        Some(w)
    }
}

/// 格子の点の外接矩形
#[derive(Clone, Copy)]
struct GridRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl GridRect {
    fn of(pts: &[IPt]) -> Self {
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

    fn union(&self, o: &GridRect) -> GridRect {
        GridRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    fn overlaps(&self, o: &GridRect) -> bool {
        self.x0 <= o.x1 && o.x0 <= self.x1 && self.y0 <= o.y1 && o.y0 <= self.y1
    }

    /// 2倍の座標の点 `p` が、矩形（端を含む）に入るか
    fn contains2(&self, p: IPt) -> bool {
        p.0 >= 2 * self.x0 && p.0 <= 2 * self.x1 && p.1 >= 2 * self.y0 && p.1 <= 2 * self.y1
    }
}

/// 奥の点を調べるときの座標の細かさ（格子の目の 1/2^24）。座標の上限（±2^28 目）の f64 は、この細かさまでしか持たないので、
/// 浮動小数で選んだ点をそのまま整数で調べられる。区間の端の座標は ±2^52 になり、外積は i128 に収まる
const FINE: i64 = 1 << 24;

/// 近さを調べる格子の1マス（8px）
const GRID_CELL: i64 = 8 * OVERLAY_GRID as i64;

/// 区間がかかる、1マス `cell` の格子のマス
fn cells_of((a, b): (IPt, IPt), cell: i64) -> impl Iterator<Item = (i64, i64)> {
    let (x0, x1) = (a.0.min(b.0).div_euclid(cell), a.0.max(b.0).div_euclid(cell));
    let (y0, y1) = (a.1.min(b.1).div_euclid(cell), a.1.max(b.1).div_euclid(cell));
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

/// 区間がかかる、近さを調べる格子のマス
fn grid_cells(sg: (IPt, IPt)) -> impl Iterator<Item = (i64, i64)> {
    cells_of(sg, GRID_CELL)
}

/// 向きのある辺 `h` の判定の形の点（向きどおりの順）
fn half_points(polys: &[Vec<IPt>], (i, forward): (usize, bool)) -> impl Iterator<Item = IPt> + '_ {
    let p = &polys[i];
    let n = p.len();
    (0..n).map(move |k| if forward { p[k] } else { p[n - 1 - k] })
}

/// 面の輪郭 `cycle` の判定の形の点（閉じた折れ線。最後の点から最初の点へ閉じる）
fn cycle_points(polys: &[Vec<IPt>], cycle: &[(usize, bool)]) -> Vec<IPt> {
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
fn area2_of(pts: &[IPt]) -> i128 {
    (0..pts.len())
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
            p.0 as i128 * q.1 as i128 - q.0 as i128 * p.1 as i128
        })
        .sum()
}

/// 向きのある辺 `h` の判定の形の最初の区間の真ん中の点（2倍の座標）
fn half_mid2(polys: &[Vec<IPt>], h: (usize, bool)) -> IPt {
    let mut it = half_points(polys, h);
    let a = it.next().unwrap_or((0, 0));
    let b = it.next().unwrap_or(a);
    (a.0 + b.0, a.1 + b.1)
}

/// 点を、重ねるときの格子の点にする。格子に置けない（±65,536px の外）ときは `None`
fn to_grid(p: Point) -> Option<i_overlay9::i_float::int::point::IntPoint<i32>> {
    let limit = (1i64 << 28) as f64;
    let x = (p.x * OVERLAY_GRID).round();
    let y = (p.y * OVERLAY_GRID).round();
    (x.abs() < limit && y.abs() < limit)
        .then(|| i_overlay9::i_float::int::point::IntPoint::new(x as i32, y as i32))
}

/// 格子の点の座標
fn from_grid(x: i64, y: i64) -> Point {
    Point::new(x as f64 / OVERLAY_GRID, y as f64 / OVERLAY_GRID)
}

/// 点を格子の点に丸めた座標（格子に置けない点はそのまま。その点を使う重ね合わせは `to_grid` で断る）
fn grid_round(p: Point) -> Point {
    to_grid(p).map_or(p, |ip| from_grid(ip.x as i64, ip.y as i64))
}
