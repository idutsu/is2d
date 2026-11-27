//! 描く道具（ブラシ・消しゴム・鉛筆・バケツ・インクボトル・スポイト）のテスト

use super::*;

/// 図形 `shapes`（下から上への並び）の、点 `pt` を含む線で囲まれた所を `color` で塗った形（バケツと同じく地図で）。
/// 囲まれていない所なら `None`
fn find_enclosed_region_at(shapes: &[FillShape], pt: Point, color: Color32) -> Option<FillShape> {
    let map = crate::planar_map::painted_map_pub(shapes)?;
    let filled = map_overlay::bucket(&map, pt, color)?;
    let (faces, _) = filled.faces();
    faces
        .iter()
        .filter(|f| f.color == color)
        .map(|f| filled.face_shape(f, 1, &[]))
        .find(|s| s.contains_point(pt))
}

/// ペンをほぼ止めて、前の点からほとんど動かない点（0.0001 画素以下のずれ）が続けて来ても、ブラシの形が描いた所から
/// はみ出さない（中心線の入口 `CenterlineBuilder::push` は、前の点の丸めの範囲（縦横 ±h の箱）に入る点を、新しい節に
/// しない。前は、ビットまで同じ点だけを捨てたので、ほぼ 0 の節の間隔が中心線に入り、解き方が悪条件になって、描いた所から
/// 数十〜数百 px 離れた所までとげができ、ほかの色の絵を削った：レビュー de92ceb の N-2001）
#[test]
fn nearly_repeated_pen_points_make_no_spikes() {
    use crate::vector_stroke::stroke_shapes;
    let mut rng = fastrand::Rng::with_seed(0x5eed_2001);
    // 手描き風の線：ゆっくり（刻みより細かい点）と速い所を混ぜ、点は f32 に丸める（実際の入力の点と同じ）
    let stroke = |rng: &mut fastrand::Rng| -> Vec<Point> {
        let mut p = Point::new(300.0 + rng.f64() * 200.0, 300.0 + rng.f64() * 200.0);
        let mut ang = rng.f64() * std::f64::consts::TAU;
        let n = 40 + rng.usize(0..160);
        let mut out = Vec::new();
        for _ in 0..n {
            ang += rng.f64() * 0.3 - 0.15;
            let speed = if rng.usize(0..4) == 0 { 0.05 + rng.f64() * 0.4 } else { 1.0 + rng.f64() * 6.0 };
            p += KVec2::new(ang.cos(), ang.sin()) * speed;
            out.push(Point::new(p.x as f32 as f64, p.y as f32 as f64));
        }
        out
    };
    let (step, z, radius) = (0.5, 1.0, 5.0);
    let mut bad = Vec::new();
    for eps in [1e-12, 1e-9, 1e-6, 3.2e-5, 1e-4] {
        for k in 0..80 {
            // 点の半分の後ろに、x に ε だけずらした点を足す
            let mut pts = Vec::new();
            for (i, q) in stroke(&mut rng).into_iter().enumerate() {
                pts.push(q);
                if i % 2 == 0 {
                    pts.push(q + KVec2::new(eps, 0.0));
                }
            }
            let reach = pts
                .iter()
                .fold(kurbo::Rect::from_points(pts[0], pts[0]), |r, &q| r.union_pt(q))
                .inflate(radius + 10.0, radius + 10.0);
            let out = stroke_shapes(&pts, radius, step, z, Color32::RED)
                .iter()
                .map(|s| s.bounding_box())
                .any(|b| b.x0 < reach.x0 || b.y0 < reach.y0 || b.x1 > reach.x1 || b.y1 > reach.y1);
            if out {
                bad.push(format!("ε={eps} の {k} 本目"));
            }
        }
    }
    assert!(bad.is_empty(), "描いた所からはみ出す形ができた：{bad:?}");
}

/// R6：点の数に上限が無い。2万点の長い線（交わりながら進むリサージュ曲線）でも、離したあとの形が、中心線（丸めの
/// 範囲に入る）を半径で太らせた形と、ずれの上限（`STROKE_DEV_PX`）の中で一致する（前は、輪郭を 7200 点に
/// 間引き直していた）
#[test]
fn long_brush_stroke_keeps_its_shape() {
    use std::f64::consts::TAU;
    let pts: Vec<Point> = (0..20_000)
        .map(|i| {
            let t = TAU * i as f64 / 20_000.0;
            Point::new(
                (400.0 + 300.0 * (7.0 * t).sin() + 40.0 * (61.0 * t).sin()).round(),
                (300.0 + 200.0 * (5.0 * t).sin() + 40.0 * (67.0 * t).cos()).round(),
            )
        })
        .collect();
    let c = check_stroke_follows_centerline(&pts, 5.0, 1.0);
    assert!(
        c.worst_px <= STROKE_DEV_PX && c.outside_box == 0 && c.uncovered == 0 && c.missing == 0,
        "2万点の線の形がくずれた：{c:?}"
    );
}

