use crate::canvas_tool::ShapeId;
use crate::planar_map::{realized, CheckedMap, LayerBase, MapInput, MapRule, PlanarMap};
use crate::stored_map::StoredMap;
use crate::vector_engine::{FillShape, SymbolInstanceBinding};
use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// タイムラインおよびシンボル内タイムラインの最大許容フレーム数（破損JSONによる巨大ループ防止）
pub(crate) const MAX_TIMELINE_FRAMES: usize = 100_000;

/// 1タイムラインあたりの最大許容レイヤー数
pub(crate) const MAX_LAYERS_PER_TIMELINE: usize = 1_000;

/// クラシックトゥイーンの回転方向（Adobe Animate のトゥイーン回転設定相当）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub(crate) enum TweenRotate {
    /// 自動：開始角度から終了角度へ最短の角度差（`-180°..=+180°`）で回転する
    #[default]
    Auto,
    /// 時計回り (CW)：時計回り（右回り）方向に回転する
    Clockwise,
    /// 反時計回り (CCW)：反時計回り（左回り）方向に回転する
    CounterClockwise,
}

/// セル（`Cel`）内に正規化して保持されるシンボルインスタンスエンティティ
/// （構成図形が0個の空白フレームや空シンボルであってもインスタンスの存在・配置情報を独立して保持し、展開図形は持たない）
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolInstance {
    pub(crate) binding: SymbolInstanceBinding,
    pub(crate) layer_id: u64,
    /// このインスタンスが属する外側の親グループ階層パス（ルート配置なら空 `[]`）
    pub(crate) parent_scope: Vec<u64>,
    /// 重なり位置：`Cel::maps`（階層の地図の並び）の何番目の手前に描くか
    pub(crate) order: usize,
}

/// キーフレーム区間におけるクラシックトゥイーン（`T` キー）の有効状態
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TweenState {
    /// トゥイーン未設定（または開始キーフレームが単一シンボル条件を満たしていない）
    None,
    /// 開始キーフレームは単一シンボルでトゥイーンONだが、終了キーフレームが存在しない（または条件不一致）→ 横点線（`----`）表示
    Incomplete,
    /// 開始・終了キーフレームとも同一シンボルの単一インスタンスが存在する完全トゥイーン → 実線矢印（`───▶`）表示＆TRS補間
    Complete {
        start_frame: usize,
        end_frame: usize,
        start_binding: SymbolInstanceBinding,
        end_binding: SymbolInstanceBinding,
        rotate_mode: TweenRotate,
    },
}

/// シンボル展開やクラシックトゥイーン補間を含むセルの衝突ゼロ署名（土台キャッシュ・背景キャッシュ用）
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedCelSig {
    pub(crate) cel_revision: u64,
    pub(crate) start_frame: usize,
    pub(crate) tween: bool,
    pub(crate) tween_rotate: TweenRotate,
    pub(crate) next_kf: Option<(usize, SymbolInstanceBinding)>,
    pub(crate) referenced_symbols: Vec<(u64, u64)>,
}

/// 2つの2Dアフィン変換行列 `start` と `end` を、それぞれのローカル基準点（ピボット）`start_pivot`, `end_pivot` を軸として
/// TRS（ピボットのワールド平行移動・指定回転方向・スケール・シアー）に分解し、
/// パラメータ `t`（`0.0..=1.0`）で補間した `(補間後の Affine, 補間後のローカルピボット Point)` を返す（クラシックトゥイーン用）
pub(crate) fn interpolate_affine_with_pivot(
    start: Affine,
    start_pivot: Point,
    end: Affine,
    end_pivot: Point,
    rotate_mode: TweenRotate,
    t: f64,
) -> (Affine, Point) {
    let start_custom = crate::vector_engine::is_custom_pivot(start_pivot);
    let end_custom = crate::vector_engine::is_custom_pivot(end_pivot);
    let (p0, p1) = match (start_custom, end_custom) {
        (true, false) => (start_pivot, start_pivot),
        (false, true) => (end_pivot, end_pivot),
        _ => (start_pivot, end_pivot),
    };

    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return (start, p0);
    }
    if t >= 1.0 {
        return (end, p1);
    }

    let (rot0, sx0, sy0, sh0) = decompose_affine_trs(start);
    let (mut rot1, mut sx1, mut sy1, mut sh1) = decompose_affine_trs(end);
    // 開始と終了で行列式の符号（反転状態）が異なり、かつ双対表現 (-sx, -sy, -sh, rot + π) の方が
    // 回転角の差分が小さい場合（水平反転など）、不要な 180° 回転を防ぐため水平スケール反転表現を採用する
    if sy0 * sy1 < 0.0 {
        let diff_keep = (rot1 - rot0 + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        let diff_flip = (rot1 - rot0).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
        if diff_flip.abs() < diff_keep.abs() {
            sx1 = -sx1;
            sy1 = -sy1;
            sh1 = -sh1;
            rot1 += std::f64::consts::PI;
        }
    }

    let cw_delta = (rot1 - rot0).rem_euclid(std::f64::consts::TAU);
    let delta_rot = match rotate_mode {
        TweenRotate::Auto => {
            if cw_delta > std::f64::consts::PI {
                cw_delta - std::f64::consts::TAU
            } else {
                cw_delta
            }
        }
        TweenRotate::Clockwise => {
            if cw_delta < 1e-5 || (std::f64::consts::TAU - cw_delta) < 1e-5 {
                std::f64::consts::TAU
            } else {
                cw_delta
            }
        }
        TweenRotate::CounterClockwise => {
            if cw_delta < 1e-5 || (std::f64::consts::TAU - cw_delta) < 1e-5 {
                -std::f64::consts::TAU
            } else {
                cw_delta - std::f64::consts::TAU
            }
        }
    };
    let rot = rot0 + delta_rot * t;
    const MIN_TWEEN_SCALE: f64 = 1e-3;
    let mut sx = sx0 + (sx1 - sx0) * t;
    let mut sy = sy0 + (sy1 - sy0) * t;
    if sx.abs() < MIN_TWEEN_SCALE {
        sx = if sx0 >= 0.0 {
            MIN_TWEEN_SCALE
        } else {
            -MIN_TWEEN_SCALE
        };
    }
    if sy.abs() < MIN_TWEEN_SCALE {
        sy = if sy0 >= 0.0 {
            MIN_TWEEN_SCALE
        } else {
            -MIN_TWEEN_SCALE
        };
    }
    let sh = sh0 + (sh1 - sh0) * t;

    let (sin_r, cos_r) = rot.sin_cos();
    let a = sx * cos_r;
    let b = sx * sin_r;
    let c = sh * cos_r - sy * sin_r;
    let d = sh * sin_r + sy * cos_r;
    let linear = Affine::new([a, b, c, d, 0.0, 0.0]);

    let local_pivot = p0.lerp(p1, t);
    let world_pivot = (start * p0).lerp(end * p1, t);
    let interp_xform = Affine::translate(world_pivot.to_vec2())
        * linear
        * Affine::translate(-local_pivot.to_vec2());
    (interp_xform, local_pivot)
}

/// 2D アフィン変換から回転・拡大縮小・せん断 `(rot, sx, sy, shear)` を取り出す（平行移動はトゥイーンでピボットから作り直すので返さない）
fn decompose_affine_trs(affine: Affine) -> (f64, f64, f64, f64) {
    let [a, b, c, d, _, _] = affine.as_coeffs();
    let sx = a.hypot(b);
    let (cos_r, sin_r, rot) = if sx > 1e-9 {
        (a / sx, b / sx, b.atan2(a))
    } else {
        (1.0, 0.0, 0.0)
    };
    let shear = c * cos_r + d * sin_r;
    let sy = -c * sin_r + d * cos_r;
    (rot, sx, sy, shear)
}

// ============================================================================
// 1. タイムラインのデータモデル（Cel / Keyframe / Selection / Layer / Timeline）
// ============================================================================

/// 1枚のセル画：階層ごとの地図（境目の線を1本で持つ本当の形）とシンボルインスタンス。面の図形は持たない（要るときに
/// 地図から作る写し：`Cel::shapes`）。重なりの順は Adobe Animate と同じく、階層の生の図形がその階層のいちばん下で、
/// その上にグループ（入れ子の階層）とインスタンスが並ぶ
#[derive(Clone, Debug)]
pub(crate) struct Cel {
    /// 階層（グループの番号の列。ルートは空）ごとの生の図形の地図。下から順で、階層の地図は、その階層の中身（入れ子の
    /// 階層の地図・インスタンス）より前にある。どの地図にも面がある（面の無くなった階層は外す）
    pub(crate) maps: Arc<Vec<ScopeMap>>,
    /// シンボルインスタンス（`instance_gid` で重複なし、CoW `Arc` 共有）。重なりの順に並び、重なり位置 `order` は
    /// `maps` の何番目の手前に描くか
    pub(crate) instances: Arc<Vec<SymbolInstance>>,
    /// セル内容が更新されるたびにインクリメントされるリビジョン番号（Undo履歴の $O(1)$ CoW 共有用）
    pub(crate) revision: u64,
}

/// セル画の1つの階層の地図
#[derive(Clone, Debug)]
pub(crate) struct ScopeMap {
    pub(crate) group_path: Vec<u64>,
    /// 地図（場所ごとの塊。変わらない塊は Undo の控えと共有する）。面の辺の側に、面ごとに1つずつ別の図形の番号を記して
    /// ある（面の図形の番号になる）
    stored: StoredMap,
}

impl ScopeMap {
    /// 階層 `group_path` の、確かめた地図 `map` の地図（面に1つずつ別の番号を記してしまう：`CheckedMap::uniquely_labeled`）。
    /// 塊は `prev` と共有する。面が無ければ `None`。セル画の地図を作るのは、これと `restricted` と `renew_labels` だけ
    /// なので、セル画の地図はいつも正しい地図で、面に1つずつ別の番号が記してある
    fn new(group_path: Vec<u64>, map: CheckedMap, prev: Option<&StoredMap>) -> Option<ScopeMap> {
        let stored = StoredMap::from_dense(map.uniquely_labeled().map(), prev);
        (!stored.is_empty()).then_some(ScopeMap { group_path, stored })
    }

    /// 面の番号が `keep` に合う面だけを残した、階層 `group_path` の地図（`keep` が `None` なら全部の面：同じ地図）。正しい
    /// 地図の面を外しても正しい地図のまま（外した面の所は何も無い所になり、辺は減るだけで、残る面の輪郭と番号は変わら
    /// ない）なので、確かめ直さない。塊はこの地図と共有する。面が無くなれば `None`
    fn restricted(
        &self,
        group_path: &[u64],
        keep: Option<&dyn Fn(ShapeId) -> bool>,
    ) -> Option<ScopeMap> {
        let stored = match keep {
            None => self.stored.clone(),
            Some(keep) => {
                let base = self.base();
                if base.map.labels_within(keep) {
                    self.stored.clone()
                } else {
                    StoredMap::from_dense(&base.map.restricted_by(keep), Some(&self.stored))
                }
            }
        };
        (!stored.is_empty()).then(|| ScopeMap {
            group_path: group_path.to_vec(),
            stored,
        })
    }

    /// 面の番号を、みな新しい番号にする（`PlanarMap::renew_labels`。番号は面の形を変えないので、正しい地図のまま）
    fn renew_labels(&mut self) {
        let mut map = self.dense();
        map.renew_labels();
        self.stored = StoredMap::from_dense(&map, None);
    }

    /// 計算に使う写し（持ち主（画面の土台など）が持っていればそれ、無ければ塊から作る：`LayerBase::of_stored`）
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        LayerBase::of_stored(&self.stored)
    }

    /// 計算に使うふつうの地図（その場で作って捨てる。保存など、面の図形を使わない所）
    pub(crate) fn dense(&self) -> PlanarMap {
        self.stored.to_dense()
    }

    /// 面の図形（写し。面の順）
    pub(crate) fn faces(&self) -> Arc<[Arc<FillShape>]> {
        self.base().faces(&self.group_path)
    }

    /// しまった地図
    #[cfg(test)]
    pub(crate) fn stored(&self) -> &StoredMap {
        &self.stored
    }
}

