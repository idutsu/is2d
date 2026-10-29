use eframe::egui::Color32;
use i_overlay::{core::fill_rule::FillRule as OverlayFillRule, float::simplify::SimplifyShape};
use kurbo::{
    Affine, ParamCurve, ParamCurveDeriv, Point, QuadBez, Vec2 as KVec2,
};

use crate::vector_engine::{
    dedup_closed_ring, fill_cumulative_arc_lengths, left_normal, normalize_vec,
    point_to_segment_dist, polygon_signed_area, ring_area_and_perimeter_f64, ring_f64_to_points,
    signed_angle, Contour, FillShape, REFERENCE_BRUSH_RADIUS,
};

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
    if points.is_empty() || !radius.is_finite() || !zoom.is_finite() {
        return Vec::new();
    }

    let z = zoom.clamp(
        crate::vector_engine::MIN_VIEW_ZOOM as f64,
        crate::vector_engine::MAX_VIEW_ZOOM as f64,
    );
    let r = radius.clamp(0.05, 2000.0);
    let screen_r = r * z;
    let min_step = (0.15 * screen_r).clamp(0.2, 0.8) / z;

    // スクリーン空間のブラシ半径に比例した最小ステップ閾値で近接重複サンプルおよび非有限座標を除去
    let mut clean: Vec<Point> = Vec::with_capacity(points.len());
    for &pt in points {
        if !pt.x.is_finite() || !pt.y.is_finite() {
            continue;
        }
        if clean.last().is_none_or(|&last| (pt - last).hypot() >= min_step) {
            clean.push(pt);
        }
    }
    if clean.is_empty() {
        return Vec::new();
    }

    // 単一クリック（スタンプ）の場合は円形輪郭を生成
    if clean.len() <= 1 {
        let c = clean[0];
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
            let arc_steps =
                ((abs_ang / (std::f64::consts::PI / 12.0)).ceil() as usize).clamp(1, 16);
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
    if raw_outline.len() < 3
        || !zoom.is_finite()
        || raw_outline.iter().any(|p| !p.x.is_finite() || !p.y.is_finite())
    {
        return Vec::new();
    }

    let z = zoom.clamp(
        crate::vector_engine::MIN_VIEW_ZOOM as f64,
        crate::vector_engine::MAX_VIEW_ZOOM as f64,
    );
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

pub(crate) fn estimate_brush_radius_from_ring(ring: &[Point]) -> f64 {
    let f64_ring: Vec<[f64; 2]> = ring.iter().map(|p| [p.x, p.y]).collect();
    estimate_brush_radius_from_shape_rings(&[f64_ring])
}

/// スクリーン空間の閉じたポリゴン頂点リング `ring` の全体の図形・線幅・角の丸みを損なわずに、
/// ブラシ半径 `r_est` とリング自身の周長半径に応じたスケール不変パラメータでピクセルジャギーだけを整え、
/// 単一の `QuadBez` で近似できる中間アンカーを統合して最小アンカー数の `Contour` を構築する。
pub(crate) fn smooth_ring_to_quad_contour(
    ring: &[Point],
    r_est: f64,
    expect_positive_area: bool,
) -> Option<Contour> {
    if ring.len() < 3 {
        return None;
    }

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

struct RingTurnMetrics {
    t_1: Vec<f64>,
    t_short: Vec<f64>,
    t_local: Vec<f64>,
    t_macro: Vec<f64>,
    w_short: usize,
    w_local: usize,
    w_macro: usize,
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

    let mut turns = RingTurnMetrics {
        t_1: vec![0.0; m],
        t_short: vec![0.0; m],
        t_local: vec![0.0; m],
        t_macro: vec![0.0; m],
        w_short,
        w_local,
        w_macro,
    };

    for i in 0..m {
        turns.t_1[i] = signed_turn_at(dense, i, 1);
        turns.t_short[i] = signed_turn_at(dense, i, w_short);
        turns.t_local[i] = signed_turn_at(dense, i, w_local);
        turns.t_macro[i] = signed_turn_at(dense, i, w_macro);
    }

    // (A) 鋭角コーナー・自己交差分岐点（外周の凹V字ノッチや、穴の内角など）の検出
    let mut is_sharp_corner = detect_sharp_corners(&turns, winding_sign, is_exterior);

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
        for (i, &sharp) in is_sharp_corner.iter().enumerate() {
            if !sharp {
                turns.t_local[i] = clamped_turn(i, w_local);
                turns.t_macro[i] = clamped_turn(i, w_macro);
            }
        }
    }

    // (B) 滑らかな丸円弧（Round Cap / Round Corner）の候補検出
    let mut is_round_arc = detect_round_arcs(&turns, &is_sharp_corner);

    let mut is_high_curv: Vec<bool> = (0..m)
        .map(|i| is_sharp_corner[i] || is_round_arc[i])
        .collect();

    if let Some(rot_idx) = find_longest_false_run_midpoint(&is_high_curv) {
        dense.rotate_left(rot_idx);
        turns.t_local.rotate_left(rot_idx);
        turns.t_macro.rotate_left(rot_idx);
        is_sharp_corner.rotate_left(rot_idx);
        is_round_arc.rotate_left(rot_idx);
        is_high_curv.rotate_left(rot_idx);
    }

    filter_protected_round_features(
        dense,
        &turns,
        &is_sharp_corner,
        &is_round_arc,
        &mut is_high_curv,
        (r_scale, actual_ds),
    );

    (is_sharp_corner, is_high_curv)
}

// ============================================================================
// 特徴点（シャープ角・丸端キャップ・高曲率コーナー）検出の幾何しきい値定数
// ============================================================================

/// 丸端キャップ隣接スキャンで「凸円弧区間」とみなす最小短距離回転角（rad）
const CAP_SCAN_MIN_CONVEX_TURN: f64 = 0.08;
/// 丸端キャップ終端で反対側の凹ノッチに到達したと判定する短距離回転角しきい値（rad）
const CAP_SCAN_OPPOSITE_NOTCH_TURN: f64 = -0.30;
/// 丸端キャップ探索を打ち切る凹方向逆転のしきい値（rad）
const CAP_SCAN_ABORT_CONCAVE_TURN: f64 = -0.10;
/// 単独の半円丸端キャップとみなす最小累積回転角（rad, 約126°）
const CAP_SCAN_MIN_NET_TURN: f64 = 2.20;
/// 両側を凹ノッチに挟まれた丸端キャップとみなす最小累積回転角（rad, 約63°）
const CAP_SCAN_NOTCH_BOUNDED_MIN_TURN: f64 = 1.10;

/// 手ブレによる周期的な蛇行（wobble）と判定する前後局所回転角の逆符号積しきい値
const WOBBLE_OPPOSITE_TURN_PRODUCT: f64 = -0.10;
/// 丸端キャップ付け根の鋭い凹ノッチ（『あ』の交差部など）と判定する短距離回転角の最小絶対値（rad）
const CAP_NOTCH_MIN_SHORT_TURN: f64 = 0.44;
/// 丸端キャップ付け根の凹ノッチにおける隣接2頂点ピーク回転角の最小値（rad）
const CAP_NOTCH_MIN_PEAK_2V_TURN: f64 = 0.38;
/// 丸端キャップ付け根の凹ノッチにおける短距離回転角に対する2頂点集中度比率
const CAP_NOTCH_MIN_PEAK_CONCENTRATION: f64 = 0.74;
/// 丸端キャップ付け根の凹ノッチで前後の1ステップ回転が逆向きに暴れていないことを確認する許容比
const CAP_NOTCH_ADJACENT_SIGN_TOLERANCE: f64 = -0.35;

/// 直角〜鋭角コーナーと判定する短距離回転角の最大余弦（cos ≈ 63° 以上）
const RIGHT_CORNER_MAX_SHORT_COS: f64 = 0.45;
/// 直角〜鋭角コーナーと判定する局所回転角の最小絶対値（rad）
const RIGHT_CORNER_MIN_LOCAL_TURN: f64 = 1.00;
/// 直角〜鋭角コーナーと判定するマクロ回転角の最大余弦
const RIGHT_CORNER_MAX_MACRO_COS: f64 = 0.48;

/// 集中した折れ角（ジャンクション）と判定する短距離・局所回転角の最小絶対値（rad）
const JUNCTION_MIN_SHORT_AND_LOCAL_TURN: f64 = 0.62;
/// 集中した折れ角と判定するマクロ回転角の最小絶対値（rad）
const JUNCTION_MIN_MACRO_TURN: f64 = 0.55;
/// 集中した折れ角における局所回転角に対する短距離回転角の最小集中比率
const JUNCTION_MIN_SHORT_TO_LOCAL_RATIO: f64 = 0.80;

/// 丸弧（Round Arc）候補と判定する短距離・局所・マクロ回転角の最小絶対値（rad）
const ROUND_ARC_MIN_SHORT_TURN: f64 = 0.11;
const ROUND_ARC_MIN_LOCAL_TURN: f64 = 0.18;
const ROUND_ARC_MIN_MACRO_TURN: f64 = 0.48;

/// 保護すべき丸端キャップ区間と判定する最小ネット回転角（rad, 約106°）
const PROTECTED_CAP_MIN_NET_TURN: f64 = 1.85;
/// 局所的な丸角（緩やかなL字等）と判定する最小ネット回転角と両脇フランジの最大許容曲がり量
const LOCAL_ROUND_CORNER_MIN_TURN: f64 = 0.58;
const LOCAL_ROUND_CORNER_MAX_FLANK_TURN: f64 = 0.28;
const LOCAL_ROUND_CORNER_MAX_FLANK_RATIO: f64 = 0.42;
/// やや大きく曲がる局所丸角と判定する最小ネット回転角・局所ピーク・フランジ比率
const STRONG_ROUND_CORNER_MIN_TURN: f64 = 0.88;
const STRONG_ROUND_CORNER_MIN_PEAK_LOCAL: f64 = 0.52;
const STRONG_ROUND_CORNER_MAX_FLANK_RATIO: f64 = 0.52;

/// G1接線連続化を適用するアンカー前後の弦方向内積および制御点接線内積の最小値（cos）
const G1_MIN_RING_DIR_DOT: f64 = 0.86;
const G1_MIN_TANGENT_DOT: f64 = 0.82;
/// G1接線交点から制御点 `P1` を求める際の最小外積および弦長に対するパラメータ許容範囲
const G1_MIN_TANGENT_CROSS: f64 = 0.04;
const G1_MIN_INTERSECT_CHORD_RATIO: f64 = 0.08;
const G1_MAX_INTERSECT_CHORD_RATIO: f64 = 1.80;
/// G1接線補正による制御点移動の最大半径方向シフト比・弦長比
const G1_MAX_RADIAL_SHIFT_BRUSH_RATIO: f64 = 0.85;
const G1_MAX_RADIAL_SHIFT_CHORD_RATIO: f64 = 0.024;
const G1_MAX_TOTAL_SHIFT_CHORD_RATIO: f64 = 0.36;

fn detect_sharp_corners(
    turns: &RingTurnMetrics,
    winding_sign: f64,
    is_exterior: bool,
) -> Vec<bool> {
    let RingTurnMetrics {
        t_1,
        t_short,
        t_local,
        t_macro,
        w_short,
        w_local,
        w_macro,
    } = turns;
    let (&w_short, &w_local, &w_macro) = (w_short, w_local, w_macro);
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
            if ts_conv >= CAP_SCAN_MIN_CONVEX_TURN {
                in_arc = true;
                net += t_1[p] * winding_sign;
            } else if in_arc {
                let adj = if forward {
                    (p + 1) % m
                } else {
                    (p + m - 1) % m
                };
                if ts_conv <= CAP_SCAN_OPPOSITE_NOTCH_TURN
                    || t_short[adj] * winding_sign <= CAP_SCAN_OPPOSITE_NOTCH_TURN
                {
                    hit_other_notch = true;
                }
                break;
            } else if d > w_short && ts_conv < CAP_SCAN_ABORT_CONCAVE_TURN {
                break;
            }
        }
        net >= CAP_SCAN_MIN_NET_TURN
            || (hit_other_notch && net >= CAP_SCAN_NOTCH_BOUNDED_MIN_TURN)
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
        let t_peak_2v = (t1_curr
            + if t1_prev.abs() >= t1_next.abs() {
                t1_prev
            } else {
                t1_next
            })
        .abs();

        let is_oscillating_wobble =
            t_short[(i + m - w_local) % m] * t_short[i] < WOBBLE_OPPOSITE_TURN_PRODUCT
                || t_short[(i + w_local) % m] * t_short[i] < WOBBLE_OPPOSITE_TURN_PRODUCT;

        let is_cap_adjacent_notch = is_exterior
            && t_short[i] * winding_sign < 0.0
            && t1_curr * winding_sign < 0.0
            && ts_abs >= CAP_NOTCH_MIN_SHORT_TURN
            && t_peak_2v >= CAP_NOTCH_MIN_PEAK_2V_TURN
            && t_peak_2v >= CAP_NOTCH_MIN_PEAK_CONCENTRATION * ts_abs
            && t_1[(i + m - 1) % m] * t1_curr >= CAP_NOTCH_ADJACENT_SIGN_TOLERANCE * t1_curr.abs()
            && t_1[(i + 1) % m] * t1_curr >= CAP_NOTCH_ADJACENT_SIGN_TOLERANCE * t1_curr.abs()
            && (scan_adjacent_cap_side(i, false) || scan_adjacent_cap_side(i, true));

        let macro_valid = !is_exterior || t_macro[i] * winding_sign < 0.0;
        let is_right_sharp = macro_valid
            && t_short[i].cos() < RIGHT_CORNER_MAX_SHORT_COS
            && tl_abs > RIGHT_CORNER_MIN_LOCAL_TURN
            && t_macro[i].cos() < RIGHT_CORNER_MAX_MACRO_COS;
        let is_concentrated_junction = macro_valid
            && !is_oscillating_wobble
            && ts_abs > JUNCTION_MIN_SHORT_AND_LOCAL_TURN
            && tl_abs > JUNCTION_MIN_SHORT_AND_LOCAL_TURN
            && tm_abs > JUNCTION_MIN_MACRO_TURN
            && ts_abs >= JUNCTION_MIN_SHORT_TO_LOCAL_RATIO * tl_abs
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

fn detect_round_arcs(turns: &RingTurnMetrics, is_sharp_corner: &[bool]) -> Vec<bool> {
    let RingTurnMetrics {
        t_short,
        t_local,
        t_macro,
        w_macro,
        ..
    } = turns;
    let w_macro = *w_macro;
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
        if ts.abs() > ROUND_ARC_MIN_SHORT_TURN
            && tl.abs() > ROUND_ARC_MIN_LOCAL_TURN
            && tm.abs() > ROUND_ARC_MIN_MACRO_TURN
            && ts * tm > 0.0
            && tl * tm > 0.0
        {
            is_round_arc[i] = true;
        }
    }

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

    if !is_sharp_corner.iter().any(|&b| b)
        && is_round_arc.iter().filter(|&&b| b).count() * 5 >= m * 3
    {
        is_round_arc.fill(false);
    }
    is_round_arc
}