/// 手でジグザグを描くと、角を少し行き過ぎて戻ったり、手ぶれで向きが細かく変わったりする。そのような線でも、角の内側・外側に切れ込み（ひげ）ができず、丸いブラシでなぞった形になること
#[test]
fn test_hand_drawn_zigzag_corners_have_no_notch() {
    use crate::vector_engine::point_to_segment_dist;

    let verts = [
        Point::new(400.0, 0.0),
        Point::new(170.0, 240.0),
        Point::new(390.0, 470.0),
        Point::new(140.0, 760.0),
        Point::new(400.0, 970.0),
    ];

    for (overshoot, jitter) in [(4.0, 0.0), (10.0, 0.0), (0.0, 0.8), (4.0, 0.8)] {
        let mut pts: Vec<Point> = Vec::new();
        let mut k = 0usize;
        for seg_idx in 0..(verts.len() - 1) {
            let a = verts[seg_idx];
            let b = verts[seg_idx + 1];
            let dir = normalize_vec(b - a);
            let len = (b - a).hypot();
            let end_s = if seg_idx + 2 < verts.len() {
                len + overshoot
            } else {
                len
            };
            let mut s = 0.0;
            loop {
                let base = a + dir * s;
                let kf = k as f64;
                let j = KVec2::new((kf * 1.7).sin(), (kf * 2.3).cos()) * jitter;
                let raw_p = base + j;
                let p = Point::new((raw_p.x * 2.0).round() * 0.5, (raw_p.y * 2.0).round() * 0.5);
                if pts.last().is_none_or(|&last| (p - last).hypot() >= 1.5) {
                    pts.push(p);
                }
                k += 1;
                if s >= end_s {
                    break;
                }
                let dist_corner = s.min((len - s).abs());
                let step = (0.5 + dist_corner / 20.0).min(9.0);
                s = (s + step).min(end_s);
            }
        }

        for r in [10.0, 25.0, 45.0] {
            let raw = brush_line(&pts, r);
            let shapes = brush_line_shapes(&raw, Color32::BLACK);
            assert!(!shapes.is_empty());

            let span = (r as i32) + 6;
            for &corner in &verts[1..4] {
                for dy in -span..=span {
                    for dx in -span..=span {
                        let p = Point::new(corner.x + dx as f64, corner.y + dy as f64);
                        let d = pts
                            .windows(2)
                            .map(|w| point_to_segment_dist(p, w[0], w[1]))
                            .fold(f64::INFINITY, f64::min);
                        let inside = shapes.iter().any(|sh| sh.contains_point(p));
                        if d < r - 2.5 {
                            assert!(
                                inside,
                                "切れ込み: corner={corner:?} p={p:?} d={d:.2} r={r} overshoot={overshoot} jitter={jitter}"
                            );
                        } else if d > r + 2.5 {
                            assert!(
                                !inside,
                                "はみ出し: corner={corner:?} p={p:?} d={d:.2} r={r} overshoot={overshoot} jitter={jitter}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn test_curved_stroke_has_strictly_uniform_width() {
    let r = 12.0;
    let base_q = QuadBez::new(
        Point::new(100.0, 250.0),
        Point::new(220.0, 130.0),
        Point::new(340.0, 250.0),
    );

    let pts: Vec<Point> = (0..=36)
        .map(|i| {
            let t = i as f64 / 36.0;
            let pt = base_q.eval(t);
            let wobble = if i == 0 || i == 36 {
                0.0
            } else {
                (i as f64 * 1.8).sin() * 0.35
            };
            Point::new(pt.x, pt.y + wobble)
        })
        .collect();

    let raw = brush_line(&pts, r);
    let shapes = brush_line_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];

    assert!(
        shape.exterior().segments.len() <= 13,
        "アーチ曲線のアンカー数が多すぎます: {}",
        shape.exterior().segments.len()
    );

    for &t in &[0.15, 0.32, 0.50, 0.68, 0.85] {
        let center = base_q.eval(t);
        let norm = left_normal(normalize_vec(base_q.deriv().eval(t).to_vec2()));

        assert!(shape.contains_point(center + norm * (r * 0.86)));
        assert!(shape.contains_point(center - norm * (r * 0.86)));
        assert!(!shape.contains_point(center + norm * (r * 1.16)));
        assert!(!shape.contains_point(center - norm * (r * 1.16)));
    }
}

#[test]
fn test_hand_drawn_rectangle_at_smoothing_100_keeps_corners_and_straight_sides() {
    let r = 8.0;
    let corners = [
        Point::new(100.0, 100.0),
        Point::new(260.0, 100.0),
        Point::new(260.0, 220.0),
        Point::new(100.0, 220.0),
        Point::new(100.0, 100.0),
    ];

    let mut pts = Vec::new();
    let mut step_idx = 0usize;
    for w in corners.windows(2) {
        let a = w[0];
        let b = w[1];
        let normal = left_normal(normalize_vec(b - a));
        let steps = 20;
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            let wobble = if k == 0 || k >= steps - 2 {
                0.0
            } else {
                ((step_idx as f64) * 1.9).sin() * 0.30
            };
            pts.push(a.lerp(b, t) + normal * wobble);
            step_idx += 1;
        }
    }
    pts.push(corners[0]);

    let raw = brush_line(&pts, r);
    let shapes = brush_line_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let rect_shape = &shapes[0];

    assert_eq!(rect_shape.holes().len(), 1, "四角形の中央に穴が1つあること");
    assert!(rect_shape.holes()[0].segments.len() <= 8);
    assert!(rect_shape.exterior().segments.len() <= 13);

    assert!(rect_shape.contains_point(Point::new(180.0, 100.0 - r * 0.82)));
    assert!(rect_shape.contains_point(Point::new(180.0, 100.0 + r * 0.82)));
    assert!(!rect_shape.contains_point(Point::new(180.0, 100.0 - r * 1.18)));
    assert!(!rect_shape.contains_point(Point::new(180.0, 100.0 + r * 1.18)));

    assert!(rect_shape.contains_point(Point::new(95.0, 95.0)));
    assert!(!rect_shape.contains_point(Point::new(92.2, 92.2)));
    assert!(rect_shape.contains_point(Point::new(265.0, 95.0)));
    assert!(!rect_shape.contains_point(Point::new(267.8, 92.2)));
}

#[test]
fn test_hand_drawn_circle_at_smoothing_100_is_round_and_uniform_width() {
    let center = Point::new(220.0, 220.0);
    let circle_r = 55.0;
    let brush_r = 9.0;

    let steps = 96usize;
    let pts: Vec<Point> = (0..=(steps + 2))
        .map(|i| {
            let theta = (i as f64 / steps as f64) * std::f64::consts::TAU;
            let wobble = (theta * 9.0).sin() * 0.32 + (theta * 15.0).cos() * 0.15;
            let r_curr = circle_r + wobble;
            Point::new(
                center.x + r_curr * theta.cos(),
                center.y + r_curr * theta.sin(),
            )
        })
        .collect();

    let raw = brush_line(&pts, brush_r);
    let shapes = brush_line_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let circle_shape = &shapes[0];
    assert_eq!(circle_shape.holes().len(), 1);

    let ext_len = circle_shape.exterior().segments.len();
    let hole_len = circle_shape.holes()[0].segments.len();
    assert!((6..=11).contains(&ext_len), "ext_len = {}", ext_len);
    assert!((6..=10).contains(&hole_len), "hole_len = {}", hole_len);

    for k in 0..16 {
        let ang = (k as f64 / 16.0) * std::f64::consts::TAU;
        let dir = KVec2::new(ang.cos(), ang.sin());

        assert!(circle_shape.contains_point(center + dir * (circle_r - brush_r * 0.82)));
        assert!(circle_shape.contains_point(center + dir * (circle_r + brush_r * 0.82)));
        assert!(!circle_shape.contains_point(center + dir * (circle_r - brush_r * 1.22)));
        assert!(!circle_shape.contains_point(center + dir * (circle_r + brush_r * 1.22)));
    }
}

#[test]
fn test_thin_2px_stroke_and_su_loop_keep_round_caps_and_uniform_width() {
    let r = 2.0;

    let h_pts: Vec<Point> = (0..=40)
        .map(|i| {
            let x = 100.0 + (i as f64) * 2.0;
            let wobble = if i == 0 || i == 40 {
                0.0
            } else {
                (i as f64 * 1.5).sin() * 0.35
            };
            Point::new(x, 120.0 + wobble)
        })
        .collect();

    let h_raw = brush_line(&h_pts, r);
    let h_shapes = brush_line_shapes(&h_raw, Color32::BLACK);
    assert_eq!(h_shapes.len(), 1);
    let h_shape = &h_shapes[0];

    assert!(h_shape.contains_point(Point::new(100.2, 120.0 - r * 0.70)));
    assert!(h_shape.contains_point(Point::new(100.2, 120.0 + r * 0.70)));
    assert!(h_shape.contains_point(Point::new(179.8, 120.0 - r * 0.70)));
    assert!(h_shape.contains_point(Point::new(179.8, 120.0 + r * 0.70)));
    assert!(!h_shape.contains_point(Point::new(140.0, 120.0 - r * 1.25)));
    assert!(!h_shape.contains_point(Point::new(140.0, 120.0 + r * 1.25)));

    let loop_center = Point::new(140.0, 160.0);
    let loop_r = 12.0;
    let loop_pts: Vec<Point> = (0..=48)
        .map(|i| {
            let theta = (i as f64 / 46.0) * std::f64::consts::TAU;
            let wobble = (theta * 6.0).sin() * 0.35;
            let rc = loop_r + wobble;
            Point::new(
                loop_center.x + rc * theta.cos(),
                loop_center.y + rc * theta.sin(),
            )
        })
        .collect();

    let l_raw = brush_line(&loop_pts, r);
    let l_shapes = brush_line_shapes(&l_raw, Color32::BLACK);
    assert_eq!(l_shapes.len(), 1);
    let l_shape = &l_shapes[0];
    assert_eq!(l_shape.holes().len(), 1);

    for k in 0..8 {
        let ang = (k as f64 / 8.0) * std::f64::consts::TAU;
        let dir = KVec2::new(ang.cos(), ang.sin());
        assert!(l_shape.contains_point(loop_center + dir * (loop_r - r * 0.72)));
        assert!(l_shape.contains_point(loop_center + dir * (loop_r + r * 0.72)));
        assert!(!l_shape.contains_point(loop_center + dir * (loop_r - r * 1.30)));
        assert!(!l_shape.contains_point(loop_center + dir * (loop_r + r * 1.30)));
    }
}

/// 1回のドラッグで往復の多いジグザグを描いても、なぞった点がすべて塗られる（中心線の間引きが途中で打ち切らない）
#[test]
fn long_zigzag_stroke_covers_every_input_point() {
    let mut points = Vec::new();
    for k in 0..100 {
        let y = 20.0 + k as f64 * 3.0;
        for i in 0..=20 {
            let x = if k % 2 == 0 { i } else { 20 - i } as f64 * 6.0 + 20.0;
            points.push(Point::new(x, y));
        }
    }
    let mut app = Is2dApp::default();
    run(
        &mut app,
        &[Action::PaintStroke {
            points: points.clone(),
            radius: 4.0,
            color: Color32::BLACK,
        }],
    );
    let shapes = scene_summary(&app).shapes.clone();
    let missing = points
        .iter()
        .filter(|&&p| !shapes.iter().any(|s| s.contains_point(p)))
        .count();
    assert_eq!(missing, 0, "塗られなかった点 {missing} / {}", points.len());
}

#[test]
fn test_bucket_fill_enclosed_hole_and_recolor() {
    let mut app = Is2dApp::default();
    let black = Color32::from_rgb(40, 45, 55);
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);

    // 中心 (200, 200)、半径 60px の黒い円環（ドーナツ状の線画）を描く
    let center = Point::new(200.0, 200.0);
    let ring_r = 60.0;
    let ring_pts: Vec<Point> = (0..=48)
        .map(|i| {
            let a = (i as f64 / 48.0) * std::f64::consts::TAU;
            Point::new(center.x + ring_r * a.cos(), center.y + ring_r * a.sin())
        })
        .collect();
    let raw_ring = brush_line(&ring_pts, 8.0);
    app.tool.brush_color = black;
    commit_brush_line(&mut app, &raw_ring);
    assert_eq!(scene_summary(&app).shapes.len(), 1);
    assert!(
        !scene_summary(&app).shapes[0].contains_point(center),
        "初期状態では中央は空白（穴）であること"
    );

    // 1. 閉じきっていない外側の背景 (20, 20) をバケツでクリックしても塗られないこと
    app.tool.brush_color = red;
    assert!(
        !doc_actions::bucket_fill_at(&mut app, Point::new(20.0, 20.0)),
        "開いた背景は塗りつぶされないこと"
    );

    // 2. 円環の内側の空白 (200, 200) を赤でバケツ塗り -> 赤い塗りシェイプが生成されること
    assert!(
        doc_actions::bucket_fill_at(&mut app, center),
        "閉領域がバケツで塗られること"
    );
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 2);
    let inner_shape = s.shapes.iter().find(|sh| sh.color() == red).unwrap();
    assert!(inner_shape.contains_point(center));

    // 3. 塗られた赤い領域を青でバケツクリック -> 青にリカラーされること
    app.tool.brush_color = blue;
    assert!(
        doc_actions::bucket_fill_at(&mut app, center),
        "既存の塗りシェイプが別色に塗り替わること"
    );
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 2);
    assert!(s
        .shapes
        .iter()
        .any(|sh| sh.color() == blue && sh.contains_point(center)));

    // 4. 内側の領域を周囲の線と同じ黒でバケツクリック -> 外周の黒リングと Union されて穴なしの1つの黒シェイプになること
    app.tool.brush_color = black;
    assert!(doc_actions::bucket_fill_at(&mut app, center));
    let s = scene_summary(&app);
    assert_eq!(
        s.shapes.len(),
        1,
        "同色でバケツ塗りすると周囲のシェイプと1つに結合されること"
    );
    assert!(s.shapes[0].holes().is_empty(), "穴が埋まって消えていること");
    assert!(s.shapes[0].contains_point(center));
}

#[test]
fn test_eyedropper_picks_topmost_shape_color() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(65, 195, 105);

    // 背面 (Layer 1) に赤シェイプ (x: 100..260, y: 200) を配置し、
    // 前面グループとして同一座標 (200, 200) に重なる青シェイプ (x: 180..320, y: 200) を重ねる
    let mut shapes = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(260.0, 200.0),
        20.0,
        red,
    );

    let mut s_blue = make_stroke(
        Point::new(180.0, 200.0),
        Point::new(320.0, 200.0),
        20.0,
        blue,
    );
    for sh in &mut s_blue {
        sh.group_path = vec![1];
    }
    shapes.extend(s_blue);
    put_cel_shapes(&mut app, 0, 0, shapes);

    app.tool.brush_color = green;
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool {
            tool: Tool::Eyedropper,
        },
    );

    // 1. 空白部分をクリックしても色は変わらないこと
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(20.0, 20.0)),
        None
    );
    assert_eq!(app.tool.brush_color, green);

    // 2. 背面の赤シェイプ単独部分 (120, 200) をクリックすると赤になること
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(120.0, 200.0)),
        Some(red)
    );
    assert_eq!(app.tool.brush_color, red);

    // 3. 赤と青が重なる同一座標 (200, 200) をクリックすると最前面の青が優先して返ること
    let s = scene_summary(&app);
    assert!(s.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(s.shapes[1].contains_point(Point::new(200.0, 200.0)));
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(200.0, 200.0)),
        Some(blue),
        "同一座標で2つの図形が重なっている場合、最前面の図形色が優先されること"
    );
    assert_eq!(app.tool.brush_color, blue);
}

#[test]
fn test_g1_tangent_continuity_on_wobbly_circle_stroke() {
    let center = Point::new(220.0, 220.0);
    let circle_r = 60.0;
    let brush_r = 10.0;

    let steps = 96usize;
    let pts: Vec<Point> = (0..=(steps + 2))
        .map(|i| {
            let theta = (i as f64 / steps as f64) * std::f64::consts::TAU;
            let wobble = (theta * 11.0).sin() * 0.35 + (theta * 17.0).cos() * 0.18;
            let r_curr = circle_r + wobble;
            Point::new(
                center.x + r_curr * theta.cos(),
                center.y + r_curr * theta.sin(),
            )
        })
        .collect();

    let raw = brush_line(&pts, brush_r);
    let shapes = brush_line_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];

    // 線は1周より少し先まで描くので、描き始めと描き終わりの半円が、もう一方の帯や半円と交わる所だけは角になる
    // （描いたとおりの形）。ほかのつなぎ目は、両側の曲線が同じ接線を使うので G1 でつながる
    for (c_idx, contour) in std::iter::once(shape.exterior())
        .chain(shape.holes().iter())
        .enumerate()
    {
        let k = contour.segments.len();
        for j in 0..k {
            let prev = (j + k - 1) % k;
            let t_in = normalize_vec(contour.segments[prev].p2 - contour.segments[prev].p1);
            let t_out = normalize_vec(contour.segments[j].p1 - contour.segments[j].p0);
            let dot = t_in.dot(t_out);
            if dot <= 0.9999 {
                // 角は、描き始め・描き終わりの半円が、もう一方の帯と交わる所（その点から半径ほど）
                let near_end = |q: Point| (contour.segments[j].p0 - q).hypot() < brush_r + 2.0;
                assert!(
                    near_end(pts[0]) || near_end(pts[pts.len() - 1]),
                    "contour {c_idx} アンカー {j}/{k} で前後のベジェ曲線の接線が G1 連続に揃っていること (dot = {dot})"
                );
            }
        }
    }
}

/// ツールのキー：鉛筆は S、インクボトルは O（利用者が決めた。Y は使わない。オニオンスキンのキーは無い）
#[test]
fn pencil_is_s_and_ink_bottle_is_o() {
    let key = |tool: Tool| {
        crate::menu::keys_text(|a| matches!(a, crate::panels::UiAction::Doc(Action::SetTool { tool: t }) if *t == tool))
    };
    assert_eq!(key(Tool::Pencil), "S");
    assert_eq!(key(Tool::InkBottle), "O");
    assert!(Tool::ALL.iter().all(|&t| !key(t).is_empty() && key(t) != "Y"));
}