/// 地図の編集（`Cel::edit_map`）の結果：書いた階層の新しい地図の、計算に使う写し（階層の面が無くなって外したときは
/// `None`）。写しを持ち続けるのは呼んだ側（画面の土台：`SceneBaseCache::hold`）
pub(crate) struct MapWritten(pub(crate) Option<Arc<LayerBase>>);

/// セル画の中身の1つ（`Cel::parts` が重なりの順に並べて返す）
#[derive(Clone, Debug)]
pub(crate) enum Part {
    /// 生の図形（文書の座標）
    Shape(Arc<FillShape>),
    /// インスタンス（重なり位置 `order` は並べるときに決める）
    Instance(SymbolInstance),
}

/// 地図にできない形（格子に置けない座標：±65,536px の外か、有限でない座標）があり、セル画に書けない
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Unmappable;

impl Default for Cel {
    fn default() -> Self {
        Self {
            maps: Arc::new(Vec::new()),
            instances: Arc::new(Vec::new()),
            revision: crate::vector_engine::next_mesh_revision(),
        }
    }
}

/// 並べる前の中身：階層の生の図形（ひとまとめにして地図にする）か、インスタンス
enum Item<T> {
    Raw(Vec<u64>, T),
    Instance(SymbolInstance),
}

impl<T> Item<T> {
    /// 中身のある階層（生の図形の階層か、インスタンスの親の階層）
    fn scope(&self) -> &[u64] {
        match self {
            Item::Raw(path, _) => path,
            Item::Instance(inst) => &inst.parent_scope,
        }
    }
}

/// 重なりの順の中身 `items` を、Adobe Animate の決まりに並べる：階層ごとに、その階層の生の図形をいちばん下に、その上に
/// グループ（入れ子の階層）とインスタンスを、出てきた順に置く。生の図形は階層ごとにまとめて `scope` で地図にする（`None`：
/// 面が無い）。戻り値は（階層の地図の並び（下から順）, インスタンス（重なりの順。重なり位置 `order` を付けたもの））
fn arranged<T, E>(
    items: Vec<Item<T>>,
    mut scope: impl FnMut(&[u64], Vec<T>) -> Result<Option<ScopeMap>, E>,
) -> Result<(Vec<ScopeMap>, Vec<SymbolInstance>), E> {
    struct Node<T> {
        raw: Vec<T>,
        children: Vec<Child>,
    }
    enum Child {
        Group(Vec<u64>),
        Instance(SymbolInstance),
    }
    fn ensure<T>(nodes: &mut HashMap<Vec<u64>, Node<T>>, path: &[u64]) {
        if nodes.contains_key(path) {
            return;
        }
        let parent = &path[..path.len() - 1];
        ensure(nodes, parent);
        if let Some(p) = nodes.get_mut(parent) {
            p.children.push(Child::Group(path.to_vec()));
        }
        nodes.insert(
            path.to_vec(),
            Node {
                raw: Vec::new(),
                children: Vec::new(),
            },
        );
    }
    let mut nodes: HashMap<Vec<u64>, Node<T>> = HashMap::new();
    nodes.insert(
        Vec::new(),
        Node {
            raw: Vec::new(),
            children: Vec::new(),
        },
    );
    for item in items {
        match item {
            Item::Raw(path, t) => {
                ensure(&mut nodes, &path);
                if let Some(n) = nodes.get_mut(&path) {
                    n.raw.push(t);
                }
            }
            Item::Instance(inst) => {
                ensure(&mut nodes, &inst.parent_scope);
                if let Some(n) = nodes.get_mut(&inst.parent_scope) {
                    n.children.push(Child::Instance(inst));
                }
            }
        }
    }
    let mut maps = Vec::new();
    let mut instances = Vec::new();
    let mut stack = vec![Child::Group(Vec::new())];
    while let Some(child) = stack.pop() {
        match child {
            Child::Instance(mut inst) => {
                inst.order = maps.len();
                instances.push(inst);
            }
            Child::Group(path) => {
                let Some(node) = nodes.remove(&path) else {
                    continue;
                };
                if !node.raw.is_empty() {
                    if let Some(m) = scope(&path, node.raw)? {
                        maps.push(m);
                    }
                }
                stack.extend(node.children.into_iter().rev());
            }
        }
    }
    Ok((maps, instances))
}

/// テスト用：階層 `path` の生の図形 `raw`（下から順。新しい図形でよい）を図形から作った地図（`LayerBase::of`。正しい地図に
/// ならない図形は塗り重ねて作る）。面に1つずつ別の番号を記す（図形の番号を使う）。塊は同じ階層の前の地図と共有する
#[cfg(test)]
fn test_scope_map_from_shapes(
    old: &[ScopeMap],
    path: &[u64],
    raw: Vec<Arc<FillShape>>,
) -> Result<Option<ScopeMap>, Unmappable> {
    let refs: Vec<&FillShape> = raw.iter().map(|s| &**s).collect();
    let base = LayerBase::of(&refs, Affine::IDENTITY).ok_or(Unmappable)?;
    let same_path = old.iter().find(|m| m.group_path == path);
    let map = base.map.clone().checked().ok_or(Unmappable)?;
    Ok(ScopeMap::new(
        path.to_vec(),
        map,
        same_path.map(|m| &m.stored),
    ))
}

impl Cel {
    /// テスト用：図形の並び `all_shapes`（下から順。インスタンスは図形の印）から作る（図形から地図を作る）。`Err`：地図に
    /// できない形がある
    #[cfg(test)]
    pub(crate) fn from_shapes(all_shapes: Vec<FillShape>) -> Result<Self, Unmappable> {
        let mut cel = Self::default();
        cel.set_new_parts(Self::parts_of(all_shapes))?;
        Ok(cel)
    }

    /// 階層ごとの地図 `maps`（正しい地図。下から順）とインスタンス（重なり位置 `order` つき）から作る（ファイルを読む
    /// とき）。面には新しい番号を記す。地図の塊は、同じレイヤーの直前のセル `prev` の同じ階層の地図と同じものを共有する
    pub(crate) fn from_maps(
        maps: Vec<(Vec<u64>, CheckedMap)>,
        mut instances: Vec<SymbolInstance>,
        prev: Option<&Cel>,
    ) -> Self {
        let maps: Vec<ScopeMap> = maps
            .into_iter()
            .filter_map(|(group_path, map)| {
                let prev_map = prev.and_then(|c| c.scope_stored(&group_path));
                ScopeMap::new(group_path, map, prev_map)
            })
            .collect();
        instances.sort_by_key(|i| i.order);
        let mut cel = Self::default();
        cel.arrange(Self::items_of(&maps, &instances));
        cel
    }

    /// テスト用：図形の並び（下から順。インスタンスは図形の印）から作る（地図にできる図形だけを渡す）
    #[cfg(test)]
    pub(crate) fn from_shapes_unwrap(all_shapes: Vec<FillShape>) -> Self {
        Self::from_shapes(all_shapes).expect("地図にできる図形")
    }

    /// テスト用：生の図形 `shapes`（下から順）とインスタンス（重なり位置 `order` は、図形の並びの何番目の手前か）から作る
    #[cfg(test)]
    pub(crate) fn new_with_instances(
        shapes: Vec<FillShape>,
        instances: Vec<SymbolInstance>,
    ) -> Self {
        let mut cel = Self::default();
        cel.set_shapes_and_instances(shapes, instances);
        cel
    }

    /// テスト用：生の図形 `shapes`（下から順）とインスタンス（重なり位置 `order` は、図形の並びの何番目の手前か）を入れる
    #[cfg(test)]
    pub(crate) fn set_shapes_and_instances(
        &mut self,
        mut shapes: Vec<FillShape>,
        mut instances: Vec<SymbolInstance>,
    ) {
        shapes.retain(|s| s.symbol_instance.is_none() && !s.exterior.segments.is_empty());
        instances.sort_by_key(|i| i.order);
        let mut parts = Vec::with_capacity(shapes.len() + instances.len());
        let mut insts = instances.into_iter().peekable();
        for (i, s) in shapes.into_iter().enumerate() {
            while let Some(inst) = insts.next_if(|inst| inst.order <= i) {
                parts.push(Part::Instance(inst));
            }
            parts.push(Part::Shape(Arc::new(s)));
        }
        parts.extend(insts.map(Part::Instance));
        self.set_new_parts(parts).expect("地図にできる図形");
    }

    /// 面の図形（写し）：階層の地図の順に、各地図の面の図形を並べたもの。インスタンスは入れない
    pub(crate) fn shapes(&self) -> Vec<Arc<FillShape>> {
        self.maps.iter().flat_map(|m| m.faces().to_vec()).collect()
    }

    /// 重なりの順の中身（図形とインスタンス）
    pub(crate) fn parts(&self) -> Vec<Part> {
        let mut parts = Vec::new();
        let mut insts = self.instances.iter().peekable();
        for (k, m) in self.maps.iter().enumerate() {
            while let Some(inst) = insts.next_if(|i| i.order <= k) {
                parts.push(Part::Instance(inst.clone()));
            }
            parts.extend(m.faces().iter().cloned().map(Part::Shape));
        }
        parts.extend(insts.cloned().map(Part::Instance));
        parts
    }

    /// 地図 `maps` とインスタンス `instances`（重なりの順）を、並べる前の中身にする
    fn items_of(maps: &[ScopeMap], instances: &[SymbolInstance]) -> Vec<Item<ScopeMap>> {
        let mut items = Vec::with_capacity(maps.len() + instances.len());
        let mut insts = instances.iter().peekable();
        for (k, m) in maps.iter().enumerate() {
            while let Some(inst) = insts.next_if(|i| i.order <= k) {
                items.push(Item::Instance(inst.clone()));
            }
            items.push(Item::Raw(m.group_path.clone(), m.clone()));
        }
        items.extend(insts.cloned().map(Item::Instance));
        items
    }

    /// 並べる前の中身（階層の地図とインスタンス）を並べて入れる（`arranged`）。同じ階層の地図が2つ以上来ることは無い
    /// （中身の生の図形の地図は `insert_content` が断り、ファイルの同じ階層の地図は読み込みの確かめ（`check_maps`）が断る。
    /// ほかの中身は、1枚のセル画の地図（階層ごとに1つ）から作る）
    fn arrange(&mut self, items: Vec<Item<ScopeMap>>) {
        let Ok((maps, instances)) = arranged(items, |_, mut maps: Vec<ScopeMap>| {
            Ok::<_, std::convert::Infallible>(maps.pop().filter(|m| !m.stored.is_empty()))
        });
        self.put(maps, instances);
    }

    /// 地図とインスタンスを入れる。地図はいつも正しい地図で、面に1つずつ別の番号が記してある（`ScopeMap` を作れるのは
    /// `ScopeMap::new`（確かめた地図から）・`restricted`・`renew_labels` だけ）。並びはいつも決まりどおり（`maps` と
    /// `instances` は `arranged` の結果か、その地図1つを同じ階層の地図に置き換えたもの）なので、確かめ直さない
    fn put(&mut self, mut maps: Vec<ScopeMap>, mut instances: Vec<SymbolInstance>) {
        maps.shrink_to_fit();
        instances.shrink_to_fit();
        self.maps = Arc::new(maps);
        self.instances = Arc::new(instances);
        self.revision = crate::vector_engine::next_mesh_revision();
    }

