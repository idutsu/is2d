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
use eframe::egui::mutex::Mutex;
use std::sync::{Arc, OnceLock, Weak};

use eframe::egui::Color32;
use kurbo::{Point, QuadBez};
use serde::de::{self, DeserializeSeed, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::planar_map::{
    EdgeStroke, STROKE_WIDTHS,
    point_order, tile_edge, EdgeGeom, IntMap, LayerBase, MapEdge, PlanarMap, Poly,
};
use crate::vector_engine::{ShapeId, SyncCache};

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
    stroke: Option<EdgeStroke>,
    /// 線の番号（線のある辺だけ：`planar_map::line_id_of`）
    line_id: Option<NonZeroU64>,
}

impl StoredEdge {
    /// 塊から引いた辺の形の辺（決まった向きにする）
    fn of_tile(te: &TileEdge) -> Self {
        let src = |id: Option<ShapeId>| id.and_then(NonZeroU64::new);
        if point_order(te.a, te.b).is_le() {
            Self {
                a: te.a,
                b: te.b,
                ctrl: te.ctrl,
                left: te.left,
                right: te.right,
                left_src: src(te.left_src),
                right_src: src(te.right_src),
                stroke: te.stroke,
                line_id: src(te.line_id),
            }
        } else {
            Self {
                a: te.b,
                b: te.a,
                ctrl: te.ctrl,
                left: te.right,
                right: te.left,
                left_src: src(te.right_src),
                right_src: src(te.left_src),
                stroke: te.stroke,
                line_id: src(te.line_id),
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
            stroke: self.stroke,
            line_id: self.line_id.map(NonZeroU64::get),
        }
    }

    /// 塊から引いた辺の形（向きはこのしまう辺の向き）
    fn tile_edge(&self) -> TileEdge {
        TileEdge {
            a: self.a,
            b: self.b,
            ctrl: self.ctrl,
            left: self.left,
            right: self.right,
            left_src: self.left_src.map(NonZeroU64::get),
            right_src: self.right_src.map(NonZeroU64::get),
            stroke: self.stroke,
            line_id: self.line_id.map(NonZeroU64::get),
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
            && self.stroke == other.stroke
    }
}

impl Eq for StoredEdge {}

impl std::hash::Hash for StoredEdge {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.geom_bits().hash(h);
        self.left.hash(h);
        self.right.hash(h);
        self.left_src.hash(h);
        self.right_src.hash(h);
        self.stroke.hash(h);
        self.line_id.hash(h);
    }
}

/// 塊から引いた辺（`StoredMap::edge_at`）：両端の座標・制御点・左右の色・左右の側の図形の番号・線
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TileEdge {
    pub(crate) a: Point,
    pub(crate) b: Point,
    pub(crate) ctrl: Point,
    pub(crate) left: Option<Color32>,
    pub(crate) right: Option<Color32>,
    pub(crate) left_src: Option<ShapeId>,
    pub(crate) right_src: Option<ShapeId>,
    pub(crate) stroke: Option<EdgeStroke>,
    /// 線の番号（線のある辺だけ。しまった番号：`planar_map::line_id_of`）
    pub(crate) line_id: Option<ShapeId>,
}

impl TileEdge {
    pub(crate) fn quad(&self) -> QuadBez {
        QuadBez::new(self.a, self.ctrl, self.b)
    }
}

/// テスト用：しまう辺1本の大きさ（バイト）
#[cfg(test)]
pub(crate) fn edge_size() -> usize {
    std::mem::size_of::<StoredEdge>()
}

/// 塊：1つの升目に入る辺の並び（決まった順。空の塊は無い）と、その辺の判定の形への弱い参照（塊を共有する地図どうしで
/// 共有する：設計 段2b の 1.）
type Tile = Arc<TileData>;

/// 番号を替えて作り直した塊の控え（元の塊の番地 → 元の塊と、作り直した塊。`StoredMap::relabeled`）。同じ替え方で替える
/// 間（1回の作業）だけ使い回すので、同じ塊は同じ新しい塊になり、塊を共有していた地図は、替えたあとも共有する。元の塊も
/// 持つので、控えている間は、その番地がほかの塊に使い回されない
#[derive(Default)]
pub(crate) struct Relabeled(IntMap<usize, (Tile, Tile)>);

/// 塊の辺ごとの判定の形（`edges` の順）。並びは `Box` に入れる：塊が持つ弱い参照が残す領域を、生かす所が無くなったあと
/// 小さくするため（`Arc<[Poly]>` だと、並びそのものが弱い参照の残す領域に入り、辺1本ごとに残る）
type TilePolys = Arc<Box<[Poly]>>;

/// 塊の中身
struct TileData {
    edges: Box<[StoredEdge]>,
    /// 辺ごとの判定の形（しまう辺の向きの曲線から作る：`EdgeGeom::of`）への弱い参照。判定の形を生かすのは、それを使う
    /// 計算に使う写しの `PolyHold` だけ（塊は文書と Undo の控えが持つので、塊が判定の形を生かすと、一度見たコマの判定の
    /// 形がタブを閉じるまで残る）。誰かが生かしている間は、塊を共有する地図の写しどうしで同じものを使う。鍵は確保しない
    /// 鍵（`egui::mutex::Mutex`）：塊は作品の大きさに比例してあり、macOS の `std::sync::Mutex` は初めてかけたときに確保
    /// した領域を塊がなくなるまで持つので、一度引いた塊ごとに残る
    polys: Mutex<Weak<Box<[Poly]>>>,
}

impl std::fmt::Debug for TileData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileData").field("edges", &self.edges).finish_non_exhaustive()
    }
}