#[test]
fn test_stage01_brush_eraser_bucket_on_cel() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 90, 220);
    let green = Color32::from_rgb(40, 190, 90);

    // 1. ブラシ：同色ストロークの自動結合（Union）
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
                radius: 14.0,
                color: red,
            },
            Action::PaintStroke {
                points: vec![Point::new(150.0, 60.0), Point::new(150.0, 140.0)],
                radius: 14.0,
                color: red,
            },
        ],
    );
    let s1 = cel_summary(&app.doc, lid, 0);
    assert_eq!(s1.shape_count, 1, "同色ストロークが1図形に結合されること");
    assert_eq!(s1.colors, vec![red]);

    // 2. ブラシ：異色ストロークによる背面図形の切り抜き（Subtraction）
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(150.0, 100.0), Point::new(250.0, 100.0)],
            radius: 12.0,
            color: blue,
        }],
    );
    let s2 = cel_summary(&app.doc, lid, 0);
    assert_eq!(s2.shape_count, 2);
    assert!(s2.colors.contains(&red) && s2.colors.contains(&blue));

    // 3. 消しゴム：中央を縦断して図形を分割消去
    let area_before_erase = s2.total_area;
    run(
        &mut app,
        &[Action::EraseStroke {
            points: vec![Point::new(120.0, 50.0), Point::new(120.0, 150.0)],
            radius: 10.0,
        }],
    );
    let s3 = cel_summary(&app.doc, lid, 0);
    assert!(
        s3.total_area < area_before_erase,
        "消しゴムでセル内図形の面積が減少すること"
    );

    // 4. バケツ：閉領域（ドーナツ穴）の塗りつぶし＋既存図形の色変更
    let mut ring_app = Is2dApp::default();
    let ring_lid = ring_app.doc.root.layers[0].id;
    let ring_pts: Vec<Point> = (0..=32)
        .map(|i| {
            let a = (i as f64 / 32.0) * std::f64::consts::TAU;
            Point::new(300.0 + 50.0 * a.cos(), 220.0 + 50.0 * a.sin())
        })
        .collect();
    run(
        &mut ring_app,
        &[
            Action::PaintStroke {
                points: ring_pts,
                radius: 10.0,
                color: red,
            },
            // 中央の穴を青でバケツ塗り
            Action::BucketFill {
                pt: Point::new(300.0, 220.0),
                color: blue,
            },
            // 外周リング自体を緑へ色変更
            Action::BucketFill {
                pt: Point::new(350.0, 220.0),
                color: green,
            },
        ],
    );
    let s_ring = cel_summary(&ring_app.doc, ring_lid, 0);
    assert!(s_ring.exists);
    assert_eq!(s_ring.hole_count, 1);
    assert_eq!(s_ring.shape_count, 2);
    assert!(s_ring.colors.contains(&blue) && s_ring.colors.contains(&green));
}

/// ブラシ・消しゴム・鉛筆の線が地図の格子の外（±65,536px）にかかるときは、線の形を作る前に「地図にできない形」と断り、
/// 作品・選択・取り消しの控えは変わらず、続けて使える（設計審査 392e9af の D-1 の確かめで見つけた：前は確かめずに線の形を
/// 作り始め、点が格子のはるか外（1e300）だと形を作る所が止まり、座標がとても大きい（1e18）とメモリを使い果たして固まった）
#[test]
fn a_stroke_off_the_grid_is_refused_before_making_its_shape() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    let doc_before = app.doc.clone();
    let (undo_before, redo_before) = (app.history.undo_stack.len(), app.history.redo_stack.len());
    let far = [
        Action::PaintStroke { points: vec![Point::new(1e300, 1.0), Point::new(5.0, 5.0)], radius: 5.0, color: Color32::GREEN },
        Action::PaintStroke { points: vec![Point::new(1e18, 1.0), Point::new(-1e18, 5.0)], radius: 1e17, color: Color32::GREEN },
        Action::PaintStroke { points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)], radius: 1e9, color: Color32::GREEN },
        Action::EraseStroke { points: vec![Point::new(1e18, 1.0), Point::new(5.0, 5.0)], radius: 5.0 },
        Action::PencilStroke {
            points: vec![Point::new(1e18, 1.0), Point::new(5.0, 5.0), Point::new(9.0, 9.0)],
            stroke: crate::planar_map::EdgeStroke { color: Color32::BLUE, width: 2 },
        },
    ];
    for action in far {
        app.notices = Default::default();
        assert!(!apply(&mut app, action), "格子の外の線を断らない");
        assert_eq!(
            app.notices.iter().last().map(|t| t.message.as_str()),
            Some("地図の範囲（±65,536px）の外や、潰れるほど小さくには置けません")
        );
        assert!(
            app.doc.root.layers.iter().zip(doc_before.root.layers.iter()).all(|(a, b)| a.keyframes.iter().zip(b.keyframes.iter()).all(|(x, y)| std::sync::Arc::ptr_eq(x.cel.maps(), y.cel.maps()))),
            "作品が変わった"
        );
        assert_eq!((app.history.undo_stack.len(), app.history.redo_stack.len()), (undo_before, redo_before), "取り消しの控えが変わった");
    }
    // 続けて使える
    assert!(apply(&mut app, Action::PaintStroke { points: vec![Point::new(400.0, 400.0), Point::new(450.0, 420.0)], radius: 10.0, color: Color32::BLUE }));
}

/// ほかの線と交わって切られた線の区間は、新しい番号になり（塗りの面が切られたときと同じ）、同じ地図の中で番号は重ならない
#[test]
fn crossing_lines_get_new_ids_and_ids_stay_unique() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 4 };
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let line = |a: Point, b: Point| -> Vec<Point> { (0..=30).map(|i| a.lerp(b, i as f64 / 30.0)).collect() };
    assert!(apply(&mut app, Action::PencilStroke { points: line(Point::new(100.0, 200.0), Point::new(400.0, 200.0)), stroke: ink }));
    let ids = |app: &Is2dApp| -> Vec<u64> { app.scene().iter().filter(|s| s.line.is_some()).map(|s| s.shape_id).collect() };
    let horizontal = ids(&app);
    // 縦の線を、横の線を横切るように引く
    assert!(apply(&mut app, Action::PencilStroke { points: line(Point::new(250.0, 100.0), Point::new(250.0, 300.0)), stroke: ink }));
    let mut all = ids(&app);
    let n = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), n, "線の番号が重なった");
    // 横の線のうち、交わった所で切られた区間は新しい番号（切られなかった区間は同じ番号）
    let crossed = app
        .scene()
        .iter()
        .filter(|s| s.line.is_some())
        .find(|s| s.center_curve().is_some_and(|q| q.p0.x.min(q.p2.x) <= 250.0 && q.p0.x.max(q.p2.x) >= 250.0 && (q.p0.y - 200.0).abs() < 1.0));
    let c = crossed.expect("横の線の、交わった所の区間がある");
    assert!(!horizontal.contains(&c.shape_id), "切られた区間が元の番号のまま");
}

#[test]
fn test_island_in_far_hole_survives_union_and_erase() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let circle_pts = |cx: f64, cy: f64, r: f64, n: usize| -> Vec<Point> {
        (0..n)
            .map(|i| {
                let a = 2.0 * std::f64::consts::PI * (i as f64) / ((n - 1) as f64);
                Point::new(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    };
    let hair_arc: Vec<Point> = (0..81)
        .map(|i| {
            let a = -std::f64::consts::PI * (i as f64) / 80.0;
            Point::new(200.0 + 104.0 * a.cos(), 150.0 + 104.0 * a.sin())
        })
        .collect();

    for use_erase in [false, true] {
        let mut app = Is2dApp::default();
        for (points, radius) in [
            (circle_pts(200.0, 150.0, 100.0, 121), 3.0),
            (circle_pts(240.0, 130.0, 12.0, 41), 2.0),
            (
                vec![Point::new(252.0, 130.0), Point::new(297.0, 130.0)],
                2.0,
            ),
            (
                vec![Point::new(240.0, 130.0), Point::new(240.1, 130.0)],
                4.0,
            ),
        ] {
            assert!(apply(
                &mut app,
                Action::PaintStroke {
                    points,
                    radius,
                    color: Color32::BLACK,
                },
            ));
        }
        assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 2);

        let action = if use_erase {
            Action::EraseStroke {
                points: hair_arc.clone(),
                radius: 3.0,
            }
        } else {
            Action::PaintStroke {
                points: hair_arc.clone(),
                radius: 3.0,
                color: Color32::BLACK,
            }
        };
        assert!(apply(&mut app, action));

        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
        assert_eq!(shapes.len(), 2);
        assert!(shapes
            .iter()
            .any(|s| s.contains_point(Point::new(240.0, 130.0))));
    }
}

fn blob_with_small_hole_and_wavy_stroke(color: Color32) -> (Vec<FillShape>, Vec<FillShape>) {
    let mut canvas = Vec::new();
    let base_raw = brush_line(&[Point::new(-250.0, 0.0), Point::new(250.0, 0.0)], 120.0);
    let base_shapes = brush_line_shapes(&base_raw, Color32::BLACK);
    merge_shapes_into_canvas(&mut canvas, base_shapes);

    let hole_raw = brush_line(&[Point::new(-1.0, 0.0), Point::new(1.0, 0.0)], 4.0);
    let hole_shapes = brush_line_shapes(&hole_raw, Color32::BLACK);
    erase_from_canvas(&mut canvas, &hole_shapes, 1, &[]);
    assert_eq!(canvas.len(), 1);
    assert_eq!(canvas[0].holes().len(), 1);
    assert!(!canvas[0].contains_point(Point::ZERO));

    let wavy_pts: Vec<Point> = (-400..=400)
        .map(|i| {
            let x = i as f64 * 0.5;
            Point::new(x, 6.0 * (x / 6.0).sin())
        })
        .collect();
    let wavy_raw = brush_line(&wavy_pts, 30.0);
    let wavy_shapes = brush_line_shapes(&wavy_raw, color);
    (canvas, wavy_shapes)
}

#[test]
fn test_thick_stroke_covering_small_hole_fills_it() {
    let (mut canvas, wavy_shapes) = blob_with_small_hole_and_wavy_stroke(Color32::BLACK);
    merge_shapes_into_canvas(&mut canvas, wavy_shapes);
    assert!(canvas.iter().any(|s| s.contains_point(Point::ZERO)));
    assert_eq!(canvas.iter().map(|s| s.holes().len()).sum::<usize>(), 0);
}

#[test]
fn test_thick_other_color_stroke_covering_small_hole_leaves_no_island() {
    let red = Color32::from_rgb(220, 60, 60);
    let (mut canvas, wavy_shapes) = blob_with_small_hole_and_wavy_stroke(red);
    merge_shapes_into_canvas(&mut canvas, wavy_shapes);
    assert_eq!(
        canvas
            .iter()
            .filter(|s| s.color() == Color32::BLACK && s.contains_point(Point::ZERO))
            .count(),
        0
    );
    assert_eq!(
        canvas
            .iter()
            .filter(|s| s.color() == red && s.contains_point(Point::ZERO))
            .count(),
        1
    );
}

#[test]
fn test_thick_eraser_covering_small_hole_leaves_no_island() {
    let (mut canvas, wavy_shapes) = blob_with_small_hole_and_wavy_stroke(Color32::BLACK);
    erase_from_canvas(&mut canvas, &wavy_shapes, 1, &[]);
    assert!(!canvas.iter().any(|s| s.contains_point(Point::ZERO)));
    assert!(!canvas
        .iter()
        .any(|s| s.contains_point(Point::new(20.0, 10.0))));
    assert!(canvas
        .iter()
        .any(|s| s.contains_point(Point::new(0.0, 80.0))));
}

#[test]
fn test_bucket_in_grid_line_art_fills_only_that_cell() {
    let n = 21;
    let step = 15.0;
    let span = (n - 1) as f64 * step;
    let mut strokes = Vec::with_capacity(n * 2);
    for i in 0..n {
        let pos = i as f64 * step;
        let h_pts: Vec<Point> = (0..=60)
            .map(|k| Point::new(span * (k as f64 / 60.0), pos))
            .collect();
        let h_raw = brush_line(&h_pts, 1.5);
        strokes.extend(brush_line_shapes(&h_raw, Color32::BLACK));

        let v_pts: Vec<Point> = (0..=60)
            .map(|k| Point::new(pos, span * (k as f64 / 60.0)))
            .collect();
        let v_raw = brush_line(&v_pts, 1.5);
        strokes.extend(brush_line_shapes(&v_raw, Color32::BLACK));
    }
    let mut canvas = Vec::new();
    merge_shapes_into_canvas(&mut canvas, strokes);
    assert_eq!(canvas.len(), 1);
    assert_eq!(canvas[0].holes().len(), 400);

    let fill_col = Color32::from_rgb(80, 180, 240);
    for (cx, cy) in [(0, 0), (5, 7), (19, 19), (10, 3)] {
        let center = Point::new((cx as f64 + 0.5) * step, (cy as f64 + 0.5) * step);
        let filled = find_enclosed_region_at(&canvas, center, fill_col)
            .expect("格子のマスの中央で閉領域が見つかること");
        assert!(filled.contains_point(center));
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            let neighbor = Point::new(center.x + dx * step, center.y + dy * step);
            assert!(!filled.contains_point(neighbor));
        }
    }
}

