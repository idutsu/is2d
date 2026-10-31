use eframe::egui::{self, epaint::Vertex, Color32, Mesh, Pos2, Vec2};
use kurbo::{
    Affine, BezPath, ParamCurve, Point, QuadBez, Rect as KRect, Shape as KurboShape, Vec2 as KVec2,
};
use lyon_tessellation::{
    math::point as lyon_point, path::Path as LyonPath, BuffersBuilder, FillOptions, FillRule,
    FillTessellator, FillVertex, VertexBuffers,
};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

pub(crate) const REFERENCE_BRUSH_RADIUS: f64 = 8.0;
pub(crate) const MIN_VIEW_ZOOM: f32 = 0.15;
pub(crate) const MAX_VIEW_ZOOM: f32 = 16.0;

/// 1輪郭（`Contour`）あたりの最大許容ベジェセグメント数
pub(crate) const MAX_CONTOUR_SEGMENTS: usize = 100_000;

/// シンボルインスタンスのフレーム再生モード（Adobe Animate のグラフィックシンボル相当）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub(crate) enum SymbolLoopMode {
    /// ループ再生：キーフレーム開始位置からシンボル内部フレームを繰り返し再生する
    #[default]
    Loop,
    /// 1回再生：キーフレーム開始位置からシンボル内部の最終フレームまで再生して停止する
    PlayOnce,
    /// 任意のフレーム：指定された `first_frame` の1コマだけを固定表示する
    SingleFrame,
}

/// シンボルインスタンスに属するシェイプが持つ幾何変換・再生紐付け情報
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SymbolInstanceBinding {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    pub(crate) xform: Affine,
    /// シンボルローカル座標系での変形中心点（ピボット）。初期値は `Point::ZERO` `(0.0, 0.0)`
    pub(crate) pivot: Point,
    pub(crate) loop_mode: SymbolLoopMode,
    pub(crate) first_frame: usize,
}

impl SymbolInstanceBinding {
    #[inline]
    pub(crate) fn new(instance_gid: u64, symbol_id: u64, xform: Affine) -> Self {
        Self {
            instance_gid,
            symbol_id,
            xform,
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
        }
    }
}

