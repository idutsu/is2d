//! 同じレイヤー・同じ階層の生の図形を、境目の線（辺）で持つ地図。セル画の中身そのもので、描く・消す・塗る・選ぶ・
//! 動かす・貼る・輪郭の変形など、生の図形を変える編集はみな、この地図を受け取って地図を返す。
//! Adobe Animate のマージ描画と同じく、辺が「曲線・左の色・右の色」を持つ。境目は1本で、重ね順は無い。
//! 塗りの形（`FillShape`）は、辺から作って描画・当たり判定に使う。

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use eframe::egui::Color32;
use kurbo::{Affine, ParamCurve, Point, QuadBez, Rect as KRect, Shape as KurboShape};

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
    #[inline]
    fn finish(&self) -> u64 {
        self.0
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

/// 辺の鍵（`edge_key`）から、その地図の辺の番号へ
pub(crate) type EdgeKeys = IntMap<[u64; 6], usize>;

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

    #[cfg(test)]
    /// 面の番号が `ids` にある面を外した地図（`restricted` の逆）
    pub(crate) fn without(&self, ids: &HashSet<ShapeId>) -> PlanarMap {
        self.restricted_by(|id| !ids.contains(&id))
    }

    /// 面の番号（辺の側に記したもの）
    pub(crate) fn labels(&self) -> impl Iterator<Item = ShapeId> + '_ {
        self.edges
            .iter()
            .flat_map(|e| [e.left_src, e.right_src])
            .flatten()
    }

    #[cfg(test)]
    /// 辺の側に記した図形の番号を、`f` が返す番号に付け替える（`None` を返す番号はそのまま。計算に使う地図の番号の
    /// 付け替えはこれ1つ。しまった地図は、計算に使う地図を作らずに塊の辺を直接替える：`StoredMap::relabeled`）
    pub(crate) fn relabel(&mut self, mut f: impl FnMut(ShapeId) -> Option<ShapeId>) {
        for e in &mut self.edges {
            for src in [&mut e.left_src, &mut e.right_src].into_iter().flatten() {
                if let Some(n) = f(*src) {
                    *src = n;
                }
            }
        }
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

    /// テスト用：正しい地図なら（`is_valid`）、その面（`faces`）。判定は本番と同じ `checked_since`（書き戻し・読み込み・
    /// 変換した地図の判定はどこでもこれ1つ）。`None`：正しい地図でない
    #[cfg(test)]
    pub(crate) fn valid_faces(&self) -> Option<Vec<MapFace>> {
        self.valid_faces_since(None)
    }

    /// `valid_faces` と同じ判定。ただし、正しい地図 `prev`（計算に使う写し）にある辺（形がビットまで同じ：`edge_key`）
    /// どうしは、判定の形が離れていることを確かめ直さない（`prev` が正しいので、その2本はもともと離れている）。セル画の
    /// 地図に書き戻すとき（`written`）に、前の地図の写しを渡す：判定の手間のうちいちばん重い、区間どうしの確かめが、
    /// 変えた辺の近くだけになる（レビュー 4cb53aa の N-2001）。判定の形も、`prev` にある辺は使い回す（`polys_since`）。
    /// テスト用（本番は `written`・`checked` が本体の `checked_since` を呼ぶ）
    #[cfg(test)]
    pub(crate) fn valid_faces_since(&self, prev: Option<&LayerBase>) -> Option<Vec<MapFace>> {
        self.checked_since(prev).map(|c| c.faces)
    }

    /// `valid_faces_since` の本体。正しい地図なら、その面と、辺ごとの判定の形
    fn checked_since(&self, prev: Option<&LayerBase>) -> Option<Checked> {
        #[cfg(test)]
        if prev.is_none() {
            FULL_CHECKS.with(|n| n.set(n.get() + 1));
        }
        // 頂点と制御点が格子に置ける（±65,536px の内で有限：`fits_grid`）こと、両側が同じ色の辺が無いことを先に確かめる
        // （格子の外の座標で、判定の形を作らない）
        if !self.fits_grid(Affine::IDENTITY) || self.edges.iter().any(|e| e.left == e.right) {
            return None;
        }
        let (polys, origin) = self.polys_since(prev);
        let fresh: Vec<bool> = origin
            .iter()
            .map(|o| matches!(o, EdgeOrigin::New))
            .collect();
        if !self.polys_apart(&polys, &fresh) {
            return None;
        }
        let (cycles, kept) = match prev {
            Some(prev) => self.face_cycles_since(&polys, prev, &origin),
            None => (self.face_cycles_with(&polys), Vec::new()),
        };
        let (regions, cycle_regions, made) = self.regions_since(
            &polys,
            &cycles,
            false,
            prev.map(|p| (&kept[..], p.cycle_regions())),
        );
        if regions.broken > 0 {
            return None;
        }
        // 何も無い側の輪郭：その最初の区間の真ん中の点が、その辺を輪郭に持たない色のある面の中（外周の中で、どの穴の外：
        // `Regions::contains`）にあるか、その面の輪郭の上にあれば、入り込んでいる。前の地図と同じ輪郭で、その点が新しく
        // 作った輪のどの外接矩形にも入らないものは、確かめ直さない（前の地図は正しい。点を含みうる組は、新しく作った輪の
        // 無い組だけで、その輪は前の地図の組の輪と同じなので、答えも同じ）
        let to_check: Vec<(usize, IPt)> = cycles
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_empty() && self.side_color(c[0]).is_none())
            .map(|(ci, c)| (ci, half_mid2(&polys, c[0])))
            .filter(|&(ci, probe)| {
                kept.get(ci).copied().flatten().is_none() || made.iter().any(|r| r.contains2(probe))
            })
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
        Some(Checked {
            faces: regions.into_faces(self),
            polys,
            cycles,
            cycle_regions,
            #[cfg(test)]
            origin,
        })
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

    /// 全部の辺の判定の形と、辺ごとの、正しい地図 `prev`（計算に使う写し）での形。`prev` にある辺（形がビットまで同じ：
    /// `edge_key`）は、向きも同じなら `prev` の判定の形をそのまま使う（同じ曲線から同じ式で作るので、ビットまで同じ）。
    /// 作るのは、`prev` に無い辺と、向きの逆な辺の分だけ
    fn polys_since(&self, prev: Option<&LayerBase>) -> (Vec<Poly>, Vec<EdgeOrigin>) {
        let Some(prev) = prev else {
            return (self.polys(), vec![EdgeOrigin::New; self.edges.len()]);
        };
        let origin = self.edge_origins(prev);
        let prev_polys = prev.polys();
        let polys = origin
            .iter()
            .enumerate()
            .map(|(e, o)| match *o {
                EdgeOrigin::Same(i) => Arc::clone(&prev_polys[i]),
                _ => EdgeGeom::of(&self.quad(e)),
            })
            .collect();
        (polys, origin)
    }

    /// 辺ごとの、前の地図 `prev` の同じ辺（`EdgeOrigin`。形がビットまで同じ辺：`edge_key`）。前の地図の辺との対応を
    /// 求めるのはこれ1つ（確かめるときの判定の形の引き継ぎ `polys_since` と、書き戻すときの残す面の決め方
    /// `rewrite_shapes`。確かめた地図が持って、書き戻しに渡す：`CheckedMap::since`）
    fn edge_origins(&self, prev: &LayerBase) -> Vec<EdgeOrigin> {
        let keys = prev.keys();
        let start = |m: &PlanarMap, e: usize| {
            let p = m.nodes[m.edges[e].a];
            [p.x.to_bits(), p.y.to_bits()]
        };
        (0..self.edges.len())
            .map(|e| match keys.get(&edge_key(self, e)) {
                Some(&i) if start(self, e) == start(prev.map(), i) => EdgeOrigin::Same(i),
                Some(&i) => EdgeOrigin::Reversed(i),
                None => EdgeOrigin::New,
            })
            .collect()
    }

    /// 判定の形の 1〜3（設計 2-2）：折れ線どうしが共有する頂点のほかで触れない・1本の折れ線が自分と触れない・
    /// 別々の頂点が別々の格子の点に丸まる。区間どうしは、どちらかが `fresh` の辺のものだけを確かめる（ほかの辺どうしは
    /// 正しい地図にそのままあった：`valid_faces_since`）。見るのは、`fresh` の辺の折れ線の外接矩形にかかる区間だけ
    fn polys_apart(&self, polys: &[Poly], fresh: &[bool]) -> bool {
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
        // 確かめる範囲：`fresh` の辺の折れ線の外接矩形（触れる区間は、この矩形にかかる）
        let Some(reach) = (0..polys.len())
            .filter(|&e| fresh[e])
            .map(|e| GridRect::of(&polys[e]))
            .reduce(|a, b| a.union(&b))
        else {
            return true;
        };
        let segs: Vec<(usize, usize)> = polys
            .iter()
            .enumerate()
            .flat_map(|(e, p)| (0..p.len() - 1).map(move |k| (e, k)))
            .filter(|&(e, k)| GridRect::of(&polys[e][k..k + 2]).overlaps(&reach))
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
                if (fresh[e] || fresh[segs[j].0])
                    && rects[i].overlaps(&rects[j])
                    && self.segs_clash(polys, segs[j], segs[i])
                {
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
        let regions = self.regions(polys, cycles, false);
        let broken = regions.broken;
        (regions.into_faces(self), broken)
    }

    /// 面の輪郭 `cycles`（`face_cycles_with`。判定の形 `polys`）を、外周（面積が正）と、その中の穴（面積が負）の組に分ける。
    /// 穴は、それを含む同じ色（色の無い所も1つの色とみなす）の外周のうち、いちばん小さいものに入れる。外周か穴かは判定の
    /// 形の面積の符号、穴の入れ先は判定の形での点の内外（`Ring::contains`）で、どちらも整数で決める。`unpainted`：色の無い
    /// 所（何も塗っていない、囲まれた所）の組も作るか。面の取り出し・正しい地図かの判定・バケツの、面の分け方はこれ1つ
    fn regions(&self, polys: &[Poly], cycles: &[Vec<(usize, bool)>], unpainted: bool) -> Regions {
        self.regions_since(polys, cycles, unpainted, None).0
    }

    /// `regions` を、正しい地図の、輪郭ごとの組み立ての結果 `prev`（（この地図の輪郭ごとの、`prev` の同じ輪郭の番号,
    /// `prev` の輪郭ごとの `CycleRegion`）：`face_cycles_since`・`LayerBase::cycle_regions`）から求める。結果と、輪郭ごとの
    /// `CycleRegion` と、新しく作った輪（前の地図から引き継がなかったもの）の外接矩形。
    /// - 前の地図と同じ輪郭の輪は、前の地図のものを使う（同じ辺の判定の形から作るので同じ）
    /// - 前の地図と同じ穴は、前の地図の入れ先の輪郭が残り、色が穴と同じで、新しく作った同じ色の外周の外接矩形が穴の調べる
    ///   点を含まなければ、その入れ先のまま（入れ先は、穴を含む同じ色の外周のうちいちばん小さいもの。残った外周の中では
    ///   前と同じものがいちばん小さく、新しい外周はその点を含まない）。ほかは `regions` と同じく調べる
    ///
    /// 組の順・穴の順は `regions` と同じ（輪郭の順）なので、結果も同じ
    fn regions_since(
        &self,
        polys: &[Poly],
        cycles: &[Vec<(usize, bool)>],
        unpainted: bool,
        prev: Option<(&[Option<usize>], &[CycleRegion])>,
    ) -> (Regions, Vec<CycleRegion>, Vec<GridRect>) {
        let mut out = Regions {
            cycles: Vec::new(),
            rings: Vec::new(),
            groups: Vec::new(),
            broken: 0,
            bands: YBands::new(std::iter::empty()),
        };
        let (kept, prev_regions): (&[Option<usize>], &[CycleRegion]) = prev.unwrap_or((&[], &[]));
        let kept_of = |ci: usize| kept.get(ci).copied().flatten();
        let mut infos = vec![CycleRegion::default(); cycles.len()];
        let mut made: Vec<GridRect> = Vec::new();
        // 新しく作った外周（色, 外接矩形）
        let mut made_exteriors: Vec<(Option<Color32>, GridRect)> = Vec::new();
        // 輪ごとの輪郭の番号
        let mut ring_cycle: Vec<usize> = Vec::new();
        let mut holes: Vec<(Option<Color32>, usize)> = Vec::new();
        for (ci, cycle) in cycles.iter().enumerate() {
            if self.is_broken_cycle(cycle) {
                out.broken += 1;
                continue;
            }
            let color = self.side_color(cycle[0]);
            if color.is_none() && !unpainted {
                continue;
            }
            let k = out.rings.len();
            let ring = match kept_of(ci).and_then(|pk| prev_regions[pk].ring.clone()) {
                Some(r) => r,
                None => {
                    let r = Arc::new(Ring::new(polys, cycle));
                    made.push(r.rect);
                    if r.area2 > 0 {
                        made_exteriors.push((color, r.rect));
                    }
                    r
                }
            };
            infos[ci].ring = Some(Arc::clone(&ring));
            if ring.area2 > 0 {
                out.groups.push((color, vec![k]));
            } else {
                holes.push((color, k));
            }
            out.rings.push(ring);
            out.cycles.push(cycle.clone());
            ring_cycle.push(ci);
        }
        out.bands = YBands::new(out.groups.iter().map(|(_, ids)| {
            let r = out.rings[ids[0]].rect;
            (2 * r.y0, 2 * r.y1)
        }));
        // 外周の輪郭ごとの組（前の地図の入れ先を引くため）
        let mut group_of: Vec<usize> = vec![usize::MAX; cycles.len()];
        for (g, (_, ids)) in out.groups.iter().enumerate() {
            group_of[ring_cycle[ids[0]]] = g;
        }
        // 前の地図の輪郭ごとの、この地図の同じ輪郭
        let mut now_of: Vec<Option<usize>> = vec![None; prev_regions.len()];
        for (ci, k) in kept.iter().enumerate() {
            if let Some(pk) = *k {
                now_of[pk] = Some(ci);
            }
        }
        // 穴を含む同じ色の外周のうち、いちばん小さいもの（同じなら組の順で前）。調べる点は、穴の最初の区間の真ん中（正しい
        // 地図では、ほかの輪郭の上に無い）
        for (color, k) in holes {
            let ci = ring_cycle[k];
            let probe = half_mid2(polys, out.cycles[k][0]);
            let kept_owner = kept_of(ci)
                .and_then(|pk| prev_regions[pk].owner)
                .and_then(|po| now_of[po])
                .map(|no| group_of[no])
                .filter(|&g| {
                    g != usize::MAX
                        && out.groups[g].0 == color
                        && !made_exteriors
                            .iter()
                            .any(|&(c, r)| c == color && r.contains2(probe))
                });
            let owner = kept_owner.or_else(|| {
                out.groups_at(probe)
                    .filter(|&g| {
                        out.groups[g].0 == color
                            && out.rings[out.groups[g].1[0]].contains(probe) == Some(true)
                    })
                    .min_by_key(|&g| out.rings[out.groups[g].1[0]].area2)
            });
            match owner {
                Some(g) => {
                    infos[ci].owner = Some(ring_cycle[out.groups[g].1[0]]);
                    out.groups[g].1.push(k);
                }
                // 色のある穴に外周が無ければ壊れている。色の無い穴に外周が無いのは、いちばん外（囲まれていない所）の穴
                None if color.is_some() => out.broken += 1,
                None => {}
            }
        }
        (out, infos, made)
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
        self.edges_at_in(v, None)
    }

    /// `edges_at` と同じ（辺の番号の順）。`index`（この地図の升目の索引）があれば、頂点のある升目の辺だけを見る
    fn edges_at_in(&self, v: usize, index: Option<MapIndex<'_>>) -> Vec<usize> {
        let touches = |e: usize| self.edges[e].a == v || self.edges[e].b == v;
        match self.edges_around(self.nodes[v], index) {
            Some(mut near) => {
                near.retain(|&e| touches(e));
                near
            }
            None => (0..self.edges.len()).filter(|&e| touches(e)).collect(),
        }
    }

    /// 点 `p`（文書の座標）を含むマスの升目にかかる辺（辺の番号の順・1つずつ）。`index`（この地図の升目の索引）が
    /// 無ければ `None`。`p` を端に持つ辺は、みなこの中にある（辺の近くを探すマスは、辺の端の点を含むマスを含む）
    fn edges_around(&self, p: Point, index: Option<MapIndex<'_>>) -> Option<Vec<usize>> {
        let ix = index?;
        let mut out = Vec::new();
        ix.ends.edges_in(ix.stored, end_key(p), &mut out);
        out.sort_unstable();
        out.dedup();
        Some(out)
    }

    /// 頂点 `v` を `target` へ動かして確定した地図（重ね合わせ `MapRule::Sweep` で。`None`：格子に置けない座標がある）。
    /// つながる辺の制御点は、動いた量の半分だけ動かす（今の頂点の移動と同じ決まり）。`target` がほかの頂点と同じ座標なら
    /// （吸着）、重ね合わせが格子の同じ点を1つの頂点にするので、2つの頂点は1つになる。テスト用（本番は `MapEdit` を
    /// 計算に使う写しで：`LayerBase::apply_edit`）
    #[cfg(test)]
    pub(crate) fn commit_move_vertex(&self, v: usize, target: Point) -> Option<PlanarMap> {
        self.moved_vertex(&[], None, v, target)
    }

    /// `commit_move_vertex` を、この地図の辺の計算に使う形 `known` と升目の索引 `index` で（`overlay_known`）
    fn moved_vertex(
        &self,
        known: &[Poly],
        index: Option<MapIndex<'_>>,
        v: usize,
        target: Point,
    ) -> Option<PlanarMap> {
        let d = target - self.nodes[v];
        let pos = |w: usize| if w == v { target } else { self.nodes[w] };
        let moved: Vec<MovedEdge> = self
            .edges_at_in(v, index)
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
        self.overlay_known(known, index, MapInput::Sweep(&moved), MapRule::Sweep)
    }

    /// 辺 `e` の `grab_t` の所を `target` へ引っぱって確定した地図（重ね合わせ `MapRule::Sweep` で。`None`：格子に置けない
    /// 座標がある）。引っぱった曲線は、今の辺のつまみと同じ決まり（両端は動かない）。テスト用（本番は `MapEdit` を計算に
    /// 使う写しで：`LayerBase::apply_edit`）
    #[cfg(test)]
    pub(crate) fn commit_pull_edge(
        &self,
        e: usize,
        grab_t: f64,
        target: Point,
    ) -> Option<PlanarMap> {
        self.pulled_edge(&[], None, e, grab_t, target)
    }

    /// `commit_pull_edge` を、この地図の辺の計算に使う形 `known` と升目の索引 `index` で（`overlay_known`）
    fn pulled_edge(
        &self,
        known: &[Poly],
        index: Option<MapIndex<'_>>,
        e: usize,
        grab_t: f64,
        target: Point,
    ) -> Option<PlanarMap> {
        let pulled = pulled_quad(self.quad(e), grab_t, target);
        self.overlay_known(
            known,
            index,
            MapInput::Sweep(&[(e, pulled)]),
            MapRule::Sweep,
        )
    }

    /// 地図を座標の変換 `xform` で置いたとき、`p` から `r` 以内でいちばん近い頂点（距離は置いた座標で測る）。テスト用
    /// （本番は升目の索引で：`LayerBase::hit_node`）
    #[cfg(test)]
    pub(crate) fn hit_node(&self, p: Point, r: f64, xform: Affine) -> Option<usize> {
        self.node_finder(r, xform, KRect::from_points(p, p))(p, None)
    }

    #[cfg(test)]
    /// 地図を座標の変換 `xform` で置いたとき、`p` から `r` 以内でいちばん近い辺と、その上のパラメータ（距離は置いた座標で
    /// 測る。パラメータはアフィン変換で変わらない）
    pub(crate) fn hit_edge(&self, p: Point, r: f64, xform: Affine) -> Option<(usize, f64)> {
        self.hit_edge_among(p, r, xform, 0..self.edges.len())
    }

    #[cfg(test)]
    /// `hit_edge` を、辺 `among`（番号の順）の中で
    fn hit_edge_among(
        &self,
        p: Point,
        r: f64,
        xform: Affine,
        among: impl Iterator<Item = usize>,
    ) -> Option<(usize, f64)> {
        let near = KRect::from_center_size(p, (2.0 * r, 2.0 * r));
        among
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

    /// 面の輪郭（`face_cycles_with`）を、正しい地図 `prev`（計算に使う写し）の輪郭から求める（`origin`：辺ごとの `prev`
    /// での形）。`prev` の輪郭のうち、辺がみなこの地図に同じ向きであり、通る頂点がどれも「まわりの辺が変わらない頂点」の
    /// ものは、そのまま使う（頂点のまわりの辺と、その判定の形が同じなので、頂点での辺の順番も同じ。たどっても同じ輪郭）。
    /// たどり直すのは残りの向きのある辺だけで、頂点のまわりの辺を並べるのも、たどる頂点だけ。輪郭の順と、輪郭の始めの辺の
    /// 決まりは `face_cycles_with` と同じ（たどる順で最初の向きのある辺から始め、その順に並べる）ので、結果も同じ
    fn face_cycles_since(
        &self,
        polys: &[Poly],
        prev: &LayerBase,
        origin: &[EdgeOrigin],
    ) -> (Vec<Cycle>, Vec<Option<usize>>) {
        let pm = &prev.map();
        let idx = |h: (usize, bool)| 2 * h.0 + h.1 as usize;
        // 前の地図の辺ごとの、この地図の同じ向きの辺
        let mut back: Vec<Option<usize>> = vec![None; pm.edges.len()];
        for (e, o) in origin.iter().enumerate() {
            if let EdgeOrigin::Same(i) = *o {
                back[i] = Some(e);
            }
        }
        // この地図の頂点ごとの、出ていく向きのある辺（並べるのは、たどる頂点だけ）
        let mut out_of: Grouped<(usize, bool)> = Grouped::new(
            self.nodes.len(),
            self.edges
                .iter()
                .enumerate()
                .flat_map(|(i, e)| [(e.a, (i, true)), (e.b, (i, false))]),
        );
        // まわりの辺が変わった前の地図の頂点：同じ向きの辺が無くなった辺の端と、この地図で同じ向きでない辺（足した辺・
        // 向きの逆な辺）の端（前の地図の同じ頂点は、つながる同じ向きの辺で引く。そういう辺が無い頂点は、前の地図に無いか、
        // まわりの辺がみな無くなっていて、もう印が付いている）
        let mut dirty = vec![false; pm.nodes.len()];
        for (i, b) in back.iter().enumerate() {
            if b.is_none() {
                dirty[pm.edges[i].a] = true;
                dirty[pm.edges[i].b] = true;
            }
        }
        for (e, o) in origin.iter().enumerate() {
            if matches!(o, EdgeOrigin::Same(_)) {
                continue;
            }
            for v in [self.edges[e].a, self.edges[e].b] {
                let same_node = out_of
                    .get(v)
                    .iter()
                    .find_map(|&(f, forward)| match origin[f] {
                        EdgeOrigin::Same(i) if forward => Some(pm.edges[i].a),
                        EdgeOrigin::Same(i) => Some(pm.edges[i].b),
                        _ => None,
                    });
                if let Some(p) = same_node {
                    dirty[p] = true;
                }
            }
        }
        // 変わらない輪郭はそのまま（この地図の辺の番号にする）
        let mut used = vec![false; 2 * self.edges.len()];
        let mut cycles: Vec<Cycle> = Vec::new();
        // 輪郭ごとの、前の地図の同じ輪郭の番号
        let mut kept: Vec<Option<usize>> = Vec::new();
        'kept: for (pk, c) in prev.cycles().iter().enumerate() {
            let mut mapped = Vec::with_capacity(c.len());
            for &(i, forward) in c {
                let head = if forward {
                    pm.edges[i].b
                } else {
                    pm.edges[i].a
                };
                match back[i] {
                    Some(e) if !dirty[head] => mapped.push((e, forward)),
                    _ => continue 'kept,
                }
            }
            for &h in &mapped {
                used[idx(h)] = true;
            }
            cycles.push(mapped);
            kept.push(Some(pk));
        }
        // 残りの向きのある辺をたどる（`face_cycles_with` と同じ決まり。頂点のまわりの辺は、初めてたどるときに並べる）
        let dir = |h: (usize, bool)| -> IPt {
            let mut it = half_points(polys, h);
            let a = it.next().unwrap_or((0, 0));
            let b = it.next().unwrap_or(a);
            (b.0 - a.0, b.1 - a.1)
        };
        let head = |h: (usize, bool)| {
            if h.1 {
                self.edges[h.0].b
            } else {
                self.edges[h.0].a
            }
        };
        let mut ready = vec![false; self.nodes.len()];
        let mut pos = vec![0u32; 2 * self.edges.len()];
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
                    let v = head(cur);
                    if !ready[v] {
                        ready[v] = true;
                        #[cfg(test)]
                        VERTICES_SORTED.with(|n| n.set(n.get() + 1));
                        let list = out_of.get_mut(v);
                        list.sort_by(|&x, &y| {
                            cmp_dir(wide(dir(x)), wide(dir(y))).then(idx(x).cmp(&idx(y)))
                        });
                        for (k, &h) in list.iter().enumerate() {
                            pos[idx(h)] = k as u32;
                        }
                    }
                    let list = out_of.get(v);
                    let k = pos[idx((cur.0, !cur.1))] as usize;
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
                kept.push(None);
            }
        }
        let order = sort_cycles(&mut cycles);
        let kept = order.iter().map(|&k| kept[k]).collect();
        (cycles, kept)
    }
}