impl std::ops::Deref for TileData {
    type Target = [StoredEdge];
    fn deref(&self) -> &[StoredEdge] {
        &self.edges
    }
}

impl TileData {
    fn new(edges: Box<[StoredEdge]>) -> Tile {
        Arc::new(Self {
            edges,
            polys: Mutex::new(Weak::new()),
        })
    }

    /// 辺ごとの判定の形：誰かが生かしていればそれ、無ければ作る（作ったものを生かすのは、受け取った所だけ）
    fn polys(&self) -> TilePolys {
        let mut slot = self.polys.lock();
        if let Some(p) = slot.upgrade() {
            return p;
        }
        let p: TilePolys = Arc::new(
            self.edges
                .iter()
                .map(|e| EdgeGeom::of(&QuadBez::new(e.a, e.ctrl, e.b)))
                .collect(),
        );
        *slot = Arc::downgrade(&p);
        p
    }

}

/// 判定の形を生かしておく所：塊ごとの判定の形（塊の並びの位置で引く。初めて要るときに作る）。計算に使う写し
/// （`LayerBase`）が、自分の地図のものを1つ持つ。写しを手放せば、ほかに使う写しの無い判定の形は返る
#[derive(Default, Debug)]
pub(crate) struct PolyHold(OnceLock<Box<[OnceLock<TilePolys>]>>);

#[cfg(test)]
thread_local! {
    /// テスト用：このスレッドで、塊を作るために並べた辺の数（`from_dense`）
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

struct Stored {
    tiles: Box<[Tile]>,
    /// この地図の計算に使う写しのうち、いま誰かが持っているもの（弱い参照。生かすのは写しを使う所：`LayerBase::of_stored`）
    dense: SyncCache<Weak<LayerBase>>,
    /// 塊ごとの、最初の辺の、計算に使う地図（`to_dense`）での番号（初めて要るときに作る：`dense_edge`）
    starts: OnceLock<Box<[u32]>>,
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored").field("tiles", &self.tiles).finish_non_exhaustive()
    }
}