fn filter_protected_round_features(
    dense: &[Point],
    turns: &RingTurnMetrics,
    is_sharp_corner: &[bool],
    is_round_arc: &[bool],
    is_high_curv: &mut [bool],
    (r_scale, actual_ds): (f64, f64),
) {
    if !is_round_arc.iter().any(|&b| b) || is_round_arc.iter().all(|&b| b) {
        return;
    }
    let RingTurnMetrics {
        t_local,
        t_macro,
        w_local,
        w_macro,
        ..
    } = turns;
    let (&w_local, &w_macro) = (w_local, w_macro);
    let m = dense.len();
    let flank_len = ((44.0 * r_scale / actual_ds).round() as usize).clamp(5, (m / 4).max(5));
    let local_to_macro = (w_macro as f64 / w_local.max(1) as f64).max(1.0);
    let mut is_cap_vertex = vec![false; m];
    let run_metrics: Vec<(usize, usize, f64)> = find_true_runs(is_round_arc)
        .into_iter()
        .map(|(rs, re)| {
            let (b_i, a_i) = clamp_run_bounds(rs, re, m, is_sharp_corner);
            let (nt, _) = span_turn_metrics(&dense[b_i..=a_i]);
            if nt >= PROTECTED_CAP_MIN_NET_TURN && (re - rs) < m / 2 {
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
            && ((net_turn >= LOCAL_ROUND_CORNER_MIN_TURN
                && flank_before < LOCAL_ROUND_CORNER_MAX_FLANK_TURN
                && flank_after < LOCAL_ROUND_CORNER_MAX_FLANK_TURN
                && flank_before < LOCAL_ROUND_CORNER_MAX_FLANK_RATIO * peak_abs_tm
                && flank_after < LOCAL_ROUND_CORNER_MAX_FLANK_RATIO * peak_abs_tm)
                || (net_turn >= STRONG_ROUND_CORNER_MIN_TURN
                    && peak_abs_tl >= STRONG_ROUND_CORNER_MIN_PEAK_LOCAL
                    && flank_before < STRONG_ROUND_CORNER_MAX_FLANK_RATIO * peak_abs_tm
                    && flank_after < STRONG_ROUND_CORNER_MAX_FLANK_RATIO * peak_abs_tm));

        if !is_round_cap && !is_localized_round_corner {
            for k in r_start..=r_end {
                if !is_sharp_corner[k] {
                    is_high_curv[k] = false;
                }
            }
        }
    }
}

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

    for (i, &sharp) in is_sharp_corner.iter().enumerate() {
        if sharp {
            locked[i] = true;
        }
    }

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

pub(crate) fn fit_quad_control_point_with_arc_lengths(
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

    let p1_mid = Point::new(
        2.0 * mid_pt.x - 0.5 * (p0.x + p2.x),
        2.0 * mid_pt.y - 0.5 * (p0.y + p2.y),
    );
    let p1_ls = fit_quad_control_point_least_squares(pts, cum, p0, p2);
    p1_mid.lerp(p1_ls, 0.50)
}

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
        if dir_before.dot(dir_after) < G1_MIN_RING_DIR_DOT {
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
        if d_in.dot(d_out) < G1_MIN_TANGENT_DOT {
            continue;
        }

        anchor_tangents[j] = Some(normalize_vec(d_in + d_out + d_ring));
    }

    let max_radial_shift = G1_MAX_RADIAL_SHIFT_BRUSH_RATIO * r_scale;

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
        let candidate_p1 = if let (true, Some(t0), Some(t2)) = (has_any_smooth, t0_opt, t2_opt) {
            let cross = t0.x * t2.y - t0.y * t2.x;
            if cross.abs() > G1_MIN_TANGENT_CROSS {
                let s0 = (chord.x * t2.y - chord.y * t2.x) / cross;
                let s2 = (t0.x * chord.y - t0.y * chord.x) / cross;
                if s0 > G1_MIN_INTERSECT_CHORD_RATIO * chord_len
                    && s2 > G1_MIN_INTERSECT_CHORD_RATIO * chord_len
                    && s0 < G1_MAX_INTERSECT_CHORD_RATIO * chord_len
                    && s2 < G1_MAX_INTERSECT_CHORD_RATIO * chord_len
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
            let local_max_radial = max_radial_shift.min(G1_MAX_RADIAL_SHIFT_CHORD_RATIO * chord_len);
            if radial_diff <= local_max_radial && total_diff <= G1_MAX_TOTAL_SHIFT_CHORD_RATIO * chord_len {
                segments[j].p1 = p1_tan;
            }
        }
    }
}

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
        .filter(|&i| locked[i] || is_high_curv[i] || i % step == 0)
        .collect();

    if anchors.len() < 4 {
        anchors = vec![0, m / 4, m / 2, (3 * m) / 4];
        anchors.sort_unstable();
        anchors.dedup();
    }

    const MAX_CORNER_TURN_RAD: f64 = 1.68;
    let mut span_pts = Vec::with_capacity(m);
    let mut cum = Vec::with_capacity(m);

    let mut eval_removal_score = |prev_idx: usize, curr_idx: usize, next_idx: usize| -> f64 {
        if locked[curr_idx] {
            return f64::INFINITY;
        }

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

    let n = anchors.len();
    if n <= 3 {
        return anchors;
    }

    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = vec![true; n];
    let mut generation = vec![0u32; n];
    let mut alive_count = n;
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u64, usize, u32)>> =
        std::collections::BinaryHeap::with_capacity(n * 2);

    for i in 0..n {
        let s = eval_removal_score(anchors[prev[i]], anchors[i], anchors[next[i]]);
        if s.is_finite() {
            heap.push(std::cmp::Reverse((s.to_bits(), i, 0)));
        }
    }

    while alive_count > 3 {
        let Some(std::cmp::Reverse((_, u, gen))) = heap.pop() else {
            break;
        };
        if !alive[u] || generation[u] != gen {
            continue;
        }
        alive[u] = false;
        alive_count -= 1;
        let p = prev[u];
        let nx = next[u];
        next[p] = nx;
        prev[nx] = p;

        if alive_count > 3 {
            for v in [p, nx] {
                generation[v] = generation[v].wrapping_add(1);
                let sv = eval_removal_score(anchors[prev[v]], anchors[v], anchors[next[v]]);
                if sv.is_finite() {
                    heap.push(std::cmp::Reverse((sv.to_bits(), v, generation[v])));
                }
            }
        }
    }

    (0..n).filter(|&i| alive[i]).map(|i| anchors[i]).collect()
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

pub(crate) fn rdp_recursive(
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
