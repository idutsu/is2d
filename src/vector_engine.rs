use eframe::egui::{self, epaint::Vertex, Color32, Mesh, Pos2, Vec2};
use i_overlay::{
    core::{fill_rule::FillRule as OverlayFillRule, overlay_rule::OverlayRule, solver::Solver},
    float::{filter::ContourFilter, overlay::FloatOverlay, simplify::SimplifyShape},
};
use kurbo::{
    Affine, BezPath, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, Point,
    QuadBez, Rect as KRect, Shape as KurboShape, Vec2 as KVec2,
};
use lyon_tessellation::{
    math::{point as lyon_point, Point as LyonPoint},
    path::Path as LyonPath,
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};
use std::sync::atomic::{AtomicU64, Ordering};

const REFERENCE_BRUSH_RADIUS: f64 = 8.0;

static NEXT_MESH_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_mesh_revision() -> u64 {
    NEXT_MESH_REVISION.fetch_add(1, Ordering::Relaxed)
}

// ============================================================================
// コアデータ構造 (2次ベジェ輪郭 & マージ描画シェイプ)
// ============================================================================

/// 閉じた2次ベジェ曲線（QuadBez）のループ輪郭
/// 隣接セグメント間で `segments[i].p2 == segments[(i + 1) % len].p0` を常に満たす。
#[derive(Clone, Debug)]
pub(crate) struct Contour {
    pub(crate) segments: Vec<QuadBez>,
}

impl Contour {
    pub(crate) fn translate(&mut self, delta: KVec2) {
        for seg in &mut self.segments {
            seg.p0 += delta;
            seg.p1 += delta;
            seg.p2 += delta;
        }
    }

    pub(crate) fn transform(&mut self, affine: Affine) {
        for seg in &mut self.segments {
            seg.p0 = affine * seg.p0;
            seg.p1 = affine * seg.p1;
            seg.p2 = affine * seg.p2;
        }
        let c = affine.as_coeffs();
        let det = c[0] * c[3] - c[1] * c[2];
        if det < 0.0 {
            self.reverse();
        }
        self.enforce_continuity();
    }