    /// 階層 `scope`（とその中）の、番号が `faces` の面を地図から外し（外した所は何も無い所になる）、番号が `instances` の
    /// インスタンスを外す（削除・シンボル化）。`false`：外すものが無い
    pub(crate) fn remove(
        &mut self,
        scope: &[u64],
        faces: &HashSet<ShapeId>,
        instances: &HashSet<u64>,
    ) -> bool {
        let mut changed = false;
        let mut items = Vec::new();
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(path, m)
                    if path.starts_with(scope)
                        && m.base().map.labels().any(|id| faces.contains(&id)) =>
                {
                    changed = true;
                    let keep = |id: ShapeId| !faces.contains(&id);
                    if let Some(kept) = m.restricted(&path, Some(&keep)) {
                        items.push(Item::Raw(path, kept));
                    }
                }
                Item::Instance(inst)
                    if inst.parent_scope.starts_with(scope)
                        && instances.contains(&inst.binding.instance_gid) =>
                {
                    changed = true;
                }
                item => items.push(item),
            }
        }
        if changed {
            self.arrange(items);
        }
        changed
    }

    /// グループ化：階層 `scope` の中の、選んだ面（番号 `faces`。`scope` より深い階層の地図の）とインスタンス（番号
    /// `instances`）の階層の番号に、新しいグループ `gid` を入れ（`scope` のすぐ下）、`scope` の中身のいちばん上へ移す。
    /// 階層 `scope` そのものの生の図形は、呼ぶ側が浮いた片の地図でグループの階層に書く。`false`：移すものが無い
    pub(crate) fn group(
        &mut self,
        scope: &[u64],
        faces: &HashSet<ShapeId>,
        instances: &HashSet<u64>,
        gid: u64,
    ) -> bool {
        let depth = scope.len();
        let into = |p: &[u64]| {
            let mut q = p.to_vec();
            q.insert(depth, gid);
            q
        };
        let mut rest: Vec<Item<ScopeMap>> = Vec::new();
        let mut moved: Vec<Item<ScopeMap>> = Vec::new();
        let mut first_at = None;
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(path, m)
                    if path.starts_with(scope)
                        && path.len() > depth
                        && m.base().map.labels().any(|id| faces.contains(&id)) =>
                {
                    first_at.get_or_insert(rest.len());
                    if let Some(kept) = m.restricted(&path, Some(&|id| !faces.contains(&id))) {
                        rest.push(Item::Raw(path.clone(), kept));
                    }
                    let to = into(&path);
                    if let Some(picked) = m.restricted(&to, Some(&|id| faces.contains(&id))) {
                        moved.push(Item::Raw(to, picked));
                    }
                }
                Item::Instance(mut inst)
                    if inst.parent_scope.starts_with(scope)
                        && instances.contains(&inst.binding.instance_gid) =>
                {
                    first_at.get_or_insert(rest.len());
                    inst.parent_scope = into(&inst.parent_scope);
                    moved.push(Item::Instance(inst));
                }
                item => rest.push(item),
            }
        }
        let Some(first_at) = first_at else {
            return false;
        };
        // 階層 `scope` の中身（残ったもの）のすぐ後ろ
        let at = rest
            .iter()
            .rposition(|it| it.scope().starts_with(scope))
            .map_or(first_at, |i| i + 1);
        rest.splice(at..at, moved);
        self.arrange(rest);
        true
    }

    /// グループの解除：階層 `scope` の中の、選んだ面（番号 `faces`。`scope` の孫より深い階層の地図の）とインスタンス
    /// （番号 `instances`。`scope` の子のグループの中のもの）の階層の番号から、`scope` の子の番号を外す（1つ上の階層へ）。
    /// 階層 `lifted`（解くグループの生の図形の階層。呼ぶ側が浮いた片にした）の地図は外す。`false`：変わるものが無い
    pub(crate) fn ungroup(
        &mut self,
        scope: &[u64],
        faces: &HashSet<ShapeId>,
        instances: &HashSet<u64>,
        lifted: &[Vec<u64>],
    ) -> bool {
        let depth = scope.len();
        let up = |p: &[u64]| {
            let mut q = p.to_vec();
            q.remove(depth);
            q
        };
        let mut changed = false;
        let mut items: Vec<Item<ScopeMap>> = Vec::new();
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(path, _) if lifted.contains(&path) => changed = true,
                Item::Raw(path, m)
                    if path.starts_with(scope)
                        && path.len() > depth + 1
                        && m.base().map.labels().any(|id| faces.contains(&id)) =>
                {
                    changed = true;
                    if let Some(kept) = m.restricted(&path, Some(&|id| !faces.contains(&id))) {
                        items.push(Item::Raw(path.clone(), kept));
                    }
                    let to = up(&path);
                    if let Some(picked) = m.restricted(&to, Some(&|id| faces.contains(&id))) {
                        items.push(Item::Raw(to, picked));
                    }
                }
                Item::Instance(mut inst)
                    if inst.parent_scope.starts_with(scope)
                        && inst.parent_scope.len() > depth
                        && instances.contains(&inst.binding.instance_gid) =>
                {
                    changed = true;
                    inst.parent_scope = up(&inst.parent_scope);
                    items.push(Item::Instance(inst));
                }
                item => items.push(item),
            }
        }
        if changed {
            self.arrange(items);
        }
        changed
    }

    /// 重ね順：階層 `scope` の子（グループ・インスタンス）の順を、番号の並び `order` にする（並びに無い子は動かさない）。
    /// `false`：変わらない
    pub(crate) fn rearrange(&mut self, scope: &[u64], order: &[u64]) -> bool {
        let depth = scope.len();
        // 中身が階層 `scope` のどの子に入るか（`scope` そのものの生の図形と、`scope` の外は `None`）
        let child = |it: &Item<ScopeMap>| -> Option<u64> {
            match it {
                Item::Raw(p, _) if p.starts_with(scope) && p.len() > depth => Some(p[depth]),
                Item::Instance(i) if i.parent_scope == scope => Some(i.binding.instance_gid),
                Item::Instance(i) if i.parent_scope.starts_with(scope) => {
                    Some(i.parent_scope[depth])
                }
                _ => None,
            }
        };
        let mut items: Vec<Option<Item<ScopeMap>>> = Self::items_of(&self.maps, &self.instances)
            .into_iter()
            .map(Some)
            .collect();
        let slots: Vec<usize> = (0..items.len())
            .filter(|&i| items[i].as_ref().and_then(&child).is_some())
            .collect();
        let mut keyed: Vec<Item<ScopeMap>> =
            slots.iter().filter_map(|&i| items[i].take()).collect();
        let before: Vec<Option<u64>> = keyed.iter().map(&child).collect();
        keyed.sort_by_key(|it| order.iter().position(|&g| Some(g) == child(it)));
        if keyed.iter().map(&child).eq(before) {
            return false;
        }
        for (slot, it) in slots.into_iter().zip(keyed) {
            items[slot] = Some(it);
        }
        self.arrange(items.into_iter().flatten().collect());
        true
    }

    /// 階層 `scope` とその中の地図とインスタンスを、みな外す（今の階層を消す）。`false`：外すものが無い
    pub(crate) fn clear(&mut self, scope: &[u64]) -> bool {
        let items = Self::items_of(&self.maps, &self.instances);
        let n = items.len();
        let kept: Vec<Item<ScopeMap>> = items
            .into_iter()
            .filter(|item| !item.scope().starts_with(scope))
            .collect();
        if kept.len() == n {
            return false;
        }
        self.arrange(kept);
        true
    }

    /// 番号が `pick` に合うインスタンスの結び付けを `f` で書き換える。`false`：当てはまるインスタンスが無いか、変わらない
    pub(crate) fn update_instances(
        &mut self,
        pick: impl Fn(u64) -> bool,
        f: impl Fn(&mut SymbolInstanceBinding),
    ) -> bool {
        if !self.instances.iter().any(|i| pick(i.binding.instance_gid)) {
            return false;
        }
        let mut changed = false;
        for inst in self.instances_mut() {
            if pick(inst.binding.instance_gid) {
                let before = inst.binding;
                f(&mut inst.binding);
                changed |= inst.binding != before;
            }
        }
        changed
    }

    #[inline]
    pub(crate) fn instances_mut(&mut self) -> &mut Vec<SymbolInstance> {
        self.revision = crate::vector_engine::next_mesh_revision();
        Arc::make_mut(&mut self.instances)
    }

    /// 階層の地図のグループの番号を書き換える（ファイルを読むとき・ほかの文書から写すときの番号の振り直し）
    pub(crate) fn remap_group_ids(&mut self, mut f: impl FnMut(u64) -> u64) {
        if self.maps.iter().all(|m| m.group_path.is_empty()) {
            return;
        }
        for m in Arc::make_mut(&mut self.maps) {
            for gid in &mut m.group_path {
                *gid = f(*gid);
            }
        }
        self.revision = crate::vector_engine::next_mesh_revision();
    }

    /// 面の図形の番号を、みな新しい番号にする（キーフレームを写したとき。同じコマに同じ番号の図形が2つ出ないように）
    pub(crate) fn renew_shape_ids(&mut self) {
        if self.maps.is_empty() {
            return;
        }
        for m in Arc::make_mut(&mut self.maps) {
            m.renew_labels();
        }
        self.revision = crate::vector_engine::next_mesh_revision();
    }

    /// テスト用：重なりの順の中身 `parts`（生の図形は新しい図形でよい）をセル画にする（図形から地図を作る：
    /// `test_scope_map_from_shapes`）
    #[cfg(test)]
    pub(crate) fn set_new_parts(&mut self, parts: Vec<Part>) -> Result<(), Unmappable> {
        let old = self.maps.clone();
        self.put_parts(parts, |path, raw| {
            test_scope_map_from_shapes(&old, path, raw)
        })
    }

    /// 重なりの順の中身 `parts` を並べ（`arranged`）、階層ごとの生の図形を `scope` で地図にして入れる（テスト用）
    #[cfg(test)]
    fn put_parts(
        &mut self,
        parts: Vec<Part>,
        scope: impl FnMut(&[u64], Vec<Arc<FillShape>>) -> Result<Option<ScopeMap>, Unmappable>,
    ) -> Result<(), Unmappable> {
        let items = parts
            .into_iter()
            .map(|p| match p {
                Part::Shape(s) => Item::Raw(s.group_path.clone(), s),
                Part::Instance(i) => Item::Instance(i),
            })
            .collect();
        let (maps, instances) = arranged(items, scope)?;
        self.put(maps, instances);
        Ok(())
    }

    /// 階層 `scope`（とその中）のうち、選んだもの（面の番号 `faces`・インスタンス `instances`）だけのセル画（新しい中身の
    /// 1枚：`clipboard_ops::Content`）。階層の番号の並びは `scope` からの相対。地図は、選んだ面だけを残した地図（塊は
    /// このセル画の地図と共有する）。`raw` があれば、階層 `scope` の生の図形の地図はそれにする（浮いた片のあるレイヤー：
    /// 残りの選んだ所に片を重ねた地図）。`tween`：トゥイーンの途中なら、そのコマのインスタンスの結び付け
    pub(crate) fn picked(
        &self,
        scope: &[u64],
        faces: &HashSet<ShapeId>,
        instances: &HashSet<u64>,
        raw: Option<CheckedMap>,
        tween: Option<SymbolInstanceBinding>,
    ) -> Cel {
        let mut items = Vec::new();
        let has_raw = raw.is_some();
        if let Some(m) =
            raw.and_then(|raw| ScopeMap::new(Vec::new(), raw, self.scope_stored(scope)))
        {
            items.push(Item::Raw(Vec::new(), m));
        }
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(path, m) => {
                    let Some(rel) = path.strip_prefix(scope) else {
                        continue;
                    };
                    if rel.is_empty() && has_raw {
                        continue;
                    }
                    if let Some(m) = m.restricted(rel, Some(&|id| faces.contains(&id))) {
                        items.push(Item::Raw(rel.to_vec(), m));
                    }
                }
                Item::Instance(mut inst) => {
                    let Some(rel) = inst.parent_scope.strip_prefix(scope) else {
                        continue;
                    };
                    if !instances.contains(&inst.binding.instance_gid) {
                        continue;
                    }
                    inst.parent_scope = rel.to_vec();
                    if let Some(b) = tween.filter(|b| b.instance_gid == inst.binding.instance_gid) {
                        inst.binding = b;
                    }
                    items.push(Item::Instance(inst));
                }
            }
        }
        let mut cel = Cel::default();
        cel.arrange(items);
        cel
    }

    /// セル画 `cels`（下から順）を1枚に重ねたもの：中身を順に並べ、同じ階層の生の図形の地図は下から順に重ねる（上が下を
    /// 隠す。面には1つずつ別の番号を記す）。`None`：格子に置けない座標がある
    pub(crate) fn stacked(cels: &[Cel]) -> Option<Cel> {
        if let [one] = cels {
            return Some(one.clone());
        }
        let items: Vec<Item<ScopeMap>> = cels
            .iter()
            .flat_map(|c| Self::items_of(&c.maps, &c.instances))
            .collect();
        let (maps, instances) = arranged(items, |path, maps: Vec<ScopeMap>| {
            if maps.len() == 1 {
                return Ok::<_, Unmappable>(maps.into_iter().next());
            }
            let mut map = PlanarMap::default();
            for m in &maps {
                let d = m.dense();
                map = if map.edges.is_empty() {
                    d
                } else {
                    map.overlay(MapInput::Map(&d), MapRule::Overlay)
                        .ok_or(Unmappable)?
                };
            }
            Ok(ScopeMap::new(
                path.to_vec(),
                map.checked().ok_or(Unmappable)?,
                None,
            ))
        })
        .ok()?;
        let mut cel = Cel::default();
        cel.put(maps, instances);
        Some(cel)
    }

    /// （生の図形の地図（階層 `[]`）, それを除いたセル画）
    pub(crate) fn split_raw(&self) -> (Option<PlanarMap>, Cel) {
        let Some(raw) = self.scope_stored(&[]).map(StoredMap::to_dense) else {
            return (None, self.clone());
        };
        let mut items = Self::items_of(&self.maps, &self.instances);
        items.retain(|it| !matches!(it, Item::Raw(p, _) if p.is_empty()));
        let mut rest = Cel::default();
        rest.arrange(items);
        (Some(raw), rest)
    }

    /// 座標の変換 `xform` をかけたセル画：地図は、その座標の格子で正しい地図にし（`realized`）、面にみな新しい番号を
    /// 記す（元の面と番号が重ならない）。インスタンスの変換に `xform` を重ねる。`None`：格子に置けない座標がある
    pub(crate) fn transformed(&self, xform: Affine) -> Option<Cel> {
        let mut items = Vec::with_capacity(self.maps.len() + self.instances.len());
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(path, m) => {
                    let map = realized(&m.base().map, xform)?.freshly_labeled();
                    if let Some(m) = ScopeMap::new(path.clone(), map, None) {
                        items.push(Item::Raw(path, m));
                    }
                }
                Item::Instance(mut inst) => {
                    inst.binding.xform = xform * inst.binding.xform;
                    items.push(Item::Instance(inst));
                }
            }
        }
        let mut cel = Cel::default();
        cel.arrange(items);
        Some(cel)
    }

    /// グループ・インスタンスの番号を `gid` で、参照するシンボルの番号を `sym` で付け替え、インスタンスのレイヤーを
    /// `layer_id` にする（新しい中身を置くとき・ほかの文書から写すとき）
    pub(crate) fn remap_ids(
        &mut self,
        mut gid: impl FnMut(u64) -> u64,
        sym: impl Fn(u64) -> u64,
        layer_id: Option<u64>,
    ) {
        self.remap_group_ids(&mut gid);
        if self.instances.is_empty() {
            return;
        }
        for inst in self.instances_mut() {
            inst.binding.symbol_id = sym(inst.binding.symbol_id);
            inst.binding.instance_gid = gid(inst.binding.instance_gid);
            for g in &mut inst.parent_scope {
                *g = gid(*g);
            }
            if let Some(l) = layer_id {
                inst.layer_id = l;
            }
        }
    }

    /// 中身 `content`（階層の番号の並びは `scope` からの相対。生の図形の地図は持たない：浮いた片にする）を、階層 `scope` に
    /// 入れる：インスタンス `replacing` があればそれと入れ替え（その重なりの位置に）、無ければ階層 `scope` のいちばん上に。
    /// グループとインスタンスは中身の順のまま（グループの生の図形は、その階層のいちばん下）。`false`：入れ替えるインスタンスが
    /// 無いか、中身に階層 `scope` の生の図形の地図がある（同じ階層の地図が2つになる。セル画は変えない）
    pub(crate) fn insert_content(
        &mut self,
        scope: &[u64],
        content: &Cel,
        replacing: Option<u64>,
    ) -> bool {
        if content.scope_stored(&[]).is_some() {
            return false;
        }
        let mut items = Self::items_of(&self.maps, &self.instances);
        let at = match replacing {
            Some(gid) => {
                let Some(i) = items.iter().position(
                    |it| matches!(it, Item::Instance(inst) if inst.binding.instance_gid == gid),
                ) else {
                    return false;
                };
                items.remove(i);
                i
            }
            None => items
                .iter()
                .rposition(|it| it.scope().starts_with(scope))
                .map_or(items.len(), |i| i + 1),
        };
        let added = Self::items_of(&content.maps, &content.instances)
            .into_iter()
            .map(|it| match it {
                Item::Raw(p, mut m) => {
                    m.group_path = [scope, &p[..]].concat();
                    Item::Raw(m.group_path.clone(), m)
                }
                Item::Instance(mut inst) => {
                    inst.parent_scope = [scope, &inst.parent_scope[..]].concat();
                    Item::Instance(inst)
                }
            });
        items.splice(at..at, added);
        self.arrange(items);
        true
    }

    /// 地図の面の番号（みな）
    pub(crate) fn face_ids(&self) -> HashSet<ShapeId> {
        self.maps
            .iter()
            .flat_map(|m| m.dense().labels().collect::<Vec<_>>())
            .collect()
    }

    /// 地図の外接矩形（辺の曲線の）。地図が無ければ `None`
    pub(crate) fn map_bounds(&self) -> Option<kurbo::Rect> {
        self.maps
            .iter()
            .filter_map(|m| m.dense().bounding_box())
            .reduce(|a, b| a.union(b))
    }

    /// 階層 `group_path` のしまった地図
    fn scope_stored(&self, group_path: &[u64]) -> Option<&StoredMap> {
        self.maps
            .iter()
            .find(|m| m.group_path == group_path)
            .map(|m| &m.stored)
    }

    /// 階層 `group_path` の地図（計算に使う形）。生の図形の無い階層は空の地図
    pub(crate) fn scope_base(&self, group_path: &[u64]) -> Arc<LayerBase> {
        self.maps
            .iter()
            .find(|m| m.group_path == group_path)
            .map_or_else(
                || Arc::new(LayerBase::new(&PlanarMap::default())),
                ScopeMap::base,
            )
    }

    /// 地図の編集：階層 `group_path` の地図（文書の座標）を `f` で編集し、返った地図をしまう。面の図形は、変わらない面は
    /// 前の図形のまま、形の変わった面は番号を引き継いだ新しい図形（`LayerBase::diff`）。`None`：変わらない・編集できない
    pub(crate) fn edit_map(
        &mut self,
        group_path: &[u64],
        f: impl FnOnce(&LayerBase) -> Option<PlanarMap>,
    ) -> Option<MapWritten> {
        let base = self.scope_base(group_path);
        let next = f(&base)?;
        let faces = base.faces(group_path);
        let own: Vec<&FillShape> = faces.iter().map(|s| &**s).collect();
        let change = base.diff(&next, &own, 0, group_path)?;
        if change.removed.is_empty() && change.added.is_empty() {
            return None;
        }
        let (map, shapes) = change.into_face_shapes(&faces)?;
        Some(MapWritten(self.put_map(group_path, map, shapes)))
    }

    /// 地図 `map`（文書の座標。面の図形 `shapes`：`MapChange::into_face_shapes`）を入れる：階層の地図をこの地図にする。
    /// 面の無くなった階層は外し、新しい階層は決まりどおりの所（グループの中身があれば、そのいちばん下。無ければ親の階層の
    /// いちばん上）に入れる。戻り値は、新しい地図の計算に使う写し。階層を外したときは `None`
    fn put_map(
        &mut self,
        group_path: &[u64],
        map: CheckedMap,
        shapes: Vec<Arc<FillShape>>,
    ) -> Option<Arc<LayerBase>> {
        let at = self.maps.iter().position(|m| m.group_path == group_path);
        let Some(m) = ScopeMap::new(group_path.to_vec(), map, self.scope_stored(group_path)) else {
            let mut items = Self::items_of(&self.maps, &self.instances);
            items.retain(|it| !matches!(it, Item::Raw(p, _) if p == group_path));
            self.arrange(items);
            return None;
        };
        let base = LayerBase::with_faces(m.stored.clone(), group_path, shapes);
        match at {
            Some(i) => {
                let mut maps = (*self.maps).clone();
                maps[i] = m;
                let instances = (*self.instances).clone();
                self.put(maps, instances);
            }
            None => {
                let mut items = Self::items_of(&self.maps, &self.instances);
                items.push(Item::Raw(group_path.to_vec(), m));
                self.arrange(items);
            }
        }
        Some(base)
    }

    /// テスト用：地図と重なりの順が決まりどおりか（どの地図も正しい地図で面があり、面に1つずつ別の番号が記してある。
    /// 同じ階層の地図は1つで、階層の地図はその親の階層の地図より後ろ。インスタンスは重なり位置の順で、位置は地図の数以下、
    /// 親の階層の地図より後ろ）。決まりどおりでなければ、その中身
    #[cfg(test)]
    pub(crate) fn maps_problem(&self) -> Option<String> {
        for (k, m) in self.maps.iter().enumerate() {
            let base = m.base();
            if !base.map.is_valid() {
                return Some(format!(
                    "階層 {:?} の地図が正しくない: {}",
                    m.group_path,
                    base.map.validity_report()
                ));
            }
            let (faces, _) = base.map.faces();
            if faces.is_empty() {
                return Some(format!("階層 {:?} の地図に面が無い", m.group_path));
            }
            let mut ids = std::collections::HashSet::new();
            for f in &faces {
                match f.srcs[..] {
                    [id] if ids.insert(id) => {}
                    _ => return Some(format!("階層 {:?}：面の番号が {:?}", m.group_path, f.srcs)),
                }
            }
            if self.maps[..k].iter().any(|o| o.group_path == m.group_path) {
                return Some(format!("階層 {:?} の地図が2つある", m.group_path));
            }
            for len in 0..m.group_path.len() {
                let parent = &m.group_path[..len];
                if self.maps[k + 1..].iter().any(|o| o.group_path == parent) {
                    return Some(format!(
                        "階層 {:?} の地図が、親の階層 {parent:?} の地図より前にある",
                        m.group_path
                    ));
                }
            }
        }
        let mut last = 0;
        for inst in self.instances.iter() {
            if inst.order < last || inst.order > self.maps.len() {
                return Some(format!(
                    "インスタンスの重なり位置 {} が順でない",
                    inst.order
                ));
            }
            last = inst.order;
            if let Some(p) = self
                .maps
                .iter()
                .position(|m| m.group_path == inst.parent_scope)
            {
                if p >= inst.order {
                    return Some(format!(
                        "インスタンスが、その階層 {:?} の生の図形より下にある",
                        inst.parent_scope
                    ));
                }
            }
        }
        None
    }

    /// テスト用：キャンバス展開図形列 `all_shapes`（下から順）を、重なりの順の中身にする（インスタンスは図形の印から
    /// 1つずつ作る）
    #[cfg(test)]
    pub(crate) fn parts_of(all_shapes: Vec<FillShape>) -> Vec<Part> {
        let mut parts = Vec::with_capacity(all_shapes.len());
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for s in all_shapes {
            if let Some(b) = s.symbol_instance {
                if seen.insert(b.instance_gid) {
                    parts.push(Part::Instance(SymbolInstance {
                        binding: b,
                        layer_id: s.layer_id,
                        parent_scope: s
                            .group_path
                            .iter()
                            .take_while(|&&gid| gid != b.instance_gid)
                            .copied()
                            .collect(),
                        order: 0,
                    }));
                }
            } else if !s.exterior.segments.is_empty() {
                parts.push(Part::Shape(Arc::new(s)));
            }
        }
        parts
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.maps.is_empty() && self.instances.is_empty()
    }

    /// このセル（キーフレーム）に「ルート直下の単一シンボルインスタンスのみ（1個だけ）」が存在する場合、
    /// その `SymbolInstanceBinding` を返す。
    /// （空セル、生シェイプのみ、通常グループのみ、複数シンボル、シンボル＋生シェイプ、シンボル＋グループ等の場合はすべて `None`）
    pub(crate) fn single_symbol_instance(&self) -> Option<SymbolInstanceBinding> {
        (self.maps.is_empty()
            && self.instances.len() == 1
            && self.instances[0].parent_scope.is_empty())
        .then(|| self.instances[0].binding)
    }
}

