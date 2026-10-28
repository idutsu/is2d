use eframe::egui::{self, epaint::Vertex, Color32, Mesh, Pos2, Vec2};
use kurbo::{
    Affine, BezPath, ParamCurve, Point, QuadBez, Rect as KRect, Shape as KurboShape, Vec2 as KVec2,
};
use lyon_tessellation::{
    math::point as lyon_point,
    path::Path as LyonPath,
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub(crate) use crate::{
    symbol::{SymbolInstanceBinding, TweenRotate},
    vector_boolean::*,
    vector_stroke::*,
};

pub(crate) const REFERENCE_BRUSH_RADIUS: f64 = 8.0;

static NEXT_MESH_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_mesh_revision() -> u64 {
    NEXT_MESH_REVISION.fetch_add(1, Ordering::Relaxed)
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
    pub(crate) segments: Arc<Vec<QuadBez>>,
}

impl Contour {
    pub(crate) fn new(segments: Vec<QuadBez>) -> Self {
        Self {
            segments: Arc::new(segments),
        }
    }

    pub(crate) fn segments_mut(&mut self) -> &mut Vec<QuadBez> {
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

    fn reverse(&mut self) {
        let segs = self.segments_mut();
        segs.reverse();
        for seg in segs {
            std::mem::swap(&mut seg.p0, &mut seg.p2);
        }
    }

    fn enforce_continuity(&mut self) {
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
        if seg_idx >= self.segments.len() {
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
    pub(crate) fn delete_anchor_at(&mut self, seg_idx: usize) -> bool {
        let n = self.segments.len();
        if n <= 2 || seg_idx >= n {
            return false;
        }
        let prev_idx = (seg_idx + n - 1) % n;
        let p0 = self.segments[prev_idx].p0;
        let p2 = self.segments[seg_idx].p2;
        let p1 = p0.lerp(p2, 0.5);

        let segs = self.segments_mut();
        segs[prev_idx] = QuadBez::new(p0, p1, p2);
        segs.remove(seg_idx);
        self.enforce_continuity();
        true
    }
}

/// スクリーン座標変換済みメッシュキャッシュのキー（`(zoom_bits, pan_x_bits, pan_y_bits, dim_backdrop)`）とメッシュのペア
pub(crate) type ScreenMeshCacheEntry = ((u32, u32, u32, bool), Arc<Mesh>);

/// オニオンスキン変換済みメッシュキャッシュのキー（`(zoom_bits, pan_x_bits, pan_y_bits, tint_rgb, alpha)`）とメッシュのペア
pub(crate) type OnionMeshCacheEntry = ((u32, u32, u32, [u8; 3], u8), Arc<Mesh>);

/// キャンバス上の1つの塗りシェイプ（色 + 外周2次ベジェ輪郭 + 穴の2次ベジェ輪郭リスト + 所属レイヤーID + グループ階層 + シンボル紐付け + 描画・判定キャッシュ）
#[derive(Clone, Debug)]
pub(crate) struct FillShape {
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
    pub(crate) fill_mesh: Arc<Mesh>,
    pub(crate) selection_mesh: Option<(u32, Arc<Mesh>)>,
}

impl FillShape {
    pub(crate) fn new(color: Color32, exterior: Contour, holes: Vec<Contour>) -> Self {
        let mut shape = Self {
            color,
            exterior,
            holes,
            layer_id: 1,
            group_path: Vec::new(),
            symbol_instance: None,
            mesh_revision: 0,
            bbox: KRect::ZERO,
            fill_mesh: Arc::new(Mesh::default()),
            selection_mesh: None,
        };
        shape.rebuild_cache();
        shape
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

    /// 外周および全内周穴の `Contour` を順に走査するイテレータを返す
    pub(crate) fn contours(&self) -> impl Iterator<Item = &Contour> {
        std::iter::once(&self.exterior).chain(self.holes.iter())
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
        let mut bbox = self.exterior.bounding_box();
        for hole in &self.holes {
            bbox = bbox.union(hole.bounding_box());
        }
        bbox
    }

    pub(crate) fn rebuild_cache(&mut self) {
        self.bbox = self.compute_bbox();
        self.fill_mesh = Arc::new(tessellate_contours_to_mesh(
            &self.exterior,
            &self.holes,
            self.color,
        ));
        self.selection_mesh = None;
        self.mesh_revision = next_mesh_revision();
    }

    /// 再テッセレーションを行わずに塗り色とキャッシュ済み頂点カラーのみを高速更新する
    pub(crate) fn set_color(&mut self, color: Color32) {
        if self.color == color {
            return;
        }
        self.color = color;
        for v in &mut Arc::make_mut(&mut self.fill_mesh).vertices {
            v.color = color;
        }
        self.mesh_revision = next_mesh_revision();
    }

    pub(crate) fn bounding_box(&self) -> KRect {
        self.bbox
    }

    pub(crate) fn contains_point(&self, pt: Point) -> bool {
        if !pt.x.is_finite() || !pt.y.is_finite() || !self.bbox.inflate(2.0, 2.0).contains(pt) {
            return false;
        }
        let mut path = BezPath::new();
        for c in self.contours() {
            c.append_to_bez_path(&mut path);
        }
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
        let d = egui::vec2(delta.x as f32, delta.y as f32);
        for v in &mut Arc::make_mut(&mut self.fill_mesh).vertices {
            v.pos += d;
        }
        if let Some((_, sel)) = &mut self.selection_mesh {
            for v in &mut Arc::make_mut(sel).vertices {
                v.pos += d;
            }
        }
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
        self.bbox = self.compute_bbox();
        for v in &mut Arc::make_mut(&mut self.fill_mesh).vertices {
            let x = v.pos.x as f64;
            let y = v.pos.y as f64;
            v.pos = Pos2::new(
                (a * x + c * y + tx) as f32,
                (b * x + d * y + ty) as f32,
            );
        }
        self.selection_mesh = None;
        self.mesh_revision = next_mesh_revision();
    }

    pub(crate) fn selection_overlay_mesh(&mut self, zoom: f32) -> Arc<Mesh> {
        let key = zoom.to_bits();
        if let Some((k, mesh)) = &self.selection_mesh {
            if *k == key {
                return Arc::clone(mesh);
            }
        }
        let mesh = Arc::new(build_polka_dot_selection_mesh(&self.fill_mesh, zoom));
        self.selection_mesh = Some((key, Arc::clone(&mesh)));
        mesh
    }

    pub(crate) fn get_contour(&self, idx: usize) -> Option<&Contour> {
        if idx == 0 {
            Some(&self.exterior)
        } else {
            self.holes.get(idx - 1)
        }
    }

    pub(crate) fn get_contour_mut(&mut self, idx: usize) -> Option<&mut Contour> {
        if idx == 0 {
            Some(&mut self.exterior)
        } else {
            self.holes.get_mut(idx - 1)
        }
    }
}

// ============================================================================
// アフィン変換・クラシックトゥイーン補間・幾何ユーティリティ関数
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
    let start_custom = (start_pivot - Point::ZERO).hypot() > 1e-6;
    let end_custom = (end_pivot - Point::ZERO).hypot() > 1e-6;
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

    let (_, _, rot0, sx0, sy0, sh0) = decompose_affine_trs(start);
    let (_, _, rot1, sx1, sy1, sh1) = decompose_affine_trs(end);

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

fn decompose_affine_trs(affine: Affine) -> (f64, f64, f64, f64, f64, f64) {
    let [a, b, c, d, tx, ty] = affine.as_coeffs();
    let sx = a.hypot(b);
    let (cos_r, sin_r, rot) = if sx > 1e-9 {
        (a / sx, b / sx, b.atan2(a))
    } else {
        (1.0, 0.0, 0.0)
    };
    let shear = c * cos_r + d * sin_r;
    let sy = -c * sin_r + d * cos_r;
    (tx, ty, rot, sx, sy, shear)
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
    let mut sel_mesh = Mesh::default();
    let inv_z = 1.0 / zoom.clamp(0.15, 16.0);
    let step: f32 = 6.0 * inv_z;
    let r_white = 1.15 * inv_z;
    let r_dark = 0.85 * inv_z;
    let offset_dark = 0.7 * inv_z;
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

    for tri in fill_mesh.indices.chunks_exact(3) {
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
                if (c0 >= 0.0 && c1 >= 0.0 && c2 >= 0.0)
                    || (c0 <= 0.0 && c1 <= 0.0 && c2 <= 0.0)
                {
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
