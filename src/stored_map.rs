//! セル画にしまう地図（境目の線を1本で持つ本当の形）：場所ごとの塊に分けた辺の並び。
//!
//! 辺は両端を座標で指し（頂点の番号を持たない）、決まった向き・決まった順に並べる。塊は、辺の弦の中点がある `TILE` px の
//! 升目ごとの辺の並び。地図をしまうときは、置き換える前の地図（読み込みでは同じレイヤーの直前のセル）の同じ升目の塊と
//! 中身が同じなら、その塊を使う（`StoredMap::from_dense`）。Undo の控えが持つのは置き換える前の地図そのものなので、編集で
//! 変わらなかった場所の塊は控えと共有したまま（控え1つ分の大きさは、変わった場所の塊の分だけ）。塊を持つのは地図だけ
//! なので、どの地図も持たなくなった塊は、その場で返る。
//!
//! 計算に使うふつうの地図（`PlanarMap`）は、いつも塊から決まった順で作る（`StoredMap::to_dense`）。頂点と辺の番号が中身
//! だけで決まるので、いつ作り直しても同じになる（編集のあとは、確かめた地図を同じ順に並べ直して作る：
//! `StoredMap::dense_in_order`）。計算に使う写し（`LayerBase`）は、それを使う所が持つ。しまう形は、いま
//! 誰かが持っている写しへの弱い参照を1つだけ持ち（`StoredMap::dense_slot`）、写しを生かすのは持ち主だけ（Undo の控えは
//! しまう形だけを持つ）。

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use eframe::egui::Color32;
use kurbo::Point;
use serde::{Deserialize, Serialize};

use crate::planar_map::{point_order, EdgeOrigin, IntMap, LayerBase, MapEdge, PlanarMap};
use crate::vector_engine::ShapeId;

/// 塊の升目の大きさ（文書の座標の px）。ブラシ1本・消しゴム1本で変わるのは、ふつう升目の数個分
const TILE: f64 = 32.0;

/// しまう辺：両端 `a`・`b` を座標で持つ。向きは決まった向き（`a` が `b` より座標の小さい順で前。逆なら左右も入れ替える）。
/// 側の図形の番号は 0 でない数で持つ（図形の番号は 1 から振る：`next_shape_id`）
#[derive(Clone, Copy, Debug)]
struct StoredEdge {
    a: Point,
    b: Point,
    ctrl: Point,
    left: Option<Color32>,
    right: Option<Color32>,
    left_src: Option<NonZeroU64>,
    right_src: Option<NonZeroU64>,
}

impl StoredEdge {
    /// 地図 `map` の辺 `e` の、決まった向きのしまう辺
    fn of(map: &PlanarMap, e: &MapEdge) -> Self {
        let (a, b) = (map.nodes[e.a], map.nodes[e.b]);
        let src = |id: Option<ShapeId>| id.and_then(NonZeroU64::new);
        if point_order(a, b).is_le() {
            Self {
                a,
                b,
                ctrl: e.ctrl,
                left: e.left,
                right: e.right,
                left_src: src(e.left_src),
                right_src: src(e.right_src),
            }
        } else {
            Self {
                a: b,
                b: a,
                ctrl: e.ctrl,
                left: e.right,
                right: e.left,
                left_src: src(e.right_src),
                right_src: src(e.left_src),
            }
        }
    }

    /// 計算に使う地図の辺（頂点の番号 `a`・`b`。向きはこのしまう辺の向き）
    fn dense(&self, a: usize, b: usize) -> MapEdge {
        MapEdge {
            a,
            b,
            ctrl: self.ctrl,
            left: self.left,
            right: self.right,
            left_src: self.left_src.map(NonZeroU64::get),
            right_src: self.right_src.map(NonZeroU64::get),
        }
    }

    /// 入る塊の升目（弦の中点）
    fn tile(&self) -> (i32, i32) {
        let m = self.a.midpoint(self.b);
        ((m.x / TILE).floor() as i32, (m.y / TILE).floor() as i32)
    }

    /// 塊の中の並びの順（形だけで決める。正しい地図に、形の同じ辺は2本無い）
    fn order(&self, other: &Self) -> std::cmp::Ordering {
        point_order(self.a, other.a)
            .then_with(|| point_order(self.b, other.b))
            .then_with(|| point_order(self.ctrl, other.ctrl))
    }

    fn geom_bits(&self) -> [u64; 6] {
        [
            self.a.x,
            self.a.y,
            self.b.x,
            self.b.y,
            self.ctrl.x,
            self.ctrl.y,
        ]
        .map(f64::to_bits)
    }
}

/// 中身が同じか（座標はビットまで）
impl PartialEq for StoredEdge {
    fn eq(&self, other: &Self) -> bool {
        self.geom_bits() == other.geom_bits()
            && self.left == other.left
            && self.right == other.right
            && self.left_src == other.left_src
            && self.right_src == other.right_src
    }
}