    pub(crate) fn append_to_bez_path(&self, path: &mut BezPath) {
        if let Some(first) = self.segments.first() {
            path.move_to(first.p0);
            for seg in &self.segments {
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
        for seg in &self.segments {
            for k in 0..SUBDIVS {
                let curr = seg.eval(k as f64 / SUBDIVS as f64);
                sum += prev.x * curr.y - curr.x * prev.y;
                prev = curr;
            }
        }
        sum += prev.x * first_pt.y - first_pt.x * prev.y;
        0.5 * sum
    }

    /// 極端に短い退化セグメントを除去し、端点の連続性を保証する
    fn cleanup_degenerate(&mut self, min_len: f64) {
        if self.segments.len() > 3 {
            let kept: Vec<QuadBez> = self
                .segments
                .iter()
                .copied()
                .filter(|seg| (seg.p2 - seg.p0).hypot() >= min_len)
                .collect();
            if kept.len() >= 3 {
                self.segments = kept;
            }
        }
        self.enforce_continuity();
    }

    /// セグメント列から退化除去・最小面積判定・ワインディング向き統一・端点連続性保証を行った `Contour` を構築する
    fn from_segments_oriented(
        segments: Vec<QuadBez>,
        min_seg_len: f64,
        min_area: f64,
        expect_positive_area: bool,
    ) -> Option<Self> {
        let mut contour = Self { segments };
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
        self.segments.reverse();
        for seg in &mut self.segments {
            std::mem::swap(&mut seg.p0, &mut seg.p2);
        }
    }

    fn enforce_continuity(&mut self) {
        let n = self.segments.len();
        if n == 0 {
            return;
        }
        for i in 0..n {
            let next = (i + 1) % n;
            let mid = self.segments[i].p2.lerp(self.segments[next].p0, 0.5);
            self.segments[i].p2 = mid;
            self.segments[next].p0 = mid;
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
        self.segments[seg_idx] = left;
        self.segments.insert(seg_idx + 1, right);
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

        self.segments[prev_idx] = QuadBez::new(p0, p1, p2);
        self.segments.remove(seg_idx);
        self.enforce_continuity();
        true
    }
}

pub(crate) use crate::symbol::{SymbolInstanceBinding, TweenRotate};

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
    bez_path: BezPath,
    pub(crate) fill_mesh: Mesh,
    pub(crate) selection_mesh: Option<(u32, Mesh)>,
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
            bez_path: BezPath::new(),
            fill_mesh: Mesh::default(),
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

    pub(crate) fn rebuild_cache(&mut self) {
        let mut path = BezPath::new();
        for contour in self.contours() {
            contour.append_to_bez_path(&mut path);
        }
        self.bbox = path.bounding_box();
        self.bez_path = path;
        let mut tessellator = FillTessellator::new();
        self.fill_mesh = tessellate_contours_to_mesh(
            &mut tessellator,
            &self.exterior,
            &self.holes,
            self.color,
        );
        self.selection_mesh = None;
        self.mesh_revision = next_mesh_revision();
    }

    pub(crate) fn bounding_box(&self) -> KRect {
        self.bbox
    }

    pub(crate) fn contains_point(&self, pt: Point) -> bool {
        if !self.bbox.inflate(2.0, 2.0).contains(pt) {
            return false;
        }
        (self.bez_path.winding(pt) & 1) != 0
    }

    pub(crate) fn translate(&mut self, delta: KVec2) {
        self.exterior.translate(delta);
        for hole in &mut self.holes {
            hole.translate(delta);
        }
        if let Some(binding) = &mut self.symbol_instance {
            binding.xform = Affine::translate(delta) * binding.xform;
        }
        self.bbox = self.bbox + delta;
        self.bez_path.apply_affine(kurbo::Affine::translate(delta));
        let d = egui::vec2(delta.x as f32, delta.y as f32);
        for v in &mut self.fill_mesh.vertices {
            v.pos += d;
        }
        if let Some((_, sel)) = &mut self.selection_mesh {
            for v in &mut sel.vertices {
                v.pos += d;
            }
        }
        self.mesh_revision = next_mesh_revision();
    }

    pub(crate) fn transform(&mut self, affine: Affine) {
        self.exterior.transform(affine);
        for hole in &mut self.holes {
            hole.transform(affine);
        }
        if let Some(binding) = &mut self.symbol_instance {
            binding.xform = affine * binding.xform;
        }
        self.rebuild_cache();
    }

    pub(crate) fn selection_overlay_mesh(&mut self, zoom: f32) -> &Mesh {
        let z_bits = zoom.to_bits();
        if self.selection_mesh.as_ref().is_none_or(|(b, _)| *b != z_bits) {
            self.selection_mesh = Some((
                z_bits,
                build_polka_dot_selection_mesh(&self.fill_mesh, zoom),
            ));
        }
        &self.selection_mesh.as_ref().unwrap().1
    }

    pub(crate) fn contour_count(&self) -> usize {
        1 + self.holes.len()
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
    // 片方のキーフレームのみピボットが移動されている（もう片方が初期位置 (0, 0) のまま）場合は、
    // 移動された側のローカルピボットを区間共通の回転・変形軸として採用する
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
    FillShape::new(Color32::WHITE, Contour { segments: segs }, Vec::new())
}

// ============================================================================
// アルゴリズム 1: ブラシ軌跡からのスケール不変な「閉じた輪郭（アウトライン）」生成
// ============================================================================

/// マウスドラッグ中の座標列 `points` とブラシ半径 `radius` から、左右の法線オフセット点列・角の丸ジョイン（Round Join）・
/// 始点終点の半円キャップ（Round Cap）を計算し、マイター突出のない厳密な丸ブラシ包絡線を生成する。
#[cfg(test)]
pub(crate) fn generate_brush_outline(points: &[Point], radius: f64) -> Vec<Point> {
    generate_brush_outline_at_zoom(points, radius, 1.0)
}

/// ズーム倍率 `zoom` に応じてスクリーン空間で100%表示時と完全に同一のサンプリング精度を保つ丸ブラシ包絡線生成
pub(crate) fn generate_brush_outline_at_zoom(
    points: &[Point],
    radius: f64,
    zoom: f64,
) -> Vec<Point> {
    if points.is_empty() {
        return Vec::new();
    }

    let z = zoom.clamp(0.15, 16.0);
    let r = radius.max(0.05);
    let screen_r = r * z;
    let min_step = (0.15 * screen_r).clamp(0.2, 0.8) / z;

    // スクリーン空間のブラシ半径に比例した最小ステップ閾値で近接重複サンプルを除去
    let mut clean: Vec<Point> = Vec::with_capacity(points.len());
    for &pt in points {
        if clean.last().is_none_or(|&last| (pt - last).hypot() >= min_step) {
            clean.push(pt);
        }
    }

    // 単一クリック（スタンプ）の場合は円形輪郭を生成
    if clean.len() <= 1 {
        let c = points[0];
        let steps = 28;
        return (0..steps)
            .map(|i| {
                let theta = (i as f64 / steps as f64) * std::f64::consts::TAU;
                Point::new(c.x + r * theta.cos(), c.y + r * theta.sin())
            })
            .collect();
    }

    let n = clean.len();
    let mut seg_tangents = Vec::with_capacity(n - 1);
    let mut seg_normals = Vec::with_capacity(n - 1);
    for i in 0..(n - 1) {
        let t = normalize_vec(clean[i + 1] - clean[i]);
        seg_tangents.push(t);
        seg_normals.push(left_normal(t));
    }

    // 始点・終点キャップの向きは、微小な手ブレに振られないよう 2 ステップ先の弦方向も加味する
    let end_step = 2usize.min(n - 1);
    let t_start = normalize_vec(clean[end_step] - clean[0]);
    let n_start = left_normal(t_start);
    let t_end = normalize_vec(clean[n - 1] - clean[n - 1 - end_step]);
    let n_end = left_normal(t_end);

    let mut left_pts = Vec::with_capacity(n * 2);
    let mut right_pts = Vec::with_capacity(n * 2);

    left_pts.push(clean[0] + n_start * r);
    right_pts.push(clean[0] - n_start * r);

    for i in 1..(n - 1) {
        let p = clean[i];
        let t_in = seg_tangents[i - 1];
        let t_out = seg_tangents[i];
        let n_in = seg_normals[i - 1];
        let n_out = seg_normals[i];

        let turn_angle = signed_angle(t_in, t_out);
        let w = 2usize.min(i).min(n - 1 - i).max(1);
        let tw_in = normalize_vec(clean[i] - clean[i - w]);
        let tw_out = normalize_vec(clean[i + w] - clean[i]);
        let macro_turn = signed_angle(tw_in, tw_out);

        if turn_angle.abs() > 0.08 && macro_turn.abs() > 0.25 && turn_angle * macro_turn > 0.0 {
            let abs_ang = turn_angle.abs();
            let arc_steps = ((abs_ang / (std::f64::consts::PI / 12.0)).ceil() as usize).clamp(1, 16);
            if turn_angle > 0.0 {
                // 右側がターンの外側（凸側） -> 半径 r の円弧を挿入
                for k in 0..=arc_steps {
                    let a = (k as f64 / arc_steps as f64) * abs_ang;
                    right_pts.push(p + (-n_in * a.cos() + t_in * a.sin()) * r);
                }
                left_pts.push(p + n_in * r);
                left_pts.push(p + n_out * r);
            } else {
                // 左側がターンの外側（凸側） -> 半径 r の円弧を挿入
                for k in 0..=arc_steps {
                    let a = (k as f64 / arc_steps as f64) * abs_ang;
                    left_pts.push(p + (n_in * a.cos() + t_in * a.sin()) * r);
                }
                right_pts.push(p - n_in * r);
                right_pts.push(p - n_out * r);
            }
        } else {
            let n_smooth = left_normal(normalize_vec(tw_in + tw_out));
            left_pts.push(p + n_smooth * r);
            right_pts.push(p - n_smooth * r);
        }
    }

    left_pts.push(clean[n - 1] + n_end * r);
    right_pts.push(clean[n - 1] - n_end * r);

    let cap_subdivs = 16usize;
    let mut contour = Vec::with_capacity(right_pts.len() + left_pts.len() + cap_subdivs * 2);

    // 1. 右側オフセット点列 (始点 -> 終点)
    contour.extend_from_slice(&right_pts);

    // 2. 終点の半円キャップ (-n_end -> +t_end -> +n_end)
    let p_end = clean[n - 1];
    for k in 1..cap_subdivs {
        let angle = (k as f64 / cap_subdivs as f64) * std::f64::consts::PI;
        contour.push(p_end + (-n_end * angle.cos() + t_end * angle.sin()) * r);
    }

    // 3. 左側オフセット点列 (終点 -> 始点)
    contour.extend(left_pts.iter().rev().copied());

    // 4. 始点の半円キャップ (+n_start -> -t_start -> -n_start)
    let p_start = clean[0];
    for k in 1..cap_subdivs {
        let angle = (k as f64 / cap_subdivs as f64) * std::f64::consts::PI;
        contour.push(p_start + (n_start * angle.cos() - t_start * angle.sin()) * r);
    }

    contour
}

// ============================================================================
// アルゴリズム 2: 描いた形・線幅・角の丸みを変えずにアンカー数を最小化する 2次ベジェ輪郭構築
// ============================================================================

/// 生のブラシ輪郭点列 `raw_outline` から、ドラッグ中に見えていた形・太さ・丸ブラシの角の丸みをそのまま保ち、
/// 不要な中間アンカーだけを削減した最小アンカー数の `FillShape` 群を生成する（Adobe Animate 方式）。
#[cfg(test)]
pub(crate) fn build_smoothed_stroke_shapes(
    raw_outline: &[Point],
    color: Color32,
) -> Vec<FillShape> {
    build_smoothed_stroke_shapes_at_zoom(raw_outline, color, 1.0)
}

/// ズーム倍率 `zoom` に合わせてスクリーン空間基準で平滑化・ベジェ近似の許容誤差をスケーリングし、
/// 100% 表示時と拡大表示時で完全に同じスムーズ化挙動を実現する。
pub(crate) fn build_smoothed_stroke_shapes_at_zoom(
    raw_outline: &[Point],
    color: Color32,
    zoom: f64,
) -> Vec<FillShape> {
    if raw_outline.len() < 3 {
        return Vec::new();
    }

    let z = zoom.clamp(0.15, 16.0);
    let to_world = Affine::scale(1.0 / z);
    let scale_to_world = |mut contour: Contour| -> Contour {
        if (z - 1.0).abs() > 1e-9 {
            contour.transform(to_world);
        }
        contour
    };

    let raw_f64: Vec<[f64; 2]> = raw_outline.iter().map(|&p| [p.x * z, p.y * z]).collect();
    let simplified = raw_f64.simplify_shape(OverlayFillRule::NonZero, 0.05);

    if simplified.is_empty() {
        let screen_outline = ring_f64_to_points(&raw_f64);
        let r_est = estimate_brush_radius_from_ring(&screen_outline);
        return smooth_ring_to_quad_contour(&screen_outline, r_est, true)
            .into_iter()
            .map(|exterior| FillShape::new(color, scale_to_world(exterior), Vec::new()))
            .collect();
    }

    let mut result = Vec::with_capacity(simplified.len());
    for shape_rings in simplified {
        if shape_rings.is_empty() {
            continue;
        }
        let r_est = estimate_brush_radius_from_shape_rings(&shape_rings);
        let ext_pts = ring_f64_to_points(&shape_rings[0]);
        let Some(exterior) = smooth_ring_to_quad_contour(&ext_pts, r_est, true) else {
            continue;
        };

        let holes = shape_rings[1..]
            .iter()
            .filter_map(|hole_ring| {
                let hole_pts = ring_f64_to_points(hole_ring);
                smooth_ring_to_quad_contour(&hole_pts, r_est, false).map(&scale_to_world)
            })
            .collect();

        result.push(FillShape::new(color, scale_to_world(exterior), holes));
    }

    result
}

/// ポリゴンリング群（外周＋内周穴）の正味面積と総周長の比（Steiner の管状近傍公式 `Area / Perimeter ≈ r`）から
/// ストロークを描いたスクリーン空間でのブラシ半径 `r` を推定する。
fn estimate_brush_radius_from_shape_rings(shape_rings: &[Vec<[f64; 2]>]) -> f64 {
    let mut net_area = 0.0;
    let mut total_perim = 0.0;
    for (idx, ring) in shape_rings.iter().enumerate() {
        if ring.len() < 3 {
            continue;
        }
        let (a, p) = ring_area_and_perimeter_f64(ring);
        net_area = if idx == 0 {
            net_area + a
        } else {
            (net_area - a).max(0.1)
        };
        total_perim += p;
    }
    if total_perim < 1e-3 {
        return REFERENCE_BRUSH_RADIUS;
    }
    let circularity = (4.0 * std::f64::consts::PI * net_area) / (total_perim * total_perim);
    let r = if shape_rings.len() == 1 && circularity > 0.78 {
        2.0 * net_area / total_perim
    } else {
        1.05 * net_area / total_perim
    };
    r.clamp(0.3, 80.0)
}

fn estimate_brush_radius_from_ring(ring: &[Point]) -> f64 {
    let f64_ring: Vec<[f64; 2]> = ring.iter().map(|p| [p.x, p.y]).collect();
    estimate_brush_radius_from_shape_rings(&[f64_ring])
}

/// スクリーン空間の閉じたポリゴン頂点リング `ring` の全体の図形・線幅・角の丸みを損なわずに、
/// ブラシ半径 `r_est` とリング自身の周長半径に応じたスケール不変パラメータでピクセルジャギーだけを整え、
/// 単一の `QuadBez` で近似できる中間アンカーを統合して最小アンカー数の `Contour` を構築する。
fn smooth_ring_to_quad_contour(
    ring: &[Point],
    r_est: f64,
    expect_positive_area: bool,
) -> Option<Contour> {
    if ring.len() < 3 {
        return None;
    }

    // 画面上（スクリーン空間）でのブラシ半径とリング自身の周長半径の両方を基準にスケール係数を算出し、
    // 小さな円や小さな内周穴でも形状が勝手に潰れたり丸められたりしないよう許容誤差を自動縮小する
    let raw_perim: f64 = (0..ring.len())
        .map(|i| (ring[(i + 1) % ring.len()] - ring[i]).hypot())
        .sum();
    let ring_r = raw_perim / std::f64::consts::TAU;
    let r_scale = (r_est / REFERENCE_BRUSH_RADIUS)
        .clamp(0.05, 1.5)
        .min((ring_r / 20.0).clamp(0.05, 1.5));
    let small_ring_factor = (ring_r / 42.0).clamp(0.25, 1.0);

    // 1. スクリーン空間で一定の精度を保つ等間隔リサンプリング
    let clean_tol = (0.35 * r_scale * small_ring_factor).clamp(0.05, 0.40);
    let (mut dense, actual_ds) = resample_closed_ring_uniform(ring, clean_tol, r_scale)?;
    let m = dense.len();

    // 2. 鋭角コーナー（交差部・四角穴の内角）と丸円弧（Round Cap / Round Corner）の分類および基準位置回転
    let w_short = ((4.0 * r_scale / actual_ds).round() as usize).clamp(1, (m / 8).max(1));
    let w_local = ((6.0 * r_scale / actual_ds).round() as usize).clamp(2, (m / 6).max(2));
    let w_macro = ((18.0 * r_scale / actual_ds).round() as usize).clamp(4, (m / 5).max(4));
    let (is_sharp_corner, is_high_curv) = classify_and_align_ring_features(
        &mut dense,
        w_short,
        w_local,
        w_macro,
        r_scale,
        actual_ds,
        expect_positive_area,
    );

    // 3. ゼロ収縮（Zero-Shrinkage）2スケール移動平均フィルタによるピクセルジャギー除去
    apply_zero_shrinkage_smoothing(&mut dense, &is_high_curv, r_scale * small_ring_factor);

    // 4. 保護された高曲率ラン（鋭角ピーク・丸コーナー出入口・180度丸キャップ中点）のロック頂点決定
    let locked = select_locked_feature_anchors(&dense, &is_sharp_corner, &is_high_curv, w_short);

    // 5. 単一 QuadBez フィット誤差に基づく反復アンカー削減
    let max_edge_tol = 0.50 * r_scale * small_ring_factor;
    let max_cap_tol = (0.36 * r_scale * small_ring_factor).max(0.06);
    let max_curve_turn_rad = 0.82 + 0.40 * small_ring_factor;
    let anchors = reduce_outline_anchors_by_quad_fit(
        &dense,
        &locked,
        &is_high_curv,
        max_edge_tol,
        max_cap_tol,
        max_curve_turn_rad,
    );
    if anchors.len() < 2 {
        return None;
    }

    // 6. 各アンカー区間に形状保持2次ベジェ曲線（QuadBez）をフィッティングし、
    //    滑らかな中間アンカーで前後の接線方向を一直線に揃える（G1連続フィッティング）
    let straight_snap_tol = (0.18 * r_scale).clamp(0.04, 0.20);
    let k = anchors.len();
    let mut segments = Vec::with_capacity(k);
    let mut is_straight = Vec::with_capacity(k);
    let mut span_pts = Vec::new();
    let mut cum = Vec::new();
    for j in 0..k {
        fill_cyclic_span(&dense, anchors[j], anchors[(j + 1) % k], &mut span_pts);
        let p0 = *span_pts.first()?;
        let p2 = *span_pts.last()?;
        let straight = max_deviation_from_chord(&span_pts) <= straight_snap_tol;
        let p1 = if straight {
            p0.lerp(p2, 0.5)
        } else {
            fill_cumulative_arc_lengths(&span_pts, &mut cum);
            fit_quad_control_point_with_arc_lengths(&span_pts, &cum, p0, p2)
        };
        segments.push(QuadBez::new(p0, p1, p2));
        is_straight.push(straight);
    }

    enforce_g1_tangent_continuity_on_segments(
        &mut segments,
        &dense,
        &anchors,
        &is_sharp_corner,
        &is_straight,
        r_scale,
    );

    Contour::from_segments_oriented(
        segments,
        (0.45 * r_scale).clamp(0.08, 0.5),
        (2.0 * r_scale * r_scale).max(0.25),
        expect_positive_area,
    )
}

/// 閉じた頂点リングをスクリーン空間スケール `r_scale` に応じた等間隔ステップでリサンプリングする
fn resample_closed_ring_uniform(
    ring: &[Point],
    clean_tol: f64,
    r_scale: f64,
) -> Option<(Vec<Point>, f64)> {
    let clean = dedup_closed_ring(ring.iter().copied(), clean_tol);
    if clean.len() < 3 {
        return None;
    }

    let mut cum_arc = vec![0.0; clean.len() + 1];
    for i in 0..clean.len() {
        cum_arc[i + 1] = cum_arc[i] + (clean[(i + 1) % clean.len()] - clean[i]).hypot();
    }
    let perimeter = cum_arc[clean.len()];
    if perimeter < 3.0 * r_scale {
        return None;
    }

    let step_ds = (2.0 * r_scale).clamp(0.15, 2.0).min(perimeter / 36.0);
    let num_dense = ((perimeter / step_ds).round() as usize).clamp(16, 7200);
    let actual_ds = perimeter / num_dense as f64;
    let mut dense = Vec::with_capacity(num_dense);
    let mut seg_idx = 0usize;

    for k in 0..num_dense {
        let target_s = (k as f64) * actual_ds;
        while seg_idx + 1 < clean.len() && cum_arc[seg_idx + 1] < target_s {
            seg_idx += 1;
        }
        let seg_len = (cum_arc[seg_idx + 1] - cum_arc[seg_idx]).max(1e-9);
        let local_t = ((target_s - cum_arc[seg_idx]) / seg_len).clamp(0.0, 1.0);
        dense.push(clean[seg_idx].lerp(clean[(seg_idx + 1) % clean.len()], local_t));
    }

    Some((dense, actual_ds))
}

/// `flags` 内の連続する `true` 区間 `(start, end)`（両端含む）を列挙する
fn find_true_runs(flags: &[bool]) -> Vec<(usize, usize)> {
    let m = flags.len();
    let mut runs = Vec::new();
    let mut idx = 0usize;
    while idx < m {
        if !flags[idx] {
            idx += 1;
            continue;
        }
        let start = idx;
        while idx + 1 < m && flags[idx + 1] {
            idx += 1;
        }
        runs.push((start, idx));
        idx += 1;
    }
    runs
}

/// 連続ラン `run_start..=run_end` の前後を、`is_sharp_corner` を跨がない範囲で最大2頂点ずつ拡張した `(before_i, after_i)` を返す
#[inline]
fn clamp_run_bounds(
    run_start: usize,
    run_end: usize,
    m: usize,
    is_sharp_corner: &[bool],
) -> (usize, usize) {
    let mut b = run_start;
    for _ in 0..2 {
        if b == 0 || is_sharp_corner[b - 1] {
            break;
        }
        b -= 1;
    }
    let mut a = run_end;
    for _ in 0..2 {
        if a + 1 >= m || is_sharp_corner[a + 1] {
            break;
        }
        a += 1;
    }
    (b, a)
}

/// 輪郭頂点を「鋭角コーナー (`is_sharp_corner`)」「丸円弧 (`is_round_arc`)」「低曲率側面」に分類し、
/// 最長の低曲率側面の中央が index 0 になるよう回転して返す。
fn classify_and_align_ring_features(
    dense: &mut [Point],
    w_short: usize,
    w_local: usize,
    w_macro: usize,
    r_scale: f64,
    actual_ds: f64,
    is_exterior: bool,
) -> (Vec<bool>, Vec<bool>) {
    let m = dense.len();
    let winding_sign = if polygon_signed_area(dense) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let signed_turn_at = |pts: &[Point], idx: usize, w: usize| -> f64 {
        let d_in = normalize_vec(pts[idx] - pts[(idx + m - w) % m]);
        let d_out = normalize_vec(pts[(idx + w) % m] - pts[idx]);
        signed_angle(d_in, d_out)
    };

    let mut t_1 = vec![0.0; m];
    let mut t_short = vec![0.0; m];
    let mut t_local = vec![0.0; m];
    let mut t_macro = vec![0.0; m];

    for i in 0..m {
        t_1[i] = signed_turn_at(dense, i, 1);
        t_short[i] = signed_turn_at(dense, i, w_short);
        t_local[i] = signed_turn_at(dense, i, w_local);
        t_macro[i] = signed_turn_at(dense, i, w_macro);
    }

    // (A) 鋭角コーナー・自己交差分岐点（外周の凹V字ノッチや、穴の内角など）の検出
    let mut is_sharp_corner = detect_sharp_corners(
        &t_1,
        &t_short,
        &t_local,
        &t_macro,
        w_short,
        w_local,
        w_macro,
        winding_sign,
        is_exterior,
    );

    // 鋭角ノッチ近傍の頂点では、`t_local` / `t_macro` の計測窓が V字ノッチを跨いで相殺されるのを防ぐため窓をクランプする
    if is_sharp_corner.iter().any(|&b| b) {
        let clamped_turn = |idx: usize, max_w: usize| -> f64 {
            let w_b = (1..=max_w)
                .find(|&step| is_sharp_corner[(idx + m - step) % m])
                .unwrap_or(max_w);
            let w_f = (1..=max_w)
                .find(|&step| is_sharp_corner[(idx + step) % m])
                .unwrap_or(max_w);
            let d_in = normalize_vec(dense[idx] - dense[(idx + m - w_b) % m]);
            let d_out = normalize_vec(dense[(idx + w_f) % m] - dense[idx]);
            signed_angle(d_in, d_out)
        };
        for i in 0..m {
            if !is_sharp_corner[i] {
                t_local[i] = clamped_turn(i, w_local);
                t_macro[i] = clamped_turn(i, w_macro);
            }
        }
    }

    // (B) 滑らかな丸円弧（Round Cap / Round Corner）の候補検出
    let mut is_round_arc =
        detect_round_arcs(&t_short, &t_local, &t_macro, &is_sharp_corner, w_macro);

    let mut is_high_curv: Vec<bool> = (0..m)
        .map(|i| is_sharp_corner[i] || is_round_arc[i])
        .collect();

    if let Some(rot_idx) = find_longest_false_run_midpoint(&is_high_curv) {
        dense.rotate_left(rot_idx);
        t_local.rotate_left(rot_idx);
        t_macro.rotate_left(rot_idx);
        is_sharp_corner.rotate_left(rot_idx);
        is_round_arc.rotate_left(rot_idx);
        is_high_curv.rotate_left(rot_idx);
    }

    // `is_round_arc` の連続ランを検証し、180度丸キャップまたは局所的な丸コーナーのみを保護する
    filter_protected_round_features(
        dense,
        &t_local,
        &t_macro,
        &is_sharp_corner,
        &is_round_arc,
        &mut is_high_curv,
        w_local,
        w_macro,
        r_scale,
        actual_ds,
    );

    (is_sharp_corner, is_high_curv)
}

/// 鋭角コーナー・自己交差分岐点（外周の凹V字ノッチや、穴の内角など）を検出する
#[allow(clippy::too_many_arguments)]
fn detect_sharp_corners(
    t_1: &[f64],
    t_short: &[f64],
    t_local: &[f64],
    t_macro: &[f64],
    w_short: usize,
    w_local: usize,
    w_macro: usize,
    winding_sign: f64,
    is_exterior: bool,
) -> Vec<bool> {
    let m = t_short.len();
    let max_cap_scan = (w_macro + w_local).min(m / 3);
    let scan_adjacent_cap_side = |idx: usize, forward: bool| -> bool {
        let mut net = 0.0_f64;
        let mut in_arc = false;
        let mut hit_other_notch = false;
        for d in 1..=max_cap_scan {
            let p = if forward {
                (idx + d) % m
            } else {
                (idx + m - d) % m
            };
            let ts_conv = t_short[p] * winding_sign;
            if ts_conv >= 0.08 {
                in_arc = true;
                net += t_1[p] * winding_sign;
            } else if in_arc {
                let adj = if forward {
                    (p + 1) % m
                } else {
                    (p + m - 1) % m
                };
                if ts_conv <= -0.30 || t_short[adj] * winding_sign <= -0.30 {
                    hit_other_notch = true;
                }
                break;
            } else if d > w_short && ts_conv < -0.10 {
                break;
            }
        }
        net >= 2.20 || (hit_other_notch && net >= 1.10)
    };

    let mut is_sharp_corner = vec![false; m];
    for i in 0..m {
        if is_exterior && t_short[i] * winding_sign > 0.0 {
            continue;
        }
        let ts_abs = t_short[i].abs();
        let tl_abs = t_local[i].abs();
        let tm_abs = t_macro[i].abs();

        let t1_curr = t_1[i];
        let t1_prev = if t_1[(i + m - 1) % m] * t1_curr > 0.0 {
            t_1[(i + m - 1) % m]
        } else {
            0.0
        };
        let t1_next = if t_1[(i + 1) % m] * t1_curr > 0.0 {
            t_1[(i + 1) % m]
        } else {
            0.0
        };
        let t_peak_2v =
            (t1_curr + if t1_prev.abs() >= t1_next.abs() { t1_prev } else { t1_next }).abs();

        let is_oscillating_wobble = t_short[(i + m - w_local) % m] * t_short[i] < -0.10
            || t_short[(i + w_local) % m] * t_short[i] < -0.10;

        let is_cap_adjacent_notch = is_exterior
            && t_short[i] * winding_sign < 0.0
            && t1_curr * winding_sign < 0.0
            && ts_abs >= 0.44
            && t_peak_2v >= 0.38
            && t_peak_2v >= 0.74 * ts_abs
            && t_1[(i + m - 1) % m] * t1_curr >= -0.35 * t1_curr.abs()
            && t_1[(i + 1) % m] * t1_curr >= -0.35 * t1_curr.abs()
            && (scan_adjacent_cap_side(i, false) || scan_adjacent_cap_side(i, true));

        let macro_valid = !is_exterior || t_macro[i] * winding_sign < 0.0;
        let is_right_sharp = macro_valid
            && t_short[i].cos() < 0.45
            && tl_abs > 1.00
            && t_macro[i].cos() < 0.48;
        let is_concentrated_junction = macro_valid
            && !is_oscillating_wobble
            && ts_abs > 0.62
            && tl_abs > 0.62
            && tm_abs > 0.55
            && ts_abs >= 0.80 * tl_abs
            && t_short[i] * t_macro[i] > 0.0;

        if is_cap_adjacent_notch || is_right_sharp || is_concentrated_junction {
            let s_i = if t_short[i] >= 0.0 { 1.0 } else { -1.0 };
            let mag_i = t_short[i] * s_i;
            let is_min = (1..=w_local).all(|d| {
                let mag_prev = t_short[(i + m - d) % m] * s_i;
                let mag_next = t_short[(i + d) % m] * s_i;
                mag_i >= mag_prev && (mag_i > mag_next || (mag_i == mag_next && d > 1))
            });
            if is_min {
                is_sharp_corner[i] = true;
            }
        }
    }
    is_sharp_corner
}

/// 滑らかな丸円弧（Round Cap / Round Corner）の候補頂点を検出する
fn detect_round_arcs(
    t_short: &[f64],
    t_local: &[f64],
    t_macro: &[f64],
    is_sharp_corner: &[bool],
    w_macro: usize,
) -> Vec<bool> {
    let m = t_short.len();
    let mut is_round_arc = vec![false; m];
    for i in 0..m {
        if is_sharp_corner[i] {
            continue;
        }
        let near_same_sign_sharp = (1..=w_macro).any(|d| {
            let next = (i + d) % m;
            let prev = (i + m - d) % m;
            (is_sharp_corner[next] && t_short[next] * t_short[i] > 0.0)
                || (is_sharp_corner[prev] && t_short[prev] * t_short[i] > 0.0)
        });
        if near_same_sign_sharp {
            continue;
        }
        let (ts, tl, tm) = (t_short[i], t_local[i], t_macro[i]);
        if ts.abs() > 0.11 && tl.abs() > 0.18 && tm.abs() > 0.48 && ts * tm > 0.0 && tl * tm > 0.0 {
            is_round_arc[i] = true;
        }
    }

    // 1頂点ギャップ埋めと孤立1頂点除去
    let orig_ra = is_round_arc.clone();
    for i in 0..m {
        let prev = orig_ra[(i + m - 1) % m];
        let next = orig_ra[(i + 1) % m];
        if !orig_ra[i] && prev && next {
            is_round_arc[i] = true;
        } else if orig_ra[i] && !prev && !next {
            is_round_arc[i] = false;
        }
    }

    // 6割以上が丸円弧で鋭角を持たない場合は円・楕円ループなので特定箇所を180度キャップと誤認しないよう解除する
    if !is_sharp_corner.iter().any(|&b| b)
        && is_round_arc.iter().filter(|&&b| b).count() * 5 >= m * 3
    {
        is_round_arc.fill(false);
    }
    is_round_arc
}

/// `is_round_arc` の連続ランを検証し、180度丸キャップまたは局所的な丸コーナーのみを `is_high_curv` として保護する
#[allow(clippy::too_many_arguments)]
fn filter_protected_round_features(
    dense: &[Point],
    t_local: &[f64],
    t_macro: &[f64],
    is_sharp_corner: &[bool],
    is_round_arc: &[bool],
    is_high_curv: &mut [bool],
    w_local: usize,
    w_macro: usize,
    r_scale: f64,
    actual_ds: f64,
) {
    if !is_round_arc.iter().any(|&b| b) || is_round_arc.iter().all(|&b| b) {
        return;
    }
    let m = dense.len();
    let flank_len = ((44.0 * r_scale / actual_ds).round() as usize).clamp(5, (m / 4).max(5));
    let local_to_macro = (w_macro as f64 / w_local.max(1) as f64).max(1.0);
    let mut is_cap_vertex = vec![false; m];
    let run_metrics: Vec<(usize, usize, f64)> = find_true_runs(is_round_arc)
        .into_iter()
        .map(|(rs, re)| {
            let (b_i, a_i) = clamp_run_bounds(rs, re, m, is_sharp_corner);
            let (nt, _) = span_turn_metrics(&dense[b_i..=a_i]);
            if nt >= 1.85 && (re - rs) < m / 2 {
                is_cap_vertex[rs..=re].fill(true);
            }
            (rs, re, nt)
        })
        .collect();

    for (r_start, r_end, net_turn) in run_metrics {
        let is_round_cap = is_cap_vertex[r_start];
        let sum_corner_tm: f64 = (r_start..=r_end).map(|k| t_macro[k]).sum();
        let corner_sign = if sum_corner_tm >= 0.0 { 1.0 } else { -1.0 };

        let mean_flank_turn = |start_idx: usize, forward: bool| -> f64 {
            let mut sum = 0.0;
            let mut cnt = 0usize;
            for d in 1..(w_local + flank_len) {
                let i = if forward {
                    (start_idx + d) % m
                } else {
                    (start_idx + m - (d % m)) % m
                };
                if is_sharp_corner[i] || is_cap_vertex[i] {
                    break;
                }
                if d >= w_local {
                    sum += t_local[i] * local_to_macro * corner_sign;
                    cnt += 1;
                }
            }
            if cnt > 0 {
                (sum / cnt as f64).max(0.0)
            } else {
                0.0
            }
        };

        let flank_before = mean_flank_turn(r_start, false);
        let flank_after = mean_flank_turn(r_end, true);
        let peak_abs_tm = (r_start..=r_end)
            .map(|k| t_macro[k].abs())
            .fold(0.0_f64, f64::max);
        let peak_abs_tl = (r_start..=r_end)
            .map(|k| t_local[k].abs())
            .fold(0.0_f64, f64::max);

        let is_localized_round_corner = (r_end - r_start) <= m / 3
            && ((net_turn >= 0.58
                && flank_before < 0.28
                && flank_after < 0.28
                && flank_before < 0.42 * peak_abs_tm
                && flank_after < 0.42 * peak_abs_tm)
                || (net_turn >= 0.88
                    && peak_abs_tl >= 0.52
                    && flank_before < 0.52 * peak_abs_tm
                    && flank_after < 0.52 * peak_abs_tm));

        if !is_round_cap && !is_localized_round_corner {
            for k in r_start..=r_end {
                if !is_sharp_corner[k] {
                    is_high_curv[k] = false;
                }
            }
        }
    }
}

/// ゼロ収縮（Zero-Shrinkage）2スケール移動平均フィルタで、描いた形や凹凸を丸めずに輪郭表面のギザギザだけを除去する
fn apply_zero_shrinkage_smoothing(dense: &mut [Point], is_high_curv: &[bool], r_scale: f64) {
    let m = dense.len();
    let mut dist_to_hc = vec![m; m];
    if is_high_curv.iter().any(|&b| b) {
        for (i, &hc) in is_high_curv.iter().enumerate() {
            if hc {
                dist_to_hc[i] = 0;
            }
        }
        for step in 0..(2 * m) {
            let idx = step % m;
            let prev = (idx + m - 1) % m;
            dist_to_hc[idx] = dist_to_hc[idx].min(dist_to_hc[prev].saturating_add(1));
        }
        for step in (0..(2 * m)).rev() {
            let idx = step % m;
            let next = (idx + 1) % m;
            dist_to_hc[idx] = dist_to_hc[idx].min(dist_to_hc[next].saturating_add(1));
        }
    }

    const SMOOTH_PASSES: usize = 4;
    let max_w = 3usize.min((m / 10).max(1));
    let w_wide = 6usize.min(m / 6).max(3);
    let max_shift = 0.45 * r_scale;
    let orig_pts = dense.to_vec();
    let mut prev_pts = vec![Point::ORIGIN; m];
    let mut deltas = vec![KVec2::ZERO; m];

    for _ in 0..SMOOTH_PASSES {
        prev_pts.copy_from_slice(dense);
        deltas.fill(KVec2::ZERO);

        for i in 0..m {
            let w = dist_to_hc[i].min(max_w);
            if w == 0 {
                continue;
            }
            let mut sum = KVec2::ZERO;
            for k in 1..=w {
                sum += prev_pts[(i + m - k) % m].to_vec2() + prev_pts[(i + k) % m].to_vec2();
            }
            deltas[i] = (sum / (2 * w) as f64).to_point() - prev_pts[i];
        }

        for i in 0..m {
            let d_hc = dist_to_hc[i];
            if d_hc == 0 {
                continue;
            }
            let mut w_sum = deltas[i];
            let mut w_cnt = 1usize;
            for k in 1..=w_wide.min(d_hc) {
                let im = (i + m - k) % m;
                let ip = (i + k) % m;
                if dist_to_hc[im] >= 1 && dist_to_hc[ip] >= 1 {
                    w_sum += deltas[im] + deltas[ip];
                    w_cnt += 2;
                }
            }
            let wide_avg = if w_cnt >= 3 {
                w_sum / w_cnt as f64
            } else {
                KVec2::ZERO
            };
            let fade = (d_hc as f64 / 2.0).clamp(0.0, 1.0);
            let eff_delta = (deltas[i] - wide_avg * 1.02) * fade;

            let next_pt = prev_pts[i] + eff_delta * 0.50;
            let local_max_shift = max_shift * fade;
            let total_disp = next_pt - orig_pts[i];
            let disp_len = total_disp.hypot();
            dense[i] = if disp_len > local_max_shift {
                orig_pts[i] + total_disp * (local_max_shift / disp_len)
            } else {
                next_pt
            };
        }
    }
}

/// 平滑化後の輪郭上で、保護された高曲率ラン（鋭角ピーク・丸コーナー出入口・180度丸キャップ中点）のロック頂点を選択する
fn select_locked_feature_anchors(
    dense: &[Point],
    is_sharp_corner: &[bool],
    is_high_curv: &[bool],
    w_short: usize,
) -> Vec<bool> {
    let m = dense.len();
    let mut locked = vec![false; m];
    if is_high_curv.iter().all(|&b| b) {
        for idx in [0, m / 4, m / 2, (3 * m) / 4] {
            locked[idx] = true;
        }
        return locked;
    }

    let mut cos_s = vec![1.0; m];
    let mut local_tangents = vec![KVec2::new(1.0, 0.0); m];
    for i in 0..m {
        let d_in = normalize_vec(dense[i] - dense[(i + m - w_short) % m]);
        let d_out = normalize_vec(dense[(i + w_short) % m] - dense[i]);
        cos_s[i] = d_in.dot(d_out);
        local_tangents[i] = normalize_vec(dense[(i + 1) % m] - dense[(i + m - 1) % m]);
    }

    // 1. 鋭角コーナー・自己交差V字ノッチは1点に折れ曲がりが集中しているため、頂点そのものを直接ロックする
    for (i, &sharp) in is_sharp_corner.iter().enumerate() {
        if sharp {
            locked[i] = true;
        }
    }

    // 2. 丸ブラシの外角や丸キャップ（`is_high_curv && !is_sharp_corner`）の連続ランを走査し、
    //    隣接する鋭角ノッチと混ざって1点ロックに退化しないよう独立に出入口（`i_start` / `i_end`）と中点をロックする
    let is_round_only: Vec<bool> = (0..m)
        .map(|i| is_high_curv[i] && !is_sharp_corner[i])
        .collect();

    for (run_start, run_end) in find_true_runs(&is_round_only) {
        let peak_i = (run_start..=run_end)
            .min_by(|&a, &b| cos_s[a].partial_cmp(&cos_s[b]).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((run_start + run_end) / 2);

        let (before_i, after_i) = clamp_run_bounds(run_start, run_end, m, is_sharp_corner);
        let t_in = local_tangents[before_i];
        let t_out = local_tangents[after_i];
        let (total_turn, _) = span_turn_metrics(&dense[before_i..=after_i]);

        if total_turn < 0.45 {
            continue;
        }

        let i_start = (before_i..=after_i)
            .take_while(|&k| signed_angle(t_in, local_tangents[k]).abs() <= 0.18)
            .last()
            .unwrap_or(run_start);

        let i_end = (before_i..=after_i)
            .rev()
            .take_while(|&k| signed_angle(local_tangents[k], t_out).abs() <= 0.18)
            .last()
            .unwrap_or(run_end);

        let adj_sharp = (1..=3).any(|d| {
            is_sharp_corner[(run_start + m - (d % m)) % m]
                || is_sharp_corner[(run_end + d) % m]
        });

        if i_end <= i_start + 1 {
            locked[peak_i] = true;
        } else {
            locked[i_start] = true;
            locked[i_end] = true;
            if adj_sharp {
                locked[peak_i] = true;
            }
            if total_turn > 1.75 && i_end >= i_start + 4 {
                let mid_target = 0.5 * total_turn;
                if let Some(best_mid) = ((i_start + 1)..i_end).min_by(|&a, &b| {
                    let da = (signed_angle(t_in, local_tangents[a]).abs() - mid_target).abs();
                    let db = (signed_angle(t_in, local_tangents[b]).abs() - mid_target).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                }) {
                    locked[best_mid] = true;
                }
            }
        }
    }

    locked
}

/// `flags` 配列内で `false` が連続する最長区間の中央インデックスを返す
fn find_longest_false_run_midpoint(flags: &[bool]) -> Option<usize> {
    let m = flags.len();
    if m == 0 || flags.iter().all(|&b| b) {
        return None;
    }
    let Some(first_true) = flags.iter().position(|&b| b) else {
        return Some(m / 16);
    };

    let mut best_start = 0usize;
    let mut best_len = 0usize;
    let mut curr_start = 0usize;
    let mut curr_len = 0usize;

    for step in 0..=m {
        let idx = (first_true + step) % m;
        if !flags[idx] {
            if curr_len == 0 {
                curr_start = idx;
            }
            curr_len += 1;
        } else if curr_len > 0 {
            if curr_len > best_len {
                best_len = curr_len;
                best_start = curr_start;
            }
            curr_len = 0;
        }
    }

    (best_len > 0).then_some((best_start + best_len / 2) % m)
}

/// 点列 `pts` の始点 `p0` と終点 `p2` を固定したとき、輪郭の弧長中点 $M$ を正確に通過する制御点 `p1_mid` と
/// 最小二乗制御点 `p1_ls` をブレンドし、曲線や丸キャップ・丸コーナーが内側に痩せない 2次ベジェ制御点 `p1` を求める。
fn fit_quad_control_point_with_arc_lengths(
    pts: &[Point],
    cum: &[f64],
    p0: Point,
    p2: Point,
) -> Point {
    let n = pts.len();
    if n <= 2 {
        return p0.lerp(p2, 0.5);
    }

    let total = cum[n - 1];
    if total < 1e-6 {
        return p0.lerp(p2, 0.5);
    }

    let half_s = 0.5 * total;
    let idx = cum
        .partition_point(|&s| s < half_s)
        .saturating_sub(1)
        .min(n - 2);
    let seg_len = (cum[idx + 1] - cum[idx]).max(1e-9);
    let local_t = ((half_s - cum[idx]) / seg_len).clamp(0.0, 1.0);
    let mid_pt = pts[idx].lerp(pts[idx + 1], local_t);

    // B(0.5) = 0.25 P0 + 0.5 P1 + 0.25 P2 = M  ==>  P1 = 2 M - 0.5 (P0 + P2)
    let p1_mid = Point::new(
        2.0 * mid_pt.x - 0.5 * (p0.x + p2.x),
        2.0 * mid_pt.y - 0.5 * (p0.y + p2.y),
    );
    let p1_ls = fit_quad_control_point_least_squares(pts, cum, p0, p2);
    p1_mid.lerp(p1_ls, 0.50)
}

/// 鋭角コーナーではない滑らかなアンカー点において、前後の2次ベジェ曲線の制御点 `p1` が
/// アンカー点を挟んで共通の接線方向（G1連続）に乗るよう調整し、アンカー箇所の微小な角度折れを解消する。
fn enforce_g1_tangent_continuity_on_segments(
    segments: &mut [QuadBez],
    dense: &[Point],
    anchors: &[usize],
    is_sharp_corner: &[bool],
    is_straight: &[bool],
    r_scale: f64,
) {
    let k = segments.len();
    let m = dense.len();
    if k < 2 || m < 6 {
        return;
    }

    let w_tan = 3usize.min(m / 8).max(1);
    let mut anchor_tangents: Vec<Option<KVec2>> = vec![None; k];

    for j in 0..k {
        let a_idx = anchors[j];
        let near_sharp = (0..=2).any(|d| {
            is_sharp_corner[(a_idx + d) % m] || is_sharp_corner[(a_idx + m - d) % m]
        });
        if near_sharp {
            continue;
        }

        let prev = (j + k - 1) % k;
        if is_straight[prev] || is_straight[j] {
            continue;
        }

        let dir_before = normalize_vec(dense[a_idx] - dense[(a_idx + m - w_tan) % m]);
        let dir_after = normalize_vec(dense[(a_idx + w_tan) % m] - dense[a_idx]);
        // 意図的な角（約30度以上の折れ）では接線連続化を行わず角をそのまま残す
        if dir_before.dot(dir_after) < 0.86 {
            continue;
        }
        let d_ring = normalize_vec(dir_before + dir_after);

        let anchor_pt = segments[j].p0;
        let v_in = anchor_pt - segments[prev].p1;
        let v_out = segments[j].p1 - anchor_pt;
        if v_in.hypot() < 1e-5 || v_out.hypot() < 1e-5 {
            continue;
        }

        let d_in = normalize_vec(v_in);
        let d_out = normalize_vec(v_out);
        if d_in.dot(d_out) < 0.82 {
            continue;
        }

        anchor_tangents[j] = Some(normalize_vec(d_in + d_out + d_ring));
    }

    let max_radial_shift = 0.85 * r_scale;

    for j in 0..k {
        if is_straight[j] {
            continue;
        }
        let next = (j + 1) % k;
        let p0 = segments[j].p0;
        let p1_init = segments[j].p1;
        let p2 = segments[j].p2;
        let chord = p2 - p0;
        let chord_len = chord.hypot();
        if chord_len < 1e-4 {
            continue;
        }
        let chord_normal = left_normal(chord / chord_len);

        let t0_opt = anchor_tangents[j].or_else(|| {
            let v0 = p1_init - p0;
            (v0.hypot() > 1e-5).then(|| normalize_vec(v0))
        });
        let t2_opt = anchor_tangents[next].or_else(|| {
            let v2 = p2 - p1_init;
            (v2.hypot() > 1e-5).then(|| normalize_vec(v2))
        });
        let has_any_smooth = anchor_tangents[j].is_some() || anchor_tangents[next].is_some();
        let candidate_p1 = if let (true, Some(t0), Some(t2)) = (has_any_smooth, t0_opt, t2_opt)
        {
            let cross = t0.x * t2.y - t0.y * t2.x;
            if cross.abs() > 0.04 {
                let s0 = (chord.x * t2.y - chord.y * t2.x) / cross;
                let s2 = (t0.x * chord.y - t0.y * chord.x) / cross;
                if s0 > 0.08 * chord_len
                    && s2 > 0.08 * chord_len
                    && s0 < 1.8 * chord_len
                    && s2 < 1.8 * chord_len
                {
                    Some(p0 + t0 * s0)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(p1_tan) = candidate_p1 {
            let radial_diff = (0.5 * (p1_tan - p1_init).dot(chord_normal)).abs();
            let total_diff = (p1_tan - p1_init).hypot();
            let local_max_radial = max_radial_shift.min(0.024 * chord_len);
            if radial_diff <= local_max_radial && total_diff <= 0.36 * chord_len {
                segments[j].p1 = p1_tan;
            }
        }
    }
}

/// 点列 `pts` の始点 `p0` と終点 `p2` を固定したときの、最小二乗誤差を最小化する2次ベジェ制御点 `p1` を閉形式で求める
fn fit_quad_control_point_least_squares(
    pts: &[Point],
    cum: &[f64],
    p0: Point,
    p2: Point,
) -> Point {
    let n = pts.len();
    let total_len = cum[n - 1];
    let mut num = KVec2::ZERO;
    let mut denom = 0.0;
    for i in 1..(n - 1) {
        let t = (cum[i] / total_len).clamp(0.05, 0.95);
        let u = 1.0 - t;
        let b0 = u * u;
        let b1 = 2.0 * u * t;
        let b2 = t * t;

        let residual = KVec2::new(
            pts[i].x - b0 * p0.x - b2 * p2.x,
            pts[i].y - b0 * p0.y - b2 * p2.y,
        );
        num += residual * b1;
        denom += b1 * b1;
    }

    if denom > 1e-9 {
        (num / denom).to_point()
    } else {
        let mid_pt = pts[n / 2];
        Point::new(
            2.0 * mid_pt.x - 0.5 * (p0.x + p2.x),
            2.0 * mid_pt.y - 0.5 * (p0.y + p2.y),
        )
    }
}

/// 密な輪郭点列 `dense` 上で、ロックされていない中間アンカーを
/// 「前後の区間を1本の `QuadBez` に統合したときの最大幾何誤差が最小のもの」から順に反復削除する。
fn reduce_outline_anchors_by_quad_fit(
    dense: &[Point],
    locked: &[bool],
    is_high_curv: &[bool],
    max_edge_tol: f64,
    max_cap_tol: f64,
    max_curve_turn_rad: f64,
) -> Vec<usize> {
    let m = dense.len();
    let step = 4usize.max(m / 48).min(8);

    let mut anchors: Vec<usize> = (0..m)
        .filter(|&i| {
            locked[i]
                || (i % step == 0
                    && !(1..=2).any(|d| locked[(i + d) % m] || locked[(i + m - d) % m]))
        })
        .collect();

    if anchors.len() < 4 {
        anchors = vec![0, m / 4, m / 2, (3 * m) / 4];
        anchors.sort_unstable();
        anchors.dedup();
    }

    const MAX_CORNER_TURN_RAD: f64 = 1.68;
    let mut span_pts = Vec::with_capacity(m);
    let mut cum = Vec::with_capacity(m);

    let mut eval_removal_score = |anchors: &[usize], pos: usize| -> f64 {
        let curr_idx = anchors[pos];
        if locked[curr_idx] {
            return f64::INFINITY;
        }

        let prev_idx = anchors[(pos + anchors.len() - 1) % anchors.len()];
        let next_idx = anchors[(pos + 1) % anchors.len()];
        fill_cyclic_span(dense, prev_idx, next_idx, &mut span_pts);

        if span_pts.len() > (m * 3) / 4 {
            return f64::INFINITY;
        }

        let (net_turn, macro_abs_turn) = span_turn_metrics(&span_pts);
        if macro_abs_turn > 0.35 && (macro_abs_turn - net_turn) > 0.24 {
            return f64::INFINITY;
        }

        let turn_limit = if is_high_curv[curr_idx] {
            MAX_CORNER_TURN_RAD
        } else {
            max_curve_turn_rad
        };
        if net_turn > turn_limit || macro_abs_turn > turn_limit + 0.25 {
            return f64::INFINITY;
        }

        let tol = if is_high_curv[curr_idx] {
            max_cap_tol
        } else {
            max_edge_tol
        };

        let p0 = span_pts[0];
        let p2 = span_pts[span_pts.len() - 1];
        let chord = p2 - p0;
        let chord_len = chord.hypot();
        if chord_len > 1e-4 && span_pts.len() >= 10 {
            let n_chord = left_normal(chord / chord_len);
            let mid = span_pts.len() / 2;
            let mean_first: f64 = span_pts[1..mid]
                .iter()
                .map(|&pt| (pt - p0).dot(n_chord))
                .sum::<f64>()
                / (mid - 1).max(1) as f64;
            let mean_second: f64 = span_pts[mid..(span_pts.len() - 1)]
                .iter()
                .map(|&pt| (pt - p0).dot(n_chord))
                .sum::<f64>()
                / (span_pts.len() - 1 - mid).max(1) as f64;
            if mean_first * mean_second < 0.0
                && mean_first.abs() > 0.72 * tol
                && mean_second.abs() > 0.72 * tol
            {
                return f64::INFINITY;
            }
        }

        fill_cumulative_arc_lengths(&span_pts, &mut cum);
        let p1 = fit_quad_control_point_with_arc_lengths(&span_pts, &cum, p0, p2);
        let err = max_quad_fit_error_with_arc_lengths(&span_pts, &cum, QuadBez::new(p0, p1, p2));

        if err <= tol {
            err + 0.18 * net_turn * net_turn
        } else {
            f64::INFINITY
        }
    };

    let mut scores: Vec<f64> = (0..anchors.len())
        .map(|pos| eval_removal_score(&anchors, pos))
        .collect();

    while anchors.len() > 3 {
        let mut best_remove_pos: Option<usize> = None;
        let mut best_score = f64::INFINITY;

        for (pos, &score) in scores.iter().enumerate() {
            if score < best_score {
                best_score = score;
                best_remove_pos = Some(pos);
            }
        }

        if let Some(pos) = best_remove_pos {
            anchors.remove(pos);
            scores.remove(pos);
            let len = anchors.len();
            if len > 3 {
                let prev_pos = (pos + len - 1) % len;
                let next_pos = pos % len;
                scores[prev_pos] = eval_removal_score(&anchors, prev_pos);
                scores[next_pos] = eval_removal_score(&anchors, next_pos);
            }
        } else {
            break;
        }
    }

    anchors
}

fn fill_cyclic_span(dense: &[Point], start_idx: usize, end_idx: usize, span: &mut Vec<Point>) {
    let m = dense.len();
    span.clear();
    let mut curr = start_idx;
    span.push(dense[curr]);
    while curr != end_idx && span.len() <= m {
        curr = (curr + 1) % m;
        span.push(dense[curr]);
    }
}

/// 点列 `span_pts` の (ネット回転角 `|signed_turn|`, 4分割マクロ絶対回転角 `macro_abs_turn`) をラジアンで返す
fn span_turn_metrics(span_pts: &[Point]) -> (f64, f64) {
    let n = span_pts.len();
    if n < 3 {
        return (0.0, 0.0);
    }
    let step = 2usize.min(n - 1).max(1);
    let mut prev_dir: Option<KVec2> = None;
    let mut signed_turn = 0.0;

    let mut idx = 0usize;
    while idx < n - 1 {
        let next_idx = (idx + step).min(n - 1);
        let d = span_pts[next_idx] - span_pts[idx];
        if d.hypot() > 1e-4 {
            let dir = normalize_vec(d);
            if let Some(pd) = prev_dir {
                signed_turn += signed_angle(pd, dir);
            }
            prev_dir = Some(dir);
        }
        idx = next_idx;
    }

    let chunks = 4usize.min(n - 1).max(1);
    let mut prev_macro_dir: Option<KVec2> = None;
    let mut macro_abs_turn = 0.0;
    for c in 0..chunks {
        let i0 = (c * (n - 1)) / chunks;
        let i1 = ((c + 1) * (n - 1)) / chunks;
        if i1 > i0 {
            let d = span_pts[i1] - span_pts[i0];
            if d.hypot() > 1e-4 {
                let dir = normalize_vec(d);
                if let Some(pd) = prev_macro_dir {
                    macro_abs_turn += signed_angle(pd, dir).abs();
                }
                prev_macro_dir = Some(dir);
            }
        }
    }

    (signed_turn.abs(), macro_abs_turn)
}

/// 点列 `span_pts` と 2次ベジェ曲線 `quad` の最大距離誤差を高速計算する
fn max_quad_fit_error_with_arc_lengths(span_pts: &[Point], cum: &[f64], quad: QuadBez) -> f64 {
    let n = span_pts.len();
    if n <= 2 {
        return 0.0;
    }

    let total_len = cum[n - 1];
    if total_len < 1e-6 {
        return 0.0;
    }

    let mut max_err = 0.0_f64;
    for i in 1..(n - 1) {
        let pt = span_pts[i];
        let t_init = (cum[i] / total_len).clamp(0.0, 1.0);
        let q_pt = quad.eval(t_init);
        let deriv = quad.deriv().eval(t_init).to_vec2();
        let d_len_sq = deriv.hypot2();
        let t_refined = if d_len_sq > 1e-6 {
            (t_init + (pt - q_pt).dot(deriv) / d_len_sq).clamp(0.0, 1.0)
        } else {
            t_init
        };
        let dist = (pt - quad.eval(t_refined)).hypot().min((pt - q_pt).hypot());
        max_err = max_err.max(dist);
    }
    max_err
}

fn max_deviation_from_chord(pts: &[Point]) -> f64 {
    if pts.len() <= 2 {
        return 0.0;
    }
    let a = pts[0];
    let b = pts[pts.len() - 1];
    pts[1..(pts.len() - 1)]
        .iter()
        .map(|&p| point_to_segment_dist(p, a, b))
        .fold(0.0_f64, f64::max)
}

fn rdp_recursive(
    all_pts: &[Point],
    start: usize,
    end: usize,
    epsilon: f64,
    keep: &mut [bool],
) {
    if end <= start + 1 {
        return;
    }
    let a = all_pts[start];
    let b = all_pts[end];

    let mut max_dist = -1.0;
    let mut split_idx = start;
    for (i, &pt) in all_pts.iter().enumerate().take(end).skip(start + 1) {
        let d = point_to_segment_dist(pt, a, b);
        if d > max_dist {
            max_dist = d;
            split_idx = i;
        }
    }

    if max_dist > epsilon {
        keep[split_idx] = true;
        rdp_recursive(all_pts, start, split_idx, epsilon, keep);
        rdp_recursive(all_pts, split_idx, end, epsilon, keep);
    }
}

// ============================================================================
// アルゴリズム 3: 平面ブーリアン演算（同色 Union / 異色・消しゴム Difference）
// & De Casteljau 2次ベジェ曲線ハイブリッド再構築エンジン
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BoolOp {
    Union,
    Difference,
    Intersection,
}

/// 新しいブラシストロークまたは変形確定シェイプをキャンバスへマージする（Flashのマージ描画モードの核心挙動）。
/// 同一の `group_path` を持つ生シェイプ同士のみが結合（`Union`）・切り抜き（`Difference`）を行い、
/// 異なるグループに属するシェイプは保護される。
pub(crate) fn merge_shapes_into_canvas(canvas: &mut Vec<FillShape>, incoming: Vec<FillShape>) {
    merge_shapes_into_canvas_inner(canvas, incoming, true);
}

/// 他の異色シェイプと重ならない塗りシェイプ（バケツ塗り・リカラー）をキャンバスへマージし、
/// 同じ `group_path` の隣接する同色シェイプがあればシームレスに結合（Union）する。
pub(crate) fn merge_disjoint_fill_into_canvas(canvas: &mut Vec<FillShape>, fill_shape: FillShape) {
    merge_shapes_into_canvas_inner(canvas, vec![fill_shape], false);
}

fn merge_shapes_into_canvas_inner(
    canvas: &mut Vec<FillShape>,
    incoming: Vec<FillShape>,
    subtract_diff_color: bool,
) {
    if incoming.is_empty() {
        return;
    }
    let target_color = incoming[0].color;
    let target_layer_id = incoming[0].layer_id;
    let target_group_path = incoming[0].group_path.clone();
    let test_bbox = union_bounding_boxes(&incoming).inflate(1.5, 1.5);

    let insert_limit = if target_group_path.is_empty() {
        canvas
            .iter()
            .rposition(|s| s.layer_id == target_layer_id)
            .map(|pos| pos + 1)
            .unwrap_or(canvas.len())
    } else {
        canvas
            .iter()
            .rposition(|s| {
                s.layer_id == target_layer_id && s.is_in_editing_scope(&target_group_path)
            })
            .or_else(|| canvas.iter().rposition(|s| s.layer_id == target_layer_id))
            .map(|pos| pos + 1)
            .unwrap_or(canvas.len())
    };
    let trailing_outside = canvas.split_off(insert_limit);

    let mut updated_canvas =
        Vec::with_capacity(canvas.len() + incoming.len() + trailing_outside.len());
    let mut overlapping_same_color = Vec::new();

    for existing in canvas.drain(..) {
        if existing.layer_id != target_layer_id || existing.group_path != target_group_path {
            updated_canvas.push(existing);
            continue;
        }
        let intersects_bbox = rects_overlap(existing.bounding_box(), test_bbox);
        if existing.color == target_color {
            if intersects_bbox {
                overlapping_same_color.push(existing);
            } else {
                updated_canvas.push(existing);
            }
        } else if subtract_diff_color && intersects_bbox {
            let color = existing.color;
            updated_canvas.extend(
                boolean_operation(&[existing], &incoming, BoolOp::Difference, color)
                    .into_iter()
                    .map(|s| s.with_scope(target_layer_id, &target_group_path)),
            );
        } else {
            updated_canvas.push(existing);
        }
    }

    if overlapping_same_color.is_empty() {
        updated_canvas.extend(incoming);
    } else {
        updated_canvas.extend(
            boolean_operation(
                &overlapping_same_color,
                &incoming,
                BoolOp::Union,
                target_color,
            )
            .into_iter()
            .map(|s| s.with_scope(target_layer_id, &target_group_path)),
        );
    }

    updated_canvas.extend(trailing_outside);
    *canvas = updated_canvas;
}

/// 消しゴムの輪郭で、同じ編集スコープ（`group_path`）直下の生シェイプに対してのみ Boolean Difference を適用する
pub(crate) fn erase_from_canvas(canvas: &mut Vec<FillShape>, eraser_shapes: &[FillShape]) {
    if eraser_shapes.is_empty() {
        return;
    }
    let target_group_path = &eraser_shapes[0].group_path;
    let test_bbox = union_bounding_boxes(eraser_shapes).inflate(1.5, 1.5);

    let mut updated_canvas = Vec::with_capacity(canvas.len());
    for existing in canvas.drain(..) {
        if existing.group_path == *target_group_path
            && rects_overlap(existing.bounding_box(), test_bbox)
        {
            let color = existing.color;
            let layer_id = existing.layer_id;
            updated_canvas.extend(
                boolean_operation(&[existing], eraser_shapes, BoolOp::Difference, color)
                    .into_iter()
                    .map(|s| s.with_scope(layer_id, target_group_path)),
            );
        } else {
            updated_canvas.push(existing);
        }
    }
    *canvas = updated_canvas;
}

pub(crate) fn union_bounding_boxes(shapes: &[FillShape]) -> KRect {
    let mut bbox = shapes[0].bounding_box();
    for s in &shapes[1..] {
        bbox = bbox.union(s.bounding_box());
    }
    bbox
}

/// `i_overlay` クレート（`FloatOverlay`）を用いたハイブリッド2次ベジェブーリアン演算＆再フィッティング
pub(crate) fn boolean_operation(
    subj: &[FillShape],
    clip: &[FillShape],
    op: BoolOp,
    out_color: Color32,
) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let subj_shapes: Vec<Vec<Vec<[f64; 2]>>> = subj
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();
    let clip_shapes: Vec<Vec<Vec<[f64; 2]>>> = clip
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();

    if (subj_shapes.is_empty() && op == BoolOp::Difference)
        || (op == BoolOp::Intersection && (subj_shapes.is_empty() || clip_shapes.is_empty()))
        || (subj_shapes.is_empty() && clip_shapes.is_empty())
    {
        return Vec::new();
    }

    let overlay_rule = match op {
        BoolOp::Union => OverlayRule::Union,
        BoolOp::Difference => OverlayRule::Difference,
        BoolOp::Intersection => OverlayRule::Intersect,
    };

    let overlay_result = run_float_overlay(
        &subj_shapes,
        &clip_shapes,
        overlay_rule,
        OverlayFillRule::NonZero,
        0.04,
    );

    overlay_result
        .iter()
        .filter_map(|poly_shape| {
            reconstruct_fill_shape_from_rings(
                poly_shape,
                &quad_table,
                out_color,
                op == BoolOp::Union,
            )
        })
        .collect()
}

fn run_float_overlay(
    subj: &[Vec<Vec<[f64; 2]>>],
    clip: &[Vec<Vec<[f64; 2]>>],
    overlay_rule: OverlayRule,
    fill_rule: OverlayFillRule,
    min_area: f64,
) -> Vec<Vec<Vec<[f64; 2]>>> {
    let filter = ContourFilter {
        min_area,
        simplify: false,
    };
    FloatOverlay::with_subj_and_clip(subj, clip).overlay_with_filter_and_solver(
        overlay_rule,
        fill_rule,
        filter,
        Solver::AUTO,
    )
}

/// `i_overlay` の出力ポリゴン群（外周リング `poly_shape[0]` ＋ 穴リング群 `poly_shape[1..]`）から
/// 2次ベジェ曲線を復元・再フィットして `FillShape` を構築する共通ヘルパー。
fn reconstruct_fill_shape_from_rings(
    poly_shape: &[Vec<[f64; 2]>],
    quad_table: &[QuadRecord],
    color: Color32,
    filter_sliver_holes: bool,
) -> Option<FillShape> {
    if poly_shape.is_empty() {
        return None;
    }
    let exterior = reconstruct_or_refit_quad_contour(&poly_shape[0], quad_table, true)?;
    let holes = poly_shape[1..]
        .iter()
        .filter(|hole_ring| {
            if !filter_sliver_holes {
                return true;
            }
            let (a, p) = ring_area_and_perimeter_f64(hole_ring);
            a >= 1.5 && (a / p.max(1e-3)) >= 0.15
        })
        .filter_map(|hole_ring| reconstruct_or_refit_quad_contour(hole_ring, quad_table, false))
        .collect();
    Some(FillShape::new(color, exterior, holes))
}

/// キャンバス上のシェイプ群によって完全に囲まれた空白領域（単一シェイプの穴、または複数シェイプで囲まれた閉領域）のうち、
/// 指定座標 `pt` を含むものを検出し、周囲の2次ベジェ曲線をそのまま引き継いだ `FillShape` を生成する。
pub(crate) fn find_enclosed_region_at(
    canvas: &[FillShape],
    pt: Point,
    fill_color: Color32,
) -> Option<FillShape> {
    if canvas.is_empty() {
        return None;
    }
    let all_bbox = union_bounding_boxes(canvas);
    if !all_bbox.contains(pt) {
        return None;
    }

    let bounds = all_bbox.inflate(32.0, 32.0);
    let mut bounds_ring = vec![
        [bounds.x0, bounds.y0],
        [bounds.x1, bounds.y0],
        [bounds.x1, bounds.y1],
        [bounds.x0, bounds.y1],
    ];
    if signed_area_f64(&bounds_ring) > 0.0 {
        bounds_ring.reverse();
    }

    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let clip_shapes: Vec<Vec<Vec<[f64; 2]>>> = canvas
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();
    if clip_shapes.is_empty() {
        return None;
    }

    let overlay_result = run_float_overlay(
        &[vec![bounds_ring]],
        &clip_shapes,
        OverlayRule::Difference,
        OverlayFillRule::NonZero,
        0.5,
    );

    for poly_shape in overlay_result {
        if poly_shape.is_empty() {
            continue;
        }
        let ext_ring = &poly_shape[0];
        let touches_bounds = ext_ring.iter().any(|&[x, y]| {
            (x - bounds.x0).abs() < 4.0
                || (x - bounds.x1).abs() < 4.0
                || (y - bounds.y0).abs() < 4.0
                || (y - bounds.y1).abs() < 4.0
        });
        if touches_bounds {
            continue;
        }

        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for &[x, y] in ext_ring {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        if pt.x < min_x - 1.0 || pt.x > max_x + 1.0 || pt.y < min_y - 1.0 || pt.y > max_y + 1.0 {
            continue;
        }

        let Some(candidate) =
            reconstruct_fill_shape_from_rings(&poly_shape, &quad_table, fill_color, false)
        else {
            continue;
        };
        if candidate.contains_point(pt) {
            return Some(candidate);
        }
    }

    None
}

#[derive(Clone, Copy, Debug)]
struct QuadRecord {
    quad: QuadBez,
    bbox: KRect,
}

#[derive(Clone, Copy, Debug)]
struct TaggedEdge {
    from: Point,
    to: Point,
    quad_id: Option<usize>,
}

fn flatten_shape_for_overlay(
    shape: &FillShape,
    quad_table: &mut Vec<QuadRecord>,
) -> Vec<Vec<[f64; 2]>> {
    let Some(ext_ring) = flatten_contour_for_overlay(&shape.exterior, true, quad_table) else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) = flatten_contour_for_overlay(hole, false, quad_table) {
            rings.push(hole_ring);
        }
    }
    rings
}

/// 輪郭変形（エッジつまみ変形・頂点移動）後のシェイプの自己交差・反対側輪郭の突き抜けを解消し、
/// 正のワインディング領域（`OverlayFillRule::Positive`）のみを残して1つまたは複数の `FillShape` に分割・再構築する。
pub(crate) fn resolve_deformed_shape(shape: &FillShape) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let Some(ext_ring) = flatten_contour_preserving_direction(&shape.exterior, &mut quad_table)
    else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) = flatten_contour_preserving_direction(hole, &mut quad_table) {
            rings.push(hole_ring);
        }
    }

    let overlay_result = run_float_overlay(
        &[rings],
        &[],
        OverlayRule::Subject,
        OverlayFillRule::Positive,
        0.5,
    );

    overlay_result
        .iter()
        .filter_map(|poly_shape| {
            reconstruct_fill_shape_from_rings(poly_shape, &quad_table, shape.color, false)
                .map(|s| s.with_scope(shape.layer_id, &shape.group_path))
        })
        .collect()
}

fn quad_subdivision_steps(seg: QuadBez) -> usize {
    let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot();
    if mid_dev <= 0.04 {
        2
    } else {
        let arc = seg.arclen(0.5);
        ((arc / 1.2).ceil() as usize).clamp(4, 256)
    }
}

fn sample_contour_ring_for_overlay(
    contour: &Contour,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    if contour.segments.is_empty() {
        return None;
    }

    let mut raw_pts = Vec::new();
    for &seg in &contour.segments {
        let bbox = seg.bounding_box().inflate(0.6, 0.6);
        quad_table.push(QuadRecord { quad: seg, bbox });

        let steps = quad_subdivision_steps(seg);
        for k in 0..steps {
            raw_pts.push(seg.eval(k as f64 / steps as f64));
        }
    }

    let deduped = dedup_closed_ring(raw_pts, 1e-4);
    if deduped.len() < 3 {
        return None;
    }

    Some(deduped.into_iter().map(|p| [p.x, p.y]).collect())
}

/// 変形前からのパラメータ進行方向を維持したまま `Contour` を `i_overlay` 用頂点リングに展開する
/// （反対側の輪郭を大きく突き抜けて見かけの符号付き面積が反転しても、元の正のワインディング方向を失わないようにする）
fn flatten_contour_preserving_direction(
    contour: &Contour,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table)?;
    ring.reverse();
    Some(ring)
}

/// 2次ベジェ輪郭 `Contour` を `i_overlay` 用の頂点リング `Vec<[f64; 2]>` に細分化する。
/// `i_overlay` の規約に合わせ、外周は `signed_area < 0`、穴は `signed_area > 0` に揃えて出力する。
fn flatten_contour_for_overlay(
    contour: &Contour,
    is_outer: bool,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table)?;
    let area = signed_area_f64(&ring);
    if area.abs() < 0.25 {
        return None;
    }