static NEXT_MESH_REVISION: AtomicU64 = AtomicU64::new(1);
static NEXT_SHAPE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CONTOUR_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_mesh_revision() -> u64 {
    NEXT_MESH_REVISION.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn next_shape_id() -> u64 {
    NEXT_SHAPE_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn next_contour_id() -> u64 {
    NEXT_CONTOUR_ID.fetch_add(1, Ordering::Relaxed)
}

/// `Arc` と共存可能なスレッドセーフ（`Send + Sync`）かつ `Clone` 対応の内部可変キャッシュラッパー
#[derive(Debug, Default)]
pub(crate) struct SyncCache<T>(Mutex<T>);

impl<T> SyncCache<T> {
    #[inline]
    pub(crate) fn new(val: T) -> Self {
        Self(Mutex::new(val))
    }

    #[inline]
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<T: Clone> Clone for SyncCache<T> {
    fn clone(&self) -> Self {
        Self::new(self.lock().clone())
    }
}

thread_local! {
    static TESSELLATOR: RefCell<FillTessellator> = RefCell::new(FillTessellator::new());
}

// ============================================================================
// コアデータ構造 (2次ベジェ輪郭 & マージ描画シェイプ)
// ============================================================================

/// 閉じた2次ベジェ曲線（QuadBez）のループ輪郭（Copy-on-Write `Arc` 共有）
/// 隣接セグメント間で `segments[i].p2 == segments[(i + 1) % len].p0` を常に満たす。
#[derive(Clone, Debug)]
pub(crate) struct Contour {
    /// 輪郭セグメント内容の単調増加リビジョンID（メモリアドレスに依存しない O(1) 同一性比較・逆引きキー用）
    pub(crate) contour_id: u64,
    pub(crate) segments: Arc<Vec<QuadBez>>,
}

impl Contour {
    pub(crate) fn new(segments: Vec<QuadBez>) -> Self {
        Self {
            contour_id: next_contour_id(),
            segments: Arc::new(segments),
        }
    }

    #[inline]
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        self.contour_id == other.contour_id && Arc::ptr_eq(&self.segments, &other.segments)
    }

    pub(crate) fn segments_mut(&mut self) -> &mut Vec<QuadBez> {
        self.contour_id = next_contour_id();
        Arc::make_mut(&mut self.segments)
    }

    pub(crate) fn translate(&mut self, delta: KVec2) {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return;
        }
        for seg in self.segments_mut() {
            seg.p0 += delta;
            seg.p1 += delta;
            seg.p2 += delta;
        }
    }

    pub(crate) fn transform(&mut self, affine: Affine) {
        let c = affine.as_coeffs();
        let det = c[0] * c[3] - c[1] * c[2];
        if !c.iter().all(|v| v.is_finite()) || det.abs() < 1e-12 {
            return;
        }
        for seg in self.segments_mut() {
            seg.p0 = affine * seg.p0;
            seg.p1 = affine * seg.p1;
            seg.p2 = affine * seg.p2;
        }
        if det < 0.0 {
            self.reverse();
        }
        self.enforce_continuity();
    }

    /// この輪郭のバウンディングボックスを2次ベジェ曲線群から直接計算する
    pub(crate) fn bounding_box(&self) -> KRect {
        let mut iter = self.segments.iter().map(KurboShape::bounding_box);
        let Some(first) = iter.next() else {
            return KRect::ZERO;
        };
        iter.fold(first, |acc, r| acc.union(r))
    }

    pub(crate) fn append_to_bez_path(&self, path: &mut BezPath) {
        if let Some(first) = self.segments.first() {
            path.move_to(first.p0);
            for seg in self.segments.iter() {
                path.quad_to(seg.p1, seg.p2);
            }
            path.close_path();
        }
    }

    /// ポリゴン近似した際の符号付き面積（Shoelace公式）
    pub(crate) fn signed_area(&self) -> f64 {
        let Some(first_seg) = self.segments.first() else {
            return 0.0;
        };
        const SUBDIVS: usize = 8;
        let first_pt = first_seg.eval(0.0);
        let mut prev = first_pt;
        let mut sum = 0.0;
        for seg in self.segments.iter() {
            for k in 0..SUBDIVS {
                let curr = seg.eval(k as f64 / SUBDIVS as f64);
                sum += prev.x * curr.y - curr.x * prev.y;
                prev = curr;
            }
        }
        sum += prev.x * first_pt.y - first_pt.x * prev.y;
        0.5 * sum
    }

    /// 極端に短い退化セグメントや非有限座標セグメントを除去し、端点の連続性を保証する
    fn cleanup_degenerate(&mut self, min_len: f64) {
        let has_non_finite = self.segments.iter().any(|s| {
            !s.p0.x.is_finite()
                || !s.p0.y.is_finite()
                || !s.p1.x.is_finite()
                || !s.p1.y.is_finite()
                || !s.p2.x.is_finite()
                || !s.p2.y.is_finite()
        });
        if has_non_finite || self.segments.len() > 3 {
            let kept: Vec<QuadBez> = self
                .segments
                .iter()
                .copied()
                .filter(|s| {
                    s.p0.x.is_finite()
                        && s.p0.y.is_finite()
                        && s.p1.x.is_finite()
                        && s.p1.y.is_finite()
                        && s.p2.x.is_finite()
                        && s.p2.y.is_finite()
                        && (s.p2 - s.p0).hypot() >= min_len
                })
                .collect();
            if has_non_finite || (kept.len() >= 3 && kept.len() != self.segments.len()) {
                self.segments = Arc::new(kept);
            }
        }
        self.enforce_continuity();
    }

    /// セグメント列から退化除去・最小面積判定・ワインディング向き統一・端点連続性保証を行った `Contour` を構築する
    pub(crate) fn from_segments_oriented(
        segments: Vec<QuadBez>,
        min_seg_len: f64,
        min_area: f64,
        expect_positive_area: bool,
    ) -> Option<Self> {
        let mut contour = Self::new(segments);
        contour.cleanup_degenerate(min_seg_len);
        let area = contour.signed_area();
        if area.abs() < min_area {
            return None;
        }
        if (expect_positive_area && area < 0.0) || (!expect_positive_area && area > 0.0) {
            contour.reverse();
        }
        contour.enforce_continuity();
        Some(contour)
    }

    pub(crate) fn reverse(&mut self) {
        let segs = self.segments_mut();
        segs.reverse();
        for seg in segs {
            std::mem::swap(&mut seg.p0, &mut seg.p2);
        }
    }

    pub(crate) fn enforce_continuity(&mut self) {
        let n = self.segments.len();
        if n == 0 {
            return;
        }
        let segs = self.segments_mut();
        for i in 0..n {
            let next = (i + 1) % n;
            let mid = segs[i].p2.lerp(segs[next].p0, 0.5);
            segs[i].p2 = mid;
            segs[next].p0 = mid;
        }
    }

    /// 指定セグメント `seg_idx` のパラメータ位置 `t` を De Casteljau 分割し、形状を一切変えずに新しいアンカーを追加する
    pub(crate) fn insert_anchor_at(&mut self, seg_idx: usize, t: f64) -> bool {
        if seg_idx >= self.segments.len() || self.segments.len() >= MAX_CONTOUR_SEGMENTS {
            return false;
        }
        let t_clamped = t.clamp(0.08, 0.92);
        let seg = self.segments[seg_idx];
        let left = seg.subsegment(0.0..t_clamped);
        let right = seg.subsegment(t_clamped..1.0);
        let segs = self.segments_mut();
        segs[seg_idx] = left;
        segs.insert(seg_idx + 1, right);
        self.enforce_continuity();
        true
    }

    /// 指定アンカー `seg_idx`（`segments[seg_idx].p0`）を削除し、前後のアンカー（点Aと点C）を直線セグメントで結ぶ
    /// （削除後に輪郭面積が潰れてゼロになる場合は削除を行わず `false` を返す）
    pub(crate) fn delete_anchor_at(&mut self, seg_idx: usize) -> bool {
        let n = self.segments.len();
        if n <= 2 || seg_idx >= n {
            return false;
        }
        let prev_idx = (seg_idx + n - 1) % n;
        let p0 = self.segments[prev_idx].p0;
        let p2 = self.segments[seg_idx].p2;
        let p1 = p0.lerp(p2, 0.5);

        let mut candidate = self.clone();
        let segs = candidate.segments_mut();
        segs[prev_idx] = QuadBez::new(p0, p1, p2);
        segs.remove(seg_idx);
        candidate.enforce_continuity();
        if candidate.signed_area().abs() <= 1e-4 {
            return false;
        }
        *self = candidate;
        true
    }
}

/// スクリーン座標変換済みメッシュキャッシュのキー（`(zoom_bits, pan_x_bits, pan_y_bits, dim_backdrop)`）とメッシュのペア
pub(crate) type ScreenMeshCacheEntry = ((u32, u32, u32, bool), Arc<Mesh>);

/// オニオンスキン変換済みメッシュキャッシュのキー（`(zoom_bits, pan_x_bits, pan_y_bits, tint_rgb, alpha)`）とメッシュのペア
pub(crate) type OnionMeshCacheEntry = ((u32, u32, u32, [u8; 3], u8), Arc<Mesh>);

/// 選択時ドット柄オーバーレイのスクリーン座標変換済みメッシュキャッシュのキー（`(zoom_bits, pan_x_bits, pan_y_bits)`）とメッシュのペア
pub(crate) type SelectionScreenMeshCacheEntry = ((u32, u32, u32), Arc<Mesh>);

/// キャンバス上の1つの塗りシェイプ（色 + 外周2次ベジェ輪郭 + 穴の2次ベジェ輪郭リスト + 所属レイヤーID + グループ階層 + シンボル紐付け + 描画・判定キャッシュ）
#[derive(Clone, Debug)]
pub(crate) struct FillShape {
    /// 各シェイプインスタンスを順序変更・ソート後も安定して識別する固有ID
    pub(crate) shape_id: u64,
    pub(crate) color: Color32,
    pub(crate) exterior: Contour,
    pub(crate) holes: Vec<Contour>,
    /// このシェイプが所属するタイムラインレイヤーの固有ID（デフォルト `1`）
    pub(crate) layer_id: u64,
    /// 外側のグループIDから内側のグループIDへの階層パス（空 `[]` ならルート階層の生シェイプ）
    pub(crate) group_path: Vec<u64>,
    /// このシェイプがシンボルインスタンスに属する場合の紐付け情報
    pub(crate) symbol_instance: Option<SymbolInstanceBinding>,
    /// メッシュ内容が更新されるたびにインクリメントされるリビジョン番号（セル結合メッシュキャッシュの差分検知用）
    pub(crate) mesh_revision: u64,
    bbox: KRect,
    fill_mesh: OnceLock<Arc<Mesh>>,
    hit_bez_path: OnceLock<Arc<BezPath>>,
    screen_mesh_cache: SyncCache<Option<ScreenMeshCacheEntry>>,
    selection_mesh: SyncCache<Option<(u32, Arc<Mesh>)>>,
    selection_screen_mesh: SyncCache<Option<SelectionScreenMeshCacheEntry>>,
}

impl FillShape {
    pub(crate) fn new(color: Color32, exterior: Contour, holes: Vec<Contour>) -> Self {
        let mut shape = Self {
            shape_id: next_shape_id(),
            color,
            exterior,
            holes,
            layer_id: 1,
            group_path: Vec::new(),
            symbol_instance: None,
            mesh_revision: 0,
            bbox: KRect::ZERO,
            fill_mesh: OnceLock::new(),
            hit_bez_path: OnceLock::new(),
            screen_mesh_cache: SyncCache::new(None),
            selection_mesh: SyncCache::new(None),
            selection_screen_mesh: SyncCache::new(None),
        };
        shape.rebuild_cache();
        shape
    }

    /// この要素が空白フレームのシンボルインスタンスを表す非表示マーカーであるか
    #[inline]
    pub(crate) fn is_empty_symbol_marker(&self) -> bool {
        self.exterior.segments.is_empty() && self.symbol_instance.is_some()
    }

    /// 描画用の塗り `egui::Mesh` を遅延テッセレーションして返す（キャッシュ済みの場合は O(1) で参照を返す）
    pub(crate) fn fill_mesh(&self) -> &Arc<Mesh> {
        self.fill_mesh.get_or_init(|| {
            Arc::new(tessellate_contours_to_mesh(
                &self.exterior,
                &self.holes,
                self.color,
            ))
        })
    }

    /// 指定された `(zoom, pan, dim_backdrop)` に対応するスクリーン座標変換済み `Arc<Mesh>` をキャッシュ経由で返す
    pub(crate) fn cached_screen_mesh(&self, zoom: f32, pan: Vec2, dim_backdrop: bool) -> Arc<Mesh> {
        let key = (
            zoom.to_bits(),
            pan.x.to_bits(),
            pan.y.to_bits(),
            dim_backdrop,
        );
        if let Some((k, mesh)) = &*self.screen_mesh_cache.lock() {
            if *k == key {
                return Arc::clone(mesh);
            }
        }
        let screen_mesh = Arc::new(transform_mesh_to_screen(
            self.fill_mesh(),
            zoom,
            pan,
            dim_backdrop,
        ));
        *self.screen_mesh_cache.lock() = Some((key, Arc::clone(&screen_mesh)));
        screen_mesh
    }

    /// いずれかの一時メッシュキャッシュ（`fill_mesh`, `hit_bez_path`, `screen_mesh_cache`, `selection_mesh`, `selection_screen_mesh`）を保持しているか
    #[inline]
    pub(crate) fn has_any_transient_cache(&self) -> bool {
        self.fill_mesh.get().is_some()
            || self.hit_bez_path.get().is_some()
            || self.screen_mesh_cache.lock().is_some()
            || self.selection_mesh.lock().is_some()
            || self.selection_screen_mesh.lock().is_some()
    }

    /// Undo 履歴保存時やバックアップ時に、幾何データから随時再生成可能な描画メッシュキャッシュを破棄してメモリを解放する
    pub(crate) fn strip_transient_caches(&mut self) {
        self.fill_mesh = OnceLock::new();
        self.hit_bez_path = OnceLock::new();
        *self.screen_mesh_cache.lock() = None;
        *self.selection_mesh.lock() = None;
        *self.selection_screen_mesh.lock() = None;
    }

    /// 輪郭データと色が同一である既存シェイプ `donor` から描画・当たり判定キャッシュを O(1) で引き継ぐ
    pub(crate) fn reuse_caches_from(&mut self, donor: &FillShape) -> bool {
        if self.color != donor.color
            || (self.mesh_revision != donor.mesh_revision
                && !self.has_same_contour_arcs(&donor.exterior, &donor.holes))
        {
            return false;
        }
        self.copy_render_caches_from(donor);
        true
    }

    /// ドナー図形 `donor` が保持している描画・当たり判定キャッシュをそのまま引き継ぐ
    pub(crate) fn copy_render_caches_from(&mut self, donor: &Self) {
        if self.fill_mesh.get().is_none() {
            if let Some(m) = donor.fill_mesh.get() {
                let _ = self.fill_mesh.set(Arc::clone(m));
            }
        }
        if self.hit_bez_path.get().is_none() {
            if let Some(p) = donor.hit_bez_path.get() {
                let _ = self.hit_bez_path.set(Arc::clone(p));
            }
        }
        if self.screen_mesh_cache.lock().is_none() {
            if let Some(sc) = &*donor.screen_mesh_cache.lock() {
                *self.screen_mesh_cache.lock() = Some(sc.clone());
            }
        }
        if self.selection_mesh.lock().is_none() {
            if let Some(sm) = &*donor.selection_mesh.lock() {
                *self.selection_mesh.lock() = Some(sm.clone());
            }
        }
        if self.selection_screen_mesh.lock().is_none() {
            if let Some(ssm) = &*donor.selection_screen_mesh.lock() {
                *self.selection_screen_mesh.lock() = Some(ssm.clone());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn has_cached_fill_mesh(&self) -> bool {
        self.fill_mesh.get().is_some()
    }

    pub(crate) fn with_layer_id(mut self, layer_id: u64) -> Self {
        self.layer_id = layer_id;
        self
    }

    pub(crate) fn with_group_path(mut self, group_path: Vec<u64>) -> Self {
        self.group_path = group_path;
        self
    }

    pub(crate) fn with_scope(mut self, layer_id: u64, group_path: &[u64]) -> Self {
        self.layer_id = layer_id;
        self.group_path = group_path.to_vec();
        self
    }

    /// 実体のある外周および全内周穴の `Contour` を順に走査するイテレータを返す
    /// （空白フレームのシンボルマーカーの場合は 0 件の空イテレータを返すため、呼び出し側での個別ガードが不要）
    pub(crate) fn contours(&self) -> impl Iterator<Item = &Contour> {
        (!self.exterior.segments.is_empty())
            .then_some(&self.exterior)
            .into_iter()
            .chain(self.holes.iter())
    }

    /// 指定されたグループ編集スコープ `scope` の内部（直下の生シェイプ、または子孫グループ）に属するか
    pub(crate) fn is_in_editing_scope(&self, scope: &[u64]) -> bool {
        self.group_path.starts_with(scope)
    }

    /// 指定されたグループ編集スコープ `scope` 直下の「生のシェイプ（未グループ）」であるか
    pub(crate) fn is_raw_at_scope(&self, scope: &[u64]) -> bool {
        self.group_path.as_slice() == scope
    }

    /// 指定されたグループ編集スコープ `scope` において、このシェイプが属する直下の子グループIDを返す
    pub(crate) fn active_group_id_at_scope(&self, scope: &[u64]) -> Option<u64> {
        if self.is_in_editing_scope(scope) {
            self.group_path.get(scope.len()).copied()
        } else {
            None
        }
    }

    fn compute_bbox(&self) -> KRect {
        if self.exterior.segments.is_empty() {
            if let Some(b) = &self.symbol_instance {
                let p = b.xform * Point::ZERO;
                return KRect::from_points(p, p);
            }
        }
        let mut bbox = self.exterior.bounding_box();
        for hole in &self.holes {
            bbox = bbox.union(hole.bounding_box());
        }
        bbox
    }

    pub(crate) fn rebuild_cache(&mut self) {
        self.bbox = self.compute_bbox();
        self.fill_mesh = OnceLock::new();
        self.hit_bez_path = OnceLock::new();
        *self.screen_mesh_cache.lock() = None;
        *self.selection_mesh.lock() = None;
        *self.selection_screen_mesh.lock() = None;
        self.mesh_revision = next_mesh_revision();
    }

    /// 再テッセレーションを行わずに塗り色とキャッシュ済み頂点カラーのみを高速更新する
    pub(crate) fn set_color(&mut self, color: Color32) {
        if self.color == color {
            return;
        }
        self.color = color;
        if let Some(arc_mesh) = self.fill_mesh.get_mut() {
            for v in &mut Arc::make_mut(arc_mesh).vertices {
                v.color = color;
            }
        }
        *self.screen_mesh_cache.lock() = None;
        self.mesh_revision = next_mesh_revision();
    }

    /// 外周・穴のすべてが指定した `Contour` の `Arc` とポインタ一致するかを返す
    pub(crate) fn has_same_contour_arcs(&self, exterior: &Contour, holes: &[Contour]) -> bool {
        self.exterior.ptr_eq(exterior)
            && self.holes.len() == holes.len()
            && self
                .holes
                .iter()
                .zip(holes.iter())
                .all(|(a, b)| a.ptr_eq(b))
    }

    pub(crate) fn bounding_box(&self) -> KRect {
        self.bbox
    }

    /// 実体のあるベクター輪郭を持つ場合のみバウンディングボックスを返す（空白フレームのシンボルマーカーの場合は `None`）
    #[inline]
    pub(crate) fn geometry_bounding_box(&self) -> Option<KRect> {
        (!self.is_empty_symbol_marker()).then_some(self.bbox)
    }

    pub(crate) fn contains_point(&self, pt: Point) -> bool {
        if !pt.x.is_finite() || !pt.y.is_finite() || !self.bbox.inflate(2.0, 2.0).contains(pt) {
            return false;
        }
        let path = self.hit_bez_path.get_or_init(|| {
            let mut p = BezPath::new();
            for c in self.contours() {
                c.append_to_bez_path(&mut p);
            }
            Arc::new(p)
        });
        (path.winding(pt) & 1) != 0
    }

    pub(crate) fn translate(&mut self, delta: KVec2) {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return;
        }
        self.exterior.translate(delta);
        for hole in &mut self.holes {
            hole.translate(delta);
        }
        if let Some(binding) = &mut self.symbol_instance {
            binding.xform = Affine::translate(delta) * binding.xform;
        }
        self.bbox = self.bbox + delta;
        self.hit_bez_path = OnceLock::new();
        let d = egui::vec2(delta.x as f32, delta.y as f32);
        if let Some(arc_mesh) = self.fill_mesh.get_mut() {
            for v in &mut Arc::make_mut(arc_mesh).vertices {
                v.pos += d;
            }
        }
        *self.screen_mesh_cache.lock() = None;
        if let Some((_, sel)) = &mut *self.selection_mesh.lock() {
            for v in &mut Arc::make_mut(sel).vertices {
                v.pos += d;
            }
        }
        *self.selection_screen_mesh.lock() = None;
        self.mesh_revision = next_mesh_revision();
    }

    pub(crate) fn transform(&mut self, affine: Affine) {
        let [a, b, c, d, tx, ty] = affine.as_coeffs();
        let det = a * d - b * c;
        if !affine.as_coeffs().iter().all(|v| v.is_finite()) || det.abs() < 1e-12 {
            return;
        }
        self.exterior.transform(affine);
        for hole in &mut self.holes {
            hole.transform(affine);
        }
        if let Some(binding) = &mut self.symbol_instance {
            binding.xform = affine * binding.xform;
        }
        let max_scale_sq = (a * a + b * b).max(c * c + d * d);
        if max_scale_sq > 1.001 * 1.001 {
            self.rebuild_cache();
        } else {
            self.bbox = self.compute_bbox();
            self.hit_bez_path = OnceLock::new();
            if let Some(arc_mesh) = self.fill_mesh.get_mut() {
                for v in &mut Arc::make_mut(arc_mesh).vertices {
                    let x = v.pos.x as f64;
                    let y = v.pos.y as f64;
                    v.pos = Pos2::new((a * x + c * y + tx) as f32, (b * x + d * y + ty) as f32);
                }
            }
            *self.screen_mesh_cache.lock() = None;
            *self.selection_mesh.lock() = None;
            *self.selection_screen_mesh.lock() = None;
            self.mesh_revision = next_mesh_revision();
        }
    }

    /// シンボルインスタンス展開やトゥイーン補間など、既にテッセレーション済みの `fill_mesh` を保持したまま
    /// アフィン変換を適用し、再生中やオニオンスキン描画での毎フレームの再テッセレーションを回避する
    pub(crate) fn transform_with_cached_mesh(&mut self, affine: Affine) {
        let [a, b, c, d, tx, ty] = affine.as_coeffs();
        let det = a * d - b * c;
        if !affine.as_coeffs().iter().all(|v| v.is_finite()) || det.abs() < 1e-12 {
            return;
        }
        self.exterior.transform(affine);
        for hole in &mut self.holes {
            hole.transform(affine);
        }
        if let Some(binding) = &mut self.symbol_instance {
            binding.xform = affine * binding.xform;
        }
        let max_scale_sq = (a * a + b * b).max(c * c + d * d);
        if max_scale_sq > 16.0 && !self.exterior.segments.is_empty() {
            self.rebuild_cache();
            return;
        }
        self.bbox = self.compute_bbox();
        self.hit_bez_path = OnceLock::new();
        if let Some(arc_mesh) = self.fill_mesh.get_mut() {
            for v in &mut Arc::make_mut(arc_mesh).vertices {
                let x = v.pos.x as f64;
                let y = v.pos.y as f64;
                v.pos = Pos2::new((a * x + c * y + tx) as f32, (b * x + d * y + ty) as f32);
            }
        }
        *self.screen_mesh_cache.lock() = None;
        *self.selection_mesh.lock() = None;
        *self.selection_screen_mesh.lock() = None;
        self.mesh_revision = next_mesh_revision();
    }

    /// 自由変形ドラッグ中、ドラッグ開始時の `orig` から既存の単一所有 `fill_mesh` 頂点バッファを再利用して `affine` を適用する
    /// （`clone_from` による `Arc` 参照カウント増加と毎フレームの頂点バッファ再確保を回避する）
    pub(crate) fn transform_from_origin(&mut self, orig: &FillShape, affine: Affine) {
        let [a, b, c, d, tx, ty] = affine.as_coeffs();
        let det = a * d - b * c;
        if !affine.as_coeffs().iter().all(|v| v.is_finite()) || det.abs() < 1e-12 {
            self.clone_from(orig);
            return;
        }
        self.exterior = orig.exterior.clone();
        self.exterior.transform(affine);
        self.holes.clone_from(&orig.holes);
        for hole in &mut self.holes {
            hole.transform(affine);
        }
        self.symbol_instance = orig.symbol_instance.map(|mut binding| {
            binding.xform = affine * binding.xform;
            binding
        });
        self.bbox = self.compute_bbox();
        self.hit_bez_path = OnceLock::new();
        if let Some(src_mesh) = orig.fill_mesh.get() {
            let reused = if let Some(dst_arc) = self.fill_mesh.get_mut() {
                if dst_arc.vertices.len() == src_mesh.vertices.len()
                    && dst_arc.indices.len() == src_mesh.indices.len()
                {
                    let dst_mesh = Arc::make_mut(dst_arc);
                    for (dst_v, src_v) in dst_mesh.vertices.iter_mut().zip(src_mesh.vertices.iter())
                    {
                        let x = src_v.pos.x as f64;
                        let y = src_v.pos.y as f64;
                        dst_v.pos =
                            Pos2::new((a * x + c * y + tx) as f32, (b * x + d * y + ty) as f32);
                        dst_v.color = src_v.color;
                        dst_v.uv = src_v.uv;
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !reused {
                let mut cloned = (**src_mesh).clone();
                for v in &mut cloned.vertices {
                    let x = v.pos.x as f64;
                    let y = v.pos.y as f64;
                    v.pos = Pos2::new((a * x + c * y + tx) as f32, (b * x + d * y + ty) as f32);
                }
                self.fill_mesh = OnceLock::from(Arc::new(cloned));
            }
        } else {
            self.fill_mesh = OnceLock::new();
        }
        *self.screen_mesh_cache.lock() = None;
        *self.selection_mesh.lock() = None;
        *self.selection_screen_mesh.lock() = None;
        self.mesh_revision = next_mesh_revision();
    }

    pub(crate) fn selection_overlay_mesh(&self, zoom: f32) -> Arc<Mesh> {
        let key = zoom.to_bits();
        if let Some((k, mesh)) = &*self.selection_mesh.lock() {
            if *k == key {
                return Arc::clone(mesh);
            }
        }
        let mesh = Arc::new(build_polka_dot_selection_mesh(self.fill_mesh(), zoom));
        *self.selection_mesh.lock() = Some((key, Arc::clone(&mesh)));
        mesh
    }

    /// 指定された `(zoom, pan)` に対応するスクリーン座標変換済みのドット柄選択オーバーレイ `Arc<Mesh>` をキャッシュ経由で返す
    pub(crate) fn cached_selection_screen_mesh(&self, zoom: f32, pan: Vec2) -> Arc<Mesh> {
        let key = (zoom.to_bits(), pan.x.to_bits(), pan.y.to_bits());
        if let Some((k, mesh)) = &*self.selection_screen_mesh.lock() {
            if *k == key {
                return Arc::clone(mesh);
            }
        }
        let world_sel = self.selection_overlay_mesh(zoom);
        let screen_sel = Arc::new(transform_mesh_to_screen(&world_sel, zoom, pan, false));
        *self.selection_screen_mesh.lock() = Some((key, Arc::clone(&screen_sel)));
        screen_sel
    }

    pub(crate) fn get_contour(&self, idx: usize) -> Option<&Contour> {
        if self.exterior.segments.is_empty() {
            return None;
        }
        if idx == 0 {
            Some(&self.exterior)
        } else {
            self.holes.get(idx - 1)
        }
    }

    pub(crate) fn get_contour_mut(&mut self, idx: usize) -> Option<&mut Contour> {
        if self.exterior.segments.is_empty() {
            return None;
        }
        if idx == 0 {
            Some(&mut self.exterior)
        } else {
            self.holes.get_mut(idx - 1)
        }
    }
}

// ============================================================================
// アフィン変換・幾何ユーティリティ関数
// ============================================================================

/// 指定基準点 `anchor` を中心とした非一様スケーリング（および反転）のアフィン変換行列を生成する
pub(crate) fn affine_scale_around(anchor: Point, sx: f64, sy: f64) -> Affine {
    Affine::translate(anchor.to_vec2())
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate(-anchor.to_vec2())
}

/// 指定基準点 `pivot` を中心とした回転のアフィン変換行列を生成する
pub(crate) fn affine_rotate_around(pivot: Point, angle_rad: f64) -> Affine {
    Affine::translate(pivot.to_vec2())
        * Affine::rotate(angle_rad)
        * Affine::translate(-pivot.to_vec2())
}

/// 矩形選択用の `KRect` から4辺の直線 `QuadBez` を持つ `FillShape` を生成する
pub(crate) fn make_marquee_rect_shape(rect: KRect) -> FillShape {
    let p00 = Point::new(rect.x0, rect.y0);
    let p10 = Point::new(rect.x1, rect.y0);
    let p11 = Point::new(rect.x1, rect.y1);
    let p01 = Point::new(rect.x0, rect.y1);
    let segs = vec![
        QuadBez::new(p00, p00.lerp(p10, 0.5), p10),
        QuadBez::new(p10, p10.lerp(p11, 0.5), p11),
        QuadBez::new(p11, p11.lerp(p01, 0.5), p01),
        QuadBez::new(p01, p01.lerp(p00, 0.5), p00),
    ];
    FillShape::new(Color32::WHITE, Contour::new(segs), Vec::new())
}

#[inline]
pub(crate) fn ring_f64_to_points(ring: &[[f64; 2]]) -> Vec<Point> {
    ring.iter().map(|&[x, y]| Point::new(x, y)).collect()
}

#[inline]
pub(crate) fn normalize_vec(v: KVec2) -> KVec2 {
    let len = v.hypot();
    if len > 1e-9 {
        v / len
    } else {
        KVec2::new(1.0, 0.0)
    }
}

#[inline]
pub(crate) fn left_normal(v: KVec2) -> KVec2 {
    KVec2::new(-v.y, v.x)
}

#[inline]
pub(crate) fn signed_angle(a: KVec2, b: KVec2) -> f64 {
    (a.x * b.y - a.y * b.x).atan2((a.x * b.x + a.y * b.y).clamp(-1.0, 1.0))
}

#[inline]
pub(crate) fn to_pos2(p: Point) -> Pos2 {
    Pos2::new(p.x as f32, p.y as f32)
}

pub(crate) fn fill_cumulative_arc_lengths(pts: &[Point], cum: &mut Vec<f64>) {
    cum.clear();
    cum.reserve(pts.len());
    if pts.is_empty() {
        return;
    }
    let mut acc = 0.0;
    cum.push(0.0);
    for i in 1..pts.len() {
        acc += (pts[i] - pts[i - 1]).hypot();
        cum.push(acc);
    }
}

pub(crate) fn dedup_closed_ring(pts: impl IntoIterator<Item = Point>, min_dist: f64) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::new();
    for p in pts {
        if out.last().is_none_or(|&last| (p - last).hypot() > min_dist) {
            out.push(p);
        }
    }
    if out.len() >= 2 && (out[0] - out[out.len() - 1]).hypot() <= min_dist {
        out.pop();
    }
    out
}

pub(crate) fn signed_area_f64(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let p0 = pts[i];
        let p1 = pts[(i + 1) % n];
        sum += p0[0] * p1[1] - p1[0] * p0[1];
    }
    0.5 * sum
}

pub(crate) fn ring_area_and_perimeter_f64(ring: &[[f64; 2]]) -> (f64, f64) {
    let area = signed_area_f64(ring).abs();
    let mut perim = 0.0;
    for i in 0..ring.len() {
        let next = (i + 1) % ring.len();
        perim += (ring[next][0] - ring[i][0]).hypot(ring[next][1] - ring[i][1]);
    }
    (area, perim)
}

pub(crate) fn polygon_signed_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let p0 = pts[i];
        let p1 = pts[(i + 1) % n];
        sum += p0.x * p1.y - p1.x * p0.y;
    }
    0.5 * sum
}

pub(crate) fn point_to_segment_dist(pt: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len_sq = ab.hypot2();
    if len_sq < 1e-9 {
        return (pt - a).hypot();
    }
    let t = ((pt - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (pt - (a + ab * t)).hypot()
}

pub(crate) fn rects_overlap(a: KRect, b: KRect) -> bool {
    a.x0 <= b.x1 && a.x1 >= b.x0 && a.y0 <= b.y1 && a.y1 >= b.y0
}

pub(crate) fn union_bounding_boxes(shapes: &[FillShape]) -> KRect {
    let Some(first) = shapes.first() else {
        return KRect::ZERO;
    };
    shapes[1..]
        .iter()
        .fold(first.bounding_box(), |acc, s| acc.union(s.bounding_box()))
}

// ============================================================================
// Lyon テッセレーション描画エンジン (2次ベジェパス -> GPU Mesh)
// ============================================================================

fn tessellate_contours_to_mesh(exterior: &Contour, holes: &[Contour], color: Color32) -> Mesh {
    let mut builder = LyonPath::builder();
    for contour in std::iter::once(exterior).chain(holes.iter()) {
        if let Some(first) = contour.segments.first() {
            let p0 = first.p0;
            builder.begin(lyon_point(p0.x as f32, p0.y as f32));
            for seg in contour.segments.iter() {
                let p1 = seg.p1;
                let p2 = seg.p2;
                builder.quadratic_bezier_to(
                    lyon_point(p1.x as f32, p1.y as f32),
                    lyon_point(p2.x as f32, p2.y as f32),
                );
            }
            builder.close();
        }
    }
    let path = builder.build();
    TESSELLATOR.with(|t| tessellate_lyon_path(&mut t.borrow_mut(), &path, color, FillRule::EvenOdd))
}

fn build_polka_dot_selection_mesh(fill_mesh: &Mesh, zoom: f32) -> Mesh {
    const MAX_POLKA_DOT_GRID_CELLS: f32 = 25_000.0;
    const MAX_POLKA_DOT_VERTICES: usize = (MAX_POLKA_DOT_GRID_CELLS as usize) * 8;

    let mut sel_mesh = Mesh::default();
    if fill_mesh.vertices.is_empty() || fill_mesh.indices.len() < 3 {
        return sel_mesh;
    }

    let mut mesh_min_x = f32::INFINITY;
    let mut mesh_max_x = f32::NEG_INFINITY;
    let mut mesh_min_y = f32::INFINITY;
    let mut mesh_max_y = f32::NEG_INFINITY;
    for v in &fill_mesh.vertices {
        mesh_min_x = mesh_min_x.min(v.pos.x);
        mesh_max_x = mesh_max_x.max(v.pos.x);
        mesh_min_y = mesh_min_y.min(v.pos.y);
        mesh_max_y = mesh_max_y.max(v.pos.y);
    }
    let bbox_area = (mesh_max_x - mesh_min_x).max(0.0) * (mesh_max_y - mesh_min_y).max(0.0);
    if !bbox_area.is_finite() {
        return sel_mesh;
    }

    let base_inv_z = 1.0 / zoom.clamp(MIN_VIEW_ZOOM, MAX_VIEW_ZOOM);
    let min_step_for_budget = (bbox_area / MAX_POLKA_DOT_GRID_CELLS).sqrt();
    let step: f32 = (6.0 * base_inv_z).max(min_step_for_budget).max(1e-3);
    let scale = step / 6.0;
    let r_white = 1.15 * scale;
    let r_dark = 0.85 * scale;
    let offset_dark = 0.7 * scale;
    let white = Color32::from_rgba_unmultiplied(255, 255, 255, 220);
    let dark = Color32::from_rgba_unmultiplied(20, 20, 20, 180);

    let push_dot_quad = |mesh: &mut Mesh, center: Pos2, r: f32, color: Color32| {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend_from_slice(&[
            Vertex {
                pos: Pos2::new(center.x - r, center.y - r),
                uv: egui::epaint::WHITE_UV,
                color,
            },
            Vertex {
                pos: Pos2::new(center.x + r, center.y - r),
                uv: egui::epaint::WHITE_UV,
                color,
            },
            Vertex {
                pos: Pos2::new(center.x + r, center.y + r),
                uv: egui::epaint::WHITE_UV,
                color,
            },
            Vertex {
                pos: Pos2::new(center.x - r, center.y + r),
                uv: egui::epaint::WHITE_UV,
                color,
            },
        ]);
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };

    'tri_loop: for tri in fill_mesh.indices.chunks_exact(3) {
        let a = fill_mesh.vertices[tri[0] as usize].pos;
        let b = fill_mesh.vertices[tri[1] as usize].pos;
        let c = fill_mesh.vertices[tri[2] as usize].pos;

        let min_x = a.x.min(b.x).min(c.x);
        let max_x = a.x.max(b.x).max(c.x);
        let min_y = a.y.min(b.y).min(c.y);
        let max_y = a.y.max(b.y).max(c.y);

        let row_start = (min_y / step).floor() as i32;
        let row_end = (max_y / step).ceil() as i32;

        let ab = b - a;
        let bc = c - b;
        let ca = a - c;

        for row in row_start..=row_end {
            let y = row as f32 * step;
            if y < min_y || y > max_y {
                continue;
            }
            let x_shift = if row.rem_euclid(2) == 0 {
                0.0
            } else {
                step * 0.5
            };
            let col_start = ((min_x - x_shift) / step).floor() as i32;
            let col_end = ((max_x - x_shift) / step).ceil() as i32;

            for col in col_start..=col_end {
                let x = col as f32 * step + x_shift;
                if x < min_x || x > max_x {
                    continue;
                }
                let c0 = ab.x * (y - a.y) - ab.y * (x - a.x);
                let c1 = bc.x * (y - b.y) - bc.y * (x - b.x);
                let c2 = ca.x * (y - c.y) - ca.y * (x - c.x);
                if (c0 >= 0.0 && c1 >= 0.0 && c2 >= 0.0) || (c0 <= 0.0 && c1 <= 0.0 && c2 <= 0.0) {
                    if sel_mesh.vertices.len() + 8 > MAX_POLKA_DOT_VERTICES {
                        break 'tri_loop;
                    }
                    let pt = Pos2::new(x, y);
                    push_dot_quad(&mut sel_mesh, pt, r_white, white);
                    push_dot_quad(
                        &mut sel_mesh,
                        Pos2::new(pt.x + offset_dark, pt.y + offset_dark),
                        r_dark,
                        dark,
                    );
                }
            }
        }
    }

    sel_mesh
}

pub(crate) fn tessellate_raw_polygon(pts: &[Point], color: Color32) -> Mesh {
    if pts.len() < 3 {
        return Mesh::default();
    }
    let mut builder = LyonPath::builder();
    builder.begin(lyon_point(pts[0].x as f32, pts[0].y as f32));
    for p in &pts[1..] {
        builder.line_to(lyon_point(p.x as f32, p.y as f32));
    }
    builder.close();
    let path = builder.build();
    TESSELLATOR.with(|t| tessellate_lyon_path(&mut t.borrow_mut(), &path, color, FillRule::NonZero))
}

fn tessellate_lyon_path(
    tessellator: &mut FillTessellator,
    path: &LyonPath,
    color: Color32,
    fill_rule: FillRule,
) -> Mesh {
    let mut buffers: VertexBuffers<Vertex, u32> = VertexBuffers::new();
    let options = FillOptions::tolerance(0.04).with_fill_rule(fill_rule);

    if tessellator
        .tessellate_path(
            path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| {
                let p = vertex.position();
                Vertex {
                    pos: Pos2::new(p.x, p.y),
                    uv: egui::epaint::WHITE_UV,
                    color,
                }
            }),
        )
        .is_err()
    {
        return Mesh::default();
    }

    Mesh {
        vertices: buffers.vertices,
        indices: buffers.indices,
        ..Default::default()
    }
}

/// ワールド座標で構築された `Mesh` を現在のビューポート（`zoom`, `pan`）に合わせてスクリーン座標へ変換する
pub(crate) fn transform_mesh_to_screen(
    mesh: &Mesh,
    zoom: f32,
    pan: Vec2,
    dim_backdrop: bool,
) -> Mesh {
    let mut out = mesh.clone();
    let is_identity_view = (zoom - 1.0).abs() < 1e-5 && pan == Vec2::ZERO;
    if is_identity_view && !dim_backdrop {
        return out;
    }
    for v in &mut out.vertices {
        v.pos = Pos2::new(v.pos.x * zoom + pan.x, v.pos.y * zoom + pan.y);
        if dim_backdrop {
            let [r, g, b, a] = v.color.to_srgba_unmultiplied();
            v.color = Color32::from_rgba_unmultiplied(
                ((r as u16 * 30 + 250 * 70) / 100) as u8,
                ((g as u16 * 30 + 250 * 70) / 100) as u8,
                ((b as u16 * 30 + 252 * 70) / 100) as u8,
                a,
            );
        }
    }
    out
}

/// オニオンスキン表示用に、ワールド座標の `Mesh` をスクリーン座標へ変換しつつ指定されたティント色・不透明度へブレンドする
pub(crate) fn transform_mesh_to_screen_onion(
    mesh: &Mesh,
    zoom: f32,
    pan: Vec2,
    tint_rgb: [u8; 3],
    alpha: u8,
) -> Mesh {
    let mut out = mesh.clone();
    for v in &mut out.vertices {
        v.pos = Pos2::new(v.pos.x * zoom + pan.x, v.pos.y * zoom + pan.y);
        let [r, g, b, _] = v.color.to_srgba_unmultiplied();
        v.color = Color32::from_rgba_unmultiplied(
            ((r as u16 * 35 + tint_rgb[0] as u16 * 65) / 100) as u8,
            ((g as u16 * 35 + tint_rgb[1] as u16 * 65) / 100) as u8,
            ((b as u16 * 35 + tint_rgb[2] as u16 * 65) / 100) as u8,
            alpha,
        );
    }
    out
}

/// 複数の `Mesh` を1つのバッチ `Mesh` に結合する（セル単位の一括キャッシュ構築用）
pub(crate) fn append_mesh(dst: &mut Mesh, src: &Mesh) {
    if src.is_empty() {
        return;
    }
    let base = dst.vertices.len() as u32;
    dst.vertices.extend_from_slice(&src.vertices);
    dst.indices
        .extend(src.indices.iter().map(|&idx| base + idx));
}

/// `Arc<egui::Mesh>` を単一所有時はムーブ、共有時はクローンで `egui::Shape::Mesh` に変換する
#[inline]
pub(crate) fn arc_mesh_to_shape(mesh: Arc<Mesh>) -> egui::Shape {
    egui::Shape::Mesh(Arc::unwrap_or_clone(mesh))
}