impl Eq for StoredEdge {}

/// テスト用：しまう辺1本の大きさ（バイト）
#[cfg(test)]
pub(crate) fn edge_size() -> usize {
    std::mem::size_of::<StoredEdge>()
}

/// 塊：1つの升目に入る辺の並び（決まった順。空の塊は無い）
type Tile = Arc<[StoredEdge]>;

/// 塊に入れる辺：（升目, しまう辺, (地図の辺の番号, 向きを逆にしたか)）
type Placed = ((i32, i32), StoredEdge, (usize, bool));

/// 地図 `map` の辺 `e` を、塊に入れる辺にする
fn placed(map: &PlanarMap, e: usize) -> Placed {
    let ed = &map.edges[e];
    let se = StoredEdge::of(map, ed);
    // `StoredEdge::of` と同じ決め方
    let flipped = point_order(map.nodes[ed.a], map.nodes[ed.b]).is_gt();
    (se.tile(), se, (e, flipped))
}

#[cfg(test)]
thread_local! {
    /// テスト用：このスレッドで、塊を作るために並べた辺の数（`from_dense_ordered`・`from_dense_since`）
    pub(crate) static EDGES_SORTED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// テスト用：このスレッドで、塊を作るために並べた辺の数
#[cfg(test)]
pub(crate) fn edges_sorted() -> usize {
    EDGES_SORTED.with(|n| n.get())
}

/// 升目の順の塊 `prev` のうち、辺の並び `run`（1つの升目の、決まった順の辺）と中身が同じ塊。`j` は `prev` をなめた位置
/// （升目の順に呼ぶ）。中身が同じなら升目も同じ（升目は辺で決まる）
fn same_tile(prev: &[Tile], j: &mut usize, run: &[StoredEdge]) -> Option<Tile> {
    let at = run[0].tile();
    while prev.get(*j).is_some_and(|t| t[0].tile() < at) {
        *j += 1;
    }
    prev.get(*j).filter(|t| t[..] == *run).cloned()
}

/// セル画にしまう地図：塊の並び（升目の順）。写しは `Arc` を1つ数えるだけ
#[derive(Clone, Debug)]
pub(crate) struct StoredMap(Arc<Stored>);

#[derive(Debug)]
struct Stored {
    tiles: Box<[Tile]>,
    /// この地図の計算に使う写しのうち、いま誰かが持っているもの（弱い参照。生かすのは写しを使う所：`LayerBase::of_stored`）
    dense: Mutex<Weak<LayerBase>>,
    /// 塊ごとの、最初の辺の、計算に使う地図（`to_dense`）での番号（初めて要るときに作る：`dense_edge`）
    starts: OnceLock<Box<[u32]>>,
}

impl StoredMap {
    /// 地図 `map` のしまう形。辺を決まった向きにして升目ごとに分け、塊の中を決まった順に並べる。置き換える前の地図
    /// `prev` の同じ升目の塊と中身が同じ塊は、その塊を共有する（塊はどちらも升目の順なので、並べて1回なめるだけ）
    pub(crate) fn from_dense(map: &PlanarMap, prev: Option<&StoredMap>) -> Self {
        Self::from_dense_ordered(map, prev, None).0
    }

    /// `from_dense` と、しまった辺の順（`to_dense` の辺の順）ごとの、`map` の辺の番号と、向きを逆にしたか（`map` の辺の
    /// 順に作ったもの、たとえば判定の形を、しまう形から作った地図に引き継ぐため）。`origin` は、`map` の辺ごとの、`prev`
    /// の辺（`prev.to_dense()` の辺の番号）のうち形が同じもの（`PlanarMap::edge_origins`）。あれば、`prev` の塊のうち、
    /// 辺がみな中身まで同じ辺として残り、ほかの辺が入らない塊は、並べ直さずにそのまま使う（`from_dense_since`）
    pub(crate) fn from_dense_ordered(
        map: &PlanarMap,
        prev: Option<&StoredMap>,
        origin: Option<&[EdgeOrigin]>,
    ) -> (Self, Vec<(usize, bool)>) {
        if let (Some(prev), Some(origin)) = (prev, origin) {
            return Self::from_dense_since(map, prev, origin);
        }
        let mut all: Vec<Placed> = (0..map.edges.len()).map(|e| placed(map, e)).collect();
        #[cfg(test)]
        EDGES_SORTED.with(|n| n.set(n.get() + all.len()));
        all.sort_unstable_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.order(&y.1)));
        let order: Vec<(usize, bool)> = all.iter().map(|x| x.2).collect();
        let (keys, edges): (Vec<(i32, i32)>, Vec<StoredEdge>) =
            all.into_iter().map(|(k, e, _)| (k, e)).unzip();
        let prev: &[Tile] = prev.map_or(&[], |p| &p.0.tiles);
        let (mut j, mut start) = (0, 0);
        let tiles: Vec<Tile> = keys
            .chunk_by(|a, b| a == b)
            .map(|k| {
                let run = &edges[start..start + k.len()];
                start += k.len();
                same_tile(prev, &mut j, run).unwrap_or_else(|| run.into())
            })
            .collect();
        let stored = Self(Arc::new(Stored {
            tiles: tiles.into(),
            dense: Mutex::new(Weak::new()),
            starts: OnceLock::new(),
        }));
        (stored, order)
    }

    /// `from_dense_ordered` を、前の地図 `prev` と辺の対応 `origin` から求める。`prev` の塊は升目の順で、`prev.to_dense()`
    /// の辺の番号は塊の順・塊の中の順なので、`origin` の辺の番号から塊が分かる。
    /// - `map` の辺のうち、`prev` の形が同じ辺と中身（色・側の図形の番号）も同じものは、その辺のまま残る（升目も同じ）
    /// - 残らなかった `prev` の辺の塊と、残る辺でない `map` の辺の升目の塊は、作り直す（その升目の辺を並べ直す）
    /// - ほかの塊は、中身が同じ（辺がみな同じ辺として残り、ほかの辺は入らない。並びの順は形だけで決まる）ので、そのまま使う
    ///
    /// 作り直す塊は、どれも `prev` の同じ升目の塊と中身が違う（残らなかった辺があるか、`prev` に無い中身の辺が入る）ので、
    /// 全部を並べ直す `from_dense_ordered` と同じ塊・同じ共有・同じ辺の順になる
    fn from_dense_since(
        map: &PlanarMap,
        prev: &StoredMap,
        origin: &[EdgeOrigin],
    ) -> (Self, Vec<(usize, bool)>) {
        let ptiles = &prev.0.tiles;
        // 塊ごとの、最初の辺の番号（`prev.to_dense()` の）と、辺ごとの塊の番号
        let mut starts: Vec<usize> = Vec::with_capacity(ptiles.len());
        let mut tile_of: Vec<u32> = Vec::new();
        for (k, t) in ptiles.iter().enumerate() {
            starts.push(tile_of.len());
            tile_of.extend(std::iter::repeat_n(k as u32, t.len()));
        }
        let prev_edge = |i: usize| {
            let k = tile_of[i] as usize;
            &ptiles[k][i - starts[k]]
        };
        let flipped = |e: usize| {
            let ed = &map.edges[e];
            point_order(map.nodes[ed.a], map.nodes[ed.b]).is_gt()
        };
        // `prev` の辺ごとの、同じ辺として残る `map` の辺（無ければ `usize::MAX`）
        let mut kept = vec![usize::MAX; tile_of.len()];
        let mut fresh: Vec<Placed> = Vec::new();
        for (e, o) in origin.iter().enumerate() {
            if let EdgeOrigin::Same(i) | EdgeOrigin::Reversed(i) = *o {
                if *prev_edge(i) == StoredEdge::of(map, &map.edges[e]) {
                    kept[i] = e;
                    continue;
                }
            }
            fresh.push(placed(map, e));
        }
        let mut redo = vec![false; ptiles.len()];
        for (i, &e) in kept.iter().enumerate() {
            if e == usize::MAX {
                redo[tile_of[i] as usize] = true;
            }
        }
        for p in &fresh {
            if let Ok(k) = ptiles.binary_search_by_key(&p.0, |t| t[0].tile()) {
                redo[k] = true;
            }
        }
        // 作り直す塊の、残る辺も並べ直す辺に入れる（中身は `map` の辺と同じ）
        for (k, t) in ptiles.iter().enumerate().filter(|(k, _)| redo[*k]) {
            for (j, se) in t.iter().enumerate() {
                let e = kept[starts[k] + j];
                if e != usize::MAX {
                    fresh.push((se.tile(), *se, (e, flipped(e))));
                }
            }
        }
        #[cfg(test)]
        EDGES_SORTED.with(|c| c.set(c.get() + fresh.len()));
        fresh.sort_unstable_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.order(&y.1)));
        // 升目の順に、そのまま使う塊と作り直した塊を並べる
        let mut tiles: Vec<Tile> = Vec::new();
        let mut order: Vec<(usize, bool)> = Vec::with_capacity(map.edges.len());
        let (mut k, mut f) = (0, 0);
        while k < ptiles.len() || f < fresh.len() {
            let pk = ptiles.get(k).map(|t| t[0].tile());
            let fk = fresh.get(f).map(|x| x.0);
            match (pk, fk) {
                (Some(a), b) if b.is_none_or(|b| a < b) => {
                    if !redo[k] {
                        tiles.push(ptiles[k].clone());
                        order.extend((starts[k]..starts[k] + ptiles[k].len()).map(|i| {
                            let e = kept[i];
                            (e, flipped(e))
                        }));
                    }
                    k += 1;
                }
                (a, Some(b)) => {
                    // 同じ升目の `prev` の塊は作り直す塊（その升目に並べ直す辺がある）
                    if a == Some(b) {
                        k += 1;
                    }
                    let run = fresh[f..].iter().take_while(|x| x.0 == b).count();
                    let group = &fresh[f..f + run];
                    tiles.push(group.iter().map(|x| x.1).collect());
                    order.extend(group.iter().map(|x| x.2));
                    f += run;
                }
                _ => unreachable!(),
            }
        }
        let stored = Self(Arc::new(Stored {
            tiles: tiles.into(),
            dense: Mutex::new(Weak::new()),
            starts: OnceLock::new(),
        }));
        (stored, order)
    }

    /// 地図 `map` を、しまった辺の順 `order`（`from_dense_ordered`）に並べ直した、計算に使う地図。`to_dense` と同じ地図
    /// （辺はしまう辺の向きにし、頂点は最初に出てきた順に番号を振る）を、座標を引かずに `map` の頂点の番号で作る（正しい
    /// 地図では、別々の頂点は別々の座標なので、同じ頂点の番号は同じ座標と同じ）
    pub(crate) fn dense_in_order(map: &PlanarMap, order: &[(usize, bool)]) -> PlanarMap {
        let mut number = vec![usize::MAX; map.nodes.len()];
        let mut nodes: Vec<Point> = Vec::new();
        let mut node = |v: usize| {
            if number[v] == usize::MAX {
                number[v] = nodes.len();
                nodes.push(map.nodes[v]);
            }
            number[v]
        };
        let edges = order
            .iter()
            .map(|&(e, flipped)| {
                let ed = &map.edges[e];
                let (a, b) = if flipped { (ed.b, ed.a) } else { (ed.a, ed.b) };
                let se = StoredEdge::of(map, ed);
                let a = node(a);
                se.dense(a, node(b))
            })
            .collect();
        PlanarMap { nodes, edges }
    }

    /// 計算に使う写しへの弱い参照（`LayerBase::of_stored` だけが使う）
    pub(crate) fn dense_slot(&self) -> &Mutex<Weak<LayerBase>> {
        &self.0.dense
    }

    /// 計算に使うふつうの地図：辺は塊の順・塊の中の順に並べ、頂点は最初に出てきた順に番号を振る（番号が中身だけで決まる）
    pub(crate) fn to_dense(&self) -> PlanarMap {
        let n: usize = self.0.tiles.iter().map(|t| t.len()).sum();
        let mut nodes: Vec<Point> = Vec::with_capacity(n);
        let mut index: crate::planar_map::IntMap<[u64; 2], usize> =
            crate::planar_map::IntMap::with_capacity_and_hasher(n, Default::default());
        let mut node = |p: Point| {
            *index
                .entry([p.x.to_bits(), p.y.to_bits()])
                .or_insert_with(|| {
                    nodes.push(p);
                    nodes.len() - 1
                })
        };
        let edges: Vec<MapEdge> = self
            .0
            .tiles
            .iter()
            .flat_map(|t| t.iter())
            .map(|e| {
                let a = node(e.a);
                e.dense(a, node(e.b))
            })
            .collect();
        nodes.shrink_to_fit();
        PlanarMap { nodes, edges }
    }

    /// 塊ごとの、最初の辺の番号（計算に使う地図 `to_dense` の辺の番号は、塊の順・塊の中の順なので、塊の大きさの和）
    fn starts(&self) -> &[u32] {
        self.0.starts.get_or_init(|| {
            let mut at = 0u32;
            self.0
                .tiles
                .iter()
                .map(|t| {
                    let s = at;
                    at += t.len() as u32;
                    s
                })
                .collect()
        })
    }

    /// 升目 `home` の塊の `j` 番目の辺の、計算に使う地図（`to_dense`）での番号。その塊が無ければ `None`
    fn dense_edge(&self, home: (i32, i32), j: u32) -> Option<usize> {
        let k = self
            .0
            .tiles
            .binary_search_by_key(&home, |t| t[0].tile())
            .ok()?;
        Some((self.starts()[k] + j) as usize)
    }

    /// `other` と、同じ升目に同じ塊（同じ `Arc`）を持つ数（塊の数に比例する手間。辺の数にはよらない）
    pub(crate) fn shared_tile_count(&self, other: &Self) -> usize {
        let (a, b) = (&self.0.tiles, &other.0.tiles);
        let (mut i, mut k, mut n) = (0, 0, 0);
        while i < a.len() && k < b.len() {
            match a[i][0].tile().cmp(&b[k][0].tile()) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => k += 1,
                std::cmp::Ordering::Equal => {
                    n += Arc::ptr_eq(&a[i], &b[k]) as usize;
                    i += 1;
                    k += 1;
                }
            }
        }
        n
    }

    /// 同じ `Arc` か（同じしまった地図）
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// この地図の番地（同じ `Arc` なら同じ。その地図を持っている間は、ほかの地図に使い回されない）
    pub(crate) fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as *const u8 as usize
    }

    /// 辺の無い地図か
    pub(crate) fn is_empty(&self) -> bool {
        self.0.tiles.is_empty()
    }

    /// 塊の並び `tiles`（升目の順）の地図
    fn of_tiles(tiles: Box<[Tile]>) -> Self {
        Self(Arc::new(Stored {
            tiles,
            dense: Mutex::new(Weak::new()),
            starts: OnceLock::new(),
        }))
    }

    /// 側の図形の番号を `renew` で替えた地図（番号の無い側は無いまま）。形と色は変えず、辺の並びも同じなので、正しい地図は
    /// 正しいまま（確かめ直さない）。番号の無い塊は、この地図と共有する
    pub(crate) fn relabeled(&self, mut renew: impl FnMut(ShapeId) -> ShapeId) -> Self {
        let mut new_src =
            |src: Option<NonZeroU64>| src.and_then(|s| NonZeroU64::new(renew(s.get())));
        let tiles = self
            .0
            .tiles
            .iter()
            .map(|t| {
                if t.iter()
                    .all(|e| e.left_src.is_none() && e.right_src.is_none())
                {
                    return t.clone();
                }
                t.iter()
                    .map(|e| StoredEdge {
                        left_src: new_src(e.left_src),
                        right_src: new_src(e.right_src),
                        ..*e
                    })
                    .collect()
            })
            .collect();
        Self::of_tiles(tiles)
    }

    /// 中身が同じか（塊と辺が、ビットまで同じ）
    #[cfg(test)]
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.0.tiles.len() == other.0.tiles.len()
            && self
                .0
                .tiles
                .iter()
                .zip(other.0.tiles.iter())
                .all(|(x, y)| x[..] == y[..])
    }

    /// テスト用：この地図の大きさ（塊の並びのバイト, 辺のバイト）。`seen` にある塊・塊の並びは数えない（共有している
    /// ものは1回だけ数える）
    #[cfg(test)]
    pub(crate) fn bytes_unseen(
        &self,
        seen: &mut std::collections::HashSet<usize>,
    ) -> (usize, usize) {
        let mut bytes = (0, 0);
        if seen.insert(Arc::as_ptr(&self.0) as *const u8 as usize) {
            bytes.0 += self.0.tiles.len() * std::mem::size_of::<Tile>();
        }
        for t in self.0.tiles.iter() {
            if seen.insert(Arc::as_ptr(t) as *const u8 as usize) {
                bytes.1 += t.len() * std::mem::size_of::<StoredEdge>();
            }
        }
        bytes
    }

    /// テスト用：塊の数
    #[cfg(test)]
    pub(crate) fn tiles(&self) -> usize {
        self.0.tiles.len()
    }

    /// テスト用：塊ごとの持ち主の数（この地図のほかに持っている地図の数＋1）
    #[cfg(test)]
    pub(crate) fn tile_owners(&self) -> Vec<usize> {
        self.0.tiles.iter().map(Arc::strong_count).collect()
    }

    /// テスト用：`other` と共有している塊（同じ `Arc`）の数
    #[cfg(test)]
    pub(crate) fn shared_tiles(&self, other: &Self) -> usize {
        let theirs: std::collections::HashSet<usize> = other
            .0
            .tiles
            .iter()
            .map(|t| Arc::as_ptr(t) as *const u8 as usize)
            .collect();
        self.0
            .tiles
            .iter()
            .filter(|t| theirs.contains(&(Arc::as_ptr(t) as *const u8 as usize)))
            .count()
    }
}

