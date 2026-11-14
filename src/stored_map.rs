//! セル画にしまう地図（境目の線を1本で持つ本当の形）：場所ごとの塊に分けた辺の並び。
//!
//! 辺は両端を座標で指し（頂点の番号を持たない）、決まった向き・決まった順に並べる。塊は、辺の弦の中点がある `TILE` px の
//! 升目ごとの辺の並び。地図をしまうときは、置き換える前の地図（読み込みでは同じレイヤーの直前のセル）の同じ升目の塊と
//! 中身が同じなら、その塊を使う（`StoredMap::from_dense`）。Undo の控えが持つのは置き換える前の地図そのものなので、編集で
//! 変わらなかった場所の塊は控えと共有したまま（控え1つ分の大きさは、変わった場所の塊の分だけ）。塊を持つのは地図だけ
//! なので、どの地図も持たなくなった塊は、その場で返る。
//!
//! 計算に使うふつうの地図（`PlanarMap`）は、いつも塊から決まった順で作る（`StoredMap::to_dense`）。頂点と辺の番号が中身
//! だけで決まるので、いつ作り直しても同じになる。計算に使う写し（`LayerBase`）は、それを使う所が持つ。しまう形は、いま
//! 誰かが持っている写しへの弱い参照を1つだけ持ち（`StoredMap::dense_slot`）、写しを生かすのは持ち主だけ（Undo の控えは
//! しまう形だけを持つ）。

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, Weak};

use eframe::egui::Color32;
use kurbo::Point;

use crate::planar_map::{point_order, IntMap, LayerBase, MapEdge, PlanarMap};
use crate::project_io::{SavedEdge, SavedTile, SavedTiles};
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
}

impl StoredMap {
    /// 地図 `map` のしまう形。辺を決まった向きにして升目ごとに分け、塊の中を決まった順に並べる。置き換える前の地図
    /// `prev` の同じ升目の塊と中身が同じ塊は、その塊を共有する（塊はどちらも升目の順なので、並べて1回なめるだけ）
    pub(crate) fn from_dense(map: &PlanarMap, prev: Option<&StoredMap>) -> Self {
        let mut all: Vec<((i32, i32), StoredEdge)> = map
            .edges
            .iter()
            .map(|e| {
                let se = StoredEdge::of(map, e);
                (se.tile(), se)
            })
            .collect();
        all.sort_unstable_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.order(&y.1)));
        let (keys, edges): (Vec<(i32, i32)>, Vec<StoredEdge>) = all.into_iter().unzip();
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
        Self(Arc::new(Stored {
            tiles: tiles.into(),
            dense: Mutex::new(Weak::new()),
        }))
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
            .map(|e| MapEdge {
                a: node(e.a),
                b: node(e.b),
                ctrl: e.ctrl,
                left: e.left,
                right: e.right,
                left_src: e.left_src.map(NonZeroU64::get),
                right_src: e.right_src.map(NonZeroU64::get),
            })
            .collect();
        nodes.shrink_to_fit();
        PlanarMap { nodes, edges }
    }

    /// 同じ `Arc` か（同じしまった地図）
    #[cfg(test)]
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