/// タイムライン上のキーフレーム（開始フレーム番号 0-indexed、セル画 `Cel`、クラシックトゥイーン有効フラグ `tween`、トゥイーン回転方向 `tween_rotate`）
#[derive(Clone, Debug)]
pub(crate) struct Keyframe {
    pub(crate) start_frame: usize,
    pub(crate) cel: Cel,
    pub(crate) tween: bool,
    pub(crate) tween_rotate: TweenRotate,
}

impl Keyframe {
    pub(crate) fn new_blank(start_frame: usize) -> Self {
        Self {
            start_frame,
            cel: Cel::default(),
            tween: false,
            tween_rotate: TweenRotate::Auto,
        }
    }

    /// キーフレーム内のセル図形とトゥイーン設定をクリアし、変更があったかどうかを返す
    pub(crate) fn clear_cel(&mut self) -> bool {
        if self.cel.is_empty() && !self.tween {
            false
        } else {
            self.cel.clear(&[]);
            self.tween = false;
            self.tween_rotate = TweenRotate::Auto;
            true
        }
    }
}

/// クラシックトゥイーンは単一シンボルのセルでだけ有効にする。無効なら回転も `Auto` に戻した `(tween, tween_rotate)` を返す
fn valid_tween_settings(tween: bool, rotate: TweenRotate, cel: &Cel) -> (bool, TweenRotate) {
    if tween && cel.single_symbol_instance().is_some() {
        (true, rotate)
    } else {
        (false, TweenRotate::Auto)
    }
}