    if (is_outer && area > 0.0) || (!is_outer && area < 0.0) {
        ring.reverse();
    }

    Some(ring)
}

/// `i_overlay` が出力したポリゴン頂点リング `&[[f64; 2]]` から 2次ベジェ輪郭 `Contour` を再構築・再フィッティングする
fn reconstruct_or_refit_quad_contour(
    ring_f64: &[[f64; 2]],
    quad_table: &[QuadRecord],
    is_outer: bool,
) -> Option<Contour> {
    let pts = dedup_closed_ring(
        ring_f64.iter().map(|&[x, y]| Point::new(x, y)),
        1e-4,
    );
    let n = pts.len();
    if n < 3 {
        return None;
    }

    let tagged_edges: Vec<TaggedEdge> = (0..n)
        .map(|i| {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            TaggedEdge {
                from: a,
                to: b,
                quad_id: match_edge_to_quad(a, b, quad_table),
            }
        })
        .collect();

    if tagged_edges.iter().all(|e| e.quad_id.is_none()) {
        let r_est = estimate_brush_radius_from_ring(&pts);
        return smooth_ring_to_quad_contour(&pts, r_est, is_outer);
    }

    let shift = (0..n)
        .find(|&i| tagged_edges[i].quad_id != tagged_edges[(i + n - 1) % n].quad_id)
        .unwrap_or(0);

    let rotated: Vec<TaggedEdge> = (0..n).map(|i| tagged_edges[(i + shift) % n]).collect();
    let mut segments: Vec<QuadBez> = Vec::new();
    let mut i = 0;

    while i < n {
        if let Some(qid) = rotated[i].quad_id {
            let orig_quad = quad_table[qid].quad;
            let run_start_pt = rotated[i].from;
            let mut run_end_pt = rotated[i].to;
            let mut run_len = 1usize;
            let mut prev_t = orig_quad.nearest(run_end_pt, 0.01).t;

            while i + run_len < n && rotated[i + run_len].quad_id == Some(qid) {
                let next_pt = rotated[i + run_len].to;
                let next_t = orig_quad.nearest(next_pt, 0.01).t;
                if (next_t - prev_t).abs() > 0.65 {
                    break;
                }
                run_end_pt = next_pt;
                prev_t = next_t;
                run_len += 1;
            }

            if shift == 0 && i == 0 && run_len == n {
                let mid_pt = rotated[n / 2].from;
                segments.push(extract_sub_quad(orig_quad, run_start_pt, mid_pt));
                segments.push(extract_sub_quad(orig_quad, mid_pt, run_start_pt));
                break;
            }

            segments.push(extract_sub_quad(orig_quad, run_start_pt, run_end_pt));
            i += run_len;
        } else {
            let mut chain_pts = vec![rotated[i].from, rotated[i].to];
            let mut run_len = 1usize;
            while i + run_len < n && rotated[i + run_len].quad_id.is_none() {
                chain_pts.push(rotated[i + run_len].to);
                run_len += 1;
            }
            segments.extend(refit_open_polyline_to_quads(&chain_pts, 0.35));
            i += run_len;
        }
    }

    if segments.len() == 1 {
        let single = segments[0];
        segments = vec![single.subsegment(0.0..0.5), single.subsegment(0.5..1.0)];
    }

    if segments.len() < 2 {
        let r_est = estimate_brush_radius_from_ring(&pts);
        return smooth_ring_to_quad_contour(&pts, r_est, is_outer);
    }

    Contour::from_segments_oriented(segments, 0.02, 0.04, is_outer)
}

