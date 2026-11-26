use crate::planar_map::{realized, CheckedMap, FaceKeep, LayerBase, PlanarMap};
use crate::stored_map::{Relabeled, StoredMap};
use crate::vector_engine::ShapeId;
use crate::vector_engine::{FillShape, SymbolInstanceBinding};
use kurbo::{Affine, Point};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock, PoisonError, Weak,
    },
};

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
    // 向きが同じなら、回す向きを決めたトゥイーンは1回転する。同じ向きかは、計算の丸めの差だけなら同じとみなす決まり
    // `SAME_TOL` で決める（角度の差（ラジアン）は、拡大率 1 の変換の係数の差と同じ桁。レビュー de92ceb の N-2002）
    let same_angle = cw_delta <= crate::vector_engine::SAME_TOL
        || std::f64::consts::TAU - cw_delta <= crate::vector_engine::SAME_TOL;
    let delta_rot = match rotate_mode {
        TweenRotate::Auto => {
            if cw_delta > std::f64::consts::PI {
                cw_delta - std::f64::consts::TAU
            } else {
                cw_delta
            }
        }
        TweenRotate::Clockwise => {
            if same_angle {
                std::f64::consts::TAU
            } else {
                cw_delta
            }
        }
        TweenRotate::CounterClockwise => {
            if same_angle {
                -std::f64::consts::TAU
            } else {
                cw_delta - std::f64::consts::TAU
            }
        }
    };
    let rot = rot0 + delta_rot * t;
    // 反転のトゥイーンでは、途中で拡大率が 0 を通る（そのコマの変換は潰れていて、インスタンスは描かない：
    // `SymbolInstanceBinding::instantiate_shapes`）
    let sx = sx0 + (sx1 - sx0) * t;
    let sy = sy0 + (sy1 - sy0) * t;
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

/// 2D アフィン変換から回転・拡大縮小・せん断 `(rot, sx, sy, shear)` を取り出す（平行移動はトゥイーンでピボットから作り直すので返さない）。
/// 向きは横の列 `(a, b)` = sx·(cos, sin) から決める。横の列が潰れているとき（反転のトゥイーンの真ん中のコマなど。長さ
/// `TINY_LEN` 以下）は、残った縦の列 `(c, d)` = sy·(−sin, cos) から決める（斜めは 0 とみなす。斜めをかけたまま反転した
/// ときは、潰れた行列から斜めと回転を見分ける手がかりが無い）。両方とも潰れていれば向きは決まらないので 0。前は横の列が
/// 潰れると回転を 0 と決め打ちし、本当の回転が斜めと縦の拡大に入ったので、そのコマで分けた（F6）トゥイーンの後ろの動きが
/// 変わった（レビュー 41766f9 の N-2002）
fn decompose_affine_trs(affine: Affine) -> (f64, f64, f64, f64) {
    let [a, b, c, d, _, _] = affine.as_coeffs();
    let sx = a.hypot(b);
    let len_y = c.hypot(d);
    let (cos_r, sin_r, rot) = if sx > crate::vector_engine::TINY_LEN {
        (a / sx, b / sx, b.atan2(a))
    } else if len_y > crate::vector_engine::TINY_LEN {
        (d / len_y, -c / len_y, (-c).atan2(d))
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

/// セル画の1つの階層の地図：塊の番号の並び（文書の塊の表 `ProjectDocument::map_tiles` の番号。升目の順）。塊はしまう形
/// （`StoredMap`）の塊そのもので、側の図形の番号も持つ
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedScopeMap {
    pub(crate) group_path: Vec<u64>,
    pub(crate) tiles: Vec<u32>,
    /// しまう形（書かない。書くときは塊の番号の並びを作るもと、読むときは塊の番号の並びから作ったもの：
    /// `ProjectDocument::of`・`into_document`）
    #[serde(skip)]
    pub(crate) stored: Option<StoredMap>,
}

/// セル画の1つの階層の地図
#[derive(Clone, Debug)]
pub(crate) struct ScopeMap {
    pub(crate) group_path: Vec<u64>,
    /// 地図（場所ごとの塊。変わらない塊は Undo の控えと共有する）。面の辺の側に、面ごとに1つずつ別の図形の番号を記して
    /// ある（面の図形の番号になる）。ファイルから読んだ地図は、正しいかを、初めて使うときに確かめる
    /// （`ScopeMap::stored`）。写し（Undo の控え・コピー）どうしで共有するので、確かめるのは1回
    map: Arc<LazyStored>,
}

/// 面を外した・残した地図と、外したときの新しい写し（`ScopeMap::restricted`）
type Restricted = (ScopeMap, Option<Arc<LayerBase>>);

/// セル画の1つの階層の地図の中身：しまった地図と、確かめた地図を入れる1回だけ書ける箱（設計：
/// claude/地図の確かめを使うときに行う設計_開く時間_0fe3fcb.md。版 7 からは、ファイルから読んだ地図もはじめからしまう形
/// で持つ）
#[derive(Debug)]
struct LazyStored {
    /// しまった地図（ファイルから読んだ地図は、読んだまま。壊れていても保存のために残す）
    stored: StoredMap,
    /// 確かめた地図（まだ確かめていなければ空。正しい地図なら、面に1つずつ別の番号を記した地図：アプリが書いた地図は
    /// みなそうなので、ふつうは `stored` と同じ地図。壊れていれば `None`。編集で作った地図は、はじめから確かめてある）
    checked: OnceLock<Option<StoredMap>>,
    /// 壊れていたときの知らせ先と、その場所（読み込みのあとに付ける：`Document::watch_broken_maps`）
    notice: OnceLock<(BrokenMaps, String)>,
}

/// 図形の番号をみな新しい番号にする替え方（1回の作業（キーフレームの移動・複製）で1つ。グループの番号の `gid_map` と
/// 同じ）：番号ごとの新しい番号（同じ番号は同じ新しい番号）と、作り直した地図の控え（元の地図の番地 → 元の地図と新しい
/// 地図。元の地図も持つので、作業の間は番地がほかの地図に使い回されない）と、作り直した塊の控え。共有していた地図・塊は、
/// 替えたあとも共有するので、運んだ先でも増えるのは中身の違う地図の分だけ（レビュー e90f27c の N-2002）
#[derive(Default)]
pub(crate) struct ShapeIdRenewal {
    fresh: HashMap<ShapeId, ShapeId>,
    maps: HashMap<usize, (Arc<LazyStored>, Arc<LazyStored>)>,
    tiles: Relabeled,
}

/// 壊れていた地図の代わりに使う、何も無い地図
fn empty_stored() -> &'static StoredMap {
    static EMPTY: OnceLock<StoredMap> = OnceLock::new();
    EMPTY.get_or_init(|| StoredMap::from_dense(&PlanarMap::default(), None))
}

impl LazyStored {
    /// 確かめてしまった地図 `stored`
    fn checked(stored: StoredMap) -> Arc<Self> {
        Arc::new(Self {
            stored: stored.clone(),
            checked: OnceLock::from(Some(stored)),
            notice: OnceLock::new(),
        })
    }

    /// 使う地図（確かめる入口はこれ1つ）。まだなら確かめ、壊れていれば何も無い地図にして、知らせる
    fn get(&self) -> &StoredMap {
        self.checked
            .get_or_init(|| self.check())
            .as_ref()
            .unwrap_or_else(|| empty_stored())
    }

    /// 確かめた地図：しまった地図の計算に使う写し（`LayerBase::of_stored`。持ち主（画面の土台など）が持っていればそれ）で
    /// 確かめる（`LayerBase::checked_stored`。受け入れの判定は `PlanarMap::checked` と同じ1つ）。正しければ、面に1つずつ
    /// 別の番号を記した地図（番号が変わらなければ、しまった地図そのもの）。確かめに作った判定の形・輪郭・面は写しに残る
    /// ので、写しの持ち主はそのまま使える（`ScopeMap::base`）。壊れていれば `None` にして知らせる
    fn check(&self) -> Option<StoredMap> {
        let checked = LayerBase::of_stored(&self.stored).checked_stored();
        if checked.is_none() {
            if let Some((sink, at)) = self.notice.get() {
                sink.push(at.clone());
            }
        }
        checked
    }
}

/// まだ確かめていない地図を、コマの順に確かめる仕事（開いたあと、裏のスレッドで行う：`Document::check_maps_in_background`）。
/// 地図は弱い参照で持つので、閉じた文書の地図や、編集で置き換わって Undo の控えにも無い地図は確かめない
pub(crate) struct UncheckedMaps(Vec<Weak<LazyStored>>);

impl UncheckedMaps {
    /// タイムライン `timelines` の、まだ確かめていない地図（タイムラインの順。タイムラインの中は、キーフレームの始まりの
    /// コマの順で、同じコマならレイヤーの順）
    pub(crate) fn of<'a>(timelines: impl IntoIterator<Item = &'a Timeline>) -> Self {
        let mut maps = Vec::new();
        for t in timelines {
            let mut at: Vec<(usize, Weak<LazyStored>)> = t
                .layers
                .iter()
                .flat_map(|l| l.keyframes_with_start())
                .flat_map(|(start, kf)| {
                    kf.cel
                        .maps
                        .iter()
                        .filter(|m| m.map.checked.get().is_none())
                        .map(move |m| (start, Arc::downgrade(&m.map)))
                })
                .collect();
            at.sort_by_key(|(frame, _)| *frame);
            maps.extend(at.into_iter().map(|(_, m)| m));
        }
        Self(maps)
    }

    /// 確かめる（使うときと同じ入口 `LazyStored::get` を通るので、同じ地図を同時に使えば、片方が待つだけ。壊れていれば
    /// 知らせる）。まだ残っていて確かめた地図の数。確かめ1回の手間は辺の数に比例するので（レビュー 96fdc26 の N-6001
    /// で測った）、確かめ終わるまでの時間は、スレッドを増やして短くする：このスレッドと、コア数から画面のスレッドの分の
    /// 1つを引いた数まで（作品の量に比例させない）のスレッドが、並びの前から1枚ずつ取って確かめる（取るのはコマの順）
    pub(crate) fn check(self) -> usize {
        let next = AtomicUsize::new(0);
        let checked = AtomicUsize::new(0);
        let work = || loop {
            let Some(m) = self.0.get(next.fetch_add(1, Ordering::Relaxed)) else {
                break;
            };
            if let Some(m) = m.upgrade() {
                m.get();
                checked.fetch_add(1, Ordering::Relaxed);
            }
        };
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(1))
            .clamp(1, self.0.len().max(1));
        std::thread::scope(|s| {
            // 作れなかったスレッドの分は、作れたスレッドが取って確かめる
            for _ in 1..threads {
                let _ = std::thread::Builder::new()
                    .name("is2d-map-check".to_owned())
                    .spawn_scoped(s, work);
            }
            work();
        });
        checked.into_inner()
    }
}