/// 升目の索引の、辺の指し方：（辺の住む塊の升目, 塊の中の番号）。塊が変わらなければ、地図の版が変わっても同じ辺を指す
type EdgeRef = ((i32, i32), u32);

/// 升目の索引の1升目：（升目, その升目にかかる辺。辺の指し方の順）
type IndexCell = ((i64, i64), Arc<[EdgeRef]>);

/// 升目の索引：索引の升目（近さを調べる格子のマス 4×4）ごとの、その升目にかかる辺（判定の形の近くを探すマス
/// `EdgeGeom::reach` か外接矩形が、その升目にかかる辺：`LayerBase::edge_tiles`）。近くの辺を探す所（重ね合わせの入れる辺を決める所・
/// カーソル合わせ）が、毎回全部の辺を格子に入れ直さずに引く。持つのは計算に使う写し（`LayerBase::index`）で、地図を
/// 書き換えたときは、前の写しの索引から、変わった塊の辺がかかる升目だけを作り直す（`derive`）。設計：
/// claude/触った所だけを計算する地図の設計_22fbbbe.md の 3.3
#[derive(Debug, Default)]
pub(crate) struct TileIndex {
    /// 索引の升目の順の（升目, その升目にかかる辺。辺の指し方の順）
    cells: Box<[IndexCell]>,
}