/// ポリゴン辺 `a -> b` が元のどの `QuadBez` セグメント上に乗っているかを高速判定する
fn match_edge_to_quad(a: Point, b: Point, quad_table: &[QuadRecord]) -> Option<usize> {
    let chord = b - a;
    let chord_len = chord.hypot();
    if chord_len < 1e-5 {
        return None;
    }
    let chord_dir = chord / chord_len;
    let mid = a.lerp(b, 0.5);

    const DIST_TOL_SQ: f64 = 0.45 * 0.45;
    let mut best_id = None;
    let mut best_score = f64::INFINITY;

    for (qid, rec) in quad_table.iter().enumerate() {
        if !rec.bbox.contains(mid) {
            continue;
        }
        let n_mid = rec.quad.nearest(mid, 1e-3);
        if n_mid.distance_sq > DIST_TOL_SQ {
            continue;
        }
        let n_a = rec.quad.nearest(a, 1e-3);
        if n_a.distance_sq > DIST_TOL_SQ {
            continue;
        }
        let n_b = rec.quad.nearest(b, 1e-3);
        if n_b.distance_sq > DIST_TOL_SQ {
            continue;
        }

        let deriv = rec.quad.deriv().eval(n_mid.t).to_vec2();
        let deriv_len = deriv.hypot();
        if deriv_len > 1e-6 && chord_dir.dot(deriv / deriv_len).abs() < 0.90 {
            continue;
        }

        let score = n_mid.distance_sq + 0.5 * (n_a.distance_sq + n_b.distance_sq);
        if score < best_score {
            best_score = score;
            best_id = Some(qid);
        }
    }

    best_id
}