/// バケツで面の色を塗り替えても、形も色も変わらないとなりの図形は書き換えない（その側の印で、変わっていないと分かる。
/// レビュー 4a9ea60 の N-1003）
#[test]
fn test_bucket_recolor_keeps_neighbor_shapes() {
    use crate::planar_map::LocalEdit;
    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    // 横に並べた3本の帯（となりどうしが触れる）
    for (i, color) in [Color32::RED, Color32::BLUE, Color32::GREEN]
        .into_iter()
        .enumerate()
    {
        app.tool.brush_color = color;
        let y = 40.0 * i as f64;
        let pts: Vec<Point> = (0..=8)
            .map(|k| Point::new(-120.0 + 30.0 * k as f64, y))
            .collect();
        commit_brush_line(&mut app, &brush_line(&pts, 24.0));
    }
    let shapes = app.scene().iter().cloned().collect::<Vec<_>>();
    assert!(crate::planar_map::PlanarMap::from_faces(shapes.iter()).is_valid());
    let blue = shapes
        .iter()
        .find(|s| s.color() == Color32::BLUE)
        .expect("青の帯")
        .shape_id;
    let base = app.session.base_cache().cel(lid).unwrap().scope_base(&[]);
    let LocalEdit::Changed(w) = base.fill_local(Point::new(0.0, 40.0), Color32::BLACK, &[]) else {
        panic!("塗り替えられない");
    };
    assert_eq!(w.removed, vec![blue], "となりの図形まで書き換えた");
    assert_eq!(w.added.len(), 1);
    assert_eq!(w.faces.by_id(w.added[0]).unwrap().color(), Color32::BLACK);
}

/// 線の多いセル画に短い線を1本描いても、前の地図にある辺の判定の形（`grid_poly`）と、重ね合わせで近くの辺を探す外接
/// 矩形とマスは作り直さず、作るのは線のまわりの分だけ。面の輪郭とその輪も触った所の分だけ作り、しまうときも変わった所の
/// 塊だけを並べ直す（触った所だけの道。レビュー f01d518 の R3）
#[test]
fn test_short_brush_builds_polylines_only_near_the_stroke() {
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut app = Is2dApp::default();
    let mut rng = map_overlay::Rng(5);
    let stroke = |rng: &mut map_overlay::Rng, n: usize| -> Vec<Point> {
        let mut p = Point::new(-200.0 + rng.f() * 400.0, -120.0 + rng.f() * 240.0);
        let mut a = rng.f() * std::f64::consts::TAU;
        (0..n)
            .map(|_| {
                a += (rng.f() - 0.5) * 0.8;
                p += kurbo::Vec2::new(a.cos(), a.sin()) * 3.0;
                p
            })
            .collect()
    };
    for k in 0..60 {
        let points = stroke(&mut rng, 30);
        let color = colors[k % 3];
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::PaintStroke {
                points,
                radius: 4.0,
                color
            }
        ));
    }
    let edges: usize = app.doc.root.layers[0].keyframes[0]
        .cel
        .maps()
        .iter()
        .map(|m| m.base().map().edges.len())
        .sum();
    assert!(edges > 800, "辺 {edges} 本");
    let nodes: usize = app.doc.root.layers[0].keyframes[0]
        .cel
        .maps()
        .iter()
        .map(|m| m.base().map().nodes.len())
        .sum();
    let cycles: usize = app.doc.root.layers[0].keyframes[0]
        .cel
        .maps()
        .iter()
        .map(|m| m.base().cycles().len())
        .sum();
    let (before, reaches_before, sorted_before, rings_before, keys_before) = (
        crate::planar_map::polys_built(),
        crate::planar_map::reaches_built(),
        crate::planar_map::vertices_sorted(),
        crate::planar_map::rings_built(),
        crate::planar_map::keys_built(),
    );
    let (placed_before, dense_before) = (
        crate::stored_map::edges_sorted(),
        crate::planar_map::dense_builds(),
    );
    let points = stroke(&mut rng, 6);
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points,
            radius: 3.0,
            color: colors[0]
        }
    ));
    let built = crate::planar_map::polys_built() - before;
    assert!(
        built * 4 < edges,
        "作った判定の形 {built} 本（辺 {edges} 本）"
    );
    // 重ね合わせで近くの辺を探す外接矩形とマスも、前の地図の写しにある辺の分は作り直さない（`EdgeGeom::reach`）
    let reaches = crate::planar_map::reaches_built() - reaches_before;
    assert!(
        reaches * 4 < edges,
        "作った外接矩形とマス {reaches} 本（辺 {edges} 本）"
    );
    // 面の輪郭も、触った所の輪郭だけをたどり、その頂点だけ、まわりの辺を並べる（触った所だけの道。大きな面に触れれば、
    // その輪郭の長さの分は並べる）
    let sorted = crate::planar_map::vertices_sorted() - sorted_before;
    assert!(
        sorted < nodes,
        "まわりの辺を並べた頂点 {sorted}（頂点 {nodes}）"
    );
    // 面の輪郭の点と面積も、触った輪郭の分だけ作る（触った所だけの道）
    let rings = crate::planar_map::rings_built() - rings_before;
    assert!(
        rings * 4 < cycles,
        "作った輪郭の点と面積 {rings}（輪郭 {cycles}）"
    );
    // 辺の鍵（`edge_key`）は、全部の辺の分を何度も求めない
    let keys = crate::planar_map::keys_built() - keys_before;
    assert!(keys < 3 * edges, "求めた辺の鍵 {keys}（辺 {edges} 本）");
    // しまうときも、変わった所の塊の辺だけを並べ直し（`StoredMap::with_diff_tiles`）、計算に使う地図は、しまう形から
    // 作り直さない
    let placed = crate::stored_map::edges_sorted() - placed_before;
    assert!(placed * 4 < edges, "並べ直した辺 {placed}（辺 {edges} 本）");
    assert_eq!(
        crate::planar_map::dense_builds(),
        dense_before,
        "しまう形から計算に使う地図を作り直した"
    );
}

/// 同じ色の四角 `(x0, y0)-(x1, y1)` の穴の輪郭（外周と逆の向き）
fn reversed_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> crate::vector_engine::Contour {
    let s = map_overlay::rect_shape(x0, y0, x1, y1, Color32::WHITE);
    crate::vector_engine::Contour::new(
        s.exterior()
            .segments
            .iter()
            .rev()
            .map(|q| kurbo::QuadBez::new(q.p2, q.p1, q.p0))
            .collect(),
    )
}

/// 線で囲まれた何も無い所をバケツで塗るとき、塗る面（`LocalMap::face_at`）を決めるのに半直線で探すのは、点を含む外周の
/// 1回だけ。囲いの中の島の外周の入れ先は、囲いの中の何も無い側の別の外周とだけ比べる（レビュー 983236e の N-1004）。
/// 書き戻し（`apply_diff`）は、塗った面の触った穴（島の外周）ごとに入れ先を探すので、島が1つ増えると、探す回数は1回
/// 増える（面を決める所でも島ごとに探すと、2回増える）
#[test]
fn bucket_in_an_enclosure_searches_the_face_once_whatever_the_islands() {
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 90, 220);
    let searches = |k: usize| -> usize {
        // 枠（四角い輪）の中に、離れた小さな四角が k×k 個と、中に何も無い所（別の外周）のある輪が1つ
        let w = k as f64 * 20.0 + 60.0;
        let frame = map_overlay::rect_shape(0.0, 0.0, w, w, red).exterior().clone();
        let inner = reversed_rect(10.0, 10.0, w - 10.0, w - 10.0);
        let ring = map_overlay::rect_shape(w - 35.0, w - 35.0, w - 15.0, w - 15.0, red).exterior().clone();
        let mut shapes = vec![
            FillShape::new(red, frame, vec![inner]),
            FillShape::new(red, ring, vec![reversed_rect(w - 30.0, w - 30.0, w - 20.0, w - 20.0)]),
        ];
        for i in 0..k * k {
            let (x, y) = ((i % k) as f64 * 20.0 + 25.0, (i / k) as f64 * 20.0 + 25.0);
            shapes.push(map_overlay::rect_shape(x, y, x + 10.0, y + 10.0, red));
        }
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, shapes);
        let before = crate::planar_map::owner_searches();
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::BucketFill {
                pt: Point::new(15.0, 15.0),
                color: blue,
            },
        ));
        let n = crate::planar_map::owner_searches() - before;
        // 塗った面：囲いの内側を外周に、島と輪の外周を穴に持つ（輪の中の何も無い所は塗らない）
        let filled: Vec<usize> = app
            .scene()
            .iter()
            .filter(|s| s.color() == blue)
            .map(|s| s.holes().len())
            .collect();
        assert_eq!(filled, vec![k * k + 1]);
        n
    };
    assert_eq!(searches(6) - searches(3), 6 * 6 - 3 * 3);
}

/// N-2006：バケツで、当たった面と同じ色を塗っても何も変わらない（変わるかは地図の側で決める：`fill_local`）。
/// 文書も Undo の履歴も変わらない
#[test]
fn bucket_with_the_same_color_changes_nothing() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, red)],
    );
    let stored = |app: &Is2dApp| {
        app.doc.root.layers[0].keyframes[0].cel.maps()[0]
            .stored()
            .clone()
    };
    let (before, undo_len) = (stored(&app), app.history.undo_stack.len());
    assert!(!apply(
        &mut app,
        Action::BucketFill {
            pt: Point::new(200.0, 200.0),
            color: red,
        },
    ));
    assert!(stored(&app).same(&before));
    assert_eq!(app.history.undo_stack.len(), undo_len);
}

#[test]
fn test_smoothing_100_preserves_width_round_caps_and_minimal_anchors() {
    let r = 10.0;
    let raw_pts: Vec<Point> = (0..=30)
        .map(|i| {
            let t = i as f64 / 30.0;
            let x = 100.0 + 200.0 * t;
            let jitter = if i == 0 || i == 30 {
                0.0
            } else {
                (i as f64 * 1.7).sin() * 0.38
            };
            Point::new(x, 200.0 + jitter)
        })
        .collect();

    let raw = brush_line(&raw_pts, r);
    let shapes = brush_line_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];

    assert!(
        shape.exterior().segments.len() <= 9,
        "単純なストロークのアンカー数が多すぎます: {}",
        shape.exterior().segments.len()
    );

    for &x in &[115.0, 200.0, 285.0] {
        assert!(
            shape.contains_point(Point::new(x, 200.0 - r * 0.88)),
            "x={} 上側内部が欠けています",
            x
        );
        assert!(
            shape.contains_point(Point::new(x, 200.0 + r * 0.88)),
            "x={} 下側内部が欠けています",
            x
        );
        assert!(
            !shape.contains_point(Point::new(x, 200.0 - r * 1.15)),
            "x={} 上側が膨らみすぎています",
            x
        );
        assert!(
            !shape.contains_point(Point::new(x, 200.0 + r * 1.15)),
            "x={} 下側が膨らみすぎています",
            x
        );
    }

    let diag = r * 0.82 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(shape.contains_point(Point::new(100.0 - diag, 200.0 - diag)));
    assert!(shape.contains_point(Point::new(100.0 - diag, 200.0 + diag)));
    assert!(shape.contains_point(Point::new(300.0 + diag, 200.0 - diag)));
    assert!(shape.contains_point(Point::new(300.0 + diag, 200.0 + diag)));
}