impl TileIndex {
    /// 地図 `map` の索引を全部作る。`tiles_of(e)` は、計算に使う地図の辺 `e` がかかる索引の升目
    pub(crate) fn build(map: &StoredMap, tiles_of: impl Fn(usize) -> Vec<(i64, i64)>) -> Self {
        let mut all: Vec<((i64, i64), EdgeRef)> = Vec::new();
        let mut e = 0usize;
        for t in map.0.tiles.iter() {
            let home = t[0].tile();
            for j in 0..t.len() {
                all.extend(tiles_of(e).into_iter().map(|c| (c, (home, j as u32))));
                e += 1;
            }
        }
        Self::from_sorted(all)
    }

    /// （升目, 辺）の並びから作る（並べ替えて升目ごとにまとめる）
    fn from_sorted(mut all: Vec<((i64, i64), EdgeRef)>) -> Self {
        all.sort_unstable();
        all.dedup();
        let cells = all
            .chunk_by(|a, b| a.0 == b.0)
            .map(|run| (run[0].0, run.iter().map(|x| x.1).collect()))
            .collect();
        Self { cells }
    }

    /// 地図 `prev` の索引（`self`）から、地図 `next` の索引を作る。`prev` と `next` で、同じ升目に同じ塊（同じ `Arc`）が
    /// ある塊の辺は、どちらでも同じ辺の指し方なので、そのまま残す。ほかの塊（消えた塊・新しい塊）の辺がかかる升目だけを
    /// 作り直す。`prev_tiles(e)`・`next_tiles(e)` は、それぞれの計算に使う地図の辺 `e` がかかる索引の升目。手間は、変わった
    /// 塊の辺の数と、塊・升目の数で決まり、辺の総数にはよらない
    pub(crate) fn derive(
        &self,
        prev: &StoredMap,
        next: &StoredMap,
        prev_tiles: impl Fn(usize) -> Vec<(i64, i64)>,
        next_tiles: impl Fn(usize) -> Vec<(i64, i64)>,
    ) -> Self {
        let (pt, nt) = (&prev.0.tiles, &next.0.tiles);
        let (ps, ns) = (prev.starts(), next.starts());
        // 消えた塊（`prev` の番号）と、新しい塊（`next` の番号）
        let (mut gone, mut fresh): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
        let (mut i, mut k) = (0, 0);
        while i < pt.len() || k < nt.len() {
            let a = pt.get(i).map(|t| t[0].tile());
            let b = nt.get(k).map(|t| t[0].tile());
            match (a, b) {
                (Some(x), Some(y)) if x == y => {
                    if !Arc::ptr_eq(&pt[i], &nt[k]) {
                        gone.push(i);
                        fresh.push(k);
                    }
                    i += 1;
                    k += 1;
                }
                (Some(x), y) if y.is_none_or(|y| x < y) => {
                    gone.push(i);
                    i += 1;
                }
                _ => {
                    fresh.push(k);
                    k += 1;
                }
            }
        }
        if gone.is_empty() && fresh.is_empty() {
            return Self {
                cells: self.cells.clone(),
            };
        }
        // 消える辺の住む塊の升目（新しい塊の升目と同じことがある：作り直した塊。どちらにしても、前の指し方は消す）
        let gone_homes: std::collections::HashSet<(i32, i32)> =
            gone.iter().map(|&i| pt[i][0].tile()).collect();
        // 作り直す升目と、足す辺
        let mut touched: Vec<(i64, i64)> = Vec::new();
        for &i in &gone {
            for e in ps[i] as usize..ps[i] as usize + pt[i].len() {
                touched.extend(prev_tiles(e));
            }
        }
        let mut added: Vec<((i64, i64), EdgeRef)> = Vec::new();
        for &k in &fresh {
            let home = nt[k][0].tile();
            for j in 0..nt[k].len() {
                for c in next_tiles(ns[k] as usize + j) {
                    touched.push(c);
                    added.push((c, (home, j as u32)));
                }
            }
        }
        touched.sort_unstable();
        touched.dedup();
        added.sort_unstable();
        // 作り直す升目の中身：前の中身から消える塊の辺を除き、足す辺を入れる
        let mut rebuilt: Vec<IndexCell> = Vec::with_capacity(touched.len());
        let mut a = 0;
        for &c in &touched {
            let mut list: Vec<EdgeRef> = match self.cells.binary_search_by_key(&c, |x| x.0) {
                Ok(at) => self.cells[at]
                    .1
                    .iter()
                    .copied()
                    .filter(|r| !gone_homes.contains(&r.0))
                    .collect(),
                Err(_) => Vec::new(),
            };
            while a < added.len() && added[a].0 < c {
                a += 1;
            }
            while a < added.len() && added[a].0 == c {
                list.push(added[a].1);
                a += 1;
            }
            if !list.is_empty() {
                list.sort_unstable();
                list.dedup();
                rebuilt.push((c, list.into()));
            }
        }
        // 升目の順に、変わらない升目と作り直した升目を並べる
        let mut cells: Vec<IndexCell> = Vec::with_capacity(self.cells.len() + rebuilt.len());
        let (mut x, mut y) = (0, 0);
        while x < self.cells.len() || y < touched.len() {
            let old = self.cells.get(x).map(|c| c.0);
            let t = touched.get(y).copied();
            match (old, t) {
                (Some(o), Some(t)) if o == t => {
                    x += 1;
                    y += 1;
                }
                (Some(o), t) if t.is_none_or(|t| o < t) => {
                    cells.push(self.cells[x].clone());
                    x += 1;
                }
                _ => y += 1,
            }
        }
        // 作り直した升目を入れる（升目の順に並べ直す）
        cells.extend(rebuilt);
        cells.sort_unstable_by_key(|c| c.0);
        Self {
            cells: cells.into(),
        }
    }