fn remap_keyframes_group_and_instance_ids(
    kfs: &mut [Keyframe],
    mut alloc_group_id: impl FnMut() -> u64,
) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    let mut alloc_gid = |old_id: u64, map: &mut HashMap<u64, u64>| -> u64 {
        *map.entry(old_id).or_insert_with(&mut alloc_group_id)
    };
    for kf in kfs {
        kf.cel.renew_shape_ids();
        kf.cel
            .remap_ids(|gid| alloc_gid(gid, &mut gid_map), |sym| sym, None);
    }
}

/// タイムライン上で選択されているセル矩形範囲（複数レイヤー × 複数フレーム対応）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineCellSelection {
    pub(crate) min_layer: usize,
    pub(crate) max_layer: usize,
    pub(crate) min_frame: usize,
    pub(crate) max_frame: usize,
}

impl TimelineCellSelection {
    pub(crate) fn single(layer: usize, frame: usize) -> Self {
        Self {
            min_layer: layer,
            max_layer: layer,
            min_frame: frame,
            max_frame: frame,
        }
    }

    pub(crate) fn from_corners(l0: usize, f0: usize, l1: usize, f1: usize) -> Self {
        Self {
            min_layer: l0.min(l1),
            max_layer: l0.max(l1),
            min_frame: f0.min(f1),
            max_frame: f0.max(f1),
        }
    }

    pub(crate) fn contains(&self, layer: usize, frame: usize) -> bool {
        layer >= self.min_layer
            && layer <= self.max_layer
            && frame >= self.min_frame
            && frame <= self.max_frame
    }

    pub(crate) fn contains_layer(&self, layer: usize) -> bool {
        layer >= self.min_layer && layer <= self.max_layer
    }
}

/// レイヤーのキーフレームの列（開始フレームの順）。
///
/// 列を一定の数（`KEYFRAME_CHUNK`）ほどずつの塊に分け、塊も塊の並びも `Arc` で共有して持つ。複製（Undo の控え）は
/// `Arc` 1つで済み、作品の長さで重くならない。書き換えは、塊の並びと、触った塊1つだけを写す（Copy-on-Write）
#[derive(Clone, Debug, Default)]
pub(crate) struct KeyframeList {
    chunks: Arc<KeyframeChunks>,
    len: usize,
}

/// 塊の並びと、各塊の先頭の番号
#[derive(Clone, Debug, Default)]
struct KeyframeChunks {
    chunks: Vec<KeyframeChunk>,
    starts: Vec<usize>,
}

/// 1つの塊のキーフレームの数の目安（足して2倍を超えたら2つに分ける）
const KEYFRAME_CHUNK: usize = 64;

impl KeyframeList {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// `i` 番目（`i` は列の長さより小さい）が入っている塊と、その中の位置
    fn locate(all: &KeyframeChunks, i: usize) -> (usize, usize) {
        let c = all.starts.partition_point(|&s| s <= i) - 1;
        (c, i - all.starts[c])
    }

    pub(crate) fn get(&self, i: usize) -> Option<&Arc<Keyframe>> {
        (i < self.len).then(|| {
            let (c, k) = Self::locate(&self.chunks, i);
            &self.chunks.chunks[c][k]
        })
    }

    pub(crate) fn first(&self) -> Option<&Arc<Keyframe>> {
        self.get(0)
    }

    #[cfg(test)]
    pub(crate) fn last(&self) -> Option<&Arc<Keyframe>> {
        self.len.checked_sub(1).and_then(|i| self.get(i))
    }

    pub(crate) fn iter(&self) -> KeyframeIter<'_> {
        self.into_iter()
    }

    /// `pred` が真から偽に変わる番号（`slice::partition_point` と同じ。列は `pred` で分かれている前提）
    pub(crate) fn partition_point(&self, mut pred: impl FnMut(&Arc<Keyframe>) -> bool) -> usize {
        let (mut lo, mut hi) = (0, self.len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(&self[mid]) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub(crate) fn insert(&mut self, i: usize, kf: Arc<Keyframe>) {
        assert!(
            i <= self.len,
            "キーフレームの番号 {i} が列の長さ {} を超える",
            self.len
        );
        let all = Arc::make_mut(&mut self.chunks);
        if all.chunks.is_empty() {
            all.chunks.push(Arc::new(Vec::new()));
        }
        let (c, k) = if i == self.len {
            let last = all.chunks.len() - 1;
            (last, all.chunks[last].len())
        } else {
            Self::locate(all, i)
        };
        let chunk = Arc::make_mut(&mut all.chunks[c]);
        chunk.insert(k, kf);
        if chunk.len() > 2 * KEYFRAME_CHUNK {
            let tail = chunk.split_off(chunk.len() / 2);
            all.chunks.insert(c + 1, Arc::new(tail));
        }
        self.len += 1;
        Self::restart(all);
    }

    pub(crate) fn remove(&mut self, i: usize) -> Arc<Keyframe> {
        assert!(
            i < self.len,
            "キーフレームの番号 {i} が列の長さ {} を超える",
            self.len
        );
        let all = Arc::make_mut(&mut self.chunks);
        let (c, k) = Self::locate(all, i);
        let removed = Arc::make_mut(&mut all.chunks[c]).remove(k);
        if all.chunks[c].is_empty() {
            all.chunks.remove(c);
        }
        self.len -= 1;
        Self::restart(all);
        removed
    }

    #[cfg(test)]
    pub(crate) fn push(&mut self, kf: Arc<Keyframe>) {
        self.insert(self.len, kf);
    }

    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// `keep` が偽のキーフレームを消す。消すものの無い塊は、写さずにそのまま共有する
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&Arc<Keyframe>) -> bool) {
        let all = Arc::make_mut(&mut self.chunks);
        for chunk in &mut all.chunks {
            if !chunk.iter().all(&mut keep) {
                Arc::make_mut(chunk).retain(&mut keep);
            }
        }
        all.chunks.retain(|c| !c.is_empty());
        self.len = all.chunks.iter().map(|c| c.len()).sum();
        Self::restart(all);
    }

    /// テスト用：`other` と共有している塊の数と、塊の数
    #[cfg(test)]
    pub(crate) fn shared_chunks_with(&self, other: &Self) -> (usize, usize) {
        let shared = self
            .chunks
            .chunks
            .iter()
            .filter(|c| other.chunks.chunks.iter().any(|o| Arc::ptr_eq(c, o)))
            .count();
        (shared, self.chunks.chunks.len())
    }

    /// 各塊の先頭の番号を数え直す
    fn restart(all: &mut KeyframeChunks) {
        all.starts.clear();
        let mut n = 0;
        for c in &all.chunks {
            all.starts.push(n);
            n += c.len();
        }
    }
}

impl From<Vec<Arc<Keyframe>>> for KeyframeList {
    fn from(list: Vec<Arc<Keyframe>>) -> Self {
        list.into_iter().collect()
    }
}

impl FromIterator<Arc<Keyframe>> for KeyframeList {
    fn from_iter<I: IntoIterator<Item = Arc<Keyframe>>>(iter: I) -> Self {
        let mut all = KeyframeChunks::default();
        let mut len = 0;
        let mut cur: Vec<Arc<Keyframe>> = Vec::new();
        for kf in iter {
            cur.push(kf);
            len += 1;
            if cur.len() == KEYFRAME_CHUNK {
                all.chunks.push(Arc::new(std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            all.chunks.push(Arc::new(cur));
        }
        KeyframeList::restart(&mut all);
        Self {
            chunks: Arc::new(all),
            len,
        }
    }
}

impl std::ops::Index<usize> for KeyframeList {
    type Output = Arc<Keyframe>;
    fn index(&self, i: usize) -> &Arc<Keyframe> {
        self.get(i)
            .unwrap_or_else(|| panic!("キーフレームの番号 {i} が列の長さ {} を超える", self.len))
    }
}

impl std::ops::IndexMut<usize> for KeyframeList {
    /// 触った塊と塊の並びだけを写す（Undo の控えと共有していれば）
    fn index_mut(&mut self, i: usize) -> &mut Arc<Keyframe> {
        assert!(
            i < self.len,
            "キーフレームの番号 {i} が列の長さ {} を超える",
            self.len
        );
        let (c, k) = Self::locate(&self.chunks, i);
        let all = Arc::make_mut(&mut self.chunks);
        &mut Arc::make_mut(&mut all.chunks[c])[k]
    }
}

/// キーフレームの塊
type KeyframeChunk = Arc<Vec<Arc<Keyframe>>>;

/// 塊1つのキーフレームをたどる
type ChunkIterFn<'a> = fn(&'a KeyframeChunk) -> std::slice::Iter<'a, Arc<Keyframe>>;

/// キーフレームの列を前から順にたどる
pub(crate) type KeyframeIter<'a> = std::iter::FlatMap<
    std::slice::Iter<'a, KeyframeChunk>,
    std::slice::Iter<'a, Arc<Keyframe>>,
    ChunkIterFn<'a>,
>;

impl<'a> IntoIterator for &'a KeyframeList {
    type Item = &'a Arc<Keyframe>;
    type IntoIter = KeyframeIter<'a>;
    fn into_iter(self) -> KeyframeIter<'a> {
        let chunk_iter: ChunkIterFn<'a> = |c| c.iter();
        self.chunks.chunks.iter().flat_map(chunk_iter)
    }
}

/// タイムラインの1レイヤー（固有ID、名前、表示/非表示、ロック状態、レイヤー固有フレーム長、開始フレーム順にソートされたキーフレーム配列）
#[derive(Clone, Debug)]
pub(crate) struct Layer {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) locked: bool,
    pub(crate) frame_count: usize,
    pub(crate) keyframes: KeyframeList,
}

