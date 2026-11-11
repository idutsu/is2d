//! セル画にしまう地図（境目の線を1本で持つ本当の形）：場所ごとの塊に分けた辺の並び。
//!
//! 辺は両端を座標で指し（頂点の番号を持たない）、決まった向き・決まった順に並べる。塊は、辺の弦の中点がある `TILE` px の
//! 升目ごとの辺の並び。中身が同じ塊はアプリ全体で1つを共有する（`TILES`）ので、編集で変わらなかった場所の塊は、Undo の
//! 控えとも共有したまま（控え1つ分の大きさは、変わった場所の塊の分だけ）。
//!
//! 計算に使うふつうの地図（`PlanarMap`）は、いつも塊から決まった順で作る（`StoredMap::to_dense`）。頂点と辺の番号が中身
//! だけで決まるので、しまい場所から出した地図も、塊から作り直した地図も同じになる（編集の結果が、しまい場所の具合に
//! よらない）。

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::sync::{Arc, Weak};

use eframe::egui::Color32;
use kurbo::Point;

use crate::canvas_tool::ShapeId;
use crate::planar_map::{MapEdge, MapKey, PlanarMap};
use crate::vector_engine::SyncCache;

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
        if coord_order(a, b).is_le() {
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
        coord_order(self.a, other.a)
            .then_with(|| coord_order(self.b, other.b))
            .then_with(|| coord_order(self.ctrl, other.ctrl))
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

impl Hash for StoredEdge {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.geom_bits().hash(h);
        (self.left, self.right, self.left_src, self.right_src).hash(h);
    }
}

/// テスト用：しまう辺1本の大きさ（バイト）
#[cfg(test)]
pub(crate) fn edge_size() -> usize {
    std::mem::size_of::<StoredEdge>()
}

/// 点の、座標の小さい順（x、同じなら y）
fn coord_order(p: Point, q: Point) -> std::cmp::Ordering {
    p.x.total_cmp(&q.x).then_with(|| p.y.total_cmp(&q.y))
}

/// 塊：1つの升目に入る辺の並び（決まった順）
type Tile = Arc<[StoredEdge]>;

/// セル画にしまう地図：塊の並び（升目の順）。写しは `Arc` を1つ数えるだけ
#[derive(Clone, Debug)]
pub(crate) struct StoredMap(Arc<[Tile]>);

impl StoredMap {
    /// 地図 `map` のしまう形。辺を決まった向きにして升目ごとに分け、塊の中を決まった順に並べる。中身が前からある塊と
    /// 同じ塊は、その塊を共有する（`TILES`）
    pub(crate) fn from_dense(map: &PlanarMap) -> Self {
        let mut all: Vec<((i32, i32), StoredEdge)> = map
            .edges
            .iter()
            .map(|e| {
                let se = StoredEdge::of(map, e);
                (se.tile(), se)
            })
            .collect();
        all.sort_unstable_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.order(&y.1)));
        let mut table = TILES.lock();
        let tiles: Vec<Tile> = all
            .chunk_by(|x, y| x.0 == y.0)
            .map(|run| table.intern(run.iter().map(|x| x.1).collect()))
            .collect();
        Self(tiles.into())
    }

    /// 計算に使うふつうの地図：辺は塊の順・塊の中の順に並べ、頂点は最初に出てきた順に番号を振る（番号が中身だけで決まる）
    pub(crate) fn to_dense(&self) -> PlanarMap {
        let n: usize = self.0.iter().map(|t| t.len()).sum();
        let mut nodes: Vec<Point> = Vec::with_capacity(n);
        let mut index: std::collections::HashMap<[u64; 2], usize> =
            std::collections::HashMap::with_capacity(n);
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
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// 中身が同じか（塊と辺が、ビットまで同じ）
    #[cfg(test)]
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .zip(other.0.iter())
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
            bytes.0 += self.0.len() * std::mem::size_of::<Tile>();
        }
        for t in self.0.iter() {
            if seen.insert(Arc::as_ptr(t) as *const u8 as usize) {
                bytes.1 += t.len() * std::mem::size_of::<StoredEdge>();
            }
        }
        bytes
    }

    /// テスト用：塊の数
    #[cfg(test)]
    pub(crate) fn tiles(&self) -> usize {
        self.0.len()
    }

    /// テスト用：`other` と共有している塊（同じ `Arc`）の数
    #[cfg(test)]
    pub(crate) fn shared_tiles(&self, other: &Self) -> usize {
        let theirs: std::collections::HashSet<usize> = other
            .0
            .iter()
            .map(|t| Arc::as_ptr(t) as *const u8 as usize)
            .collect();
        self.0
            .iter()
            .filter(|t| theirs.contains(&(Arc::as_ptr(t) as *const u8 as usize)))
            .count()
    }

    fn downgrade(&self) -> Weak<[Tile]> {
        Arc::downgrade(&self.0)
    }
}