/// テスト用：メッシュの三角形を格子に入れて、点が三角形のどれかに入るか（その形が塗られて見えるか）を速く調べる
struct TriGrid {
    cell: f64,
    cells: std::collections::HashMap<(i64, i64), Vec<[Point; 3]>>,
    verts: Vec<Point>,
}

impl TriGrid {
    fn new(meshes: &[&egui::Mesh], cell: f64) -> Self {
        let mut cells: std::collections::HashMap<(i64, i64), Vec<[Point; 3]>> = Default::default();
        let mut verts = Vec::new();
        for m in meshes {
            let at = |i: u32| {
                let v = m.vertices[i as usize].pos;
                Point::new(v.x as f64, v.y as f64)
            };
            verts.extend(
                m.vertices
                    .iter()
                    .map(|v| Point::new(v.pos.x as f64, v.pos.y as f64)),
            );
            for t in m.indices.chunks_exact(3) {
                let tri = [at(t[0]), at(t[1]), at(t[2])];
                let k = |v: f64| (v / cell).floor() as i64;
                let (x0, x1) = (
                    tri.iter().map(|p| p.x).fold(f64::MAX, f64::min),
                    tri.iter().map(|p| p.x).fold(f64::MIN, f64::max),
                );
                let (y0, y1) = (
                    tri.iter().map(|p| p.y).fold(f64::MAX, f64::min),
                    tri.iter().map(|p| p.y).fold(f64::MIN, f64::max),
                );
                for x in k(x0)..=k(x1) {
                    for y in k(y0)..=k(y1) {
                        cells.entry((x, y)).or_default().push(tri);
                    }
                }
            }
        }
        Self { cell, cells, verts }
    }

    fn contains(&self, p: Point) -> bool {
        let key = (
            (p.x / self.cell).floor() as i64,
            (p.y / self.cell).floor() as i64,
        );
        self.cells.get(&key).into_iter().flatten().any(|t| {
            let d: Vec<f64> = (0..3)
                .map(|i| (t[(i + 1) % 3] - t[i]).cross(p - t[i]))
                .collect();
            d.iter().all(|&v| v >= 0.0) || d.iter().all(|&v| v <= 0.0)
        })
    }
}

/// R6（N-2005）：描いている間に見えていた形と、離したあとの形が、当てはめの許容の中で同じ。長い線（渦巻き・
/// ハッチング）を画面の操作で描き、離す直前の表示（確定した所と、確定していない所のメッシュ）と、離したあとの図形を
/// 比べる。ずれの上限は `STROKE_DEV_PX`（画面の px）：
/// - 表示の頂点は、どれも離したあとの形の中か、その輪郭から上限以内にある（表示が形からはみ出さない）
/// - 離したあとの輪郭の各点で、法線に沿って上限だけ内側の点は表示の中、外側の点は表示の外（形が表示からはみ出さない。
///   その2点が形の内と外に分かれるときだけ見る：狭いすき間で向こう側の線に入る点は見ない）
#[test]
fn brush_preview_matches_the_committed_shape() {
    use eframe::egui;
    use std::f64::consts::TAU;
    // 画面の点（整数に丸める）を 3px ほどおきに
    let spiral: Vec<Pos2> = {
        let mut out = Vec::new();
        let mut t = 0.0f64;
        while t < 4.0 * TAU {
            let r = 8.0 + 172.0 * t / (4.0 * TAU);
            out.push(Pos2::new(
                (500.0 + r * t.cos()).round() as f32,
                (330.0 + r * t.sin()).round() as f32,
            ));
            t += 3.0 / r;
        }
        out
    };
    let hatch: Vec<Pos2> = (0..24)
        .flat_map(|k| {
            let y = 200.0 + 11.0 * k as f64;
            (0..120).map(move |i| {
                let x = if k % 2 == 0 {
                    320.0 + 3.0 * i as f64
                } else {
                    680.0 - 3.0 * i as f64
                };
                Pos2::new(x as f32, (y + 1.5 * (0.31 * i as f64).sin()).round() as f32)
            })
        })
        .collect();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));
    for (name, path) in [("渦巻き", spiral), ("ハッチング", hatch)] {
        let ctx = egui::Context::default();
        let mut app = Is2dApp::default();
        app.tool.tool = Tool::Brush;
        app.tool.brush_radius = 6.0;
        let frame = |app: &mut Is2dApp, events: Vec<egui::Event>| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect,
                    events,
                    ..Default::default()
                },
                |ctx| app.update_ui(ctx),
            );
        };
        let press = |pos: Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![egui::Event::PointerMoved(path[0])]);
        frame(
            &mut app,
            vec![egui::Event::PointerMoved(path[0]), press(path[0], true)],
        );
        for &p in &path[1..] {
            frame(&mut app, vec![egui::Event::PointerMoved(p)]);
        }
        let (committed, tail) = match &app.tab().session.drag {
            canvas_tool::ActiveCanvasDrag::PaintStroke {
                committed_world_mesh,
                tail_mesh,
                ..
            } => (committed_world_mesh.clone(), tail_mesh.clone()),
            _ => panic!("{name}：ブラシで描いている途中になっていない"),
        };
        frame(&mut app, vec![press(*path.last().unwrap(), false)]);
        let shapes = scene_summary(&app).shapes;
        assert!(!shapes.is_empty(), "{name}：離したあとの形が無い");
        let zoom = app.tab().view.zoom() as f64;
        let tol = STROKE_DEV_PX / zoom;
        let outline = shape_outline_polylines(&shapes, 0.005 / zoom);
        let near_outline = SegGrid::new(&outline, true, 4.0);
        let inside = |p: Point| shapes.iter().any(|s| s.contains_point(p));
        let preview = TriGrid::new(&[&committed, &tail], 4.0);
        // 線の全体を描けている（マウスの点がどれも塗られている）
        let unpainted = path
            .iter()
            .filter(|&&p| !inside(app.view.screen_to_world(p)))
            .count();
        assert_eq!(unpainted, 0, "{name}：塗られていないマウスの点がある");
        // 表示が形からはみ出さない
        let sticking_out: Vec<Point> = preview
            .verts
            .iter()
            .copied()
            .filter(|&v| !inside(v) && !near_outline.dist(v, tol).is_finite())
            .collect();
        // 形が表示からはみ出さない
        let mut beyond: Vec<Point> = Vec::new();
        let mut looked = 0usize;
        for line in &outline {
            for i in 0..line.len() {
                let (a, b) = (line[i], line[(i + 1) % line.len()]);
                if (b - a).hypot() < 1e-9 {
                    continue;
                }
                let m = a.midpoint(b);
                let nrm = normalize_vec(left_normal(b - a)) * tol;
                let (p, q) = (m + nrm, m - nrm);
                let (pin, pout) = match (inside(p), inside(q)) {
                    (true, false) => (p, q),
                    (false, true) => (q, p),
                    _ => continue,
                };
                looked += 1;
                if !preview.contains(pin) || preview.contains(pout) {
                    beyond.push(m);
                }
            }
        }
        assert!(
            looked > 1000,
            "{name}：調べた輪郭の点が少なすぎる（{looked}）"
        );
        assert!(
            sticking_out.is_empty() && beyond.is_empty(),
            "{name}：描いている間の表示と離したあとの形が {STROKE_DEV_PX}px より離れた所がある（表示がはみ出す頂点 {}・最初 {:?}、形がはみ出す輪郭の点 {}/{looked}・最初 {:?}）",
            sticking_out.len(),
            sticking_out.first(),
            beyond.len(),
            beyond.first()
        );
    }
}

#[test]
fn test_sharp_v_and_l_corners_preserve_inner_and_outer_vertices() {
    let v_corners = [
        Point::new(100.0, 100.0),
        Point::new(125.0, 130.0),
        Point::new(150.0, 100.0),
    ];
    let mut v_pts = Vec::new();
    for w in v_corners.windows(2) {
        for k in 0..16 {
            v_pts.push(w[0].lerp(w[1], k as f64 / 16.0));
        }
    }
    v_pts.push(v_corners[2]);
    let r_v = 8.0;
    let v_raw = brush_line(&v_pts, r_v);
    let v_shapes = brush_line_shapes(&v_raw, Color32::BLACK);
    assert_eq!(v_shapes.len(), 1);
    let v_shape = &v_shapes[0];
    assert!(v_shape.contains_point(Point::new(125.0, 130.0 + r_v * 0.85)));
    assert!(!v_shape.contains_point(Point::new(125.0, 114.0)));

    let l_corners = [
        Point::new(100.0, 100.0),
        Point::new(100.0, 200.0),
        Point::new(200.0, 200.0),
    ];
    let mut l_pts = Vec::new();
    for w in l_corners.windows(2) {
        for k in 0..25 {
            l_pts.push(w[0].lerp(w[1], k as f64 / 25.0));
        }
    }
    l_pts.push(l_corners[2]);
    let r_l = 10.0;
    let l_raw = brush_line(&l_pts, r_l);
    let l_shapes = brush_line_shapes(&l_raw, Color32::BLACK);
    assert_eq!(l_shapes.len(), 1);
    let l_shape = &l_shapes[0];
    assert!(l_shape.contains_point(Point::new(91.5, 198.5)));
    assert!(!l_shape.contains_point(Point::new(88.0, 185.0)));
    assert!(!l_shape.contains_point(Point::new(112.5, 187.5)));
}