impl PlanarMap {
    /// テスト用：正しい地図 `prev` の輪郭から求めた面の輪郭（`face_cycles_since`）が、全部たどったもの
    /// （`face_cycles_with`）と同じか（正しくない地図でも比べる）
    #[cfg(test)]
    pub(crate) fn cycles_since_match_full(&self, prev: &LayerBase) -> bool {
        let (polys, origin) = self.polys_since(Some(prev));
        self.face_cycles_since(&polys, prev, &origin).0 == self.face_cycles_with(&polys)
    }
}

/// 面の輪郭を、`face_cycles_with` と同じ並びにする：輪郭ごとに、たどる順（辺の番号、同じ辺なら向きどおりが先）で最初の
/// 向きのある辺から始め、輪郭はその始めの辺の順に並べる（閉じない輪郭は、たどった順のまま）。戻り値は、並べた後の
/// 輪郭ごとの、並べる前の番号
fn sort_cycles(cycles: &mut Vec<Cycle>) -> Vec<usize> {
    let key = |h: &(usize, bool)| (h.0, !h.1);
    for c in cycles.iter_mut() {
        if c.iter().any(|h| h.0 == usize::MAX) {
            continue;
        }
        if let Some(k) = (0..c.len()).min_by_key(|&k| key(&c[k])) {
            c.rotate_left(k);
        }
    }
    let mut order: Vec<usize> = (0..cycles.len()).collect();
    order.sort_by_key(|&k| cycles[k].first().map(key));
    let mut taken: Vec<Option<Vec<(usize, bool)>>> =
        std::mem::take(cycles).into_iter().map(Some).collect();
    *cycles = order
        .iter()
        .map(|&k| taken[k].take().expect("並べる前の輪郭は1回ずつ"))
        .collect();
    order
}

/// 面の輪郭1つの、面の組み立ての結果（`regions_since`。前の地図から引き継ぐ：`LayerBase::cycle_regions`）
#[derive(Clone, Debug, Default)]
pub(super) struct CycleRegion {
    /// 判定の形の輪（組み立てに使った輪郭だけ：閉じた、色のある輪郭）
    ring: Option<Arc<Ring>>,
    /// 色のある穴の入れ先（外周の輪郭の番号）
    owner: Option<usize>,
}

/// 辺の、正しい地図（前の地図の計算に使う写し）での形（`PlanarMap::polys_since`）
#[derive(Clone, Copy, Debug)]
pub(crate) enum EdgeOrigin {
    /// 前の地図の辺 `i` と、形も向きも同じ
    Same(usize),
    /// 前の地図の辺 `i` と形が同じだが、向きが逆
    Reversed(usize),
    /// 前の地図に無い
    New,
}