/// ファイルから読んだ地図のうち、使うときに確かめて壊れていたものの場所の知らせ。文書ごとに1つ（`Document::broken_maps`）
#[derive(Clone, Debug, Default)]
pub(crate) struct BrokenMaps(Arc<Mutex<Vec<String>>>);

impl BrokenMaps {
    fn push(&self, at: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(at);
    }

    /// まだ知らせていない場所（取り出すと空になる）
    pub(crate) fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl ScopeMap {
    /// 階層 `group_path` の、確かめた地図 `map` の地図（面に1つずつ別の番号を記してしまう：`CheckedMap::uniquely_labeled`）。
    /// 塊は `prev` と共有する。面が無ければ `None`。セル画の地図を作るのは、これと `unchecked`・`restricted`・
    /// `renew_labels` だけなので、セル画の地図は、使うときにはいつも正しい地図で、面に1つずつ別の番号が記してある
    /// （`unchecked` の地図は使うときに確かめ、壊れていれば何も無い地図）
    fn new(group_path: Vec<u64>, map: CheckedMap, prev: Option<&StoredMap>) -> Option<ScopeMap> {
        Self::of_checked_stored(group_path, map.into_stored(prev))
    }

    /// 階層 `group_path` の、確かめた地図のしまう形 `stored`（`CheckedMap::into_stored`）の地図。面が無ければ `None`
    fn of_checked_stored(group_path: Vec<u64>, stored: StoredMap) -> Option<ScopeMap> {
        (!stored.is_empty()).then(|| ScopeMap {
            group_path,
            map: LazyStored::checked(stored),
        })
    }

    /// ファイルから読んだ、まだ確かめていない地図 `stored`（階層 `group_path`。使うときに確かめる：`stored`）。
    /// 辺が無ければ `None`（面が無い）
    fn unchecked(group_path: Vec<u64>, stored: StoredMap) -> Option<ScopeMap> {
        (!stored.is_empty()).then(|| ScopeMap {
            group_path,
            map: Arc::new(LazyStored {
                stored,
                checked: OnceLock::new(),
                notice: OnceLock::new(),
            }),
        })
    }

    /// 地図が無い（面が無い）。まだ確かめていない地図と、確かめたら壊れていた地図（保存のために残す）は、空でない。確かめ
    /// させない
    fn is_empty(&self) -> bool {
        self.map.stored.is_empty()
    }

    /// 保存する形（`SavedScopeMap`。塊の番号の並びは `ProjectDocument::of` が作る）。まだ確かめていない地図と、壊れて
    /// いた地図も、しまった地図をそのまま書く（確かめさせない）。番号の列は、いまの `group_path`（読み込みで振り直して
    /// いることがある）
    pub(crate) fn saved_form(&self) -> SavedScopeMap {
        let stored = match self.map.checked.get() {
            Some(Some(checked)) => checked,
            _ => &self.map.stored,
        };
        SavedScopeMap {
            group_path: self.group_path.clone(),
            tiles: Vec::new(),
            stored: Some(stored.clone()),
        }
    }

    /// 確かめたら壊れていた（何も無い地図として使っている。保存では読んだままの地図を書く）
    #[cfg(test)]
    pub(crate) fn is_broken(&self) -> bool {
        matches!(self.map.checked.get(), Some(None))
    }

    /// まだ確かめていない
    #[cfg(test)]
    pub(crate) fn is_unchecked(&self) -> bool {
        self.map.checked.get().is_none()
    }

    /// 壊れていたときの知らせ先 `sink` と場所 `at` を付ける（まだ確かめていない地図だけ。読み込みのあと）
    fn watch(&self, sink: &BrokenMaps, at: impl FnOnce() -> String) {
        if self.map.checked.get().is_none() {
            let _ = self.map.notice.set((sink.clone(), at()));
        }
    }

    /// 面を丸ごと外した・残した（`keep`）、階層 `group_path` の地図（面が無くなれば `None`）と、外したときは新しい地図の
    /// 計算に使う写し（面の表と升目の索引を前の写しから引き継いだもの。画面の土台が持つ：`Cel::remove` など）。外すのは
    /// この地図の書き換えなので、外したしまう形（`LayerBase::restricted_local`）を、この地図に対する差分として1本の道に
    /// 渡す（`replace_local`）。残すのは取り出し（コピーと同じく、正しい地図の一部を新しく置く。塊はこの地図と共有する）。
    /// `Err`：1本の道が書けないと答えた（正しい地図の面を外しても正しいので、起きないはず。使う側は何も変えずに断る）
    fn restricted(
        &self,
        group_path: &[u64],
        keep: FaceKeep<'_>,
    ) -> Result<Option<Restricted>, crate::planar_map::LocalRefusal> {
        use crate::planar_map::LocalEdit;
        let base = self.base();
        let Some(next) = base.restricted_local(keep, &self.group_path) else {
            return Ok(None);
        };
        match keep {
            FaceKeep::Without(_) => match base.replace_local(&next, &self.group_path) {
                LocalEdit::Same => Ok(Some((
                    ScopeMap {
                        group_path: group_path.to_vec(),
                        map: self.map.clone(),
                    },
                    None,
                ))),
                LocalEdit::Changed(w) => {
                    let Some(m) = ScopeMap::of_checked_stored(group_path.to_vec(), w.stored) else {
                        return Ok(None);
                    };
                    let held = LayerBase::of_local(m.stored().clone(), group_path, w.faces, &base);
                    Ok(Some((m, Some(held))))
                }
                LocalEdit::Refused(r) => Err(r),
            },
            FaceKeep::Only(_) => {
                Ok(ScopeMap::of_checked_stored(group_path.to_vec(), next).map(|m| (m, None)))
            }
        }
    }

    /// 面の番号を、番号ごとにみな新しい番号にする（同じ番号は同じ新しい番号。番号は面の形を変えないので、正しい地図の
    /// まま）。しまった地図の辺の番号を直接替えるので、計算に使う地図を作らず、まだ確かめていない地図も確かめない
    /// （レビュー f01d518 の N-2001）。壊れていたときの知らせ先と場所は元のまま（壊れた地図を読んだ所）。替え方 `r` は
    /// 1回の作業で1つなので、同じ地図は同じ新しい地図になる（`ShapeIdRenewal`）
    fn renew_labels(&mut self, r: &mut ShapeIdRenewal) {
        let ShapeIdRenewal { fresh, maps, tiles } = r;
        let old = &self.map;
        let made = maps.entry(Arc::as_ptr(old) as usize).or_insert_with(|| {
            let mut renew = |m: &StoredMap| {
                m.relabeled(
                    |id| {
                        *fresh
                            .entry(id)
                            .or_insert_with(crate::vector_engine::next_shape_id)
                    },
                    tiles,
                )
            };
            // 正しいかは番号で変わらないので、確かめた結果も知らせ先も引き継ぐ（まだ確かめていなければ確かめない）
            let (stored, checked) = match old.checked.get() {
                Some(Some(checked)) => {
                    let m = renew(checked);
                    (m.clone(), OnceLock::from(Some(m)))
                }
                Some(None) => (renew(&old.stored), OnceLock::from(None)),
                None => (renew(&old.stored), OnceLock::new()),
            };
            let notice = old.notice.get().cloned();
            let notice = notice.map_or_else(OnceLock::new, OnceLock::from);
            (old.clone(), Arc::new(LazyStored { stored, checked, notice }))
        });
        self.map = made.1.clone();
    }

    /// しまった地図（地図を取り出す入口はこれ1つ。まだ確かめていなければ、ここで確かめる：`LazyStored::get`）
    pub(crate) fn stored(&self) -> &StoredMap {
        self.map.get()
    }

    /// 同じ地図か（同じ中身の箱を共有する、同じ階層の地図。先読みの一覧を見分ける：`prefetch::Prefetch`）
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.map, &other.map) && self.group_path == other.group_path
    }

    /// 計算に使う写し（持ち主（画面の土台など）が持っていればそれ、無ければ塊から作る：`LayerBase::of_stored`）。まだ
    /// 確かめていない地図は、しまった地図の写しを先に作って持ってから確かめる（`LazyStored::check` がこの写しで確かめる
    /// ので、確かめに作った判定の形・輪郭・面を、この写しでそのまま使う。正しい地図は、ふつう、しまった地図そのもの）
    pub(crate) fn base(&self) -> Arc<LayerBase> {
        let _read = LayerBase::of_stored(&self.map.stored);
        LayerBase::of_stored(self.stored())
    }

    /// 計算に使うふつうの地図（その場で作って捨てる。面の図形を使わない所）
    pub(crate) fn dense(&self) -> PlanarMap {
        self.stored().to_dense()
    }

    /// 描く図形：面の図形を全部並べたあとに、線の図形を並べたもの（`LayerBase::drawn_shapes`。線は塗りより上）
    pub(crate) fn drawn_shapes(&self) -> Vec<Arc<FillShape>> {
        self.base().drawn_shapes(&self.group_path)
    }
}