impl StoredMap {
    /// 地図 `map` のしまう形。辺を決まった向きにして升目ごとに分け、塊の中を決まった順に並べる。置き換える前の地図
    /// `prev` の同じ升目の塊と中身が同じ塊は、その塊を共有する（塊はどちらも升目の順なので、並べて1回なめるだけ）
    pub(crate) fn from_dense(map: &PlanarMap, prev: Option<&StoredMap>) -> Self {
        // 塊に入れる辺：（升目, しまう辺）
        let mut all: Vec<((i32, i32), StoredEdge)> = map
            .edges
            .iter()
            .map(|ed| {
                let se = StoredEdge::of_tile(&tile_edge(map, ed));
                (se.tile(), se)
            })
            .collect();
        #[cfg(test)]
        EDGES_SORTED.with(|n| n.set(n.get() + all.len()));
        all.sort_unstable_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.order(&y.1)));
        let (keys, edges): (Vec<(i32, i32)>, Vec<StoredEdge>) = all.into_iter().unzip();
        let prev_tiles: &[Tile] = prev.map_or(&[], |p| &p.0.tiles);
        let (mut j, mut start) = (0, 0);
        let tiles: Vec<Tile> = keys
            .chunk_by(|a, b| a == b)
            .map(|k| {
                let run = &edges[start..start + k.len()];
                start += k.len();
                same_tile(prev_tiles, &mut j, run).unwrap_or_else(|| TileData::new(run.into()))
            })
            .collect();
        // 塊がみな前の地図の塊そのものなら、前の地図そのもの（同じ地図を、写しも含めて共有する）
        if let Some(p) = prev {
            if tiles.len() == prev_tiles.len()
                && tiles.iter().zip(prev_tiles).all(|(a, b)| Arc::ptr_eq(a, b))
            {
                return p.clone();
            }
        }
        Self(Arc::new(Stored {
            tiles: tiles.into(),
            dense: SyncCache::new(Weak::new()),
            starts: OnceLock::new(),
        }))
    }

    /// 計算に使う写しへの弱い参照（`LayerBase::of_stored` だけが使う）
    pub(crate) fn dense_slot(&self) -> &SyncCache<Weak<LayerBase>> {
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

    /// 線を乗せた辺と、その計算に使う地図（`to_dense`）での番号（番号の順。線の図形を作る：`LayerBase::lines`）
    pub(crate) fn stroked_edges(&self) -> Vec<(usize, TileEdge)> {
        self.0
            .tiles
            .iter()
            .flat_map(|t| t.iter())
            .enumerate()
            .filter(|(_, se)| se.stroke.is_some())
            .map(|(i, se)| (i, se.tile_edge()))
            .collect()
    }

    /// 計算に使う地図（`to_dense`）の辺 `e` を、塊から引く（両端は座標で。向きはしまう辺の向きで、`to_dense` の辺と同じ。
    /// 写しの通し番号の一覧を作らずに辺を引く：設計 段2b の 7. の 2b-7a）
    pub(crate) fn edge_at(&self, e: usize) -> TileEdge {
        let starts = self.starts();
        let k = starts.partition_point(|&s| s as usize <= e) - 1;
        self.0.tiles[k][e - starts[k] as usize].tile_edge()
    }

    /// 計算に使う地図（`to_dense`）の辺の数
    pub(crate) fn edge_count(&self) -> usize {
        self.0.tiles.iter().map(|t| t.len()).sum()
    }

    /// `hold`（この地図の判定の形を生かしておく所）の、塊ごとの置き場
    fn hold_slots<'h>(&self, hold: &'h PolyHold) -> &'h [OnceLock<TilePolys>] {
        let slots = hold
            .0
            .get_or_init(|| (0..self.0.tiles.len()).map(|_| OnceLock::new()).collect());
        debug_assert_eq!(slots.len(), self.0.tiles.len(), "別の地図の置き場");
        slots
    }

    /// 計算に使う地図（`to_dense`）の辺 `e` の判定の形。辺の住む塊の判定の形を、誰かが生かしていればそれを、無ければ
    /// 作って、`hold`（この地図の判定の形を生かしておく所）に入れる
    pub(crate) fn poly<'h>(&self, e: usize, hold: &'h PolyHold) -> &'h Poly {
        let starts = self.starts();
        let k = starts.partition_point(|&s| s as usize <= e) - 1;
        let tile = self.hold_slots(hold)[k].get_or_init(|| self.0.tiles[k].polys());
        &tile[e - starts[k] as usize]
    }

    /// テスト用：判定の形を誰かが生かしている塊の数
    #[cfg(test)]
    pub(crate) fn tiles_with_live_polys(&self) -> usize {
        self.0
            .tiles
            .iter()
            .filter(|t| t.polys.lock().strong_count() > 0)
            .count()
    }

    /// テスト用：塊の数
    #[cfg(test)]
    pub(crate) fn tile_count(&self) -> usize {
        self.0.tiles.len()
    }

    /// `hold`（この地図の判定の形を生かしておく所）を手放す。塊の判定の形を生かしているのが `hold` だけなら、塊の弱い参照も
    /// 消す（弱い参照は、生かす所が無くなったあとも小さな領域を残すので、一度引いた塊ごとに残り続けないように）。生かす
    /// 所が増えるのは塊の鍵をかけて引くときだけなので、鍵をかけて数えれば、ほかの所が使っている判定の形は消さない
    pub(crate) fn release_polys(&self, hold: &mut PolyHold) {
        let Some(slots) = hold.0.take() else {
            return;
        };
        for (t, slot) in self.0.tiles.iter().zip(slots.into_vec()) {
            let Some(p) = slot.into_inner() else {
                continue;
            };
            let mut weak = t.polys.lock();
            if Arc::strong_count(&p) == 1 && Weak::ptr_eq(&weak, &Arc::downgrade(&p)) {
                *weak = Weak::new();
            }
        }
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

    /// 計算に使う地図の辺 `e` のしまう辺
    fn stored_edge(&self, e: usize) -> &StoredEdge {
        let starts = self.starts();
        let k = starts.partition_point(|&s| s as usize <= e) - 1;
        &self.0.tiles[k][e - starts[k] as usize]
    }

    /// 窓で重ねた結果の差分（設計 段2b の 4. の 2b-2）を、前の地図の一覧を使わずに塊から：窓の辺 `window`（計算に使う地図の
    /// 番号）と結果 `result` を、しまう辺（決まった向き・中身まで）で比べて（消えた辺, 増えた結果の辺）。窓の外の辺は変わらない
    pub(crate) fn window_diff_tiles(
        &self,
        window: &[usize],
        result: &PlanarMap,
    ) -> (Vec<usize>, Vec<usize>) {
        let olds: crate::planar_map::IntSet<StoredEdge> =
            window.iter().map(|&e| *self.stored_edge(e)).collect();
        let news: crate::planar_map::IntSet<StoredEdge> = result
            .edges
            .iter()
            .map(|ed| StoredEdge::of_tile(&tile_edge(result, ed)))
            .collect();
        let removed = window
            .iter()
            .copied()
            .filter(|&e| !news.contains(self.stored_edge(e)))
            .collect();
        let added = (0..result.edges.len())
            .filter(|&e| !olds.contains(&StoredEdge::of_tile(&tile_edge(result, &result.edges[e]))))
            .collect();
        (removed, added)
    }

    /// 形が `q` の辺（向きが逆でもよい）の、計算に使う地図での番号と、`q` と同じ向きか。辺は弦の中点の升目の塊にあるので、
    /// その塊の中だけを探す
    pub(crate) fn find_edge(&self, q: &QuadBez) -> Option<(usize, bool)> {
        let (a, b) = if point_order(q.p0, q.p2).is_le() {
            (q.p0, q.p2)
        } else {
            (q.p2, q.p0)
        };
        let probe = StoredEdge {
            a,
            b,
            ctrl: q.p1,
            left: None,
            right: None,
            left_src: None,
            right_src: None,
            stroke: None,
            line_id: None,
        };
        let k = self
            .0
            .tiles
            .binary_search_by_key(&probe.tile(), |t| t[0].tile())
            .ok()?;
        let j = self.0.tiles[k]
            .iter()
            .position(|se| se.a == a && se.b == b && se.ctrl == q.p1)?;
        Some((self.starts()[k] as usize + j, q.p0 == a))
    }

    /// この地図から地図 `other` への差分：消えた辺（この地図の、計算に使う地図での番号）と増えた辺。2つの地図が共有する
    /// 塊（同じ升目で同じ塊）は比べない（手間は、共有しない塊の分。`other` がこの地図に差分を当てて作ったものなら、変わった
    /// 塊の分）。共有しない塊どうしは、中身が同じ辺を除く
    pub(crate) fn diff_tiles(&self, other: &StoredMap) -> (Vec<usize>, Vec<TileEdge>) {
        let (a, b) = (&self.0.tiles, &other.0.tiles);
        let starts = self.starts();
        let mut removed: Vec<usize> = Vec::new();
        let mut added: Vec<TileEdge> = Vec::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.len() || j < b.len() {
            let ka = a.get(i).map(|t| t[0].tile());
            let kb = b.get(j).map(|t| t[0].tile());
            match (ka, kb) {
                (Some(x), Some(y)) if x == y => {
                    if !Arc::ptr_eq(&a[i], &b[j]) {
                        let olds: crate::planar_map::IntSet<&StoredEdge> = a[i].iter().collect();
                        let news: crate::planar_map::IntSet<&StoredEdge> = b[j].iter().collect();
                        for (k, se) in a[i].iter().enumerate() {
                            if !news.contains(se) {
                                removed.push(starts[i] as usize + k);
                            }
                        }
                        added.extend(
                            b[j].iter()
                                .filter(|se| !olds.contains(se))
                                .map(StoredEdge::tile_edge),
                        );
                    }
                    i += 1;
                    j += 1;
                }
                (Some(x), Some(y)) if x < y => {
                    removed.extend((0..a[i].len()).map(|k| starts[i] as usize + k));
                    i += 1;
                }
                (Some(_), None) => {
                    removed.extend((0..a[i].len()).map(|k| starts[i] as usize + k));
                    i += 1;
                }
                _ => {
                    added.extend(b[j].iter().map(StoredEdge::tile_edge));
                    j += 1;
                }
            }
        }
        (removed, added)
    }

    /// この地図に差分を当てた地図：計算に使う地図の辺 `removed` を消し、塊の辺の形の辺 `added` を足す。作り直すのは、消す辺か
    /// 足す辺のある塊だけで、ほかの塊はこの地図と共有する（塊の順・塊の中の順は `from_dense` と同じ）
    pub(crate) fn with_diff_tiles(&self, removed: &[usize], added: &[TileEdge]) -> Self {
        let starts = self.starts();
        let tiles = &self.0.tiles;
        let mut gone: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
        for &e in removed {
            let k = starts.partition_point(|&s| s as usize <= e) - 1;
            gone.entry(k).or_default().push(e - starts[k] as usize);
        }
        let mut redo: std::collections::BTreeMap<(i32, i32), Vec<StoredEdge>> = Default::default();
        for (&k, js) in &gone {
            redo.entry(tiles[k][0].tile()).or_default().extend(
                tiles[k]
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| !js.contains(j))
                    .map(|(_, e)| *e),
            );
        }
        for te in added {
            let se = StoredEdge::of_tile(te);
            let at = se.tile();
            redo.entry(at)
                .or_insert_with(|| match tiles.binary_search_by_key(&at, |t| t[0].tile()) {
                    Ok(k) => tiles[k].to_vec(),
                    Err(_) => Vec::new(),
                })
                .push(se);
        }
        let mut out: Vec<Tile> = Vec::with_capacity(tiles.len() + redo.len());
        let push = |mut list: Vec<StoredEdge>, out: &mut Vec<Tile>| {
            if !list.is_empty() {
                list.sort_unstable_by(|x, y| x.order(y));
                out.push(TileData::new(list.into()));
            }
        };
        let mut fresh = redo.into_iter().peekable();
        let mut i = 0;
        loop {
            match (tiles.get(i), fresh.peek()) {
                (Some(t), Some((c, _))) if t[0].tile() < *c => {
                    out.push(t.clone());
                    i += 1;
                }
                (Some(t), Some(_)) => {
                    let (c, list) = fresh.next().expect("見た");
                    if t[0].tile() == c {
                        i += 1;
                    }
                    push(list, &mut out);
                }
                (Some(t), None) => {
                    out.push(t.clone());
                    i += 1;
                }
                (None, Some(_)) => {
                    let (_, list) = fresh.next().expect("見た");
                    push(list, &mut out);
                }
                (None, None) => break,
            }
        }
        Self::of_tiles(out.into())
    }

    /// 差分（消す辺 `removed`：計算に使う地図の番号、足す辺 `added`）を当てると、中身が変わるか（消す辺と足す辺が、
    /// 中身まで同じ辺の集まりでなければ変わる）。手間は差分の大きさの分
    pub(crate) fn diff_changes(&self, removed: &[usize], added: &[TileEdge]) -> bool {
        if removed.len() != added.len() {
            return true;
        }
        let mut count: std::collections::HashMap<StoredEdge, i64> = Default::default();
        for &e in removed {
            *count.entry(*self.stored_edge(e)).or_default() += 1;
        }
        for te in added {
            *count.entry(StoredEdge::of_tile(te)).or_default() -= 1;
        }
        count.values().any(|&n| n != 0)
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
            dense: SyncCache::new(Weak::new()),
            starts: OnceLock::new(),
        }))
    }

    /// 側の図形の番号を `renew` で替えた地図（番号の無い側は無いまま）。形と色は変えず、辺の並びも同じなので、正しい地図は
    /// 正しいまま（確かめ直さない）。番号の無い塊は、この地図と共有する。番号のある塊は、作り直した塊の控え `done`（同じ
    /// `renew` で替える間だけ使い回す）にあればそれを使う
    pub(crate) fn relabeled(
        &self,
        mut renew: impl FnMut(ShapeId) -> ShapeId,
        done: &mut Relabeled,
    ) -> Self {
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
                // 形は変わらないので、判定の形は前の塊のものを使う
                let key = Arc::as_ptr(t) as *const u8 as usize;
                let made = done.0.entry(key).or_insert_with(|| {
                    let edges = t
                        .iter()
                        .map(|e| StoredEdge {
                            left_src: new_src(e.left_src),
                            right_src: new_src(e.right_src),
                            ..*e
                        })
                        .collect();
                    let polys = Mutex::new(t.polys.lock().clone());
                    (t.clone(), Arc::new(TileData { edges, polys }))
                });
                made.1.clone()
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
        let theirs: crate::planar_map::IntSet<usize> = other
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
type IndexCell = ((i64, i64), Box<[EdgeRef]>);

/// 升目の索引：索引の升目（近さを調べる格子のマス 4×4）ごとの、その升目にかかる辺（判定の形の近くを探すマス
/// `EdgeGeom::reach` が、その升目にかかる辺：`LayerBase::edge_tiles`）。近くの辺を探す所（重ね合わせの窓・
/// カーソル合わせ）が、毎回全部の辺を格子に入れ直さずに引く。持つのは計算に使う写し（`LayerBase::index`）で、地図を
/// 書き換えたときは、前の写しの索引から、変わった塊の辺がかかる升目だけを作り直す（`derive`）。設計：
/// claude/触った所だけを計算する地図の設計_22fbbbe.md の 3.3
#[derive(Debug, Default)]
pub(crate) struct TileIndex {
    /// 索引の升目の順の（升目, その升目にかかる辺。辺の指し方の順）。塊に分けて持ち、作り直すときは、作り直す升目の
    /// かかる塊だけを作り直し、ほかの塊は前の索引と共有する（`ChunkedList::replace_sorted`。設計 段2b の 15.）
    cells: crate::chunked_list::ChunkedList<IndexCell>,
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
            .map(|run| Arc::new((run[0].0, run.iter().map(|x| x.1).collect())))
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
        let gone_homes: crate::planar_map::IntSet<(i32, i32)> =
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
        let mut rebuilt: Vec<Arc<IndexCell>> = Vec::with_capacity(touched.len());
        let mut a = 0;
        for &c in &touched {
            let mut list: Vec<EdgeRef> = match self.cells.find_sorted(c, |x| x.0) {
                Some(cell) => cell
                    .1
                    .iter()
                    .copied()
                    .filter(|r| !gone_homes.contains(&r.0))
                    .collect(),
                None => Vec::new(),
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
                rebuilt.push(Arc::new((c, list.into())));
            }
        }
        // 作り直した升目を差し替える（作り直して空になった升目は消える）。ほかの升目の塊は前の索引と共有する
        let mut cells = self.cells.clone();
        cells.replace_sorted(&touched, rebuilt, |x| x.0);
        Self { cells }
    }

    /// 索引の升目 `tile` にかかる辺の、地図 `map`（この索引の地図）の計算に使う地図での番号を `out` に足す
    pub(crate) fn edges_in(&self, map: &StoredMap, tile: (i64, i64), out: &mut Vec<usize>) {
        if let Some(cell) = self.cells.find_sorted(tile, |x| x.0) {
            out.extend(
                cell.1
                    .iter()
                    .filter_map(|&(home, j)| map.dense_edge(home, j)),
            );
        }
    }

    /// 索引の升目の矩形 `lo`..=`hi`（両端を含む）の中の、辺の入っている升目ごとに、その升目と、その升目にかかる辺の地図
    /// `map` の計算に使う地図での番号を `f` に渡す（升目の順）。辺の入っている升目だけを、x の列ごとに y の範囲の始めから二分探索で
    /// 引くので、手間は矩形の中の辺の入っている升目と列の数で決まり、矩形の広さによらない（レビュー 96fdc26 の N-1004）。
    /// 矩形で索引を引くのはこれ1つ
    pub(crate) fn edges_over(
        &self,
        map: &StoredMap,
        lo: (i64, i64),
        hi: (i64, i64),
        mut f: impl FnMut((i64, i64), &mut dyn Iterator<Item = usize>),
    ) {
        let mut from = lo;
        'seek: loop {
            for cell in self.cells.sorted_from(&from, |x| x.0) {
                let (x, y) = cell.0;
                if x > hi.0 {
                    return;
                }
                if y < lo.1 {
                    from = (x, lo.1);
                    continue 'seek;
                }
                if y > hi.1 {
                    from = (x + 1, lo.1);
                    continue 'seek;
                }
                f(
                    cell.0,
                    &mut cell
                        .1
                        .iter()
                        .filter_map(|&(home, j)| map.dense_edge(home, j)),
                );
            }
            return;
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

/// 線を乗せた辺の線の保存の形（版 9 から。塊の表の最後の、文書に1つの線の並びに書く。線の無い作品では空の並びだけなので、
/// ファイルの大きさも読む手間も変わらない）：塊（塊の表の番号）、辺（その塊の辺の並びの番号）、線の色（文書の色の表の
/// 1 からの番号）、太さ（`STROKE_WIDTHS`）
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct SavedStroke {
    pub(crate) tile: u32,
    pub(crate) edge: u32,
    pub(crate) color: u32,
    pub(crate) width: u8,
    /// 線の番号の印（版 10 から。側の図形の印と同じ、文書の中の 1 からの通し番号。読むときに新しい線の番号に替える）
    pub(crate) id: u64,
}

/// しまう辺の保存の形：両端（塊の点の番号）、制御点の座標、左右の色（文書の色の表の 1 からの番号。0 は何も無い）、左右の
/// 側の図形の印（文書の中の 1 からの通し番号で、同じ印は同じ図形。0 は無い。読むときに新しい図形の番号に替える：
/// `WriteTiles`・`TileReader`）。辺の線は、塊の表の線の並び（`SavedStroke`）に書く
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

/// 書く塊の表：地図の塊（文書の塊そのもの。写さない）を番号の順に並べたものと、色の表。書くときは、ファイルの
/// 塊の表の並び（色の表 → 塊の表）で、塊ごとの点の表と辺をその場で作って流す（塊の表の写しを丸ごと
/// 持たないので、保存の一時領域は塊の数に比例する番号の表だけ：レビュー 983236e の N-5001）。同じ塊（同じ `Arc`）は
/// 1回だけ書くので、キーフレームや Undo の控えで共有している塊は、ファイルでも共有したまま（ファイルの大きさが変わった
/// 塊の量に見合う：レビュー f01d518 の N-5001）
#[derive(Clone, Default)]
pub(crate) struct WriteTiles {
    /// 塊の番地から、塊の番号
    index: IntMap<usize, u32>,
    /// 番号の順の塊
    tiles: Vec<Tile>,
    /// 色から、色の番号（1 から。初めて出てきた順）と、番号の順の色
    color_no: std::collections::HashMap<[u8; 4], u32>,
    colors: Vec<[u8; 4]>,
    /// 図形の番号から、書く印（1 からの通し番号。初めて出てきた順）
    labels: IntMap<ShapeId, u64>,
    /// 線の並び（塊の順、塊の中は辺の順。塊を表に足すときに、色を数えるのと同じなめ方で拾う）
    strokes: Vec<SavedStroke>,
}

impl WriteTiles {
    /// 地図 `map` の塊の番号の並び（まだ並べていない塊は表に足し、その辺の色と図形の印に、出てきた順に番号を振る）
    pub(crate) fn tiles_of(&mut self, map: &StoredMap) -> Vec<u32> {
        map.0
            .tiles
            .iter()
            .map(|t| {
                let key = Arc::as_ptr(t) as *const u8 as usize;
                if let Some(&i) = self.index.get(&key) {
                    return i;
                }
                let i = self.tiles.len() as u32;
                for (ei, e) in t.iter().enumerate() {
                    let stroke = e.stroke.map(|s| s.color);
                    for c in [e.left, e.right, stroke].into_iter().flatten() {
                        let next = self.colors.len() as u32 + 1;
                        if *self.color_no.entry(c.to_array()).or_insert(next) == next {
                            self.colors.push(c.to_array());
                        }
                    }
                    for id in [e.left_src, e.right_src].into_iter().flatten() {
                        let next = self.labels.len() as u64 + 1;
                        self.labels.entry(id.get()).or_insert(next);
                    }
                    if let (Some(st), Some(line_id)) = (e.stroke, e.line_id) {
                        let next = self.labels.len() as u64 + 1;
                        let id = *self.labels.entry(line_id.get()).or_insert(next);
                        self.strokes.push(SavedStroke {
                            tile: i,
                            edge: ei as u32,
                            color: self.color_no[&st.color.to_array()],
                            width: st.width,
                            id,
                        });
                    }
                }
                self.tiles.push(t.clone());
                self.index.insert(key, i);
                i
            })
            .collect()
    }

    /// 塊 `t` の保存の形（書く1つだけを作る）
    fn saved_tile(&self, t: &TileData) -> SavedTile {
        let mut tile = SavedTile::default();
        let mut node_of: IntMap<[u64; 2], u32> = IntMap::default();
        let color = |c: Option<Color32>| c.map_or(0, |c| self.color_no[&c.to_array()]);
        let label = |id: Option<NonZeroU64>| id.map_or(0, |id| self.labels[&id.get()]);
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
            tile.edges.push(SavedEdge {
                a,
                b,
                ctrl: [e.ctrl.x, e.ctrl.y],
                left: color(e.left),
                right: color(e.right),
                left_src: label(e.left_src),
                right_src: label(e.right_src),
            });
        }
        tile
    }
}

/// ファイルの塊の表の並びで書く：色の表と、塊の表と、線の並び（塊の順、塊の中は辺の順）
impl Serialize for WriteTiles {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        struct Tiles<'a>(&'a WriteTiles);
        impl Serialize for Tiles<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_seq(self.0.tiles.iter().map(|t| self.0.saved_tile(t)))
            }
        }
        let mut st = s.serialize_struct("SavedTiles", 3)?;
        st.serialize_field("colors", &self.colors)?;
        st.serialize_field("tiles", &Tiles(self))?;
        st.serialize_field("strokes", &self.strokes)?;
        st.end()
    }
}

