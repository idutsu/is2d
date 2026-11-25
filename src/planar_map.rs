//! 同じレイヤー・同じ階層の生の図形を、境目の線（辺）で持つ地図。セル画の中身そのもので、描く・消す・塗る・選ぶ・
//! 動かす・貼る・輪郭の変形など、生の図形を変える編集はみな、この地図を受け取って地図を返す。
//! Adobe Animate のマージ描画と同じく、辺が「曲線・左の色・右の色」を持つ。境目は1本で、重ね順は無い。
//! 塗りの形（`FillShape`）は、辺から作って描画・当たり判定に使う。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use eframe::egui::Color32;
use kurbo::{Affine, ParamCurve, ParamCurveArclen, Point, QuadBez, Rect as KRect, Shape as KurboShape};

use crate::stored_map::StoredMap;
use crate::vector_engine::{nearest_on_quad, rects_overlap, Contour, FillShape, SyncCache};
use crate::vector_engine::{LayerId, ShapeId};

mod grid;
// 前の地図＋差分として触った所だけを計算する道具（段2b）
mod local;
mod overlay;
pub(crate) use self::grid::*;
pub(crate) use self::local::*;
pub(crate) use self::overlay::*;

/// 辺の（左の色, 右の色）
type Sides = (Option<Color32>, Option<Color32>);

/// 面の印：色と、その面の図形の番号（重ね合わせで、色と一緒に伝える）
type Label = (Option<Color32>, Option<ShapeId>);

/// 線の太さの範囲（px。整数だけ）
pub(crate) const STROKE_WIDTHS: std::ops::RangeInclusive<u8> = 1..=10;

/// 点が辺に合う距離：辺の色のある側（`filled`）と何も無い側（`empty`）で別に持つ（色のある側は、輪郭の近くでも塗りを
/// つかめるように狭い）
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EdgeGrab {
    pub(crate) filled: f64,
    pub(crate) empty: f64,
}

impl EdgeGrab {
    /// 辺に合う距離の決まり（画面の px）：選択ツールで辺が光る・辺を選ぶ距離と、インクボトルで辺1本に線を乗せる距離
    pub(crate) const SCREEN_PX: EdgeGrab = EdgeGrab { filled: 5.5, empty: 9.5 };

    /// 長さを `k` 倍したもの（画面の px を、置いた座標や文書の座標の長さにする）
    pub(crate) fn scaled(self, k: f64) -> EdgeGrab {
        EdgeGrab { filled: self.filled * k, empty: self.empty * k }
    }

    /// 点のある側に色がある（`filled`）ときの、辺に合う距離
    pub(crate) fn at(self, filled: bool) -> f64 {
        if filled {
            self.filled
        } else {
            self.empty
        }
    }
}

/// 辺に乗せた線：色と太さ（px。`STROKE_WIDTHS` の内）。端と角は丸だけ（設計：claude/線の設計_68a596e.md の 5章）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EdgeStroke {
    pub(crate) color: Color32,
    pub(crate) width: u8,
}

/// 辺。a → b に進んだときの左（面積の式で正の側）の色と、右の色を持つ。`None` は何も無い所。
/// `left_src`・`right_src` は、その側の面の図形の番号（セル画の地図は面ごとに1つずつ別の番号を記してしまう：
/// `CheckedMap::uniquely_labeled`。重ね合わせでは、どの図形から来たかを運ぶ）。`stroke` は辺に乗せた線（無ければ `None`。
/// 線は向きを持たないので、左右を入れ替えても変わらない）
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MapEdge {
    pub(crate) a: usize,
    pub(crate) b: usize,
    pub(crate) ctrl: Point,
    pub(crate) left: Option<Color32>,
    pub(crate) right: Option<Color32>,
    pub(crate) left_src: Option<ShapeId>,
    pub(crate) right_src: Option<ShapeId>,
    pub(crate) stroke: Option<EdgeStroke>,
}

/// 辺を残すか（辺を残す決まりはこれ1つ）：左右の色が違うか、線が付いている（設計：claude/線の設計_68a596e.md の 6.2 の (a)）
pub(crate) fn edge_needed(left: Option<Color32>, right: Option<Color32>, stroke: Option<EdgeStroke>) -> bool {
    left != right || stroke.is_some()
}

impl MapEdge {
    /// この辺を地図に残すか（`edge_needed`）
    pub(crate) fn needed(&self) -> bool {
        edge_needed(self.left, self.right, self.stroke)
    }

    /// つながった2本（向きをそろえたもの）を1本の辺にまとめてよい中身か：左右の色と線が同じ（2本のまとめ `merge_degree2`
    /// とアンカーの削除 `with_anchor_deleted` の決まりはこれ1つ）
    fn same_content(&self, next: &MapEdge) -> bool {
        self.left == next.left && self.right == next.right && self.stroke == next.stroke
    }

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
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MapFace {
    pub(crate) color: Color32,
    pub(crate) cycles: Vec<Cycle>,
    pub(crate) srcs: Vec<ShapeId>,
}

/// 整数（頂点・辺・格子の番号、座標のビット）を鍵にする表のハッシュ。標準の SipHash は、頂点の格子のように何十万回も
/// 引くと重いので、掛け算1回で混ぜる軽いものを使う（鍵は外から来ない値なので、偏りを狙われる心配は無い）
#[derive(Default, Clone, Copy)]
pub(crate) struct IntHasher(u64);

impl std::hash::Hasher for IntHasher {
    /// 掛け算は下の桁を上の桁へしか混ぜない（下の桁が 0 の多い座標のビットだと、表の位置を決める下の桁がそろう）ので、
    /// よく混ざった上の桁を下へ回す。混ぜ方（`write_u64` の回す量 5 と掛ける数）は rustc-hash 1 の `FxHasher`、ここで回す
    /// 量 26 は rustc-hash 2 の `FxHasher::finish`（同じ理由で `rotate_left(26)` する）と同じ
    #[inline]
    fn finish(&self) -> u64 {
        self.0.rotate_left(26)
    }
    /// 整数の配列の鍵（辺の鍵 `[u64; 6]`、頂点の座標のビット `[u64; 2]` など）は、まとめてバイトで来るので、8 バイトずつ
    /// 混ぜる
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            let mut word = [0u8; 8];
            word.copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(word));
        }
        for &b in chunks.remainder() {
            self.write_u64(b as u64);
        }
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64);
    }
    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.write_u64(i as u32 as u64);
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }
}

/// 整数を鍵にする表（`IntHasher`）
pub(crate) type IntMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<IntHasher>>;