/// 地図の編集（`Cel::edit_local`）の結果：書いた階層の新しい地図の、計算に使う写し（階層の面が無くなって外したときは
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

/// 地図にできない形（格子に置けない座標：±65,536px の外か、有限でない座標）か、数が有限でないインスタンスがあり、セル画に
/// 書けない
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
    let base = LayerBase::of(&refs).ok_or(Unmappable)?;
    let same_path = old.iter().find(|m| m.group_path == path);
    let map = base.map().clone().checked().ok_or(Unmappable)?;
    Ok(ScopeMap::new(
        path.to_vec(),
        map,
        same_path.map(ScopeMap::stored),
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

    /// ファイルから読んだ、階層ごとの地図 `maps`（下から順。同じ階層は2つ無い。しまう形は塊の表から作ったもの）とインスタンス
    /// （重なり位置 `order` つき）から作る（ファイルを読むとき）。地図は確かめず、初めて使うときに確かめる
    /// （`ScopeMap::stored`）
    pub(crate) fn from_saved(maps: Vec<SavedScopeMap>, mut instances: Vec<SymbolInstance>) -> Self {
        let maps: Vec<ScopeMap> = maps
            .into_iter()
            .filter_map(|m| ScopeMap::unchecked(m.group_path, m.stored?))
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

    /// 描く生の図形（写し）：階層の地図の順に、各地図の描く図形（面の図形と線の図形：`ScopeMap::drawn_shapes`）を並べた
    /// もの。インスタンスは入れない
    pub(crate) fn shapes(&self) -> Vec<Arc<FillShape>> {
        self.maps.iter().flat_map(|m| m.drawn_shapes()).collect()
    }

    /// 重なりの順の中身（図形とインスタンス）
    pub(crate) fn parts(&self) -> Vec<Part> {
        let mut parts = Vec::new();
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(_, m) => parts.extend(m.drawn_shapes().into_iter().map(Part::Shape)),
                Item::Instance(inst) => parts.push(Part::Instance(inst)),
            }
        }
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
    /// （中身の生の図形の地図は `insert_content` が断り、ファイルの同じ階層の地図は読み込み（`Keyframe::from_saved`）が断る。
    /// ほかの中身は、1枚のセル画の地図（階層ごとに1つ）から作る）
    fn arrange(&mut self, items: Vec<Item<ScopeMap>>) {
        let Ok((maps, instances)) = arranged(items, |_, mut maps: Vec<ScopeMap>| {
            Ok::<_, std::convert::Infallible>(maps.pop().filter(|m| !m.is_empty()))
        });
        self.put(maps, instances);
    }

    /// 地図とインスタンスを入れる。地図は、使うときにはいつも正しい地図で、面に1つずつ別の番号が記してある（`ScopeMap` を
    /// 作れるのは `ScopeMap::new`（確かめた地図から）・`unchecked`（使うときに確かめる）・`restricted`・`renew_labels`
    /// だけ）。並びはいつも決まりどおり（`maps` と `instances` は `arranged` の結果）なので、確かめ直さない
    fn put(&mut self, mut maps: Vec<ScopeMap>, mut instances: Vec<SymbolInstance>) {
        maps.shrink_to_fit();
        instances.shrink_to_fit();
        self.maps = Arc::new(maps);
        self.instances = Arc::new(instances);
        self.revision = crate::vector_engine::next_mesh_revision();
    }

    /// 階層 `scope`（とその中）の、番号が `faces` の面を地図から外し（外した所は何も無い所になる）、番号が `instances` の
    /// インスタンスを外す（削除・シンボル化）。面を外した地図の新しい写しは `held` に入れる（画面の土台が持つ）。`Ok(false)`：
    /// 外すものが無い。`Err`：書けない（何も変えない。前は「外すものが無い」と同じ `false` で返したので、呼ぶ側が断ったことを
    /// 知らせられなかった：設計審査 de92ceb の D-5）
    pub(crate) fn remove(
        &mut self,
        scope: &[u64],
        faces: &HashSet<ShapeId>,
        instances: &HashSet<u64>,
        held: &mut Vec<Arc<LayerBase>>,
    ) -> Result<bool, crate::planar_map::LocalRefusal> {
        let mut changed = false;
        let mut items = Vec::new();
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(path, m)
                    if path.starts_with(scope) && m.base().has_any_face(faces, &path) =>
                {
                    changed = true;
                    // 書けなければ何も変えずに断る（全部か無しか）
                    if let Some((kept, base)) = m.restricted(&path, FaceKeep::Without(faces))? {
                        held.extend(base);
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
        Ok(changed)
    }

    /// グループ化：階層 `scope` の中の、選んだ面（番号 `faces`。`scope` より深い階層の地図の）とインスタンス（番号
    /// `instances`）の階層の番号に、新しいグループ `gid` を入れ（`scope` のすぐ下）、`scope` の中身のいちばん上へ移す。
    /// 階層 `scope` そのものの生の図形は、呼ぶ側が浮いた片の地図でグループの階層に書く。面を外した地図の新しい写しは
    /// `held` に入れる。`Ok(false)`：移すものが無い。`Err`：書けない（何も変えない）
    pub(crate) fn group(
        &mut self,
        scope: &[u64],
        faces: &HashSet<ShapeId>,
        instances: &HashSet<u64>,
        gid: u64,
        held: &mut Vec<Arc<LayerBase>>,
    ) -> Result<bool, crate::planar_map::LocalRefusal> {
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
                        && m.base().has_any_face(faces, &path) =>
                {
                    first_at.get_or_insert(rest.len());
                    let kept = m.restricted(&path, FaceKeep::Without(faces))?;
                    let picked = m.restricted(&into(&path), FaceKeep::Only(faces))?;
                    if let Some((kept, base)) = kept {
                        held.extend(base);
                        rest.push(Item::Raw(path.clone(), kept));
                    }
                    if let Some((picked, _)) = picked {
                        moved.push(Item::Raw(into(&path), picked));
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
            return Ok(false);
        };
        // 階層 `scope` の中身（残ったもの）のすぐ後ろ
        let at = rest
            .iter()
            .rposition(|it| it.scope().starts_with(scope))
            .map_or(first_at, |i| i + 1);
        rest.splice(at..at, moved);
        self.arrange(rest);
        Ok(true)
    }

    /// グループの解除：階層 `scope` の中の、選んだ面（番号 `faces`。`scope` の孫より深い階層の地図の）とインスタンス
    /// （番号 `instances`。`scope` の子のグループの中のもの）の階層の番号から、`scope` の子の番号を外す（1つ上の階層へ）。
    /// 階層 `lifted`（解くグループの生の図形の階層。呼ぶ側が浮いた片にした）の地図は外す。面を外した地図の新しい写しは
    /// `held` に入れる。`Ok(false)`：変わるものが無い。`Err`：書けない（何も変えない）
    pub(crate) fn ungroup(
        &mut self,
        scope: &[u64],
        faces: &HashSet<ShapeId>,
        instances: &HashSet<u64>,
        lifted: &[Vec<u64>],
        held: &mut Vec<Arc<LayerBase>>,
    ) -> Result<bool, crate::planar_map::LocalRefusal> {
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
                        && m.base().has_any_face(faces, &path) =>
                {
                    changed = true;
                    let kept = m.restricted(&path, FaceKeep::Without(faces))?;
                    let picked = m.restricted(&up(&path), FaceKeep::Only(faces))?;
                    if let Some((kept, base)) = kept {
                        held.extend(base);
                        items.push(Item::Raw(path.clone(), kept));
                    }
                    if let Some((picked, _)) = picked {
                        items.push(Item::Raw(up(&path), picked));
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
        Ok(changed)
    }

    /// 中身 `it` が、階層 `scope` のどの子（グループ・インスタンスの番号）に入るか（`scope` そのものの生の図形と、`scope` の
    /// 外は `None`）。重ね順の子の見分け方はこれ1つ（`children`・`rearrange`）
    fn child_of(it: &Item<ScopeMap>, scope: &[u64]) -> Option<u64> {
        let depth = scope.len();
        match it {
            Item::Raw(p, _) if p.starts_with(scope) && p.len() > depth => Some(p[depth]),
            Item::Instance(i) if i.parent_scope == scope => Some(i.binding.instance_gid),
            Item::Instance(i) if i.parent_scope.starts_with(scope) => Some(i.parent_scope[depth]),
            _ => None,
        }
    }

    /// 階層 `scope` の子（グループ・インスタンスの番号）を、重なりの順（下から）に並べたもの。文書の中身から作るので、画面に
    /// 図形を出さない子（変換が潰れたインスタンスなど）も入る（重ね順の変更の並び：`select_ops::prepare_arrange_selected`）
    pub(crate) fn children(&self, scope: &[u64]) -> Vec<u64> {
        let mut out: Vec<u64> = Vec::new();
        for it in Self::items_of(&self.maps, &self.instances) {
            if let Some(g) = Self::child_of(&it, scope) {
                if !out.contains(&g) {
                    out.push(g);
                }
            }
        }
        out
    }

    /// 重ね順：階層 `scope` の子（グループ・インスタンス）の順を、番号の並び `order` にする（並びに無い子は動かさない）。
    /// `false`：変わらない
    pub(crate) fn rearrange(&mut self, scope: &[u64], order: &[u64]) -> bool {
        let child = |it: &Item<ScopeMap>| Self::child_of(it, scope);
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

    /// 番号が `pick` に合うインスタンスの結び付けを `f` で書き換える。`Ok(false)`：当てはまるインスタンスが無いか、変わらない。
    /// `Err`：数が有限でない結び付けになる（`SymbolInstanceBinding::is_finite`。何も変えない）
    pub(crate) fn update_instances(
        &mut self,
        pick: impl Fn(u64) -> bool,
        f: impl Fn(&mut SymbolInstanceBinding),
    ) -> Result<bool, Unmappable> {
        if !self.instances.iter().any(|i| pick(i.binding.instance_gid)) {
            return Ok(false);
        }
        let mut instances = (*self.instances).clone();
        let mut changed = false;
        for inst in instances
            .iter_mut()
            .filter(|i| pick(i.binding.instance_gid))
        {
            let before = inst.binding;
            f(&mut inst.binding);
            if !inst.binding.is_finite() {
                return Err(Unmappable);
            }
            changed |= inst.binding != before;
        }
        if changed {
            *self.instances_mut() = instances;
        }
        Ok(changed)
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

    /// 面の図形の番号を、みな新しい番号にする（キーフレームを写したとき。同じコマに同じ番号の図形が2つ出ないように）。
    /// 替え方 `r` は1回の作業で1つ（`ShapeIdRenewal`）
    pub(crate) fn renew_shape_ids(&mut self, r: &mut ShapeIdRenewal) {
        if self.maps.is_empty() {
            return;
        }
        for m in Arc::make_mut(&mut self.maps) {
            m.renew_labels(r);
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
                    if let Ok(Some((m, _))) = m.restricted(rel, FaceKeep::Only(faces)) {
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
            let map = crate::planar_map::stacked(
                PlanarMap::default(),
                maps.iter().map(|m| Some(m.dense())),
            )
            .ok_or(Unmappable)?;
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

    /// （生の図形の地図（階層 `[]`）, それを除いたセル画）。地図は確かめた地図のまま渡す（セル画の地図は使う入口
    /// `ScopeMap::stored` で確かめてあるので、その写しの一部 `LayerBase::checked_part` にする。確かめ直さない）
    pub(crate) fn split_raw(&self) -> (Option<CheckedMap>, Cel) {
        let Some(raw) = self.maps.iter().find(|m| m.group_path.is_empty()) else {
            return (None, self.clone());
        };
        let raw = raw.base().checked_part(None, &[]);
        let mut items = Self::items_of(&self.maps, &self.instances);
        items.retain(|it| !matches!(it, Item::Raw(p, _) if p.is_empty()));
        let mut rest = Cel::default();
        rest.arrange(items);
        (Some(raw), rest)
    }

    /// 座標の変換 `xform` をかけたセル画：地図は、その座標の格子で正しい地図にし（`realized`）、面にみな新しい番号を
    /// 記す（元の面と番号が重ならない）。インスタンスの変換に `xform` を重ねる。`None`：格子に置けない座標があるか、数が
    /// 有限でないインスタンスになる（書く入口 `insert_content` と同じ判定：`SymbolInstanceBinding::is_finite`）
    pub(crate) fn transformed(&self, xform: Affine) -> Option<Cel> {
        let mut items = Vec::with_capacity(self.maps.len() + self.instances.len());
        for item in Self::items_of(&self.maps, &self.instances) {
            match item {
                Item::Raw(path, m) => {
                    let map = realized(m.base().map(), xform)?.freshly_labeled();
                    if let Some(m) = ScopeMap::new(path.clone(), map, None) {
                        items.push(Item::Raw(path, m));
                    }
                }
                Item::Instance(mut inst) => {
                    inst.binding.xform = xform * inst.binding.xform;
                    if !inst.binding.is_finite() {
                        return None;
                    }
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
    /// 無いか、中身に階層 `scope` の生の図形の地図があるか（同じ階層の地図が2つになる）、数が有限でないインスタンスがある
    /// （`SymbolInstanceBinding::is_finite`）。断るときはセル画を変えない
    pub(crate) fn insert_content(
        &mut self,
        scope: &[u64],
        content: &Cel,
        replacing: Option<u64>,
    ) -> bool {
        // 階層があるかだけを見る（地図を確かめさせない）
        if content.maps.iter().any(|m| m.group_path.is_empty())
            || !content.instances.iter().all(|i| i.binding.is_finite())
        {
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

    /// 地図の図形の番号（みな。面と線：`shape_ids_of`）
    pub(crate) fn shape_ids(&self) -> HashSet<ShapeId> {
        self.maps
            .iter()
            .flat_map(|m| crate::planar_map::shape_ids_of(m.stored()))
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
            .map(ScopeMap::stored)
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

    /// 1本の道の地図の編集（`LayerBase::edit_local`・`fill_local`・`anchor_local` など）：階層 `group_path` の地図を `f` で編集し、
    /// 書くものがあれば入れる（`put_local`）。`Ok(None)`：変わらない、`Err`：書けない
    pub(crate) fn edit_local(
        &mut self,
        group_path: &[u64],
        f: impl FnOnce(&LayerBase) -> crate::planar_map::LocalEdit,
    ) -> Result<Option<MapWritten>, crate::planar_map::LocalRefusal> {
        use crate::planar_map::LocalEdit;
        let base = self.scope_base(group_path);
        match f(&base) {
            LocalEdit::Same => Ok(None),
            LocalEdit::Refused(r) => Err(r),
            LocalEdit::Changed(w) => Ok(Some(MapWritten(self.put_local(group_path, w, &base)))),
        }
    }

    /// 触った所だけで行った編集の書くもの `w` を入れる（階層の地図を新しいしまう形にする。面の無くなった階層は外し、
    /// 新しい階層は決まりどおりの所に入れる）。戻り値は新しい地図の写し（`LayerBase::of_local`。前の写し `prev` から升目の索引を作る）
    pub(crate) fn put_local(
        &mut self,
        group_path: &[u64],
        w: crate::planar_map::LocalWrite,
        prev: &LayerBase,
    ) -> Option<Arc<LayerBase>> {
        let at = self.maps.iter().position(|m| m.group_path == group_path);
        let Some(m) = ScopeMap::of_checked_stored(group_path.to_vec(), w.stored) else {
            let mut items = Self::items_of(&self.maps, &self.instances);
            items.retain(|it| !matches!(it, Item::Raw(p, _) if p == group_path));
            self.arrange(items);
            return None;
        };
        let base = LayerBase::of_local(m.stored().clone(), group_path, w.faces, prev);
        match at {
            Some(i) => {
                Arc::make_mut(&mut self.maps)[i] = m;
                self.revision = crate::vector_engine::next_mesh_revision();
            }
            None => {
                let mut items = Self::items_of(&self.maps, &self.instances);
                items.push(Item::Raw(group_path.to_vec(), m));
                self.arrange(items);
            }
        }
        Some(base)
    }

    /// テスト用：地図と重なりの順が決まりどおりか（どの地図も正しい地図で辺があり（線だけの地図は面が無い）、面に1つずつ別の番号が記してある。
    /// 同じ階層の地図は1つで、階層の地図はその親の階層の地図より後ろ。インスタンスは重なり位置の順で、位置は地図の数以下、
    /// 親の階層の地図より後ろ）。決まりどおりでなければ、その中身
    #[cfg(test)]
    pub(crate) fn maps_problem(&self) -> Option<String> {
        for (k, m) in self.maps.iter().enumerate() {
            let base = m.base();
            if !base.map().is_valid() {
                return Some(format!(
                    "階層 {:?} の地図が正しくない: {}",
                    m.group_path,
                    base.map().validity_report()
                ));
            }
            let (faces, _) = base.map().faces();
            if base.map().edges.is_empty() {
                return Some(format!("階層 {:?} の地図に辺が無い", m.group_path));
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

/// タイムライン上のキーフレーム（セル画 `Cel`、クラシックトゥイーン有効フラグ `tween`、トゥイーン回転方向 `tween_rotate`、
/// 音 `sound`）。
/// 始まりのコマ（0 から）はキーフレームの列が持つ（`Layer::start_of`。キーフレームには列の塊の基準からの差だけを持つ：
/// `ChunkedList::key`）
#[derive(Clone, Debug)]
pub(crate) struct Keyframe {
    rel_start: i64,
    pub(crate) cel: Cel,
    pub(crate) tween: bool,
    pub(crate) tween_rotate: TweenRotate,
    /// 付けた音の区間（音を付けたキーフレームは音だけ：セル画は空。設計：claude/オーディオの設計_9693dbf.md）
    pub(crate) sound: Option<crate::sound::SoundClip>,
}

impl crate::chunked_list::Keyed for Keyframe {
    fn rel(&self) -> i64 {
        self.rel_start
    }
    fn set_rel(&mut self, rel: i64) {
        self.rel_start = rel;
    }
}

impl Keyframe {
    pub(crate) fn new_blank() -> Self {
        Self::with_cel(Cel::default(), false, TweenRotate::Auto)
    }

    /// セル画 `cel` とトゥイーンの設定のキーフレーム（始まりのコマは、列に入れるときに決める）
    pub(crate) fn with_cel(cel: Cel, tween: bool, tween_rotate: TweenRotate) -> Self {
        Self {
            rel_start: 0,
            cel,
            tween,
            tween_rotate,
            sound: None,
        }
    }

    /// 音の区間を `sound` にしたもの
    pub(crate) fn with_sound(mut self, sound: Option<crate::sound::SoundClip>) -> Self {
        self.sound = sound;
        self
    }

    /// 中身の無い空白のキーフレームか：セル画が空で、トゥイーンも音も無い。「空白か」の決まりはこれ1つ
    /// （`F7` で次へ進むか・消せるか・縮められるか・クリアで変わるか）
    pub(crate) fn is_blank(&self) -> bool {
        self.cel.is_empty() && !self.tween && self.sound.is_none()
    }

    /// キーフレーム内のセル図形・トゥイーン設定・音をクリアし、変更があったかどうかを返す
    pub(crate) fn clear_cel(&mut self) -> bool {
        if self.is_blank() {
            false
        } else {
            self.cel.clear(&[]);
            self.tween = false;
            self.tween_rotate = TweenRotate::Auto;
            self.sound = None;
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
    kfs: &mut [(usize, Keyframe)],
    mut alloc_group_id: impl FnMut() -> u64,
) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    let mut alloc_gid = |old_id: u64, map: &mut HashMap<u64, u64>| -> u64 {
        *map.entry(old_id).or_insert_with(&mut alloc_group_id)
    };
    let mut renewal = ShapeIdRenewal::default();
    for (_, kf) in kfs {
        kf.cel.renew_shape_ids(&mut renewal);
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

/// レイヤーのキーフレームの列（開始フレームの順）。塊ごとに Undo の控えと共有する列（`ChunkedList`）
pub(crate) type KeyframeList = crate::chunked_list::ChunkedList<Keyframe>;

/// 区間の先頭からの始まりのコマと組にしたキーフレームの並び（コマの移動・複製で運ぶもの：`Layer::extract_keyframes_in_range`）
pub(crate) type RelativeKeyframes = Vec<(usize, Keyframe)>;

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
            keyframes: KeyframeList::from_keyed([(0, Arc::new(Keyframe::new_blank()))]),
        }
    }

    pub(crate) fn new_with_id(id: u64, name: impl Into<String>) -> Self {
        Self::new_with_id_and_frames(id, name, 1)
    }

    /// `i` 番目のキーフレームへの可変参照を返す（Undo 履歴と共有されている場合は Copy-on-Write で複製する）
    pub(crate) fn keyframe_mut(&mut self, i: usize) -> &mut Keyframe {
        Arc::make_mut(&mut self.keyframes[i])
    }

    /// `i` 番目のキーフレームの始まりのコマ（0 から）
    pub(crate) fn start_of(&self, i: usize) -> usize {
        self.keyframes.key(i)
    }

    /// 始まりのコマとキーフレームを、前から順に
    pub(crate) fn keyframes_with_start(
        &self,
    ) -> impl Iterator<Item = (usize, &Arc<Keyframe>)> + '_ {
        self.keyframes.iter_keyed()
    }

    /// 中身が `other` と同じか（`Timeline::same_content`）：名前・表示・ロック・コマの数と、キーフレームの列（始まりのコマ・
    /// トゥイーン・音・セル画。セル画は地図とインスタンスの並びが同じもの（番地が同じか、どちらも空））
    fn same_content(&self, other: &Layer) -> bool {
        let same_cel = |a: &Cel, b: &Cel| {
            (Arc::ptr_eq(&a.maps, &b.maps) || (a.maps.is_empty() && b.maps.is_empty()))
                && (Arc::ptr_eq(&a.instances, &b.instances)
                    || (a.instances.is_empty() && b.instances.is_empty()))
        };
        let same_keyframe = |a: &Keyframe, b: &Keyframe| {
            a.tween == b.tween
                && a.tween_rotate == b.tween_rotate
                && a.sound == b.sound
                && same_cel(&a.cel, &b.cel)
        };
        self.id == other.id
            && self.name == other.name
            && self.visible == other.visible
            && self.locked == other.locked
            && self.frame_count == other.frame_count
            && (self.keyframes.ptr_eq(&other.keyframes)
                || (self.keyframes.len() == other.keyframes.len()
                    && self.keyframes_with_start().zip(other.keyframes_with_start()).all(
                        |((sa, a), (sb, b))| sa == sb && (Arc::ptr_eq(a, b) || same_keyframe(a, b)),
                    )))
    }

    /// 始まりのコマが `frame` より前のキーフレームの数（`frame` に入れるキーフレームの番号）
    fn keyframes_before(&self, frame: usize) -> usize {
        self.keyframes.partition_point_key(|s| s < frame)
    }

    /// 始まりのコマが `frame` のキーフレームの番号
    pub(crate) fn keyframe_index_starting_at(&self, frame: usize) -> Option<usize> {
        let i = self.keyframes_before(frame);
        (i < self.keyframes.len() && self.start_of(i) == frame).then_some(i)
    }

    /// 指定フレーム番号 `frame` において有効な（保持されている）キーフレームのインデックスを返す
    /// （そのレイヤーの `frame_count` を超えるフレームでは `None` を返す）
    pub(crate) fn keyframe_index_at(&self, frame: usize) -> Option<usize> {
        if frame >= self.frame_count {
            return None;
        }
        let idx = self.keyframes.partition_point_key(|s| s <= frame);
        idx.checked_sub(1)
    }

    /// `kf_idx` 番目のキーフレームが保持される終端フレーム番号（inclusive）を返す
    pub(crate) fn span_end_for_keyframe(&self, kf_idx: usize) -> usize {
        let max_f = self.frame_count.saturating_sub(1);
        if kf_idx + 1 < self.keyframes.len() {
            self.start_of(kf_idx + 1).saturating_sub(1).min(max_f)
        } else {
            max_f
        }
    }

    /// 指定フレーム番号 `frame` で表示される `Cel` への不変参照を返す
    pub(crate) fn cel_at(&self, frame: usize) -> Option<&Cel> {
        let idx = self.keyframe_index_at(frame)?;
        Some(&self.keyframes[idx].cel)
    }

    /// キーフレーム `kf` を、始まりのコマ `frame` で入れる（`frame` にキーフレームが無いこと）
    fn insert_keyframe_starting_at(&mut self, frame: usize, kf: Keyframe) {
        let i = self.keyframes_before(frame);
        self.keyframes.insert_keyed(i, frame, Arc::new(kf));
    }

    /// 指定フレーム `frame` にキーフレームを挿入する（`clone_prev == true`: `F6` 複製 / `false`: `F7` 空白）
    /// （`fps`：作品の fps。音の区間を分けた位置を、音の中の位置にする：`SoundClip::advanced`）
    pub(crate) fn insert_keyframe(&mut self, frame: usize, clone_prev: bool, fps: u32) -> bool {
        let mut changed = false;
        if frame >= self.frame_count {
            self.frame_count = frame + 1;
            changed = true;
        }
        if let Some(existing_idx) = self.keyframe_index_starting_at(frame) {
            if !clone_prev && !self.keyframes[existing_idx].is_blank() {
                changed |= self.keyframe_mut(existing_idx).clear_cel();
            }
            return changed;
        }

        let prev = self.keyframe_index_at(frame);
        let new_kf = if clone_prev {
            self.snapshot_keyframe_at(frame, fps)
        } else {
            Keyframe::new_blank()
        };
        // 音の区間の途中で分けたら、前の区間は分けた所で終わる（後ろは続きから鳴る）
        let split_at = new_kf.sound.map(|c| c.start);
        self.insert_keyframe_starting_at(frame, new_kf);
        if let (Some(at), Some(p)) = (split_at, prev) {
            if let Some(c) = &mut self.keyframe_mut(p).sound {
                c.end = at;
            }
        }
        true
    }

    /// 指定フレーム `frame` を含むキーフレームを削除（フレーム0の先頭の場合は空白化）する（`Shift+F6`）
    pub(crate) fn delete_keyframe_at(&mut self, frame: usize) -> bool {
        let Some(idx) = self.keyframe_index_at(frame) else {
            return false;
        };
        if self.start_of(idx) != frame {
            return false;
        }
        if idx == 0 {
            self.keyframe_mut(0).clear_cel()
        } else {
            self.keyframes.remove(idx);
            true
        }
    }

    /// 指定フレーム `frame` の保持尺を1フレーム延長し、後続のキーフレームを +1 フレーム後ろへずらす（`F5`）。
    /// ずらすのは、後ろのキーフレームの塊の基準だけ（列の長さによらない：`ChunkedList::shift_keys_from`）
    pub(crate) fn insert_frame_at(&mut self, frame: usize) {
        if frame >= self.frame_count {
            self.frame_count = frame + 1;
        } else {
            let first_after = self.keyframes.partition_point_key(|s| s <= frame);
            self.keyframes.shift_keys_from(first_after, 1);
            self.frame_count += 1;
        }
    }

    /// Adobe Animate 仕様のフレーム削除（`Shift+F5`）を1コマ分：`remove_frames(frame, 1, fps)`
    pub(crate) fn remove_frame_at(&mut self, frame: usize, fps: u32) -> bool {
        self.remove_frames(frame, 1, fps)
    }

    /// Adobe Animate 仕様のフレーム削除（`Shift+F5`）を、コマ `first` から `count` コマ分、一度に行う（キーフレームの
    /// 並びは、`first` で1コマずつ `count` 回消すのと同じ）：
    /// - 消すコマ `[first, first + count)` に始まるキーフレームは外す。ただし、消すコマのすぐ後ろのコマを保持する
    ///   キーフレームは、そのコマの姿（`snapshot_keyframe_at`：F6・コマの移動と同じ決まり。音・シンボルの中のコマ・
    ///   トゥイーンの中割りは、そのコマから続く。`fps`：作品の fps）にして `first` に置く（消したコマのあとの絵と音が、
    ///   `first` から続く）。
    /// - 後ろのキーフレームは `count` コマ左へ詰める。
    /// - コマを全部消すときは、1コマだけ残し、最後のキーフレームを空白にしてコマ 1 に置く。
    pub(crate) fn remove_frames(&mut self, first: usize, count: usize, fps: u32) -> bool {
        let n = self.frame_count;
        if first >= n || count == 0 {
            return false;
        }
        let removed = count.min(n - first);
        if removed == n {
            let last_start = self.start_of(self.keyframes.len() - 1);
            if self.keyframes.len() > 1 {
                self.keyframes.retain_keyed(|s, _| s == last_start);
            }
            if last_start != 0 {
                self.keyframes.set_key(0, 0);
            }
            let cleared = self.keyframe_mut(0).clear_cel();
            self.frame_count = 1;
            return n > 1 || cleared;
        }
        let end = first + removed;
        // 消すコマのすぐ後ろのコマ `end` を保持するキーフレームが消すコマに始まるなら、`end` の姿を `first` に置く
        let kept = self
            .keyframe_index_at(end)
            .filter(|&k| (first..end).contains(&self.start_of(k)))
            .map(|_| self.snapshot_keyframe_at(end, fps));
        self.keyframes.retain_keyed(|s, _| !(first..end).contains(&s));
        self.keyframes
            .shift_keys_from(self.keyframes_before(first), -(removed as i64));
        if let Some(kf) = kept {
            self.insert_keyframe_starting_at(first, kf);
        }
        self.frame_count = n - removed;
        self.ensure_head_keyframe();
        true
    }

    /// 先頭（0 フレーム目）のキーフレームが無ければ、空白のキーフレームを入れる
    fn ensure_head_keyframe(&mut self) {
        if self.keyframes.is_empty() || self.start_of(0) > 0 {
            self.insert_keyframe_starting_at(0, Keyframe::new_blank());
        }
    }

    /// 指定フレーム `frame` における評価済みセル姿勢（クラシックトゥイーン中割り姿勢および経過フレーム分の内部 `first_frame` 進行を含む）を、
    /// 新しい `Keyframe` として構築して返す（始まりのコマは、列に入れるときに決める）
    /// 音の区間は、途中のコマなら、そこまで進んだ続きから鳴らす区間にする（`SoundClip::advanced`。`fps`：作品の fps）
    pub(crate) fn snapshot_keyframe_at(&self, frame: usize, fps: u32) -> Keyframe {
        if frame >= self.frame_count {
            return Keyframe::new_blank();
        }
        let Some(kf_idx) = self.keyframe_index_at(frame) else {
            return Keyframe::new_blank();
        };
        let kf = &self.keyframes[kf_idx];
        let kf_start = self.start_of(kf_idx);
        if kf_start == frame {
            return Keyframe::with_cel(kf.cel.clone(), kf.tween, kf.tween_rotate).with_sound(kf.sound);
        }

        // 図形の列は元のキーフレームと共有する（このレイヤーのセルの図形は、もともと layer_id がこのレイヤー）。
        // 変えるのは、インスタンスの再生位置と、トゥイーンの中割りの姿勢だけ
        let mut cel = kf.cel.clone();
        if !cel.instances.is_empty() {
            let elapsed = frame.saturating_sub(kf_start);
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
        Keyframe::with_cel(cel, tween, tween_rotate)
            .with_sound(kf.sound.and_then(|c| c.advanced(frame - kf_start, fps)))
    }

    /// 指定フレーム区間 `[min_frame..=max_frame]` 内のキーフレーム群を、相対フレーム位置（`0..=max_frame - min_frame`）と組にして抽出する
    /// （`min_frame` がキーフレーム途中やトゥイーン中割りフレームにある場合は、そのフレームの評価済み中割り姿勢を引き継ぐ）
    pub(crate) fn extract_keyframes_in_range(
        &self,
        min_frame: usize,
        max_frame: usize,
        fps: u32,
    ) -> RelativeKeyframes {
        let mut kfs: RelativeKeyframes = self
            .keyframes_with_start()
            .filter(|&(s, _)| s >= min_frame && s <= max_frame)
            .map(|(s, kf)| (s - min_frame, Keyframe::clone(kf)))
            .collect();

        if kfs.first().is_none_or(|&(rel, _)| rel != 0) {
            kfs.insert(0, (0, self.snapshot_keyframe_at(min_frame, fps)));
        }
        kfs
    }

    /// 指定フレーム区間 `[min_frame..=max_frame]` のキーフレームを外す（コマの移動の移動元）。Adobe Animate と同じく、
    /// 空いたコマには前のキーフレームの絵が続く（コマ 1 のキーフレームが抜けたときだけ、コマ 1 に空白キーフレームを置く）
    pub(crate) fn remove_keyframes_in_range(&mut self, min_frame: usize, max_frame: usize) {
        self.keyframes
            .retain_keyed(|s, _| s < min_frame || s > max_frame);
        self.ensure_head_keyframe();
    }

    /// 抽出された相対キーフレーム群 `kfs`（相対の始まりのコマと組）を `[dst_min_frame..=dst_max_frame]` に配置する（区間にあったキーフレームは
    /// 置き換える）。Adobe Animate と同じく、区間の後ろのコマには、置いた最後のキーフレームの絵が次のキーフレームまで続く
    pub(crate) fn place_keyframes_at(
        &mut self,
        dst_min_frame: usize,
        dst_max_frame: usize,
        kfs: RelativeKeyframes,
    ) {
        if dst_max_frame >= self.frame_count {
            self.frame_count = dst_max_frame + 1;
        }
        self.keyframes
            .retain_keyed(|s, _| s < dst_min_frame || s > dst_max_frame);
        let dst_lid = self.id;
        for (rel, mut kf) in kfs {
            if kf.cel.instances.iter().any(|i| i.layer_id != dst_lid) {
                for inst in kf.cel.instances_mut() {
                    inst.layer_id = dst_lid;
                }
            }
            let (valid_tween, rotate_mode) =
                valid_tween_settings(kf.tween, kf.tween_rotate, &kf.cel);
            let target_f = dst_min_frame + rel;
            if let Some(existing_idx) = self.keyframe_index_starting_at(target_f) {
                let existing = self.keyframe_mut(existing_idx);
                existing.cel = kf.cel;
                existing.tween = valid_tween;
                existing.tween_rotate = rotate_mode;
                existing.sound = kf.sound;
            } else {
                self.insert_keyframe_starting_at(
                    target_f,
                    Keyframe::with_cel(kf.cel, valid_tween, rotate_mode).with_sound(kf.sound),
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
        let start_frame = self.start_of(kf_idx);
        let next = (kf_idx + 1 < self.keyframes.len())
            .then(|| (self.start_of(kf_idx + 1), &self.keyframes[kf_idx + 1]));
        if let Some((end_frame, next_kf)) =
            next.filter(|&(end, _)| end < self.frame_count && end > start_frame)
        {
            if let Some(end_binding) = next_kf
                .cel
                .single_symbol_instance()
                .filter(|b| b.symbol_id == start_binding.symbol_id)
            {
                return TweenState::Complete {
                    start_frame,
                    end_frame,
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

    /// 指定フレーム `frame` においてこのレイヤーが編集可能（表示中・非ロック・フレーム作成済み・キーフレームあり・トゥイーンの途中ではない・音の付いたキーフレームではない）であるかを返す
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
        } else if self
            .keyframe_index_at(frame)
            .is_some_and(|k| self.keyframes[k].sound.is_some())
        {
            Some(EditBlock::SoundKeyframe)
        } else {
            None
        }
    }

    /// コマ `frame` で音を付けられるキーフレームの番号：絵を描けるのと同じ決まり（音がもう付いていることだけは構わない：
    /// 付け替える）で、セル画が空のキーフレーム。音を付けられる所の決まりはこれ1つ（読み込み・プロパティの欄）
    pub(crate) fn sound_slot_at(&self, frame: usize) -> Option<usize> {
        if !matches!(
            self.edit_block_at(frame),
            None | Some(EditBlock::SoundKeyframe)
        ) {
            return None;
        }
        let k = self.keyframe_index_at(frame)?;
        self.keyframes[k].cel.is_empty().then_some(k)
    }

    /// キーフレーム `k` の音が鳴るコマ：始まりから、音の区間が鳴り終わる所（`SoundClip::frames`）とキーフレームの区間の
    /// 終わりの早いほうまで。音が無ければ `None`。波形を描く所と、両端のつまみの所はこれで決める
    pub(crate) fn sounding_frames(&self, k: usize, fps: u32) -> Option<std::ops::Range<usize>> {
        let clip = self.keyframes[k].sound?;
        let start = self.start_of(k);
        Some(start..(start + clip.frames(fps)).min(self.span_end_for_keyframe(k) + 1))
    }

    /// キーフレーム `k` の音の区間の端 `edge` を動かせる所（コマの境目の番号：始まりはキーフレームの始まりのコマ、終わりは
    /// 鳴り終わったコマの次のコマ。設計の 7 章）。音が無い・音のライブラリ `sounds` に無いなら `None`
    /// - 終わり：始まりより1コマ以上後ろから、音の本当の終わりに届くコマと、キーフレームの区間の終わりの早いほうまで
    /// - 始まり：鳴り終わるコマの1つ手前まで削れる。戻すのは、区間の始まりが音の頭より前にならない所まで（削った頭に
    ///   入りきるコマの数だけ、キーフレームより前へ戻せる）
    pub(crate) fn sound_trim_range(
        &self,
        k: usize,
        edge: crate::sound::SoundEdge,
        sounds: &[Arc<crate::sound::Sound>],
        fps: u32,
    ) -> Option<std::ops::RangeInclusive<usize>> {
        use crate::sound::{frames_to_reach, SoundEdge};
        let clip = self.keyframes[k].sound?;
        let len = sounds.iter().find(|s| s.id == clip.sound)?.len();
        let shown = self.sounding_frames(k, fps)?;
        let span_end = self.span_end_for_keyframe(k) + 1;
        Some(match edge {
            SoundEdge::End => {
                shown.start + 1..=span_end.min(shown.start + frames_to_reach(len - clip.start, fps))
            }
            SoundEdge::Start => {
                let back = frames_to_reach(clip.start + 1, fps) - 1;
                shown.start.saturating_sub(back)..=shown.end - 1
            }
        })
    }

    /// キーフレーム `k` の音の区間の端 `edge` を、コマの境目 `frame` へ動かす（設計の 7 章。動かせる所は
    /// `sound_trim_range`）。終わりは、そのコマの境目に当たる音の中の位置にする（音の本当の終わりは越えない）。始まりは、
    /// 区間の始まりを、キーフレームからそのコマまでの時間だけ進める（戻す）。どちらの端も、キーフレームは動かさない（頭を
    /// 削ると、残りの音がキーフレームのコマから鳴る）。動かせない所・変わらないなら断る
    pub(crate) fn trim_sound(
        &mut self,
        k: usize,
        edge: crate::sound::SoundEdge,
        frame: usize,
        sounds: &[Arc<crate::sound::Sound>],
        fps: u32,
    ) -> bool {
        use crate::sound::{frames_to_sound, SoundEdge};
        let in_range = self
            .sound_trim_range(k, edge, sounds, fps)
            .is_some_and(|r| r.contains(&frame));
        let Some(clip) = self.keyframes[k].sound.filter(|_| in_range) else {
            return false;
        };
        let Some(sound) = sounds.iter().find(|x| x.id == clip.sound) else {
            return false;
        };
        let s = self.start_of(k);
        let trimmed = match edge {
            SoundEdge::End => crate::sound::SoundClip {
                end: (clip.start + frames_to_sound(frame - s, fps)).min(sound.len()),
                ..clip
            },
            SoundEdge::Start => {
                let start = if frame >= s {
                    clip.pos_at(frame - s, fps)
                } else {
                    clip.start.checked_sub(frames_to_sound(s - frame, fps))
                };
                let Some(start) = start else {
                    return false;
                };
                crate::sound::SoundClip { start, ..clip }
            }
        };
        if trimmed == clip {
            return false;
        }
        self.keyframe_mut(k).sound = Some(trimmed);
        true
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
    /// 音の付いたキーフレーム（音だけ：絵は描けない）
    SoundKeyframe,
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
    /// 中身（今のコマ・選んでいるレイヤー・タイムラインの選択を除いたもの）が `other` と同じか（中身が変わったかの決まりは
    /// これ1つ：取り消しの控えを積むか `doc_actions::mutate_timeline_with_undo`）。書き換えていないレイヤーは、キーフレームの
    /// 列を共有したままなので番地を見るだけ。書き換えたレイヤーも、変わっていないキーフレームは番地が同じ（手間は触った
    /// レイヤーのキーフレームの数の分）
    pub(crate) fn same_content(&self, other: &Timeline) -> bool {
        self.total_frames == other.total_frames
            && self.next_layer_id == other.next_layer_id
            && self.layers.len() == other.layers.len()
            && self.layers.iter().zip(&other.layers).all(|(a, b)| a.same_content(b))
    }

    /// まだ確かめていない地図に、壊れていたときの知らせ先 `sink` と場所（`place` に続けて、レイヤーの名前とキーフレームの
    /// コマの番号）を付ける（`Document::watch_broken_maps`）
    pub(crate) fn watch_broken_maps(&self, sink: &BrokenMaps, place: &str) {
        for layer in &self.layers {
            for (start, kf) in layer.keyframes_with_start() {
                for m in kf.cel.maps.iter() {
                    m.watch(sink, || {
                        format!("{place}レイヤー「{}」のコマ {}", layer.name, start + 1)
                    });
                }
            }
        }
    }

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

    /// レイヤー `layer_id` の名前を `new_name` にする。無い番号・同じ名前なら変えない
    pub(crate) fn rename_layer(&mut self, layer_id: u64, new_name: String) -> bool {
        let Some(layer) = self.layers.iter_mut().find(|l| l.id == layer_id) else {
            return false;
        };
        if layer.name == new_name {
            return false;
        }
        layer.name = new_name;
        true
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

    /// 全レイヤー・全キーフレームの音の区間を列挙する
    pub(crate) fn sound_clips(&self) -> impl Iterator<Item = crate::sound::SoundClip> + '_ {
        self.layers
            .iter()
            .flat_map(|l| l.keyframes.iter())
            .filter_map(|kf| kf.sound)
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

    /// 対象レイヤー群 `targets` に対して現在フレーム（または直後フレーム）へキーフレームを挿入する（`F6` / `F7`。中身が
    /// 変わったかは、呼ぶ側が前後を比べて決める：`Timeline::same_content`）
    pub(crate) fn insert_keyframe_on_targets(
        &mut self,
        targets: &[usize],
        clone_prev: bool,
        fps: u32,
    ) {
        let curr_f = self.current_frame;
        let should_advance = if clone_prev {
            targets.iter().all(|&l_idx| {
                self.layers
                    .get(l_idx)
                    .is_some_and(|l| l.keyframe_index_starting_at(curr_f).is_some())
            })
        } else {
            targets.iter().any(|&l_idx| {
                self.layers.get(l_idx).is_some_and(|l| {
                    l.keyframe_index_starting_at(curr_f)
                        .is_some_and(|i| !l.keyframes[i].is_blank())
                })
            })
        };

        let target_frame = if should_advance { curr_f + 1 } else { curr_f };
        let shift_for_blank = !clone_prev
            && should_advance
            && targets.iter().any(|&l_idx| {
                self.layers.get(l_idx).is_some_and(|l| {
                    l.keyframe_index_starting_at(target_frame)
                        .is_some_and(|i| !l.keyframes[i].is_blank())
                })
            });

        for &l_idx in targets {
            if let Some(layer) = self.layers.get_mut(l_idx) {
                if shift_for_blank {
                    layer.insert_frame_at(curr_f);
                }
                layer.insert_keyframe(target_frame, clone_prev, fps);
            }
        }
        self.sync_total_frames();
        self.current_frame = target_frame;
        if let Some(sel) = &mut self.cell_selection {
            sel.min_frame = target_frame;
            sel.max_frame = target_frame;
        }
    }

    /// 対象レイヤー群 `targets` の現在フレームに削除可能なキーフレームが存在するかを返す
    pub(crate) fn can_delete_keyframe_on_targets(&self, targets: &[usize]) -> bool {
        let curr_f = self.current_frame;
        targets.iter().any(|&l_idx| {
            self.layers.get(l_idx).is_some_and(|layer| {
                layer.keyframe_index_at(curr_f).is_some_and(|idx| {
                    layer.start_of(idx) == curr_f && (idx > 0 || !layer.keyframes[idx].is_blank())
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
                    && (l.frame_count > 1 || l.keyframes.first().is_some_and(|kf| !kf.is_blank()))
            })
        })
    }

    /// 対象レイヤー群 `targets` のフレーム（範囲選択時は選択範囲まとめて、単一選択時は1フレーム）を削除する（`Shift+F5`）
    pub(crate) fn remove_frame_on_targets(&mut self, targets: &[usize], fps: u32) -> bool {
        let range_sel = self.cell_selection.filter(|s| s.max_frame > s.min_frame);
        let mut any_changed = false;
        if let Some(sel) = range_sel {
            for &l_idx in targets {
                if let Some(layer) = self.layers.get_mut(l_idx) {
                    if layer.remove_frames(sel.min_frame, sel.max_frame - sel.min_frame + 1, fps) {
                        any_changed = true;
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
                    if layer.remove_frame_at(curr_f, fps) {
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
        fps: u32,
        mut alloc_group_id: impl FnMut() -> u64,
    ) {
        let extracted: Vec<(usize, usize, usize, usize, RelativeKeyframes)> = (sel.min_layer
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
                let mut kfs =
                    src_layer.extract_keyframes_in_range(sel.min_frame, src_max_frame, fps);
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

    /// レイヤーをもう1枚足せるか（上限 `MAX_LAYERS_PER_TIMELINE` の決まりはこれ1つ：足す所 `add_layer_above_active` と、
    /// 浮いた片を確定する前に断る `apply` の入口）
    pub(crate) fn can_add_layer(&self) -> bool {
        self.layers.len() < MAX_LAYERS_PER_TIMELINE
    }

    /// 新しいレイヤーを現在のアクティブレイヤーの1つ上（前面）に追加して選択する（足せない（`can_add_layer`）なら追加しない）
    pub(crate) fn add_layer_above_active(&mut self) {
        if !self.can_add_layer() {
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
        Some((layer.start_of(kf_idx), kf.tween_rotate, is_complete))
    }

    /// 作業中のレイヤーの今のコマで音を付けられる所 `(レイヤーの番号, キーフレームの番号)`（`Layer::sound_slot_at`）。
    /// 読み込みのメニュー・プロパティの「サウンド」の欄・付ける操作のどれもがこれで決める
    pub(crate) fn active_sound_slot(&self) -> Option<(usize, usize)> {
        let k = self
            .layers
            .get(self.active_layer)?
            .sound_slot_at(self.current_frame)?;
        Some((self.active_layer, k))
    }
}