#[test]
fn test_egui_brush_drag_press_origin_and_right_click_ignored() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 1. ブラシドラッグ開始時 (H1): 押下位置 (300, 300) からドラッグ認識閾値を超えて (360, 300) へ動いたフレームで、
    //    描き始めの始点が欠落せずストロークに含まれること
    let press_pos = Pos2::new(300.0, 300.0);
    let drag_pos = Pos2::new(360.0, 300.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerMoved(press_pos)],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let expected_start_world = app.view.screen_to_world(press_pos);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(press_pos),
                egui::Event::PointerButton {
                    pos: press_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerMoved(drag_pos)],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerButton {
                pos: drag_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(scene_summary(&app).len, 1);
    assert!(
        scene_summary(&app).shapes[0].contains_point(expected_start_world),
        "ブラシのドラッグ開始位置 press_origin が欠落せず塗りに含まれていること"
    );

    // 2. 右ボタンドラッグ（PointerButton::Secondary）ではブラシ描画が発火しないこと (#2)
    let r_start = Pos2::new(450.0, 300.0);
    let r_end = Pos2::new(520.0, 300.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(r_start),
                egui::Event::PointerButton {
                    pos: r_start,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerMoved(r_end)],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerButton {
                pos: r_end,
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(
        scene_summary(&app).len,
        1,
        "右クリックドラッグではブラシストロークが描画されないこと"
    );
}

/// 描いている間の形（仮表示）は、離したあとの形と同じつなぎ目（`piece_ends`）で区切って作り、区切りどうしは、つなぎ目の
/// 平らに閉じた端の2点を共有して接する（重ならない）。区切るので、輪郭の自分との交わりは1つの区切りの中だけになる（レビュー
/// 41766f9 の N-2001：前は描いている間の形だけ区切らず、太いブラシで急に曲がると1回の三角形分けが 50ms を超えた）
#[test]
fn the_brush_preview_is_built_in_pieces_that_meet_at_joints() {
    let mut rng = fastrand::Rng::with_seed(0x0020_0100_2026);
    let mut split = 0;
    for _ in 0..200 {
        let zoom = [0.25, 1.0, 4.0][rng.usize(0..3)];
        let pts = random_hand_stroke(&mut rng, zoom);
        let radius = (0.5 + rng.f64() * 39.5) / zoom;
        let cl = vector_stroke::centerline(&pts, 1.0 / zoom);
        if cl.len() == 0 {
            continue;
        }
        let rings = vector_stroke::stroke_preview_outline(&cl, 0, cl.len() - 1, radius, zoom);
        assert!(!rings.is_empty());
        if rings.len() > 1 {
            split += 1;
        }
        let bits = |p: &Point| (p.x.to_bits(), p.y.to_bits());
        for w in rings.windows(2) {
            let shared = w[1].iter().filter(|p| w[0].iter().any(|q| bits(q) == bits(p))).count();
            assert!(shared >= 2, "隣の区切りが、つなぎ目の端の2点を共有していない（共有 {shared} 点）");
        }
    }
    assert!(split > 0, "区切られた仮表示が無い（試しの線がつなぎ目を作らない）");
}

/// 描いている間に、確定した所の形（確定するたびに足す）と、確定していない所の形（点が来るたびに作り直す）は、境目の点で
/// 平らな端どうしが接する（丸い端どうしが重ならない）。丸く閉じるのは、線の始まりと今の終わりだけ（利用者の指摘：前は境目
/// ごとに丸が重なり、半透明の色で描くと、濃い丸がスタンプのように見えた）
#[test]
fn brush_preview_pieces_meet_flat_at_their_boundaries() {
    let mut rng = fastrand::Rng::with_seed(0x0020_0200_2026);
    let near = |a: &Point, b: &Point| (*a - *b).hypot() < 1e-9;
    // 輪郭 `ring` の始まりの端が、輪郭 `prev` と平らな端（同じ直径の2点）で接するか：輪郭は、左の点を前へ・終わりの端・右の
    // 点を後ろへ・始めの端の順に並ぶので、始めの端が平らなら、最初と最後の点が直径の両端で、どちらも `prev` の点。丸い端
    // なら、最後に円弧の点が入る
    let meets_flat = |ring: &[Point], prev: &[Point]| {
        let shared = |p: &Point| prev.iter().any(|q| near(q, p));
        shared(&ring[0]) && shared(&ring[ring.len() - 1])
    };
    let mut boundaries = 0;
    for _ in 0..200 {
        let zoom = [0.25, 1.0, 4.0][rng.usize(0..3)];
        let pts = random_hand_stroke(&mut rng, zoom);
        let radius = (0.5 + rng.f64() * 39.5) / zoom;
        let mut b = vector_stroke::CenterlineBuilder::new(1.0 / zoom);
        // 前に確定した所の形の、最後の区切り
        let mut last_piece: Option<Vec<Point>> = None;
        for &p in &pts {
            let settled_before = b.settled();
            let fixed_before = b.push(p);
            let cl = b.current();
            if fixed_before.is_some() {
                let piece = vector_stroke::stroke_preview_outline(&cl, settled_before, b.settled(), radius, zoom);
                if let (Some(prev), Some(first)) = (&last_piece, piece.first()) {
                    let shared = first.iter().filter(|p| prev.iter().any(|q| near(q, p))).count();
                    assert!(shared >= 2, "確定した所どうしの境目が接していない（共有 {shared} 点）");
                    assert!(meets_flat(first, prev), "確定した所どうしの境目が、平らな端で接しない（丸い端どうしが重なる）");
                    boundaries += 1;
                }
                last_piece = piece.last().cloned();
            }
            if cl.len() > 0 && b.settled() > 0 {
                if let Some(prev) = &last_piece {
                    let tail = vector_stroke::stroke_preview_outline(&cl, b.settled(), cl.len() - 1, radius, zoom);
                    let shared = tail[0].iter().filter(|p| prev.iter().any(|q| near(q, p))).count();
                    assert!(shared >= 2, "確定した所と確定していない所の境目が接していない（共有 {shared} 点）");
                    assert!(meets_flat(&tail[0], prev), "確定した所と確定していない所の境目が、平らな端で接しない");
                }
            }
        }
    }
    assert!(boundaries > 0, "確定した所の境目が無い");
}

/// 線を1本動かしたあと、元の所に同じ色・太さでインクボトルをかけ直しても付く（新しい線の番号は、塗りの番号と同じく、
/// 書き戻しで新しく取る。前は形・線・側の番号から計算で振ったので、動かした線が持ち続ける元の所の番号と、かけ直した線の
/// 番号が同じになり、重なりの確かめが書き込みを断った：レビュー de92ceb の N-1002）
#[test]
fn ink_bottle_again_where_a_line_was_moved_away() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 4 };
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 200.0), stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    crate::canvas_tool::select_ops::apply_click_selection(&mut app, Point::new(200.0, 100.0), false, true);
    assert!(app.scene().iter().any(|s| s.line.is_some() && app.session.selection().contains_shape(s)), "上の辺の線を選んだ");
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(0.0, -50.0) }));
    assert!(apply(&mut app, Action::Deselect));
    assert!(app.session.floating().is_none());
    assert!(apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 200.0), stroke: ink }), "かけ直したインクボトルが断られた: {:?}", app.notices.iter().last().map(|t| &t.message));
    let ids: Vec<u64> = app.scene().iter().filter(|s| s.line.is_some()).map(|s| s.shape_id).collect();
    let mut uniq = ids.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), ids.len(), "線の番号が重なった");
}

#[test]
fn test_space_during_brush_drag_preserves_stroke_and_idle_space_drag_pans() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 1. ブラシのドラッグの途中に Space を押して離し、そのあとマウスを離すと、線が1本残り Undo が1つ積まれること
    let press_pos = Pos2::new(300.0, 300.0);
    let mid_pos = Pos2::new(360.0, 300.0);
    let end_pos = Pos2::new(420.0, 300.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerMoved(press_pos)],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(press_pos),
                egui::Event::PointerButton {
                    pos: press_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerMoved(mid_pos)],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(end_pos),
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: end_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(
        scene_summary(&app).len,
        1,
        "ブラシのドラッグ中に Space に触れても描いていた線が消えず1本残ること"
    );
    assert_eq!(
        app.history.undo_stack.len(),
        1,
        "ブラシのドラッグ完了で Undo が1つ積まれること"
    );

    // 2. ドラッグをしていないときに Space を押しながら左ボタンでドラッグすると、今までどおりパンになり、絵は描かれないこと
    let pan_before = app.view.pan;
    let pan_start = Pos2::new(500.0, 300.0);
    let pan_end = Pos2::new(580.0, 350.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(pan_start),
                egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: pan_start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::PointerMoved(pan_end)],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerButton {
                    pos: pan_end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        (app.view.pan - (pan_before + (pan_end - pan_start))).length() < 1e-3,
        "ドラッグしていないときの Space + 左ボタンドラッグでキャンバスがパンすること"
    );
    assert_eq!(
        scene_summary(&app).len,
        1,
        "Space パン中に余分な図形が描かれないこと"
    );
    assert_eq!(app.history.undo_stack.len(), 1);
}

#[test]
fn test_eyedropper_samples_interpolated_tween_position_on_locked_layer() {
    let mut tween_eye_app = Is2dApp::default();
    let tween_color = Color32::from_rgb(240, 120, 30);
    put_cel_shapes(
        &mut tween_eye_app,
        0,
        0,
        make_stroke(
            Point::new(90.0, 200.0),
            Point::new(110.0, 200.0),
            12.0,
            tween_color,
        ),
    );
    tween_eye_app.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut tween_eye_app).unwrap();
    doc_actions::timeline_toggle_tween(&mut tween_eye_app);
    doc_actions::select_timeline_cells(&mut tween_eye_app, 0, 4, 0, 4);
    doc_actions::timeline_insert_keyframe(&mut tween_eye_app, true);
    doc_actions::nudge_selected_shapes(&mut tween_eye_app, kurbo::Vec2::new(200.0, 0.0));
    doc_actions::set_layer_locked(&mut tween_eye_app, 0, true);
    doc_actions::scrub_ruler_to_frame(&mut tween_eye_app, 2);
    assert_eq!(
        tool_paint::topmost_visible_color_at(&tween_eye_app, Point::new(200.0, 200.0)),
        Some(tween_color),
    );
    assert_eq!(
        tool_paint::topmost_visible_color_at(&tween_eye_app, Point::new(100.0, 200.0)),
        None,
    );
}

/// レビュー 96fdc26 の N-6004：スポイトとバケツの「上に重なっているか」は、画面に描くのと同じ写しを上から順にたどる
/// （編集できるレイヤーは画面の図形、ロック中のレイヤーはセル画の結合メッシュの写し）。ロック中のレイヤーを文書から
/// 作り直す2つ目の道は無い
#[test]
fn eyedropper_and_bucket_see_locked_layers_through_the_drawn_copy() {
    let (red, blue, green) = (
        Color32::from_rgb(220, 50, 50),
        Color32::from_rgb(40, 90, 220),
        Color32::from_rgb(40, 180, 60),
    );
    let mut app = Is2dApp::default();
    app.tool.brush_color = red;
    commit_brush_line(
        &mut app,
        &brush_line(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 12.0),
    );
    doc_actions::timeline_add_layer(&mut app);
    app.tool.brush_color = blue;
    commit_brush_line(
        &mut app,
        &brush_line(&[Point::new(200.0, 100.0), Point::new(200.0, 300.0)], 12.0),
    );
    doc_actions::set_layer_locked(&mut app, 1, true);
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    // 重なる所は、上のロック中のレイヤーの青。赤だけの所は赤。何も無い所は無い
    let color_at = |app: &Is2dApp, p: Point| tool_paint::topmost_visible_color_at(app, p);
    let crossing = Point::new(200.0, 200.0);
    let red_only = Point::new(120.0, 200.0);
    assert_eq!(color_at(&app, crossing), Some(blue));
    assert_eq!(color_at(&app, red_only), Some(red));
    assert_eq!(color_at(&app, Point::new(20.0, 20.0)), None);
    // バケツは、上のロック中のレイヤーに覆われた所では塗らず、覆われていない所では塗る
    app.tool.brush_color = green;
    assert!(!doc_actions::bucket_fill_at(&mut app, crossing));
    assert_eq!(color_at(&app, red_only), Some(red));
    assert!(doc_actions::bucket_fill_at(&mut app, red_only));
    assert_eq!(color_at(&app, red_only), Some(green));
    assert_eq!(color_at(&app, crossing), Some(blue));
}

#[test]
fn test_bucket_fill_ignores_other_layer_walls_and_cluster_culling() {
    let red = Color32::from_rgb(220, 50, 50);
    // (a) バケツツール：アクティブレイヤーの円環の中を、ロックされた別レイヤーの線が横切っていても、その線は壁にしない
    //     （Adobe Animate と同じく、同じレイヤー・同じ階層の線だけが壁）。円環の中が全部塗られること
    let mut bucket_app = Is2dApp::default();
    let center = Point::new(200.0, 200.0);
    let ring_pts: Vec<Point> = (0..=48)
        .map(|i| {
            let a = (i as f64 / 48.0) * std::f64::consts::TAU;
            Point::new(center.x + 60.0 * a.cos(), center.y + 60.0 * a.sin())
        })
        .collect();
    let raw_ring = brush_line(&ring_pts, 6.0);
    bucket_app.tool.brush_color = Color32::BLACK;
    commit_brush_line(&mut bucket_app, &raw_ring);

    // Layer 2 を追加して円の中央 (x = 200) に縦の分割線を引き、Layer 2 をロック (🔒) する
    doc_actions::timeline_add_layer(&mut bucket_app);
    let raw_div = brush_line(&[Point::new(200.0, 120.0), Point::new(200.0, 280.0)], 6.0);
    commit_brush_line(&mut bucket_app, &raw_div);
    // (b) で使う壁（円環と分割線）
    let walls_before = scene_summary(&bucket_app).shapes;
    doc_actions::set_layer_locked(&mut bucket_app, 1, true);
    doc_actions::switch_to_layer_and_frame(&mut bucket_app, 0, 0);

    bucket_app.tool.brush_color = red;
    assert!(doc_actions::bucket_fill_at(
        &mut bucket_app,
        Point::new(170.0, 200.0)
    ));
    let summary_bucket = scene_summary(&bucket_app);
    let red_fill = summary_bucket
        .shapes
        .iter()
        .find(|s| s.color() == red)
        .expect("赤の塗りシェイプが生成されていること");
    assert!(red_fill.contains_point(Point::new(170.0, 200.0)));
    assert!(
        red_fill.contains_point(Point::new(230.0, 200.0)),
        "別のレイヤーの線は壁にならず、円環の中の反対側まで塗られること"
    );

    // (b) 大量の無関係な遠方シェイプが存在する場合も スイープライン＋Union-Find AABB 連結クラスタ事前カリングにより
    // クラスタ外クリックは即座に None を返し、対象閉領域内クリックは正確に抽出されること
    let mut many_walls = walls_before;
    for idx in 0..80 {
        let ox = 1500.0 + (idx % 10) as f64 * 120.0;
        let oy = 1500.0 + (idx / 10) as f64 * 120.0;
        many_walls.extend(make_stroke(
            Point::new(ox, oy),
            Point::new(ox + 50.0, oy + 20.0),
            6.0,
            Color32::BLACK,
        ));
    }
    assert!(
        find_enclosed_region_at(&many_walls, Point::new(900.0, 900.0), Color32::GREEN).is_none(),
        "どの連結AABBクラスタにも含まれない空白座標はオーバーレイ構築前に None となること"
    );
    let enclosed_right =
        find_enclosed_region_at(&many_walls, Point::new(230.0, 200.0), Color32::GREEN).expect(
            "無関係な遠方シェイプが多数存在しても対象クラスタ内の閉領域が正確に抽出されること",
        );
    assert!(enclosed_right.contains_point(Point::new(230.0, 200.0)));
    assert!(!enclosed_right.contains_point(Point::new(170.0, 200.0)));
}

