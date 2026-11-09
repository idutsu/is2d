//! 同じレイヤー・同じ階層の生の図形を、境目の線（辺）で持つ地図。選択ツールの輪郭の変形とカーソル合わせで使う。
//! Adobe Animate のマージ描画と同じく、辺が「曲線・左の色・右の色」を持つ。境目は1本で、重ね順は無い。
//! 塗りの形（`FillShape`）は、辺から作って描画・当たり判定に使う。

use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;
use std::sync::Arc;

use eframe::egui::Color32;
use kurbo::{
    BezPath, ParamCurve, Point, QuadBez, Rect as KRect, Shape as KurboShape, Vec2 as KVec2,
};

use crate::canvas_tool::{HoverTarget, LayerId, Scene, Session, ShapeId};
use crate::vector_engine::{rects_overlap, Contour, FillShape};

/// 2本の線（2つの点）を同じとみなす距離。今の図形の境目のずれは、測って最大 0.024
pub(crate) const SNAP_TOL: f64 = 0.05;

/// 図形から地図を作るとき（`from_faces`）に、同じ点・同じ線とみなす距離。生の図形はどれも地図から書き出したもので、
/// 境目は両側の図形でビットまで同じ曲線なので、許容はほぼ 0 でよい（0.05 にすると、地図の上で 0.05 より近くに
/// できた2つの頂点がまとめられて、地図が壊れることがある）
pub(crate) const FACE_TOL: f64 = 1e-6;
/// 図形から地図を作るとき、区間の途中にある頂点で区間を切る距離。地図から書き出した図形では、頂点はいつも区間の端にあるので
/// ほぼ 0 でよい（重ねた結果の曲線は、ほかの頂点の 1e-6 ほどの所を通ることがあり、`FACE_TOL` で切ると形が変わる）
const SPLIT_TOL: f64 = 1e-9;

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
    fn in_rect(&self, r: KRect) -> impl Iterator<Item = usize> + '_ {
        let (x0, y0) = self.key(Point::new(r.x0, r.y0));
        let (x1, y1) = self.key(Point::new(r.x1, r.y1));
        (x0..=x1).flat_map(move |x| {
            (y0..=y1).flat_map(move |y| self.cells.get(&(x, y)).into_iter().flatten().copied())
        })
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

/// 始点から出ていく向き。始点から `SNAP_TOL`（短い辺は長さの半分）ほど進んだ所への向きで見る
/// （制御点が始点のごく近く、わずかに後ろにあると、接線は逆を向くが、曲線はすぐ反対の端の方へ向かうため）
fn tangent_start(q: &QuadBez) -> KVec2 {
    let a = q.p1 - q.p0;
    let b = q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2();
    let r = SNAP_TOL.min(0.5 * (q.p2 - q.p0).hypot());
    // 始点からの距離 |2ta + t²b| が r になる t の見積もり（1次の項と2次の項のうち、先に r に届く方）
    let t = (r / (2.0 * a.hypot() + 1e-300))
        .min((r / (b.hypot() + 1e-300)).sqrt())
        .min(1.0);
    let d = a * (2.0 * t) + b * (t * t);
    if d.hypot2() > 1e-24 {
        d
    } else {
        q.p2 - q.p0
    }
}

/// 終点へ入ってくる向き（`tangent_start` と同じ見方を、終点の側から）
fn tangent_end(q: &QuadBez) -> KVec2 {
    -tangent_start(&QuadBez::new(q.p2, q.p1, q.p0))
}

/// 始点での曲がり具合（左へ曲がるほど大きい）
fn curvature_start(q: &QuadBez) -> f64 {
    let d1 = (q.p1 - q.p0) * 2.0;
    let d2 = (q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2()) * 2.0;
    let len = d1.hypot();
    if len < 1e-12 {
        0.0
    } else {
        d1.cross(d2) / (len * len * len)
    }
}

/// 頂点で、`back`（入ってきた辺を引き返す向き）から時計回りに見たとき、出ていく曲線 `q` が先に来るほど小さい値。
/// 接線が同じときは、左へ曲がる曲線を先にする
fn turn_key(back: KVec2, q: &QuadBez) -> (f64, f64) {
    let a = cw_angle(back, tangent_start(q));
    ((a * 1e9).round() / 1e9, -curvature_start(q))
}

fn cmp_turn(back: KVec2, x: &QuadBez, y: &QuadBez) -> std::cmp::Ordering {
    let (kx, ky) = (turn_key(back, x), turn_key(back, y));
    kx.0.total_cmp(&ky.0).then(kx.1.total_cmp(&ky.1))
}

/// `u` から時計回り（面積の式の向きで）に `w` まで回る角度（0 は 2π とみなす）
fn cw_angle(u: KVec2, w: KVec2) -> f64 {
    cw_angle_of(u.y.atan2(u.x), w.y.atan2(w.x))
}