/// 塊の置き場：中身のハッシュごとの、生きている塊（弱い参照）。持ち主（セル画・Undo の控え・地図のしまい場所）がみな
/// 手放した塊は消え、置き場からも、入れた数が生きている数を超えたときにまとめて外す
struct TileTable {
    by_hash: BTreeMap<u64, Vec<Weak<[StoredEdge]>>>,
    /// 前にまとめて外してから入れた数
    inserted: usize,
}

static TILES: SyncCache<TileTable> = SyncCache::new(TileTable {
    by_hash: BTreeMap::new(),
    inserted: 0,
});

impl TileTable {
    /// 辺の並び `edges` の塊。中身が同じ生きている塊があれば、それを返す
    fn intern(&mut self, edges: Vec<StoredEdge>) -> Arc<[StoredEdge]> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        edges.hash(&mut h);
        let bucket = self.by_hash.entry(h.finish()).or_default();
        bucket.retain(|w| w.strong_count() > 0);
        if let Some(t) = bucket
            .iter()
            .filter_map(Weak::upgrade)
            .find(|t| t[..] == edges[..])
        {
            return t;
        }
        let t: Arc<[StoredEdge]> = edges.into();
        bucket.push(Arc::downgrade(&t));
        self.inserted += 1;
        if self.inserted > self.by_hash.len().max(1024) {
            self.by_hash.retain(|_, b| {
                b.retain(|w| w.strong_count() > 0);
                !b.is_empty()
            });
            self.inserted = 0;
        }
        t
    }
}

/// 本当の地図の索引：鍵（図形の並び。`MapKey`）ごとの、生きているしまった地図（弱い参照）。鍵が同じなら地図も同じ
/// （鍵の図形は、その地図の面そのもの）なので、生きている地図は鍵ごとに1つにする（`StoredMaps::intern`）
pub(crate) struct StoredMaps {
    by_key: BTreeMap<MapKey, Weak<[Tile]>>,
    inserted: usize,
}

impl StoredMaps {
    pub(crate) const fn new() -> Self {
        Self {
            by_key: BTreeMap::new(),
            inserted: 0,
        }
    }

    /// 鍵 `key` の地図として `stored` を引けるようにする。同じ鍵の生きている地図があれば、それを返す
    pub(crate) fn intern(&mut self, key: &MapKey, stored: StoredMap) -> StoredMap {
        if let Some(live) = self.find(key) {
            return live;
        }
        self.by_key.insert(key.clone(), stored.downgrade());
        self.inserted += 1;
        if self.inserted > self.by_key.len().max(256) {
            self.by_key.retain(|_, w| w.strong_count() > 0);
            self.inserted = 0;
        }
        stored
    }

    /// 鍵 `key` の、生きている地図
    pub(crate) fn find(&self, key: &MapKey) -> Option<StoredMap> {
        self.by_key.get(key)?.upgrade().map(StoredMap)
    }
}