impl Layer {
    pub(crate) fn new_with_id_and_frames(
        id: u64,
        name: impl Into<String>,
        frame_count: usize,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            locked: false,
            frame_count: frame_count.max(1),
            keyframes: vec![Arc::new(Keyframe::new_blank(0))].into(),
        }
    }

    pub(crate) fn new_with_id(id: u64, name: impl Into<String>) -> Self {
        Self::new_with_id_and_frames(id, name, 1)
    }

    /// `i` 番目のキーフレームへの可変参照を返す（Undo 履歴と共有されている場合は Copy-on-Write で複製する）
    pub(crate) fn keyframe_mut(&mut self, i: usize) -> &mut Keyframe {
        Arc::make_mut(&mut self.keyframes[i])
    }

    /// 指定フレーム番号 `frame` において有効な（保持されている）キーフレームのインデックスを返す
    /// （そのレイヤーの `frame_count` を超えるフレームでは `None` を返す）
    pub(crate) fn keyframe_index_at(&self, frame: usize) -> Option<usize> {
        if frame >= self.frame_count {
            return None;
        }
        let idx = self.keyframes.partition_point(|kf| kf.start_frame <= frame);
        idx.checked_sub(1)
    }

    /// `kf_idx` 番目のキーフレームが保持される終端フレーム番号（inclusive）を返す
    pub(crate) fn span_end_for_keyframe(&self, kf_idx: usize) -> usize {
        let max_f = self.frame_count.saturating_sub(1);
        self.keyframes
            .get(kf_idx + 1)
            .map(|next| next.start_frame.saturating_sub(1).min(max_f))
            .unwrap_or(max_f)
    }

    /// 指定フレーム番号 `frame` で表示される `Cel` への不変参照を返す
    pub(crate) fn cel_at(&self, frame: usize) -> Option<&Cel> {
        let idx = self.keyframe_index_at(frame)?;
        Some(&self.keyframes[idx].cel)
    }

    /// 指定フレーム `frame` にキーフレームを挿入する（`clone_prev == true`: `F6` 複製 / `false`: `F7` 空白）
    pub(crate) fn insert_keyframe(&mut self, frame: usize, clone_prev: bool) -> bool {
        let mut changed = false;
        if frame >= self.frame_count {
            self.frame_count = frame + 1;
            changed = true;
        }
        if let Some(existing_idx) = self.keyframes.iter().position(|kf| kf.start_frame == frame) {
            if !clone_prev
                && (!self.keyframes[existing_idx].cel.is_empty()
                    || self.keyframes[existing_idx].tween)
            {
                changed |= self.keyframe_mut(existing_idx).clear_cel();
            }
            return changed;
        }

        let new_kf = if clone_prev {
            self.snapshot_keyframe_at(frame, frame)
        } else {
            Keyframe::new_blank(frame)
        };
        let insert_idx = self.keyframes.partition_point(|kf| kf.start_frame < frame);
        self.keyframes.insert(insert_idx, Arc::new(new_kf));
        true
    }

    /// 指定フレーム `frame` を含むキーフレームを削除（フレーム0の先頭の場合は空白化）する（`Shift+F6`）
    pub(crate) fn delete_keyframe_at(&mut self, frame: usize) -> bool {
        let Some(idx) = self.keyframe_index_at(frame) else {
            return false;
        };
        if self.keyframes[idx].start_frame != frame {
            return false;
        }
        if idx == 0 {
            self.keyframe_mut(0).clear_cel()
        } else {
            self.keyframes.remove(idx);
            true
        }
    }

    /// 指定フレーム `frame` の保持尺を1フレーム延長し、後続のキーフレームを +1 フレーム後ろへずらす（`F5`）
    pub(crate) fn insert_frame_at(&mut self, frame: usize) {
        if frame >= self.frame_count {
            self.frame_count = frame + 1;
        } else {
            for i in 0..self.keyframes.len() {
                if self.keyframes[i].start_frame > frame {
                    self.keyframe_mut(i).start_frame += 1;
                }
            }
            self.frame_count += 1;
        }
    }

    /// Adobe Animate 仕様のフレーム削除（`Shift+F5`）：
    /// - 対象キーフレーム区間に保持フレーム（2コマ以上の長さ）がある場合は、キーフレームを残したまま保持尺を1フレーム縮め、後続キーフレームを左へ1フレーム詰める。
    /// - 対象キーフレーム区間が1フレーム幅（保持フレームなし）の場合は、そのキーフレーム自体を削除し、後続キーフレームを左へ1フレーム詰めて繰り上げる。
    pub(crate) fn remove_frame_at(&mut self, frame: usize) -> bool {
        if self.frame_count == 0 || frame >= self.frame_count {
            return false;
        }
        let Some(kf_idx) = self.keyframe_index_at(frame) else {
            return false;
        };
        let span_start = self.keyframes[kf_idx].start_frame;
        let span_end = self.span_end_for_keyframe(kf_idx);
        let span_len = span_end.saturating_sub(span_start) + 1;

        if span_len >= 2 {
            for i in (kf_idx + 1)..self.keyframes.len() {
                let kf = self.keyframe_mut(i);
                kf.start_frame = kf.start_frame.saturating_sub(1);
            }
            self.frame_count = (self.frame_count - 1).max(1);
            true
        } else if self.frame_count <= 1 {
            self.keyframe_mut(0).clear_cel()
        } else {
            self.keyframes.remove(kf_idx);
            for i in kf_idx..self.keyframes.len() {
                let kf = self.keyframe_mut(i);
                kf.start_frame = kf.start_frame.saturating_sub(1);
            }
            self.frame_count = (self.frame_count - 1).max(1);
            self.ensure_head_keyframe();
            true
        }
    }

    /// 先頭（0 フレーム目）のキーフレームが無ければ、空白のキーフレームを入れる
    fn ensure_head_keyframe(&mut self) {
        if self.keyframes.first().is_none_or(|kf| kf.start_frame > 0) {
            self.keyframes.insert(0, Arc::new(Keyframe::new_blank(0)));
        }
    }

    /// 指定フレーム `frame` における評価済みセル姿勢（クラシックトゥイーン中割り姿勢および経過フレーム分の内部 `first_frame` 進行を含む）を、
    /// 指定された `start_frame` を持つ新しい `Keyframe` として構築して返す
    pub(crate) fn snapshot_keyframe_at(&self, frame: usize, start_frame: usize) -> Keyframe {
        if frame >= self.frame_count {
            return Keyframe::new_blank(start_frame);
        }
        let Some(kf_idx) = self.keyframe_index_at(frame) else {
            return Keyframe::new_blank(start_frame);
        };
        let kf = &self.keyframes[kf_idx];
        if kf.start_frame == frame {
            return Keyframe {
                start_frame,
                cel: kf.cel.clone(),
                tween: kf.tween,
                tween_rotate: kf.tween_rotate,
            };
        }

        // 図形の列は元のキーフレームと共有する（このレイヤーのセルの図形は、もともと layer_id がこのレイヤー）。
        // 変えるのは、インスタンスの再生位置と、トゥイーンの中割りの姿勢だけ
        let mut cel = kf.cel.clone();
        if !cel.instances.is_empty() {
            let elapsed = frame.saturating_sub(kf.start_frame);
            let split_tween_binding = self.interpolated_tween_binding_at(frame);
            for inst in cel.instances_mut() {
                if let Some(interp_b) = split_tween_binding {
                    if inst.binding.instance_gid == interp_b.instance_gid {
                        inst.binding = interp_b;
                    }
                }
                inst.binding.advance(elapsed);
            }
        }
        let (tween, tween_rotate) = valid_tween_settings(kf.tween, kf.tween_rotate, &cel);
        Keyframe {
            start_frame,
            cel,
            tween,
            tween_rotate,
        }
    }

    /// 指定フレーム区間 `[min_frame..=max_frame]` 内のキーフレーム群を相対フレーム位置（`0..=max_frame - min_frame`）で抽出する
    /// （`min_frame` がキーフレーム途中やトゥイーン中割りフレームにある場合は、そのフレームの評価済み中割り姿勢を引き継ぐ）
    pub(crate) fn extract_keyframes_in_range(
        &self,
        min_frame: usize,
        max_frame: usize,
    ) -> Vec<Keyframe> {
        let mut kfs: Vec<Keyframe> = self
            .keyframes
            .iter()
            .filter(|kf| kf.start_frame >= min_frame && kf.start_frame <= max_frame)
            .map(|kf| Keyframe {
                start_frame: kf.start_frame - min_frame,
                cel: kf.cel.clone(),
                tween: kf.tween,
                tween_rotate: kf.tween_rotate,
            })
            .collect();

        if !kfs.iter().any(|kf| kf.start_frame == 0) {
            kfs.insert(0, self.snapshot_keyframe_at(min_frame, 0));
        }
        kfs
    }

    fn snapshot_post_range_keyframe(&self, max_frame: usize) -> Option<Keyframe> {
        let after = max_frame + 1;
        (after < self.frame_count && !self.keyframes.iter().any(|kf| kf.start_frame == after))
            .then(|| self.snapshot_keyframe_at(after, after))
    }

    /// 指定フレーム区間 `[min_frame..=max_frame]` を移動元から切り取って空白化し、
    /// 移動範囲直後（`max_frame + 1`）のフレーム姿勢を保持する境界キーフレームを自動挿入する
    pub(crate) fn remove_keyframes_in_range(&mut self, min_frame: usize, max_frame: usize) {
        let post_range_kf = self.snapshot_post_range_keyframe(max_frame);

        self.keyframes
            .retain(|kf| kf.start_frame < min_frame || kf.start_frame > max_frame);

        self.ensure_head_keyframe();
        if min_frame < self.frame_count {
            let prev_idx = self
                .keyframes
                .partition_point(|kf| kf.start_frame <= min_frame)
                .saturating_sub(1);
            if !self.keyframes[prev_idx].cel.is_empty() || self.keyframes[prev_idx].tween {
                let insert_pos = self
                    .keyframes
                    .partition_point(|kf| kf.start_frame < min_frame);
                self.keyframes
                    .insert(insert_pos, Arc::new(Keyframe::new_blank(min_frame)));
            }
        }

        if let Some(after_kf) = post_range_kf {
            if !after_kf.cel.is_empty() || after_kf.tween {
                let pos = self
                    .keyframes
                    .partition_point(|existing_kf| existing_kf.start_frame < after_kf.start_frame);
                self.keyframes.insert(pos, Arc::new(after_kf));
            }
        }
    }

    /// 抽出された相対キーフレーム群 `kfs` を `[dst_min_frame..=dst_max_frame]` に配置する
    /// （移動先範囲直後のフレーム姿勢を保持する境界キーフレームを自動挿入する）
    pub(crate) fn place_keyframes_at(
        &mut self,
        dst_min_frame: usize,
        dst_max_frame: usize,
        kfs: Vec<Keyframe>,
    ) {
        let post_range_kf = self.snapshot_post_range_keyframe(dst_max_frame);

        if dst_max_frame >= self.frame_count {
            self.frame_count = dst_max_frame + 1;
        }
        self.keyframes
            .retain(|kf| kf.start_frame < dst_min_frame || kf.start_frame > dst_max_frame);
        if let Some(after_kf) = post_range_kf {
            let pos = self
                .keyframes
                .partition_point(|existing_kf| existing_kf.start_frame < after_kf.start_frame);
            self.keyframes.insert(pos, Arc::new(after_kf));
        }
        let dst_lid = self.id;
        for mut kf in kfs {
            if kf.cel.instances.iter().any(|i| i.layer_id != dst_lid) {
                for inst in kf.cel.instances_mut() {
                    inst.layer_id = dst_lid;
                }
            }
            let (valid_tween, rotate_mode) =
                valid_tween_settings(kf.tween, kf.tween_rotate, &kf.cel);
            let target_f = dst_min_frame + kf.start_frame;
            if let Some(existing_idx) = self
                .keyframes
                .iter()
                .position(|existing_kf| existing_kf.start_frame == target_f)
            {
                let existing = self.keyframe_mut(existing_idx);
                existing.cel = kf.cel;
                existing.tween = valid_tween;
                existing.tween_rotate = rotate_mode;
            } else {
                let pos = self
                    .keyframes
                    .partition_point(|existing_kf| existing_kf.start_frame < target_f);
                self.keyframes.insert(
                    pos,
                    Arc::new(Keyframe {
                        start_frame: target_f,
                        cel: kf.cel,
                        tween: valid_tween,
                        tween_rotate: rotate_mode,
                    }),
                );
            }
        }
        self.ensure_head_keyframe();
    }

    /// `kf_idx` 番目のキーフレームにおけるトゥイーン状態（未設定 / 点線の未完トゥイーン / 実線矢印の完全トゥイーン）を判定する
    pub(crate) fn tween_state_at_keyframe(&self, kf_idx: usize) -> TweenState {
        let Some(kf) = self.keyframes.get(kf_idx) else {
            return TweenState::None;
        };
        if !kf.tween {
            return TweenState::None;
        }
        let Some(start_binding) = kf.cel.single_symbol_instance() else {
            return TweenState::Incomplete;
        };
        if let Some(next_kf) = self
            .keyframes
            .get(kf_idx + 1)
            .filter(|next| next.start_frame < self.frame_count && next.start_frame > kf.start_frame)
        {
            if let Some(end_binding) = next_kf
                .cel
                .single_symbol_instance()
                .filter(|b| b.symbol_id == start_binding.symbol_id)
            {
                return TweenState::Complete {
                    start_frame: kf.start_frame,
                    end_frame: next_kf.start_frame,
                    start_binding,
                    end_binding,
                    rotate_mode: kf.tween_rotate,
                };
            }
        }
        TweenState::Incomplete
    }

    /// 指定フレーム `frame` が完全トゥイーン（`TweenState::Complete`）の中間フレーム（`start_frame < frame < end_frame`）である場合、
    /// TRS補間後のバインディング（`instance_gid` は開始バインディングと同じ）を返す
    pub(crate) fn interpolated_tween_binding_at(
        &self,
        frame: usize,
    ) -> Option<SymbolInstanceBinding> {
        let kf_idx = self.keyframe_index_at(frame)?;
        match self.tween_state_at_keyframe(kf_idx) {
            TweenState::Complete {
                start_frame,
                end_frame,
                start_binding,
                end_binding,
                rotate_mode,
            } if frame > start_frame && frame < end_frame => {
                let t = (frame - start_frame) as f64 / (end_frame - start_frame) as f64;
                let (interp_xform, interp_pivot) = interpolate_affine_with_pivot(
                    start_binding.xform,
                    start_binding.pivot,
                    end_binding.xform,
                    end_binding.pivot,
                    rotate_mode,
                    t,
                );
                Some(SymbolInstanceBinding {
                    instance_gid: start_binding.instance_gid,
                    symbol_id: start_binding.symbol_id,
                    xform: interp_xform,
                    pivot: interp_pivot,
                    loop_mode: start_binding.loop_mode,
                    first_frame: start_binding.first_frame,
                    color_effect: start_binding.color_effect.lerp(end_binding.color_effect, t),
                })
            }
            _ => None,
        }
    }

    /// 指定フレーム `frame` においてこのレイヤーが編集可能（表示中・非ロック・フレーム作成済み・キーフレームあり・トゥイーンの途中ではない）であるかを返す
    pub(crate) fn is_editable_at(&self, frame: usize) -> bool {
        self.edit_block_at(frame).is_none()
    }

    /// コマ `frame` でこのレイヤーの絵を編集できない理由（編集できれば `None`）。編集できるかの決まりはこれ1つ
    /// （`is_editable_at`・画面の注意書き）
    pub(crate) fn edit_block_at(&self, frame: usize) -> Option<EditBlock> {
        if !self.visible || self.locked {
            Some(EditBlock::HiddenOrLocked)
        } else if frame >= self.frame_count || self.keyframes.is_empty() {
            Some(EditBlock::NoFrame)
        } else if self.interpolated_tween_binding_at(frame).is_some() {
            Some(EditBlock::TweenMiddle)
        } else {
            None
        }
    }

    /// タイムラインの操作（フレームの挿入・削除・移動、トゥイーンの設定）で、このレイヤーを変えてよいか（ロックして
    /// いない）。絵は書き換えないので、非表示のレイヤーも変える。この決まりはこれ1つ
    pub(crate) fn accepts_frame_ops(&self) -> bool {
        !self.locked
    }
}