    /// 索引の升目 `tile` にかかる辺の、地図 `map`（この索引の地図）の計算に使う地図での番号を `out` に足す
    pub(crate) fn edges_in(&self, map: &StoredMap, tile: (i64, i64), out: &mut Vec<usize>) {
        if let Ok(at) = self.cells.binary_search_by_key(&tile, |x| x.0) {
            out.extend(
                self.cells[at]
                    .1
                    .iter()
                    .filter_map(|&(home, j)| map.dense_edge(home, j)),
            );
        }
    }

    /// 辺の入っている升目の x のうち、いちばん大きいもの（辺が無ければ `None`。升目は x・y の順に並べてある）
    pub(crate) fn max_tile_x(&self) -> Option<i64> {
        self.cells.last().map(|c| c.0 .0)
    }

    /// テスト用：索引の中身（升目ごとの、辺の指し方の並び）が同じか
    #[cfg(test)]
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.cells.len() == other.cells.len()
            && self
                .cells
                .iter()
                .zip(other.cells.iter())
                .all(|(a, b)| a.0 == b.0 && a.1[..] == b.1[..])
    }
}

/// 塊の保存の形：塊の辺の端の点の座標（丸めない：読み戻すとビットまで同じ値になる。辺どうしで共有する端は1回だけ書く）
/// と、辺の並び
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct SavedTile {
    pub(crate) nodes: Vec<[f64; 2]>,
    pub(crate) edges: Vec<SavedEdge>,
}