/// `cw_angle` の、向きの角度（atan2）を先に求めてあるとき
fn cw_angle_of(u: f64, w: f64) -> f64 {
    let a = (u - w).rem_euclid(TAU);
    if a < 1e-12 {
        TAU
    } else {
        a
    }
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

    /// 生の図形の並びから作る。境目の2本の線をそろえて1本の辺にし、区間の途中にある頂点（分かれ目）で区切る。
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
        // 2. 区間の途中にある頂点で切り分ける
        let mut split: Vec<Seg> = Vec::with_capacity(segs.len());
        for s in segs {
            let bbox = s.q.bounding_box().inflate(SPLIT_TOL, SPLIT_TOL);
            // 曲線は、両端を結ぶ線と、そこから制御点までの距離の半分の線の間の帯に入る。帯から離れた頂点は調べない
            let chord = s.q.p2 - s.q.p0;
            let len = chord.hypot();
            let band = (len > 1e-12).then(|| {
                let n = KVec2::new(-chord.y, chord.x) / len;
                let h = n.dot(s.q.p1 - s.q.p0) * 0.5;
                (n, h.min(0.0) - SPLIT_TOL, h.max(0.0) + SPLIT_TOL)
            });
            let mut cuts: Vec<(f64, usize)> = grid
                .in_rect(bbox)
                .filter(|&v| v != s.a && v != s.b)
                .filter_map(|v| {
                    let p = nodes[v];
                    if (p - s.q.p0).hypot() <= FACE_TOL || (p - s.q.p2).hypot() <= FACE_TOL {
                        return None;
                    }
                    if let Some((n, lo, hi)) = band {
                        let o = n.dot(p - s.q.p0);
                        if o < lo || o > hi {
                            return None;
                        }
                    }
                    let (t, d) = nearest_on_quad(&s.q, p);
                    (d <= SPLIT_TOL && t > 1e-6 && t < 1.0 - 1e-6).then_some((t, v))
                })
                .collect();
            if cuts.is_empty() {
                split.push(s);
                continue;
            }
            cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
            let (mut t0, mut n0) = (0.0, s.a);
            for (t, v) in cuts.into_iter().chain(std::iter::once((1.0, s.b))) {
                if v == n0 || t - t0 < 1e-9 {
                    continue;
                }
                let mut q = s.q.subsegment(t0..t);
                q.p0 = nodes[n0];
                q.p2 = nodes[v];
                split.push(Seg {
                    a: n0,
                    b: v,
                    q,
                    color: s.color,
                    order: s.order,
                    src: s.src,
                });
                t0 = t;
                n0 = v;
            }
        }
        // 3. 同じ2頂点を結び、向きが逆で形の近い区間を、1本の辺にまとめる
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

    /// 正しい地図か：辺どうしが端のほかで交わらず、面の輪郭が閉じて色がそろい（`faces` の壊れた数が 0）、何も無い側の輪郭が
    /// 色のある面の中に入り込んでいない（重なった図形をそのまま地図にしたものでない）。何も無い側の輪郭は、辺の真ん中の点が、
    /// その辺を輪郭に持たない色のある面の中（外周の中で、どの穴の外）にあれば、入り込んでいる
    pub(crate) fn is_valid(&self) -> bool {
        let cycles = self.face_cycles();
        let (faces, broken) = self.faces_of(&cycles);
        if broken > 0 {
            return false;
        }
        let all: Vec<usize> = (0..self.edges.len()).collect();
        if !self
            .crossings_grid(&all, &vec![false; self.edges.len()])
            .is_empty()
        {
            return false;
        }
        let mut edge_faces: Vec<[usize; 2]> = vec![[usize::MAX; 2]; self.edges.len()];
        let mut regions: Vec<Vec<(KRect, Option<BandedCycle>)>> = Vec::with_capacity(faces.len());
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
                    .map(|c| {
                        let bbox = c
                            .iter()
                            .map(|h| self.half_quad(*h).bounding_box())
                            .reduce(|a, b| a.union(b))
                            .unwrap_or(KRect::ZERO);
                        (bbox, None)
                    })
                    .collect(),
            );
        }
        for c in &cycles {
            if c.is_empty() || self.side_color(c[0]).is_some() {
                continue;
            }
            let h = c[0];
            let probe = self.half_quad(h).eval(0.5);
            for (fi, f) in faces.iter().enumerate() {
                if edge_faces[h.0].contains(&fi) || !regions[fi][0].0.contains(probe) {
                    continue;
                }
                let mut inside = |k: usize| {
                    let (bbox, banded) = &mut regions[fi][k];
                    bbox.contains(probe)
                        && banded
                            .get_or_insert_with(|| BandedCycle::new(self, &f.cycles[k], *bbox))
                            .winding(probe)
                            != 0
                };
                if inside(0) && !(1..f.cycles.len()).any(&mut inside) {
                    return false;
                }
            }
        }
        true
    }

    /// 面を取り出す。外周（面積が正）に、それを含むいちばん小さい同じ色の外周の穴（面積が負）を入れる。
    /// 戻り値の2つ目は、閉じなかった輪郭・色がまざった輪郭・入れ先の無い穴の数（壊れていれば 0 より大きい）
    pub(crate) fn faces(&self) -> (Vec<MapFace>, usize) {
        self.faces_of(&self.face_cycles())
    }

    /// `faces` と同じ。面の輪郭 `cycles`（`face_cycles` の結果）を先に求めてあるときに使う
    fn faces_of(&self, cycles: &[Vec<(usize, bool)>]) -> (Vec<MapFace>, usize) {
        let mut broken = 0;
        // （面, 面積, 巻き数を調べる帯（要るときに作る）, 外接矩形）
        let mut exteriors: Vec<(MapFace, f64, Option<BandedCycle>, KRect)> = Vec::new();
        let mut holes: Vec<(Color32, Vec<(usize, bool)>)> = Vec::new();
        for cycle in cycles {
            if self.is_broken_cycle(cycle) {
                broken += 1;
                continue;
            }
            let Some(color) = self.side_color(cycle[0]) else {
                continue;
            };
            let area = self.cycle_area(cycle);
            if area > 0.0 {
                let bbox = cycle
                    .iter()
                    .map(|h| self.half_quad(*h).bounding_box())
                    .reduce(|a, b| a.union(b))
                    .unwrap_or(KRect::ZERO);
                exteriors.push((
                    MapFace {
                        color,
                        cycles: vec![cycle.clone()],
                        srcs: Vec::new(),
                    },
                    area,
                    None,
                    bbox,
                ));
            } else {
                holes.push((color, cycle.clone()));
            }
        }
        // 穴の入れ先は、穴を含む同じ色の外周のうち、いちばん小さいもの。小さい外周から順に調べ、最初に含むものを選ぶ
        let mut by_area: Vec<usize> = (0..exteriors.len()).collect();
        by_area.sort_by(|&a, &b| exteriors[a].1.total_cmp(&exteriors[b].1));
        for (color, cycle) in holes {
            let probe = self.half_quad(cycle[0]).eval(0.5);
            let owner = by_area.iter().copied().find(|&i| {
                let e = &mut exteriors[i];
                if e.0.color != color || !e.3.contains(probe) {
                    return false;
                }
                let bbox = e.3;
                let banded =
                    e.2.get_or_insert_with(|| BandedCycle::new(self, &e.0.cycles[0], bbox));
                banded.winding(probe) != 0
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

    /// 面の輪郭の符号付き面積（`Contour::signed_area` と同じ計算を、輪郭を作らずに行う）
    fn cycle_area(&self, cycle: &[(usize, bool)]) -> f64 {
        const SUBDIVS: usize = 8;
        let first_pt = self.half_quad(cycle[0]).eval(0.0);
        let mut prev = first_pt;
        let mut sum = 0.0;
        for h in cycle {
            let seg = self.half_quad(*h);
            for k in 0..SUBDIVS {
                let curr = seg.eval(k as f64 / SUBDIVS as f64);
                sum += prev.x * curr.y - curr.x * prev.y;
                prev = curr;
            }
        }
        sum += prev.x * first_pt.y - first_pt.x * prev.y;
        0.5 * sum
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
        let mut out_of: Vec<Vec<(usize, bool)>> = vec![Vec::new(); self.nodes.len()];
        for (i, e) in self.edges.iter().enumerate() {
            out_of[e.a].push((i, true));
            out_of[e.b].push((i, false));
        }
        let head = |h: (usize, bool)| {
            if h.1 {
                self.edges[h.0].b
            } else {
                self.edges[h.0].a
            }
        };
        let idx = |h: (usize, bool)| 2 * h.0 + h.1 as usize;
        // 向きのある辺ごとの（出ていく向きの角度, 曲がり具合）。`cmp_turn` と同じ材料を1回だけ求める
        // （入ってきた辺を引き返す向きは、逆向きの辺の出ていく向きそのもの）
        let turn: Vec<(f64, f64)> = (0..2 * self.edges.len())
            .map(|k| {
                let q = self.half_quad((k / 2, k % 2 == 1));
                let d = tangent_start(&q);
                (d.y.atan2(d.x), -curvature_start(&q))
            })
            .collect();
        let key = |back: f64, h: (usize, bool)| {
            let (ang, curv) = turn[idx(h)];
            ((cw_angle_of(back, ang) * 1e9).round() / 1e9, curv)
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
                    let twin = (cur.0, !cur.1);
                    let back = turn[idx(twin)].0;
                    // 引き返す向き（同じ辺の逆向き）は、ほかに辺が無いときだけ選ぶ
                    let next = out_of[head(cur)]
                        .iter()
                        .copied()
                        .filter(|&h| h != twin)
                        .min_by(|&x, &y| {
                            let (kx, ky) = (key(back, x), key(back, y));
                            kx.0.total_cmp(&ky.0).then(kx.1.total_cmp(&ky.1))
                        })
                        .unwrap_or(twin);
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
        const CELL: f64 = 8.0;
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
        // 交わりを調べるための、辺の外接矩形と格子（初めて要るときに作る。古い所に残った番号は、今の外接矩形で確かめて除く）
        let quad_of = |edges: &[MapEdge], i: usize| {
            QuadBez::new(
                self.nodes[edges[i].a],
                edges[i].ctrl,
                self.nodes[edges[i].b],
            )
        };
        let cells = |b: &KRect| {
            let (x0, y0) = ((b.x0 / CELL).floor() as i64, (b.y0 / CELL).floor() as i64);
            let (x1, y1) = ((b.x1 / CELL).floor() as i64, (b.y1 / CELL).floor() as i64);
            (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
        };
        type EdgeIndex = (Vec<KRect>, IntMap<(i64, i64), Vec<usize>>);
        let mut index: Option<EdgeIndex> = None;
        let candidate =
            |v: usize, at: &[Vec<usize>]| at[v].len() == 2 && area.contains(self.nodes[v]);
        let mut pending: std::collections::BTreeSet<usize> = (0..self.nodes.len())
            .filter(|&v| candidate(v, &at))
            .collect();
        // まとめると、ほかの辺と交わってしまう頂点（まとめない）
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
            // 2本が1本の曲線に近く、1本の曲線も2本に近いときだけ（片側だけ確かめると、
            // ほぼ一直線の2本で制御点が端の後ろに来て、曲線が端を越えて折り返すのを見逃す）
            let fits = [&q1, &q2]
                .iter()
                .all(|qq| (1..4).all(|k| nearest_on_quad(&q, qq.eval(k as f64 / 4.0)).1 < 0.01))
                && nearest_on_quad(&q, self.nodes[v]).1 < 0.01
                && (1..8).all(|k| {
                    let p = q.eval(k as f64 / 8.0);
                    nearest_on_quad(&q1, p).1 < 0.01 || nearest_on_quad(&q2, p).1 < 0.01
                });
            // 制御点が両端を結ぶ線の外に出ると、曲線が端を越えて折り返す。短い辺とまとめると折り返しが小さく、
            // 上の確かめ（0.01 以内）をすり抜けて、端に集まるほかの辺と交わる
            let chord = q.p2 - q.p0;
            let u = (q.p1 - q.p0).dot(chord) / chord.hypot2().max(1e-24);
            if !fits || !(0.0..=1.0).contains(&u) {
                continue;
            }
            // ほかの辺と交わるか（共有する頂点そのものは除く）
            let (boxes, grid) = index.get_or_insert_with(|| {
                let boxes: Vec<KRect> = (0..n).map(|i| quad_of(&edges, i).bounding_box()).collect();
                let mut grid: IntMap<(i64, i64), Vec<usize>> = IntMap::default();
                for (i, b) in boxes.iter().enumerate().filter(|(i, _)| alive[*i]) {
                    for c in cells(b) {
                        grid.entry(c).or_default().push(i);
                    }
                }
                (boxes, grid)
            });
            let bq = q.bounding_box();
            let mut seen: Vec<usize> = Vec::new();
            let crosses = cells(&bq).any(|c| {
                grid.get(&c).into_iter().flatten().any(|&f| {
                    if f == i || f == j || !alive[f] || seen.contains(&f) {
                        return false;
                    }
                    seen.push(f);
                    if !rects_overlap(bq, boxes[f]) {
                        return false;
                    }
                    let (fa, fb) = (edges[f].a, edges[f].b);
                    let shared: Vec<Point> = [e1.a, e2.b]
                        .into_iter()
                        .filter(|w| *w == fa || *w == fb)
                        .map(|w| self.nodes[w])
                        .collect();
                    !quad_intersections(&q, &quad_of(&edges, f), &shared).is_empty()
                })
            });
            if crosses {
                rejected[v] = true;
                continue;
            }
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
            boxes[i] = bq;
            for c in cells(&bq) {
                grid.entry(c).or_default().push(i);
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
        let mut half: Vec<(usize, usize, QuadBez, Color32)> =
            Vec::with_capacity(self.edges.len() * 2);
        for (i, e) in self.edges.iter().enumerate() {
            let q = self.quad(i);
            if let Some(c) = e.left {
                half.push((e.a, e.b, q, c));
            }
            if let Some(c) = e.right {
                half.push((e.b, e.a, QuadBez::new(q.p2, q.p1, q.p0), c));
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
                let back = -tangent_end(&half[cur].2);
                let next = out_of[half[cur].1]
                    .iter()
                    .copied()
                    .filter(|&h| half[h].3 == color && (!used[h] || h == start))
                    .min_by(|&x, &y| cmp_turn(back, &half[x].2, &half[y].2));
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

/// 編集中の階層の生の図形（選択中の図形を除く）から、レイヤーごとに地図を作る（前回と図形が同じなら作り直さない）
pub(crate) fn scope_maps(scene: Scene<'_>, session: &Session) -> Arc<ScopeMaps> {
    use std::hash::{Hash, Hasher};
    let scope = &session.editing_group_path;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    scope.hash(&mut h);
    // 選択中の図形（浮いている図形は、ほかの図形と重なることがある）は入れない
    let in_map = |s: &FillShape| is_map_shape(s, scope) && !session.selection.contains_shape(s);
    for s in scene.iter().filter(|s| in_map(s)) {
        (s.layer_id, s.shape_id, s.mesh_revision).hash(&mut h);
    }
    let key = h.finish();
    let mut cache = session.planar_maps.lock();
    if let Some(c) = cache.as_ref().filter(|c| c.key == key) {
        return c.clone();
    }
    let mut by_layer: Vec<(LayerId, usize, Vec<&FillShape>)> = Vec::new();
    for (idx, s) in scene.iter().enumerate().filter(|(_, s)| in_map(s)) {
        match by_layer.iter_mut().find(|x| x.0 == s.layer_id) {
            Some(x) => x.2.push(s),
            None => by_layer.push((s.layer_id, idx, vec![s])),
        }
    }
    // 地図にできないレイヤー（格子に置けない座標がある）は入れない（カーソル合わせも変形もしない）
    let maps = by_layer
        .into_iter()
        .filter_map(|(layer_id, first_idx, shapes)| {
            let (map, repainted) = layer_map(&shapes)?;
            Some(LayerMap {
                layer_id,
                group_path: scope.clone(),
                map: Arc::new(map),
                repainted,
                first_idx,
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
/// 追加は辺の上の位置を 0.08〜0.92 に収める。削除は、消せる頂点（`with_anchor_deleted` が形を返す頂点）だけ
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
            if best.as_ref().is_none_or(|b| d < b.0) && !hidden_by_front(scene, session, lm, p) {
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

    /// 編集をかけた形と、動いた辺
    pub(crate) fn apply_edit(&self, edit: MapEdit) -> Option<(PlanarMap, Vec<usize>)> {
        match edit {
            MapEdit::PullEdge {
                orig_seg,
                grab_t,
                target,
            } => {
                let (e, forward) = self.find_edge(&orig_seg)?;
                let t = if forward { grab_t } else { 1.0 - grab_t };
                Some((self.with_edge_pulled(e, t, target), vec![e]))
            }
            MapEdit::MoveVertex {
                orig_vertex,
                target,
            } => {
                let v = (0..self.nodes.len()).find(|&v| self.nodes[v] == orig_vertex)?;
                Some((self.with_node_moved(v, target), self.edges_at(v)))
            }
            MapEdit::AddAnchor { .. } | MapEdit::DeleteAnchor { .. } => None,
        }
    }

    /// 辺 `e` を `t`（0.08〜0.92 に収める）の所で2本に切った形（形は変わらない）
    pub(crate) fn with_anchor_added(&self, e: usize, t: f64) -> PlanarMap {
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
        m
    }

    /// 頂点 `v` を消し、前後の辺を1本の直線にした形。消せないときは `None`：
    /// つながる辺が2本でない（3色以上が接する点など）、前後が同じ頂点（細いレンズ形）、
    /// 直線がほかの辺と交わる・ほかの頂点や辺をまたぐ、両側の面が潰れる・裏返る
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
        let mut m = self.clone();
        m.edges[at[0]] = MapEdge {
            a: e1.a,
            b: e2.b,
            ctrl: pa.midpoint(pb),
            ..e1
        };
        let ne = if at[0] == m.edges.len() - 1 {
            at[1]
        } else {
            at[0]
        };
        m.edges.swap_remove(at[1]);
        if !m
            .crossings_grid(&[ne], &vec![false; m.edges.len()])
            .is_empty()
        {
            return None;
        }
        // 前の道筋（消す頂点を通る2本）と直線で囲む所に、ほかの頂点や辺が入らないか
        let mut swept = BezPath::new();
        swept.move_to(pa);
        swept.quad_to(e1.ctrl, self.nodes[v]);
        swept.quad_to(e2.ctrl, pb);
        swept.close_path();
        let others_inside = (0..m.nodes.len())
            .filter(|&w| w != v && w != e1.a && w != e2.b)
            .any(|w| swept.winding(m.nodes[w]) != 0)
            || (0..m.edges.len())
                .filter(|&f| f != ne)
                .any(|f| swept.winding(m.quad(f).eval(0.5)) != 0);
        if others_inside {
            return None;
        }
        // 色のある側の面が、潰れたり裏返ったりしないか（前の面と面積の正負が同じで、面積が 1e-4 より大きい）
        let forward = self.edges[at[0]].b == v;
        let (cycles0, cycles1) = (self.face_cycles(), m.face_cycles());
        for (before, after, color) in [
            ((at[0], forward), (ne, true), e1.left),
            ((at[0], !forward), (ne, false), e1.right),
        ] {
            if color.is_none() {
                continue;
            }
            // 向きのある辺 `h` を含む面の輪郭の面積（閉じない輪郭は、たどれた所まで）
            let area = |map: &PlanarMap, cycles: &[Vec<(usize, bool)>], h: (usize, bool)| {
                let cycle: Vec<(usize, bool)> = cycles
                    .iter()
                    .find(|c| c.contains(&h))
                    .map(|c| c.iter().copied().filter(|x| x.0 != usize::MAX).collect())
                    .unwrap_or_default();
                if cycle.is_empty() {
                    0.0
                } else {
                    map.cycle_area(&cycle)
                }
            };
            let (a0, a1) = (area(self, &cycles0, before), area(&m, &cycles1, after));
            if a1.abs() <= 1e-4 || a0.signum() != a1.signum() {
                return None;
            }
        }
        m.compact_nodes();
        Some(m)
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
            map.with_anchor_added(e, if forward { t } else { 1.0 - t })
        }
        MapEdit::DeleteAnchor { orig_vertex } => {
            let v = (0..map.nodes.len()).find(|&v| map.nodes[v] == orig_vertex)?;
            map.with_anchor_deleted(v)?
        }
        MapEdit::PullEdge { .. } | MapEdit::MoveVertex { .. } => {
            let (edited, moved) = map.apply_edit(edit)?;
            commit_edit(map, &base.cycles, edited, &moved).0
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
        let (map_faces, broken) = map.faces();
        let faces: Vec<FillShape> = map_faces
            .iter()
            .map(|f| map.face_shape(f, layer_id, &group_path))
            .collect();
        let all: Vec<usize> = (0..map.edges.len()).collect();
        // 壊れた数（`faces` の2つ目）には、閉じない輪郭と色がまざる輪郭も入っている
        if broken > 0 || !map.crossings_grid(&all, &vec![false; all.len()]).is_empty() {
            // 境目がそろっていない（地図にならない）ときは、まとめずにそのまま
            out.extend(list);
            continue;
        }
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

/// 生の図形 `own`（下から上への並び）の地図と、塗り重ねて作り直したか。図形から作った地図に壊れた輪郭（閉じない・色がまざる。
/// 前の版で描いた、重なった図形など）があれば、並びの順に塗り重ねて作り直す（`painted_map`。絵は同じ）。
/// `None`：格子に置けない座標がある
fn layer_map(own: &[&FillShape]) -> Option<(PlanarMap, bool)> {
    let map = PlanarMap::from_faces(own.iter().copied());
    if map.face_cycles().iter().any(|c| map.is_broken_cycle(c)) {
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
    let dd = (q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2()).hypot();
    // 2次ベジェを n 等分した折れ線の離れ具合は、およそ |p0 - 2p1 + p2| / (4 n²)
    let n = ((dd / (4.0 * tol)).sqrt().ceil() as usize).clamp(1, 512);
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            (q.eval(t), t)
        })
        .collect()
}

/// 整数の向きを、角度の順（x 軸の正の向きから、面積の式で正の回り方）に比べる
fn cmp_dir(a: (i64, i64), b: (i64, i64)) -> std::cmp::Ordering {
    let half = |d: (i64, i64)| {
        if d.1 > 0 || (d.1 == 0 && d.0 > 0) {
            0u8
        } else {
            1
        }
    };
    half(a).cmp(&half(b)).then_with(|| {
        let c = a.0 as i128 * b.1 as i128 - a.1 as i128 * b.0 as i128;
        0i128.cmp(&c)
    })
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

/// 面の輪郭の巻き数を速く数えるための、輪郭の区間を横の帯に分けた道筋。点の帯にかかる区間だけを数える
/// （大きな面の輪郭は区間が多く、穴ごとに全部を数えると重い）。巻き数は区間ごとの和なので、輪郭全体を数えたときと同じ。
/// 帯ごとの道筋は、区間ごとに始点へ移って描くだけで閉じない（閉じる線が足されない）
struct BandedCycle {
    y0: f64,
    band_h: f64,
    bands: Vec<BezPath>,
}

impl BandedCycle {
    /// 地図 `map` の面の輪郭 `cycle`（外接矩形 `bbox`）から作る
    fn new(map: &PlanarMap, cycle: &[(usize, bool)], bbox: KRect) -> Self {
        let n = ((cycle.len() as f64).sqrt() as usize).clamp(1, 256);
        let mut me = Self {
            y0: bbox.y0,
            band_h: (bbox.height() / n as f64).max(1e-9),
            bands: vec![BezPath::new(); n],
        };
        for h in cycle {
            let q = map.half_quad(*h);
            let b = q.bounding_box();
            let (lo, hi) = (me.band_of(b.y0), me.band_of(b.y1));
            for band in &mut me.bands[lo..=hi] {
                band.move_to(q.p0);
                band.quad_to(q.p1, q.p2);
            }
        }
        me
    }

    fn band_of(&self, y: f64) -> usize {
        (((y - self.y0) / self.band_h).max(0.0) as usize).min(self.bands.len() - 1)
    }

    fn winding(&self, p: Point) -> i32 {
        self.bands[self.band_of(p.y)].winding(p)
    }
}

impl PlanarMap {
    /// 点 `p` の色：右へ伸ばした半直線が最初に当たる辺の、点の側の色（`first_right_hit`）
    #[cfg(test)]
    pub(crate) fn color_at(&self, p: Point) -> Option<Color32> {
        let items: Vec<_> = (0..self.edges.len())
            .map(|e| (self.quad(e), (self.edges[e].left, self.edges[e].right)))
            .collect();
        first_right_hit(p, &items).flatten()
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
        let all = self.face_cycles();
        let broken = self.cycle_counts(&all) != (0, 0);
        // 閉じない輪郭（壊れた所）は使わない
        let cycles: Vec<Vec<(usize, bool)>> = all
            .into_iter()
            .filter(|c| c.iter().all(|h| h.0 != usize::MAX))
            .collect();
        let info: Vec<(f64, BezPath, KRect)> = cycles
            .iter()
            .map(|c| {
                let contour = Contour::new(c.iter().map(|h| self.half_quad(*h)).collect());
                let mut path = BezPath::new();
                contour.append_to_bez_path(&mut path);
                (contour.signed_area(), path, contour.bounding_box())
            })
            .collect();
        // 点を含む、いちばん小さい外周（面積が正）
        let inside = |k: usize, q: Point| info[k].2.contains(q) && info[k].1.winding(q) != 0;
        let owner_of = |q: Point| {
            (0..cycles.len())
                .filter(|&k| info[k].0 > 0.0 && inside(k, q))
                .min_by(|&x, &y| info[x].0.total_cmp(&info[y].0))
        };
        let Some(ext) = owner_of(p) else {
            return (!broken).then(|| self.clone());
        };
        if self.side_color(cycles[ext][0]) == Some(color) {
            return Some(self.clone());
        }
        // その外周の中にある穴（面積が負）のうち、いちばん近い外周がこの外周のもの。穴は外周の外接矩形の中にあるものだけ、
        // それより小さい外周のどれにも入らず、外周の中にあるかを見る（大きな面の外周は区間が多いので、帯に分けて数える）
        let ext_box = info[ext].2;
        let ext_banded = BandedCycle::new(self, &cycles[ext], ext_box);
        let in_ext = |q: Point| ext_box.contains(q) && ext_banded.winding(q) != 0;
        let mut face: Vec<usize> = vec![ext];
        for k in 0..cycles.len() {
            if info[k].0 < 0.0 && info[ext].2.contains(info[k].2.center()) {
                // 穴の輪郭の上の点は、どの外周に入るかがあいまいなので、穴を少し内側（左側）から見る
                let q = self.half_quad(cycles[k][0]);
                let tan = quad_deriv(&q, 0.5);
                let len = tan.hypot();
                if len < 1e-12 {
                    continue;
                }
                let probe = q.eval(0.5) + KVec2::new(-tan.y, tan.x) / len * 1e-4;
                let smaller_owner = (0..cycles.len())
                    .any(|j| info[j].0 > 0.0 && info[j].0 < info[ext].0 && inside(j, probe));
                if !smaller_owner && in_ext(probe) {
                    face.push(k);
                }
            }
        }
        let mut m = self.clone();
        for k in face {
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
        m.merge_degree2(info[ext].2.inflate(1.0, 1.0));
        m.compact_nodes();
        Some(m)
    }

    /// 形か地図 `input` を、`rule` の決め方で重ねた地図
    /// （設計：claude/曲線の当て直しを決まりで行う設計_7215b88.md）。
    ///
    /// 1. i_overlay に入れる前からある辺を、交わりで決める（`fed_edges`）。重ねる線と一緒に細かい折れ線にして格子に置き、
    ///    i_overlay で交わる所で切る。前からある辺は Subject、重ねる線は Clip（偶奇の規則。重ねる形は重なりの無い形にしておくこと）
    /// 2. 入れなかった辺が、切った区間と交わっていたら（交点を格子に丸めたずれなど）、その辺も入れて切り直す
    ///    （入れる辺は増える一方なので、必ず終わる）
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
    /// （`GRID_REACH`）以内：i_overlay は交点を格子の点に丸めるので、切った区間は折れ線からその距離まで動く。
    /// ただし、入れた辺と頂点を共有する辺は、その頂点でいつも触れているので、共有する頂点のほかで交わるかで見る。
    /// 入れた辺の端の頂点は格子に置き直すので、その頂点につながる入れない辺は、置き直した形で調べ直す
    /// （ほかの入れない辺と交わらないかも見る）。`None`：格子に置けない座標がある
    fn fed_edges(&self, new_quads: &[QuadBez], mut fed: Vec<bool>) -> Option<FedEdges> {
        let mut snapped = vec![false; self.nodes.len()];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        let mut edge_grid = BoxGrid::new(64.0);
        // 辺の外接矩形。格子に置き直すと端は最大で `GRID_REACH` 動くので、その分と、近くとみなす距離の分を広げておく
        let boxes: Vec<KRect> = (0..self.edges.len())
            .map(|e| {
                let r = 2.0 * GRID_REACH;
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
        let curve = |e: usize, snapped: &[bool]| -> QuadBez {
            let ed = self.edges[e];
            let pos = |v: usize| {
                if snapped[v] {
                    grid_round(self.nodes[v])
                } else {
                    self.nodes[v]
                }
            };
            QuadBez::new(pos(ed.a), ed.ctrl, pos(ed.b))
        };
        // 折れ線の区間：（始点, 終点, 始点の頂点, 終点の頂点）。頂点は、前からある辺の折れ線の両端だけ
        type GridSeg = (Point, Point, Option<usize>, Option<usize>);
        // 折れ線：（外接矩形, 区間の範囲）
        type GridLine = (KRect, std::ops::Range<usize>);
        fn add_polyline(
            q: &QuadBez,
            ends: (Option<usize>, Option<usize>),
            segs: &mut Vec<GridSeg>,
            seg_grid: &mut BoxGrid,
            lines: &mut Vec<GridLine>,
        ) -> Option<()> {
            let line = grid_polyline(q)?;
            let last = line.len().saturating_sub(1);
            let start = segs.len();
            let mut bbox: Option<KRect> = None;
            for (k, (a, b)) in line.into_iter().enumerate() {
                let r = KRect::from_points(a, b);
                bbox = Some(bbox.map_or(r, |x| x.union(r)));
                seg_grid.insert(segs.len(), r);
                segs.push((
                    a,
                    b,
                    if k == 0 { ends.0 } else { None },
                    if k == last { ends.1 } else { None },
                ));
            }
            if let Some(r) = bbox {
                lines.push((r, start..segs.len()));
            }
            Some(())
        }
        // 入れない辺 `e`（形 `c`）が、区間 `s` の近くを通るか
        let near_seg = |e: usize, c: &QuadBez, s: &GridSeg| -> bool {
            if !rects_overlap(boxes[e], KRect::from_points(s.0, s.1)) {
                return false;
            }
            let ed = self.edges[e];
            let shared: Vec<Point> = [(s.2, s.0), (s.3, s.1)]
                .into_iter()
                .filter(|(v, _)| v.is_some_and(|v| v == ed.a || v == ed.b))
                .map(|(_, p)| p)
                .collect();
            if shared.is_empty() {
                quad_seg_distance(c, s.0, s.1) <= GRID_REACH
            } else {
                !quad_intersections(c, &QuadBez::new(s.0, s.0.midpoint(s.1), s.1), &shared)
                    .is_empty()
            }
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
                    let c = curve(e, &snapped);
                    if segs[range.clone()].iter().any(|s| near_seg(e, &c, s)) {
                        to_feed.push(e);
                    }
                }
            } else if let Some(e) = edge_queue.pop() {
                if fed[e] {
                    continue;
                }
                let c = curve(e, &snapped);
                let r = c.bounding_box().inflate(GRID_REACH, GRID_REACH);
                seg_grid.query(r, &mut found);
                let mut hit = found.iter().any(|&i| near_seg(e, &c, &segs[i]));
                if !hit {
                    // 端を置き直した形が、ほかの入れない辺と交わらないか（共有する頂点は除く）
                    edge_grid.query(r, &mut found);
                    let ed = self.edges[e];
                    hit = found.iter().any(|&f| {
                        let fd = self.edges[f];
                        if f == e || fed[f] {
                            return false;
                        }
                        let cf = curve(f, &snapped);
                        let shared: Vec<Point> = [(ed.a, c.p0), (ed.b, c.p2)]
                            .into_iter()
                            .filter(|(v, _)| *v == fd.a || *v == fd.b)
                            .map(|(_, p)| p)
                            .collect();
                        !quad_intersections(&c, &cf, &shared).is_empty()
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
        // 重ねる地図の辺も、塗り（Clip の左右）を使わないので、前からある辺と同じく向きをそろえて入れる
        let fill_unused_for_new = matches!(input, MapInput::Map(_));
        for (si, (q, kind)) in srcs.iter().enumerate() {
            let old = matches!(kind, OverlaySrc::Old(_));
            let canonical = old || fill_unused_for_new;
            let st = if old {
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
                // 塗りを使わない辺（前からある辺と、重ねる地図の辺）の区間は、いつも小さい点から大きい点の向きで入れる
                // （ほぼ重なった2本の辺が、格子で同じ区間になったときに打ち消し合って消えないように）
                let (a, b, t0, t1) = if canonical && ip < prev.0 {
                    (ip, prev.0, t, prev.1)
                } else {
                    (prev.0, ip, prev.1, t)
                };
                let id = ov.data_store_mut().push(vec![Prov {
                    src: si as u32,
                    t0,
                    t1,
                }]);
                ov.add_edge(InputEdge { a, b, data: id }, st);
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
                    let c = QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]);
                    let shared: Vec<Point> = [p.a, p.b]
                        .into_iter()
                        .filter(|v| *v == ed.a || *v == ed.b)
                        .map(|v| nodes[v])
                        .collect();
                    let near = if shared.is_empty() {
                        quad_seg_distance(&c, a, b) <= GRID_REACH
                    } else {
                        !quad_intersections(&c, &QuadBez::new(a, a.midpoint(b), b), &shared)
                            .is_empty()
                    };
                    if near {
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
            list.sort_by(|x, y| cmp_dir(dir_of(x), dir_of(y)));
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
        let mut stops: IntMap<usize, Vec<(Turn, KVec2)>> = IntMap::default();
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
                for (end, at_start) in [(ed.a, true), (ed.b, false)] {
                    if end != v {
                        continue;
                    }
                    let other = if at_start { nodes[ed.b] } else { nodes[ed.a] };
                    let mut d = ed.ctrl - nodes[v];
                    if d.hypot2() == 0.0 {
                        d = other - nodes[v];
                    }
                    stops
                        .entry(v)
                        .or_default()
                        .push((Turn::Stop(e, at_start), d));
                }
            }
        }
        let mut next_old_of = vec![Turn::Piece(0, false); 2 * pieces.len()];
        for (&v, list) in &around {
            let mut turns: Vec<(Turn, KVec2)> = list
                .iter()
                .map(|&(k, fwd)| {
                    let (dx, dy) = dir_of(&(k, fwd));
                    (Turn::Piece(k, fwd), KVec2::new(dx as f64, dy as f64))
                })
                .collect();
            if let Some(extra) = stops.get(&v) {
                turns.extend(extra.iter().copied());
            }
            turns.sort_by(|x, y| cmp_dir_f(x.1, y.1));
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
        // 輪郭をたどっても決まらない所の色は、点から右へ伸ばした半直線が最初に当たる辺の、点の側の色で決める
        // （`first_right_hit`）。前の色：前の色を持つ区間と、入れなかった前からある辺。重ねる地図の色：重ねる地図の色を
        // 持つ区間。どれも、結果の地図と同じ形（切った区間・格子に置き直した頂点）で、面ごとに決めた色（`face_colors`）で見る
        let piece_quad = |k: usize| {
            let (a, b) = (nodes[pieces[k].a], nodes[pieces[k].b]);
            QuadBez::new(a, a.midpoint(b), b)
        };
        // 入れなかった前からある辺（結果の地図と同じ形）。前の地図の面を分ける
        let unfed_quads: Vec<(QuadBez, Sides)> = self
            .edges
            .iter()
            .enumerate()
            .filter(|(e, _)| !fed[*e])
            .map(|(_, ed)| {
                (
                    QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]),
                    (ed.left, ed.right),
                )
            })
            .collect();
        // 地図の元の形での点の色（面の色の材料が食い違うときだけ使う）
        let old_truth = |q: Point| {
            let items: Vec<(QuadBez, Sides)> = (0..self.edges.len())
                .map(|e| (self.quad(e), (self.edges[e].left, self.edges[e].right)))
                .collect();
            first_right_hit(q, &items).flatten()
        };
        let new_truth = |q: Point| {
            let items: Vec<(QuadBez, Sides)> = new_quads
                .iter()
                .copied()
                .zip(new_cols.iter().copied())
                .collect();
            first_right_hit(q, &items).flatten()
        };
        let piece_segs: Vec<(Point, Point)> =
            pieces.iter().map(|p| (nodes[p.a], nodes[p.b])).collect();
        let old_faces = face_colors(
            &old_of,
            &next_old,
            &|e| {
                let ed = self.edges[e];
                (ed.left, ed.right)
            },
            &piece_segs,
            &unfed_quads,
            &old_truth,
        );
        let new_faces = match input {
            MapInput::Shapes(_) => None,
            MapInput::Map(_) => Some(face_colors(
                &new_of,
                &next_new,
                &|_| unreachable!("重ねる地図の辺はみな区間になる"),
                &piece_segs,
                &[],
                &new_truth,
            )),
        };
        // 面ごとに決めた色での、区間の左右の色
        let resolved = |faces: &FaceColors, k: usize| -> Sides {
            (
                faces.color[faces.root[2 * k + 1] as usize].flatten(),
                faces.color[faces.root[2 * k] as usize].flatten(),
            )
        };
        // 前の色の索引は、材料の無い面が出たときだけ作る
        let old_index: std::cell::OnceCell<RayIndex<Option<Color32>>> = std::cell::OnceCell::new();
        let build_old_index = || {
            let mut old_items: Vec<(QuadBez, Sides)> = (0..pieces.len())
                .filter(|&k| old_of[k].is_some())
                .map(|k| (piece_quad(k), resolved(&old_faces, k)))
                .collect();
            old_items.extend(unfed_quads.iter().copied());
            RayIndex::new(old_items)
        };
        let new_index = new_faces.as_ref().map(|nf| {
            RayIndex::new(
                (0..pieces.len())
                    .filter(|&k| new_of[k].is_some())
                    .map(|k| (piece_quad(k), resolved(nf, k)))
                    .collect(),
            )
        });
        let new_fallback = |q: Point| new_index.as_ref().and_then(|ix| ix.hit(q)).flatten();
        // 材料の無い面の色：面の中の区間の真ん中の点から、半直線で調べる（面ごとに1回）
        let mut old_memo: IntMap<u32, Option<Color32>> = IntMap::default();
        let mut new_memo: IntMap<u32, Option<Color32>> = IntMap::default();
        let color_of = |faces: &FaceColors,
                        memo: &mut IntMap<u32, Option<Color32>>,
                        h: (usize, bool),
                        fallback: &dyn Fn(Point) -> Option<Color32>|
         -> Option<Color32> {
            let r = faces.root[2 * h.0 + h.1 as usize];
            if let Some(c) = faces.color[r as usize] {
                return c;
            }
            *memo.entry(r).or_insert_with(|| {
                let p = &pieces[h.0];
                fallback(nodes[p.a].midpoint(nodes[p.b]))
            })
        };
        let old_fallback = |q: Point| old_index.get_or_init(build_old_index).hit(q).flatten();
        // 重ねる形の中か：区間ごとに i_overlay が数えた、左右が重ねる形の中か
        let fill_index = match input {
            MapInput::Shapes(_) => Some(RayIndex::new(
                (0..pieces.len())
                    .map(|k| {
                        let f = pieces[k].fill;
                        (piece_quad(k), (f & CLIP_LEFT != 0, f & CLIP_RIGHT != 0))
                    })
                    .collect(),
            )),
            MapInput::Map(_) => None,
        };
        // 5. 区間の新しい色。左右が違う区間だけ残す：（始点, 終点, (左の色, 右の色), 区間の番号）
        let mut kept: Vec<(usize, usize, Sides, usize)> = Vec::new();
        for (k, piece) in pieces.iter().enumerate() {
            let mut side = |fwd: bool| {
                let old = color_of(&old_faces, &mut old_memo, (k, fwd), &old_fallback);
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
                        let c = color_of(nf, &mut new_memo, (k, fwd), &new_fallback);
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
        // 中の辺（重ねるものの範囲の中にあって、入れなかった辺）：辺の真ん中の点で、重ねるものの中か（地図なら色）を決めて
        // 両側に `rule` を当てる。区間と交わらないので、辺のどこでも同じ。左右が同じ色になれば消える
        let mut interior_kept: Vec<(usize, MapEdge)> = Vec::new();
        for e in (0..self.edges.len()).filter(|&e| interior[e]) {
            let ed = self.edges[e];
            let m = QuadBez::new(nodes[ed.a], ed.ctrl, nodes[ed.b]).eval(0.5);
            let (inside, p) = match input {
                MapInput::Shapes(_) => (
                    fill_index.as_ref().and_then(|f| f.hit(m)).unwrap_or(false),
                    None,
                ),
                MapInput::Map(_) => {
                    let c = new_fallback(m);
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
            .map(|r| r.inflate(SPLIT_TOL, SPLIT_TOL));
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
    /// 代表ごとの面の色。材料が無い面は `None`
    color: Vec<Option<Option<Color32>>>,
}

/// 片方の地図の色を、切った区間（端は `segs`）の面ごとに1つに決める（`FaceColors`）。
///
/// 同じ面とみなすのは、面の輪郭をたどってつながる向きのある区間（`next` が区間を返す）と、その地図の色を持たない区間の
/// 両側（その地図の面を分けない）。面の色の材料は、面の中の区間の出どころの辺の色（`of`）と、たどって止まった
/// 入れなかった前からある辺の色（`stop_sides`）。材料がみな同じなら、その色。
///
/// 材料が食い違うのは、その地図に、折れ線にして格子に置く間に裏返るほど細い面がある所だけ（細い面をはさむ2本の辺の
/// 折れ線が入れかわると、それぞれの辺が、自分の側だと思っている面が、格子の上では別の面になる）。そのときは、面の中の
/// 奥の点の色を、その地図の元の形（`truth`）で調べる。奥の点：面のいちばん長い区間の真ん中から、面の側へ垂直に
/// 伸ばした半直線が、その地図の面を分ける最初の線（色を持つ区間と `bounds`）に当たるまでの、半分の所
/// （当たらなければ、面は外へ開いているので、色は無い）
fn face_colors(
    of: &[Option<Sides>],
    next: &dyn Fn((usize, bool)) -> Turn,
    stop_sides: &dyn Fn(usize) -> Sides,
    segs: &[(Point, Point)],
    bounds: &[(QuadBez, Sides)],
    truth: &dyn Fn(Point) -> Option<Color32>,
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
    // 向きのある区間の始点と終点
    let ends = |h: usize| {
        let (a, b) = segs[h / 2];
        if h % 2 == 1 {
            (a, b)
        } else {
            (b, a)
        }
    };
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
    let mut longest: IntMap<u32, (f64, usize)> = IntMap::default();
    for (h, &r) in root.iter().enumerate() {
        if claim[r as usize] != Claim::Mixed {
            continue;
        }
        let (a, b) = ends(h);
        let len2 = (b - a).hypot2();
        let slot = longest.entry(r).or_insert((-1.0, h));
        if len2 > slot.0 {
            *slot = (len2, h);
        }
    }
    let mut color: Vec<Option<Option<Color32>>> = claim
        .iter()
        .map(|c| match c {
            Claim::One(c) => Some(*c),
            _ => None,
        })
        .collect();
    for (r, (_, h)) in longest {
        let (a, b) = ends(h);
        let d = (b - a) / (b - a).hypot();
        let (m, nrm) = (a.midpoint(b), KVec2::new(-d.y, d.x));
        let k = h / 2;
        let hit = (0..n)
            .filter(|&j| j != k && of[j].is_some())
            .map(|j| {
                let (pa, pb) = segs[j];
                QuadBez::new(pa, pa.midpoint(pb), pb)
            })
            .chain(bounds.iter().map(|(q, _)| *q))
            .filter_map(|q| ray_quad_dist(m, nrm, q))
            .reduce(f64::min);
        color[r as usize] = Some(hit.and_then(|s| truth(m + nrm * (s * 0.5))));
    }
    FaceColors { root, color }
}

/// 点 `m` から向き `dir`（長さ1）へ伸ばした半直線が、曲線 `q` に最初に当たるまでの距離（当たらなければ `None`）
fn ray_quad_dist(m: Point, dir: KVec2, q: QuadBez) -> Option<f64> {
    // 半直線に垂直な向きの成分 g(t) = dir × (q(t) − m) が 0 になる t
    let g = |p: Point| dir.cross(p - m);
    let (g0, g1, g2) = (g(q.p0), g(q.p1), g(q.p2));
    kurbo::common::solve_quadratic(g0, 2.0 * (g1 - g0), g0 - 2.0 * g1 + g2)
        .into_iter()
        .filter(|t| (0.0..=1.0).contains(t))
        .map(|t| dir.dot(q.eval(t) - m))
        .filter(|s| *s > 0.0)
        .reduce(f64::min)
}

/// 向きを、角度の順（x 軸の正の向きから、面積の式で正の回り方）に比べる（`cmp_dir` の小数版）
fn cmp_dir_f(a: KVec2, b: KVec2) -> std::cmp::Ordering {
    let half = |d: KVec2| {
        if d.y > 0.0 || (d.y == 0.0 && d.x > 0.0) {
            0u8
        } else {
            1
        }
    };
    half(a)
        .cmp(&half(b))
        .then_with(|| 0.0f64.total_cmp(&a.cross(b)))
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

/// 直線の区間の鎖を、元の曲線の一部へ置き換える（設計 2-3）。置き換えてよいのは、今の地図で次の3つを満たすときだけ：
/// 1. 曲線 C が、ほかの辺と、共有する端のほかで交わらない（触れるのも交わりとみなす）
/// 2. 区間の列 P と C で囲まれた所（帯）の中に、ほかの頂点が無い。読み直し（`from_faces`）で辺が切られないよう、
///    C から `SPLIT_TOL` 以内にも頂点が無い
/// 3. 帯の両端の頂点につながるほかの辺が、帯の中に無い（その辺は P とも C とも交わらないので、真ん中の点1つで決まる）
///
/// 満たさなければ鎖を真ん中で2つに分け、区間1つでも満たさなければ直線のまま残す
struct ChainRestorer {
    map: PlanarMap,
    boxes: Vec<KRect>,
    alive: Vec<bool>,
    adj: Vec<Vec<usize>>,
    deg: Vec<usize>,
    edge_grid: BoxGrid,
    node_grid: NodeGrid,
    found: Vec<usize>,
}

impl ChainRestorer {
    /// `active`：置き換える曲線と帯がすべて入る矩形（`None` なら鎖が無い）。調べる相手（格子に入れる辺・頂点）は、
    /// これに外接矩形がかかるものだけでよい
    fn new(map: PlanarMap, active: Option<KRect>) -> Self {
        if active.is_none() {
            // 置き換える鎖が無い
            return Self {
                alive: vec![true; map.edges.len()],
                map,
                boxes: Vec::new(),
                adj: Vec::new(),
                deg: Vec::new(),
                edge_grid: BoxGrid::new(16.0),
                node_grid: NodeGrid::new(16.0),
                found: Vec::new(),
            };
        }
        let mut adj = vec![Vec::new(); map.nodes.len()];
        let mut deg = vec![0; map.nodes.len()];
        let mut edge_grid = BoxGrid::new(16.0);
        let mut node_grid = NodeGrid::new(16.0);
        let boxes: Vec<KRect> = match active {
            Some(_) => (0..map.edges.len())
                .map(|e| map.quad(e).bounding_box())
                .collect(),
            None => vec![KRect::ZERO; map.edges.len()],
        };
        for (e, ed) in map.edges.iter().enumerate() {
            adj[ed.a].push(e);
            adj[ed.b].push(e);
            deg[ed.a] += 1;
            deg[ed.b] += 1;
            if active.is_some_and(|area| rects_overlap(boxes[e], area)) {
                edge_grid.insert(e, boxes[e]);
            }
        }
        if let Some(area) = active {
            for (v, p) in map.nodes.iter().enumerate() {
                if deg[v] > 0 && area.contains(*p) {
                    node_grid.insert(*p, v);
                }
            }
        }
        Self {
            alive: vec![true; map.edges.len()],
            map,
            boxes,
            adj,
            deg,
            edge_grid,
            node_grid,
            found: Vec::new(),
        }
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

    /// 鎖 `c` の区間 `i..j` を1本の曲線に置き換えられれば置き換える
    fn try_replace(&mut self, c: &Chain, ids: &[usize], i: usize, j: usize) -> bool {
        let (na, nb) = (c.steps[i].0, c.steps[j - 1].1);
        let (ta, tb) = (c.steps[i].2, c.steps[j - 1].3);
        if na == nb || ta == tb {
            return false;
        }
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
        // 帯：P をたどり、C を逆にたどって閉じる
        let mut band = BezPath::new();
        band.move_to(nodes[na]);
        for s in &c.steps[i..j] {
            band.line_to(nodes[s.1]);
        }
        band.quad_to(ctrl, nodes[na]);
        band.close_path();
        let reach = band
            .bounding_box()
            .union(q.bounding_box())
            .inflate(SPLIT_TOL, SPLIT_TOL);

        // 1. 交わり
        let qbox = q.bounding_box();
        self.edge_grid.query(qbox, &mut self.found);
        for &f in &self.found {
            if !self.alive[f] || !rects_overlap(qbox, self.boxes[f]) || part.contains(&f) {
                continue;
            }
            let ed = self.map.edges[f];
            let shared: Vec<Point> = [na, nb]
                .into_iter()
                .filter(|v| *v == ed.a || *v == ed.b)
                .map(|v| nodes[v])
                .collect();
            if !quad_intersections(&q, &self.map.quad(f), &shared).is_empty() {
                return false;
            }
        }
        // 2. 帯の中と、C のすぐそばの頂点
        let blocked = self.node_grid.in_rect(reach).any(|v| {
            reach.contains(nodes[v])
                && self.deg[v] > 0
                && v != na
                && v != nb
                && !inner.contains(&v)
                && (band.winding(nodes[v]) != 0 || nearest_on_quad(&q, nodes[v]).1 <= SPLIT_TOL)
        });
        if blocked {
            return false;
        }
        // 3. 帯の両端の頂点につながる辺
        for v in [na, nb] {
            for &f in &self.adj[v] {
                if self.alive[f]
                    && !part.contains(&f)
                    && band.winding(self.map.quad(f).eval(0.5)) != 0
                {
                    return false;
                }
            }
        }

        for &f in part {
            self.alive[f] = false;
            let ed = self.map.edges[f];
            self.deg[ed.a] -= 1;
            self.deg[ed.b] -= 1;
        }
        let (left_src, right_src) = whole.map_or((None, None), |w| (w.1, w.2));
        let e = self.map.edges.len();
        self.map.edges.push(MapEdge {
            a: na,
            b: nb,
            ctrl,
            left: c.left,
            right: c.right,
            left_src,
            right_src,
        });
        self.alive.push(true);
        self.adj[na].push(e);
        self.adj[nb].push(e);
        self.deg[na] += 1;
        self.deg[nb] += 1;
        self.edge_grid.insert(e, qbox);
        self.boxes.push(qbox);
        true
    }

    /// 置き換え終わった地図（使わなくなった辺を除く）
    fn finish(self) -> PlanarMap {
        let mut map = self.map;
        let alive = self.alive;
        let mut k = 0;
        map.edges.retain(|_| {
            k += 1;
            alive[k - 1]
        });
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

/// 曲線を、重ねるときと同じ折れ線（格子の点）にした区間の列
fn grid_polyline(q: &QuadBez) -> Option<Vec<(Point, Point)>> {
    let pts = flatten_with_t(q, FLAT_TOL);
    let mut prev = to_grid(pts[0].0)?;
    let mut out = Vec::new();
    for &(p, _) in &pts[1..] {
        let ip = to_grid(p)?;
        if ip != prev {
            out.push((
                from_grid(prev.x as i64, prev.y as i64),
                from_grid(ip.x as i64, ip.y as i64),
            ));
            prev = ip;
        }
    }
    Some(out)
}

/// 点 `p` から右（+x の向き）へ伸ばした半直線が、`items` の辺（曲線, (左の値, 右の値)）のうち最初に当たるものの、
/// 点の側の値（当たらなければ `None`）。曲線は y について単調な部分に分け、部分の下の端は含め、上の端は含めない
/// （半開区間）ので、頂点にちょうど当たっても、そこを通る辺のどちらか一方だけに当たる。同じ所に当たる辺どうしは、
/// 点を上へごくわずか動かしたときに先に当たるもの（上へ進むと左へ寄るもの）を選ぶ
fn first_right_hit<'a, T: Copy + 'a>(
    p: Point,
    items: impl IntoIterator<Item = &'a (QuadBez, (T, T))>,
) -> Option<T> {
    let mut best: Option<(f64, f64, T)> = None;
    for (q, sides) in items {
        ray_hit_into(p, q, *sides, &mut best);
    }
    best.map(|b| b.2)
}

/// `first_right_hit` の辺1本分：辺 `q` に当たれば、今までの一番近い当たり `best`（x, 傾き, 値）と比べて入れ替える
fn ray_hit_into<T: Copy>(
    p: Point,
    q: &QuadBez,
    (left, right): (T, T),
    best: &mut Option<(f64, f64, T)>,
) {
    let b = q.bounding_box();
    if p.y < b.y0 || p.y >= b.y1 || b.x1 <= p.x {
        return;
    }
    // y の極値で単調な部分に分ける
    let den = q.p0.y - 2.0 * q.p1.y + q.p2.y;
    let t_ext = if den != 0.0 {
        (q.p0.y - q.p1.y) / den
    } else {
        -1.0
    };
    let mut cuts = [0.0, 1.0, 1.0];
    let n = if t_ext > 0.0 && t_ext < 1.0 {
        cuts[1] = t_ext;
        3
    } else {
        2
    };
    for w in cuts[..n].windows(2) {
        let (ta, tb) = (w[0], w[1]);
        let (ya, yb) = (q.eval(ta).y, q.eval(tb).y);
        if !(ya.min(yb) <= p.y && p.y < ya.max(yb)) {
            continue;
        }
        // 単調な部分の中で y(t) = p.y になる t（二分法。y は ta から tb へ単調。区間がそれ以上縮まなくなるまで）
        let up = yb > ya;
        let (mut lo, mut hi) = (ta, tb);
        loop {
            let mid = 0.5 * (lo + hi);
            if mid <= lo || mid >= hi {
                break;
            }
            if (q.eval(mid).y < p.y) == up {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let t = 0.5 * (lo + hi);
        let x = q.eval(t).x;
        if x <= p.x {
            continue;
        }
        let d = quad_deriv(q, t);
        let slope = if d.y != 0.0 { d.x / d.y } else { 0.0 };
        if best.is_none_or(|(bx, bs, _)| x < bx || (x == bx && slope < bs)) {
            *best = Some((x, slope, if up { left } else { right }));
        }
    }
}

/// `first_right_hit` を速く行うための、辺を横の帯に分けた索引（点の帯にかかる辺だけを見る。答えは全部を見たときと同じ）
struct RayIndex<T> {
    items: Vec<(QuadBez, (T, T))>,
    x0s: Vec<f64>,
    y0: f64,
    band_h: f64,
    bands: Vec<Vec<u32>>,
}

impl<T: Copy> RayIndex<T> {
    fn new(items: Vec<(QuadBez, (T, T))>) -> Self {
        let boxes: Vec<KRect> = items.iter().map(|(q, _)| q.bounding_box()).collect();
        let (y0, y1) = boxes
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), b| {
                (lo.min(b.y0), hi.max(b.y1))
            });
        // 帯の数は辺の数の平方根（速さだけに関わる）
        let n = ((items.len() as f64).sqrt().ceil() as usize).max(1);
        let band_h = if y1 > y0 { (y1 - y0) / n as f64 } else { 1.0 };
        let mut me = Self {
            items,
            x0s: Vec::new(),
            y0,
            band_h,
            bands: vec![Vec::new(); n],
        };
        for (i, b) in boxes.iter().enumerate() {
            let (lo, hi) = (me.band_of(b.y0), me.band_of(b.y1));
            for band in &mut me.bands[lo..=hi] {
                band.push(i as u32);
            }
        }
        // 帯の中は、外接矩形の左端の順（左から見ていき、いちばん近い当たりより右から始まる辺で止める）
        for band in &mut me.bands {
            band.sort_by(|&x, &y| boxes[x as usize].x0.total_cmp(&boxes[y as usize].x0));
        }
        me.x0s = boxes.iter().map(|b| b.x0).collect();
        me
    }

    fn band_of(&self, y: f64) -> usize {
        (((y - self.y0) / self.band_h).max(0.0) as usize).min(self.bands.len() - 1)
    }

    fn hit(&self, p: Point) -> Option<T> {
        if self.items.is_empty() {
            return None;
        }
        let mut best: Option<(f64, f64, T)> = None;
        for &i in &self.bands[self.band_of(p.y)] {
            if best.is_some_and(|b| self.x0s[i as usize] > b.0) {
                break;
            }
            let (q, sides) = &self.items[i as usize];
            ray_hit_into(p, q, *sides, &mut best);
        }
        best.map(|b| b.2)
    }
}

/// 曲線 `q` と直線の区間 `a`–`b` の、いちばん近い所の距離。近い所は、端（曲線の端・区間の端）か、
/// 曲線が区間の線と交わる所か、曲線が区間の線と平行になる所のどれか
fn quad_seg_distance(q: &QuadBez, a: Point, b: Point) -> f64 {
    let d = b - a;
    let len2 = d.hypot2();
    if len2 == 0.0 {
        return nearest_on_quad(q, a).1;
    }
    let to_seg = |p: Point| {
        let s = ((p - a).dot(d) / len2).clamp(0.0, 1.0);
        (p - (a + d * s)).hypot()
    };
    let mut best = nearest_on_quad(q, a)
        .1
        .min(nearest_on_quad(q, b).1)
        .min(to_seg(q.p0))
        .min(to_seg(q.p2));
    // 区間の線からの符号つきの距離 f(t) = n·(q(t) - a) = A t² + B t + C
    let n = KVec2::new(-d.y, d.x) / len2.sqrt();
    let (f0, f1, f2) = (n.dot(q.p0 - a), n.dot(q.p1 - a), n.dot(q.p2 - a));
    let (aa, bb, cc) = (f0 - 2.0 * f1 + f2, 2.0 * (f1 - f0), f0);
    let mut ts: Vec<f64> = Vec::with_capacity(3);
    if aa != 0.0 {
        ts.push(-bb / (2.0 * aa));
        let disc = bb * bb - 4.0 * aa * cc;
        if disc >= 0.0 {
            let r = -0.5 * (bb + bb.signum() * disc.sqrt());
            ts.push(r / aa);
            if r != 0.0 {
                ts.push(cc / r);
            }
        }
    } else if bb != 0.0 {
        ts.push(-cc / bb);
    }
    for t in ts {
        if (0.0..=1.0).contains(&t) {
            let p = q.eval(t);
            let s = (p - a).dot(d) / len2;
            if (0.0..=1.0).contains(&s) {
                best = best.min(n.dot(p - a).abs());
            }
        }
    }
    best
}