/// レイヤーの絵を編集できない理由（`Layer::edit_block_at`）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditBlock {
    /// 非表示かロック中
    HiddenOrLocked,
    /// コマを作っていない（キーフレームが無い）
    NoFrame,
    /// トゥイーンの途中のコマ
    TweenMiddle,
}

/// レイヤー配列・現在のフレーム・セル範囲選択を管理するタイムライン構造体（永続ドキュメント状態）
#[derive(Clone, Debug)]
pub(crate) struct Timeline {
    pub(crate) layers: Vec<Layer>,
    pub(crate) current_frame: usize,
    pub(crate) active_layer: usize,
    pub(crate) total_frames: usize,
    pub(crate) next_layer_number: usize,
    pub(crate) next_layer_id: u64,
    pub(crate) cell_selection: Option<TimelineCellSelection>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            layers: vec![Layer::new_with_id(1, "Layer 1")],
            current_frame: 0,
            active_layer: 0,
            total_frames: 1,
            next_layer_number: 2,
            next_layer_id: 2,
            cell_selection: Some(TimelineCellSelection::single(0, 0)),
        }
    }
}

impl Timeline {
    /// 全レイヤーの `frame_count` の最大値からタイムライン全体の `total_frames` を再計算し、レイヤー選択インデックスを範囲内へ補正する
    pub(crate) fn sync_total_frames(&mut self) {
        self.total_frames = self
            .layers
            .iter()
            .map(|l| l.frame_count)
            .max()
            .unwrap_or(1)
            .max(1);
        let max_l = self.layers.len().saturating_sub(1);
        self.active_layer = self.active_layer.min(max_l);
        if let Some(sel) = &mut self.cell_selection {
            sel.min_layer = sel.min_layer.min(max_l);
            sel.max_layer = sel.max_layer.min(max_l);
        }
    }

    /// 指定した `(layer_idx, frame_idx)` をアクティブレイヤー・現在フレーム・単一セル選択状態に設定する
    /// （`frame_idx` は現在の `total_frames` より後ろの未作成フレーム番号も選択可能）
    pub(crate) fn select_single_cell(&mut self, layer_idx: usize, frame_idx: usize) {
        let clamped_layer = layer_idx.min(self.layers.len().saturating_sub(1));
        self.active_layer = clamped_layer;
        self.current_frame = frame_idx;
        self.cell_selection = Some(TimelineCellSelection::single(clamped_layer, frame_idx));
    }

    /// 現在アクティブなレイヤーの固有 `layer_id` を返す
    pub(crate) fn active_layer_id(&self) -> u64 {
        self.layers
            .get(self.active_layer)
            .map(|l| l.id)
            .unwrap_or(1)
    }