/// 確かめた地図の、確かめるときに作ったもの：面と、辺ごとの判定の形と、面の輪郭と、輪郭ごとの組み立ての結果
/// （`checked_since`）
struct Checked {
    faces: Vec<MapFace>,
    polys: Vec<Poly>,
    cycles: Vec<Cycle>,
    cycle_regions: Vec<CycleRegion>,
    /// 辺ごとの、前の地図の同じ辺（`edge_origins`。前の地図が無ければ、みな `New`。前の地図に対して確かめ、書き戻す
    /// 前の道 `written` が使う：テストのお手本だけ）
    #[cfg(test)]
    origin: Vec<EdgeOrigin>,
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
        let _ = self.merge_degree2_indexed(candidates, None);
    }

    /// `merge_degree2` の本体。`built`：この地図の辺の判定の形の索引と、辺の番号 → 索引の番号（曲線に戻したあとの
    /// 索引：`ChainRestorer::finish`）。無ければ初めて要るときに作る。どちらでも、まとめが索引の範囲に収まらなければ、
    /// 範囲を広げて作り直す（索引は範囲にかかる区間と頂点をみな持つので、範囲に収まる調べの答えは、索引の範囲によらない）
    /// 戻り値は、最後に使った判定の形の索引の範囲（索引を使わなければ `None`。ほかの辺を調べたのはこの中だけ：重ね合わせを
    /// 窓で行うとき、窓がこの範囲を覆うかを確かめる）
    fn merge_degree2_indexed(
        &mut self,
        candidates: impl IntoIterator<Item = usize>,
        built: Option<(PolyIndex, Vec<usize>)>,
    ) -> Option<GridRect> {
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
        // 判定の形の索引（受け取らなければ、初めて要るときに作る）と、辺の番号 → 索引の番号。作るときの範囲は、まとめる
        // 候補の頂点につながる辺の外接矩形（まとめた辺が範囲から出るときは、広げて作り直す）
        let (mut index, mut ix_id): (Option<PolyIndex>, Vec<usize>) = match built {
            Some((ix, ids)) => (Some(ix), ids),
            None => (None, (0..n).collect()),
        };
        debug_assert_eq!(ix_id.len(), n, "辺ごとの索引の番号");
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
        index.and_then(|ix| ix.area)
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
    #[cfg(test)]
    KEYS_BUILT.with(|n| n.set(n.get() + 1));
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

/// 確定した地図 `next`（面は `faces`）を、元の図形 `orig_shapes`（地図 `orig` を作った図形。`origin` は `next` の辺ごとの
/// `orig` の同じ辺：`edge_origins`）に書き戻すときの差分。戻り値は（消す元の図形, 新しく入れる塗りの形）。
/// - 面のすべての辺の形が残り、面のすべての辺の側の印が同じ1つの元の図形で、その図形と色も同じで、その図形のどの辺も
///   消えていないなら、その面は元の図形のまま残す
/// - それ以外の面は、新しい塗りの形にして入れる。新しい形は、辺の側の印のうち、残した面の図形でないものでいちばん多い
///   元の図形（同じ数なら小さい番号：`top_vote`。重ね合わせで面に伝える番号と同じ決まり）を引き継ぐ形として元の番号を
///   使う（同じ元の図形を引き継ぐ面が2つ以上あれば、その印の付いた側がいちばん多い面。同じなら、`keeps_label_over` と
///   同じく形の鍵 `face_key` の小さい面）。面が分かれても、まとまっても（動かした片が残りとつながるなど）、長い輪郭の
///   側が番号を保つので、地図の、その面のほかの辺の印も変わらない（しまう地図で、変わった場所の塊だけが新しくなる）
/// - 残した面の元の図形のほかは、みな消す（どの面にも残らない元の図形が、新しい形と重なって残ることは無い。番号を
///   引き継いだ形は、消した元の図形と入れ替わる）
///
/// 戻り値の3つ目は、面ごとの図形の番号（残した面は元の図形、ほかは新しい形の番号。`faces` の順）。番号はみな別
/// （同じ元の図形を残す・引き継ぐ面はそれぞれ1つだけ、新しい形には新しい番号）なので、面と図形が1対1になる
/// 書き戻しで、面が元の図形と番号をそのまま残すか（残す面の決まりはこれ1つ：全体の道 `rewrite_shapes`・1本の道
/// `LocalMap::rewrite`・切り出した片の番号 `LayerBase::piece_ids`。引き方だけが違う）。面の輪郭の向きのある辺 `halves` の
/// 側の番号 `new_src` がみな1つの番号 s で、その面の側の元の地図の番号 `old_src`（形が同じ元の辺の同じ側。そういう辺が
/// 無ければ `None`）もみな s で、面の色 `color` が元の図形 s の色 `orig_color(s)` と同じで、元の図形 s の辺がみな残っている
/// （`cut(s)` でない）とき、s（同じ形の面が入れ替わっても、元のその面の所にある面だけが、その面の図形を残す）。同じ s を
/// 残せるのは1つの面だけ（使う側が決める）
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

fn rewrite_shapes(
    orig: &PlanarMap,
    origin: &[EdgeOrigin],
    next: &PlanarMap,
    faces: &[MapFace],
    orig_shapes: &[&FillShape],
    layer_id: u64,
    group_path: &[u64],
) -> (Vec<ShapeId>, Vec<FillShape>, Vec<ShapeId>) {
    // `next` の辺 `e` の形が `orig` にあるか
    let same = |e: usize| !matches!(origin[e], EdgeOrigin::New);
    // 辺が消えた元の図形
    let mut remains = vec![false; orig.edges.len()];
    for o in origin {
        if let EdgeOrigin::Same(i) | EdgeOrigin::Reversed(i) = *o {
            remains[i] = true;
        }
    }
    let mut cut: HashSet<ShapeId> = HashSet::new();
    for (ed, _) in orig.edges.iter().zip(&remains).filter(|(_, r)| !**r) {
        cut.extend(ed.left_src);
        cut.extend(ed.right_src);
    }
    // 元の図形の色（番号ごと）。残す面は、元の図形と番号・色まで同じ面だけ（印が重ねた片や色を変えた片から来ても、元の
    // 図形でなければ新しい図形にする。残す面はいつも元の図形にあるので、`MapChange::into_face_shapes` が図形を引ける）
    let orig_color: HashMap<ShapeId, Color32> =
        orig_shapes.iter().map(|s| (s.shape_id, s.color)).collect();
    let mut changed: Vec<usize> = Vec::new();
    let mut kept: HashSet<ShapeId> = HashSet::new();
    let mut ids: Vec<ShapeId> = vec![0; faces.len()];
    // 向きのある辺 `h` の側の、`orig` の同じ形の辺の同じ側の番号
    let old_src = |h: (usize, bool)| match origin[h.0] {
        EdgeOrigin::Same(i) => orig.side_src((i, h.1)),
        EdgeOrigin::Reversed(i) => orig.side_src((i, !h.1)),
        EdgeOrigin::New => None,
    };
    for (i, f) in faces.iter().enumerate() {
        let keep = kept_src(
            f.cycles.iter().flatten().copied(),
            |h| next.side_src(h),
            old_src,
            f.color,
            |s| orig_color.get(&s).copied(),
            |s| cut.contains(&s),
        );
        // 同じ元の図形を残せるのは1つの面だけ（面の番号がみな別になる）
        match keep {
            Some(s) if kept.insert(s) => ids[i] = s,
            _ => changed.push(i),
        }
    }
    // 形の変わらない輪郭（触っていない穴など）は、元の図形の輪郭をそのまま使い回す
    let reuse = |cycle: &[(usize, bool)]| -> Option<Contour> {
        if !cycle.iter().all(|h| same(h.0)) {
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
    // 元の図形を引き継ぐ面：面ごとに、面の側の印のうち、残す面の図形でないものでいちばん多い元の図形（`top_vote`。
    // 重ね合わせで面に伝える番号と同じ決まり。面が分かれても、まとまっても、長い輪郭の側が番号を保つ）。同じ元の図形が
    // いちばん多い面が2つ以上あれば、その印の付いた側がいちばん多い面（同じ数なら形の鍵 `face_key` の小さい面。
    // `keeps_label_over` と同じく、地図の番号の順で決めない）が引き継ぐ：（元の図形 → (その印の付いた側の数, 面)）
    let mut heir: HashMap<ShapeId, (usize, usize)> = HashMap::new();
    let mut choice: Vec<Option<ShapeId>> = vec![None; faces.len()];
    for &i in &changed {
        let mut votes: Vec<(ShapeId, usize)> = Vec::new();
        for h in faces[i].cycles.iter().flatten() {
            if let Some(src) = next.side_src(*h).filter(|s| !kept.contains(s)) {
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
                || (sides == best.0 && face_key(next, &faces[i]) < face_key(next, &faces[best.1])))
        {
            *best = (sides, i);
        }
    }
    let added = changed
        .into_iter()
        .map(|i| {
            let f = &faces[i];
            let mut s = next.face_shape_with(f, layer_id, group_path, reuse);
            if let Some(src) = choice[i].filter(|src| heir.get(src).is_some_and(|h| h.1 == i)) {
                s.shape_id = src;
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

/// 番号の票（番号, 数）に、番号 `id` の1票を足す
fn add_vote(votes: &mut Vec<(ShapeId, usize)>, id: ShapeId) {
    match votes.iter_mut().find(|v| v.0 == id) {
        Some(v) => v.1 += 1,
        None => votes.push((id, 1)),
    }
}

/// 票のうち、いちばん多い番号とその数（同じ数なら小さい番号）。面の番号を票で決める決まりはこれ1つ（重ね合わせで面に
/// 伝える番号、書き戻しで面が引き継ぐ番号：`rewrite_shapes`）
fn top_vote(votes: &[(ShapeId, usize)]) -> Option<(ShapeId, usize)> {
    votes
        .iter()
        .copied()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
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

/// 差分（窓の結果, 消えた辺, 増えた辺）：`LayerBase::apply_diff` に渡すもの
type LocalDiff = (PlanarMap, Vec<usize>, Vec<usize>);

/// 地図 `m` の辺 `edges` を、塊の辺の形にする（差分を当てる `StoredMap::with_diff_tiles` に渡すため）
fn tile_edges_of(m: &PlanarMap, edges: &[usize]) -> Vec<crate::stored_map::TileEdge> {
    edges
        .iter()
        .map(|&e| {
            let ed = m.edges[e];
            crate::stored_map::TileEdge {
                a: m.nodes[ed.a],
                b: m.nodes[ed.b],
                ctrl: ed.ctrl,
                left: ed.left,
                right: ed.right,
                left_src: ed.left_src,
                right_src: ed.right_src,
            }
        })
        .collect()
}

/// 道具が作る、重ね合わせに渡すもの（`LayerBase::paint_change`・`reshape_change`）：重ねるもの・その曲線・その左右の色
/// （輪郭の変形）・決まり。1本の道（`LayerBase::edit_local`）は、これだけを受け取る
pub(crate) struct LocalChange<'a> {
    input: ChangeInput<'a>,
    quads: Vec<QuadBez>,
    cols: Vec<(Option<Color32>, Option<Color32>)>,
    rule: MapRule,
}

/// 重ねるもの（`MapInput` の、持ち物にしたもの）
enum ChangeInput<'a> {
    Shapes(&'a [FillShape]),
    Sweep(Vec<(usize, QuadBez)>),
    Map(&'a PlanarMap),
}

impl LocalChange<'_> {
    fn input(&self) -> MapInput<'_> {
        match &self.input {
            ChangeInput::Shapes(s) => MapInput::Shapes(s),
            ChangeInput::Sweep(m) => MapInput::Sweep(m),
            ChangeInput::Map(p) => MapInput::Map(p),
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
    pub(crate) faces: Arc<[Arc<FillShape>]>,
    /// 前の表から消した図形の番号と、新しい表に入れた図形の番号（浮いた片の残りが、隠す図形と出す図形を決めるため）
    pub(crate) removed: Vec<ShapeId>,
    pub(crate) added: Vec<ShapeId>,
}

/// お手本との比べ（設計 段2b の 0.）：2つの結果の地図の並びが、しまう形（頂点と辺の番号によらない形：座標・制御点・
/// 色・図形の番号がビットまで同じ）で同じか。どちらも無い（重ねられない）のも同じ
#[cfg(test)]
pub(crate) fn same_results(a: Option<&[PlanarMap]>, b: Option<&[PlanarMap]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter().zip(b).all(|(x, y)| {
                    StoredMap::from_dense(x, None).same_content(&StoredMap::from_dense(y, None))
                })
        }
        (None, None) => true,
        _ => false,
    }
}

/// お手本の計算 `f` を、手間を数える試しの数に入れずに行う（このスレッドの数を、行う前の値に戻す）
#[cfg(test)]
pub(crate) fn uncounted<R>(f: impl FnOnce() -> R) -> R {
    type Counter = std::thread::LocalKey<std::cell::Cell<usize>>;
    let counters: [&'static Counter; 13] = [
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

impl PlanarMap {
    #[cfg(test)]
    /// 編集で作った、セル画に書き戻す地図を確かめたもの（`valid_faces_since`：読み込みと同じ判定で、書き戻す先の今の
    /// 地図 `prev` にある辺どうしは確かめ直さない）。確かめは地図を作った所でこの1回だけで、書き戻し（`LayerBase::diff`）は
    /// 確かめた地図を受け取る。`None`：正しい地図でない（書き戻さずに断る。書けた地図は、保存して開いても必ず読める）。
    /// テストでは、正しい地図でない結果を作った操作を、そこで見つける
    pub(crate) fn written(self, prev: &LayerBase) -> Option<CheckedMap> {
        let checked = self.checked_since(Some(prev));
        #[cfg(test)]
        if test_checks() {
            assert!(
                checked.is_some(),
                "書き戻す地図が正しい地図でない: {}",
                self.validity_report()
            );
        }
        let Checked {
            faces,
            polys,
            cycles,
            cycle_regions,
            origin,
        } = checked?;
        Some(CheckedMap {
            map: self,
            faces,
            polys,
            cycles,
            cycle_regions,
            since: Some((prev.stored.clone(), origin)),
        })
    }
}

// ============================================================================
// 計算に使う写しと、地図を書き戻すときの差分（どの編集も使う）
// ============================================================================

/// 1つのレイヤー・階層の地図の、計算に使う写し：ふつうの形（いつもしまう形から決まった順で作ったもの：
/// `StoredMap::to_dense`）と、図形に書き戻すときの下ごしらえ（辺の鍵。書き戻すときに1回だけ作る）と、辺ごとの判定の形
/// （地図ごとに1回だけ作る。編集で作った地図は、確かめるときに作った判定の形を引き継ぐ：`StoredGeom`）と、面の図形（写し。
/// 初めて要るときに作る：`LayerBase::faces`）。写しは、それを使う所が持つ（画面の土台・地図の編集の間・カーソル合わせ）。
/// 文書と Undo の控えは、しまう形（`StoredMap`）だけを持つ
#[derive(Debug)]
pub(crate) struct LayerBase {
    /// 計算に使うふつうの地図（しまう形から決まった順で作ったもの：`StoredMap::to_dense`）。要る所が初めて使うときに作る
    /// （`map`。差分から作った写しは、これを作らずに使える：設計 段2b の 7. の 2b-7c）
    dense: OnceLock<PlanarMap>,
    pub(crate) stored: StoredMap,
    keys: OnceLock<EdgeKeys>,
    /// 辺ごとの判定の形（`map` の辺の順）
    polys: OnceLock<Vec<Poly>>,
    /// 面の輪郭（`face_cycles_with`）
    cycles: OnceLock<Vec<Cycle>>,
    /// 輪郭ごとの組み立ての結果（`regions_since`。`cycles` の順）
    cycle_regions: OnceLock<Vec<CycleRegion>>,
    /// 面の図形：（階層, 面ごとの図形。番号は面の印。番号の順）
    faces: SyncCache<Option<FaceShapes>>,
    /// 升目の索引（初めて要るときに作る。地図を書き換えたあとの写しは、前の写しの索引から変わった所だけ作り直したもの
    /// を持つ：`with_faces`・`of_stored_after`）
    index: OnceLock<crate::stored_map::TileIndex>,
    /// 端の升目の索引（升目ごとの、端がその升目にある辺。頂点の辺を引く所が使う：`end_tiles`。作り方は `index` と同じ）
    ends: OnceLock<crate::stored_map::TileIndex>,
}

/// しまう形の辺の順（`StoredMap::to_dense` の辺の順）に並べた、辺ごとの判定の形と面の輪郭
/// （`CheckedMap::into_stored_with_polys`。確かめた地図のものを、しまう形から作った写しに引き継ぐ）
pub(crate) struct StoredGeom {
    /// しまう形から作る地図（`StoredMap::to_dense` と同じもの：`StoredMap::dense_in_order`）
    map: PlanarMap,
    polys: Vec<Poly>,
    cycles: Vec<Cycle>,
    regions: Vec<CycleRegion>,
}

/// 地図の面の図形（写し）：（階層, 面の番号の順の図形）
type FaceShapes = (Vec<u64>, Arc<[Arc<FillShape>]>);

impl LayerBase {
    /// 地図 `map` を、しまう形にして決まった順に並べ直したもの（文書に入らない、その場で使う地図）
    pub(crate) fn new(map: &PlanarMap) -> Self {
        Self::from_stored(StoredMap::from_dense(map, None))
    }

    fn from_stored(stored: StoredMap) -> Self {
        #[cfg(test)]
        DENSE_BUILDS.with(|n| n.set(n.get() + 1));
        Self::of_dense(stored.to_dense(), stored)
    }

    /// しまった地図 `stored` と、その計算に使う地図 `map`（`stored.to_dense()` と同じもの）の写し
    fn of_dense(map: PlanarMap, stored: StoredMap) -> Self {
        Self {
            dense: OnceLock::from(map),
            stored,
            keys: OnceLock::new(),
            polys: OnceLock::new(),
            cycles: OnceLock::new(),
            cycle_regions: OnceLock::new(),
            faces: SyncCache::new(None),
            index: OnceLock::new(),
            ends: OnceLock::new(),
        }
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
    /// `MapChange::into_face_shapes`。変わらない面は前の図形、形の変わった面は番号を引き継いだ新しい図形）と、辺ごとの判定の
    /// 形 `polys`（`CheckedMap::into_stored_with_polys`）で作る（地図の編集のあと。面も判定の形も求め直さない。画面の描画
    /// キャッシュは、番号と版が同じ図形から引き継がれる）
    pub(crate) fn with_faces(
        stored: StoredMap,
        group_path: &[u64],
        mut shapes: Vec<Arc<FillShape>>,
        polys: StoredGeom,
        prev: Option<&LayerBase>,
    ) -> Arc<Self> {
        let base = Self::of_dense(polys.map, stored);
        debug_assert_eq!(polys.polys.len(), base.map().edges.len());
        #[cfg(test)]
        if test_checks() {
            for (e, p) in polys.polys.iter().enumerate() {
                assert!(
                    p[..] == base.stored.poly(e)[..],
                    "塊の判定の形が、写しの一覧の判定の形と違う（辺 {e}）"
                );
            }
        }
        let _ = base.polys.set(polys.polys);
        let _ = base.cycles.set(polys.cycles);
        let _ = base.cycle_regions.set(polys.regions);
        shapes.sort_unstable_by_key(|s| s.shape_id);
        *base.faces.lock() = Some((group_path.to_vec(), shapes.into()));
        base.inherit_index(prev);
        let base = Arc::new(base);
        *base
            .stored
            .dense_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&base);
        base
    }

    /// しまった地図 `stored` の計算に使う写し（`of_stored` と同じ）。新しく作るときは、写し `prev`（`stored` と塊を共有
    /// する地図の写し）の升目の索引から、変わった塊の分だけ作り直した索引を持たせる
    pub(crate) fn of_stored_after(stored: &StoredMap, prev: &LayerBase) -> Arc<Self> {
        let mut slot = stored
            .dense_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
        faces: Arc<[Arc<FillShape>]>,
        prev: &LayerBase,
    ) -> Arc<Self> {
        let base = Self {
            dense: OnceLock::new(),
            stored,
            keys: OnceLock::new(),
            polys: OnceLock::new(),
            cycles: OnceLock::new(),
            cycle_regions: OnceLock::new(),
            faces: SyncCache::new(Some((group_path.to_vec(), faces))),
            index: OnceLock::new(),
            ends: OnceLock::new(),
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
        *base
            .stored
            .dense_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&base);
        base
    }

    /// しまった地図 `stored` の、いま誰かが持っている計算に使う写し（無ければ作らずに `None`）
    pub(crate) fn live(stored: &StoredMap) -> Option<Arc<Self>> {
        stored
            .dense_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade()
    }

    /// 写し `prev` が升目の索引を持っていれば、そこから、この写しの索引を作る（変わった塊の辺がかかる升目だけを作り
    /// 直す：`TileIndex::derive`）。持っていなければ何もしない（初めて要るときに全部作る）
    fn inherit_index(&self, prev: Option<&LayerBase>) {
        let Some(prev) = prev else {
            return;
        };
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

    /// まだ升目の索引を持っていなければ、写し `donors` のうち索引を持ち、この地図といちばん多く塊を共有するものから作る
    /// （Undo・やり直しで画面の土台が写しを作り直すとき、直前まで画面に出していた写しから。共有する塊が無ければ何も
    /// しない：初めて要るときに全部作る）
    pub(crate) fn adopt_index(&self, donors: &[Arc<LayerBase>]) {
        if self.index.get().is_some() {
            return;
        }
        let best = donors
            .iter()
            .filter(|d| d.index.get().is_some() && !d.stored.same(&self.stored))
            .map(|d| (self.stored.shared_tile_count(&d.stored), d))
            .filter(|(n, _)| *n > 0)
            .max_by_key(|(n, _)| *n);
        if let Some((_, d)) = best {
            self.inherit_index(Some(d));
        }
    }

    /// 辺 `e` がかかる、升目の索引の升目：辺の近くを探すマス（`EdgeGeom::reach`）の升目と、外接矩形（`reach_box`）にかかる
    /// 升目。マスで探す所（`NearEdges`）も、外接矩形が重なるかで探す所（重ね合わせを窓で行うときの窓）も、その升目を引けば
    /// 必ず見つかる
    fn edge_tiles(&self, e: usize) -> Vec<(i64, i64)> {
        // 写しの一覧を作らずに、塊の判定の形と辺から（差分から作った写しでも、一覧を作らない）
        let (bbox, cells) = self.stored.poly(e).reach(self.stored.edge_at(e).quad());
        let mut t = index_tiles(cells);
        t.extend(index_tiles_over(*bbox));
        t.sort_unstable();
        t.dedup();
        t
    }

    /// 頂点 `v` から出ていく向きのある辺（辺の番号, 辺の向きに出ていくか）を、角度の順に並べたもの（`face_cycles_with` と
    /// 同じ並べ方：判定の形の最初の区間の向き、同じなら番号の順）。頂点の辺は升目の索引で引き、判定の形は塊から引く
    /// （写しの一覧を作らない。設計 段2b の 4. の 2b-3）
    #[cfg(test)]
    fn out_sorted(&self, v: usize) -> Vec<(usize, bool)> {
        let idx = |h: (usize, bool)| 2 * h.0 + h.1 as usize;
        let dir = |h: (usize, bool)| -> IPt {
            let p = self.stored.poly(h.0);
            let n = p.len();
            let at = |k: usize| if h.1 { p[k] } else { p[n - 1 - k] };
            let a = if n > 0 { at(0) } else { (0, 0) };
            let b = if n > 1 { at(1) } else { a };
            (b.0 - a.0, b.1 - a.1)
        };
        let mut list: Vec<(usize, bool)> = Vec::new();
        for e in self.map().edges_at_in(v, Some(self.map_index())) {
            let ed = self.map().edges[e];
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

    /// 向きのある辺 `start` の左の面の輪郭を、その場でたどる（`face_cycles_with` と同じ決まり：次は、入ってきた辺を
    /// 引き返す向きの、角度の順で1つ前）。輪郭は、いちばん小さい向きのある辺（番号, 逆向きが後）から始める（`sort_cycles`
    /// と同じ回し方）。手間は輪郭の長さの分で、写しの輪郭の一覧を作らない（設計 段2b の 4. の 2b-3）
    #[cfg(test)]
    pub(crate) fn trace_cycle(&self, start: (usize, bool)) -> Vec<(usize, bool)> {
        let mut around: HashMap<usize, Vec<(usize, bool)>> = HashMap::new();
        let mut cycle = Vec::new();
        let mut cur = start;
        loop {
            cycle.push(cur);
            let ed = self.map().edges[cur.0];
            let head = if cur.1 { ed.b } else { ed.a };
            let list = around.entry(head).or_insert_with(|| self.out_sorted(head));
            let back = (cur.0, !cur.1);
            let k = list
                .iter()
                .position(|&h| h == back)
                .expect("頂点から出ていく辺に入っている");
            let next = list[(k + list.len() - 1) % list.len()];
            if next == start {
                break;
            }
            assert!(cycle.len() <= 2 * self.map().edges.len(), "輪郭が閉じない");
            cur = next;
        }
        let key = |h: &(usize, bool)| (h.0, !h.1);
        if let Some(k) = (0..cycle.len()).min_by_key(|&k| key(&cycle[k])) {
            cycle.rotate_left(k);
        }
        cycle
    }

    /// 道具の側（ブラシ・消しゴム）：形 `shapes`（文書の座標）を決まり `rule` で重ねるときに、重ね合わせに渡すもの。形の線の
    /// 端は、前からある頂点から `FLAT_TOL` 以内なら、その頂点に寄せる（`overlay` の形と同じ）
    pub(crate) fn paint_change<'a>(
        &self,
        shapes: &'a [FillShape],
        rule: MapRule,
    ) -> LocalChange<'a> {
        let mut quads: Vec<QuadBez> = shapes
            .iter()
            .flat_map(|s| s.contours())
            .flat_map(|c| c.segments.iter().copied())
            .collect();
        self.snap_ends(&mut quads);
        LocalChange {
            input: ChangeInput::Shapes(shapes),
            quads,
            cols: Vec::new(),
            rule,
        }
    }

    /// 道具の側（輪郭の変形）：辺の引っぱり・頂点の移動 `edit` で、重ね合わせに渡すもの（動かした辺と、動かした後の曲線。
    /// `MapEdit::apply_known` と同じ決まり。辺・頂点は塊から引く）。アンカーの追加・削除は重ね合わせでないので `None`。
    /// 辺・頂点が無ければ `Some(None)`
    pub(crate) fn reshape_change(&self, edit: MapEdit) -> Option<Option<LocalChange<'static>>> {
        let moved: Vec<(usize, QuadBez)> = match edit {
            MapEdit::PullEdge {
                orig_seg,
                grab_t,
                target,
            } => {
                let Some((e, forward)) = self.stored.find_edge(&orig_seg) else {
                    return Some(None);
                };
                let t = if forward { grab_t } else { 1.0 - grab_t };
                vec![(e, pulled_quad(self.stored.edge_at(e).quad(), t, target))]
            }
            MapEdit::MoveVertex {
                orig_vertex,
                target,
            } => {
                let ix = self.map_index();
                let mut at: Vec<usize> = Vec::new();
                ix.ends
                    .edges_in(&self.stored, end_key(orig_vertex), &mut at);
                at.sort_unstable();
                at.dedup();
                at.retain(|&e| {
                    let te = self.stored.edge_at(e);
                    te.a == orig_vertex || te.b == orig_vertex
                });
                if at.is_empty() {
                    return Some(None);
                }
                let d = target - orig_vertex;
                let pos = |p: Point| if p == orig_vertex { target } else { p };
                at.iter()
                    .map(|&e| {
                        let te = self.stored.edge_at(e);
                        let k = (te.a == orig_vertex) as u8 + (te.b == orig_vertex) as u8;
                        (
                            e,
                            QuadBez::new(pos(te.a), te.ctrl + d * (0.5 * k as f64), pos(te.b)),
                        )
                    })
                    .collect()
            }
            MapEdit::AddAnchor { .. } | MapEdit::DeleteAnchor { .. } => return None,
        };
        let quads = moved.iter().map(|&(_, q)| q).collect();
        let cols = moved
            .iter()
            .map(|&(e, _)| {
                let te = self.stored.edge_at(e);
                (te.left, te.right)
            })
            .collect();
        Some(Some(LocalChange {
            input: ChangeInput::Sweep(moved),
            quads,
            cols,
            rule: MapRule::Sweep,
        }))
    }

    /// 1本の道：道具が作った重ね合わせに渡すもの `change` で、地図を触った所だけで編集する（設計 段2b の 7. の 2b-7c と
    /// 8.）：窓で重ね（`window_overlay`）、差分を前の地図＋差分として確かめ（`LocalMap`）、触った面の書き戻しの差分を決め
    /// （`rewrite`）、前のしまう形に差分を当てた新しいしまう形と、新しい面の図形の表を作る。道具の種類では分かれない。地図
    /// 全体の一覧（`map`）を作らない。答えは、同じものを前の地図全体で重ねて確かめ、書き戻すとき（`overlay_rules_known`・
    /// `written`・`Cel::outcome`・`put_map`）と同じ（テストでは毎回お手本と比べる）。`group_path`：この地図の階層
    pub(crate) fn edit_local(&self, change: &LocalChange<'_>, group_path: &[u64]) -> LocalEdit {
        let got = self.edit_local_inner(change, group_path);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| self.edit_matches_full(change, group_path, &got));
            assert!(
                ok,
                "触った所だけで行った編集が、前の地図全体で行った編集（お手本）と違う"
            );
        }
        got
    }

    fn edit_local_inner(&self, change: &LocalChange<'_>, group_path: &[u64]) -> LocalEdit {
        match self.overlay_diff(change) {
            Ok(None) => LocalEdit::Same,
            Ok(Some((m, removed, added))) => self.apply_diff(&m, &removed, &added, group_path),
            Err(r) => LocalEdit::Refused(r),
        }
    }

    /// 重ね合わせの差分（`edit_local` が差分から先に渡すもの）：窓で重ね（`window_overlay`）、窓の結果と窓の辺の違いを
    /// 差分にする。（窓の結果, 消えた辺, 増えた辺）。重ねるものが無ければ `None`。`Err`：格子に置けない
    fn overlay_diff(&self, change: &LocalChange<'_>) -> Result<Option<LocalDiff>, LocalRefusal> {
        if change.quads.is_empty() {
            return Ok(None);
        }
        let ix = self.map_index();
        let (maps, wedges) = PlanarMap::window_overlay(
            ix,
            change.input(),
            &[change.rule],
            &change.quads,
            &change.cols,
        )
        .ok_or(LocalRefusal::OffGrid)?;
        let [m]: [PlanarMap; 1] = maps.try_into().expect("決まりごとに1つの地図");
        let (removed, added) = self.stored.window_diff_tiles(&wedges, &m);
        Ok(Some((m, removed, added)))
    }

    /// 道具の側（矩形で切る）：選ぶ所 `region`（形か地図）で、この写しの地図を、中だけ残した地図（片。確かめる前の、窓の
    /// 結果）と、中を消した地図（残り。前のしまう形に窓の差分を当てたしまう形。確かめと書き戻しは、使う側がセル画の地図に
    /// 対して1本の道で行う：`cut_out`）に分ける。窓で重ねるのは1回（2つの決まりを一度に）。`None`：格子に置けない
    pub(crate) fn cut_local(&self, region: MapInput<'_>) -> Option<(PlanarMap, StoredMap)> {
        let (quads, cols): (Vec<QuadBez>, Vec<Sides>) = match region {
            MapInput::Shapes(shapes) => {
                let mut q: Vec<QuadBez> = shapes
                    .iter()
                    .flat_map(|s| s.contours())
                    .flat_map(|c| c.segments.iter().copied())
                    .collect();
                self.snap_ends(&mut q);
                (q, Vec::new())
            }
            MapInput::Map(p) => (
                (0..p.edges.len()).map(|e| p.quad(e)).collect(),
                p.edges.iter().map(|e| (e.left, e.right)).collect(),
            ),
            MapInput::Sweep(_) | MapInput::Stack(_) => return None,
        };
        if quads.is_empty() {
            return Some((PlanarMap::default(), self.stored.clone()));
        }
        let rules = [MapRule::KeepInside, MapRule::Paint(None)];
        let (maps, wedges) =
            PlanarMap::window_overlay(self.map_index(), region, &rules, &quads, &cols)?;
        let [inside, rest]: [PlanarMap; 2] = maps.try_into().expect("決まりごとに1つの地図");
        let (removed, added) = self.stored.window_diff_tiles(&wedges, &rest);
        let rest = self
            .stored
            .with_diff_tiles(&removed, &tile_edges_of(&rest, &added));
        // お手本：中だけ残した地図と、前のしまう形に残りの差分を当てた形が、前の地図全体で重ねた結果と同じ
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                match self
                    .map()
                    .overlay_rules_known(self.polys(), None, region, rules)
                {
                    Some([fi, fr]) => {
                        StoredMap::from_dense(&inside, None)
                            .same_content(&StoredMap::from_dense(&fi, None))
                            && rest.same_content(&StoredMap::from_dense(&fr, None))
                    }
                    None => false,
                }
            });
            assert!(
                ok,
                "矩形で切る窓の答えが、前の地図全体で重ねた結果（お手本）と違う"
            );
        }
        Some((inside, rest))
    }

    /// 道具の側（片を置く・貼り付け）：地図 `p` を重ねる（`MapRule::Overlay`）とき、重ね合わせに渡すもの
    pub(crate) fn map_change<'a>(&self, p: &'a PlanarMap) -> LocalChange<'a> {
        LocalChange {
            input: ChangeInput::Map(p),
            quads: (0..p.edges.len()).map(|e| p.quad(e)).collect(),
            cols: p.edges.iter().map(|e| (e.left, e.right)).collect(),
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
        let maps: Vec<Option<PlanarMap>> = maps.into_iter().collect();
        let got = self.stacked_local_inner(&maps);
        #[cfg(test)]
        if test_checks() {
            // お手本：地図全体で1枚ずつ重ねる（`stacked`）。ただし、重ねた途中の地図は、しまう形の辺の向きにしてから次を
            // 重ねる（重ね合わせの答えは、辺と頂点の番号にはよらないが、辺の向きにはよることがある：段0 のテスト
            // edits_do_not_depend_on_edge_and_node_numbering の前提。セル画の地図はいつもしまう形の向きなので、1枚目は
            // これまでの道と同じ。2枚目からは、途中の地図の向きが重ね合わせの出した向きのままだった前の道と違いうる）
            let ok = uncounted(|| {
                let mut full = Some(self.map().clone());
                for m in &maps {
                    full = full.and_then(|c| {
                        let m = m.as_ref()?;
                        let n = if c.edges.is_empty() {
                            m.clone()
                        } else {
                            c.overlay(MapInput::Map(m), MapRule::Overlay)?
                        };
                        Some(StoredMap::from_dense(&n, None).to_dense())
                    });
                }
                match (&got, full) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.same_content(&StoredMap::from_dense(&b, None)),
                    _ => false,
                }
            });
            assert!(
                ok,
                "片を1枚ずつ窓で重ねたしまう形が、地図全体で重ねた地図（お手本）と違う"
            );
        }
        got
    }

    fn stacked_local_inner(self: &Arc<Self>, maps: &[Option<PlanarMap>]) -> Option<StoredMap> {
        // 重ねる写し（次の1枚を重ねるときに作る。最後の1枚のあとは、しまう形だけでよい）
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

    /// 道具の側（片を置く確定）：この写し（セル画の地図）の地図を、しまう形 `next`（セル画の地図と塊を共有するもの：
    /// `stacked_local`）にする差分を作り、1本の道の差分から先（`apply_diff`）に渡す。差分は、共有しない塊の分だけ比べて
    /// 作る（`StoredMap::diff_tiles`）
    pub(crate) fn replace_local(&self, next: &StoredMap, group_path: &[u64]) -> LocalEdit {
        let (removed, added) = self.stored.diff_tiles(next);
        let mut m = PlanarMap::default();
        let mut at: HashMap<[u64; 2], usize> = HashMap::new();
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
            });
        }
        let added: Vec<usize> = (0..m.edges.len()).collect();
        self.apply_diff(&m, &removed, &added, group_path)
    }

    /// 道具の側（バケツ）：点 `p`（文書の座標）を含む面（色のある面、または同じ階層の線で囲まれた何も無い所）を `color` に
    /// する差分を作り、1本の道の差分から先（`apply_diff`）に渡す（設計 段2b の 9.）。答えは今のバケツ（`with_face_filled`
    /// で地図全体から面を探して塗り、書き戻す道）と同じ（テストでは毎回お手本と比べる）
    pub(crate) fn fill_local(&self, p: Point, color: Color32, group_path: &[u64]) -> LocalEdit {
        let diff = self.fill_diff(p, color, group_path);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| {
                let full = self.with_face_filled(p, color);
                match (&diff, full) {
                    (None, None) => true,
                    (Some((m, removed, added)), Some(fm)) => self
                        .stored
                        .with_diff_tiles(removed, &tile_edges_of(m, added))
                        .same_content(&StoredMap::from_dense(&fm, None)),
                    _ => false,
                }
            });
            assert!(
                ok,
                "バケツの差分が、地図全体から面を探して塗った地図（お手本）と違う"
            );
        }
        match diff {
            None => LocalEdit::Same,
            Some((m, removed, added)) => self.apply_diff(&m, &removed, &added, group_path),
        }
    }

    /// バケツの差分（`fill_local`）：点を含む面（`LocalMap::face_at`）の輪郭の向きのある辺の側を `color` にし、両側が同じ色に
    /// なった辺を消して、2本だけがつながる頂点をまとめる（`with_sides_colored` と同じ）。計算は、面の輪郭の辺と、その端の頂点に
    /// つながる辺と、消える辺のまわりの辺を並べた窓で行う。2本をまとめる所がほかの辺を調べた範囲が窓の範囲から出たら、範囲を
    /// 広げてやり直す（重ね合わせの窓と同じ決まり）。（窓の結果, 消えた辺, 増えた辺）。変わらなければ `None`
    fn fill_diff(
        &self,
        p: Point,
        color: Color32,
        group_path: &[u64],
    ) -> Option<(PlanarMap, Vec<usize>, Vec<usize>)> {
        const WINDOW_PAD: f64 = 2.0;
        let ix = self.map_index();
        let empty = PlanarMap::default();
        let local = LocalMap::new(ix, &empty, &[], &[]);
        let faces_list = self.faces(group_path);
        let lookup = self.faces_lookup(&faces_list);
        let g = gpt(p);
        let mut memo = CycleMemo::default();
        let (face_color, halves) = local.face_at((2 * g.0, 2 * g.1), &lookup, &mut memo)?;
        if face_color == Some(color) {
            return None;
        }
        let stored = &self.stored;
        let hull = |e: usize| {
            let q = stored.edge_at(e).quad();
            KRect::from_points(q.p0, q.p2).union_pt(q.p1)
        };
        let edges_at = |p: Point| self.edges_at_node(p);
        let side_edges: std::collections::BTreeSet<usize> = halves.iter().map(|h| h.0).collect();
        // 消える辺（塗ったあとに両側が同じ色になる辺）と、その端につながる辺の外形
        let mut region: Option<KRect> = None;
        for &(e, forward) in &halves {
            let te = stored.edge_at(e);
            let other = if forward { te.right } else { te.left };
            if other == Some(color) {
                for f in std::iter::once(e)
                    .chain(edges_at(te.a))
                    .chain(edges_at(te.b))
                {
                    let h = hull(f);
                    region = Some(region.map_or(h, |r| r.union(h)));
                }
            }
        }
        let mut region = region.map(|r| r.inflate(WINDOW_PAD, WINDOW_PAD));
        loop {
            let mut in_window: std::collections::BTreeSet<usize> = side_edges.clone();
            for &e in &side_edges {
                let te = stored.edge_at(e);
                in_window.extend(edges_at(te.a));
                in_window.extend(edges_at(te.b));
            }
            if let Some(r) = region {
                let mut found: Vec<usize> = Vec::new();
                for t in index_tiles_over(r) {
                    ix.index.edges_in(stored, t, &mut found);
                }
                in_window.extend(
                    found
                        .into_iter()
                        .filter(|&e| rects_overlap(reach_box(stored.edge_at(e).quad()), r)),
                );
            }
            let wedges: Vec<usize> = in_window.into_iter().collect();
            let (wmap, _) = tile_window(stored, &wedges);
            let to_window: HashMap<usize, usize> =
                wedges.iter().enumerate().map(|(i, &e)| (e, i)).collect();
            let mut m = wmap;
            for &(e, forward) in &halves {
                let w = to_window[&e];
                if forward {
                    m.edges[w].left = Some(color);
                } else {
                    m.edges[w].right = Some(color);
                }
            }
            let dropped_ends: Vec<usize> = m
                .edges
                .iter()
                .filter(|e| e.left == e.right)
                .flat_map(|e| [e.a, e.b])
                .collect();
            m.edges.retain(|e| e.left != e.right);
            let used = m.merge_degree2_indexed(dropped_ends, None);
            m.compact_nodes();
            if let Some(u) = used {
                let u = KRect::new(
                    u.x0 as f64 / OVERLAY_GRID,
                    u.y0 as f64 / OVERLAY_GRID,
                    u.x1 as f64 / OVERLAY_GRID,
                    u.y1 as f64 / OVERLAY_GRID,
                );
                let inside = region
                    .is_some_and(|r| u.x0 >= r.x0 && u.y0 >= r.y0 && u.x1 <= r.x1 && u.y1 <= r.y1);
                if !inside {
                    let grown = region
                        .map_or(u, |r| r.union(u))
                        .inflate(WINDOW_PAD, WINDOW_PAD);
                    region = Some(grown);
                    continue;
                }
            }
            let (removed, added) = stored.window_diff_tiles(&wedges, &m);
            return Some((m, removed, added));
        }
    }

    /// 道具の側（アンカーの追加・削除）：`edit` の差分を作り、1本の道の差分から先（`apply_diff`）に渡す（設計 段2b の 9.）。
    /// 差分は、関わる辺（追加なら分ける辺、削除なら頂点につながる辺）のまわりの窓で、今の `with_anchor_added`・
    /// `with_anchor_deleted` をそのまま使って作る（置き換えてよいかの確かめ `can_replace_path` が調べるのは、関わる辺と
    /// 置き換える曲線の外形の範囲だけなので、その範囲を覆う窓で同じ答え）。編集できなければ（辺・頂点が無い・消せない頂点）
    /// `Same`。答えは今のアンカー（`MapEdit::apply` で地図全体を編集し、書き戻す道）と同じ（テストでは毎回お手本と比べる）
    pub(crate) fn anchor_local(&self, edit: MapEdit, group_path: &[u64]) -> LocalEdit {
        let diff = self.anchor_diff(edit);
        #[cfg(test)]
        if test_checks() {
            let ok = uncounted(|| match (&diff, edit.apply(self.map())) {
                (None, None) => true,
                (Some((m, removed, added)), Some(fm)) => self
                    .stored
                    .with_diff_tiles(removed, &tile_edges_of(m, added))
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
    fn anchor_diff(&self, edit: MapEdit) -> Option<(PlanarMap, Vec<usize>, Vec<usize>)> {
        const WINDOW_PAD: f64 = 2.0;
        let ix = self.map_index();
        let stored = &self.stored;
        let edges_at = |p: Point| self.edges_at_node(p);
        // 関わる辺
        let involved: Vec<usize> = match edit {
            MapEdit::AddAnchor { orig_seg, .. } => vec![stored.find_edge(&orig_seg)?.0],
            MapEdit::DeleteAnchor { orig_vertex } => edges_at(orig_vertex),
            MapEdit::PullEdge { .. } | MapEdit::MoveVertex { .. } => return None,
        };
        if involved.is_empty() {
            return None;
        }
        let region = involved
            .iter()
            .map(|&e| {
                let q = stored.edge_at(e).quad();
                KRect::from_points(q.p0, q.p2).union_pt(q.p1)
            })
            .reduce(|a, b| a.union(b))?
            .inflate(WINDOW_PAD, WINDOW_PAD);
        let mut in_window: std::collections::BTreeSet<usize> = involved.iter().copied().collect();
        let mut found: Vec<usize> = Vec::new();
        for t in index_tiles_over(region) {
            ix.index.edges_in(stored, t, &mut found);
        }
        in_window.extend(
            found
                .into_iter()
                .filter(|&e| rects_overlap(reach_box(stored.edge_at(e).quad()), region)),
        );
        for &e in &involved {
            let te = stored.edge_at(e);
            in_window.extend(edges_at(te.a));
            in_window.extend(edges_at(te.b));
        }
        let wedges: Vec<usize> = in_window.into_iter().collect();
        let (wmap, _) = tile_window(stored, &wedges);
        let m = match edit {
            MapEdit::AddAnchor { orig_seg, t } => {
                let (e, forward) = stored.find_edge(&orig_seg)?;
                let w = wedges.binary_search(&e).ok()?;
                wmap.with_anchor_added(w, if forward { t } else { 1.0 - t })?
            }
            MapEdit::DeleteAnchor { orig_vertex } => {
                let v = wmap.nodes.iter().position(|&p| p == orig_vertex)?;
                wmap.with_anchor_deleted(v)?
            }
            MapEdit::PullEdge { .. } | MapEdit::MoveVertex { .. } => return None,
        };
        let (removed, added) = stored.window_diff_tiles(&wedges, &m);
        Some((m, removed, added))
    }

    /// 前の地図の面の表（`BaseFaces`）：写しの面の図形 `faces_list` から、面の輪郭の最初の辺を形で引く
    fn faces_lookup<'f>(
        &'f self,
        faces_list: &'f [Arc<FillShape>],
    ) -> impl Fn(ShapeId) -> Vec<(usize, bool)> + 'f {
        move |id: ShapeId| match faces_list.binary_search_by_key(&id, |s| s.shape_id) {
            Ok(i) => faces_list[i]
                .contours()
                .filter_map(|c| c.segments.first().and_then(|q| self.stored.find_edge(q)))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 1本の道の、差分から先（設計 段2b の 9.）：差分（`removed`：消えた前の地図の辺、`added`：地図 `result` の増えた辺の
    /// 番号。`result` の頂点は、座標が同じなら前の地図の頂点）を前の地図に当てて確かめ（`LocalMap`）、触った面の書き戻しの
    /// 差分を決め、前のしまう形に差分を当てた新しいしまう形と、新しい面の図形の表を作る。差分の作り方（重ね合わせ・バケツ・
    /// アンカー）や道具の種類では分かれない。答えは、差分を当てた新しい地図を前の道で確かめ、書き戻すときと同じ（テストでは
    /// 毎回お手本と比べる）
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
                let next = self.materialized(result, removed, added);
                self.matches_next(&next, group_path, &got)
            });
            assert!(
                ok,
                "差分から先を触った所だけで行った答えが、差分を当てた地図を前の道で書き戻した答え（お手本）と違う"
            );
        }
        got
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
        let lookup = |id: ShapeId| -> Vec<(usize, bool)> {
            match faces_list.binary_search_by_key(&id, |s| s.shape_id) {
                Ok(i) => faces_list[i]
                    .contours()
                    .filter_map(|c| c.segments.first().and_then(|q| self.stored.find_edge(q)))
                    .collect(),
                Err(_) => Vec::new(),
            }
        };
        let Some(Some(faces)) = local.regions_faces(&touched, &lookup, &mut memo) else {
            return LocalEdit::Refused(LocalRefusal::Unwritable);
        };
        let orig_shape = |id: ShapeId| -> Option<Arc<FillShape>> {
            faces_list
                .binary_search_by_key(&id, |s| s.shape_id)
                .ok()
                .map(|i| Arc::clone(&faces_list[i]))
        };
        let change = local.rewrite(&faces, &lookup, &orig_shape, &memo, 0, group_path);
        if change.removed.is_empty() && change.added.is_empty() {
            return LocalEdit::Same;
        }
        let (relabel, add_edges) = local.relabeled_edges(&faces, &change.ids, &memo);
        let mut all_removed = removed.to_vec();
        all_removed.extend(relabel);
        all_removed.sort_unstable();
        all_removed.dedup();
        let stored = self.stored.with_diff_tiles(&all_removed, &add_edges);
        let added: Vec<Arc<FillShape>> = change
            .added
            .into_iter()
            .map(|mut s| {
                s.strip_transient_caches();
                Arc::new(s)
            })
            .collect();
        let gone: HashSet<ShapeId> = change.removed.iter().copied().collect();
        let mut faces_new: Vec<Arc<FillShape>> = faces_list
            .iter()
            .filter(|s| !gone.contains(&s.shape_id))
            .cloned()
            .chain(added.iter().cloned())
            .collect();
        faces_new.sort_unstable_by_key(|s| s.shape_id);
        LocalEdit::Changed(LocalWrite {
            stored,
            faces: faces_new.into(),
            removed: change.removed,
            added: added.iter().map(|s| s.shape_id).collect(),
        })
    }

    /// 重ねる形の線の端を、前からある頂点から `FLAT_TOL` 以内なら、その頂点（いちばん近いもの）に寄せる（`overlay` の形の
    /// 線の端を寄せる所と同じ。頂点は塊から引き、升目の索引で端の近くの辺の端だけを見る）
    fn snap_ends(&self, quads: &mut [QuadBez]) {
        let Some(area) = quads
            .iter()
            .flat_map(|q| [q.p0, q.p2])
            .map(|p| KRect::from_points(p, p))
            .reduce(|a, b| a.union(b))
        else {
            return;
        };
        let ix = self.map_index();
        let mut edges: Vec<usize> = Vec::new();
        for t in end_keys_over(area.inflate(FLAT_TOL, FLAT_TOL)) {
            ix.ends.edges_in(&self.stored, t, &mut edges);
        }
        edges.sort_unstable();
        edges.dedup();
        let mut pts: Vec<Point> = edges
            .iter()
            .flat_map(|&e| {
                let te = self.stored.edge_at(e);
                [te.a, te.b]
            })
            .collect();
        pts.sort_by(|a, b| point_order(*a, *b));
        pts.dedup();
        let snap = |p: Point| -> Point {
            let mut best: Option<(f64, Point)> = None;
            for &q in &pts {
                let d = (q - p).hypot();
                if d <= FLAT_TOL && best.is_none_or(|b| d < b.0) {
                    best = Some((d, q));
                }
            }
            best.map_or(p, |b| b.1)
        };
        for q in quads.iter_mut() {
            q.p0 = snap(q.p0);
            q.p2 = snap(q.p2);
        }
    }

    /// テスト用（お手本との比べ）：`edit_local` の答え `got` が、同じ渡すものを前の地図全体で重ねて確かめ、書き戻すときの
    /// 答えと同じか
    /// （しまう形・消す図形・入れる図形。新しく振る番号は、入れる図形の形で対応づけて比べる）
    #[cfg(test)]
    fn edit_matches_full(
        &self,
        change: &LocalChange<'_>,
        group_path: &[u64],
        got: &LocalEdit,
    ) -> bool {
        // 同じ渡すものを、前の地図全体で重ねる（升目の索引を渡さない道。形の線の端の寄せも、その道が行う）
        let polys = self.polys().to_vec();
        let Some([next]) =
            self.map()
                .overlay_rules_known(&polys, None, change.input(), [change.rule])
        else {
            return matches!(got, LocalEdit::Refused(LocalRefusal::OffGrid));
        };
        self.matches_next(&next, group_path, got)
    }

    /// テスト用：前の地図に差分（`apply_diff` と同じ受け取り方）を当てた、計算に使うふつうの地図（お手本と比べるため）
    #[cfg(test)]
    fn materialized(&self, result: &PlanarMap, removed: &[usize], added: &[usize]) -> PlanarMap {
        let base = self.map();
        let gone: HashSet<usize> = removed.iter().copied().collect();
        let mut out = PlanarMap {
            nodes: base.nodes.clone(),
            edges: (0..base.edges.len())
                .filter(|e| !gone.contains(e))
                .map(|e| base.edges[e])
                .collect(),
        };
        let mut at: HashMap<[u64; 2], usize> = base
            .nodes
            .iter()
            .enumerate()
            .map(|(v, p)| ([p.x.to_bits(), p.y.to_bits()], v))
            .collect();
        let mut node = |p: Point, out: &mut PlanarMap| -> usize {
            *at.entry([p.x.to_bits(), p.y.to_bits()]).or_insert_with(|| {
                out.nodes.push(p);
                out.nodes.len() - 1
            })
        };
        for &r in added {
            let ed = result.edges[r];
            let (a, b) = (
                node(result.nodes[ed.a], &mut out),
                node(result.nodes[ed.b], &mut out),
            );
            out.edges.push(MapEdge { a, b, ..ed });
        }
        out.compact_nodes();
        out
    }

    /// テスト用（お手本との比べ）：新しい地図 `next` を前の道で確かめ、書き戻すとき（`written`・`diff`・
    /// `into_stored_with_polys`）の答えと、触った所だけの答え `got` が同じか（しまう形・消す図形・入れる図形。新しく振る
    /// 番号は、入れる図形の形で対応づけて比べる）
    #[cfg(test)]
    fn matches_next(&self, next: &PlanarMap, group_path: &[u64], got: &LocalEdit) -> bool {
        let next = next.clone();
        if !self.written_ok(&next) {
            return matches!(got, LocalEdit::Refused(LocalRefusal::Unwritable));
        }
        let checked = next.written(self).expect("正しい地図");
        let own_faces = self.faces(group_path);
        let own: Vec<&FillShape> = own_faces.iter().map(|s| &**s).collect();
        let change = self.diff(checked, &own, 0, group_path);
        if change.removed.is_empty() && change.added.is_empty() {
            return matches!(got, LocalEdit::Same);
        }
        let LocalEdit::Changed(w) = got else {
            return false;
        };
        // 消した図形は前の表にだけ、入れた図形は新しい表にだけある（変わらない面は同じ図形を共有する）
        let in_list =
            |list: &[Arc<FillShape>], s: &Arc<FillShape>| list.iter().any(|x| Arc::ptr_eq(x, s));
        let got_removed: HashSet<ShapeId> = own_faces
            .iter()
            .filter(|s| !in_list(&w.faces, s))
            .map(|s| s.shape_id)
            .collect();
        let got_added: Vec<Arc<FillShape>> = w
            .faces
            .iter()
            .filter(|s| !in_list(&own_faces, s))
            .cloned()
            .collect();
        let full_removed: HashSet<ShapeId> = change.removed.iter().copied().collect();
        if full_removed != got_removed || w.removed != change.removed {
            return false;
        }
        let added_ids: HashSet<ShapeId> = got_added.iter().map(|s| s.shape_id).collect();
        if w.added.iter().copied().collect::<HashSet<ShapeId>>() != added_ids {
            return false;
        }
        let orig: HashSet<ShapeId> = own.iter().map(|s| s.shape_id).collect();
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
        // 入れる図形を形で対応づける（元の図形の番号を引き継いだかも同じこと）
        let mut rename: HashMap<ShapeId, ShapeId> = HashMap::new();
        let full_by_key: HashMap<_, ShapeId> = change
            .added
            .iter()
            .map(|s| (shape_key(s), s.shape_id))
            .collect();
        if full_by_key.len() != got_added.len() {
            return false;
        }
        for s in &got_added {
            let Some(&fid) = full_by_key.get(&shape_key(s)) else {
                return false;
            };
            if orig.contains(&fid) || orig.contains(&s.shape_id) {
                if fid != s.shape_id {
                    return false;
                }
            } else {
                rename.insert(s.shape_id, fid);
            }
        }
        let Some((map, _)) = change.into_face_shapes(&own_faces) else {
            return false;
        };
        let (full_stored, _) = map.into_stored_with_polys(Some(&self.stored));
        w.stored
            .relabeled(|id| rename.get(&id).copied().unwrap_or(id))
            .same_content(&full_stored)
    }

    /// テスト用：この地図に差分を当てた地図（`LocalMap::new`）
    #[cfg(test)]
    pub(crate) fn local_map<'a>(
        &'a self,
        result: &PlanarMap,
        removed: &[usize],
        added: &[usize],
    ) -> LocalMap<'a> {
        LocalMap::new(self.map_index(), result, removed, added)
    }

    /// テスト用：新しい地図 `next` の、辺の決まり（格子に置ける・両側の色が違う・判定の形が2点以上・区間どうしが触れない・
    /// 別々の頂点が別々の格子の点）を、この地図にある辺どうしを除いて全部で調べた答え（`checked_since` の前半と同じ）
    #[cfg(test)]
    pub(crate) fn edges_ok_full(&self, next: &PlanarMap) -> bool {
        let (polys, origin) = next.polys_since(Some(self));
        let fresh: Vec<bool> = origin
            .iter()
            .map(|o| matches!(o, EdgeOrigin::New))
            .collect();
        next.fits_grid(Affine::IDENTITY)
            && next.edges.iter().all(|e| e.left != e.right)
            && next.polys_apart(&polys, &fresh)
    }

    /// テスト用：地図 `next` の全部の輪郭が閉じて、輪郭に沿って左側の色がそろうか（`face_cycles_with` と
    /// `is_broken_cycle`。`checked_since` の輪郭の決まり）
    #[cfg(test)]
    pub(crate) fn cycles_ok_full(next: &PlanarMap) -> bool {
        let polys = next.polys();
        next.face_cycles_with(&polys)
            .iter()
            .all(|c| !next.is_broken_cycle(c))
    }

    /// テスト用：地図 `next` が、この地図を前の地図として確かめて正しいか（`written` と同じ判定。正しくなくても止めない）
    #[cfg(test)]
    pub(crate) fn written_ok(&self, next: &PlanarMap) -> bool {
        next.checked_since(Some(self)).is_some()
    }

    /// テスト用：前の地図の面の表（`BaseFaces`）を、この地図の全部の面から作る（本番では 2b-7 で、写しが持つ面の表から
    /// 引く）
    #[cfg(test)]
    pub(crate) fn faces_table(&self) -> HashMap<ShapeId, Vec<(usize, bool)>> {
        let (faces, _) = self.map().faces();
        faces
            .iter()
            .filter_map(|f| match f.srcs[..] {
                [id] => Some((id, f.cycles.iter().map(|c| c[0]).collect())),
                _ => None,
            })
            .collect()
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
        }
    }

    /// `PlanarMap::hit_node` を、升目の索引で（見る頂点を、点のまわりの升目にかかる辺の端だけにする。答えは同じ）
    pub(crate) fn hit_node(&self, p: Point, r: f64, xform: Affine) -> Option<Point> {
        self.nearest_node(p, r, xform, None)
    }

    /// `PlanarMap::snap_target` を、升目の索引で（動かす頂点は位置 `skip` で渡す）
    pub(crate) fn snap_target(
        &self,
        skip: Point,
        p: Point,
        radius: f64,
        xform: Affine,
    ) -> Option<Point> {
        self.nearest_node(p, radius, xform, Some(skip))
    }

    /// 地図を座標の変換 `xform` で置いたとき、点 `p`（置いた座標）から `radius` 以内でいちばん近い頂点の位置（文書の
    /// 座標。位置が `skip` の頂点は除く）。頂点は、点のまわりの端の升目の索引にある辺の端から引く（地図全体の写しを
    /// 作らない：設計 段2b の 7. の 2b-7d）。同じ距離の頂点は、座標の順で先のもの
    fn nearest_node(
        &self,
        p: Point,
        radius: f64,
        xform: Affine,
        skip: Option<Point>,
    ) -> Option<Point> {
        // 潰れた変換（逆にできない）で置いた地図は描かない（面積 0 の絵は見えない）ので、何にも合わない
        if xform.determinant().abs() <= 0.0 {
            return None;
        }
        let area = KRect::from_points(p, p).inflate(radius, radius);
        let doc_area = xform.inverse().transform_rect_bbox(area);
        let ix = self.map_index();
        let mut edges: Vec<usize> = Vec::new();
        for t in end_keys_over(doc_area) {
            ix.ends.edges_in(ix.stored, t, &mut edges);
        }
        let mut pts: Vec<Point> = edges
            .iter()
            .flat_map(|&e| {
                let te = self.stored.edge_at(e);
                [te.a, te.b]
            })
            .collect();
        pts.sort_by(|a, b| point_order(*a, *b));
        pts.dedup();
        let mut best: Option<(f64, Point)> = None;
        for v in pts {
            if Some(v) == skip {
                continue;
            }
            let d = (xform * v - p).hypot();
            if d <= radius && best.is_none_or(|b| d < b.0) {
                best = Some((d, v));
            }
        }
        best.map(|b| b.1)
    }

    /// 頂点 `p`（文書の座標）を端に持つ辺（計算に使う地図での番号の順）。端の升目の索引で引く
    pub(crate) fn edges_at_node(&self, p: Point) -> Vec<usize> {
        let ix = self.map_index();
        let mut at: Vec<usize> = Vec::new();
        ix.ends.edges_in(&self.stored, end_key(p), &mut at);
        at.sort_unstable();
        at.dedup();
        at.retain(|&e| {
            let te = self.stored.edge_at(e);
            te.a == p || te.b == p
        });
        at
    }

    /// 面（階層 `group_path`）のうち、番号が `ids` にあるものがあるか（面の表で引く）
    pub(crate) fn has_any_face(&self, ids: &HashSet<ShapeId>, group_path: &[u64]) -> bool {
        let faces = self.faces(group_path);
        ids.iter()
            .any(|id| faces.binary_search_by_key(id, |s| s.shape_id).is_ok())
    }

    /// 番号が `ids` の面の輪郭の辺（計算に使う地図での番号の順）：面の表の図形の輪郭の曲線を、塊から引く（手間は、その
    /// 面の大きさの分）
    fn faces_edges(&self, ids: &HashSet<ShapeId>, group_path: &[u64]) -> Vec<usize> {
        let faces = self.faces(group_path);
        let mut out: Vec<usize> = Vec::new();
        for id in ids {
            let Ok(i) = faces.binary_search_by_key(id, |s| s.shape_id) else {
                continue;
            };
            for q in faces[i].contours().flat_map(|c| c.segments.iter()) {
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
        for e in self.faces_edges(ids, group_path) {
            let te = self.stored.edge_at(e);
            let left_src = te.left_src.filter(|id| ids.contains(id));
            let right_src = te.right_src.filter(|id| ids.contains(id));
            if left_src.is_none() && right_src.is_none() {
                continue;
            }
            let (a, b) = (node(te.a, &mut m), node(te.b, &mut m));
            m.edges.push(MapEdge {
                a,
                b,
                ctrl: te.ctrl,
                left: te.left.filter(|_| left_src.is_some()),
                right: te.right.filter(|_| right_src.is_some()),
                left_src,
                right_src,
            });
        }
        m
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
                let touched = self.faces_edges(ids, group_path);
                let added: Vec<crate::stored_map::TileEdge> = touched
                    .iter()
                    .filter_map(|&e| {
                        let te = self.stored.edge_at(e);
                        let left_src = te.left_src.filter(|&id| keep.keeps(id));
                        let right_src = te.right_src.filter(|&id| keep.keeps(id));
                        (left_src.is_some() || right_src.is_some()).then_some(
                            crate::stored_map::TileEdge {
                                left: te.left.filter(|_| left_src.is_some()),
                                right: te.right.filter(|_| right_src.is_some()),
                                left_src,
                                right_src,
                                ..te
                            },
                        )
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
    pub(crate) fn anchor_possible(&self, edit: MapEdit) -> bool {
        self.anchor_diff(edit).is_some()
    }

    /// `PlanarMap::hit_edge` を、升目の索引で（見る辺を、点のまわりの升目にかかる辺だけにする。`p` から `r` 以内を通る
    /// 辺は、その所を含むマスの升目にあるので、答えは同じ）
    pub(crate) fn hit_edge(&self, p: Point, r: f64, xform: Affine) -> Option<(usize, f64)> {
        // 潰れた変換で置いた地図は描かないので、何にも合わない（`nearest_node` と同じ）
        if xform.determinant().abs() <= 0.0 {
            return None;
        }
        let near = KRect::from_center_size(p, (2.0 * r, 2.0 * r));
        let ix = self.map_index();
        let mut cands: Vec<usize> = Vec::new();
        for t in index_tiles_over(xform.inverse().transform_rect_bbox(near)) {
            ix.index.edges_in(ix.stored, t, &mut cands);
        }
        cands.sort_unstable();
        cands.dedup();
        // `PlanarMap::hit_edge_among` と同じ（辺は塊から引く）
        cands
            .into_iter()
            .map(|e| (e, xform * self.stored.edge_at(e).quad()))
            .filter(|(_, q)| rects_overlap(q.bounding_box(), near))
            .map(|(e, q)| {
                let (t, d) = nearest_on_quad(&q, p);
                (e, t, d)
            })
            .filter(|x| x.2 <= r)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(e, t, _)| (e, t))
    }

    /// 辺の鍵（`edge_key`）の集まり（初めて要るときに作る）。書き戻す地図の確かめ（`PlanarMap::written`）と、書き戻しの
    /// 差分（`diff`）が使う
    pub(crate) fn keys(&self) -> &EdgeKeys {
        self.keys.get_or_init(|| {
            (0..self.map().edges.len())
                .map(|e| (edge_key(self.map(), e), e))
                .collect()
        })
    }

    /// 辺ごとの判定の形（`map` の辺の順。初めて要るときに作る。編集で作った写しは、確かめたときのものを引き継いでいる）
    fn polys(&self) -> &[Poly] {
        self.polys.get_or_init(|| {
            (0..self.map().edges.len())
                .map(|e| Arc::clone(self.stored.poly(e)))
                .collect()
        })
    }

    /// 面の輪郭（`face_cycles_with`。初めて要るときに作る。編集で作った写しは、確かめたときのものを引き継いでいる）
    pub(crate) fn cycles(&self) -> &[Vec<(usize, bool)>] {
        self.cycles
            .get_or_init(|| self.map().face_cycles_with(self.polys()))
    }

    /// 輪郭ごとの組み立ての結果（`regions_since`。初めて要るときに作る。編集で作った写しは、確かめたときのものを引き継いで
    /// いる）
    fn cycle_regions(&self) -> &[CycleRegion] {
        self.cycle_regions.get_or_init(|| {
            self.map()
                .regions_since(self.polys(), self.cycles(), false, None)
                .1
        })
    }

    /// テスト用：持っている判定の形と面の輪郭（引き継いだもの）が、地図から作り直したものと同じか。まだ持っていなければ
    /// `None`
    #[cfg(test)]
    pub(crate) fn derived_match_full(&self) -> Option<bool> {
        let (polys, cycles, regions) = (
            self.polys.get()?,
            self.cycles.get()?,
            self.cycle_regions.get()?,
        );
        let fresh = self.map().polys();
        let fresh_cycles = self.map().face_cycles_with(&fresh);
        let fresh_regions = self
            .map()
            .regions_since(&fresh, &fresh_cycles, false, None)
            .1;
        // 輪郭の点の始まりは、輪郭の始まりの区間で決まる。前の地図から引き継いだ輪郭は、番号の付け直しで始まりの区間が
        // 変わることがあるので、点を回した並びとして比べる（`Ring` の使い道は、どこから始めても同じ：`Ring` の説明）
        let same_points = |x: &[IPt], y: &[IPt]| {
            x.len() == y.len()
                && (x.is_empty() || (0..x.len()).any(|k| x[k..].iter().chain(&x[..k]).eq(y.iter())))
        };
        let same_region = |a: &CycleRegion, b: &CycleRegion| {
            a.owner == b.owner
                && match (&a.ring, &b.ring) {
                    (Some(x), Some(y)) => same_points(&x.pts, &y.pts) && x.area2 == y.area2,
                    (None, None) => true,
                    _ => false,
                }
        };
        Some(
            polys.len() == fresh.len()
                && polys.iter().zip(&fresh).all(|(a, b)| a[..] == b[..])
                && *cycles == fresh_cycles
                && regions.len() == fresh_regions.len()
                && regions
                    .iter()
                    .zip(&fresh_regions)
                    .all(|(a, b)| same_region(a, b)),
        )
    }

    #[cfg(test)]
    /// バケツ（`PlanarMap::with_face_filled`）を、この写しの判定の形で（地図全体から面を探す前の道。本番のバケツは 1本の道
    /// `fill_local` で、これはテストのお手本）
    pub(crate) fn with_face_filled(&self, p: Point, color: Color32) -> Option<PlanarMap> {
        self.map()
            .face_filled_with(self.polys(), self.cycles(), p, color)
    }

    #[cfg(test)]
    /// 重ね合わせ（`PlanarMap::overlay`）を、この写しの辺の計算に使う形で（近くの辺を探す外接矩形とマスを作り直さない）
    pub(crate) fn overlay(&self, input: MapInput<'_>, rule: MapRule) -> Option<PlanarMap> {
        self.map()
            .overlay_known(self.polys(), Some(self.map_index()), input, rule)
    }

    #[cfg(test)]
    /// 同じ重ねるもの `input` を決まりごとに重ねた地図（`PlanarMap::overlay_rules`）を、この写しの辺の計算に使う形と
    /// 升目の索引で
    pub(crate) fn overlay_rules<const N: usize>(
        &self,
        input: MapInput<'_>,
        rules: [MapRule; N],
    ) -> Option<[PlanarMap; N]> {
        self.map()
            .overlay_rules_known(self.polys(), Some(self.map_index()), input, rules)
    }

    #[cfg(test)]
    /// この地図に、地図 `maps` を下から順に重ねた地図（`stacked` と同じ。最初の1枚は、この写しの辺の計算に使う形と
    /// 升目の索引で重ねる）
    pub(crate) fn stacked(
        &self,
        maps: impl IntoIterator<Item = Option<PlanarMap>>,
    ) -> Option<PlanarMap> {
        let mut maps = maps.into_iter();
        let Some(first) = maps.next() else {
            return Some(self.map().clone());
        };
        let first = first?;
        let start = if self.map().edges.is_empty() {
            first
        } else {
            self.overlay(MapInput::Map(&first), MapRule::Overlay)?
        };
        stacked(start, maps)
    }

    #[cfg(test)]
    /// 地図の編集（`MapEdit::apply`）を、この写しの辺の計算に使う形と升目の索引で
    pub(crate) fn apply_edit(&self, edit: MapEdit) -> Option<PlanarMap> {
        edit.apply_known(self.map(), self.polys(), Some(self.map_index()))
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
        let (faces, _) = self.map().faces_of(self.polys(), self.cycles());
        let ids = face_ids(self.map(), &faces, FaceIds::Keep);
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

    /// この地図（文書の座標）の面の図形 `own` に、編集して確かめた地図 `next`（`PlanarMap::written` か `realized`）を
    /// 書き戻す差分（`MapChange`。新しい形も文書の座標。変わらない面は前の図形のまま：`rewrite_shapes`）
    pub(crate) fn diff(
        &self,
        next: CheckedMap,
        own: &[&FillShape],
        layer_id: LayerId,
        group_path: &[u64],
    ) -> MapChange {
        // 確かめたときの辺の対応を使う（同じしまった地図から作った写しなら、辺の番号も同じ：`StoredMap::to_dense`）
        let origin = match &next.since {
            Some((stored, origin)) if stored.same(&self.stored) => Cow::Borrowed(&origin[..]),
            _ => Cow::Owned(next.map.edge_origins(self)),
        };
        let (removed, added, ids) = rewrite_shapes(
            self.map(),
            &origin,
            &next.map,
            &next.faces,
            own,
            layer_id,
            group_path,
        );
        MapChange {
            removed,
            added,
            map: next.with_ids(&ids),
            ids,
        }
    }

    /// テスト用：`diff` を、座標の変換 `xform` で置いた図形 `own`（文書の無い、ただの図形の並び。この地図は `LayerBase::of`
    /// で作ったもの）で受け取り、新しい形を `xform` で置いた写しで返すもの。図形がそのまま正しい地図にならず、塗り重ねて
    /// 作った地図なら、地図の面と図形は1対1でないので、図形を全部書き換える
    #[cfg(test)]
    pub(crate) fn diff_placed(
        &self,
        next: CheckedMap,
        own: &[&FillShape],
        layer_id: LayerId,
        group_path: &[u64],
        xform: Affine,
    ) -> Option<MapChange> {
        let mut change = with_doc_shapes(own, xform, |own| {
            if PlanarMap::from_faces(own.iter().copied()).is_valid() {
                return Some(self.diff(next, own, layer_id, group_path));
            }
            let added: Vec<FillShape> = next
                .faces
                .iter()
                .map(|f| next.map.face_shape(f, layer_id, group_path))
                .collect();
            let ids: Vec<ShapeId> = added.iter().map(|s| s.shape_id).collect();
            Some(MapChange {
                removed: own.iter().map(|s| s.shape_id).collect(),
                map: next.with_ids(&ids),
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
    /// テスト用：このスレッドで、地図を全部確かめた回数（`valid_faces_since` の前の地図が無いとき）
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
    /// テスト用：このスレッドで、面の輪郭をたどるために、頂点のまわりの辺を並べた数（`face_cycles_with`・
    /// `face_cycles_since`）
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
        self.apply_known(map, &[], None)
    }

    /// `apply` を、地図 `map` の辺の計算に使う形 `known` と升目の索引 `index` で（計算に使う写しが持つもの：
    /// `LayerBase::apply_edit`）。索引があれば、頂点と辺は、その位置の升目の辺から探す（正しい地図では、同じ位置の頂点・
    /// 同じ形の辺は1つなので、答えは全部を探したときと同じ）
    fn apply_known(
        self,
        map: &PlanarMap,
        known: &[Poly],
        index: Option<MapIndex<'_>>,
    ) -> Option<PlanarMap> {
        let vertex = |p: Point| match map.edges_around(p, index) {
            Some(near) => near
                .iter()
                .flat_map(|&e| [map.edges[e].a, map.edges[e].b])
                .filter(|&v| map.nodes[v] == p)
                .min(),
            None => (0..map.nodes.len()).find(|&v| map.nodes[v] == p),
        };
        let find_edge = |q: &QuadBez| match map.edges_around(q.p0, index) {
            Some(near) => map.find_edge_in(q, near.into_iter()),
            None => map.find_edge(q),
        };
        match self {
            MapEdit::AddAnchor { orig_seg, t } => {
                let (e, forward) = find_edge(&orig_seg)?;
                map.with_anchor_added(e, if forward { t } else { 1.0 - t })
            }
            MapEdit::DeleteAnchor { orig_vertex } => map.with_anchor_deleted(vertex(orig_vertex)?),
            MapEdit::PullEdge {
                orig_seg,
                grab_t,
                target,
            } => {
                let (e, forward) = find_edge(&orig_seg)?;
                let t = if forward { grab_t } else { 1.0 - grab_t };
                map.pulled_edge(known, index, e, t, target)
            }
            MapEdit::MoveVertex {
                orig_vertex,
                target,
            } => map.moved_vertex(known, index, vertex(orig_vertex)?, target),
        }
    }
}

impl PlanarMap {
    /// 形が `q` の辺（向きが逆でもよい）と、`q` と同じ向きか
    fn find_edge(&self, q: &QuadBez) -> Option<(usize, bool)> {
        self.find_edge_in(q, 0..self.edges.len())
    }

    /// `find_edge` を、辺 `among`（番号の順）の中で
    fn find_edge_in(
        &self,
        q: &QuadBez,
        mut among: impl Iterator<Item = usize>,
    ) -> Option<(usize, bool)> {
        among.find_map(|e| {
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
    #[cfg(test)]
    pub(crate) fn snap_target(
        &self,
        v: usize,
        p: Point,
        radius: f64,
        xform: Affine,
    ) -> Option<Point> {
        let near = self.node_finder(radius, xform, KRect::from_points(p, p));
        near(p, Some(v)).map(|w| self.nodes[w])
    }

    /// 近い頂点を引く関数：点（置いた座標）から `radius`（正）以内でいちばん近い頂点の番号（2つ目の引数の頂点は除く）。
    /// 頂点は座標の変換 `xform` で置いて測り、見るのは `area`（置いた座標）から `radius` 以内のものだけ。近い頂点を引くのは
    /// みなこれ（カーソル合わせ `hit_node`・頂点への吸い付き `snap_target`・重ねる形の線の端を前からある頂点に寄せる
    /// `overlay`）
    #[cfg(test)]
    pub(crate) fn node_finder(
        &self,
        radius: f64,
        xform: Affine,
        area: KRect,
    ) -> impl Fn(Point, Option<usize>) -> Option<usize> + '_ {
        self.node_finder_in(radius, xform, area, None)
    }

    /// `node_finder` と同じ。`index`（この地図の升目の索引）があれば、見る頂点を、`area` の
    /// まわりの升目にかかる辺の端だけにする（頂点の番号の順に見るので、答えは全部の頂点を見たときと同じ）
    pub(crate) fn node_finder_in(
        &self,
        radius: f64,
        xform: Affine,
        area: KRect,
        index: Option<MapIndex<'_>>,
    ) -> impl Fn(Point, Option<usize>) -> Option<usize> + '_ {
        let cell = move |p: Point| ((p.x / radius).floor() as i64, (p.y / radius).floor() as i64);
        let area = area.inflate(radius, radius);
        let mut near: IntMap<(i64, i64), Vec<(usize, Point)>> = IntMap::default();
        let mut put = |v: usize| {
            let q = xform * self.nodes[v];
            if area.contains(q) {
                near.entry(cell(q)).or_default().push((v, q));
            }
        };
        match index.filter(|_| xform.determinant().abs() > 0.0) {
            Some(ix) => {
                // `area` の中の頂点は、文書の座標では、逆の変換で写した `area` の外接矩形の中にあり、その点を含むマスの
                // 升目に、その頂点を端に持つ辺が入っている
                let doc_area = xform.inverse().transform_rect_bbox(area);
                let mut edges: Vec<usize> = Vec::new();
                for t in end_keys_over(doc_area) {
                    ix.ends.edges_in(ix.stored, t, &mut edges);
                }
                let mut vs: Vec<usize> = edges
                    .iter()
                    .flat_map(|&e| [self.edges[e].a, self.edges[e].b])
                    .collect();
                vs.sort_unstable();
                vs.dedup();
                for v in vs {
                    put(v);
                }
            }
            None => {
                for v in 0..self.nodes.len() {
                    put(v);
                }
            }
        }
        move |p: Point, skip: Option<usize>| {
            let (cx, cy) = cell(p);
            // 格子に置けない座標（重ね合わせがあとで断る）でも溢れないように
            let around = |c: i64| c.saturating_sub(1)..=c.saturating_add(1);
            let mut best: Option<(f64, usize)> = None;
            for k in around(cx).flat_map(|x| around(cy).map(move |y| (x, y))) {
                for &(v, q) in near.get(&k).into_iter().flatten() {
                    let d = (q - p).hypot();
                    if Some(v) != skip && d <= radius && best.is_none_or(|b| d < b.0) {
                        best = Some((d, v));
                    }
                }
            }
            best.map(|b| b.1)
        }
    }
}

/// 輪郭の変形のドラッグ中の表示：地図 `base` に編集 `edit`（文書の座標。辺の引っぱり・頂点の移動）をかけて確定した結果
/// を、図形の差分（消す図形の番号, 入れる図形）で返す。離したときの確定と同じ1本の道（`reshape_change` → `edit_local`）。
/// 入れる図形は、レイヤー `layer_id` の、座標の変換 `xform` で置いた写し。辺・頂点が無いか、確定できなければ `None`
pub(crate) fn reshape_result(
    base: &LayerBase,
    edit: MapEdit,
    layer_id: LayerId,
    group_path: &[u64],
    xform: Affine,
) -> Option<(Vec<ShapeId>, Vec<FillShape>)> {
    let change = base.reshape_change(edit)??;
    match base.edit_local(&change, group_path) {
        LocalEdit::Same => Some((Vec::new(), Vec::new())),
        LocalEdit::Refused(_) => None,
        LocalEdit::Changed(w) => {
            let added = w
                .added
                .iter()
                .filter_map(|id| {
                    let i = w.faces.binary_search_by_key(id, |s| s.shape_id).ok()?;
                    let mut s = (*w.faces[i]).clone();
                    s.layer_id = layer_id;
                    Some(placed(s, xform))
                })
                .collect();
            Some((w.removed, added))
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
            .then(|| self.with_sides_colored(sides, color))
    }

    /// 辺の側 `sides`（辺の番号と、左の側なら `true`）の色を `color` にした地図。面の色を変えるのはみなこれ（選んだ面の
    /// 塗り替え `recolored`・バケツ `with_face_filled`）。両側が同じ色になった辺は外し、外した辺の端を、2本だけが
    /// つながる頂点をまとめる候補にする（`merge_degree2`）
    fn with_sides_colored(
        &self,
        sides: impl IntoIterator<Item = (usize, bool)>,
        color: Color32,
    ) -> PlanarMap {
        let mut m = self.clone();
        for (i, left) in sides {
            if left {
                m.edges[i].left = Some(color);
            } else {
                m.edges[i].right = Some(color);
            }
        }
        let dropped_ends: Vec<usize> = m
            .edges
            .iter()
            .filter(|e| e.left == e.right)
            .flat_map(|e| [e.a, e.b])
            .collect();
        m.edges.retain(|e| e.left != e.right);
        m.merge_degree2(dropped_ends);
        m.compact_nodes();
        m
    }
}

/// 曲線 `q` の `t` の所を `target` へ引っぱった曲線（両端は動かさず、制御点を動かす。輪郭のつまみの決まり。確定
/// `commit_pull_edge` とドラッグ中の表示が同じこれを使う）。`t` は `grab_t` で寄せたもの
pub(crate) fn pulled_quad(q: QuadBez, t: f64, target: Point) -> QuadBez {
    QuadBez::new(
        q.p0,
        q.p1 + (target - q.eval(t)) / (2.0 * t * (1.0 - t)),
        q.p2,
    )
}

/// 輪郭のつまみで、つかんだ所の t（曲線の上）を、引っぱる所に寄せたもの（0.18〜0.82）。`pulled_quad` は制御点を
/// 1 / (2t(1−t)) 倍動かすので、端に近い t ほど少しの動きで辺全体が大きく膨らむ。端の近くをつかむと、端から 18% の所が
/// カーソルの動きどおりに動く（曲線はカーソルから離れた所を通る）。曲線がいつもカーソルの下を通る決まりは、長い辺の端の
/// 近くを少し引くと辺全体が大きく膨らむので採らなかった（GEOMETRY.md 5.1）
pub(crate) fn grab_t(t: f64) -> f64 {
    t.clamp(0.18, 0.82)
}

/// 地図 `map`（文書の座標）に変換 `xform` をかけ、その座標の格子で正しい地図にしたもの。変換した地図が正しくなければ
/// （縮小で頂点が近づくなど）、変換した面を塗り重ねて作る（面は重ならないので、絵は同じ。面の番号は残る）。
/// 「変形した形を、その座標の格子で地図にする」決まり。結果は確かめた地図（`checked`）。`None`：格子に置けない座標が
/// あるか、正しい地図にならない
pub(crate) fn realized(map: &PlanarMap, xform: Affine) -> Option<CheckedMap> {
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
/// 同じ数なら形の鍵（`face_key`）の小さい面。面の番号の同点の決め方はこれ1つ（`face_ids`・`rewrite_shapes`）。
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

/// 正しい地図（`valid_faces` で確かめたもの）と、その面。作れるのは `PlanarMap::checked` と、正しい地図を正しいまま
/// 変える、このあとのメソッドだけなので、これを受け取る所（セル画にしまう地図：`ScopeMap::new`）は確かめ直さない
#[derive(Clone, Debug)]
pub(crate) struct CheckedMap {
    map: PlanarMap,
    faces: Vec<MapFace>,
    /// 辺ごとの判定の形（`map` の辺の順。確かめるときに作ったもの）
    polys: Vec<Poly>,
    /// 面の輪郭（`face_cycles_with` と同じ並び。確かめるときに作ったもの）
    cycles: Vec<Cycle>,
    /// 輪郭ごとの組み立ての結果（`cycles` の順）
    cycle_regions: Vec<CycleRegion>,
    /// 書き戻す先の地図（しまった地図）と、辺ごとの、その地図の同じ辺（`PlanarMap::written` で確かめたときのもの。
    /// 書き戻しの差分 `LayerBase::diff` が、同じ地図に書き戻すときに使う）
    since: Option<(StoredMap, Vec<EdgeOrigin>)>,
}

impl PlanarMap {
    /// 正しい地図なら（`valid_faces`。受け入れの判定はこれ1つ）、確かめた地図にする。`None`：正しい地図でない
    pub(crate) fn checked(self) -> Option<CheckedMap> {
        let Checked {
            faces,
            polys,
            cycles,
            cycle_regions,
            ..
        } = self.checked_since(None)?;
        Some(CheckedMap {
            map: self,
            faces,
            polys,
            cycles,
            cycle_regions,
            since: None,
        })
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

    /// `into_stored` と、しまう形から作る地図（`StoredMap::to_dense`）の辺の順に並べた、辺ごとの判定の形と面の輪郭
    /// （確かめたときに作ったものを引き継ぐ。判定の形は辺の向きの順に作るので、しまう形で向きを逆にした辺だけ作り直す。
    /// 輪郭は辺の番号と向きを付け替え、`face_cycles_with` と同じ並びにする）
    pub(crate) fn into_stored_with_polys(
        self,
        prev: Option<&StoredMap>,
    ) -> (StoredMap, StoredGeom) {
        let checked = self.uniquely_labeled();
        // 確かめたときの辺の対応が、しまう前の地図 `prev` に対してのものなら、変わった所の塊だけを作り直す
        let origin = checked
            .since
            .as_ref()
            .filter(|(s, _)| prev.is_some_and(|p| s.same(p)))
            .map(|(_, o)| &o[..]);
        let (stored, order) = StoredMap::from_dense_ordered(&checked.map, prev, origin);
        let map = StoredMap::dense_in_order(&checked.map, &order);
        #[cfg(test)]
        if test_checks() {
            let (full, full_order) = StoredMap::from_dense_ordered(&checked.map, prev, None);
            // 比べるために並べた分は数えない（`stored_map::edges_sorted` は、しまうときに並べた辺の数）
            crate::stored_map::EDGES_SORTED.with(|n| n.set(n.get() - checked.map.edges.len()));
            assert!(stored.same_content(&full) && order == full_order);
            if let Some(prev) = prev {
                assert_eq!(stored.shared_tiles(prev), full.shared_tiles(prev));
            }
            let dense = stored.to_dense();
            assert!(
                map.edges == dense.edges
                    && map.nodes.len() == dense.nodes.len()
                    && map
                        .nodes
                        .iter()
                        .zip(&dense.nodes)
                        .all(|(a, b)| a.x.to_bits() == b.x.to_bits()
                            && a.y.to_bits() == b.y.to_bits())
            );
        }
        let polys: Vec<Poly> = order
            .iter()
            .map(|&(e, flipped)| {
                if flipped {
                    let q = checked.map.quad(e);
                    EdgeGeom::of(&QuadBez::new(q.p2, q.p1, q.p0))
                } else {
                    Arc::clone(&checked.polys[e])
                }
            })
            .collect();
        stored.seed_polys(&polys);
        // この地図の辺ごとの、しまう形の辺の番号と、向きを逆にしたか
        let mut at = vec![(0, false); order.len()];
        for (s, &(e, flipped)) in order.iter().enumerate() {
            at[e] = (s, flipped);
        }
        let mut cycles: Vec<Cycle> = checked
            .cycles
            .into_iter()
            .map(|c| {
                c.into_iter()
                    .map(|(e, forward)| {
                        let (s, flipped) = at[e];
                        (s, forward != flipped)
                    })
                    .collect()
            })
            .collect();
        let order = sort_cycles(&mut cycles);
        // 輪郭ごとの組み立ての結果も、並べた輪郭の順にし、穴の入れ先の輪郭の番号を付け替える
        let mut now_of = vec![0; order.len()];
        for (k, &was) in order.iter().enumerate() {
            now_of[was] = k;
        }
        let regions = order
            .iter()
            .map(|&was| {
                let r = &checked.cycle_regions[was];
                CycleRegion {
                    ring: r.ring.clone(),
                    owner: r.owner.map(|o| now_of[o]),
                }
            })
            .collect();
        (
            stored,
            StoredGeom {
                map,
                polys,
                cycles,
                regions,
            },
        )
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
        CheckedMap {
            map,
            faces,
            polys: self.polys,
            cycles: self.cycles,
            cycle_regions: self.cycle_regions,
            since: self.since,
        }
    }

    /// 辺の側に記した図形の番号を、`f` が返す番号に付け替える（`PlanarMap::relabel`。面の番号も）
    #[cfg(test)]
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
    #[cfg(test)]
    let inside_orig = inside.clone();
    let piece = inside.checked()?;
    let ids = cel.piece_ids(&piece, group_path);
    let piece = piece.with_ids(&ids);
    // お手本：前の道（セル画の地図に対して確かめ、書き戻して番号を新しくする）と、同じ地図で、セル画の番号のまま残る面が
    // 同じ
    #[cfg(test)]
    if test_checks() {
        let ok = uncounted(|| {
            let Some(full) = inside_orig.written(cel) else {
                return false;
            };
            let own_faces = cel.faces(group_path);
            let own: Vec<&FillShape> = own_faces.iter().map(|s| &**s).collect();
            let mut change = cel.diff(full, &own, 0, group_path);
            change.renew_ids();
            let cel_ids: HashSet<ShapeId> = own.iter().map(|s| s.shape_id).collect();
            let kept = |ids: &[ShapeId]| -> std::collections::BTreeSet<ShapeId> {
                ids.iter()
                    .copied()
                    .filter(|id| cel_ids.contains(id))
                    .collect()
            };
            kept(&change.ids) == kept(&ids) && change.ids.len() == ids.len()
        });
        assert!(
            ok,
            "片だけで決めた番号が、セル画の地図に対して決めた番号（お手本）と違う"
        );
    }
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
                let shape = |s: ShapeId| {
                    faces_list
                        .binary_search_by_key(&s, |x| x.shape_id)
                        .ok()
                        .map(|i| &faces_list[i])
                };
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

/// テスト用（お手本）：`cut_out` を、地図全体で行う前の道（残りもセル画の地図に対して確かめ、書き戻す）
#[cfg(test)]
pub(crate) fn cut_out_full(
    cel: &LayerBase,
    own: &[&FillShape],
    content: &LayerBase,
    region: MapInput<'_>,
    layer_id: LayerId,
    group_path: &[u64],
) -> Option<(MapChange, MapChange)> {
    let [inside, rest] =
        content.overlay_rules(region, [MapRule::KeepInside, MapRule::Paint(None)])?;
    let inside = inside.written(cel)?;
    let rest = rest.written(cel)?;
    let rest_change = cel.diff(rest, own, layer_id, group_path);
    let mut inside_change = cel.diff(inside, own, layer_id, group_path);
    inside_change.renew_ids();
    Some((rest_change, inside_change))
}

impl MapChange {
    #[cfg(test)]
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
    Changed(Box<MapChange>),
    /// 形が変わらない
    Unchanged,
    /// 地図にできない（格子に置けない座標がある：±65,536px の外）か、結果に壊れた輪郭がある（`PlanarMap::written`）
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
    let Some(change) = edit(base.map())
        .and_then(|m| m.written(&base))
        .and_then(|next| base.diff_placed(next, &own, layer_id, group_path, xform))
    else {
        return MapEditOutcome::Refused;
    };
    if change.removed.is_empty() && change.added.is_empty() {
        MapEditOutcome::Unchanged
    } else {
        MapEditOutcome::Changed(Box::new(change))
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