/// しまう辺の保存の形：両端（塊の点の番号）、制御点の座標、左右の色（文書の色の表の 1 からの番号。0 は何も無い）、左右の
/// 側の図形の印（文書の中の 1 からの通し番号で、同じ印は同じ図形。0 は無い。読むときに新しい図形の番号に替える：
/// `TileWriter`・`TileReader`）
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SavedEdge {
    pub(crate) a: u32,
    pub(crate) b: u32,
    pub(crate) ctrl: [f64; 2],
    pub(crate) left: u32,
    pub(crate) right: u32,
    pub(crate) left_src: u64,
    pub(crate) right_src: u64,
}

/// 文書の地図の塊の表：色の表と、塊の表。地図は塊の番号の並びで指すので、キーフレームどうしで共有する塊は1回だけ書く
/// （`TileWriter`・`TileReader`）
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct SavedTiles {
    pub(crate) colors: Vec<[u8; 4]>,
    pub(crate) tiles: Vec<SavedTile>,
}

/// 地図を保存の形（塊の表と、地図ごとの塊の番号の並び：`SavedTiles`）にする。同じ塊（同じ `Arc`）は1回だけ書くので、
/// キーフレームや Undo の控えで共有している塊は、ファイルでも共有したまま（ファイルの大きさが変わった塊の量に見合う：
/// レビュー f01d518 の N-5001）
#[derive(Default)]
pub(crate) struct TileWriter {
    index: IntMap<usize, u32>,
    colors: std::collections::HashMap<[u8; 4], u32>,
    /// 図形の番号から、書く印（1 からの通し番号。初めて出てきた順）
    labels: IntMap<ShapeId, u64>,
    out: SavedTiles,
}

