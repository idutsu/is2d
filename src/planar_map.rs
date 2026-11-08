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

/// 頂点を探すための格子
struct NodeGrid {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl NodeGrid {
    fn new(cell: f64) -> Self {
        Self {
            cell,
            cells: HashMap::new(),
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
        let (kx, ky) = self.key(p);
        let mut best = None;
        let mut best_d = tol;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &id in self.cells.get(&(kx + dx, ky + dy)).into_iter().flatten() {
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
    let a = (u.y.atan2(u.x) - w.y.atan2(w.x)).rem_euclid(TAU);
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
    fn rec(
        qa: &QuadBez,
        (a0, a1): (f64, f64),
        qb: &QuadBez,
        (b0, b1): (f64, f64),
        depth: u32,
        out: &mut Vec<(f64, f64)>,
    ) {
        let (sa, sb) = (qa.subsegment(a0..a1), qb.subsegment(b0..b1));
        let (ra, rb) = (sa.bounding_box(), sb.bounding_box());
        if !rects_overlap(ra.inflate(1e-9, 1e-9), rb) {
            return;
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
                rec(qa, ra, qb, rb, depth + 1, out);
            }
        }
    }
    let mut raw = Vec::new();
    rec(qa, (0.0, 1.0), qb, (0.0, 1.0), 0, &mut raw);
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
            let bbox = s.q.bounding_box().inflate(FACE_TOL, FACE_TOL);
            let mut cuts: Vec<(f64, usize)> = grid
                .in_rect(bbox)
                .filter(|&v| v != s.a && v != s.b)
                .filter_map(|v| {
                    let p = nodes[v];
                    if (p - s.q.p0).hypot() <= FACE_TOL || (p - s.q.p2).hypot() <= FACE_TOL {
                        return None;
                    }
                    let (t, d) = nearest_on_quad(&s.q, p);
                    (d <= FACE_TOL && t > 1e-6 && t < 1.0 - 1e-6).then_some((t, v))
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
        let mut groups: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
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

    /// 面を取り出す。外周（面積が正）に、それを含むいちばん小さい同じ色の外周の穴（面積が負）を入れる。
    /// 戻り値の2つ目は、閉じなかった輪郭・色がまざった輪郭・入れ先の無い穴の数（壊れていれば 0 より大きい）
    pub(crate) fn faces(&self) -> (Vec<MapFace>, usize) {
        self.faces_of(&self.face_cycles())
    }

    /// `faces` と同じ。面の輪郭 `cycles`（`face_cycles` の結果）を先に求めてあるときに使う
    fn faces_of(&self, cycles: &[Vec<(usize, bool)>]) -> (Vec<MapFace>, usize) {
        let mut broken = 0;
        let mut exteriors: Vec<(MapFace, f64, BezPath, KRect)> = Vec::new();
        let mut holes: Vec<(Color32, Vec<(usize, bool)>)> = Vec::new();
        for cycle in cycles.iter().cloned() {
            if cycle.iter().any(|h| h.0 == usize::MAX) {
                broken += 1;
                continue;
            }
            let color = self.side_color(cycle[0]);
            if cycle.iter().any(|h| self.side_color(*h) != color) {
                broken += 1;
                continue;
            }
            let Some(color) = color else {
                continue;
            };
            let contour = Contour::new(cycle.iter().map(|h| self.half_quad(*h)).collect());
            let area = contour.signed_area();
            if area > 0.0 {
                let mut path = BezPath::new();
                contour.append_to_bez_path(&mut path);
                let bbox = contour.bounding_box();
                exteriors.push((
                    MapFace {
                        color,
                        cycles: vec![cycle],
                        srcs: Vec::new(),
                    },
                    area,
                    path,
                    bbox,
                ));
            } else {
                holes.push((color, cycle));
            }
        }
        for (color, cycle) in holes {
            let probe = self.half_quad(cycle[0]).eval(0.5);
            let owner = exteriors
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.0.color == color && e.3.contains(probe) && e.2.winding(probe) != 0
                })
                .min_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
                .map(|(i, _)| i);
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
    /// 辺 `moved` とほかの辺（`moved` どうしも）の交わる所：（辺, t, 辺, u）
    fn crossings(&self, moved: &[usize]) -> Vec<(usize, f64, usize, f64)> {
        let mut out = Vec::new();
        // 動いた辺の、`moved` の中の順番（動いていない辺は usize::MAX）
        let mut pos = vec![usize::MAX; self.edges.len()];
        for (mi, &e) in moved.iter().enumerate() {
            pos[e] = pos[e].min(mi);
        }
        for (mi, &e) in moved.iter().enumerate() {
            let qe = self.quad(e);
            let be = qe.bounding_box();
            for f in 0..self.edges.len() {
                if f == e || pos[f] < mi {
                    continue;
                }
                let qf = self.quad(f);
                if !rects_overlap(be, qf.bounding_box()) {
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
                for (t, u) in quad_intersections(&qe, &qf, &shared) {
                    out.push((e, t, f, u));
                }
            }
        }
        out
    }

    /// テスト用：辺 `moved` とほかの辺の交わる所
    #[cfg(test)]
    pub(crate) fn crossings_pub(&self, moved: &[usize]) -> Vec<(usize, f64, usize, f64)> {
        self.crossings(moved)
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
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (e, b) in boxes.iter().enumerate() {
            for c in cells(b) {
                grid.entry(c).or_default().push(e);
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
    let hits = edited.crossings(moved);
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
    let hits = m.crossings(&checked);
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
    if m.cycle_check() != (0, 0) || !m.crossings(&near).is_empty() {
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

    /// 向きのある辺 `start` から、左側が同じ面になる輪を1つたどる（`face_cycles` と同じ決まり）
    fn cycle_from(&self, start: (usize, bool)) -> Vec<(usize, bool)> {
        let head = |h: (usize, bool)| {
            if h.1 {
                self.edges[h.0].b
            } else {
                self.edges[h.0].a
            }
        };
        let mut cycle = vec![start];
        let mut cur = start;
        while cycle.len() <= 2 * self.edges.len() {
            let v = head(cur);
            let back = -tangent_end(&self.half_quad(cur));
            let twin = (cur.0, !cur.1);
            let next = (0..self.edges.len())
                .flat_map(|i| [(i, true), (i, false)])
                .filter(|h| {
                    *h != twin
                        && (if h.1 {
                            self.edges[h.0].a
                        } else {
                            self.edges[h.0].b
                        }) == v
                })
                .min_by(|x, y| cmp_turn(back, &self.half_quad(*x), &self.half_quad(*y)))
                .unwrap_or(twin);
            if next == start {
                break;
            }
            cycle.push(next);
            cur = next;
        }
        cycle
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
                    let back = -tangent_end(&self.half_quad(cur));
                    let twin = (cur.0, !cur.1);
                    // 引き返す向き（同じ辺の逆向き）は、ほかに辺が無いときだけ選ぶ
                    let next = out_of[head(cur)]
                        .iter()
                        .copied()
                        .filter(|&h| h != twin)
                        .min_by(|&x, &y| cmp_turn(back, &self.half_quad(x), &self.half_quad(y)))
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
        let cycles = self.face_cycles();
        let open = cycles
            .iter()
            .filter(|c| c.iter().any(|h| h.0 == usize::MAX))
            .count();
        let mixed = cycles
            .iter()
            .filter(|c| {
                let cols: Vec<Option<Color32>> = c
                    .iter()
                    .filter(|h| h.0 != usize::MAX)
                    .map(|&(i, f)| {
                        if f {
                            self.edges[i].left
                        } else {
                            self.edges[i].right
                        }
                    })
                    .collect();
                cols.windows(2).any(|w| w[0] != w[1])
            })
            .count();
        (open, mixed)
    }
}

impl PlanarMap {
    /// 頂点 `a` から `b` への曲線 `q` が、`skip` 以外の辺と交わるか（共有する頂点そのものは除く）
    fn crosses_others(&self, q: &QuadBez, a: usize, b: usize, skip: [usize; 2]) -> bool {
        let bq = q.bounding_box();
        (0..self.edges.len()).any(|f| {
            if skip.contains(&f) {
                return false;
            }
            let qf = self.quad(f);
            if !rects_overlap(bq, qf.bounding_box()) {
                return false;
            }
            let (fa, fb) = (self.edges[f].a, self.edges[f].b);
            let shared: Vec<Point> = [a, b]
                .into_iter()
                .filter(|v| *v == fa || *v == fb)
                .map(|v| self.nodes[v])
                .collect();
            !quad_intersections(q, &qf, &shared).is_empty()
        })
    }

    /// 範囲 `area` の中で、2本の辺だけがつながる頂点のうち、両側の色が同じで、2本を1本の曲線で表せるものを消す
    /// （交わりで切った点が、相手の辺が消えた後に残らないように）
    pub(crate) fn merge_degree2(&mut self, area: KRect) {
        // まとめると、ほかの辺と交わってしまう頂点（まとめない）
        let mut rejected: Vec<usize> = Vec::new();
        loop {
            let mut at: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
            for (i, e) in self.edges.iter().enumerate() {
                at[e.a].push(i);
                if e.b != e.a {
                    at[e.b].push(i);
                }
            }
            let mut merged = false;
            for (v, edges_at_v) in at.iter().enumerate() {
                if edges_at_v.len() != 2 || !area.contains(self.nodes[v]) || rejected.contains(&v) {
                    continue;
                }
                let (i, j) = (edges_at_v[0], edges_at_v[1]);
                // i は v で終わり、j は v から始まる向きにそろえる
                let (e1, e2) = (self.oriented(i, false, v), self.oriented(j, true, v));
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
                let fits = [&q1, &q2].iter().all(|qq| {
                    (1..4).all(|k| nearest_on_quad(&q, qq.eval(k as f64 / 4.0)).1 < 0.01)
                }) && nearest_on_quad(&q, self.nodes[v]).1 < 0.01
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
                if self.crosses_others(&q, e1.a, e2.b, [i, j]) {
                    rejected.push(v);
                    continue;
                }
                self.edges[i] = MapEdge {
                    a: e1.a,
                    b: e2.b,
                    ctrl: q.p1,
                    ..e1
                };
                self.edges.swap_remove(j);
                merged = true;
                break;
            }
            if !merged {
                break;
            }
        }
    }

    /// 辺 `i` を、`start` なら頂点 `v` から始まる向き、そうでなければ `v` で終わる向きにした写し
    fn oriented(&self, i: usize, start: bool, v: usize) -> MapEdge {
        let e = self.edges[i];
        let ok = if start { e.a == v } else { e.b == v };
        if ok {
            e
        } else {
            MapEdge {
                a: e.b,
                b: e.a,
                ctrl: e.ctrl,
                left: e.right,
                right: e.left,
                left_src: e.right_src,
                right_src: e.left_src,
            }
        }
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
    let maps = by_layer
        .into_iter()
        .map(|(layer_id, first_idx, shapes)| LayerMap {
            layer_id,
            group_path: scope.clone(),
            map: Arc::new(PlanarMap::from_faces(shapes)),
            first_idx,
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
            self.oriented(at[0], false, v),
            self.oriented(at[1], true, v),
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
        if !m.crossings(&[ne]).is_empty() {
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
        for (before, after, color) in [
            ((at[0], forward), (ne, true), e1.left),
            ((at[0], !forward), (ne, false), e1.right),
        ] {
            if color.is_none() {
                continue;
            }
            let area = |map: &PlanarMap, h: (usize, bool)| {
                Contour::new(
                    map.cycle_from(h)
                        .iter()
                        .map(|x| map.half_quad(*x))
                        .collect(),
                )
                .signed_area()
            };
            let (a0, a1) = (area(self, before), area(&m, after));
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
    cycles: Vec<Vec<(usize, bool)>>,
    keys: HashSet<[u64; 8]>,
}

impl ReshapeBase {
    pub(crate) fn new(map: Arc<PlanarMap>) -> Self {
        let cycles = map.face_cycles();
        let keys = (0..map.edges.len()).map(|e| edge_key(&map, e)).collect();
        Self { map, cycles, keys }
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
    // 編集した地図と、編集で変わる元の辺（アンカーの追加は切る辺、削除は消す頂点の2本、ドラッグは動かす辺）
    let (next, touched) = match edit {
        MapEdit::AddAnchor { orig_seg, t } => {
            let (e, forward) = map.find_edge(&orig_seg)?;
            (
                map.with_anchor_added(e, if forward { t } else { 1.0 - t }),
                vec![e],
            )
        }
        MapEdit::DeleteAnchor { orig_vertex } => {
            let v = (0..map.nodes.len()).find(|&v| map.nodes[v] == orig_vertex)?;
            (map.with_anchor_deleted(v)?, map.edges_at(v))
        }
        MapEdit::PullEdge { .. } | MapEdit::MoveVertex { .. } => {
            let (edited, moved) = map.apply_edit(edit)?;
            (commit_edit(map, &base.cycles, edited, &moved).0, moved)
        }
    };
    // 変わる辺が、元の地図の壊れた面の輪郭（閉じない・色がまざる）に入っていれば編集しない。
    // 壊れた輪郭の面は図形に書き戻せないので、編集するとその面の図形が消えてしまう
    let broken = |c: &Vec<(usize, bool)>| {
        c.iter().any(|h| h.0 == usize::MAX)
            || c.windows(2)
                .any(|w| map.side_color(w[0]) != map.side_color(w[1]))
    };
    if base
        .cycles
        .iter()
        .any(|c| broken(c) && c.iter().any(|h| touched.contains(&h.0)))
    {
        return None;
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
    let base = ReshapeBase::new(Arc::new(PlanarMap::from_faces(
        shapes
            .iter()
            .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path)),
    )));
    reshape_result(&base, edit, shapes, layer_id, group_path)
        .filter(|(removed, added)| !removed.is_empty() || !added.is_empty())
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
        .unwrap_or_else(|| {
            crate::vector_boolean::raw_scope_insert_index(shapes, layer_id, group_path)
        })
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
    let map = PlanarMap::from_faces(own.iter());
    let rect_map = PlanarMap::from_faces(std::iter::once(rect));
    let inside = map.overlay(MapInput::Map(&rect_map), MapRule::KeepInside)?;
    let rest = map.overlay(MapInput::Map(&rect_map), MapRule::Paint(None))?;
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
    let piece_map = PlanarMap::from_faces(pieces.iter());
    let all: Vec<usize> = (0..piece_map.edges.len()).collect();
    if piece_map.cycle_check() != (0, 0)
        || piece_map.faces().1 != 0
        || !piece_map
            .crossings_grid(&all, &vec![false; all.len()])
            .is_empty()
    {
        // 片どうしが重なっている・境目がそろっていない（地図にならない）
        return MapEditOutcome::Refused;
    }
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
        return MapEditOutcome::Changed(
            Vec::new(),
            pieces
                .iter()
                .map(|p| p.clone().with_scope(layer_id, group_path))
                .collect(),
        );
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
        if broken > 0
            || map.cycle_check() != (0, 0)
            || !map.crossings_grid(&all, &vec![false; all.len()]).is_empty()
        {
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
    /// 地図では編集できない（重ねた結果が安全網で止まった・壊れた輪郭の面に触れる）
    Refused,
}

/// 図形の並び `shapes` のうち、レイヤー `layer_id`・階層 `group_path` の生の図形を地図にして、`edit` で編集した差分。
/// 元の地図の壊れた輪郭（閉じない・色がまざる）の辺を変える編集は `Refused`（壊れた輪郭の面は図形に書き戻せず、消えてしまうため）
pub(crate) fn map_edit_diff(
    shapes: &[FillShape],
    layer_id: LayerId,
    group_path: &[u64],
    edit: impl FnOnce(&PlanarMap) -> Option<PlanarMap>,
) -> MapEditOutcome {
    let map = PlanarMap::from_faces(
        shapes
            .iter()
            .filter(|s| s.layer_id == layer_id && is_map_shape(s, group_path)),
    );
    let Some(next) = edit(&map) else {
        return MapEditOutcome::Refused;
    };
    let keys: HashSet<[u64; 8]> = (0..map.edges.len()).map(|e| edge_key(&map, e)).collect();
    let next_keys: HashSet<[u64; 8]> = (0..next.edges.len()).map(|e| edge_key(&next, e)).collect();
    let broken_touched = map.face_cycles().iter().any(|c| {
        let broken = c.iter().any(|h| h.0 == usize::MAX)
            || c.windows(2)
                .any(|w| map.side_color(w[0]) != map.side_color(w[1]));
        broken
            && c.iter()
                .any(|h| h.0 != usize::MAX && !next_keys.contains(&edge_key(&map, h.0)))
    });
    if broken_touched {
        return MapEditOutcome::Refused;
    }
    let (removed, added) = rewrite_shapes(&map, &keys, &next, shapes, layer_id, group_path);
    if removed.is_empty() && added.is_empty() {
        MapEditOutcome::Unchanged
    } else {
        MapEditOutcome::Changed(removed, added)
    }
}

// ============================================================================
// 形・地図を地図に重ねる（ブラシ・消しゴム・矩形で選んだ片の取り出し・持ち上げた片を置く・貼り付け）
//
// 重ねる線と前からある辺を細かい折れ線にして、i_overlay（vendor/i_overlay）の `EdgeOverlay` で交わる所で切る。
// 交わりは整数の格子（1/4096px）で計算するので、ほぼ重なった線でも食い違わない。切った区間ごとに、
// 左右が重ねる形の中か（i_overlay が数える）と、出どころ（どの曲線の t のどこからどこまでか）が分かるので、
// 面をたどったり穴の入れ先を決めたりせずに、区間の左右の色が決まる。境目の曲線は、出どころの曲線から当て直す。
// ============================================================================

/// 重ねるときの格子（1px をいくつに分けるか）。地図の頂点のうち重ねる所にかかるものは、この格子の上に置き直す
const OVERLAY_GRID: f64 = 4096.0;
/// 曲線を折れ線にするときの、曲線と折れ線の離れ具合の上限（px）
const FLAT_TOL: f64 = 0.002;

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
fn combine_sides(
    mut entries: Vec<((Option<Color32>, Option<Color32>), Point)>,
    mid: Point,
    dir: KVec2,
) -> Option<(Option<Color32>, Option<Color32>)> {
    if entries.len() <= 1 {
        return entries.pop().map(|e| e.0);
    }
    entries.sort_by(|x, y| dir.cross(y.1 - mid).total_cmp(&dir.cross(x.1 - mid)));
    Some((entries[0].0 .0, entries[entries.len() - 1].0 .1))
}

impl PlanarMap {
    /// 点 `p` の色（`color_regions` の結果から）
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
    /// どの輪郭にも囲まれていない所・すでに `color` の面は `None`
    pub(crate) fn with_face_filled(&self, p: Point, color: Color32) -> Option<PlanarMap> {
        // 閉じない輪郭（壊れた所）は使わない
        let cycles: Vec<Vec<(usize, bool)>> = self
            .face_cycles()
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
        let ext = owner_of(p)?;
        // その外周の中にある穴（面積が負）のうち、いちばん近い外周がこの外周のもの
        let mut face: Vec<usize> = vec![ext];
        for k in 0..cycles.len() {
            if info[k].0 < 0.0 {
                // 穴の輪郭の上の点は、どの外周に入るかがあいまいなので、穴を少し内側（左側）から見る
                let q = self.half_quad(cycles[k][0]);
                let tan = quad_deriv(&q, 0.5);
                let len = tan.hypot();
                if len < 1e-12 {
                    continue;
                }
                let probe = q.eval(0.5) + KVec2::new(-tan.y, tan.x) / len * 1e-4;
                if owner_of(probe) == Some(ext) {
                    face.push(k);
                }
            }
        }
        if self.side_color(cycles[ext][0]) == Some(color) {
            return None;
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

    /// 形か地図 `input` を、`rule` の決め方で重ねた地図。結果が壊れていれば（安全網）`None`。
    ///
    /// 1. 重ねるものの外接矩形にかかる辺と、重ねる線を、細かい折れ線にして格子に置き、i_overlay で交わる所で切る。
    ///    前からある辺は Subject、重ねる線は Clip（偶奇の規則。重ねる形は重なりの無い形にしておくこと）
    /// 2. 前からある頂点のうち、切る辺の端にあたるものは、格子の上に置き直す（i_overlay が見た位置と同じにする）
    /// 3. 切った区間ごとに、左右の前の色（出どころの辺の色。重ねる線だけの区間は、その側の面の輪郭をたどって
    ///    最初に出会う前からある辺の色）と、重ねるものの中か・重ねる地図の色を決め、`rule` で新しい色にする
    /// 4. 左右が同じ色の区間は捨てる。同じ曲線から来て t が続き、色が同じ区間を1本につなぎ、元の曲線から当て直す
    /// 5. 当て直した曲線がほかの辺と交わるとき・面の輪郭の色がそろわないときは、その辺を折れ線のまま（短い直線の辺）にする
    pub(crate) fn overlay(&self, input: MapInput<'_>, rule: MapRule) -> Option<PlanarMap> {
        use i_overlay9::core::edge_overlay::{EdgeOverlay, InputEdge};
        use i_overlay9::core::fill_rule::FillRule as IFill;
        use i_overlay9::core::overlay::ShapeType;
        use i_overlay9::i_float::int::point::IntPoint;
        use i_overlay9::vector::edge::{CLIP_LEFT, CLIP_RIGHT};
        // 外が前のままのときだけ、重ねるものから離れた辺をそのまま残す
        let copy_untouched = rule != MapRule::KeepInside;

        // 0. 出どころの曲線：重ねるものの外接矩形にかかる前からある辺と、重ねる線
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
        let area = new_quads
            .iter()
            .map(|q| q.bounding_box())
            .reduce(|a, b| a.union(b))?
            .inflate(1.0, 1.0);
        let mut srcs: Vec<(QuadBez, OverlaySrc)> = Vec::new();
        let mut touched = vec![false; self.edges.len()];
        for e in 0..self.edges.len() {
            let q = self.quad(e);
            if rects_overlap(q.bounding_box(), area) {
                touched[e] = true;
                srcs.push((q, OverlaySrc::Old(e)));
            }
        }
        for (i, q) in new_quads.iter().enumerate() {
            srcs.push((*q, OverlaySrc::New(i)));
        }

        // 1. 折れ線にして格子に置き、i_overlay で切る。格子は原点から（何回重ねても、格子の上の点は動かない）
        let limit = (1i64 << 28) as f64;
        let to_int = |p: Point| -> Option<IntPoint<i32>> {
            let x = (p.x * OVERLAY_GRID).round();
            let y = (p.y * OVERLAY_GRID).round();
            (x.abs() < limit && y.abs() < limit).then(|| IntPoint::new(x as i32, y as i32))
        };
        let to_pt = |x: i64, y: i64| Point::new(x as f64 / OVERLAY_GRID, y as f64 / OVERLAY_GRID);
        let mut ov: EdgeOverlay<i32, ProvId> = EdgeOverlay::new(srcs.len() * 8);
        for (si, (q, kind)) in srcs.iter().enumerate() {
            let old = matches!(kind, OverlaySrc::Old(_));
            let st = if old {
                ShapeType::Subject
            } else {
                ShapeType::Clip
            };
            let pts = flatten_with_t(q, FLAT_TOL);
            let mut prev = (to_int(pts[0].0)?, pts[0].1);
            for &(p, t) in &pts[1..] {
                let ip = to_int(p)?;
                if ip == prev.0 {
                    continue;
                }
                // 前からある辺の区間は、いつも小さい点から大きい点の向きで入れる（ほぼ重なった2本の辺が、
                // 格子で同じ区間になったときに打ち消し合って消えないように。Subject の塗りは使わない）
                let (a, b, t0, t1) = if old && ip < prev.0 {
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

        // 2. 頂点：前からある頂点（格子に丸めた所が同じ）は、その番号のまま格子の上に置く。新しい頂点は後ろに足す
        let mut nodes: Vec<Point> = self.nodes.clone();
        let mut node_of: HashMap<(i32, i32), usize> = HashMap::new();
        for (e, ed) in self.edges.iter().enumerate() {
            if touched[e] {
                for v in [ed.a, ed.b] {
                    let ip = to_int(self.nodes[v])?;
                    node_of.insert((ip.x, ip.y), v);
                    nodes[v] = to_pt(ip.x as i64, ip.y as i64);
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
                    nodes.push(to_pt(k.0 as i64, k.1 as i64));
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
        if pieces.is_empty() {
            return None;
        }

        // 3. 頂点ごとに、出ていく区間を角度の順に並べる：（区間, 区間の向きに出ていくか）
        let mut around: HashMap<usize, Vec<(usize, bool)>> = HashMap::new();
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
        // 向きのある区間の左の面を、次にたどる区間（角度の順で、来た向きの逆のすぐ前）
        let next = |h: (usize, bool)| -> (usize, bool) {
            let v = if h.1 { pieces[h.0].b } else { pieces[h.0].a };
            let list = &around[&v];
            let i = list.iter().position(|x| *x == (h.0, !h.1)).unwrap_or(0);
            list[(i + list.len() - 1) % list.len()]
        };

        // 4. 区間の左右の色の材料
        let side_of = |p: &Piece, want_old: bool| -> Option<(Option<Color32>, Option<Color32>)> {
            let (a, b) = (to_pt(p.ia.0, p.ia.1), to_pt(p.ib.0, p.ib.1));
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
        // 向きのある区間 `h` の左の色：`of` に色があればそれ。無ければ、左の面の輪郭をたどって最初に出会う、色のある区間の
        // 面の側の色（色の無い区間は、その地図の面を分けないので、越えてよい）。出会わなければ `fallback` で調べる
        let walk = |h: (usize, bool),
                    of: &[Option<(Option<Color32>, Option<Color32>)>],
                    memo: &mut HashMap<(usize, bool), Option<Color32>>,
                    fallback: &mut dyn FnMut(Point) -> Option<Color32>|
         -> Option<Color32> {
            if let Some((l, r)) = of[h.0] {
                return if h.1 { l } else { r };
            }
            if let Some(&c) = memo.get(&h) {
                return c;
            }
            let mut path = vec![h];
            let mut cur = h;
            let found = loop {
                cur = next(cur);
                if let Some((l, r)) = of[cur.0] {
                    break Some(if cur.1 { l } else { r });
                }
                if cur == h || path.len() > pieces.len() {
                    break None;
                }
                path.push(cur);
            };
            let c = found.unwrap_or_else(|| {
                let p = &pieces[h.0];
                fallback(nodes[p.a].midpoint(nodes[p.b]))
            });
            for x in path {
                memo.insert(x, c);
            }
            c
        };
        let mut old_regions: Option<Vec<(Color32, BezPath)>> = None;
        let mut new_regions: Option<Vec<(Color32, BezPath)>> = None;
        let mut old_fallback =
            |q: Point| Self::color_in(old_regions.get_or_insert_with(|| self.color_regions()), q);
        let mut new_fallback = |q: Point| match input {
            MapInput::Map(pm) => {
                Self::color_in(new_regions.get_or_insert_with(|| pm.color_regions()), q)
            }
            MapInput::Shapes(_) => None,
        };
        let (mut old_memo, mut new_memo) = (HashMap::new(), HashMap::new());

        // 5. 区間の新しい色。左右が違う区間だけ残す：（始点, 終点, 左の色, 右の色, 区間の番号）
        let mut kept: Vec<(usize, usize, Option<Color32>, Option<Color32>, usize)> = Vec::new();
        for k in 0..pieces.len() {
            let mut side = |fwd: bool| {
                let old = walk((k, fwd), &old_of, &mut old_memo, &mut old_fallback);
                let (inside, p) = match input {
                    MapInput::Shapes(_) => {
                        let f = pieces[k].fill;
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
                        let c = walk((k, fwd), &new_of, &mut new_memo, &mut new_fallback);
                        (c.is_some(), c)
                    }
                };
                rule.apply(inside, p, old)
            };
            let (left, right) = (side(true), side(false));
            if left != right {
                kept.push((pieces[k].a, pieces[k].b, left, right, k));
            }
        }

        // 6. 区間をつないで辺にする。頂点の次数が 2 で、出どころと左右の色が同じで、t が続いていれば、つなげる
        let mut at: HashMap<usize, Vec<usize>> = HashMap::new();
        for (ki, &(a, b, ..)) in kept.iter().enumerate() {
            at.entry(a).or_default().push(ki);
            at.entry(b).or_default().push(ki);
        }
        let mut untouched_deg: HashMap<usize, usize> = HashMap::new();
        if copy_untouched {
            for (e, ed) in self.edges.iter().enumerate() {
                if !touched[e] {
                    *untouched_deg.entry(ed.a).or_insert(0) += 1;
                    *untouched_deg.entry(ed.b).or_insert(0) += 1;
                }
            }
        }
        // 区間の主な出どころ（前からある辺を先に）。区間の向き（a→b）での t
        let main_src = |ki: usize| -> Prov {
            let pv = &pieces[kept[ki].4].prov;
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
        // 重ねるものから離れた辺は、そのまま（元の図形の印も残す）
        let mut edges: Vec<MapEdge> = if copy_untouched {
            (0..self.edges.len())
                .filter(|&e| !touched[e])
                .map(|e| self.edges[e])
                .collect()
        } else {
            Vec::new()
        };
        let n_untouched = edges.len();
        let mut used = vec![false; kept.len()];
        // 当て直した辺（辺の番号 → 折れ線の区間）と、そのうち前からある辺が丸ごと元の曲線のまま残ったもの
        let mut restored: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        let mut whole_old: HashSet<usize> = HashSet::new();
        for start in 0..kept.len() {
            if used[start] {
                continue;
            }
            let mut chain: std::collections::VecDeque<(usize, bool)> =
                std::collections::VecDeque::from([(start, true)]);
            used[start] = true;
            let (_, _, sl, sr, _) = kept[start];
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
                    let (na, nb, nl, nr, _) = kept[nk];
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
            let (n0, _, t0, _) = piece_ends(chain.front().unwrap());
            let (_, n1, _, t1) = piece_ends(chain.back().unwrap());
            let (q, kind) = srcs[s_src as usize];
            // 前からある辺が丸ごと残ったときは、元の曲線と元の図形の印（色が変わらない側だけ）をそのまま使う
            if let OverlaySrc::Old(e) = kind {
                let ed = self.edges[e];
                let whole = t0.min(t1) < 1e-9 && t0.max(t1) > 1.0 - 1e-9;
                let same_dir = (n0, n1) == (ed.a, ed.b);
                if whole && n0 != n1 && (same_dir || (n0, n1) == (ed.b, ed.a)) {
                    let (ol, or, osl, osr) = if same_dir {
                        (ed.left, ed.right, ed.left_src, ed.right_src)
                    } else {
                        (ed.right, ed.left, ed.right_src, ed.left_src)
                    };
                    whole_old.insert(edges.len());
                    restored.insert(
                        edges.len(),
                        chain
                            .iter()
                            .map(|c| {
                                let (a, b, ..) = piece_ends(c);
                                (a, b)
                            })
                            .collect(),
                    );
                    edges.push(MapEdge {
                        a: n0,
                        b: n1,
                        ctrl: ed.ctrl,
                        left: sl,
                        right: sr,
                        left_src: if sl == ol { osl } else { None },
                        right_src: if sr == or { osr } else { None },
                    });
                    continue;
                }
            }
            let ctrl = (n0 != n1 && (t1 - t0).abs() > 1e-12)
                .then(|| {
                    let sub = if t0 < t1 {
                        q.subsegment(t0..t1)
                    } else {
                        let r = q.subsegment(t1..t0);
                        QuadBez::new(r.p2, r.p1, r.p0)
                    };
                    QuadBez::new(nodes[n0], sub.p1, nodes[n1])
                })
                .filter(|sub| {
                    // 鎖の折れ線が、当て直した曲線から離れていないか
                    chain.iter().all(|c| {
                        let (a, b, ..) = piece_ends(c);
                        nearest_on_quad(sub, nodes[a].midpoint(nodes[b])).1 <= 4.0 * FLAT_TOL
                    })
                })
                .map(|sub| sub.p1);
            match ctrl {
                Some(ctrl) => {
                    restored.insert(
                        edges.len(),
                        chain
                            .iter()
                            .map(|c| {
                                let (a, b, ..) = piece_ends(c);
                                (a, b)
                            })
                            .collect(),
                    );
                    edges.push(MapEdge {
                        a: n0,
                        b: n1,
                        ctrl,
                        left: sl,
                        right: sr,
                        left_src: None,
                        right_src: None,
                    });
                }
                None => {
                    for c in &chain {
                        let (a, b, ..) = piece_ends(c);
                        edges.push(MapEdge {
                            a,
                            b,
                            ctrl: nodes[a].midpoint(nodes[b]),
                            left: sl,
                            right: sr,
                            left_src: None,
                            right_src: None,
                        });
                    }
                }
            }
        }
        let mut m = PlanarMap {
            node_src: vec![None; nodes.len()],
            nodes,
            edges,
        };

        // 7. 当て直した曲線は、折れ線から最大 4 × FLAT_TOL ずれるので、ほかの辺と交わったり、細い面で左右が入れ替わったり
        //    することがある。そうなった辺は折れ線のまま（短い直線の辺）にする。折れ線どうしは交わらない（i_overlay の切り分けそのもの）
        let mut at_untouched = vec![false; m.nodes.len()];
        for ed in &m.edges[..n_untouched] {
            at_untouched[ed.a] = true;
            at_untouched[ed.b] = true;
        }
        // 交わりうるのは、当て直した辺と、前のままの辺に頂点でつながる新しい辺
        let mut near_untouched: Vec<usize> = (n_untouched..m.edges.len())
            .filter(|&i| at_untouched[m.edges[i].a] || at_untouched[m.edges[i].b])
            .collect();
        let to_straight = |m: &mut PlanarMap,
                           e: usize,
                           restored: &mut HashMap<usize, Vec<(usize, usize)>>,
                           near_untouched: &mut Vec<usize>| {
            let Some(segs) = restored.remove(&e) else {
                return;
            };
            let (l, r) = (m.edges[e].left, m.edges[e].right);
            for (j, &(a, b)) in segs.iter().enumerate() {
                let ed = MapEdge {
                    a,
                    b,
                    ctrl: m.nodes[a].midpoint(m.nodes[b]),
                    left: l,
                    right: r,
                    left_src: None,
                    right_src: None,
                };
                let idx = if j == 0 {
                    m.edges[e] = ed;
                    e
                } else {
                    m.edges.push(ed);
                    m.edges.len() - 1
                };
                if at_untouched[a] || at_untouched[b] {
                    near_untouched.push(idx);
                }
            }
        };
        let check_list = |restored: &HashMap<usize, Vec<(usize, usize)>>,
                          near_untouched: &[usize],
                          len: usize| {
            let orig: Vec<bool> = (0..len)
                .map(|e| e < n_untouched || (whole_old.contains(&e) && restored.contains_key(&e)))
                .collect();
            let mut check: Vec<usize> = restored
                .keys()
                .copied()
                .chain(near_untouched.iter().copied())
                .collect();
            check.sort_unstable();
            check.dedup();
            (check, orig)
        };
        for _ in 0..4 {
            // 交わり：交わった当て直しの辺を折れ線にして、交わりが無くなるまで
            for _ in 0..8 {
                let (check, orig) = check_list(&restored, &near_untouched, m.edges.len());
                let hits = m.crossings_grid(&check, &orig);
                let mut hit: Vec<usize> = hits
                    .iter()
                    .flat_map(|&(e, _, f, _)| [e, f])
                    .filter(|e| restored.contains_key(e))
                    .collect();
                hit.sort_unstable();
                hit.dedup();
                if hit.is_empty() {
                    break;
                }
                for e in hit {
                    to_straight(&mut m, e, &mut restored, &mut near_untouched);
                }
            }
            // 面の輪郭の色
            let mut hit: Vec<usize> = Vec::new();
            for c in m.face_cycles() {
                let bad = c.iter().any(|h| h.0 == usize::MAX)
                    || c.windows(2)
                        .any(|w| m.side_color(w[0]) != m.side_color(w[1]));
                if bad {
                    hit.extend(
                        c.iter()
                            .filter(|h| h.0 != usize::MAX && restored.contains_key(&h.0))
                            .map(|h| h.0),
                    );
                }
            }
            hit.sort_unstable();
            hit.dedup();
            if hit.is_empty() {
                break;
            }
            for e in hit {
                to_straight(&mut m, e, &mut restored, &mut near_untouched);
            }
        }
        // 安全網：交わりが残っていないこと
        let (check, orig) = check_list(&restored, &near_untouched, m.edges.len());
        if !m.crossings_grid(&check, &orig).is_empty() {
            return None;
        }
        m.compact_nodes();
        // 安全網：面の輪郭が閉じて色がそろい、面が取り出せること（元の地図にもともとある壊れた所は数に入れる）
        let (open, mixed) = m.cycle_check();
        let broken = m.faces().1;
        if open + mixed + broken > 0 {
            let (o_open, o_mixed) = self.cycle_check();
            if open > o_open || mixed > o_mixed || broken > self.faces().1 {
                return None;
            }
        }
        Some(m)
    }
}