/// 開いた頂点列 `pts` に対してコーナー保持 RDP 間引きと形状保持2次ベジェ再フィッティングを行う
fn refit_open_polyline_to_quads(pts: &[Point], epsilon: f64) -> Vec<QuadBez> {
    let n = pts.len();
    if n < 2 {
        return Vec::new();
    }
    if n == 2 {
        return vec![QuadBez::new(pts[0], pts[0].lerp(pts[1], 0.5), pts[1])];
    }

    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;

    for i in 1..(n - 1) {
        let d_in = normalize_vec(pts[i] - pts[i - 1]);
        let d_out = normalize_vec(pts[i + 1] - pts[i]);
        if d_in.dot(d_out) < 0.57 {
            keep[i] = true;
        }
    }

    let mut seg_start = 0;
    for i in 1..n {
        if keep[i] {
            rdp_recursive(pts, seg_start, i, epsilon, &mut keep);
            seg_start = i;
        }
    }

    let anchors: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
    let mut cum = Vec::new();
    anchors
        .windows(2)
        .map(|w| {
            let span = &pts[w[0]..=w[1]];
            let p0 = pts[w[0]];
            let p2 = pts[w[1]];
            fill_cumulative_arc_lengths(span, &mut cum);
            let p1 = fit_quad_control_point_with_arc_lengths(span, &cum, p0, p2);
            QuadBez::new(p0, p1, p2)
        })
        .collect()
}