/// 整数を鍵にする集まり（`IntHasher`）
pub(crate) type IntSet<K> = HashSet<K, std::hash::BuildHasherDefault<IntHasher>>;

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

    /// 面の番号（辺の側に記したもの）
    #[cfg(test)]
    pub(crate) fn labels(&self) -> impl Iterator<Item = ShapeId> + '_ {
        self.edges
            .iter()
            .flat_map(|e| [(e.left, e.left_src), (e.right, e.right_src)])
            .filter_map(|(c, src)| c.and(src))
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

    /// 面の番号と線の番号が `keep` に合う面と線だけを残した地図（`restricted`。辺の決まりは `restricted_sides`）
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
            let Some(([l, r], stroke)) = restricted_sides(
                [(e.left, e.left_src), (e.right, e.right_src)],
                e.stroke,
                line_id_of(&tile_edge(self, e)),
                &keep,
            ) else {
                continue;
            };
            edges.push(MapEdge {
                a: node(e.a),
                b: node(e.b),
                ctrl: e.ctrl,
                left: l.0,
                right: r.0,
                left_src: l.1,
                right_src: r.1,
                stroke,
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
                stroke: None,
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

    /// テスト用：正しい地図なら（`is_valid`）、その面（`faces`）。判定は本番と同じ `check`（読み込み・変換した地図の判定は
    /// どこでもこれ1つ）。`None`：正しい地図でない
    #[cfg(test)]
    pub(crate) fn valid_faces(&self) -> Option<Vec<MapFace>> {
        self.check()
    }

    /// 正しい地図かの判定（`checked`・`valid_faces`）。正しい地図なら、その面
    fn check(&self) -> Option<Vec<MapFace>> {
        self.check_with(|| self.polys(), |polys| self.face_cycles_with(polys))
    }

    /// 正しい地図かの判定の本体（`check`・読んだ地図の確かめ `LayerBase::checked_stored`）。判定の形を `polys` で、面の
    /// 輪郭を `cycles` で得る（どちらも、要ると分かってから呼ぶ）。正しい地図なら、その面
    fn check_with<P: std::ops::Deref<Target = [Poly]>, C: std::ops::Deref<Target = [Cycle]>>(
        &self,
        polys: impl FnOnce() -> P,
        cycles: impl FnOnce(&[Poly]) -> C,
    ) -> Option<Vec<MapFace>> {
        #[cfg(test)]
        FULL_CHECKS.with(|n| n.set(n.get() + 1));
        // 頂点と制御点が格子に置ける（±65,536px の内で有限：`fits_grid`）こと、両側が同じ色の辺が無いことを先に確かめる
        // （格子の外の座標で、判定の形を作らない）
        if !self.fits_grid(Affine::IDENTITY) || self.edges.iter().any(|e| !e.needed()) {
            return None;
        }
        let polys = polys();
        if !self.polys_apart(&polys) {
            return None;
        }
        let cycles = cycles(&polys);
        let regions = self.regions(&polys, &cycles);
        if regions.broken > 0 {
            return None;
        }
        // 何も無い側の輪郭：その最初の区間の真ん中の点が、その辺を輪郭に持たない色のある面の中（外周の中で、どの穴の外：
        // `Regions::contains`）にあるか、その面の輪郭の上にあれば、入り込んでいる
        let to_check: Vec<(usize, IPt)> = cycles
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_empty() && self.side_color(c[0]).is_none())
            .map(|(ci, c)| (ci, half_mid2(&polys, c[0])))
            .collect();
        if !to_check.is_empty() {
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
            for (ci, probe) in to_check {
                let h = cycles[ci][0];
                for g in regions.groups_at(probe) {
                    if !edge_faces[h.0].contains(&g) && regions.contains(g, probe) != Some(false) {
                        return None;
                    }
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
    fn polys(&self) -> Vec<Poly> {
        (0..self.edges.len())
            .map(|e| EdgeGeom::of(&self.quad(e)))
            .collect()
    }

    /// 判定の形の 1〜3（設計 2-2）：折れ線どうしが共有する頂点のほかで触れない・1本の折れ線が自分と触れない・
    /// 別々の頂点が別々の格子の点に丸まる（全部の区間どうしを確かめる）
    fn polys_apart(&self, polys: &[Poly]) -> bool {
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
    fn segs_clash(&self, polys: &[Poly], (e, k): (usize, usize), (f, m): (usize, usize)) -> bool {
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
    fn faces_of(&self, polys: &[Poly], cycles: &[Vec<(usize, bool)>]) -> (Vec<MapFace>, usize) {
        let regions = self.regions(polys, cycles);
        let broken = regions.broken;
        (regions.into_faces(self), broken)
    }

    /// 面の輪郭 `cycles`（`face_cycles_with`。判定の形 `polys`）を、外周（面積が正）と、その中の穴（面積が負）の組に分ける。
    /// 穴は、それを含む同じ色（色の無い所も1つの色とみなす）の外周のうち、いちばん小さいものに入れる。外周か穴かは判定の
    /// 形の面積の符号、穴の入れ先は判定の形での点の内外（`Ring::contains`）で、どちらも整数で決める。色の無い所の輪郭は、
    /// 組にしない（面にならない）。面の取り出しと正しい地図かの判定の、面の分け方はこれ1つ
    fn regions(&self, polys: &[Poly], cycles: &[Vec<(usize, bool)>]) -> Regions {
        let mut out = Regions {
            cycles: Vec::new(),
            rings: Vec::new(),
            groups: Vec::new(),
            broken: 0,
            bands: YBands::new(std::iter::empty()),
        };
        let mut holes: Vec<(Option<Color32>, usize)> = Vec::new();
        for cycle in cycles {
            if self.is_broken_cycle(cycle) {
                out.broken += 1;
                continue;
            }
            let color = self.side_color(cycle[0]);
            if color.is_none() {
                continue;
            }
            let k = out.rings.len();
            let ring = Arc::new(Ring::new(polys, cycle));
            if ring.is_exterior() {
                out.groups.push((color, vec![k]));
            } else {
                holes.push((color, k));
            }
            out.rings.push(ring);
            out.cycles.push(cycle.clone());
        }
        out.bands = YBands::new(out.groups.iter().map(|(_, ids)| {
            let r = out.rings[ids[0]].rect;
            (2 * r.y0, 2 * r.y1)
        }));
        // 穴を含む同じ色の外周のうち、いちばん小さいもの（同じなら組の順で前）。調べる点は、穴の最初の区間の真ん中（正しい
        // 地図では、ほかの輪郭の上に無い）
        for (color, k) in holes {
            let probe = half_mid2(polys, out.cycles[k][0]);
            let owner = out
                .groups_at(probe)
                .filter(|&g| {
                    out.groups[g].0 == color
                        && out.rings[out.groups[g].1[0]].contains(probe) == Some(true)
                })
                .min_by_key(|&g| out.rings[out.groups[g].1[0]].area2);
            match owner {
                Some(g) => out.groups[g].1.push(k),
                // 色のある穴に外周が無ければ壊れている
                None => out.broken += 1,
            }
        }
        out
    }

    /// 面 `f` の今の形を、塗りの形にする（`cycles_shape`）
    pub(crate) fn face_shape(&self, f: &MapFace, layer_id: u64, group_path: &[u64]) -> FillShape {
        let cycles = f
            .cycles
            .iter()
            .map(|c| c.iter().map(|&h| self.half_quad(h)).collect());
        cycles_shape(f.color, cycles, |_, _| None, layer_id, group_path)
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

    /// 頂点 `v` につながる辺（辺の番号の順）
    pub(crate) fn edges_at(&self, v: usize) -> Vec<usize> {
        (0..self.edges.len())
            .filter(|&e| self.edges[e].a == v || self.edges[e].b == v)
            .collect()
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
    fn face_cycles_with(&self, polys: &[Poly]) -> Vec<Cycle> {
        let idx = |h: (usize, bool)| 2 * h.0 + h.1 as usize;
        let dir = |h: (usize, bool)| -> IPt {
            let mut it = half_points(polys, h);
            let a = it.next().unwrap_or((0, 0));
            let b = it.next().unwrap_or(a);
            (b.0 - a.0, b.1 - a.1)
        };
        let mut out_of: Vec<Cycle> = vec![Vec::new(); self.nodes.len()];
        for (i, e) in self.edges.iter().enumerate() {
            out_of[e.a].push((i, true));
            out_of[e.b].push((i, false));
        }
        // 頂点ごとに、出ていく向きのある辺を角度の順に並べ、並びの中の位置を覚える
        let mut pos = vec![0usize; 2 * self.edges.len()];
        #[cfg(test)]
        VERTICES_SORTED.with(|n| n.set(n.get() + out_of.len()));
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
    /// 候補は、その操作で消えた線（左右が同じ色になって捨てた区間・辺）の端の頂点（`overlay`・面の塗り替え `color_sides`）：
    /// 操作が作った切れ目（交わりで切った点・消えた境目の端）だけを消し、操作の前から2本だけだった頂点（足した角）は
    /// どの操作でも候補にならないので残る。切れ目の後始末の決まりはこれ1つ。
    /// 頂点は位置の順（`point_order`）に調べ、まとめたら、つながりの変わった両端の頂点だけを調べ直す（まとめた結果は、
    /// まとめるたびに最初から調べ直すのと同じ）。辺の並びは元の番号の順のまま（まとめた2本は番号が先の辺に入れ、後ろの辺を
    /// 抜く）。番号の順でなく位置の順に調べ、辺の並びを抜くだけにするのは、同じ線の上に候補が続くとき、まとめる
    /// 順と残す辺で結果（曲線の数の最後の桁と、引き継ぐ面の番号）が変わるので、頂点の番号の振り方と辺の数が違う窓と地図
    /// 全体で同じ答えにするため（窓の辺は、地図全体の辺を番号の順のまま抜き出したもの。線の辺の行き止まりや、線の両側の
    /// 同じ色の面で、候補が続きやすくなった：設計 claude/線の設計_68a596e.md）。
    /// `built`：この地図の辺の判定の形の索引と、辺の番号 → 索引の番号（曲線に戻したあとの
    /// 索引：`ChainRestorer::finish`）。無ければ初めて要るときに作る。どちらでも、まとめが索引の範囲に収まらなければ、
    /// 範囲を広げて作り直す（索引は範囲にかかる区間と頂点をみな持つので、範囲に収まる調べの答えは、索引の範囲によらない）
    /// 戻り値は、判定の形の索引でほかの辺を調べた所（`PolyIndex::can_replace` が引いたマス。調べなければ空。重ね合わせを
    /// 窓で行うとき、窓の範囲がこの所を覆うかを確かめる：`CellSet::covers`）
    pub(crate) fn merge_degree2(
        &mut self,
        candidates: impl IntoIterator<Item = usize>,
        built: Option<(PolyIndex, Vec<usize>)>,
    ) -> CellSet {
        let mut is_cand = vec![false; self.nodes.len()];
        for v in candidates {
            is_cand[v] = true;
        }
        let n = self.edges.len();
        // 辺は元の番号のまま持ち、消した辺は印だけ付ける（残った辺の並びは元の番号の順）
        let mut edges = std::mem::take(&mut self.edges);
        let mut alive = vec![true; n];
        let mut at: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        for (i, e) in edges.iter().enumerate() {
            at[e.a].push(i);
            if e.b != e.a {
                at[e.b].push(i);
            }
        }
        let candidate = |v: usize, at: &[Vec<usize>]| at[v].len() == 2 && is_cand[v];
        let place = |v: usize| (Place(self.nodes[v]), v);
        let mut pending: std::collections::BTreeSet<(Place, usize)> = (0..self.nodes.len())
            .filter(|&v| candidate(v, &at))
            .map(place)
            .collect();
        // 判定の形の索引（受け取らなければ、初めて要るときに作る）と、辺の番号 → 索引の番号。作るときの範囲は、まとめる
        // 候補の頂点につながる辺の外接矩形（まとめた辺が範囲から出るときは、広げて作り直す）
        let (mut index, mut ix_id): (Option<PolyIndex>, Vec<usize>) = match built {
            Some((ix, ids)) => (Some(ix), ids),
            None => (None, (0..n).collect()),
        };
        debug_assert_eq!(ix_id.len(), n, "辺ごとの索引の番号");
        let mut ix_area = pending
            .iter()
            .flat_map(|&(_, v)| at[v].iter())
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
        let mut used = CellSet::default();
        while let Some((_, v)) = pending.pop_first() {
            if rejected[v] || !candidate(v, &at) {
                continue;
            }
            // 番号が先の辺が i（v で終わる向き）、後ろが j（v から始まる向き）
            let (i, j) = if at[v][0] < at[v][1] {
                (at[v][0], at[v][1])
            } else {
                (at[v][1], at[v][0])
            };
            let (e1, e2) = (edges[i].oriented(false, v), edges[j].oriented(true, v));
            if e1.a == e2.b || !e1.same_content(&e2) {
                continue;
            }
            let (q1, q2) = (
                QuadBez::new(self.nodes[e1.a], e1.ctrl, self.nodes[v]),
                QuadBez::new(self.nodes[v], e2.ctrl, self.nodes[e2.b]),
            );
            // 2本が、1本の2次ベジェ q を t で切ったものか（de Casteljau：q1.p1・v・q2.p1 が一直線に並び、v がその間を
            // t:(1−t) に分け、q の制御点 c は q1.p1 = (1−t)q1.p0 + t·c、q2.p1 = (1−t)c + t·q2.p2 を満たす）。切れ目の
            // 頂点は、切った曲線から `FLAT_TOL + SPLIT_REACH` 以内にしか乗っていない（折れ線は曲線から `FLAT_TOL` 以内、
            // i_overlay は切れ目の点を折れ線どうしの交点から `SPLIT_REACH` 以内に置く）ので、q を t で切った2本が q1・q2 と
            // その許容で同じ曲線（`same_curve`）のときだけまとめる（絵の形はその許容の中でしか変わらない）。直線も同じ
            // 決まりでまとまる
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
            let tol = FLAT_TOL + SPLIT_REACH;
            if !(same_curve(&q.subsegment(0.0..t), &q1, tol)
                && same_curve(&q.subsegment(t..1.0), &q2, tol))
            {
                continue;
            }
            // 判定の形で、2本を1本に置き換えてよいか（設計 2-4 の3つの決まり。曲線が端を越えて折り返すときも、
            // 頂点のまわりの順番か自分との重なりで見つかる）
            let pc = grid_poly(&q);
            let ix = index.get_or_insert_with(|| {
                PolyIndex::build(&self.nodes, &edges, |f| alive[f], Some(ix_area))
            });
            let path_of = |ix: &PolyIndex, ix_id: &[usize]| -> Vec<IPt> {
                let (p1, p2) = (ix.poly(ix_id[i]), ix.poly(ix_id[j]));
                let mut path: Vec<IPt> = if edges[i].b == v {
                    p1.to_vec()
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
                ix_area = ix_area.union(reach.px());
                *ix = PolyIndex::build(&self.nodes, &edges, |f| alive[f], Some(ix_area));
                ix_id = (0..n).collect();
                path = path_of(ix, &ix_id);
            }
            if !ix.can_replace(
                &[ix_id[i], ix_id[j]],
                &path,
                &[v],
                &pc,
                e1.a,
                e2.b,
                &mut used,
            ) {
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
            for w in [e1.a, e2.b] {
                if candidate(w, &at) && !rejected[w] {
                    pending.insert(place(w));
                }
            }
        }
        self.edges = (0..n).filter(|&i| alive[i]).map(|i| edges[i]).collect();
        used
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

/// 色 `color` の面の、輪郭（外周、穴の順）の曲線の並び `cycles` から、塗りの形を作る（面の図形を作る決まりはこれ1つ：
/// 地図から作る `PlanarMap::face_shape`・書き戻しで作る `LocalMap::rewrite`）。輪郭は座標のいちばん小さい点から始め
/// （`reuse` が k 番目の輪郭と形の同じ輪郭（前の図形の輪郭。同じ決まりで作ったもの）を返せばそれ）、穴はその点の順に
/// 並べる（形は地図の面だけで決まり、辺の並びや作り方（差分から作る・しまった地図から作り直す）によらない）
fn cycles_shape(
    color: Color32,
    cycles: impl Iterator<Item = Vec<QuadBez>>,
    reuse: impl Fn(usize, &[QuadBez]) -> Option<Contour>,
    layer_id: u64,
    group_path: &[u64],
) -> FillShape {
    let mut contours = cycles.enumerate().map(|(k, quads)| {
        reuse(k, &quads).unwrap_or_else(|| Contour::new(from_lowest_point(quads)))
    });
    let exterior = contours.next().unwrap_or_else(|| Contour::new(Vec::new()));
    let mut holes: Vec<Contour> = contours.collect();
    holes.sort_by(|a, b| starts_order(&a.segments, &b.segments));
    FillShape::new(color, exterior, holes).with_scope(layer_id, group_path)
}

/// 線の番号の印：線の番号はこの桁が立った数（面の番号は 1 から順に振るので、この桁には届かず、ぶつからない）
const LINE_ID_BIT: ShapeId = 1 << 63;

/// 番号 `id` が線の番号か（`line_id`）
pub(crate) fn is_line_id(id: ShapeId) -> bool {
    id & LINE_ID_BIT != 0
}

/// 線の図形の表：線を乗せた辺1本ずつ（アンカーとアンカーの間。クリックで選ぶ単位：設計 claude/線の設計_68a596e.md の
/// 6.4）の線の図形を、番号（`line_id_of`）の順に並べたもの（面の表と同じ入れ物。変わった所のかかる塊だけを作り直す）
pub(crate) type LineTable = crate::chunked_list::ChunkedList<FillShape>;

/// 辺 `te` の線の図形の番号（線の無い辺は `None`）：辺の形（決まった向きの両端と制御点のビット）、線、辺の側の番号（面の
/// 番号と線のラベル：`line_label`。決まった向きにそろえる）から決める（形・線・番号がみな同じ間だけ同じ番号なので、形が
/// 少しでも変われば別の番号になり、図形を作り直す。番号を入れるので、同じ形の線がほかのレイヤーに写してあっても、ぶつから
/// ない。線の番号の印 `LINE_ID_BIT` を立てる）。辺1本から決まるので、ほかの辺を見ずに引ける
pub(crate) fn line_id_of(te: &crate::stored_map::TileEdge) -> Option<ShapeId> {
    use std::hash::{Hash, Hasher};
    let stroke = te.stroke?;
    let (a, b, srcs) = if point_order(te.a, te.b).is_le() {
        (te.a, te.b, [te.left_src, te.right_src])
    } else {
        (te.b, te.a, [te.right_src, te.left_src])
    };
    let mut h = IntHasher::default();
    [a.x, a.y, b.x, b.y, te.ctrl.x, te.ctrl.y].map(f64::to_bits).hash(&mut h);
    stroke.hash(&mut h);
    srcs.hash(&mut h);
    Some(LINE_ID_BIT | (h.finish() >> 1))
}

/// しまう形 `stored` の図形の番号：面の番号（色のある側に記した番号）と、線の番号（線を乗せた辺ごとの `line_id_of`。線の
/// 図形の番号と同じ決まり）。同じ番号をくり返す。地図から図形の番号を並べる決まりはこれ1つ（浮いた片の番号
/// `PieceGroup::ids`・置いた中身を選ぶ `Cel::shape_ids`）。前は面の番号だけを並べる所があり、線が選択や片の番号から
/// 漏れた
pub(crate) fn shape_ids_of(stored: &StoredMap) -> impl Iterator<Item = ShapeId> + '_ {
    (0..stored.edge_count()).flat_map(move |e| {
        let te = stored.edge_at(e);
        [te.left.and(te.left_src), te.right.and(te.right_src), line_id_of(&te)]
            .into_iter()
            .flatten()
    })
}

/// 地図 `m` の辺 `ed` を、塊から引いた辺の形にする（線の番号 `line_id_of` を引く・差分を当てる・しまう辺にする）
pub(crate) fn tile_edge(m: &PlanarMap, ed: &MapEdge) -> crate::stored_map::TileEdge {
    crate::stored_map::TileEdge {
        a: m.nodes[ed.a],
        b: m.nodes[ed.b],
        ctrl: ed.ctrl,
        left: ed.left,
        right: ed.right,
        left_src: ed.left_src,
        right_src: ed.right_src,
        stroke: ed.stroke,
    }
}

/// 線を乗せた辺 `te` を、線の太さに太らせた形の線の図形（端の形は `ends`：`line_ends`。階層 `group_path`、番号 `id`）。輪郭と
/// メッシュは、中心線の左右の帯と端の形から直接作る（`quad_band`。描画の許容 `MESH_TOL`。交わりを解く計算も、多角形を三角形に
/// 分ける計算もしないので、手間は点の数に比例する）。輪郭は曲がる内側で自分と重なることがあるので、線の図形は NonZero で塗る
/// （`FillShape::fills_nonzero`）
fn line_shape(
    te: &crate::stored_map::TileEdge,
    ends: [crate::vector_stroke::LineEnd; 2],
    id: ShapeId,
    group_path: &[u64],
) -> Option<FillShape> {
    let stroke = te.stroke?;
    let (exterior, holes, mesh) = line_band(te.quad(), stroke, ends)?;
    let mut s = FillShape::new(stroke.color, exterior, holes)
        .with_scope(0, group_path)
        .with_line(stroke, te.quad(), ends)
        .with_mesh(mesh);
    s.shape_id = id;
    Some(s)
}

/// 中心の曲線 `q`・線 `stroke`・端の形 `ends` の線の図形の、輪郭（外周と、つなぎ目の扇）とメッシュ（中心線の左右の帯と端の形
/// から直接作る：`quad_band`。描画の許容 `MESH_TOL`。メッシュは多角形を三角形に分けず、帯から直接作ったもの）。線の形を作る
/// のはこれ1つ（地図の線の図形 `line_shape` と、太さを変えずに写して作り直す `FillShape::transform_keeping_line_width`）
pub(crate) fn line_band(
    q: QuadBez,
    stroke: EdgeStroke,
    ends: [crate::vector_stroke::LineEnd; 2],
) -> Option<(Contour, Vec<Contour>, eframe::egui::Mesh)> {
    let (rings, mesh) = crate::vector_stroke::quad_band(
        q,
        stroke.width as f64 / 2.0,
        crate::vector_engine::MESH_TOL,
        stroke.color,
        ends,
    );
    let mut contours = rings.into_iter().map(|ring| {
        let segs = (0..ring.len())
            .map(|i| {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                QuadBez::new(a, a.midpoint(b), b)
            })
            .collect();
        Contour::new(segs)
    });
    let exterior = contours.next()?;
    Some((exterior, contours.collect(), mesh))
}

/// 線を乗せた辺 `te` の、始め（`te.a`）と終わり（`te.b`）の端の形（線の端の決まりはこれ1つ）。`others(p)` は、点 `p` に端の
/// ある線を乗せた辺（`te` も含めてよい）。その点にほかの線がちょうど1本で同じ線なら、つなぎ目：2本のうち形の鍵（決まった
/// 向きの両端と制御点のビット）が小さい方が外側のすき間を扇で埋め（`Join`。隣の線の、その点から出ていく向きを渡す）、もう
/// 1本は何も足さない（`Open`）。そうでなければ（行き止まり・交わり・線の変わり目）半円（`Cap`）。辺の向きはどちらでもよい
fn line_ends(
    te: &crate::stored_map::TileEdge,
    others: impl Fn(Point) -> Vec<crate::stored_map::TileEdge>,
) -> [crate::vector_stroke::LineEnd; 2] {
    use crate::vector_stroke::{quad_tangent, LineEnd};
    let key = |e: &crate::stored_map::TileEdge| {
        let (a, b) = if point_order(e.a, e.b).is_le() { (e.a, e.b) } else { (e.b, e.a) };
        [a.x, a.y, b.x, b.y, e.ctrl.x, e.ctrl.y].map(f64::to_bits)
    };
    let me = key(te);
    [te.a, te.b].map(|p| {
        let near: Vec<crate::stored_map::TileEdge> =
            others(p).into_iter().filter(|e| key(e) != me).collect();
        match near.as_slice() {
            [f] if f.stroke == te.stroke => {
                if key(f) < me {
                    return LineEnd::Open;
                }
                let q = f.quad();
                LineEnd::Join(if f.a == p { quad_tangent(&q, 0.0) } else { -quad_tangent(&q, 1.0) })
            }
            _ => LineEnd::Cap,
        }
    })
}

/// 辺の形が同じかを比べるための鍵（向きはそろえる）。色は入れない：辺のその側が変わっていないかは、側の元の図形の印で
/// 分かる（重ね合わせ・バケツは、色を変えた側の印を消す）
fn edge_key(m: &PlanarMap, e: usize) -> [u64; 6] {
    #[cfg(test)]
    KEYS_BUILT.with(|n| n.set(n.get() + 1));
    local::quad_key(&m.quad(e))
}

/// 書き戻しで、面が元の図形と番号をそのまま残すか（残す面の決まりはこれ1つ：1本の道 `LocalMap::rewrite`・切り出した片の
/// 番号 `LayerBase::piece_ids`。引き方だけが違う）。面の輪郭の向きのある辺 `halves` の側の番号 `new_src` がみな1つの
/// 番号 s で、その面の側の元の地図の番号 `old_src`（形が同じ元の辺の同じ側。そういう辺が無ければ `None`）もみな s で、
/// 面の色 `color` が元の図形 s の色 `orig_color(s)` と同じで、元の図形 s の辺がみな残っている（`cut(s)` でない）とき、
/// s（同じ形の面が入れ替わっても、元のその面の所にある面だけが、その面の図形を残す）。同じ s を残せるのは1つの面だけ
/// （使う側が決める）
pub(super) fn kept_src(
    halves: impl IntoIterator<Item = (usize, bool)>,
    new_src: impl Fn((usize, bool)) -> Option<ShapeId>,
    old_src: impl Fn((usize, bool)) -> Option<ShapeId>,
    color: Color32,
    orig_color: impl Fn(ShapeId) -> Option<Color32>,
    cut: impl Fn(ShapeId) -> bool,
) -> Option<ShapeId> {
    let mut src = None;
    for h in halves {
        let s = new_src(h)?;
        if *src.get_or_insert(s) != s || old_src(h) != Some(s) {
            return None;
        }
    }
    let s = src?;
    (orig_color(s) == Some(color) && !cut(s)).then_some(s)
}

/// 番号の票（番号, 数）に、番号 `id` の1票を足す
fn add_vote(votes: &mut Vec<(ShapeId, usize)>, id: ShapeId) {
    match votes.iter_mut().find(|v| v.0 == id) {
        Some(v) => v.1 += 1,
        None => votes.push((id, 1)),
    }
}

/// 票のうち、いちばん多い番号とその数（同じ数なら小さい番号）。面の番号を票で決める決まりはこれ1つ（重ね合わせで面に
/// 伝える番号、書き戻しで面が引き継ぐ番号：`LocalMap::rewrite`）
fn top_vote(votes: &[(ShapeId, usize)]) -> Option<(ShapeId, usize)> {
    votes
        .iter()
        .copied()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
}

/// 地図 `map` の辺の側に、面 `faces` の図形の番号 `ids` を記した地図（面の無い側は無し）。文書にしまう地図は、面の辺の側に
/// いつもその面の図形の番号を持つ（図形と地図の面が1対1に対応する）
fn labeled(mut map: PlanarMap, faces: &[MapFace], ids: &[ShapeId]) -> PlanarMap {
    // 色の無い側の番号は、線を乗せた辺なら線のラベルとして残す（`line_label`）
    for ed in &mut map.edges {
        ed.left_src = line_label(ed.left, ed.left_src, ed.stroke);
        ed.right_src = line_label(ed.right, ed.right_src, ed.stroke);
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

/// 辺の側の、線のラベル（ラベルの決まりはこれ1つ）：線を乗せた辺の色の無い側の番号は、その線のラベル（線の番号 `line_id`
/// を、文書の中で別の線とぶつからないようにする印。何も無い所に引いた線は、引いたときに新しい番号を付け、面の輪郭だった線は、
/// その面の番号から作る（`label_of_face`）。レイヤーへ写す・貼り付ける・読み込むときは、面の番号と同じく付け替える）。
/// 色のある側と、線の無い辺の側は `None`
pub(crate) fn line_label(
    color: Option<Color32>,
    src: Option<ShapeId>,
    stroke: Option<EdgeStroke>,
) -> Option<ShapeId> {
    src.filter(|_| color.is_none() && stroke.is_some())
}

/// 面 `face` の輪郭だった線の、線のラベル：面の番号に、線の番号の印 `LINE_ID_BIT` を立てたもの（面の番号は 1 から順に振る
/// のでこの桁に届かず、まだ残っているその面の番号とぶつからない。線のラベルを面と取り違えない）。色の無い側の番号（もう
/// 線のラベル）はそのまま
pub(crate) fn label_of_face(color: Option<Color32>, src: Option<ShapeId>) -> Option<ShapeId> {
    match color {
        Some(_) => src.map(|id| id | LINE_ID_BIT),
        None => src,
    }
}

/// 面と線を残す・外すときの辺の両側と線（決まりはこれ1つ：地図全体の `PlanarMap::restricted_by`、塊から作る
/// `LayerBase::only_faces_map`・`restricted_local`）。`keep` は面の番号と線の番号（`line_id`）に答える。色のある側は、その面を
/// 残すなら残し、外すなら何も無いにする。線は、その辺の線の図形の番号 `line` を残すなら残し、外すなら外す（Animate と
/// 同じく、面と線は別々に選ぶ。設計：claude/線の設計_68a596e.md の 6.4）。線を残した辺の色の無い側には、線のラベル
/// （`line_label`。外した面の側は、その面の番号）を残す。残す辺でなければ `None`
#[allow(clippy::type_complexity)]
fn restricted_sides(
    sides: [Label; 2],
    stroke: Option<EdgeStroke>,
    line: Option<ShapeId>,
    keep: impl Fn(ShapeId) -> bool,
) -> Option<([Label; 2], Option<EdgeStroke>)> {
    let stroke = stroke.filter(|_| line.is_some_and(&keep));
    let side = |(color, src): Label| match color {
        Some(_) if src.is_some_and(&keep) => (color, src),
        _ => (None, line_label(None, label_of_face(color, src), stroke)),
    };
    let out = [side(sides[0]), side(sides[1])];
    edge_needed(out[0].0, out[1].0, stroke).then_some((out, stroke))
}

/// 頂点を位置の順（`point_order`）に並べる鍵（2本のまとめ `merge_degree2` の調べる順）
#[derive(Clone, Copy, PartialEq)]
struct Place(Point);

impl Eq for Place {}

impl PartialOrd for Place {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Place {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        point_order(self.0, other.0)
    }
}

/// 差分（窓の結果, 消えた辺, 増えた辺）：`LayerBase::apply_diff` に渡すもの
type LocalDiff = (PlanarMap, Vec<usize>, Vec<usize>);

/// 地図 `m` の辺 `edges` を、塊の辺の形にする（差分を当てる `StoredMap::with_diff_tiles` に渡すため）
fn tile_edges_of(m: &PlanarMap, edges: &[usize]) -> Vec<crate::stored_map::TileEdge> {
    edges.iter().map(|&e| tile_edge(m, &m.edges[e])).collect()
}

/// 道具が作る、重ね合わせに渡すもの（`LayerBase::paint_change`・`reshape_change`）：重ねるもの・その曲線・その左右の色
/// （輪郭の変形）・決まり。1本の道（`LayerBase::edit_local`）は、これだけを受け取る
pub(crate) struct LocalChange<'a> {
    input: ChangeInput<'a>,
    /// 重ねるものの線（`MapInput::lines`）
    lines: Lines,
    rule: MapRule,
}

/// 重ねるもの（`MapInput` の、持ち物にしたもの）
enum ChangeInput<'a> {
    Shapes(&'a [FillShape]),
    Sweep(Vec<(usize, QuadBez)>),
    Map(&'a PlanarMap),
    Lines(&'a [LineSeg], ShapeId),
}

impl LocalChange<'_> {
    fn input(&self) -> MapInput<'_> {
        match &self.input {
            ChangeInput::Shapes(s) => MapInput::Shapes(s),
            ChangeInput::Sweep(m) => MapInput::Sweep(m),
            ChangeInput::Map(p) => MapInput::Map(p),
            ChangeInput::Lines(l, label) => MapInput::Lines(l, *label),
        }
    }
}

/// 面を丸ごと外す・残す（`LayerBase::restricted_local`。`PlanarMap::restricted_by` と同じ決まり）：番号が並びにある面を
/// 外すか、並びにある面だけを残す
#[derive(Clone, Copy)]
pub(crate) enum FaceKeep<'a> {
    Without(&'a HashSet<ShapeId>),
    Only(&'a HashSet<ShapeId>),
}

impl FaceKeep<'_> {
    /// 番号 `id` の面を残すか
    fn keeps(&self, id: ShapeId) -> bool {
        match self {
            FaceKeep::Without(ids) => !ids.contains(&id),
            FaceKeep::Only(ids) => ids.contains(&id),
        }
    }
}

/// 触った所だけで行う編集（`LayerBase::edit_local`）の答え
pub(crate) enum LocalEdit {
    /// 変わらない
    Same,
    /// 書けない（`OffGrid`：格子に置けない座標がある、`Unwritable`：正しい地図にならない）
    Refused(LocalRefusal),
    /// 書く
    Changed(LocalWrite),
}

/// 触った所だけで行う編集を書けない理由
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalRefusal {
    OffGrid,
    Unwritable,
}

/// 触った所だけで行った編集の書くもの：前のしまう形に差分を当てた新しいしまう形（正しい地図で、面に1つずつ別の番号が
/// 記してある）と、新しい面の図形の表（番号の順。変わらない面は前の表と同じ図形を共有する：消した図形は前の表にだけ、
/// 入れた図形は新しい表にだけある）
pub(crate) struct LocalWrite {
    pub(crate) stored: StoredMap,
    pub(crate) faces: FaceTable,
    /// 前の表から消した図形の番号と、新しい表に入れた図形の番号（浮いた片の残りが、隠す図形と出す図形を決めるため）
    pub(crate) removed: Vec<ShapeId>,
    pub(crate) added: Vec<ShapeId>,
}

/// お手本の計算 `f` を、手間を数える試しの数に入れずに行う（このスレッドの数を、行う前の値に戻す）
#[cfg(test)]
pub(crate) fn uncounted<R>(f: impl FnOnce() -> R) -> R {
    type Counter = std::thread::LocalKey<std::cell::Cell<usize>>;
    let counters: [&'static Counter; 15] = [
        &FROM_SHAPES,
        &FULL_CHECKS,
        &DENSE_BUILDS,
        &FED_RUNS,
        &INDEX_BUILDS,
        &POLYS_BUILT,
        &REACHES_BUILT,
        &VERTICES_SORTED,
        &RINGS_BUILT,
        &KEYS_BUILT,
        &CUTS,
        &OUTSIDE_WINDS,
        &RANGE_ENTRIES,
        &OWNER_SEARCHES,
        &crate::stored_map::EDGES_SORTED,
    ];
    let saved: Vec<usize> = counters.iter().map(|c| c.with(|n| n.get())).collect();
    let out = f();
    for (c, v) in counters.iter().zip(saved) {
        c.with(|n| n.set(v));
    }
    out
}

/// テストで、書くたびに地図を確かめるか（速さを測る試しでは、環境変数 `IS2D_NO_CHECKS` で外す）
#[cfg(test)]
pub(crate) fn test_checks() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("IS2D_NO_CHECKS").is_err())
}

// ============================================================================
// 計算に使う写しと、地図を書き戻すときの差分（どの編集も使う）
// ============================================================================

/// 1つのレイヤー・階層の地図の、計算に使う写し：ふつうの形（いつもしまう形から決まった順で作ったもの：
/// `StoredMap::to_dense`）と、辺ごとの判定の形（地図ごとに1回だけ作る：`StoredMap::poly`）と、面の図形（写し。
/// 初めて要るときに作る：`LayerBase::faces`）。写しは、それを使う所が持つ（画面の土台・地図の編集の間・カーソル合わせ）。
/// 文書と Undo の控えは、しまう形（`StoredMap`）だけを持つ
#[derive(Debug)]
pub(crate) struct LayerBase {
    /// 計算に使うふつうの地図（しまう形から決まった順で作ったもの：`StoredMap::to_dense`）。要る所が初めて使うときに作る
    /// （`map`。差分から作った写しは、これを作らずに使える：設計 段2b の 7. の 2b-7c）
    dense: OnceLock<PlanarMap>,
    pub(crate) stored: StoredMap,
    /// 辺ごとの判定の形（`map` の辺の順）
    polys: OnceLock<Vec<Poly>>,
    /// 面の輪郭（`face_cycles_with`）
    cycles: OnceLock<Vec<Cycle>>,
    /// 面（`faces_of`。読んだ地図の確かめで作った面は、ここに残す：`checked_stored`）
    map_faces: OnceLock<Vec<MapFace>>,
    /// 面の図形：（階層, 面ごとの図形。番号は面の印。番号の順）
    faces: SyncCache<Option<FaceShapes>>,
    /// 線の図形の表：（階層, 線の図形の表）。初めて要るときに作る（`lines`）
    lines: SyncCache<Option<LineShapes>>,
    /// 前の写し（この写しの地図に書き換える前の地図の写し）が作ってあった線の図形の表と、その地図。線の図形の表を作る
    /// とき、前の表から、変わった辺の分だけを替えて作る（作り終えたら手放す）
    prev_lines: SyncCache<Option<(LineShapes, StoredMap)>>,
    /// 升目の索引（初めて要るときに作る。地図を書き換えたあとの写しは、前の写しの索引から変わった所だけ作り直したもの
    /// を持つ：`of_local`・`of_stored_after`）
    index: OnceLock<crate::stored_map::TileIndex>,
    /// 端の升目の索引（升目ごとの、端がその升目にある辺。頂点の辺を引く所が使う：`end_tiles`。作り方は `index` と同じ）
    ends: OnceLock<crate::stored_map::TileIndex>,
    /// しまう形の判定の形を生かしておく所（この写しが引いた塊の分。塊そのものは判定の形を生かさない：`StoredMap::poly`）
    hold: crate::stored_map::PolyHold,
}

impl Drop for LayerBase {
    /// 判定の形を生かしておく所を手放す（ほかに使う写しの無い塊の判定の形は返り、塊の弱い参照も消える）
    fn drop(&mut self) {
        self.stored.release_polys(&mut self.hold);
    }
}

/// 地図の面の図形（写し）：（階層, 面の番号の順の図形）
type FaceShapes = (Vec<u64>, FaceTable);

/// 地図の線の図形の表：（階層, 線の図形の表）
type LineShapes = (Vec<u64>, LineTable);

/// 面の表：面の図形を番号の順に並べたもの（塊に分けて持つ。編集のあとの表は、変わった面のかかる塊だけを作り直し、
/// ほかの塊は前の表と共有する：`ChunkedList::replace_sorted`。設計 段2b の 15.）
pub(crate) type FaceTable = crate::chunked_list::ChunkedList<FillShape>;

impl FaceTable {
    /// 番号が `id` の面の図形
    pub(crate) fn by_id(&self, id: ShapeId) -> Option<&Arc<FillShape>> {
        self.find_sorted(id, |s| s.shape_id)
    }
}

impl LayerBase {
    /// 地図 `map` を、しまう形にして決まった順に並べ直したもの（文書に入らない、その場で使う地図）
    pub(crate) fn new(map: &PlanarMap) -> Self {
        Self::from_stored(StoredMap::from_dense(map, None))
    }

    /// しまった地図 `stored` の写し。地図全体の一覧（`map`）・判定の形・輪郭・面の表・升目の索引は、要る所が初めて使う
    /// ときに作る（写しを作るだけでは、地図全体をなめない）
    fn from_stored(stored: StoredMap) -> Self {
        Self::with_dense(OnceLock::new(), stored)
    }

    fn with_dense(dense: OnceLock<PlanarMap>, stored: StoredMap) -> Self {
        Self {
            dense,
            stored,
            polys: OnceLock::new(),
            cycles: OnceLock::new(),
            map_faces: OnceLock::new(),
            faces: SyncCache::new(None),
            lines: SyncCache::new(None),
            prev_lines: SyncCache::new(None),
            index: OnceLock::new(),
            ends: OnceLock::new(),
            hold: Default::default(),
        }
    }

    /// しまう形の辺 `e` の判定の形（この写しが生かす：`StoredMap::poly`）
    pub(crate) fn stored_poly(&self, e: usize) -> &Poly {
        self.stored.poly(e, &self.hold)
    }

    /// 計算に使うふつうの地図（初めて要るときに、しまう形から作る：`StoredMap::to_dense`）
    pub(crate) fn map(&self) -> &PlanarMap {
        self.dense.get_or_init(|| {
            #[cfg(test)]
            DENSE_BUILDS.with(|n| n.set(n.get() + 1));
            self.stored.to_dense()
        })
    }

    /// しまった地図 `stored` の計算に使う写し：いま誰かが持っていればそれ、無ければ作る（作ったものを生かすのは、
    /// 受け取った所だけ）
    pub(crate) fn of_stored(stored: &StoredMap) -> Arc<Self> {
        let mut slot = stored.dense_slot().lock();
        if let Some(base) = slot.upgrade() {
            return base;
        }
        let base = Arc::new(Self::from_stored(stored.clone()));
        *slot = Arc::downgrade(&base);
        base
    }

    /// しまった地図 `stored` の計算に使う写し（`of_stored` と同じ）。新しく作るときは、写し `prev`（`stored` と塊を共有
    /// する地図の写し）の升目の索引から、変わった塊の分だけ作り直した索引を持たせる
    pub(crate) fn of_stored_after(stored: &StoredMap, prev: &LayerBase) -> Arc<Self> {
        let mut slot = stored.dense_slot().lock();
        if let Some(base) = slot.upgrade() {
            return base;
        }
        let base = Self::from_stored(stored.clone());
        base.inherit_index(Some(prev));
        let base = Arc::new(base);
        *slot = Arc::downgrade(&base);
        base
    }

    /// 1本の道で行った編集（`apply_diff`）の新しいしまう形 `stored`（階層 `group_path`、面の図形の表 `faces`）の写し。
    /// 地図全体の一覧（`map`）・判定の形・輪郭の一覧は作らず、要る所が初めて使うときに作る。升目の索引は、前の写し `prev`
    /// の索引から、変わった塊の分だけ作る（設計 段2b の 7. の 2b-7c）
    pub(crate) fn of_local(
        stored: StoredMap,
        group_path: &[u64],
        faces: FaceTable,
        prev: &LayerBase,
    ) -> Arc<Self> {
        let base = Self {
            dense: OnceLock::new(),
            stored,
            polys: OnceLock::new(),
            cycles: OnceLock::new(),
            map_faces: OnceLock::new(),
            faces: SyncCache::new(Some((group_path.to_vec(), faces))),
            lines: SyncCache::new(None),
            prev_lines: SyncCache::new(None),
            index: OnceLock::new(),
            ends: OnceLock::new(),
            hold: Default::default(),
        };
        base.inherit_index(Some(prev));
        // テストでは毎回：面の表の図形の輪郭の辺が、みなしまう形にあり、その面の側にその面の番号が記してある（面の表と
        // 地図の番号が食い違わない）
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                base.faces(group_path).iter().all(|f| {
                    f.contours().flat_map(|c| c.segments.iter()).all(|q| {
                        base.stored.find_edge(q).is_some_and(|(e, fwd)| {
                            let te = base.stored.edge_at(e);
                            // 輪郭は面を左に見る向き
                            let side = if fwd { te.left_src } else { te.right_src };
                            side == Some(f.shape_id)
                        })
                    })
                })
            });
            assert!(ok, "面の表の図形と、地図の辺の側の番号が食い違う");
        }
        let base = Arc::new(base);
        *base.stored.dense_slot().lock() = Arc::downgrade(&base);
        base
    }

    /// 写し `prev` が升目の索引を持っていれば、そこから、この写しの索引を作る（変わった塊の辺がかかる升目だけを作り
    /// 直す：`TileIndex::derive`）。持っていなければ何もしない（初めて要るときに全部作る）
    fn inherit_index(&self, prev: Option<&LayerBase>) {
        let Some(prev) = prev else {
            return;
        };
        // 前の写しの線の図形の表も受け取る（`lines` が変わった辺の分だけを替える）
        *self.prev_lines.lock() = prev.lines.lock().clone().map(|t| (t, prev.stored.clone()));
        self.derive_index(prev, |b| &b.index, LayerBase::edge_tiles);
        self.derive_index(prev, |b| &b.ends, LayerBase::end_tiles);
    }

    /// 写し `prev` の索引（`slot` で選ぶ：升目の索引か端の升目の索引）から、この写しの同じ索引を作る（変わった塊の分だけ
    /// 作り直す）。`tiles`：辺がかかる升目の決め方
    fn derive_index(
        &self,
        prev: &LayerBase,
        slot: impl Fn(&LayerBase) -> &OnceLock<crate::stored_map::TileIndex>,
        tiles: fn(&LayerBase, usize) -> Vec<(i64, i64)>,
    ) {
        if slot(self).get().is_some() {
            return;
        }
        let Some(old) = slot(prev).get() else {
            return;
        };
        let derived = old.derive(
            &prev.stored,
            &self.stored,
            |e| tiles(prev, e),
            |e| tiles(self, e),
        );
        #[cfg(test)]
        if test_checks() {
            let full = crate::stored_map::TileIndex::build(&self.stored, |e| tiles(self, e));
            assert!(
                derived.same_content(&full),
                "前の写しから作った索引が、全部作ったものと違う"
            );
        }
        let _ = slot(self).set(derived);
    }

    /// 辺 `e` の端がある、端の升目の索引の升目（`end_key`）
    fn end_tiles(&self, e: usize) -> Vec<(i64, i64)> {
        let te = self.stored.edge_at(e);
        let mut t = vec![end_key(te.a), end_key(te.b)];
        t.sort_unstable();
        t.dedup();
        t
    }

    /// 端の升目の索引（初めて要るときに全部作る）
    pub(crate) fn ends_index(&self) -> &crate::stored_map::TileIndex {
        self.ends.get_or_init(|| {
            crate::stored_map::TileIndex::build(&self.stored, |e| self.end_tiles(e))
        })
    }

    /// 辺 `e` がかかる、升目の索引の升目：辺の近くを探すマス（`EdgeGeom::reach`）の升目。マスで探す所（窓の辺
    /// `region_edges`・`NearEdges`）も、点のまわりや半直線の所を探す所（辺の点は、その点を含むマスにある）も、その升目を
    /// 引けば必ず見つかる
    fn edge_tiles(&self, e: usize) -> Vec<(i64, i64)> {
        // 写しの一覧を作らずに、塊の判定の形と辺から（差分から作った写しでも、一覧を作らない）
        let (_, cells) = self.stored_poly(e).reach(self.stored.edge_at(e).quad());
        index_tiles(cells.iter().copied())
    }

    /// 辺 `e`（計算に使う地図の番号）の曲線と左右の色と線（輪郭の変形で動かした辺の前の曲線と色：`MapInput::lines`）。塊から
    /// 引く
    fn old_edge(&self, e: usize) -> (QuadBez, Sides, Option<EdgeStroke>) {
        let te = self.stored.edge_at(e);
        (te.quad(), (te.left, te.right), te.stroke)
    }

    /// 道具の側（ブラシ・消しゴム）：形 `shapes`（文書の座標）を決まり `rule` で重ねるときに、重ね合わせに渡すもの。形の線の
    /// 端は、前からある頂点から `FLAT_TOL` 以内なら、その頂点に寄せる（`overlay` の形と同じ）
    pub(crate) fn paint_change<'a>(
        &self,
        shapes: &'a [FillShape],
        rule: MapRule,
    ) -> LocalChange<'a> {
        let mut lines = MapInput::Shapes(shapes).lines(|e| self.old_edge(e));
        self.snap_ends(&mut lines.quads);
        LocalChange {
            input: ChangeInput::Shapes(shapes),
            lines,
            rule,
        }
    }

    /// 道具の側（鉛筆）：線 `segs`（文書の座標。線のラベルは `label`）を引くときに、重ね合わせに渡すもの（`MapRule::Line`）。
    /// 線の端は、前からある頂点から `FLAT_TOL` 以内なら、その頂点に寄せる（形と同じ）
    pub(crate) fn line_change<'a>(&self, segs: &'a [LineSeg], label: ShapeId) -> LocalChange<'a> {
        let mut lines = MapInput::Lines(segs, label).lines(|e| self.old_edge(e));
        self.snap_ends(&mut lines.quads);
        LocalChange {
            input: ChangeInput::Lines(segs, label),
            lines,
            rule: MapRule::Line,
        }
    }

    /// 道具の側（輪郭の変形）：辺・頂点の引っぱり・頂点の移動 `edit` で、重ね合わせに渡すもの（動かした辺と、動かした後の曲線：
    /// `pulled_edges`・`moved_quad`。辺・頂点は塊から引く）。確定（`edit_local`）とドラッグ中の表示（`reshape_result`）の
    /// どちらもこれで作る。辺・頂点が無ければ `None`
    pub(crate) fn reshape_change(&self, edit: ReshapeEdit) -> Option<LocalChange<'static>> {
        let moved: Vec<(usize, QuadBez)> = match edit {
            ReshapeEdit::PullEdge {
                orig_seg,
                grab_t,
                target,
            } => self.pulled_edges(PullGrab::Edge { orig_seg, grab_t }, target)?,
            ReshapeEdit::PullVertex {
                orig_vertex,
                target,
            } => self.pulled_edges(PullGrab::Vertex(orig_vertex), target)?,
            ReshapeEdit::MoveVertex {
                orig_vertex,
                target,
            } => {
                let at = self.edges_at_node(orig_vertex);
                if at.is_empty() {
                    return None;
                }
                at.into_iter()
                    .map(|e| {
                        (
                            e,
                            moved_quad(self.stored.edge_at(e).quad(), orig_vertex, target),
                        )
                    })
                    .collect()
            }
        };
        let lines = MapInput::Sweep(&moved).lines(|e| self.old_edge(e));
        Some(LocalChange {
            input: ChangeInput::Sweep(moved),
            lines,
            rule: MapRule::Sweep,
        })
    }

    /// 輪郭の引っぱり：つかんだ所 `grab`（辺の上の点か頂点）を `target` へ引っぱったときに動く辺と、動かした後の曲線（しまう
    /// 向き）。まわりの辺も、つかんだ所から離れるほど小さく動きながらついてくる（Animate の手ざわり。利用者が決めた。設計
    /// claude/引っぱりでまわりがついてくる設計_ea9f676.md）。つかんだ所から線を外へたどる（辺の上の点なら両側の2方向、
    /// 頂点ならそこから出る辺の数だけ：`pull_side`）。範囲の頂点と制御点を、つかんだ所からの曲線の長さの重み（`PullWeights`）
    /// に比例して動かし、つかんだ点がちょうど `target` へ行くように大きさを決める。範囲が、つかんだ辺1本のときは、両端を
    /// 動かさず制御点だけを Δ / (2t(1−t)) 動かす曲線（前の決まり）と同じ。頂点から出る辺がみな1本で止まるときは、まっすぐな
    /// 辺（左右対称の辺）なら頂点を動かし制御点を半分動かす曲線（頂点だけを動かす決まり `moved_quad`）と同じ（片寄って曲がった
    /// 辺では、制御点の重みが t = 1/2 の点までの長さで決まるので、半分から少しずれる）。確定とドラッグ中の表示
    /// （`reshape_change`）・引っぱっている辺の表示が、どれもこれを使う。辺・頂点が無ければ `None`
    pub(crate) fn pulled_edges(&self, grab: PullGrab, target: Point) -> Option<Vec<(usize, QuadBez)>> {
        let mut out = Vec::new();
        match grab {
            PullGrab::Edge { orig_seg, grab_t } => {
                let (e0, forward) = self.stored.find_edge(&orig_seg)?;
                let q0 = self.stored.edge_at(e0).quad();
                let t = if forward { grab_t } else { 1.0 - grab_t };
                // 両側（端 b の側と a の側）：先頭は、つかんだ辺の、つかんだ所から端までの切れ端
                let mut used = HashSet::from([e0]);
                let to_b = self.pull_side(e0, q0.subsegment(t..1.0), &mut used);
                let to_a = self.pull_side(e0, reversed(q0.subsegment(0.0..t)), &mut used);
                let (wb, wa) = (PullWeights::new(&to_b), PullWeights::new(&to_a));
                // つかんだ辺の制御点の重み：t = 1/2 の点のある側の、その点の重み（つかんだ所からの長さは、端から測った長さの
                // 差。長さ 0 の曲線の長さは式が割り算で壊れるので測らない）
                let from_grab = (q0.subsegment(0.0..0.5).arclen(FLAT_TOL)
                    - q0.subsegment(0.0..t).arclen(FLAT_TOL))
                .abs();
                let w_ctrl = if t <= 0.5 { wb.at(from_grab) } else { wa.at(from_grab) };
                let (w_a, w_b) = (wa.verts[0], wb.verts[0]);
                let reach =
                    (1.0 - t) * (1.0 - t) * w_a + 2.0 * t * (1.0 - t) * w_ctrl + t * t * w_b;
                let amp = (target - q0.eval(t)) / reach;
                out.push((
                    e0,
                    QuadBez::new(q0.p0 + amp * w_a, q0.p1 + amp * w_ctrl, q0.p2 + amp * w_b),
                ));
                self.push_pulled(&to_b[1..], &wb, 1, w_b, amp, &mut out);
                self.push_pulled(&to_a[1..], &wa, 1, w_a, amp, &mut out);
            }
            PullGrab::Vertex(v) => {
                let at = self.edges_at_node(v);
                if at.is_empty() {
                    return None;
                }
                // 頂点の重みは 1 なので、大きさは動かした量そのもの。頂点から出る辺は、みな、たどる前から使ったことにする
                // （ほかの方向から回ってきて、頂点から出る辺に入らない）
                let amp = target - v;
                let mut used: HashSet<usize> = at.iter().copied().collect();
                for &e in &at {
                    let te = self.stored.edge_at(e);
                    let first = if te.a == v { te.quad() } else { reversed(te.quad()) };
                    let side = self.pull_side(e, first, &mut used);
                    self.push_pulled(&side, &PullWeights::new(&side), 0, 1.0, amp, &mut out);
                }
            }
        }
        Some(out)
    }

    /// 引っぱりの片側 `side`（外へ向けた曲線の並び。片側の `from` 番目から）の、動かした後の曲線（しまう向き）を `out` に足す。
    /// 曲線の始めの頂点の重みは、前の曲線の終わりの頂点の重み（先頭は `start_w`）
    fn push_pulled(
        &self,
        side: &[(usize, QuadBez)],
        w: &PullWeights,
        from: usize,
        start_w: f64,
        amp: kurbo::Vec2,
        out: &mut Vec<(usize, QuadBez)>,
    ) {
        let mut w0 = start_w;
        for (k, &(e, q)) in side.iter().enumerate() {
            let (wm, w2) = (w.mids[from + k], w.verts[from + k]);
            let m = QuadBez::new(q.p0 + amp * w0, q.p1 + amp * wm, q.p2 + amp * w2);
            let stored_forward = self.stored.edge_at(e).a == q.p0;
            out.push((e, if stored_forward { m } else { reversed(m) }));
            w0 = w2;
        }
    }

    /// 引っぱりの片側：辺 `e0` の、つかんだ所から外へ向けた曲線 `piece`（辺の上をつかんだときは切れ端、頂点をつかんだときは
    /// 辺全体）から、線を外へたどって、ついてくる辺を（外へ向けた曲線で）並べる（先頭は `piece`）。たどった辺は `used` に
    /// 入れ、入っている辺には進まない（閉じた線をぐるりと回って、ほかの方向に着いたとき）。両端が同じ辺（自分に戻る輪）は
    /// 無い（2次ベジェで両端が同じなら、行って同じ道を戻る線になり自分と重なるので、正しい地図に無い）。止まるのは、頂点につながる辺が2本でないとき（線の端・ほかの線と
    /// つながる所）と、つかんだ所から向きが `PULL_REACH_TURN` 以上回るとき（その頂点は動かない）。回った量は、右へ・左へ
    /// 曲がった量を打ち消さずに足したもの（符号付きで足すと、ゆらゆらした波の線は打ち消し合っていつまでも届かず、線全体が
    /// ついてきた。2次ベジェは1本の中で曲がる向きが変わらないので、1本の回った量は両端の向きの間の角）
    fn pull_side(
        &self,
        e0: usize,
        piece: QuadBez,
        used: &mut HashSet<usize>,
    ) -> Vec<(usize, QuadBez)> {
        let mut turn = turn_between(start_dir(piece), end_dir(piece)).abs();
        let mut side = vec![(e0, piece)];
        loop {
            let (cur, q) = *side.last().expect("先頭は切れ端");
            let v = q.p2;
            let at = self.edges_at_node(v);
            let [x, y] = at[..] else { break };
            let next = if x == cur { y } else { x };
            if used.contains(&next) {
                break;
            }
            let te = self.stored.edge_at(next);
            let nq = if te.a == v { te.quad() } else { reversed(te.quad()) };
            let joined = turn + turn_between(end_dir(q), start_dir(nq)).abs();
            if joined >= PULL_REACH_TURN {
                break;
            }
            turn = joined + turn_between(start_dir(nq), end_dir(nq)).abs();
            used.insert(next);
            side.push((next, nq));
        }
        side
    }

    /// 1本の道：道具が作った重ね合わせに渡すもの `change` で、地図を触った所だけで編集する（設計 段2b の 7. の 2b-7c と
    /// 8.）：窓で重ね（`window_overlay`）、差分を前の地図＋差分として確かめ（`LocalMap`）、触った面の書き戻しの差分を決め
    /// （`rewrite`）、前のしまう形に差分を当てた新しいしまう形と、新しい面の図形の表を作る。道具の種類では分かれない。地図
    /// 全体の一覧（`map`）を作らない（テストでは毎回、窓の答えを `overlay_diff` が、差分から先の答えを `apply_diff` が
    /// お手本と比べる）。`group_path`：この地図の階層
    pub(crate) fn edit_local(&self, change: &LocalChange<'_>, group_path: &[u64]) -> LocalEdit {
        match self.overlay_diff(change) {
            Ok(None) => LocalEdit::Same,
            Ok(Some((m, removed, added))) => self.apply_diff(&m, &removed, &added, group_path),
            Err(r) => LocalEdit::Refused(r),
        }
    }

    /// 重ね合わせの差分（`edit_local` が差分から先に渡すもの）：窓で重ね（`window_overlay`）、窓の結果と窓の辺の違いを
    /// 差分にする。（窓の結果, 消えた辺, 増えた辺）。重ねるものが無ければ `None`。`Err`：格子に置けない。テストでは毎回、
    /// 前のしまう形に差分を当てた形が、地図全体で重ねた地図（`overlay`。窓を使わない）と同じことを確かめる
    fn overlay_diff(&self, change: &LocalChange<'_>) -> Result<Option<LocalDiff>, LocalRefusal> {
        let got = self.overlay_diff_in_window(change);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                let full = self.map().overlay(change.input(), change.rule);
                match &got {
                    Err(_) => full.is_none(),
                    Ok(d) => full.is_some_and(|f| {
                        self.diffed(d.as_ref())
                            .same_content(&StoredMap::from_dense(&f, None))
                    }),
                }
            });
            assert!(ok, "窓で重ねた差分が、地図全体で重ねた地図（お手本）と違う");
        }
        got
    }

    fn overlay_diff_in_window(
        &self,
        change: &LocalChange<'_>,
    ) -> Result<Option<LocalDiff>, LocalRefusal> {
        if change.lines.quads.is_empty() {
            return Ok(None);
        }
        let ix = self.map_index();
        let (maps, wedges) =
            PlanarMap::window_overlay(ix, change.input(), &[change.rule], &change.lines)
                .ok_or(LocalRefusal::OffGrid)?;
        let [m]: [PlanarMap; 1] = maps.try_into().expect("決まりごとに1つの地図");
        let (removed, added) = self.stored.window_diff_tiles(&wedges, &m);
        Ok(Some((m, removed, added)))
    }

    /// 道具の側（矩形で切る）：選ぶ所 `region`（形か地図）で、この写しの地図を、中だけ残した地図（片。確かめる前の、窓の
    /// 結果）と、中を消した地図（残り。前のしまう形に窓の差分を当てたしまう形。確かめと書き戻しは、使う側がセル画の地図に
    /// 対して1本の道で行う：`cut_out`）に分ける。窓で重ねるのは1回（2つの決まりを一度に）。`None`：格子に置けない
    pub(crate) fn cut_local(&self, region: MapInput<'_>) -> Option<(PlanarMap, StoredMap)> {
        let mut lines = region.lines(|e| self.old_edge(e));
        if let MapInput::Shapes(_) = region {
            self.snap_ends(&mut lines.quads);
        }
        if lines.quads.is_empty() {
            return Some((PlanarMap::default(), self.stored.clone()));
        }
        let rules = [MapRule::KeepInside, MapRule::Paint(None)];
        let (maps, wedges) = PlanarMap::window_overlay(self.map_index(), region, &rules, &lines)?;
        let [inside, rest]: [PlanarMap; 2] = maps.try_into().expect("決まりごとに1つの地図");
        let (removed, added) = self.stored.window_diff_tiles(&wedges, &rest);
        let rest = self
            .stored
            .with_diff_tiles(&removed, &tile_edges_of(&rest, &added));
        // テストでは毎回：中だけ残した地図と、前のしまう形に残りの差分を当てた形が、地図全体で重ねた結果（`overlay_rules`）と
        // 同じ
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| match self.map().overlay_rules(region, rules) {
                Some([fi, fr]) => {
                    StoredMap::from_dense(&inside, None)
                        .same_content(&StoredMap::from_dense(&fi, None))
                        && rest.same_content(&StoredMap::from_dense(&fr, None))
                }
                None => false,
            });
            assert!(
                ok,
                "矩形で切る窓の答えが、地図全体で重ねた結果（お手本）と違う"
            );
        }
        Some((inside, rest))
    }

    /// 道具の側（片を置く・貼り付け）：地図 `p` を重ねる（`MapRule::Overlay`）とき、重ね合わせに渡すもの
    pub(crate) fn map_change<'a>(&self, p: &'a PlanarMap) -> LocalChange<'a> {
        LocalChange {
            input: ChangeInput::Map(p),
            lines: MapInput::Map(p).lines(|e| self.old_edge(e)),
            rule: MapRule::Overlay,
        }
    }

    /// この写しの地図に、地図 `maps` を下から順に重ねた地図のしまう形（`stacked` と同じ答え。1枚ずつ窓で重ね、差分を
    /// しまう形に当て、升目の索引も変わった所だけ作り直して次を重ねる）。地図が空なら、重ねる地図をそのまま使う
    /// （`stacked` と同じ）。`None`：格子に置けない・重ねる地図が無い（`maps` の中の `None`）
    pub(crate) fn stacked_local(
        self: &Arc<Self>,
        maps: impl IntoIterator<Item = Option<PlanarMap>>,
    ) -> Option<StoredMap> {
        // 重ねる写し（次の1枚を重ねるときに作る。最後の1枚のあとは、しまう形だけでよい）。1枚ずつの答えは、テストでは毎回
        // 地図全体で重ねた答えと比べる（`overlay_diff`）
        let mut cur: Arc<LayerBase> = Arc::clone(self);
        let mut stored = self.stored.clone();
        for m in maps {
            let m = m.as_ref()?;
            if !stored.same(&cur.stored) {
                cur = LayerBase::of_stored_after(&stored, &cur);
            }
            stored = if stored.edge_count() == 0 {
                StoredMap::from_dense(m, Some(&stored))
            } else {
                match cur.overlay_diff(&cur.map_change(m)) {
                    Ok(None) => continue,
                    Ok(Some((w, removed, added))) => cur
                        .stored
                        .with_diff_tiles(&removed, &tile_edges_of(&w, &added)),
                    Err(_) => return None,
                }
            };
        }
        Some(stored)
    }

    /// 道具の側（グループの変形）：この写し（階層 `group_path` の地図。文書の座標）に変換 `xform` をかけ、その座標の格子で
    /// 正しい地図にしたもの（`realized`）にする差分を、1本の道の差分から先に渡す（`replace_local`）。地図にできなければ
    /// 書けない
    pub(crate) fn transformed_local(&self, xform: Affine, group_path: &[u64]) -> LocalEdit {
        let Some(next) = realized(self.map(), xform) else {
            return LocalEdit::Refused(LocalRefusal::Unwritable);
        };
        self.replace_local(&StoredMap::from_dense(next.map(), Some(&self.stored)), group_path)
    }

    /// 道具の側（片を置く確定）：この写し（セル画の地図）の地図を、しまう形 `next`（セル画の地図と塊を共有するもの：
    /// `stacked_local`）にする差分を作り、1本の道の差分から先（`apply_diff`）に渡す。差分は、共有しない塊の分だけ比べて
    /// 作る（`StoredMap::diff_tiles`）
    pub(crate) fn replace_local(&self, next: &StoredMap, group_path: &[u64]) -> LocalEdit {
        let (removed, added) = self.stored.diff_tiles(next);
        let mut m = PlanarMap::default();
        let mut at: IntMap<[u64; 2], usize> = IntMap::default();
        let mut node = |p: Point, m: &mut PlanarMap| -> usize {
            *at.entry([p.x.to_bits(), p.y.to_bits()]).or_insert_with(|| {
                m.nodes.push(p);
                m.nodes.len() - 1
            })
        };
        for te in &added {
            let (a, b) = (node(te.a, &mut m), node(te.b, &mut m));
            m.edges.push(MapEdge {
                a,
                b,
                ctrl: te.ctrl,
                left: te.left,
                right: te.right,
                left_src: te.left_src,
                right_src: te.right_src,
                stroke: te.stroke,
            });
        }
        let added: Vec<usize> = (0..m.edges.len()).collect();
        self.apply_diff(&m, &removed, &added, group_path)
    }

    /// 道具の側（バケツ）：点 `p`（文書の座標）を含む面（色のある面、または同じ階層の線で囲まれた何も無い所）を `color` に
    /// する差分を作り、1本の道の差分から先（`apply_diff`）に渡す（設計 段2b の 9.）。テストでは毎回、窓で作った差分が、
    /// 同じ面の側を地図全体で塗り替えた地図（同じ `color_sides`）と同じになることを確かめる
    pub(crate) fn fill_local(&self, p: Point, color: Color32, group_path: &[u64]) -> LocalEdit {
        let diff = self.fill_diff(p, color, group_path);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                let whole = self.fill_sides(p, color, group_path).map(|halves| {
                    let mut m = self.map().clone();
                    m.color_sides(halves, color);
                    StoredMap::from_dense(&m, None)
                });
                diff.is_some() == whole.is_some()
                    && whole.is_none_or(|w| self.diffed(diff.as_ref()).same_content(&w))
            });
            assert!(
                ok,
                "バケツの差分が、地図全体で塗り替えた地図（お手本）と違う"
            );
        }
        match diff {
            None => LocalEdit::Same,
            Some((m, removed, added)) => self.apply_diff(&m, &removed, &added, group_path),
        }
    }

    /// 点 `p`（文書の座標）を含む面（`LocalMap::face_at`。バケツとインクボトルで同じ決まり）
    fn face_at_point(&self, p: Point, group_path: &[u64]) -> Option<FaceAt> {
        let empty = PlanarMap::default();
        let local = LocalMap::new(self.map_index(), &empty, &[], &[]);
        let faces_list = self.faces(group_path);
        let lookup = self.faces_lookup(&faces_list);
        let g = gpt(p);
        let mut memo = CycleMemo::default();
        local.face_at((2 * g.0, 2 * g.1), &lookup, &mut memo)
    }

    /// バケツで塗る面の、輪郭の向きのある辺（前の地図の番号）：点 `p`（文書の座標）を含む面（`face_at_point`）。
    /// 面が無いか、すでに `color` なら `None`
    fn fill_sides(&self, p: Point, color: Color32, group_path: &[u64]) -> Option<Vec<(usize, bool)>> {
        let (face_color, halves) = self.face_at_point(p, group_path)?;
        (face_color != Some(color)).then_some(halves)
    }

    /// バケツの差分（`fill_local`）：点を含む面（`LocalMap::face_at`）の輪郭の向きのある辺の側を `color` にし、両側が同じ色に
    /// なった辺を消して、2本だけがつながる頂点をまとめる（`color_sides`）。計算は、面の輪郭の辺と、その端の頂点に
    /// つながる辺と、消える辺のまわりの辺を並べた窓で行う。窓の範囲は、消える辺ごとに、その辺と端につながる辺の外形の
    /// マス（辺ごとに分けるので、離れた所にある消える辺の間のマスは入らない）。2本をまとめる所がほかの辺を調べた所が窓の
    /// 範囲から出たら、範囲を広げてやり直す（重ね合わせの窓と同じ決まり）。（窓の結果, 消えた辺, 増えた辺）。変わらなければ
    /// `None`
    fn fill_diff(&self, p: Point, color: Color32, group_path: &[u64]) -> Option<LocalDiff> {
        let halves = self.fill_sides(p, color, group_path)?;
        let ix = self.map_index();
        let stored = &self.stored;
        let edges_at = |p: Point| self.edges_at_node(p);
        let side_edges: std::collections::BTreeSet<usize> = halves.iter().map(|h| h.0).collect();
        // 消える辺（塗ったあとに両側が同じ色になる辺）ごとの、その辺と端につながる辺の外形
        let mut region = CellSet::default();
        region.add_rects(halves.iter().filter_map(|&(e, forward)| {
            let te = stored.edge_at(e);
            let other = if forward { te.right } else { te.left };
            (!edge_needed(Some(color), other, te.stroke)).then(|| {
                let hull = edges_at(te.a)
                    .into_iter()
                    .chain(edges_at(te.b))
                    .map(|f| hull_of(stored.edge_at(f).quad()))
                    .fold(hull_of(te.quad()), |a, b| a.union(b));
                window_rect(hull)
            })
        }));
        loop {
            let wedges = window_edges(ix, side_edges.iter().copied(), &region);
            let (mut m, _) = tile_window(ix, &wedges);
            let to_window: IntMap<usize, usize> =
                wedges.iter().enumerate().map(|(i, &e)| (e, i)).collect();
            let used = m.color_sides(halves.iter().map(|&(e, f)| (to_window[&e], f)), color);
            if !region.covers(&used) {
                region.add_set(&used);
                continue;
            }
            let (removed, added) = stored.window_diff_tiles(&wedges, &m);
            return Some((m, removed, added));
        }
    }

    /// 道具の側（インクボトル）：点 `p`（文書の座標）の線か面の輪郭（`stroke_targets`）の辺に、線 `stroke` を乗せる差分を
    /// 作り、1本の道の差分から先（`apply_diff`）に渡す。形は変わらないので、差分は線の替わる辺だけ（2本のまとめはしない：
    /// まとめる候補は操作が作った切れ目だけで、線を替えても切れ目はできない）。替わる辺が無ければ `Same`。テストでは毎回、
    /// 地図全体の同じ辺の線を替えた地図（お手本）と同じことを確かめる（設計：claude/線の設計_68a596e.md の 2.1）
    pub(crate) fn stroke_local(
        &self,
        p: Point,
        stroke: EdgeStroke,
        reach: EdgeGrab,
        group_path: &[u64],
    ) -> LocalEdit {
        let edges: Vec<usize> = self
            .stroke_targets(p, reach, group_path)
            .into_iter()
            .filter(|&e| self.stored.edge_at(e).stroke != Some(stroke))
            .collect();
        if edges.is_empty() {
            return LocalEdit::Same;
        }
        let (mut m, _) = tile_window(self.map_index(), &edges);
        for ed in &mut m.edges {
            ed.stroke = Some(stroke);
        }
        let (removed, added) = self.stored.window_diff_tiles(&edges, &m);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                let mut whole = self.map().clone();
                for &e in &edges {
                    whole.edges[e].stroke = Some(stroke);
                }
                self.diffed(Some(&(m.clone(), removed.clone(), added.clone())))
                    .same_content(&StoredMap::from_dense(&whole, None))
            });
            assert!(
                ok,
                "インクボトルの差分が、地図全体で線を替えた地図（お手本）と違う"
            );
        }
        self.apply_diff(&m, &removed, &added, group_path)
    }

    /// インクボトルで線を乗せる辺（計算に使う地図の番号）：点 `p` がいちばん近い辺の上か近く（線の上：線の太さの半分以内、
    /// または辺に合う距離 `reach` の中）なら、その辺1本（アンカーとアンカーの間。線を選ぶ単位と同じ）。そうでなければ、点を
    /// 含む色のある面の輪郭の辺。どちらでもなければ空
    fn stroke_targets(&self, p: Point, reach: EdgeGrab, group_path: &[u64]) -> Vec<usize> {
        let search = (*STROKE_WIDTHS.end() as f64 / 2.0).max(reach.filled).max(reach.empty);
        if let Some((e, t)) = self.hit_edge(p, search, Affine::IDENTITY) {
            let te = self.stored.edge_at(e);
            let q = te.quad();
            let at = q.eval(t);
            let d = at.distance(p);
            let tan = crate::vector_engine::quad_deriv(&q, t);
            let side = if (p - at).dot(kurbo::Vec2::new(-tan.y, tan.x)) >= 0.0 { te.left } else { te.right };
            let on_line = te.stroke.is_some_and(|s| d <= s.width as f64 / 2.0);
            if on_line || d <= reach.at(side.is_some()) {
                return vec![e];
            }
        }
        match self.face_at_point(p, group_path) {
            Some((Some(_), halves)) => {
                let mut edges: Vec<usize> = halves.iter().map(|h| h.0).collect();
                edges.sort_unstable();
                edges.dedup();
                edges
            }
            _ => Vec::new(),
        }
    }

    /// 道具の側（アンカーの追加・削除）：`edit` の差分を作り、1本の道の差分から先（`apply_diff`）に渡す（設計 段2b の 9.）。
    /// 差分は、関わる辺（追加なら分ける辺、削除なら頂点につながる辺）のまわりの窓の地図に、浮いた片の地図の編集と同じ
    /// `AnchorEdit::apply`（`with_anchor_added`・`with_anchor_deleted`）をかけて作る（置き換えてよいかの確かめ `can_replace_path` が調べるのは、関わる辺と
    /// 置き換える曲線の外形の範囲だけなので、その範囲を覆う窓で同じ答え）。編集できなければ（辺・頂点が無い・消せない頂点）
    /// `Same`。テストでは毎回、前のしまう形に差分を当てた形が、地図全体に同じ `AnchorEdit::apply` をかけた地図と同じ
    /// ことを確かめる
    pub(crate) fn anchor_local(&self, edit: AnchorEdit, group_path: &[u64]) -> LocalEdit {
        let diff = self.anchor_diff(edit);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| match (&diff, edit.apply(self.map())) {
                (None, None) => true,
                (Some(d), Some(fm)) => self
                    .diffed(Some(d))
                    .same_content(&StoredMap::from_dense(&fm, None)),
                _ => false,
            });
            assert!(
                ok,
                "アンカーの差分が、地図全体を編集した地図（お手本）と違う"
            );
        }
        match diff {
            None => LocalEdit::Same,
            Some((m, removed, added)) => self.apply_diff(&m, &removed, &added, group_path),
        }
    }

    /// アンカーの差分（`anchor_local`）：（窓の結果, 消えた辺, 増えた辺）。編集できなければ `None`
    fn anchor_diff(&self, edit: AnchorEdit) -> Option<LocalDiff> {
        let ix = self.map_index();
        let stored = &self.stored;
        // 関わる辺
        let involved: Vec<usize> = match edit {
            AnchorEdit::Add { orig_seg, .. } => vec![stored.find_edge(&orig_seg)?.0],
            AnchorEdit::Delete { orig_vertex } => self.edges_at_node(orig_vertex),
        };
        let hull = involved
            .iter()
            .map(|&e| hull_of(stored.edge_at(e).quad()))
            .reduce(|a, b| a.union(b))?;
        let mut region = CellSet::default();
        region.add_rects([window_rect(hull)]);
        let wedges = window_edges(ix, involved.iter().copied(), &region);
        let (wmap, _) = tile_window(ix, &wedges);
        // 窓の地図に、浮いた片の地図と同じ編集をかける
        let m = edit.apply(&wmap)?;
        let (removed, added) = stored.window_diff_tiles(&wedges, &m);
        Some((m, removed, added))
    }

    /// 前の地図の面の表（`BaseFaces`）：写しの面の図形 `faces_list` から、面の輪郭の最初の辺を形で引く
    fn faces_lookup<'f>(
        &'f self,
        faces_list: &'f FaceTable,
    ) -> impl Fn(ShapeId) -> Vec<(usize, bool)> + 'f {
        move |id: ShapeId| match faces_list.by_id(id) {
            Some(f) => f
                .contours()
                .filter_map(|c| c.segments.first().and_then(|q| self.stored.find_edge(q)))
                .collect(),
            None => Vec::new(),
        }
    }

    /// 1本の道の、差分から先（設計 段2b の 9.）：差分（`removed`：消えた前の地図の辺、`added`：地図 `result` の増えた辺の
    /// 番号。`result` の頂点は、座標が同じなら前の地図の頂点）を前の地図に当てて確かめ（`LocalMap`）、触った面の書き戻しの
    /// 差分を決め、前のしまう形に差分を当てた新しいしまう形と、新しい面の図形の表を作る。差分の作り方（重ね合わせ・バケツ・
    /// アンカー）や道具の種類では分かれない。テストでは毎回、同じ差分から先を地図全体を触った所として行った答え
    /// （`whole_touched`）と比べ、書けるかが本番の判定（`check`）と食い違わないことを確かめる（`matches_whole`）
    pub(crate) fn apply_diff(
        &self,
        result: &PlanarMap,
        removed: &[usize],
        added: &[usize],
        group_path: &[u64],
    ) -> LocalEdit {
        let got = self.apply_diff_inner(result, removed, added, group_path);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                let next = self
                    .stored
                    .with_diff_tiles(removed, &tile_edges_of(result, added));
                self.matches_whole(&next.to_dense(), group_path, &got)
            });
            assert!(
                ok,
                "差分から先を触った所だけで行った答えが、地図全体を触った所として行った答え（お手本）と違う"
            );
        }
        got
    }

    /// テスト用：1本の道の差分から先（`apply_diff_inner`）が確かめに使う、差分を前の地図に当てた地図（同じ `LocalMap::new`。
    /// どの決まりで断ったかをテストが数えるため）
    #[cfg(test)]
    pub(crate) fn local_map<'a>(
        &'a self,
        result: &'a PlanarMap,
        removed: &'a [usize],
        added: &'a [usize],
    ) -> LocalMap<'a> {
        LocalMap::new(self.map_index(), result, removed, added)
    }

    /// テスト用（お手本）：この地図を地図 `next` にする差分を、地図全体を触った所として（前の辺をみな消し、`next` の辺を
    /// みな足す）1本の道の差分から先に通した答え。触った所だけで行った答えは、これと同じになる
    #[cfg(test)]
    fn whole_touched(&self, next: &PlanarMap, group_path: &[u64]) -> LocalEdit {
        let removed: Vec<usize> = (0..self.stored.edge_count()).collect();
        let added: Vec<usize> = (0..next.edges.len()).collect();
        self.apply_diff_inner(next, &removed, &added, group_path)
    }

    /// テスト用：差分 `diff`（窓の結果, 消えた辺, 増えた辺）を前のしまう形に当てたしまう形（`None`：差分が無い。前のまま）
    #[cfg(test)]
    fn diffed(&self, diff: Option<&LocalDiff>) -> StoredMap {
        match diff {
            Some((m, removed, added)) => self
                .stored
                .with_diff_tiles(removed, &tile_edges_of(m, added)),
            None => self.stored.clone(),
        }
    }

    /// テスト用（お手本との比べ）：この地図を地図 `next` にする1本の道の答え `got` が、地図全体を触った所として行った答え
    /// （`whole_touched`）と同じか（書けるか・しまう形・消す図形・入れる図形・面の表。新しく振る番号は、入れる図形の形で
    /// 対応づけて比べる）。書けるかは、`next` が正しい地図か（本番の判定 `check`）とも食い違わない
    #[cfg(test)]
    fn matches_whole(&self, next: &PlanarMap, group_path: &[u64], got: &LocalEdit) -> bool {
        let valid = next.is_valid();
        let (w, f) = match (got, self.whole_touched(next, group_path)) {
            (LocalEdit::Same, LocalEdit::Same) => return valid,
            (LocalEdit::Refused(a), LocalEdit::Refused(b)) => return *a == b && !valid,
            (LocalEdit::Changed(w), LocalEdit::Changed(f)) if valid => (w, f),
            _ => return false,
        };
        let mut removed = (w.removed.clone(), f.removed.clone());
        removed.0.sort_unstable();
        removed.1.sort_unstable();
        if removed.0 != removed.1 {
            return false;
        }
        // 入れる図形を形で対応づける（前の図形の番号を引き継いだ図形は、番号まで同じ）
        let shape_key = |s: &FillShape| {
            let bits = |c: &Contour| -> Vec<[u64; 6]> {
                c.segments
                    .iter()
                    .map(|q| [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y].map(f64::to_bits))
                    .collect()
            };
            (
                s.color.to_array(),
                s.contours().map(bits).collect::<Vec<_>>(),
            )
        };
        let own = self.faces(group_path);
        let whole_by_key: HashMap<_, ShapeId> = f
            .added
            .iter()
            .filter_map(|&id| f.faces.by_id(id))
            .map(|s| (shape_key(s), s.shape_id))
            .collect();
        if whole_by_key.len() != w.added.len() || f.added.len() != w.added.len() {
            return false;
        }
        let mut rename: HashMap<ShapeId, ShapeId> = HashMap::new();
        for id in &w.added {
            let Some(&fid) = w
                .faces
                .by_id(*id)
                .and_then(|s| whole_by_key.get(&shape_key(s)))
            else {
                return false;
            };
            if own.by_id(fid).is_some() || own.by_id(*id).is_some() {
                if fid != *id {
                    return false;
                }
            } else {
                rename.insert(*id, fid);
            }
        }
        let renamed = |id: ShapeId| rename.get(&id).copied().unwrap_or(id);
        let mut ids: Vec<ShapeId> = w.faces.iter().map(|s| renamed(s.shape_id)).collect();
        ids.sort_unstable();
        ids.iter().copied().eq(f.faces.iter().map(|s| s.shape_id))
            && w.stored
                .relabeled(renamed, &mut Default::default())
                .same_content(&f.stored)
    }

    fn apply_diff_inner(
        &self,
        result: &PlanarMap,
        removed: &[usize],
        added: &[usize],
        group_path: &[u64],
    ) -> LocalEdit {
        if removed.is_empty() && added.is_empty() {
            return LocalEdit::Same;
        }
        let ix = self.map_index();
        let local = LocalMap::new(ix, result, removed, added);
        if !local.added_edges_ok() {
            return LocalEdit::Refused(LocalRefusal::Unwritable);
        }
        let mut memo = CycleMemo::default();
        let Some(touched) = local.touched_cycles_ok(&mut memo) else {
            return LocalEdit::Refused(LocalRefusal::Unwritable);
        };
        let faces_list = self.faces(group_path);
        let lookup = self.faces_lookup(&faces_list);
        let Some(Some(faces)) = local.regions_faces(&touched, &lookup, &mut memo) else {
            return LocalEdit::Refused(LocalRefusal::Unwritable);
        };
        let orig_shape = |id: ShapeId| -> Option<Arc<FillShape>> { faces_list.by_id(id).cloned() };
        let change = local.rewrite(&faces, &lookup, &orig_shape, &memo, 0, group_path);
        let (relabel, add_edges) = local.relabeled_edges(&faces, &change.ids, &memo);
        let mut all_removed = removed.to_vec();
        all_removed.extend(relabel);
        all_removed.sort_unstable();
        all_removed.dedup();
        // 面も辺の中身も変わらなければ、変わらない（線だけの辺は、面を変えずに増える・消える・線が替わる：設計
        // claude/線の設計_68a596e.md の 6.2）
        if change.removed.is_empty()
            && change.added.is_empty()
            && !self.stored.diff_changes(&all_removed, &add_edges)
        {
            return LocalEdit::Same;
        }
        let stored = self.stored.with_diff_tiles(&all_removed, &add_edges);
        let added: Vec<Arc<FillShape>> = change
            .added
            .into_iter()
            .map(|mut s| {
                s.strip_transient_caches();
                Arc::new(s)
            })
            .collect();
        // 新しい面の表：消した面と入れた面のかかる塊だけを作り直し、ほかの塊は前の表と共有する
        let mut gone = change.removed.clone();
        gone.sort_unstable();
        let mut sorted_added = added.clone();
        sorted_added.sort_unstable_by_key(|s| s.shape_id);
        let mut faces_new = faces_list.clone();
        faces_new.replace_sorted(&gone, sorted_added, |s| s.shape_id);
        LocalEdit::Changed(LocalWrite {
            stored,
            faces: faces_new,
            removed: change.removed,
            added: added.iter().map(|s| s.shape_id).collect(),
        })
    }

    /// 重ねる形の線の端を、前からある頂点から `FLAT_TOL` 以内なら、その頂点（いちばん近いもの：`NearPoints`。`overlay` の
    /// 形の線の端を寄せる所と同じ）に寄せる。頂点は塊から引き、端の点ごとに、そのまわりの端の升目の索引の升目にある辺の
    /// 端だけを見る（線の端の点みなの外接矩形はなめない。手間は端の点の数とまわりの中身で決まる：レビュー 96fdc26 の
    /// N-1004）
    fn snap_ends(&self, quads: &mut [QuadBez]) {
        let ix = self.map_index();
        let mut edges: Vec<usize> = Vec::new();
        for p in quads.iter().flat_map(|q| [q.p0, q.p2]) {
            let [lo, hi] = end_key_span(KRect::from_points(p, p).inflate(FLAT_TOL, FLAT_TOL));
            ix.ends
                .edges_over(&self.stored, lo, hi, |_, es| edges.extend(es));
        }
        edges.sort_unstable();
        edges.dedup();
        let pts = edges.iter().flat_map(|&e| {
            let te = self.stored.edge_at(e);
            [te.a, te.b]
        });
        let near = NearPoints::new(pts, FLAT_TOL);
        let snap = |p: Point| near.nearest(p).unwrap_or(p);
        for q in quads.iter_mut() {
            q.p0 = snap(q.p0);
            q.p2 = snap(q.p2);
        }
    }

    /// 升目の索引（初めて要るときに全部作る）
    pub(crate) fn index(&self) -> &crate::stored_map::TileIndex {
        self.index.get_or_init(|| {
            crate::stored_map::TileIndex::build(&self.stored, |e| self.edge_tiles(e))
        })
    }

    /// 重ね合わせに渡す、この写しの升目の索引
    fn map_index(&self) -> MapIndex<'_> {
        MapIndex {
            index: self.index(),
            ends: self.ends_index(),
            stored: &self.stored,
            hold: &self.hold,
        }
    }

    /// カーソル合わせ：地図を座標の変換 `xform` で置いたとき、点 `p`（置いた座標）から `radius` 以内でいちばん近い頂点の
    /// 位置（文書の座標。距離は置いた座標で測る）。頂点は、点のまわりの端の升目の索引にある辺の端から引く（地図全体の写しを
    /// 作らない：設計 段2b の 7. の 2b-7d。引くのは辺の入っている升目だけなので、小さく縮めて置いた地図で文書の座標の
    /// 範囲が広くなっても、手間はまわりの中身で決まる：`TileIndex::edges_over`）。同じ距離の頂点は、座標の順で先のもの
    pub(crate) fn hit_node(&self, p: Point, radius: f64, xform: Affine) -> Option<Point> {
        // 潰れた変換（逆にできない：`invertible`）で置いた地図は描かない（面積 0 の絵は見えない）ので、何にも合わない
        if !crate::vector_engine::invertible(xform) {
            return None;
        }
        let area = KRect::from_points(p, p).inflate(radius, radius);
        let doc_area = xform.inverse().transform_rect_bbox(area);
        let ix = self.map_index();
        let mut edges: Vec<usize> = Vec::new();
        let [lo, hi] = end_key_span(doc_area);
        ix.ends
            .edges_over(ix.stored, lo, hi, |_, es| edges.extend(es));
        let nearest = |edges: &mut dyn Iterator<Item = usize>| {
            let pts = edges.flat_map(|e| {
                let te = self.stored.edge_at(e);
                [te.a, te.b]
            });
            nearest_of(pts, radius, |v| {
                (xform * v - p).hypot()
            })
        };
        let got = nearest(&mut edges.into_iter());
        // お手本：同じ決まり（`nearest_of`）に、地図全体の辺の端を渡した答えと同じ（索引で集めた頂点に、半径の中の頂点が
        // みなある）
        #[cfg(test)]
        if test_checks() {
            let whole = uncounted(|| nearest(&mut (0..self.stored.edge_count())));
            assert_eq!(
                got, whole,
                "索引で引いた近い頂点が、地図全体の頂点で引いた答え（お手本）と違う"
            );
        }
        got
    }

    /// 頂点 `p`（文書の座標）を端に持つ辺（計算に使う地図での番号の順）。端の升目の索引で引く
    pub(crate) fn edges_at_node(&self, p: Point) -> Vec<usize> {
        self.map_index().edges_at_node(p)
    }

    /// 面と線（階層 `group_path`）のうち、番号が `ids` にあるものがあるか（面の表と線の図形で引く。線は面と別に選ぶ：設計
    /// claude/線の設計_68a596e.md の 6.4）
    pub(crate) fn has_any_face(&self, ids: &HashSet<ShapeId>, group_path: &[u64]) -> bool {
        let faces = self.faces(group_path);
        let lines = self.lines(group_path);
        ids.iter().any(|&id| {
            faces.by_id(id).is_some()
                || (is_line_id(id) && lines.find_sorted(id, |s| s.shape_id).is_some())
        })
    }

    /// 番号が `ids` の面の輪郭の辺（計算に使う地図での番号の順）：面の表の図形の輪郭の曲線を、塊から引く（手間は、その
    /// 面の大きさの分）
    fn faces_edges(&self, ids: &HashSet<ShapeId>, group_path: &[u64]) -> Vec<usize> {
        let faces = self.faces(group_path);
        let mut out: Vec<usize> = Vec::new();
        for id in ids {
            let Some(f) = faces.by_id(*id) else {
                continue;
            };
            for q in f.contours().flat_map(|c| c.segments.iter()) {
                if let Some((e, _)) = self.stored.find_edge(q) {
                    out.push(e);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 番号が `ids` の面だけを残した、計算に使うふつうの地図（`PlanarMap::restricted` と同じ地図：辺と頂点の順も同じ）。
    /// 残す面の輪郭の辺だけを塊から引いて作る（手間は残す面の大きさの分）
    pub(crate) fn only_faces_map(&self, ids: &HashSet<ShapeId>, group_path: &[u64]) -> PlanarMap {
        let mut m = PlanarMap::default();
        let mut at: HashMap<[u64; 2], usize> = HashMap::new();
        let mut node = |p: Point, m: &mut PlanarMap| -> usize {
            *at.entry([p.x.to_bits(), p.y.to_bits()]).or_insert_with(|| {
                m.nodes.push(p);
                m.nodes.len() - 1
            })
        };
        for e in self.selected_edges(ids, group_path) {
            let te = self.stored.edge_at(e);
            let Some(([l, r], stroke)) = restricted_sides(
                [(te.left, te.left_src), (te.right, te.right_src)],
                te.stroke,
                line_id_of(&te),
                |id| ids.contains(&id),
            ) else {
                continue;
            };
            let (a, b) = (node(te.a, &mut m), node(te.b, &mut m));
            m.edges.push(MapEdge {
                a,
                b,
                ctrl: te.ctrl,
                left: l.0,
                right: r.0,
                left_src: l.1,
                right_src: r.1,
                stroke,
            });
        }
        m
    }

    /// この写しの地図の、番号が `ids` の面だけを残した地図（`only_faces_map`。`None` なら地図全体）を、確かめた地図にする。
    /// 確かめ直さない：写しの地図はいつも正しい地図で、面に1つずつ別の番号が記してある（写しを作るのは、確かめた地図の
    /// しまう形からだけ：セル画の地図は使う入口 `ScopeMap::stored`、片の組は `PieceGroup::new`、残りは1本の道の答え）。
    /// 正しい地図の面を丸ごと残した一部は、そのまま正しい地図（`PlanarMap::restricted`。`ScopeMap::restricted` と同じ
    /// 決まり）。面は残した地図から取り出す（地図全体のときは、写しの判定の形と輪郭から）。テストでは毎回、確かめた地図
    /// （`checked`）と同じことを確かめる
    pub(crate) fn checked_part(
        &self,
        ids: Option<&HashSet<ShapeId>>,
        group_path: &[u64],
    ) -> CheckedMap {
        let (map, faces) = match ids {
            None => (self.map().clone(), self.map_faces().to_vec()),
            Some(ids) => {
                let map = self.only_faces_map(ids, group_path);
                let (faces, _) = map.faces();
                (map, faces)
            }
        };
        // お手本：確かめた地図（`checked`）と同じ
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| map.clone().checked().is_some_and(|c| c.faces == faces));
            assert!(ok, "正しい地図の一部が、確かめた地図（お手本）と違う");
        }
        CheckedMap { map, faces }
    }

    /// 面を丸ごと外した・残した地図のしまう形（`PlanarMap::restricted_by` と同じ地図：外す面の側の色と番号を消し、両側とも
    /// 消えた辺を除く。正しい地図の面を外しても、正しい地図のまま）を、触った所だけで作る。外すときは、外す面の輪郭の辺
    /// だけを替える（前のしまう形に差分を当てる。書くのは、この地図に対する差分として1本の道に渡す：`ScopeMap::restricted`）。
    /// 残すときは、残す面の輪郭の辺だけで新しい地図を作る（取り出し。塊は前のしまう形と共有する）。面が残らなければ `None`
    pub(crate) fn restricted_local(
        &self,
        keep: FaceKeep<'_>,
        group_path: &[u64],
    ) -> Option<StoredMap> {
        let stored = match keep {
            FaceKeep::Without(ids) => {
                let touched = self.selected_edges(ids, group_path);
                let added: Vec<crate::stored_map::TileEdge> = touched
                    .iter()
                    .filter_map(|&e| {
                        let te = self.stored.edge_at(e);
                        let ([l, r], stroke) = restricted_sides(
                            [(te.left, te.left_src), (te.right, te.right_src)],
                            te.stroke,
                            line_id_of(&te),
                            |id| keep.keeps(id),
                        )?;
                        Some(crate::stored_map::TileEdge {
                            left: l.0,
                            right: r.0,
                            left_src: l.1,
                            right_src: r.1,
                            stroke,
                            ..te
                        })
                    })
                    .collect();
                self.stored.with_diff_tiles(&touched, &added)
            }
            FaceKeep::Only(ids) => {
                StoredMap::from_dense(&self.only_faces_map(ids, group_path), Some(&self.stored))
            }
        };
        let got = (!stored.is_empty()).then_some(stored);
        // お手本：地図全体で面を外した・残した地図（`restricted_by`）と同じ
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                let full = self.map().restricted_by(|id| keep.keeps(id));
                match &got {
                    None => full.edges.is_empty(),
                    Some(stored) => stored.same_content(&StoredMap::from_dense(&full, None)),
                }
            });
            assert!(
                ok,
                "面を外した・残した地図が、地図全体で外した・残した地図（お手本）と違う"
            );
        }
        got
    }

    /// アンカーの追加・削除 `edit` ができるか（`anchor_diff` が差分を作れるか：今の `with_anchor_added`・
    /// `with_anchor_deleted` が形を返すか）
    pub(crate) fn anchor_possible(&self, edit: AnchorEdit) -> bool {
        self.anchor_diff(edit).is_some()
    }

    /// カーソル合わせ：地図を座標の変換 `xform` で置いたとき、`p` から `r` 以内でいちばん近い辺と、その上のパラメータ
    /// （距離は置いた座標で測る。パラメータはアフィン変換で変わらない）。見る辺は、点のまわりの升目にかかる辺だけ（`p` から
    /// `r` 以内を通る辺は、その所を含むマスの升目にある。引くのは辺の入っている升目だけ：`TileIndex::edges_over`）
    pub(crate) fn hit_edge(&self, p: Point, r: f64, xform: Affine) -> Option<(usize, f64)> {
        // 潰れた変換で置いた地図は描かないので、何にも合わない（`hit_node` と同じ）
        if !crate::vector_engine::invertible(xform) {
            return None;
        }
        let near = KRect::from_center_size(p, (2.0 * r, 2.0 * r));
        let ix = self.map_index();
        let mut cands: Vec<usize> = Vec::new();
        let [lo, hi] = index_tile_span(xform.inverse().transform_rect_bbox(near));
        ix.index
            .edges_over(ix.stored, lo, hi, |_, es| cands.extend(es));
        cands.sort_unstable();
        cands.dedup();
        // 辺（番号の順）のうち、いちばん近いもの。辺は塊から引く
        let nearest = |cands: &mut dyn Iterator<Item = usize>| {
            cands
                .map(|e| (e, xform * self.stored.edge_at(e).quad()))
                .filter(|(_, q)| rects_overlap(q.bounding_box(), near))
                .map(|(e, q)| {
                    let (t, d) = nearest_on_quad(&q, p);
                    (e, t, d)
                })
                .filter(|x| x.2 <= r)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(e, t, _)| (e, t))
        };
        let got = nearest(&mut cands.into_iter());
        // お手本：同じ決まりに、地図全体の辺を渡した答えと同じ（索引で集めた辺に、半径の中を通る辺がみなある）
        #[cfg(test)]
        if test_checks() {
            let whole = uncounted(|| nearest(&mut (0..self.stored.edge_count())));
            assert_eq!(
                got, whole,
                "索引で引いた近い辺が、地図全体の辺で引いた答え（お手本）と違う"
            );
        }
        got
    }

    /// 辺ごとの判定の形（`map` の辺の順。初めて要るときに作る。編集で作った写しは、確かめたときのものを引き継いでいる）
    fn polys(&self) -> &[Poly] {
        self.polys.get_or_init(|| {
            (0..self.map().edges.len())
                .map(|e| Arc::clone(self.stored_poly(e)))
                .collect()
        })
    }

    /// 面の輪郭（`face_cycles_with`。初めて要るときに作る。編集で作った写しは、確かめたときのものを引き継いでいる）
    pub(crate) fn cycles(&self) -> &[Vec<(usize, bool)>] {
        self.cycles
            .get_or_init(|| self.map().face_cycles_with(self.polys()))
    }

    /// 面（`faces_of`。初めて要るときに、この写しの判定の形と輪郭から作る。読んだ地図の確かめで作った面があればそれ）
    fn map_faces(&self) -> &[MapFace] {
        self.map_faces
            .get_or_init(|| self.map().faces_of(self.polys(), self.cycles()).0)
    }

    /// 読んだ地図（この写しのしまう形。まだ確かめていない）の確かめ：正しい地図なら（判定は `PlanarMap::checked` と同じ
    /// `check_with`。判定の形と面の輪郭は、この写しのものを使い、この写しに残る）、面に1つずつ別の番号を記したしまう形
    /// （`CheckedMap::uniquely_labeled`）。番号が変わらなければ（アプリが書いた地図はみなそう）、この写しのしまう形
    /// そのもので、作った面もこの写しに残す（面の図形はそこから作る：初めて見るときの計算が1回で済む。レビュー 983236e
    /// の N-6001）。変われば、その塊を共有して作る。`None`：正しい地図でない
    pub(crate) fn checked_stored(&self) -> Option<StoredMap> {
        let map = self.map();
        let faces = map.check_with(|| self.polys(), |_| self.cycles())?;
        let labeled = CheckedMap {
            map: map.clone(),
            faces,
        }
        .uniquely_labeled();
        let same = labeled
            .map
            .edges
            .iter()
            .zip(&map.edges)
            .all(|(a, b)| (a.left_src, a.right_src) == (b.left_src, b.right_src));
        if !same {
            return Some(StoredMap::from_dense(&labeled.map, Some(&self.stored)));
        }
        self.map_faces.get_or_init(|| labeled.faces);
        Some(self.stored.clone())
    }

    /// テスト用：持っている判定の形と面の輪郭（引き継いだもの）が、地図から作り直したものと同じか。まだ持っていなければ
    /// `None`
    #[cfg(test)]
    pub(crate) fn derived_match_full(&self) -> Option<bool> {
        let (polys, cycles) = (self.polys.get()?, self.cycles.get()?);
        let fresh = self.map().polys();
        let fresh_cycles = self.map().face_cycles_with(&fresh);
        Some(
            polys.len() == fresh.len()
                && polys.iter().zip(&fresh).all(|(a, b)| a[..] == b[..])
                && *cycles == fresh_cycles,
        )
    }

    /// 面の図形（写し。階層 `group_path`）。面の順で、図形の番号は面の印（セル画の地図の面には、いつも1つずつ別の番号が
    /// 記してある）。`layer_id` は付けない（使う側が付ける）
    pub(crate) fn faces(&self, group_path: &[u64]) -> FaceTable {
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

    /// 線の図形の表（階層 `group_path` の文書の座標。写しに控える）：線を乗せた辺1本ずつ（アンカーとアンカーの間）を、
    /// 太らせた形の図形にしたもの（番号は `line_id_of`。番号の順）。同じ地図の面の図形を全部描いたあとに描く（線は塗りより
    /// 上：`drawn_shapes`）。前の写しの表があれば、前の地図との差分（`StoredMap::diff_tiles`：共有しない塊の中で、中身の
    /// 変わった辺）の分だけを替えて作る（手間は変わった所の分。消えた辺の線を外し、増えた辺の線を足す。番号の同じ図形は
    /// 使い回す）。無ければ（初めて見る地図）、線を乗せた辺をみな図形にする（設計：claude/線の設計_68a596e.md の 6.3）
    pub(crate) fn lines(&self, group_path: &[u64]) -> LineTable {
        let mut cached = self.lines.lock();
        if let Some((gp, table)) = &*cached {
            if gp == group_path {
                return table.clone();
            }
        }
        let prev = self.prev_lines.lock().take().filter(|((gp, _), _)| gp == group_path);
        let table = match prev {
            Some(((_, mut table), prev_stored)) => {
                let (removed, added) = prev_stored.diff_tiles(&self.stored);
                // 作り直す線：増えた辺と、消えた辺・増えた辺の端につながる線（つなぎ目の形が変わりうる）
                let redo = self.lines_near(&removed.iter().map(|&e| prev_stored.edge_at(e)).collect::<Vec<_>>(), &added);
                let mut gone: Vec<ShapeId> = removed
                    .iter()
                    .filter_map(|&e| line_id_of(&prev_stored.edge_at(e)))
                    .chain(redo.iter().filter_map(line_id_of))
                    .collect();
                gone.sort_unstable();
                gone.dedup();
                let mut new: Vec<Arc<FillShape>> = redo
                    .iter()
                    .filter_map(|te| self.line_shape_at(te, group_path).map(Arc::new))
                    .collect();
                new.sort_unstable_by_key(|s| s.shape_id);
                new.dedup_by_key(|s| s.shape_id);
                table.replace_sorted(&gone, new, |s| s.shape_id);
                // お手本：地図全体の線を乗せた辺から作った表と、番号と形（輪郭のビット）が同じ
                #[cfg(test)]
                if test_checks() {
                    let ok = uncounted(|| {
                        let sig = |s: &FillShape| -> (ShapeId, Vec<[u64; 6]>) {
                            let bits = s
                                .contours()
                                .flat_map(|c| c.segments.iter())
                                .map(|q| [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y].map(f64::to_bits))
                                .collect();
                            (s.shape_id, bits)
                        };
                        let whole: Vec<_> = self.whole_lines(group_path).iter().map(|s| sig(s)).collect();
                        let got: Vec<_> = table.iter().map(|s| sig(s)).collect();
                        whole == got
                    });
                    assert!(ok, "変わった所だけで作った線の図形の表が、地図全体から作った表（お手本）と違う");
                }
                table
            }
            None => self.whole_lines(group_path).into(),
        };
        *cached = Some((group_path.to_vec(), table.clone()));
        table
    }

    /// 地図全体の線を乗せた辺から作った線の図形（番号の順。初めて見る地図）。端の形は、線を乗せた辺の端の表で決める
    fn whole_lines(&self, group_path: &[u64]) -> Vec<Arc<FillShape>> {
        let stroked = self.stored.stroked_edges();
        let key = |p: Point| [p.x.to_bits(), p.y.to_bits()];
        let mut at: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
        for (i, (_, te)) in stroked.iter().enumerate() {
            at.entry(key(te.a)).or_default().push(i);
            at.entry(key(te.b)).or_default().push(i);
        }
        let others = |p: Point| -> Vec<crate::stored_map::TileEdge> {
            at.get(&key(p)).map_or_else(Vec::new, |v| v.iter().map(|&i| stroked[i].1).collect())
        };
        let mut shapes: Vec<Arc<FillShape>> = stroked
            .iter()
            .filter_map(|(_, te)| line_shape(te, line_ends(te, others), line_id_of(te)?, group_path).map(Arc::new))
            .collect();
        shapes.sort_unstable_by_key(|s| s.shape_id);
        shapes.dedup_by_key(|s| s.shape_id);
        shapes
    }

    /// この地図の線を乗せた辺 `te` の線の図形（端の形は、頂点の辺の索引で引いたまわりの線で決める：`line_ends`）
    fn line_shape_at(&self, te: &crate::stored_map::TileEdge, group_path: &[u64]) -> Option<FillShape> {
        let others = |p: Point| -> Vec<crate::stored_map::TileEdge> {
            self.edges_at_node(p)
                .into_iter()
                .map(|e| self.stored.edge_at(e))
                .filter(|e| e.stroke.is_some())
                .collect()
        };
        line_shape(te, line_ends(te, others), line_id_of(te)?, group_path)
    }

    /// 前の地図から消えた辺 `gone` と増えた辺 `added`（この地図の辺）があるとき、線の図形を作り直す、この地図の線を乗せた辺：
    /// 増えた線と、消えた線・増えた線の端につながる線（つなぎ目の形が変わりうる）。線の端の形は、その点の線を乗せた辺だけで
    /// 決まる（`line_ends`）ので、線の無い辺が消えても増えても、まわりの線は作り直さない。頂点の辺は索引で引く（手間は変わった
    /// 線の分）
    fn lines_near(
        &self,
        gone: &[crate::stored_map::TileEdge],
        added: &[crate::stored_map::TileEdge],
    ) -> Vec<crate::stored_map::TileEdge> {
        let mut out: Vec<crate::stored_map::TileEdge> = added.iter().filter(|e| e.stroke.is_some()).copied().collect();
        let mut seen: IntSet<usize> = IntSet::default();
        let lined = gone.iter().chain(added).filter(|e| e.stroke.is_some());
        for p in lined.flat_map(|e| [e.a, e.b]) {
            for e in self.edges_at_node(p) {
                if seen.insert(e) {
                    let te = self.stored.edge_at(e);
                    if te.stroke.is_some() {
                        out.push(te);
                    }
                }
            }
        }
        out
    }

    /// 番号が `id` の線の図形の辺（計算に使う地図の番号。線の図形の表で引き、その曲線の辺を塊から引く）
    fn line_edge(&self, id: ShapeId, group_path: &[u64]) -> Option<usize> {
        let lines = self.lines(group_path);
        let s = lines.find_sorted(id, |s| s.shape_id)?;
        let q = s.center_curve()?;
        Some(self.stored.find_edge(&q)?.0)
    }

    /// 番号が `ids` の線（階層 `group_path`）を持ち上げて動かす間の線の図形：（動かす線, 動かす線の端につながる残る線）。線の
    /// 端の形は隣の線で決まる（`line_ends`）ので、動かす線は動かす線どうしだけを、残る線は残る線どうしだけを隣として作り
    /// 直す（持ち上げて確定したときの、浮いた片と残りの地図の線と同じ形）。番号は変わらない。手間は動かす線と、その端に
    /// つながる線の分（頂点の辺は索引で引く）
    pub(crate) fn detached_lines(
        &self,
        ids: &HashSet<ShapeId>,
        group_path: &[u64],
    ) -> (Vec<FillShape>, Vec<FillShape>) {
        let moving: Vec<crate::stored_map::TileEdge> = ids
            .iter()
            .filter(|&&id| is_line_id(id))
            .filter_map(|&id| Some(self.stored.edge_at(self.line_edge(id, group_path)?)))
            .collect();
        let is_moving = |e: &crate::stored_map::TileEdge| line_id_of(e).is_some_and(|id| ids.contains(&id));
        // 点 `p` に端のある線を乗せた辺のうち、動かす線か（`moved`）、残る線か
        let at = |p: Point, moved: bool| -> Vec<crate::stored_map::TileEdge> {
            self.edges_at_node(p)
                .into_iter()
                .map(|e| self.stored.edge_at(e))
                .filter(|e| e.stroke.is_some() && is_moving(e) == moved)
                .collect()
        };
        let shape = |te: &crate::stored_map::TileEdge, moved: bool| {
            line_shape(te, line_ends(te, |p| at(p, moved)), line_id_of(te)?, group_path)
        };
        let moved = moving.iter().filter_map(|te| shape(te, true)).collect();
        let mut seen: HashSet<ShapeId> = HashSet::new();
        let staying = moving
            .iter()
            .flat_map(|te| [te.a, te.b])
            .flat_map(|p| at(p, false))
            .filter(|te| line_id_of(te).is_some_and(|id| seen.insert(id)))
            .filter_map(|te| shape(&te, false))
            .collect();
        (moved, staying)
    }

    /// この地図を地図 `next_base`（この地図に差分を当てたものの写し）にしたときの、線の図形の変わり方：消えた線の番号と、
    /// 増えた線の図形（`next_base` の表から）。手間は変わった所の分（`StoredMap::diff_tiles`）
    pub(crate) fn line_changes(
        &self,
        next_base: &LayerBase,
        group_path: &[u64],
    ) -> (Vec<ShapeId>, Vec<Arc<FillShape>>) {
        let (removed, added) = self.stored.diff_tiles(&next_base.stored);
        let removed: Vec<crate::stored_map::TileEdge> = removed.iter().map(|&e| self.stored.edge_at(e)).collect();
        // 作り直した線（増えた辺と、端につながる線）は、前の線を隠して新しい線を出す（番号が同じでも、つなぎ目の形が変わる）
        let redo = next_base.lines_near(&removed, &added);
        let then = next_base.lines(group_path);
        let gone = removed
            .iter()
            .chain(&redo)
            .filter_map(line_id_of)
            .collect();
        let made = redo
            .iter()
            .filter_map(|te| then.find_sorted(line_id_of(te)?, |s| s.shape_id).cloned())
            .collect();
        (gone, made)
    }

    /// ダブルクリックで一緒に選ぶもの（階層 `group_path`）：線の番号 `id` なら、その線の辺と、端でつながった同じ線の辺を
    /// たどったもの（頂点の辺は索引で引く：`edges_at_node`。手間はつながった線の分）。面の番号なら、その面と、輪郭の辺の線
    /// （Animate と同じ：設計 claude/線の設計_68a596e.md の 6.4）
    pub(crate) fn connected_ids(&self, id: ShapeId, group_path: &[u64]) -> HashSet<ShapeId> {
        let mut out: HashSet<ShapeId> = HashSet::from([id]);
        if !is_line_id(id) {
            for e in self.faces_edges(&HashSet::from([id]), group_path) {
                out.extend(line_id_of(&self.stored.edge_at(e)));
            }
            return out;
        }
        let Some(start) = self.line_edge(id, group_path) else {
            return out;
        };
        let stroke = self.stored.edge_at(start).stroke;
        let mut seen: IntSet<usize> = IntSet::default();
        seen.insert(start);
        let mut todo = vec![start];
        while let Some(f) = todo.pop() {
            let te = self.stored.edge_at(f);
            for p in [te.a, te.b] {
                for g in self.edges_at_node(p) {
                    let ge = self.stored.edge_at(g);
                    if ge.stroke == stroke && seen.insert(g) {
                        out.extend(line_id_of(&ge));
                        todo.push(g);
                    }
                }
            }
        }
        out
    }

    /// 番号 `ids`（面の番号と線の番号）の面の輪郭の辺と線の辺（計算に使う地図の番号。小さい順・重なり無し）
    fn selected_edges(&self, ids: &HashSet<ShapeId>, group_path: &[u64]) -> Vec<usize> {
        let mut out = self.faces_edges(ids, group_path);
        if ids.iter().any(|&id| is_line_id(id)) {
            out.extend(
                ids.iter()
                    .filter(|&&id| is_line_id(id))
                    .filter_map(|&id| self.line_edge(id, group_path)),
            );
            out.sort_unstable();
            out.dedup();
        }
        out
    }

    /// 描く図形：面の図形を全部並べたあとに、線の図形を並べたもの（線は塗りより上）
    pub(crate) fn drawn_shapes(&self, group_path: &[u64]) -> Vec<Arc<FillShape>> {
        let mut out: Vec<Arc<FillShape>> = self.faces(group_path).iter().cloned().collect();
        out.extend(self.lines(group_path).iter().cloned());
        out
    }

    /// 面の図形を地図から作る（`faces`。番号の順）
    fn face_shapes(&self, group_path: &[u64]) -> FaceTable {
        let faces = self.map_faces();
        let ids = face_ids(self.map(), faces, FaceIds::Keep);
        let mut shapes: Vec<Arc<FillShape>> = faces
            .iter()
            .zip(&ids)
            .map(|(f, &id)| {
                let mut s = self.map().face_shape(f, 0, group_path);
                s.shape_id = id;
                Arc::new(s)
            })
            .collect();
        shapes.sort_unstable_by_key(|s| s.shape_id);
        shapes.into()
    }

    /// テスト用：図形から作る地図：図形 `own`（下から上への並び。文書の座標）の地図。本番は図形から地図を作らない（新しい
    /// 中身も地図で運ぶ）ので、テストの下ごしらえだけで使う。正しい地図でない図形（重なった図形など）は、並びの順に塗り
    /// 重ねて作り直す（`painted_map`。絵は同じ）。`None`：格子に置けない座標がある
    #[cfg(test)]
    pub(crate) fn of(own: &[&FillShape]) -> Option<Arc<Self>> {
        FROM_SHAPES.with(|n| n.set(n.get() + 1));
        if !own.iter().all(|s| shape_on_grid(s)) {
            return None;
        }
        let map = PlanarMap::from_faces(own.iter().copied());
        Some(Arc::new(if map.is_valid() {
            Self::new(&map)
        } else {
            Self::new(&painted_map(own)?)
        }))
    }
}