#[test]
fn test_eraser_only_erases_active_layer_and_preserves_other_unlocked_layers() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 100, 220);

    // Layer 1 (index 0, id 1) に赤い横線を描画
    app.tool.tool = Tool::Brush;
    app.tool.brush_color = red;
    let raw_red = brush_line(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 14.0);
    commit_brush_line(&mut app, &raw_red);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 1);

    // Layer 2 (index 1, id 2) を追加して同じ座標に青い横線を描画（両レイヤーとも非ロック）
    doc_actions::timeline_add_layer(&mut app);
    assert_eq!(app.doc.root.active_layer, 1);
    app.tool.brush_color = blue;
    let raw_blue = brush_line(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 14.0);
    commit_brush_line(&mut app, &raw_blue);
    assert_eq!(app.doc.root.layers[1].keyframes[0].cel.shapes().len(), 1);

    // Layer 2 がアクティブな状態で消しゴムツールを実行 → Layer 2 の青い線のみが削られ、Layer 1 の赤い線は無傷であること
    app.tool.tool = Tool::Eraser;
    let raw_erase = brush_line(&[Point::new(200.0, 150.0), Point::new(200.0, 250.0)], 16.0);
    commit_brush_line(&mut app, &raw_erase);

    let layer1_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
    let layer2_shapes = &app.doc.root.layers[1].keyframes[0].cel.shapes();
    assert_eq!(
        layer1_shapes.len(),
        1,
        "非アクティブな Layer 1 のシェイプは分割・消去されず1個のまま無傷であること"
    );
    assert!(
        layer1_shapes[0].contains_point(Point::new(200.0, 200.0)),
        "非アクティブな Layer 1 の中央部分は消しゴムで削られていないこと"
    );
    assert_eq!(
        layer2_shapes.len(),
        2,
        "アクティブな Layer 2 のシェイプのみが中央で左右2つに分割消去されること"
    );
    assert!(!layer2_shapes
        .iter()
        .any(|s| s.contains_point(Point::new(200.0, 200.0))));
}

#[test]
fn test_noop_eraser_does_not_push_undo_and_active_layer_lock_blocks_paint_tools() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(140.0, 100.0),
            10.0,
            red,
        ),
    );
    doc_actions::timeline_add_layer(&mut app);
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);

    // 図形に当たらない空振り消しゴムで Undo が増えないこと (H7)
    let undo_before_noop_erase = app.history.undo_stack.len();
    app.tool.tool = Tool::Eraser;
    let miss_erase = brush_line(&[Point::new(700.0, 400.0), Point::new(720.0, 400.0)], 10.0);
    commit_brush_line(&mut app, &miss_erase);
    assert_eq!(
        app.history.undo_stack.len(),
        undo_before_noop_erase,
        "図形に当たらない空振り消しゴム操作では Undo 履歴が積まれないこと"
    );

    // アクティブレイヤーがロック中は消しゴム・バケツが別レイヤーへフォールバックせず拒否されること
    doc_actions::set_layer_locked(&mut app, 1, true);
    app.doc.root.active_layer = 1;
    app.tool.tool = Tool::Eraser;
    assert!(
        !app.is_current_tool_allowed(),
        "アクティブレイヤーがロック中は他レイヤーが非ロックでも消しゴムが禁止判定になること"
    );
    app.tool.tool = Tool::Bucket;
    assert!(
        !app.is_current_tool_allowed(),
        "アクティブレイヤーがロック中は他レイヤーが非ロックでもバケツが禁止判定になること"
    );
}

/// レビュー d91cd4c の N-3001：スポイトは、ロックの有無によらず、点でいちばん手前に見えているものを吸う。見えているのが
/// 線なら線の色と太さを線の設定に、塗りなら塗りの色に（前は、線かどうかをロック中のレイヤーを含まない画面の図形だけで
/// 決めたので、ロックした上の塗りの下の線を吸い、ロックしたレイヤーの線の色を塗りの色に入れていた）
#[test]
fn the_eyedropper_picks_what_is_seen_even_with_locked_layers() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let green = EdgeStroke { color: Color32::from_rgb(40, 180, 90), width: 6 };
    let red = Color32::from_rgb(235, 75, 75);
    let white = Color32::WHITE;
    let default_stroke = EdgeStroke { color: Color32::BLACK, width: 2 };
    // 下のレイヤー 0：緑の線（太さ 6）。上のレイヤー 1：線を覆う赤い塗り
    let setup = || {
        let mut app = Is2dApp::default();
        apply(&mut app, Action::SetTool { tool: Tool::Pencil });
        let line: Vec<Point> = (0..=100).map(|i| Point::new(100.0 + 3.0 * i as f64, 200.0)).collect();
        assert!(apply(&mut app, Action::PencilStroke { points: line, stroke: green }));
        apply(&mut app, Action::AddLayer);
        let top = app.tab().active_timeline().active_layer;
        put_cel_shapes(&mut app, top, 0, vec![map_overlay::rect_shape(150.0, 150.0, 300.0, 250.0, red)]);
        app.tool.brush_color = white;
        app.tool.stroke = default_stroke;
        (app, top)
    };
    let on_line_under_fill = Point::new(200.0, 200.0);
    let on_line_alone = Point::new(350.0, 200.0);
    for lock_top in [false, true] {
        let (mut app, top) = setup();
        app.tab_mut().with_active_timeline_mut(|tl, _| tl.layers[top].locked = lock_top);
        assert_eq!(tool_paint::pick_color_at(&mut app, on_line_under_fill), Some(red), "上をロック {lock_top}");
        assert_eq!(app.tool.brush_color, red, "上をロック {lock_top}：見えている赤い塗りを吸わない");
        assert_eq!(app.tool.stroke, default_stroke, "上をロック {lock_top}：隠れている線を吸った");
    }
    // 線のレイヤーをロックする：線の上なら、線の色と太さを線の設定に吸う（塗りの色は変えない）。線がグループやシンボルの
    // 中にあっても同じ（ロック中のレイヤーの図形は、描くのと同じ写しからたどる）
    for wrap in [None, Some(Action::Group), Some(Action::ConvertToSymbol)] {
        let (mut app, top) = setup();
        if let Some(wrap) = wrap.clone() {
            // 下のレイヤーの線だけを選んで包む（上のレイヤーは、包む間だけロック）
            app.tab_mut().with_active_timeline_mut(|tl, _| tl.layers[top].locked = true);
            doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
            apply(&mut app, Action::SetTool { tool: Tool::Select });
            assert!(apply(&mut app, Action::SelectAll));
            assert!(apply(&mut app, wrap));
            apply(&mut app, Action::Deselect);
            app.tab_mut().with_active_timeline_mut(|tl, _| tl.layers[top].locked = false);
        }
        app.tab_mut().with_active_timeline_mut(|tl, _| tl.layers[0].locked = true);
        assert_eq!(tool_paint::pick_color_at(&mut app, on_line_alone), Some(green.color), "{wrap:?}");
        assert_eq!(app.tool.stroke, green, "{wrap:?}：ロックしたレイヤーの線の色と太さを吸わない");
        assert_eq!(app.tool.brush_color, white, "{wrap:?}：ロックしたレイヤーの線の色を塗りの色に入れた");
    }
}

#[test]
fn test_g1_3_locked_layer_eyedropper_nested_20x5_fast() {
    use std::time::Instant;

    let mut app = Is2dApp::default();
    let target_color = Color32::from_rgb(240, 120, 40);
    // 空の葉シンボル S1 を起点に、1段あたり20個のインスタンスを5段重ねたシンボル階層（20^5 = 320万経路）を作成
    let mut prev_sym_id = app.doc.register_new_symbol(Vec::new());
    let mut next_gid = 100_u64;
    for _ in 0..4 {
        let sym_id = app.doc.register_new_symbol(Vec::new());
        let instances: Vec<timeline::SymbolInstance> = (0..20)
            .map(|i| {
                let gid = next_gid;
                next_gid += 1;
                timeline::SymbolInstance {
                    binding: symbol_binding::SymbolInstanceBinding::new(
                        gid,
                        prev_sym_id,
                        kurbo::Affine::translate((i as f64 * 0.5, 0.0)),
                    ),
                    layer_id: 1,
                    parent_scope: Vec::new(),
                    order: 0,
                }
            })
            .collect();
        write_symbol(&mut app.doc, sym_id, |tl| {
            tl.layers[0]
                .keyframe_mut(0)
                .cel
                .set_shapes_and_instances(Vec::new(), instances)
        });
        prev_sym_id = sym_id;
    }

    // ルートのロックレイヤーに色付き図形1個と、最上位シンボル20個（合計 20^5 = 320万経路）を配置
    let mut bg_shape = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(140.0, 100.0),
        8.0,
        target_color,
    );
    bg_shape[0].layer_id = 1;
    let root_instances: Vec<timeline::SymbolInstance> = (0..20)
        .map(|i| {
            let gid = next_gid;
            next_gid += 1;
            timeline::SymbolInstance {
                binding: symbol_binding::SymbolInstanceBinding::new(
                    gid,
                    prev_sym_id,
                    kurbo::Affine::translate((i as f64 * 0.5, 0.0)),
                ),
                layer_id: 1,
                parent_scope: Vec::new(),
                order: 1,
            }
        })
        .collect();
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_shapes_and_instances(bg_shape, root_instances);
    app.doc.root.layers[0].locked = true;
    app.refresh_scene_base();

    let t0 = Instant::now();
    let picked = tool_paint::pick_color_at(&mut app, Point::new(110.0, 100.0));
    let elapsed = t0.elapsed();
    println!(
        "[G1-3] locked layer 20^5 nested symbol eyedropper = {:.3} ms",
        elapsed.as_secs_f64() * 1000.0
    );
    assert_eq!(picked, Some(target_color));
}