impl TileWriter {
    /// 地図 `map` の塊の番号の並び（まだ書いていない塊は表に足す）
    pub(crate) fn tiles_of(&mut self, map: &StoredMap) -> Vec<u32> {
        map.0
            .tiles
            .iter()
            .map(|t| {
                let key = Arc::as_ptr(t) as *const u8 as usize;
                if let Some(&i) = self.index.get(&key) {
                    return i;
                }
                let mut tile = SavedTile::default();
                let mut node_of: IntMap<[u64; 2], u32> = IntMap::default();
                for e in t.iter() {
                    let mut node = |p: Point| -> u32 {
                        *node_of
                            .entry([p.x.to_bits(), p.y.to_bits()])
                            .or_insert_with(|| {
                                tile.nodes.push([p.x, p.y]);
                                tile.nodes.len() as u32 - 1
                            })
                    };
                    let (a, b) = (node(e.a), node(e.b));
                    let edge = self.saved_edge(e, a, b);
                    tile.edges.push(edge);
                }
                let i = self.out.tiles.len() as u32;
                self.out.tiles.push(tile);
                self.index.insert(key, i);
                i
            })
            .collect()
    }

    fn saved_edge(&mut self, e: &StoredEdge, a: u32, b: u32) -> SavedEdge {
        let mut color = |c: Option<Color32>| -> u32 {
            let Some(c) = c else {
                return 0;
            };
            let next = self.colors.len() as u32 + 1;
            let no = *self.colors.entry(c.to_array()).or_insert(next);
            if no == next {
                self.out.colors.push(c.to_array());
            }
            no
        };
        let left = color(e.left);
        let right = color(e.right);
        let mut label = |id: Option<NonZeroU64>| -> u64 {
            let Some(id) = id else {
                return 0;
            };
            let next = self.labels.len() as u64 + 1;
            *self.labels.entry(id.get()).or_insert(next)
        };
        SavedEdge {
            a,
            b,
            ctrl: [e.ctrl.x, e.ctrl.y],
            left,
            right,
            left_src: label(e.left_src),
            right_src: label(e.right_src),
        }
    }