#[cfg(test)]
thread_local! {
    /// テスト用：このスレッドで、図形から地図を作った回数（`LayerBase::of`）
    static FROM_SHAPES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、地図を全部確かめた回数（`check`）
    static FULL_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、しまう地図から計算に使う写しを作った回数（`LayerBase::from_stored`）
    static DENSE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、重ね合わせの入れる辺を決めた回数（`fed_edges`）
    static FED_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、判定の形の索引を作った回数（`PolyIndex::build`）
    static INDEX_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、辺の判定の形を作った数（`grid_poly`）
    pub(super) static POLYS_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、重ね合わせで近くの辺を探す外接矩形とマスを作った数（`edge_reach`）
    pub(super) static REACHES_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、面の輪郭をたどるために、頂点のまわりの辺を並べた数（`face_cycles_with`）
    static VERTICES_SORTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、面の輪郭の点と面積（`Ring::new`）を作った数
    pub(super) static RINGS_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、辺の鍵（`edge_key`）を求めた数
    static KEYS_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、重ね合わせで i_overlay に切らせた回数（`overlay_fed`）
    pub(super) static CUTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、窓で重ね合わせるとき、点の色の数えに窓の外の辺の分（前の地図の面の色：`first_crossing`）を
    /// 入れた回数（`Outside`）
    pub(super) static OUTSIDE_WINDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、範囲（窓の範囲 `CellSet`・輪の上と中の升 `polygon_squares`）に入れた項目の数
    pub(super) static RANGE_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// テスト用：このスレッドで、点を含むいちばん小さい外周を、点から右へ伸ばした半直線で探した回数（`LocalMap::owner`）
    pub(super) static OWNER_SEARCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// テスト用：このスレッドで、点を含むいちばん小さい外周を半直線で探した回数
#[cfg(test)]
pub(crate) fn owner_searches() -> usize {
    OWNER_SEARCHES.with(|n| n.get())
}

/// テスト用：このスレッドで、範囲に入れた項目の数
#[cfg(test)]
pub(crate) fn range_entries() -> usize {
    RANGE_ENTRIES.with(|n| n.get())
}

/// テスト用：このスレッドで、点の色の数えに窓の外の辺の分を入れた回数
#[cfg(test)]
pub(crate) fn outside_winds() -> usize {
    OUTSIDE_WINDS.with(|n| n.get())
}

/// テスト用：このスレッドで、重ね合わせで i_overlay に切らせた回数
#[cfg(test)]
pub(crate) fn cuts() -> usize {
    CUTS.with(|n| n.get())
}

/// テスト用：このスレッドで、辺の鍵を求めた数
#[cfg(test)]
pub(crate) fn keys_built() -> usize {
    KEYS_BUILT.with(|n| n.get())
}

/// テスト用：このスレッドで、面の輪郭の点と面積を作った数
#[cfg(test)]
pub(crate) fn rings_built() -> usize {
    RINGS_BUILT.with(|n| n.get())
}

/// テスト用：このスレッドで、面の輪郭をたどるために、頂点のまわりの辺を並べた数
#[cfg(test)]
pub(crate) fn vertices_sorted() -> usize {
    VERTICES_SORTED.with(|n| n.get())
}

/// テスト用：このスレッドで、重ね合わせで近くの辺を探す外接矩形とマスを作った数
#[cfg(test)]
pub(crate) fn reaches_built() -> usize {
    REACHES_BUILT.with(|n| n.get())
}

/// テスト用：このスレッドで、辺の判定の形を作った数
#[cfg(test)]
pub(crate) fn polys_built() -> usize {
    POLYS_BUILT.with(|n| n.get())
}

/// テスト用：このスレッドで、判定の形の索引を作った回数
#[cfg(test)]
pub(crate) fn index_builds() -> usize {
    INDEX_BUILDS.with(|n| n.get())
}

/// テスト用：このスレッドで、重ね合わせの入れる辺を決めた回数
#[cfg(test)]
pub(crate) fn fed_runs() -> usize {
    FED_RUNS.with(|n| n.get())
}

/// テスト用：このスレッドで、しまう地図から計算に使う写しを作った回数
#[cfg(test)]
pub(crate) fn dense_builds() -> usize {
    DENSE_BUILDS.with(|n| n.get())
}

/// テスト用：このスレッドで、地図を全部確かめた回数
#[cfg(test)]
pub(crate) fn full_checks() -> usize {
    FULL_CHECKS.with(|n| n.get())
}

/// テスト用：このスレッドで、図形から地図を作った回数
#[cfg(test)]
pub(crate) fn from_shape_builds() -> usize {
    FROM_SHAPES.with(|n| n.get())
}

/// 文書の座標の図形 `d` を、座標の変換 `xform` で置いた写し（`FillShape::place`）
fn placed(mut d: FillShape, xform: Affine) -> FillShape {
    d.place(xform);
    d
}

/// 地図にする生の図形か：階層 `scope` の生の図形（シンボルの図形でない）で、輪郭がある
pub(crate) fn is_map_shape(s: &FillShape, scope: &[u64]) -> bool {
    s.is_raw_at_scope(scope) && s.symbol_instance.is_none() && !s.exterior.segments.is_empty()
}

/// 輪郭の変形の1回分（どの辺・頂点かは、形と位置で探す）。確定もドラッグ中の表示も、重ね合わせに渡すものは
/// `LayerBase::reshape_change` の1つで作る
#[derive(Clone, Copy, Debug)]
pub(crate) enum ReshapeEdit {
    /// 形が `orig_seg` の辺の `grab_t` の所を `target` へ引っぱる
    PullEdge {
        orig_seg: QuadBez,
        grab_t: f64,
        target: Point,
    },
    /// `orig_vertex` にある頂点を `target` へ動かす（その頂点だけ：アンカーを表示しているとき）
    MoveVertex { orig_vertex: Point, target: Point },
    /// `orig_vertex` にある頂点を `target` へ引っぱる（まわりがついてくる：アンカーを表示していないとき。線の上を引っぱるのと
    /// 同じ決まり `LayerBase::pulled_edges`）
    PullVertex { orig_vertex: Point, target: Point },
}

impl ReshapeEdit {
    /// 頂点 `orig_vertex` を `target` へ動かす1回分：まわりがついてくる引っぱり（`follow`）か、その頂点だけの移動か
    pub(crate) fn vertex(orig_vertex: Point, target: Point, follow: bool) -> Self {
        if follow {
            Self::PullVertex {
                orig_vertex,
                target,
            }
        } else {
            Self::MoveVertex {
                orig_vertex,
                target,
            }
        }
    }
}

/// 引っぱりでつかんだ所（`LayerBase::pulled_edges`）
#[derive(Clone, Copy, Debug)]
pub(crate) enum PullGrab {
    /// 形が `orig_seg` の辺の `grab_t` の所
    Edge { orig_seg: QuadBez, grab_t: f64 },
    /// その位置にある頂点
    Vertex(Point),
}

/// アンカーの追加・削除の1回分（どの辺・頂点かは、形と位置で探す）
#[derive(Clone, Copy, Debug)]
pub(crate) enum AnchorEdit {
    /// 形が `orig_seg` の辺の `t` の所に頂点を足す（境目なら両側の図形に入る）
    Add { orig_seg: QuadBez, t: f64 },
    /// `orig_vertex` にある頂点を消し、前後の辺を1本の直線にする（線の行き止まりの端なら、その区間を取り除く：
    /// `with_anchor_deleted`）
    Delete { orig_vertex: Point },
}

impl AnchorEdit {
    /// 地図 `map` にこの編集をかけた地図（浮いた片の地図の編集。セル画の地図の窓での編集 `anchor_diff` も、窓の地図に同じ
    /// `with_anchor_added`・`with_anchor_deleted` をかける）。`None`：その辺・頂点が無いか、編集できない
    pub(crate) fn apply(self, map: &PlanarMap) -> Option<PlanarMap> {
        match self {
            AnchorEdit::Add { orig_seg, t } => {
                let (e, forward) = map.find_edge(&orig_seg)?;
                map.with_anchor_added(e, if forward { t } else { 1.0 - t })
            }
            AnchorEdit::Delete { orig_vertex } => map
                .with_anchor_deleted((0..map.nodes.len()).find(|&v| map.nodes[v] == orig_vertex)?),
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

    /// 頂点 `v` を消した形。つながる辺が2本なら、前後の辺を1本の直線にする。つながる辺が1本（線の行き止まりの端）なら、
    /// その区間を取り除き、線が1区間短くなる（1区間だけの線は、両端の頂点も残らないので線ごと消える。利用者が決めた）。
    /// 行き止まりの区間は両側が同じ面（面の輪郭を作らない）なので、取り除いても面の形は変わらず、ほかの辺とも交わらない。
    /// 消せないときは `None`：つながる辺が1本・2本でない（3色以上が接する点・線の先が輪郭につながる点など）、
    /// 行き止まりの区間の両側が違う（取り除くと塗りの形が変わる）か閉じた輪、前後が同じ頂点（細いレンズ形）、
    /// 判定の形でつながりが変わる（直線がほかの辺に触れる・ほかの頂点や辺をまたぐ）
    pub(crate) fn with_anchor_deleted(&self, v: usize) -> Option<PlanarMap> {
        let at = self.edges_at(v);
        if let [e] = at[..] {
            let ed = self.edges[e];
            if ed.a == ed.b || ed.left != ed.right || ed.left_src != ed.right_src {
                return None;
            }
            let mut m = self.clone();
            m.edges.swap_remove(e);
            m.compact_nodes();
            return Some(m);
        }
        if at.len() != 2 {
            return None;
        }
        let (e1, e2) = (
            self.edges[at[0]].oriented(false, v),
            self.edges[at[1]].oriented(true, v),
        );
        if e1.a == e2.b || e1.a == v || e2.b == v || !e1.same_content(&e2) {
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
        let area = GridRect::of(&path).union(&GridRect::of(pc)).px();
        PolyIndex::new(self, Some(area)).can_replace(
            part,
            &path,
            &inner,
            pc,
            na,
            nb,
            &mut CellSet::default(),
        )
    }
}

/// 輪郭の変形のドラッグ中の表示：地図 `base` に編集 `edit`（文書の座標。辺の引っぱり・頂点の移動）をかけて確定した結果
/// を、図形の差分（消す図形の番号, 入れる図形）で返す。離したときの確定と同じ1本の道（`reshape_change` → `edit_local`）。
/// 入れる図形は、レイヤー `layer_id` の、座標の変換 `xform` で置いた写し。辺・頂点が無いか、確定できなければ `None`
pub(crate) fn reshape_result(
    base: &LayerBase,
    edit: ReshapeEdit,
    layer_id: LayerId,
    group_path: &[u64],
    xform: Affine,
) -> Option<(Vec<ShapeId>, Vec<FillShape>)> {
    let change = base.reshape_change(edit)?;
    match base.edit_local(&change, group_path) {
        LocalEdit::Same => Some((Vec::new(), Vec::new())),
        LocalEdit::Refused(_) => None,
        LocalEdit::Changed(w) => {
            let mut added: Vec<FillShape> = w
                .added
                .iter()
                .filter_map(|id| {
                    let mut s = (**w.faces.by_id(*id)?).clone();
                    s.layer_id = layer_id;
                    Some(placed(s, xform))
                })
                .collect();
            // 線の図形：変わった辺の線を消し、増えた辺の線を入れる（手間は変わった所の分：`line_changes`）
            let mut removed = w.removed;
            let next = LayerBase::of_local(w.stored, group_path, w.faces, base);
            let (gone, made) = base.line_changes(&next, group_path);
            removed.extend(gone);
            added.extend(made.into_iter().map(|s| {
                let mut s = (*s).clone();
                s.layer_id = layer_id;
                placed(s, xform)
            }));
            Some((removed, added))
        }
    }
}

/// `items` のそれぞれに `f` をかけた結果（`items` の順）。2つ以上あれば、CPU の数までのスレッドで同時に計算する
/// （各スレッドは、まだ誰も取っていないものを順に取る）。レイヤーごとの地図の計算や、オニオンスキンのコマごとのセル画は、
/// ほかを見ないので、同時に計算しても結果は同じ
pub(crate) fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    if threads < 2 {
        return items.iter().map(&f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = std::iter::repeat_with(|| None).take(items.len()).collect();
    std::thread::scope(|scope| {
        let (f, next) = (&f, &next);
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = items.get(i) else {
                            return done;
                        };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        for h in handles {
            for (i, r) in h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)) {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("どれも、どれかのスレッドが計算した"))
        .collect()
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

    /// 面の番号が `ids` にある面の色を `color` にした地図（同じ色でとなり合う面の間の辺は消え、面は1つになる）。
    /// `None`：変わらない（そういう面が無いか、みなすでに `color`）
    pub(crate) fn recolored(&self, ids: &HashSet<ShapeId>, color: Color32) -> Option<PlanarMap> {
        let sides: Vec<(usize, bool)> = self
            .edges
            .iter()
            .enumerate()
            .flat_map(|(i, e)| {
                [(i, true, e.left_src), (i, false, e.right_src)]
                    .into_iter()
                    .filter(|(_, _, src)| src.is_some_and(|id| ids.contains(&id)))
                    .map(|(i, left, _)| (i, left))
            })
            .collect();
        sides
            .iter()
            .any(|&(i, left)| self.side_color((i, left)) != Some(color))
            .then(|| {
                let mut m = self.clone();
                m.color_sides(sides, color);
                m
            })
    }

    /// 辺の側 `sides`（辺の番号と、左の側なら `true`）の色を `color` にする（面の塗り替えはこれ1つ：選んだ面の塗り替え
    /// `recolored` は地図全体で、バケツ `fill_diff` は窓で）。両側が同じ色になった辺は外し、外した辺の端を、2本だけが
    /// つながる頂点をまとめる候補にする（`merge_degree2`）。戻り値は、まとめるときにほかの辺を調べた所
    /// （`merge_degree2`。窓で行うとき、窓がこの所を覆うかを確かめる）
    fn color_sides(
        &mut self,
        sides: impl IntoIterator<Item = (usize, bool)>,
        color: Color32,
    ) -> CellSet {
        // 何も無かった側を塗ったら、その側の線のラベル（`line_label`）は外す（その側は面になる。重ね合わせで色が替わった側の
        // 番号を外すのと同じ）。色のあった側の面の番号は残す（塗り替えた面が番号を引き継ぐ）
        for (i, left) in sides {
            let ed = &mut self.edges[i];
            let (side, src) = if left {
                (&mut ed.left, &mut ed.left_src)
            } else {
                (&mut ed.right, &mut ed.right_src)
            };
            if side.is_none() {
                *src = None;
            }
            *side = Some(color);
        }
        let dropped_ends: Vec<usize> = self
            .edges
            .iter()
            .filter(|e| !e.needed())
            .flat_map(|e| [e.a, e.b])
            .collect();
        self.edges.retain(MapEdge::needed);
        let used = self.merge_degree2(dropped_ends, None);
        self.compact_nodes();
        used
    }
}

/// 引っぱりで、まわりの辺がついてくる範囲：つかんだ所から線をたどって、向きがこれだけ回る手前まで（45°：線の向きが、
/// つかんだ所の向きに沿うより横切る向きに近くなる所。「V」「L」の先（90° 前後回る所。直角を境にすると、ちょうど直角の
/// 角や少し開いた L が、計算の端数しだいで越えてしまう）・「U」の底からの立ち上がり・丸の8分の1周で止まり、反対側は
/// 動かない（利用者が決めた動き）：`LayerBase::pulled_edges`）
pub(crate) const PULL_REACH_TURN: f64 = std::f64::consts::FRAC_PI_4;

/// 引っぱりの片側の重み（`LayerBase::pulled_edges`）：つかんだ所からの曲線の長さ s、その側の端までの長さ L について
/// (1 + cos(πs/L)) / 2（つかんだ所で 1、端で 0。両端で傾きが 0 なので、つかんだ所と、動かない所とのつなぎ目に折れが
/// 出ない）
struct PullWeights {
    /// 片側の長さ L
    total: f64,
    /// 外へ向けた曲線 k の終わりの頂点の重み（最後は端なので 0：動かない）
    verts: Vec<f64>,
    /// 曲線 k の t = 1/2 の点の重み（その曲線の制御点を動かす重み）
    mids: Vec<f64>,
}

impl PullWeights {
    fn new(side: &[(usize, QuadBez)]) -> Self {
        let lens: Vec<f64> = side.iter().map(|(_, q)| q.arclen(FLAT_TOL)).collect();
        let mut w = Self {
            total: lens.iter().sum(),
            verts: Vec::with_capacity(side.len()),
            mids: Vec::with_capacity(side.len()),
        };
        let mut at = 0.0;
        for ((_, q), len) in side.iter().zip(&lens) {
            w.mids.push(w.at(at + q.subsegment(0.0..0.5).arclen(FLAT_TOL)));
            at += len;
            w.verts.push(w.at(at));
        }
        *w.verts.last_mut().expect("片側には曲線がある") = 0.0;
        w
    }

    /// つかんだ所から曲線の長さ `s` の所の重み
    fn at(&self, s: f64) -> f64 {
        0.5 * (1.0 + (std::f64::consts::PI * (s / self.total).min(1.0)).cos())
    }
}

/// 曲線の向きを逆にしたもの
fn reversed(q: QuadBez) -> QuadBez {
    QuadBez::new(q.p2, q.p1, q.p0)
}

/// 曲線の始めの向き（制御点が端と同じなら、もう一方の端への向き）
fn start_dir(q: QuadBez) -> kurbo::Vec2 {
    if q.p1 == q.p0 { q.p2 - q.p0 } else { q.p1 - q.p0 }
}

/// 曲線の終わりの向き（制御点が端と同じなら、もう一方の端からの向き）
fn end_dir(q: QuadBez) -> kurbo::Vec2 {
    if q.p1 == q.p2 { q.p2 - q.p0 } else { q.p2 - q.p1 }
}

/// 向き `a` から `b` へ回る角（左回りが正、−π〜π）
fn turn_between(a: kurbo::Vec2, b: kurbo::Vec2) -> f64 {
    a.cross(b).atan2(a.dot(b))
}

/// 曲線 `q` の、`orig` にある端を `target` へ動かした曲線（輪郭の頂点の移動の決まり：動かす端は `target` へ、制御点は
/// 動いた量の半分だけ動かす。両端が `orig` の辺は両端とも動き、制御点も動いた量だけ動く）
pub(crate) fn moved_quad(q: QuadBez, orig: Point, target: Point) -> QuadBez {
    let pos = |p: Point| if p == orig { target } else { p };
    let k = (q.p0 == orig) as u8 + (q.p2 == orig) as u8;
    QuadBez::new(
        pos(q.p0),
        q.p1 + (target - orig) * (0.5 * k as f64),
        pos(q.p2),
    )
}

/// 輪郭のつまみで、つかんだ所の t（曲線の上）を、引っぱる所に寄せたもの（0.18〜0.82）。引っぱる辺1本だけが動くとき（`pulled_edges`）は制御点を
/// 1 / (2t(1−t)) 倍動かすので、端に近い t ほど少しの動きで辺全体が大きく膨らむ。端の近くをつかむと、端から 18% の所が
/// カーソルの動きどおりに動く（曲線はカーソルから離れた所を通る）。曲線がいつもカーソルの下を通る決まりは、長い辺の端の
/// 近くを少し引くと辺全体が大きく膨らむので採らなかった（GEOMETRY.md 5.1）
pub(crate) fn grab_t(t: f64) -> f64 {
    t.clamp(0.18, 0.82)
}

/// 地図 `map`（文書の座標）に変換 `xform` をかけ、その座標の格子で正しい地図にしたもの（「変形した形を、その座標の格子で
/// 地図にする」決まり）。変換した地図が正しければそのまま（切り直す所が無い）。正しくなければ（縮小で頂点や辺が近づくなど）、
/// 辺どうしもみな切り直す（`regridded`。色・面の番号・線は重ね合わせの決まりで運ぶので、面も線も残る）。結果は確かめた地図
/// （`checked`）。`None`：格子に置けない座標があるか、正しい地図にならない
pub(crate) fn realized(map: &PlanarMap, xform: Affine) -> Option<CheckedMap> {
    if !map.fits_grid(xform) {
        return None;
    }
    let t = map.transformed(xform);
    match t.check() {
        Some(faces) => Some(CheckedMap { map: t, faces }),
        None => t.regridded()?.checked(),
    }
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
    /// 辺の側に記された番号が1つの面はその番号（同じ番号の面が2つ以上あれば、そのうち `keeps_label_over` で勝つ面だけ）、
    /// ほかは新しい番号
    Keep,
    /// みな新しい番号（新しい中身を置くとき。元の面と番号が重ならない）
    Fresh,
}

/// 地図 `map` の面 `faces` の番号（面ごとに1つずつ別の番号）。面の番号の決め方はこれ1つ。どの面が番号を保つかは、
/// 地図の頂点と辺の番号によらない（`keeps_label_over`）
fn face_ids(map: &PlanarMap, faces: &[MapFace], how: FaceIds) -> Vec<ShapeId> {
    // 番号ごとの、その番号を保つ面
    let mut holder: HashMap<ShapeId, usize> = HashMap::new();
    if let FaceIds::Keep = how {
        for (i, f) in faces.iter().enumerate() {
            let [id] = f.srcs[..] else {
                continue;
            };
            let h = holder.entry(id).or_insert(i);
            if *h != i && keeps_label_over(map, f, &faces[*h]) {
                *h = i;
            }
        }
    }
    faces
        .iter()
        .enumerate()
        .map(|(i, f)| match f.srcs[..] {
            [id] if holder.get(&id) == Some(&i) => id,
            _ => crate::vector_engine::next_shape_id(),
        })
        .collect()
}

/// 同じ元の図形の番号を保てる面が2つ以上あるとき、面 `a` が面 `b` より先に保つか：輪郭の辺（向きのある辺）が多い面、
/// 同じ数なら形の鍵（`face_key`）の小さい面。面の番号の同点の決め方はこれ1つ（`face_ids`・`LocalMap::rewrite`）。
/// 地図の番号の順で決めない（同じ地図を番号の振り方を変えて作っても、同じ面が保つ：段0の
/// `edits_do_not_depend_on_edge_and_node_numbering`）
fn keeps_label_over(map: &PlanarMap, a: &MapFace, b: &MapFace) -> bool {
    let sides = |f: &MapFace| f.cycles.iter().map(Vec::len).sum::<usize>();
    let (sa, sb) = (sides(a), sides(b));
    sa > sb || (sa == sb && face_key(map, a) < face_key(map, b))
}

/// 面の、番号によらない鍵：面の輪郭の向きのある辺の（形の鍵 `edge_key`, その辺の決まった向き（`point_order`）の左の
/// 側か）のうち、いちばん小さいもの。向きのある辺は1つの面にしか無いので、面ごとに別の鍵になる
fn face_key(map: &PlanarMap, f: &MapFace) -> ([u64; 6], bool) {
    f.cycles
        .iter()
        .flatten()
        .map(|&(e, forward)| {
            let ed = &map.edges[e];
            let canonical = point_order(map.nodes[ed.a], map.nodes[ed.b]).is_le();
            (edge_key(map, e), forward == canonical)
        })
        .min()
        .expect("面の輪郭には辺がある")
}

/// 正しい地図（`valid_faces` で確かめたもの）と、その面。作れるのは `PlanarMap::checked` と、正しい地図の一部を取り出す
/// `LayerBase::checked_part` と、正しい地図を正しいまま変える、このあとのメソッドだけなので、これを受け取る所（セル画に
/// しまう地図：`ScopeMap::new`、片の組：`PieceGroup::new`）は確かめ直さない
#[derive(Clone, Debug)]
pub(crate) struct CheckedMap {
    map: PlanarMap,
    faces: Vec<MapFace>,
}

impl PlanarMap {
    /// 正しい地図なら（`valid_faces`。受け入れの判定はこれ1つ）、確かめた地図にする。`None`：正しい地図でない
    pub(crate) fn checked(self) -> Option<CheckedMap> {
        let faces = self.check()?;
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

    /// しまう形（面に1つずつ別の番号を記す：`uniquely_labeled`。塊は `prev` と共有する：`StoredMap::from_dense`）
    pub(crate) fn into_stored(self, prev: Option<&StoredMap>) -> StoredMap {
        StoredMap::from_dense(self.uniquely_labeled().map(), prev)
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
        let ids = face_ids(&self.map, &self.faces, how);
        let mut c = self.with_ids(&ids);
        // みな新しい番号にするときは、線のラベルも新しい番号にする（同じラベルは同じ新しい番号）
        if matches!(how, FaceIds::Fresh) {
            let mut fresh: IntMap<ShapeId, ShapeId> = IntMap::default();
            let mut renew = |src: &mut Option<ShapeId>| {
                if let Some(id) = src {
                    *id = *fresh.entry(*id).or_insert_with(crate::vector_engine::next_shape_id);
                }
            };
            for ed in &mut c.map.edges {
                if ed.left.is_none() {
                    renew(&mut ed.left_src);
                }
                if ed.right.is_none() {
                    renew(&mut ed.right_src);
                }
            }
        }
        c
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

/// 浮いた片を切り出す：セル画の地図 `cel` の、中身 `content`（`cel` の地図そのものか、前に切り出した残り）を、選ぶ所
/// `region` で1回切る（`LayerBase::cut_local`）。（残りの書くもの, 片の地図）：残りは、セル画の地図を残りの地図にする
/// 1本の道の答え（`LayerBase::replace_local`。消した図形が隠す図形、入れた図形が残りの図形：`Origin::of_local`）。
/// 片は小さいので片だけで確かめ、丸ごと中にあるセル画の面はその番号のまま、ほかの面には新しい番号を振る
/// （`LayerBase::piece_ids`。片の新しい形は、残りの形と同じ元の図形を引き継ぐことがあるので、番号を新しくする。地図の
/// 印も）。残りの形は、セル画の地図の書き換えと同じく、元の図形の番号を引き継ぐ（隠す元の図形と入れ替わる。残りの地図の
/// 印が変わらないので、残りの地図は、切り口から離れた所の塊をセル画の地図と共有する）。中と外は1回の重ね合わせで
/// 同じ区間・同じ頂点から作る（`overlay_rules`）ので、切り口の頂点は両側で同じ。切り口の区間を曲線に戻すかどうかは
/// 地図ごとに決める（調べる相手の辺が両側で違う）ので、点の並びはビットまでは同じにならないことがある（地図の精度
/// `FLAT_TOL` の中で同じ線。重ね直すと同じ線として1本になる：`same_curve`）。`None`：格子に置けない座標がある
pub(crate) fn cut_out(
    cel: &LayerBase,
    content: &LayerBase,
    region: MapInput<'_>,
    group_path: &[u64],
) -> Option<(LocalEdit, CheckedMap)> {
    let (inside, rest) = content.cut_local(region)?;
    // 残りは、セル画の地図を残りのしまう形にする差分として1本の道に渡す（中身が前の残りでも、セル画に対する答え）
    let rest = cel.replace_local(&rest, group_path);
    if matches!(rest, LocalEdit::Refused(_)) {
        return None;
    }
    // 片は小さいので、片だけで確かめる。番号は、セル画の面と丸ごと同じ面はその番号のまま、ほかは新しい番号
    let piece = inside.checked()?;
    let ids = cel.piece_ids(&piece, group_path);
    // テストでは毎回：セル画の番号のまま残る面が、セル画の地図を片にする差分を地図全体を触った所として1本の道に通した答え
    // （`whole_touched`）で、前の図形のまま残る面と同じ（残す面の決まりはどちらも `kept_src`）
    #[cfg(test)]
    if test_checks() {
        let ok = uncounted(|| {
            let cel_faces = cel.faces(group_path);
            let (kept, count): (std::collections::BTreeSet<ShapeId>, usize) =
                match cel.whole_touched(piece.map(), group_path) {
                    LocalEdit::Same => (
                        cel_faces.iter().map(|s| s.shape_id).collect(),
                        cel_faces.len(),
                    ),
                    LocalEdit::Changed(w) => (
                        w.faces
                            .iter()
                            .map(|s| s.shape_id)
                            .filter(|id| !w.added.contains(id))
                            .collect(),
                        w.faces.len(),
                    ),
                    LocalEdit::Refused(_) => return false,
                };
            let mine: std::collections::BTreeSet<ShapeId> = ids
                .iter()
                .copied()
                .filter(|&id| cel_faces.by_id(id).is_some())
                .collect();
            kept == mine && count == ids.len()
        });
        assert!(
            ok,
            "片だけで決めた番号が、地図全体を触った所として決めた番号（お手本）と違う"
        );
    }
    let piece = piece.with_ids(&ids);
    Some((rest, piece))
}

impl LayerBase {
    /// 切り出した片（確かめた地図 `piece`）の面ごとの番号：この写しの面（階層 `group_path`）を残す面（残す面の決まり
    /// `kept_src`。元の図形の辺がみな残っているかは、その図形の輪郭の辺がみな片にあるか）はその番号（同じ番号は1つの面
    /// だけ）。ほかは新しい番号。手間は片の大きさと、番号を引いた面の大きさの分
    fn piece_ids(&self, piece: &CheckedMap, group_path: &[u64]) -> Vec<ShapeId> {
        let faces_list = self.faces(group_path);
        let m = &piece.map;
        let keys: HashSet<[u64; 6]> = (0..m.edges.len()).map(|e| edge_key(m, e)).collect();
        let mut kept: HashSet<ShapeId> = HashSet::new();
        piece
            .faces
            .iter()
            .map(|f| {
                let shape = |s: ShapeId| faces_list.by_id(s);
                let keep = kept_src(
                    f.cycles.iter().flatten().copied(),
                    |h| m.side_src(h),
                    // 形が同じこの地図の辺の、同じ側の番号
                    |h| {
                        let (e, fwd) = self.stored.find_edge(&m.quad(h.0))?;
                        let te = self.stored.edge_at(e);
                        if fwd == h.1 {
                            te.left_src
                        } else {
                            te.right_src
                        }
                    },
                    f.color,
                    |s| shape(s).map(|x| x.color),
                    // 元の図形の辺がみな片にあるか
                    |s| {
                        !shape(s).is_some_and(|x| {
                            x.contours()
                                .flat_map(|c| c.segments.iter())
                                .all(|q| keys.contains(&local::quad_key(q)))
                        })
                    },
                );
                match keep {
                    Some(src) if kept.insert(src) => src,
                    _ => crate::vector_engine::next_shape_id(),
                }
            })
            .collect()
    }
}

/// テスト用：図形 `shapes`（下から上への並び）を、並びの順に空の地図へ塗り重ねた地図（重なる所は上の図形が見える）。図形
/// ごとにブラシと同じく塗る（`MapRule::Paint`。偶奇の規則なので、輪郭が自分と交わる図形もそのまま扱える）。`None`：格子に
/// 置けない座標がある
#[cfg(test)]
fn painted_map(shapes: &[&FillShape]) -> Option<PlanarMap> {
    shapes.iter().try_fold(PlanarMap::default(), |m, s| {
        m.overlay(MapInput::Shapes(std::slice::from_ref(*s)), MapRule::Paint(Some(s.color)))
    })
}

/// テスト用：図形 `shapes`（下から上への並び）を塗り重ねた地図（`painted_map`）
#[cfg(test)]
pub(crate) fn painted_map_pub(shapes: &[FillShape]) -> Option<PlanarMap> {
    let refs: Vec<&FillShape> = shapes.iter().collect();
    painted_map(&refs)
}

/// 点の並び `pts` のうち、`dist` が `radius` 以内でいちばん小さいもの（同じなら座標の順で先：`point_order`）。近い頂点を
/// 探す所（カーソル合わせ・形の線の端の寄せ）の決まりはこれ1つ
pub(crate) fn nearest_of(
    pts: impl IntoIterator<Item = Point>,
    radius: f64,
    dist: impl Fn(Point) -> f64,
) -> Option<Point> {
    let mut best: Option<(f64, Point)> = None;
    for q in pts {
        let d = dist(q);
        if d <= radius
            && best.is_none_or(|(bd, bq)| d < bd || (d == bd && point_order(q, bq).is_lt()))
        {
            best = Some((d, q));
        }
    }
    best.map(|b| b.1)
}

/// 多くの点から、`radius` 以内でいちばん近い点を何度も引く道具（形の線の端を前からある頂点へ寄せる所）。点を `radius` の
/// マスに分け、引く点のまわりの 3×3 マスの点だけを `nearest_of` で比べる
pub(crate) struct NearPoints {
    radius: f64,
    cells: IntMap<(i64, i64), Vec<Point>>,
}

impl NearPoints {
    pub(crate) fn new(pts: impl IntoIterator<Item = Point>, radius: f64) -> Self {
        let mut cells: IntMap<(i64, i64), Vec<Point>> = IntMap::default();
        for p in pts {
            cells.entry(Self::cell(p, radius)).or_default().push(p);
        }
        Self { radius, cells }
    }

    fn cell(p: Point, radius: f64) -> (i64, i64) {
        ((p.x / radius).floor() as i64, (p.y / radius).floor() as i64)
    }

    /// 点 `p` から `radius` 以内でいちばん近い点
    pub(crate) fn nearest(&self, p: Point) -> Option<Point> {
        let (cx, cy) = Self::cell(p, self.radius);
        // 格子に置けない座標（重ね合わせがあとで断る）でも溢れないように
        let around = |c: i64| c.saturating_sub(1)..=c.saturating_add(1);
        let pts = around(cx)
            .flat_map(|x| around(cy).map(move |y| (x, y)))
            .flat_map(|k| self.cells.get(&k).into_iter().flatten().copied());
        nearest_of(pts, self.radius, |q| (q - p).hypot())
    }
}