/// シンボルの中身を変えたすぐあと（描き直す前）でも、スポイト・バケツがロック中のレイヤーを見る道（`Tab::cached_cel_seen_at`）
/// は新しい中身の色を拾う（シンボルの中身のメモを使う入口が、毎回ライブラリが変わったかを確かめる。直す前は描画の始めに
/// しか確かめなかったので、古い色を拾った：約束の棚卸し 5616fcc の B の D1・E1）
#[test]
fn colour_picking_sees_symbol_edits_before_the_next_redraw() {
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(350.0, 350.0), additive: false }));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let pt = Point::new(200.0, 200.0);
    let seen = |app: &Is2dApp| app.tab().cached_cel_seen_at(0, 0, pt).map(|(c, _)| c);
    assert_eq!(seen(&app), Some(Color32::RED));
    let gid = app.doc.root.layers[0].keyframes[0].cel.instances()[0].binding.instance_gid;
    assert!(apply(&mut app, Action::EnterSymbolTimeline { instance_gid: gid }));
    assert!(apply(&mut app, Action::BucketFill { pt, color: Color32::BLUE }));
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    assert_eq!(seen(&app), Some(Color32::BLUE), "シンボルの中身を変えたのに、古い色を拾った");
}

#[test]
fn test_wavy_and_asymmetric_small_loop_strokes_preserve_hand_drawn_contours() {
    let r_wave = 8.0;
    let wave_pts: Vec<Point> = (0..=80)
        .map(|i| {
            let t = i as f64 / 80.0;
            let x = 100.0 + 200.0 * t;
            let y = 200.0 + 4.5 * (t * 4.0 * std::f64::consts::PI).sin();
            Point::new(x, y)
        })
        .collect();
    let wave_raw = brush_line(&wave_pts, r_wave);
    let wave_shapes = brush_line_shapes(&wave_raw, Color32::BLACK);
    assert_eq!(wave_shapes.len(), 1);
    let wave_shape = &wave_shapes[0];
    assert!(wave_shape.contains_point(Point::new(125.0, 211.0)));
    assert!(!wave_shape.contains_point(Point::new(125.0, 194.5)));
    assert!(wave_shape.contains_point(Point::new(175.0, 189.0)));
    assert!(!wave_shape.contains_point(Point::new(175.0, 205.5)));

    let small_center = Point::new(300.0, 300.0);
    let r_small_brush = 6.0;
    let small_pts: Vec<Point> = (0..=64)
        .map(|i| {
            let theta = (i as f64 / 62.0) * std::f64::consts::TAU;
            let rx = 17.0 + 2.2 * (theta * 2.0).sin();
            let ry = 12.0 - 1.5 * (theta * 3.0).cos();
            Point::new(
                small_center.x + rx * theta.cos(),
                small_center.y + ry * theta.sin(),
            )
        })
        .collect();
    let small_raw = brush_line(&small_pts, r_small_brush);
    let small_shapes = brush_line_shapes(&small_raw, Color32::BLACK);
    assert_eq!(small_shapes.len(), 1);
    let small_shape = &small_shapes[0];
    assert_eq!(small_shape.holes().len(), 1);
    assert!(small_shape.contains_point(Point::new(322.2, 300.0)));
    assert!(!small_shape.contains_point(Point::new(300.0, 319.2)));
    assert!(!small_shape.contains_point(Point::new(309.8, 300.0)));
}

#[test]
fn test_wavy_stroke_fully_covering_small_dot_erases_cuts_or_merges_it() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    // 太さ 10px の線を細かく波打たせて、半径 2px の点を完全に覆う（N-1001）
    let wavy = |y: f64| -> Vec<Point> {
        (0..=200)
            .map(|i| {
                let x = 100.0 + 3.0 * i as f64;
                Point::new(x, y + 1.5 * (x * 0.1).sin())
            })
            .collect()
    };
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 60, 220);
    let paint = |points: Vec<Point>, color: Color32, radius: f32| Action::PaintStroke {
        points,
        radius,
        color,
    };
    let move_onto_dot = |color: Color32| {
        vec![
            paint(wavy(400.0), color, 10.0),
            Action::SelectSingleAt {
                pt: wavy(400.0)[67],
            },
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(0.0, -200.0),
            },
            Action::Deselect,
        ]
    };
    for x in [330.0, 340.0, 350.0] {
        let cases = [
            (
                "消しゴムで覆うと点が消える",
                vec![Action::EraseStroke {
                    points: wavy(200.0),
                    radius: 10.0,
                }],
                vec![],
            ),
            (
                "違う色の線で覆うと点が削られる",
                vec![paint(wavy(200.0), blue, 10.0)],
                vec![blue],
            ),
            (
                "同じ色の線で覆うと点が線と結合される",
                vec![paint(wavy(200.0), red, 10.0)],
                vec![red],
            ),
            (
                "違う色の線を動かして覆うと点が削られる",
                move_onto_dot(blue),
                vec![blue],
            ),
            (
                "同じ色の線を動かして覆うと点が線と結合される",
                move_onto_dot(red),
                vec![red],
            ),
        ];
        for (label, actions, expected) in cases {
            let mut app = Is2dApp::default();
            assert!(apply(&mut app, paint(vec![Point::new(x, 200.0)], red, 2.0)));
            run(&mut app, &actions);
            let colors: Vec<Color32> = app.doc.root.layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .map(|s| s.color())
                .collect();
            assert_eq!(colors, expected, "{label} (x={x})");
        }
    }
}

#[test]
fn random_hand_strokes_follow_their_centerlines() {
    let mut rng = fastrand::Rng::with_seed(0x0052_6000_2026);
    let zooms = [0.25, 0.5, 1.0, 2.0, 4.0];
    let mut worst = (0.0f64, 0usize);
    let mut bad: Vec<String> = Vec::new();
    for k in 0..2000 {
        let zoom = zooms[rng.usize(0..zooms.len())];
        let pts = random_hand_stroke(&mut rng, zoom);
        let r_screen = 0.5 + 40.0 * rng.f64().powi(2);
        let radius = (r_screen / zoom).clamp(0.5, 70.0);
        let c = check_stroke_follows_centerline(&pts, radius, zoom);
        if c.worst_px > worst.0 {
            worst = (c.worst_px, k);
        }
        if c.worst_px > STROKE_DEV_PX || c.outside_box + c.uncovered + c.missing > 0 {
            bad.push(format!(
                "線 {k}（{} 点・半径 {radius:.2}・倍率 {zoom}）：{c:?}",
                pts.len()
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{} 本で形が中心線からずれた（いちばん大きいずれ {:.3}px・線 {}）：{:?}",
        bad.len(),
        worst.0,
        worst.1,
        &bad[..bad.len().min(5)]
    );
    let colors = [Color32::RED, Color32::BLUE, Color32::from_rgb(20, 160, 60)];
    for group in 0..10 {
        let mut app = Is2dApp::default();
        for k in 0..20 {
            let zoom = zooms[rng.usize(0..zooms.len())];
            let pts = random_hand_stroke(&mut rng, zoom);
            let radius = ((0.5 + 30.0 * rng.f64().powi(2)) / zoom).clamp(0.5, 70.0);
            if rng.u32(0..4) == 0 {
                app.tool.tool = Tool::Eraser;
            } else {
                app.tool.tool = Tool::Brush;
                app.tool.brush_color = colors[rng.usize(0..colors.len())];
            }
            commit_brush_line(&mut app, &brush_line_at_zoom(&pts, radius, zoom));
            assert!(
                layer_map_is_valid(&app),
                "組 {group} の {k} 本目（{} 点・半径 {radius:.2}・倍率 {zoom}）で正しい地図でなくなった",
                pts.len()
            );
        }
    }
}

#[test]
fn test_fine_brush_and_zoom_invariant_stroke_smoothing() {
    let mut app = Is2dApp::default();
    app.view.set_zoom(8.0);
    app.tool.brush_radius = 0.5;
    let fine_raw = brush_line_at_zoom(
        &[Point::new(320.0, 240.0), Point::new(323.0, 240.0)],
        app.tool.brush_radius as f64,
        app.view.zoom() as f64,
    );
    commit_brush_line(&mut app, &fine_raw);
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 1);
    assert!(s.shapes[0].contains_point(Point::new(321.5, 240.0)));

    let screen_pts: Vec<Point> = (0..=40)
        .map(|i| {
            let t = i as f64 / 40.0;
            Point::new(
                100.0 + 200.0 * t,
                200.0 + 40.0 * (t * std::f64::consts::PI).sin(),
            )
        })
        .collect();
    let raw_1x = brush_line_at_zoom(&screen_pts, 8.0, 1.0);
    let shapes_1x = brush_line_shapes_at_zoom(&raw_1x, Color32::BLACK, 1.0);

    let world_pts_4x: Vec<Point> = screen_pts
        .iter()
        .map(|p| Point::new(p.x / 4.0, p.y / 4.0))
        .collect();
    let raw_4x = brush_line_at_zoom(&world_pts_4x, 2.0, 4.0);
    let shapes_4x = brush_line_shapes_at_zoom(&raw_4x, Color32::BLACK, 4.0);

    assert_eq!(shapes_1x.len(), 1);
    assert_eq!(shapes_4x.len(), 1);
    assert_eq!(
        shapes_1x[0].exterior().segments.len(),
        shapes_4x[0].exterior().segments.len(),
    );
    let area_ratio = shapes_1x[0].exterior().signed_area() / shapes_4x[0].exterior().signed_area();
    assert!((area_ratio - 16.0).abs() < 0.05);
}

/// M4-5 (N-1001): 3色・半径 0.5〜25・長さ1〜60点のブラシと消しゴムをランダムに300回行い、
/// 毎回、塗られた面積が理論上の範囲（許容 1%）に入ること
#[test]
fn test_m4_5_random_brush_and_eraser_area_bounds_300_steps() {
    let colors = [
        Color32::from_rgb(220, 60, 60),
        Color32::from_rgb(60, 180, 90),
        Color32::from_rgb(60, 120, 220),
    ];
    let shape_area = |s: &FillShape| -> f64 {
        (s.exterior().signed_area().abs()
            - s.holes().iter().map(|h| h.signed_area().abs()).sum::<f64>())
        .max(0.0)
    };

    let mut seed = 0x1234_5678_9abc_def0_u64;
    let mut next_f64 = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((seed >> 32) as u32 as f64) / (u32::MAX as f64)
    };

    let mut canvas: Vec<FillShape> = Vec::new();
    for step in 0..300 {
        let is_erase = step > 0 && next_f64() < 0.28;
        let radius = 0.5 + next_f64() * 24.5;
        let len = 1 + (next_f64() * 60.0) as usize;
        let color = colors[(next_f64() * 3.0) as usize % 3];
        let mut x = 100.0 + next_f64() * 600.0;
        let mut y = 80.0 + next_f64() * 440.0;
        let mut ang = next_f64() * std::f64::consts::TAU;
        let mut pts = Vec::with_capacity(len);
        for _ in 0..len {
            pts.push(Point::new(x, y));
            ang += (next_f64() - 0.5) * 0.7;
            let step_d = 2.0 + next_f64() * 8.0;
            x = (x + ang.cos() * step_d).clamp(50.0, 750.0);
            y = (y + ang.sin() * step_d).clamp(40.0, 560.0);
        }
        let raw = brush_line(&pts, radius);
        let stroke: Vec<FillShape> = brush_line_shapes(&raw, color)
            .into_iter()
            .map(|s| s.with_scope(1, &[]))
            .collect();
        if stroke.is_empty() {
            continue;
        }
        let stroke_area: f64 = stroke.iter().map(&shape_area).sum();
        let prev_area: f64 = canvas.iter().map(&shape_area).sum();

        if is_erase {
            erase_from_canvas(&mut canvas, &stroke, 1, &[]);
        } else {
            merge_shapes_into_canvas(&mut canvas, stroke);
        }

        let after_area: f64 = canvas.iter().map(&shape_area).sum();
        let (min_exp, max_exp) = if is_erase {
            ((prev_area - stroke_area).max(0.0), prev_area)
        } else {
            (prev_area.max(stroke_area), prev_area + stroke_area)
        };
        let tol = max_exp.max(stroke_area) * 0.01 + 1.5;
        assert!(
            after_area >= min_exp - tol && after_area <= max_exp + tol,
            "step={step} is_erase={is_erase} radius={radius:.2} len={len}: prev={prev_area:.2} stroke={stroke_area:.2} after={after_area:.2} expected=[{min_exp:.2}, {max_exp:.2}] tol={tol:.2}"
        );
    }
}