/// 読んだ塊の表：ファイルの塊の表（色の表 → 塊の表）を読みながら、塊ごとにしまう形の辺の並びにしたもの（保存の形の
/// 塊の表を丸ごと持たない：レビュー e90f27c の N-5002）。postcard が書いた並びの順に読む。辺の側の図形の印は、まだファイル
/// の印のまま（新しい図形の番号に替えるのは、ファイルが壊れていないと確かめたあと：`TileReader::new`）。読みながらは断ら
/// ない：色・点の番号が表の外の塊、辺の無い塊があれば `broken` に、有限でない座標があれば `non_finite` に記す
#[derive(Default)]
pub(crate) struct ReadTiles {
    colors: Vec<[u8; 4]>,
    tiles: Vec<Box<[StoredEdge]>>,
    /// 辺の側の数と、いちばん大きい印
    sides: u64,
    labels: u64,
    broken: bool,
    non_finite: bool,
}

impl ReadTiles {
    /// 座標（辺の端の点・制御点）がみな有限か
    pub(crate) fn only_finite_numbers(&self) -> bool {
        !self.non_finite
    }

    /// 読んだ塊 `t` を、しまう形の辺の並びにして足す
    fn push(&mut self, t: SavedTile) {
        self.non_finite |= !(t.nodes.iter().flatten().all(|v| v.is_finite())
            && t.edges.iter().all(|e| e.ctrl.iter().all(|v| v.is_finite())));
        self.sides += 2 * t.edges.len() as u64;
        let labels = t.edges.iter().map(|e| e.left_src.max(e.right_src)).max();
        self.labels = self.labels.max(labels.unwrap_or(0));
        let color = |i: u32| -> Option<Option<Color32>> {
            match i {
                0 => Some(None),
                _ => self
                    .colors
                    .get(i as usize - 1)
                    .map(|c| Some(Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3]))),
            }
        };
        let node = |i: u32| t.nodes.get(i as usize).map(|p| Point::new(p[0], p[1]));
        let edge = |e: &SavedEdge| {
            Some(StoredEdge {
                a: node(e.a)?,
                b: node(e.b)?,
                ctrl: Point::new(e.ctrl[0], e.ctrl[1]),
                left: color(e.left)?,
                right: color(e.right)?,
                left_src: NonZeroU64::new(e.left_src),
                right_src: NonZeroU64::new(e.right_src),
                stroke: None,
                line_id: None,
            })
        };
        // 塊の辺の並びは、ちょうどの大きさで1回だけ取る
        let mut edges = Vec::with_capacity(t.edges.len());
        edges.extend(t.edges.iter().map_while(edge));
        if edges.is_empty() || edges.len() < t.edges.len() {
            self.broken = true;
        } else {
            self.tiles.push(edges.into_boxed_slice());
        }
    }

    /// 線の並びの線を、読んだ塊の辺に乗せる。塊・辺の番号が表の内、色が表の内（線なしの 0 でない）、太さが範囲の内で、
    /// 同じ辺に2回乗せないこと。そうでない線があれば `broken` に記す（壊れた塊があれば、塊の番号がずれるので乗せない）
    fn put_strokes(&mut self, strokes: &[SavedStroke]) {
        if self.broken {
            return;
        }
        for s in strokes {
            let color = match s.color {
                0 => None,
                i => self.colors.get(i as usize - 1).copied(),
            };
            let stroke = color.filter(|_| STROKE_WIDTHS.contains(&s.width)).map(|c| EdgeStroke {
                color: Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3]),
                width: s.width,
            });
            let edge = self
                .tiles
                .get_mut(s.tile as usize)
                .and_then(|t| t.get_mut(s.edge as usize));
            // 線の番号の印も、側の図形の印と同じく数える（新しい番号に替えるのは `TileReader::new`）
            self.sides += 1;
            self.labels = self.labels.max(s.id);
            match (edge, stroke, NonZeroU64::new(s.id)) {
                (Some(e), Some(stroke), Some(id)) if e.stroke.is_none() => {
                    e.stroke = Some(stroke);
                    e.line_id = Some(id);
                }
                _ => {
                    self.broken = true;
                    return;
                }
            }
        }
    }
}