    /// 指定した `layer_id` を持つレイヤーのインデックス（`0..layers.len()`）を返す
    pub(crate) fn layer_index_by_id(&self, layer_id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == layer_id)
    }

    /// 指定した `layer_id` のレイヤーが現在フレームにおいて表示中かつ非ロック（編集可能）であるかを返す
    pub(crate) fn is_layer_editable_by_id(&self, layer_id: u64) -> bool {
        self.layers
            .iter()
            .any(|l| l.id == layer_id && l.is_editable_at(self.current_frame))
    }

    /// 現在フレームにおいて表示中かつ非ロック（編集可能）なレイヤーが1つ以上存在するかを返す
    pub(crate) fn has_any_editable_layer(&self) -> bool {
        self.layers
            .iter()
            .any(|l| l.is_editable_at(self.current_frame))
    }

    /// 全レイヤー・全キーフレーム内のシンボルインスタンスを列挙する
    pub(crate) fn instances(&self) -> impl Iterator<Item = &SymbolInstance> {
        self.layers
            .iter()
            .flat_map(|l| l.keyframes.iter())
            .flat_map(|kf| kf.cel.instances.iter())
    }

    /// `F5` / `Shift+F5` 等のフレーム操作対象となるレイヤーインデックス一覧を返す：
    /// - 特定のレイヤー（または複数レイヤー範囲）が選択されていれば、その選択レイヤー群のみ。
    /// - どのレイヤーも選択されていなければ（`cell_selection == None`）、全非ロックレイヤー。
    pub(crate) fn target_layers_for_frame_op(&self) -> Vec<usize> {
        if let Some(sel) = self.cell_selection {
            (sel.min_layer..=sel.max_layer)
                .filter(|&i| self.layers.get(i).is_some_and(|l| l.accepts_frame_ops()))
                .collect()
        } else {
            (0..self.layers.len())
                .filter(|&i| self.layers[i].accepts_frame_ops())
                .collect()
        }
    }

    /// 現在アクティブなレイヤー・フレームの `Cel` の面の図形（写し）
    #[cfg(test)]
    pub(crate) fn active_cel_shapes(&self) -> Vec<std::sync::Arc<FillShape>> {
        self.layers
            .get(self.active_layer)
            .and_then(|layer| layer.cel_at(self.current_frame))
            .map(|cel| cel.shapes())
            .unwrap_or_default()
    }

    /// レイヤー `preferred` が編集できればそれ、できなければ作業中のレイヤー、それもできなければ編集できる最初のレイヤー
    /// （新しい中身を置く先）
    pub(crate) fn editable_layer_or(&self, preferred: u64) -> u64 {
        if self.is_layer_editable_by_id(preferred) {
            return preferred;
        }
        if self.is_active_layer_editable() {
            return self.active_layer_id();
        }
        self.layers
            .iter()
            .find(|l| l.is_editable_at(self.current_frame))
            .map_or_else(|| self.active_layer_id(), |l| l.id)
    }

    /// 現在アクティブなレイヤーが現在フレームにおいて編集可能（表示中・非ロック・フレーム作成済み）であるかを返す
    pub(crate) fn is_active_layer_editable(&self) -> bool {
        self.layers
            .get(self.active_layer)
            .is_some_and(|l| l.is_editable_at(self.current_frame))
    }

    /// 対象レイヤー群 `targets` に対して現在フレーム（または直後フレーム）へキーフレームを挿入する（`F6` / `F7`）
    pub(crate) fn insert_keyframe_on_targets(
        &mut self,
        targets: &[usize],
        clone_prev: bool,
    ) -> bool {
        let curr_f = self.current_frame;
        let should_advance = if clone_prev {
            targets.iter().all(|&l_idx| {
                self.layers
                    .get(l_idx)
                    .is_some_and(|l| l.keyframes.iter().any(|kf| kf.start_frame == curr_f))
            })
        } else {
            targets.iter().any(|&l_idx| {
                self.layers.get(l_idx).is_some_and(|l| {
                    l.keyframes
                        .iter()
                        .any(|kf| kf.start_frame == curr_f && !kf.cel.is_empty())
                })
            })
        };

        let target_frame = if should_advance { curr_f + 1 } else { curr_f };
        let shift_for_blank = !clone_prev
            && should_advance
            && targets.iter().any(|&l_idx| {
                self.layers.get(l_idx).is_some_and(|l| {
                    l.keyframes
                        .iter()
                        .any(|kf| kf.start_frame == target_frame && !kf.cel.is_empty())
                })
            });

        let mut changed = should_advance || shift_for_blank;
        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                if shift_for_blank {
                    layer.insert_frame_at(curr_f);
                }
                changed |= layer.insert_keyframe(target_frame, clone_prev);
            }
        }
        self.sync_total_frames();
        self.current_frame = target_frame;
        if let Some(sel) = &mut self.cell_selection {
            sel.min_frame = target_frame;
            sel.max_frame = target_frame;
        }
        changed
    }

    /// 対象レイヤー群 `targets` の現在フレームに削除可能なキーフレームが存在するかを返す
    pub(crate) fn can_delete_keyframe_on_targets(&self, targets: &[usize]) -> bool {
        let curr_f = self.current_frame;
        targets.iter().any(|&l_idx| {
            self.layers.get(l_idx).is_some_and(|layer| {
                layer.keyframe_index_at(curr_f).is_some_and(|idx| {
                    let kf = &layer.keyframes[idx];
                    kf.start_frame == curr_f && (idx > 0 || !kf.cel.is_empty() || kf.tween)
                })
            })
        })
    }

    /// 対象レイヤー群 `targets` の現在フレームにあるキーフレームを削除（または空白化）する（`Shift+F6`）
    pub(crate) fn delete_keyframe_on_targets(&mut self, targets: &[usize]) {
        let curr_f = self.current_frame;
        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                layer.delete_keyframe_at(curr_f);
            }
        }
    }

    /// 対象レイヤー群 `targets` の現在フレーム（または選択終端）に保持尺を挿入する（`F5`）
    pub(crate) fn extend_frame_on_targets(&mut self, targets: &[usize]) {
        let target_f = self
            .cell_selection
            .map(|s| s.max_frame.max(self.current_frame))
            .unwrap_or(self.current_frame);
        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                layer.insert_frame_at(target_f);
            }
        }
        self.sync_total_frames();
    }

    /// 対象レイヤー群 `targets` の現在フレーム（または範囲選択の先頭）に短縮可能なフレームが存在するかを返す
    pub(crate) fn can_remove_frame_on_targets(&self, targets: &[usize]) -> bool {
        let start_f = self
            .cell_selection
            .filter(|s| s.max_frame > s.min_frame)
            .map_or(self.current_frame, |s| s.min_frame);
        targets.iter().any(|&l_idx| {
            self.layers.get(l_idx).is_some_and(|l| {
                start_f < l.frame_count
                    && (l.frame_count > 1
                        || l.keyframes
                            .first()
                            .is_some_and(|kf| !kf.cel.is_empty() || kf.tween))
            })
        })
    }

    /// 対象レイヤー群 `targets` のフレーム（範囲選択時は選択範囲まとめて、単一選択時は1フレーム）を削除する（`Shift+F5`）
    pub(crate) fn remove_frame_on_targets(&mut self, targets: &[usize]) -> bool {
        let range_sel = self.cell_selection.filter(|s| s.max_frame > s.min_frame);
        let mut any_changed = false;
        if let Some(sel) = range_sel {
            for &l_idx in targets {
                if let Some(layer) = self.layers.get_mut(l_idx) {
                    if sel.min_frame < layer.frame_count {
                        let count = sel.max_frame.min(layer.frame_count - 1) - sel.min_frame + 1;
                        for _ in 0..count {
                            if layer.remove_frame_at(sel.min_frame) {
                                any_changed = true;
                            }
                        }
                    }
                }
            }
            self.sync_total_frames();
            let next_f = sel.min_frame.min(self.total_frames.saturating_sub(1));
            self.current_frame = next_f;
            if let Some(s) = &mut self.cell_selection {
                s.min_frame = next_f;
                s.max_frame = next_f;
            }
        } else {
            let curr_f = self.current_frame;
            for &l_idx in targets {
                if let Some(layer) = self.layers.get_mut(l_idx) {
                    if layer.remove_frame_at(curr_f) {
                        any_changed = true;
                    }
                }
            }
            self.sync_total_frames();
            self.current_frame = self.current_frame.min(self.total_frames.saturating_sub(1));
            if let Some(sel) = &mut self.cell_selection {
                sel.min_frame = sel.min_frame.min(self.current_frame);
                sel.max_frame = sel.max_frame.min(self.current_frame);
            }
        }
        any_changed
    }

    /// フレーム移動・複製の移動先範囲 `(dst_sel, actual_dl)` を検証・算出する（移動量が0またはロックレイヤーを含む場合は `None`）
    pub(crate) fn resolve_frame_move_bounds(
        &self,
        sel: TimelineCellSelection,
        delta_layer: isize,
        delta_frame: isize,
    ) -> Option<(TimelineCellSelection, isize)> {
        let num_layers = self.layers.len();
        if num_layers == 0 {
            return None;
        }
        let span_layers = sel.max_layer.saturating_sub(sel.min_layer);
        let span_frames = sel.max_frame.saturating_sub(sel.min_frame);

        let max_min_layer = num_layers.saturating_sub(1 + span_layers);
        let dst_min_layer =
            (sel.min_layer as isize + delta_layer).clamp(0, max_min_layer as isize) as usize;
        let dst_max_layer = dst_min_layer + span_layers;
        let actual_dl = dst_min_layer as isize - sel.min_layer as isize;

        let dst_min_frame = (sel.min_frame as isize + delta_frame).max(0) as usize;
        let dst_max_frame = dst_min_frame + span_frames;
        let actual_df = dst_min_frame as isize - sel.min_frame as isize;

        if actual_dl == 0 && actual_df == 0 {
            return None;
        }
        for src_l in sel.min_layer..=sel.max_layer {
            let dst_l = (src_l as isize + actual_dl) as usize;
            let src_layer = self.layers.get(src_l)?;
            let dst_layer = self.layers.get(dst_l)?;
            if !src_layer.accepts_frame_ops() || !dst_layer.accepts_frame_ops() {
                return None;
            }
        }
        Some((
            TimelineCellSelection {
                min_layer: dst_min_layer,
                max_layer: dst_max_layer,
                min_frame: dst_min_frame,
                max_frame: dst_max_frame,
            },
            actual_dl,
        ))
    }

    /// 検証済みの移動先 `dst_sel` へ選択範囲 `sel` のキーフレーム群を移動または複製する
    pub(crate) fn move_or_copy_frames(
        &mut self,
        sel: TimelineCellSelection,
        dst_sel: TimelineCellSelection,
        actual_dl: isize,
        copy_mode: bool,
        mut alloc_group_id: impl FnMut() -> u64,
    ) {
        let extracted: Vec<(usize, usize, usize, usize, Vec<Keyframe>)> = (sel.min_layer
            ..=sel.max_layer)
            .filter_map(|src_l| {
                let dst_l = (src_l as isize + actual_dl) as usize;
                let _ = self.layers.get(dst_l)?;
                let src_layer = self.layers.get(src_l)?;
                if sel.min_frame >= src_layer.frame_count {
                    return None;
                }
                let src_max_frame = sel.max_frame.min(src_layer.frame_count - 1);
                let dst_max_frame = dst_sel.min_frame + (src_max_frame - sel.min_frame);
                let mut kfs = src_layer.extract_keyframes_in_range(sel.min_frame, src_max_frame);
                // 番号を振り直すのは、別のレイヤーへ移す・複製するときだけ。同じレイヤーの別のコマは
                // 同時に表示されないので、F6 と同じく番号のまま置き、セルを元と共有する
                if actual_dl != 0 {
                    remap_keyframes_group_and_instance_ids(&mut kfs, &mut alloc_group_id);
                }
                Some((src_l, dst_l, src_max_frame, dst_max_frame, kfs))
            })
            .collect();

        if !copy_mode {
            for &(src_l, _, src_max_frame, _, _) in &extracted {
                if let Some(layer) = self.layers.get_mut(src_l) {
                    layer.remove_keyframes_in_range(sel.min_frame, src_max_frame);
                }
            }
        }

        for (_, dst_l, _, dst_max_frame, kfs) in extracted {
            if let Some(layer) = self.layers.get_mut(dst_l) {
                layer.place_keyframes_at(dst_sel.min_frame, dst_max_frame, kfs);
            }
        }

        self.sync_total_frames();
        self.active_layer = dst_sel.max_layer;
        self.current_frame = dst_sel.min_frame;
        self.cell_selection = Some(dst_sel);
    }

    /// 新しいレイヤーを現在のアクティブレイヤーの1つ上（前面）に追加して選択する（上限 `MAX_LAYERS_PER_TIMELINE` を超える場合は追加しない）
    pub(crate) fn add_layer_above_active(&mut self) {
        if self.layers.len() >= MAX_LAYERS_PER_TIMELINE {
            return;
        }
        let name = format!("Layer {}", self.next_layer_number);
        self.next_layer_number = self.next_layer_number.saturating_add(1);
        let mut lid = self.next_layer_id.max(1);
        while self.layers.iter().any(|l| l.id == lid) {
            lid = if lid < u64::MAX { lid + 1 } else { 1 };
        }
        self.next_layer_id = lid.saturating_add(1);
        let insert_idx = (self.active_layer + 1).min(self.layers.len());
        let fc = self.total_frames.max(1);
        self.layers
            .insert(insert_idx, Layer::new_with_id_and_frames(lid, name, fc));
        self.select_single_cell(insert_idx, self.current_frame);
    }

    /// 現在アクティブなレイヤーを削除し、総フレーム数と選択セルを再同期する（最低1レイヤーは維持する）
    pub(crate) fn delete_active_layer(&mut self) {
        if self.layers.len() <= 1 || self.active_layer >= self.layers.len() {
            return;
        }
        self.layers.remove(self.active_layer);
        self.sync_total_frames();
        self.select_single_cell(self.active_layer, self.current_frame);
    }

    /// 指定レイヤー `from_idx` を `to_idx` の重ね順位置へ移動して選択する
    pub(crate) fn reorder_layer(&mut self, from_idx: usize, to_idx: usize) {
        if from_idx >= self.layers.len() || to_idx >= self.layers.len() {
            return;
        }
        let layer = self.layers.remove(from_idx);
        self.layers.insert(to_idx, layer);
        self.select_single_cell(to_idx, self.current_frame);
    }

    /// 現在のフレーム・選択レイヤーにおいてクラシックトゥイーンの切替が可能か `(切替可能か, 現在トゥイーンONか)` を返す
    pub(crate) fn can_toggle_tween_at_current_frame(&self) -> (bool, bool) {
        let targets = self.target_layers_for_frame_op();
        if targets.is_empty() {
            return (false, false);
        }
        let curr_f = self.current_frame;
        let mut any_tweened = false;
        let mut any_can_enable = false;
        for &l_idx in &targets {
            let layer = &self.layers[l_idx];
            if let Some(kf_idx) = layer.keyframe_index_at(curr_f) {
                let kf = &layer.keyframes[kf_idx];
                if kf.tween {
                    any_tweened = true;
                } else if kf.cel.single_symbol_instance().is_some() {
                    any_can_enable = true;
                }
            }
        }
        (any_tweened || any_can_enable, any_tweened)
    }

    /// 現在選択されているアクティブレイヤー・現在フレームが属するキーフレームがクラシックトゥイーン有効である場合、
    /// `(キーフレーム開始フレーム 0-indexed, 現在の回転モード TweenRotate, 完全トゥイーンか)` を返す
    pub(crate) fn active_tween_keyframe_info(&self) -> Option<(usize, TweenRotate, bool)> {
        let layer = self.layers.get(self.active_layer)?;
        let kf_idx = layer.keyframe_index_at(self.current_frame)?;
        let kf = layer.keyframes.get(kf_idx)?;
        if !kf.tween || kf.cel.single_symbol_instance().is_none() {
            return None;
        }
        let is_complete = matches!(
            layer.tween_state_at_keyframe(kf_idx),
            TweenState::Complete { .. }
        );
        Some((kf.start_frame, kf.tween_rotate, is_complete))
    }
}