/// 元の `QuadBez` 上の2点 `p_start`, `p_end` に対応する部分曲線を De Casteljau 法で切り出す
fn extract_sub_quad(orig: QuadBez, p_start: Point, p_end: Point) -> QuadBez {
    let t0 = orig.nearest(p_start, 1e-3).t.clamp(0.0, 1.0);
    let t1 = orig.nearest(p_end, 1e-3).t.clamp(0.0, 1.0);

    if (t1 - t0).abs() < 1e-4 {
        return QuadBez::new(p_start, p_start.lerp(p_end, 0.5), p_end);
    }

    let mut sub = orig.subsegment(t0..t1);
    sub.p0 = p_start;
    sub.p2 = p_end;
    sub
}

// ============================================================================
// 幾何ユーティリティ関数
// ============================================================================

#[inline]
fn ring_f64_to_points(ring: &[[f64; 2]]) -> Vec<Point> {
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
fn signed_angle(a: KVec2, b: KVec2) -> f64 {
    (a.x * b.y - a.y * b.x).atan2((a.x * b.x + a.y * b.y).clamp(-1.0, 1.0))
}

#[inline]
pub(crate) fn to_pos2(p: Point) -> Pos2 {
    Pos2::new(p.x as f32, p.y as f32)
}

fn fill_cumulative_arc_lengths(pts: &[Point], cum: &mut Vec<f64>) {
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

fn dedup_closed_ring(pts: impl IntoIterator<Item = Point>, min_dist: f64) -> Vec<Point> {
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

fn signed_area_f64(pts: &[[f64; 2]]) -> f64 {
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

fn ring_area_and_perimeter_f64(ring: &[[f64; 2]]) -> (f64, f64) {
    let area = signed_area_f64(ring).abs();
    let mut perim = 0.0;
    for i in 0..ring.len() {
        let next = (i + 1) % ring.len();
        perim += (ring[next][0] - ring[i][0]).hypot(ring[next][1] - ring[i][1]);
    }
    (area, perim)
}

fn polygon_signed_area(pts: &[Point]) -> f64 {
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

fn point_to_segment_dist(pt: Point, a: Point, b: Point) -> f64 {
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

fn tessellate_contours_to_mesh(
    tessellator: &mut FillTessellator,
    exterior: &Contour,
    holes: &[Contour],
    color: Color32,
) -> Mesh {
    let mut builder = LyonPath::builder();
    for contour in std::iter::once(exterior).chain(holes.iter()) {
        if let Some(first) = contour.segments.first() {
            let p0 = first.p0;
            builder.begin(lyon_point(p0.x as f32, p0.y as f32));
            for seg in &contour.segments {
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
    tessellate_lyon_path(tessellator, &path, color, FillRule::EvenOdd)
}

/// テッセレーション済みの三角形メッシュから Flash 風の網点（Polka-Dot）選択オーバーレイ用 Mesh を1回だけ構築してキャッシュする。
/// 従来の `BezPath::winding()` 毎フレーム全グリッド走査を廃止し、各三角形のBBox内のみで高速外積判定を行う。
/// ズーム倍率 `zoom` に応じてワールド空間でのドット間隔・半径を逆スケーリングし、画面上で常に一定の網点サイズを保つ。
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

pub(crate) fn tessellate_raw_polygon(
    tessellator: &mut FillTessellator,
    pts: &[Point],
    color: Color32,
) -> Mesh {
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
    tessellate_lyon_path(tessellator, &path, color, FillRule::NonZero)
}

fn tessellate_lyon_path(
    tessellator: &mut FillTessellator,
    path: &LyonPath,
    color: Color32,
    fill_rule: FillRule,
) -> Mesh {
    let mut buffers: VertexBuffers<LyonPoint, u32> = VertexBuffers::new();
    // 高倍率ズーム（最大 1600%）でも曲線がカクつかないよう許容誤差を 0.04 に設定
    let options = FillOptions::tolerance(0.04).with_fill_rule(fill_rule);

    if tessellator
        .tessellate_path(
            path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| vertex.position()),
        )
        .is_err()
    {
        return Mesh::default();
    }

    Mesh {
        vertices: buffers
            .vertices
            .into_iter()
            .map(|p| Vertex {
                pos: Pos2::new(p.x, p.y),
                uv: egui::epaint::WHITE_UV,
                color,
            })
            .collect(),
        indices: buffers.indices,
        ..Default::default()
    }
}

/// ワールド座標で構築された `Mesh` を現在のビューポート（`zoom`, `pan`）に合わせてスクリーン座標へ変換する。
/// `dim_backdrop == true` の場合はグループ編集外の背景シェイプとして薄くフェードさせる。
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