    /// 書いた塊の表
    pub(crate) fn finish(self) -> SavedTiles {
        self.out
    }
}

/// 保存の形の塊の表（`SavedTiles`）から読んだ塊。地図は塊の番号の並びから作り、同じ番号の塊は共有する（ファイルで共有
/// していた塊は、読んでも共有したまま。見る順によらない）
pub(crate) struct TileReader {
    tiles: Vec<Tile>,
}

impl TileReader {
    /// 塊の表 `saved` を読む。色の番号・点の番号が表の外か、辺の無い塊があれば `None`（壊れたファイル）。辺の側の図形の
    /// 印（1 からの通し番号：`TileWriter`）は、続けて取った新しい図形の番号に替える（同じ印は同じ番号。ファイルに書いた
    /// 数がそのまま番号になることは無いので、細工したファイルでも、これから振る番号が今ある番号と重ならない）。印が辺の
    /// 側の数より大きければ（アプリは書かない）`None`
    pub(crate) fn new(saved: &SavedTiles) -> Option<Self> {
        let sides: u64 = saved.tiles.iter().map(|t| 2 * t.edges.len() as u64).sum();
        let labels = saved
            .tiles
            .iter()
            .flat_map(|t| &t.edges)
            .map(|e| e.left_src.max(e.right_src))
            .max()
            .unwrap_or(0);
        if labels > sides {
            return None;
        }
        let first = crate::vector_engine::new_shape_ids(labels);
        let label = |n: u64| NonZeroU64::new(n).and_then(|n| NonZeroU64::new(first + n.get() - 1));
        let color = |i: u32| -> Option<Option<Color32>> {
            match i {
                0 => Some(None),
                _ => saved
                    .colors
                    .get(i as usize - 1)
                    .map(|c| Some(Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3]))),
            }
        };
        let mut tiles = Vec::with_capacity(saved.tiles.len());
        for t in &saved.tiles {
            let node = |i: u32| t.nodes.get(i as usize).map(|p| Point::new(p[0], p[1]));
            let edges: Option<Vec<StoredEdge>> = t
                .edges
                .iter()
                .map(|e| {
                    Some(StoredEdge {
                        a: node(e.a)?,
                        b: node(e.b)?,
                        ctrl: Point::new(e.ctrl[0], e.ctrl[1]),
                        left: color(e.left)?,
                        right: color(e.right)?,
                        left_src: label(e.left_src),
                        right_src: label(e.right_src),
                    })
                })
                .collect();
            let edges = edges?;
            if edges.is_empty() {
                return None;
            }
            tiles.push(edges.into());
        }
        Some(Self { tiles })
    }

    /// 塊の番号の並び `indices` の地図。番号が表の外なら `None`（壊れたファイル）
    pub(crate) fn map(&self, indices: &[u32]) -> Option<StoredMap> {
        let tiles: Option<Box<[Tile]>> = indices
            .iter()
            .map(|&i| self.tiles.get(i as usize).cloned())
            .collect();
        Some(StoredMap::of_tiles(tiles?))
    }
}