impl<'de> Deserialize<'de> for ReadTiles {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_struct(
            "SavedTiles",
            &["colors", "tiles", "strokes"],
            ReadTiles::default(),
        )
    }
}

/// 塊の表（色の表、塊の並び、線の並び）を読む
impl<'de> Visitor<'de> for ReadTiles {
    type Value = ReadTiles;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("塊の表")
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<ReadTiles, A::Error> {
        let expected = &"色の表と塊の並びと線の並び";
        self.colors = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, expected))?;
        seq.next_element_seed(&mut self)?
            .ok_or_else(|| de::Error::invalid_length(1, expected))?;
        let strokes: Vec<SavedStroke> = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(2, expected))?;
        self.put_strokes(&strokes);
        Ok(self)
    }
}

/// 塊の並びを、1つずつ読んでしまう形にする（保存の形の塊は、読んでいる1つだけ持つ）
impl<'de> DeserializeSeed<'de> for &mut ReadTiles {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for &mut ReadTiles {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("塊の並び")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(t) = seq.next_element::<SavedTile>()? {
            self.push(t);
        }
        Ok(())
    }
}

/// 読んだ塊の表の塊。地図は塊の番号の並びから作り、同じ番号の塊は共有する（ファイルで共有していた塊は、読んでも共有した
/// まま。見る順によらない）
pub(crate) struct TileReader {
    tiles: Vec<Tile>,
}

impl TileReader {
    /// 読んだ塊の表 `read` の塊。表の外の番号・辺の無い塊があれば `None`（壊れたファイル）。辺の側の図形の印（1 からの
    /// 通し番号：`WriteTiles`）は、続けて取った新しい図形の番号に替える（同じ印は同じ番号。ファイルに書いた数がそのまま
    /// 番号になることは無いので、細工したファイルでも、これから振る番号が今ある番号と重ならない）。印が辺の側の数より
    /// 大きければ（アプリは書かない）`None`
    pub(crate) fn new(read: ReadTiles) -> Option<Self> {
        if read.broken || read.labels > read.sides {
            return None;
        }
        let first = crate::vector_engine::new_shape_ids(read.labels);
        let label = |n: Option<NonZeroU64>| n.and_then(|n| NonZeroU64::new(first + n.get() - 1));
        let tiles = read
            .tiles
            .into_iter()
            .map(|mut edges| {
                for e in edges.iter_mut() {
                    e.left_src = label(e.left_src);
                    e.right_src = label(e.right_src);
                    e.line_id = label(e.line_id)
                        .and_then(|id| NonZeroU64::new(id.get() | crate::planar_map::LINE_ID_BIT));
                }
                TileData::new(edges)
            })
            .collect();
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
