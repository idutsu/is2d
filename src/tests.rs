use super::*;
use crate::canvas_tool::HoverTarget;
use kurbo::{ParamCurve, ParamCurveDeriv, ParamCurveNearest, QuadBez, Vec2 as KVec2};
use vector_boolean::{merge_shapes_into_canvas, resolve_deformed_shape};
use vector_engine::{left_normal, normalize_vec};
use vector_stroke::{
    build_smoothed_stroke_shapes, build_smoothed_stroke_shapes_at_zoom, generate_brush_outline,
    generate_brush_outline_at_zoom,
};

fn make_stroke(p0: Point, p1: Point, radius: f32, color: Color32) -> Vec<FillShape> {
    let pts: Vec<Point> = (0..=12).map(|i| p0.lerp(p1, i as f64 / 12.0)).collect();
    let raw = generate_brush_outline(&pts, radius as f64);
    build_smoothed_stroke_shapes(&raw, color)
}

#[test]
fn test_same_color_union_and_diff_split_with_i_overlay() {
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 220);

    let mut canvas: Vec<FillShape> = Vec::new();

    // 1. 横方向の赤いブラシストローク
    let stroke1 = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        20.0,
        red,
    );
    merge_shapes_into_canvas(&mut canvas, stroke1);
    assert_eq!(canvas.len(), 1);

    // 2. 交差する同色（赤）の縦ストローク -> Union で1つのシェイプに融合されること
    let stroke2 = make_stroke(
        Point::new(200.0, 120.0),
        Point::new(200.0, 280.0),
        20.0,
        red,
    );
    merge_shapes_into_canvas(&mut canvas, stroke2);
    assert_eq!(canvas.len(), 1);
    assert!(canvas[0].contains_point(Point::new(200.0, 200.0)));
    assert!(canvas[0].contains_point(Point::new(120.0, 200.0)));
    assert!(canvas[0].contains_point(Point::new(200.0, 140.0)));

    // 3. 異色（青）のストロークで中央を横断 -> 赤シェイプが Difference で分断され、青シェイプが追加されること
    let stroke3 = make_stroke(
        Point::new(80.0, 200.0),
        Point::new(320.0, 200.0),
        26.0,
        blue,
    );
    merge_shapes_into_canvas(&mut canvas, stroke3);

    let red_count = canvas.iter().filter(|s| s.color == red).count();
    let blue_count = canvas.iter().filter(|s| s.color == blue).count();
    assert_eq!(red_count, 2, "赤シェイプが上下2つに分断されること");
    assert_eq!(blue_count, 1, "青シェイプが1つ追加されること");
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

    let raw = generate_brush_outline(&raw_pts, r);
    let shapes = build_smoothed_stroke_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];

    assert!(
        shape.exterior.segments.len() <= 8,
        "単純なストロークのアンカー数が多すぎます: {}",
        shape.exterior.segments.len()
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

    let raw = generate_brush_outline(&pts, r);
    let shapes = build_smoothed_stroke_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];

    assert!(
        shape.exterior.segments.len() <= 10,
        "アーチ曲線のアンカー数が多すぎます: {}",
        shape.exterior.segments.len()
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

    let raw = generate_brush_outline(&pts, r);
    let shapes = build_smoothed_stroke_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let rect_shape = &shapes[0];

    assert_eq!(rect_shape.holes.len(), 1, "四角形の中央に穴が1つあること");
    assert!(rect_shape.holes[0].segments.len() <= 6);
    assert!(rect_shape.exterior.segments.len() <= 12);

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

    let raw = generate_brush_outline(&pts, brush_r);
    let shapes = build_smoothed_stroke_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let circle_shape = &shapes[0];
    assert_eq!(circle_shape.holes.len(), 1);

    let ext_len = circle_shape.exterior.segments.len();
    let hole_len = circle_shape.holes[0].segments.len();
    assert!((6..=10).contains(&ext_len), "ext_len = {}", ext_len);
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

    let h_raw = generate_brush_outline(&h_pts, r);
    let h_shapes = build_smoothed_stroke_shapes(&h_raw, Color32::BLACK);
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

    let l_raw = generate_brush_outline(&loop_pts, r);
    let l_shapes = build_smoothed_stroke_shapes(&l_raw, Color32::BLACK);
    assert_eq!(l_shapes.len(), 1);
    let l_shape = &l_shapes[0];
    assert_eq!(l_shape.holes.len(), 1);

    for k in 0..8 {
        let ang = (k as f64 / 8.0) * std::f64::consts::TAU;
        let dir = KVec2::new(ang.cos(), ang.sin());
        assert!(l_shape.contains_point(loop_center + dir * (loop_r - r * 0.72)));
        assert!(l_shape.contains_point(loop_center + dir * (loop_r + r * 0.72)));
        assert!(!l_shape.contains_point(loop_center + dir * (loop_r - r * 1.30)));
        assert!(!l_shape.contains_point(loop_center + dir * (loop_r + r * 1.30)));
    }
}

#[test]
fn test_pull_edge_across_circle_splits_into_two_shapes() {
    let center = Point::new(200.0, 200.0);
    let r = 50.0;
    let raw = generate_brush_outline(&[center], r);
    let shapes = build_smoothed_stroke_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);

    let mut circle = shapes[0].clone();
    // 上端 (200, 150) に最も近いエッジを探す
    let top_probe = Point::new(200.0, 150.0);
    let (seg_idx, grab_t, orig_seg) = circle
        .exterior
        .segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let n = seg.nearest(top_probe, 0.01);
            (i, n.t, *seg, n.distance_sq)
        })
        .min_by(|a, b| a.3.partial_cmp(&b.3).unwrap())
        .map(|(i, t, s, _)| (i, t.clamp(0.18, 0.82), s))
        .unwrap();

    // 円の上端エッジを円内部を通って下端 (200, 250) の向こう側 (200, 275) まで突っ切るようにドラッグ
    crate::tool_select::apply_pull_edge_to_shape(
        &mut circle,
        (0, seg_idx),
        grab_t,
        orig_seg,
        Point::new(200.0, 275.0),
    );

    let split_shapes = resolve_deformed_shape(&circle);
    assert_eq!(
            split_shapes.len(),
            2,
            "円を向こう側の円周へ突っ切るようにドラッグすると2つのシェイプに分割されること (actual = {})",
            split_shapes.len()
        );

    // 左右の半円領域は塗られており、突っ切った中央 (200, 200) と突き抜けた先 (200, 265) は空であること
    let any_contains = |pt: Point| split_shapes.iter().any(|s| s.contains_point(pt));
    assert!(
        any_contains(Point::new(165.0, 200.0)),
        "左側の分割片が存在すること"
    );
    assert!(
        any_contains(Point::new(235.0, 200.0)),
        "右側の分割片が存在すること"
    );
    assert!(
        !any_contains(Point::new(200.0, 200.0)),
        "突っ切った中央は削られていること"
    );
    assert!(
        !any_contains(Point::new(200.0, 265.0)),
        "向こう側の円周を突き抜けた裏返りループが消えていること"
    );

    // 向こう側の円周ギリギリ (200, 249) でドロップした場合も punch_opposite により2分割されること
    let mut circle_on_edge = shapes[0].clone();
    crate::tool_select::apply_pull_edge_to_shape(
        &mut circle_on_edge,
        (0, seg_idx),
        grab_t,
        orig_seg,
        Point::new(200.0, 249.0),
    );
    let split_on_edge = resolve_deformed_shape(&circle_on_edge);
    assert_eq!(
        split_on_edge.len(),
        2,
        "向こう側の円周上でドロップした場合も2つのシェイプに分割されること"
    );
}

#[test]
fn test_add_and_delete_anchor_on_contour() {
    let center = Point::new(200.0, 200.0);
    let raw = generate_brush_outline(&[center], 40.0);
    let mut shapes = build_smoothed_stroke_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);

    let orig_n = shapes[0].exterior.segments.len();
    let orig_mid = shapes[0].exterior.segments[0].eval(0.5);

    // 1. アンカー追加（De Casteljau 分割）で形状が変わらずアンカー数が +1 されること
    assert!(shapes[0].exterior.insert_anchor_at(0, 0.5));
    shapes[0].rebuild_cache();
    assert_eq!(shapes[0].exterior.segments.len(), orig_n + 1);
    assert!(
        (shapes[0].exterior.segments[1].p0 - orig_mid).hypot() < 1e-6,
        "追加されたアンカーが元のベジェ曲線上の正確な座標にあること"
    );

    // 2. 追加したアンカー（インデックス 1）を削除すると元のアンカー数に戻り、点Aと点Cを結ぶ直線セグメントになること
    let pt_a = shapes[0].exterior.segments[0].p0;
    let pt_c = shapes[0].exterior.segments[1].p2;
    assert!(shapes[0].exterior.delete_anchor_at(1));
    shapes[0].rebuild_cache();
    assert_eq!(shapes[0].exterior.segments.len(), orig_n);
    let merged_seg = shapes[0].exterior.segments[0];
    assert!(
        (merged_seg.p0 - pt_a).hypot() < 1e-6 && (merged_seg.p2 - pt_c).hypot() < 1e-6,
        "アンカー削除後の始点と終点が点A・点Cと一致すること"
    );
    assert!(
        (merged_seg.p1 - pt_a.lerp(pt_c, 0.5)).hypot() < 1e-6,
        "点Bを削除したとき、山なりを残さず点Aと点Cを結ぶ直線（制御点が中点）になること"
    );
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
    let raw_ring = generate_brush_outline(&ring_pts, 8.0);
    app.tool.brush_color = black;
    tool_paint::commit_raw_outline(&mut app, &raw_ring);
    assert_eq!(app.session.shapes.len(), 1);
    assert!(
        !app.session.shapes[0].contains_point(center),
        "初期状態では中央は空白（穴）であること"
    );

    // 1. 閉じきっていない外側の背景 (20, 20) をバケツでクリックしても塗られないこと
    app.tool.brush_color = red;
    assert!(
        !tool_paint::bucket_fill_at(&mut app, Point::new(20.0, 20.0)),
        "開いた背景は塗りつぶされないこと"
    );

    // 2. 円環の内側の空白 (200, 200) を赤でバケツ塗り -> 赤い塗りシェイプが生成されること
    assert!(
        tool_paint::bucket_fill_at(&mut app, center),
        "閉領域がバケツで塗られること"
    );
    assert_eq!(app.session.shapes.len(), 2);
    let inner_shape = app.session.shapes.iter().find(|s| s.color == red).unwrap();
    assert!(inner_shape.contains_point(center));

    // 3. 塗られた赤い領域を青でバケツクリック -> 青にリカラーされること
    app.tool.brush_color = blue;
    assert!(
        tool_paint::bucket_fill_at(&mut app, center),
        "既存の塗りシェイプが別色に塗り替わること"
    );
    assert_eq!(app.session.shapes.len(), 2);
    assert!(app
        .session
        .shapes
        .iter()
        .any(|s| s.color == blue && s.contains_point(center)));

    // 4. 内側の領域を周囲の線と同じ黒でバケツクリック -> 外周の黒リングと Union されて穴なしの1つの黒シェイプになること
    app.tool.brush_color = black;
    assert!(tool_paint::bucket_fill_at(&mut app, center));
    assert_eq!(
        app.session.shapes.len(),
        1,
        "同色でバケツ塗りすると周囲のシェイプと1つに結合されること"
    );
    assert!(
        app.session.shapes[0].holes.is_empty(),
        "穴が埋まって消えていること"
    );
    assert!(app.session.shapes[0].contains_point(center));
}

#[test]
fn test_marquee_select_partial_slice_and_shift_multi_select() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    app.tool.brush_color = red;

    // x: 100..300, y: 200 の太い横長ストローク（半径 25px）を描画
    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        25.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, stroke);
    assert_eq!(app.session.shapes.len(), 1);

    // 1. 左端 (x: 70..140, y: 150..250) を矩形選択 -> 左端だけスライスされて選択状態になること
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(140.0, 250.0),
        false,
    );
    assert_eq!(
        app.session.shapes.len(),
        2,
        "部分矩形選択で内外2つにスライスされること"
    );
    assert_eq!(
        app.session.selected_indices().len(),
        1,
        "スライスされた内側のみが選択されていること"
    );
    assert!(
        app.session.shapes[app.session.selected_indices()[0]].contains_point(Point::new(110.0, 200.0)),
        "選択された断片が左端領域を含んでいること"
    );

    // 2. 移動も削除もせずに選択解除（clear_selection）すると、無劣化で元の1つのシェイプに戻ること
    tool_select::clear_selection(&mut app);
    assert_eq!(
        app.session.shapes.len(),
        1,
        "未編集で選択解除すると元の1つのシェイプに復元されること"
    );
    assert!(app.session.selection.is_empty());

    // 3. 左端 (x: 70..140) を矩形選択し、続いて Shift キー押下（additive = true）で右端 (x: 260..330) も連続矩形選択
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(140.0, 250.0),
        false,
    );
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(260.0, 150.0),
        Point::new(330.0, 250.0),
        true,
    );
    assert_eq!(
        app.session.shapes.len(),
        3,
        "左右両端のスライスと中央の非選択領域の計3つに分割されること"
    );
    assert_eq!(
        app.session.selected_indices().len(),
        2,
        "Shift 連続矩形選択で左右2つの領域が同時に選択されていること"
    );

    // 4. 選択された左右両端を削除すると、中央 (x: 200, y: 200) のみが残ること
    assert!(tool_select::delete_selected_shapes(&mut app));
    assert_eq!(
        app.session.shapes.len(),
        1,
        "選択された左右両端が削除され中央の1つだけ残ること"
    );
    assert!(app.session.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(!app.session.shapes[0].contains_point(Point::new(110.0, 200.0)));
    assert!(!app.session.shapes[0].contains_point(Point::new(290.0, 200.0)));

    // 5. 1回の Undo でスライス前の元の1つのシェイプに完全復帰すること
    app.undo();
    assert_eq!(app.session.shapes.len(), 1);
    assert!(app.session.shapes[0].contains_point(Point::new(110.0, 200.0)));
    assert!(app.session.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(app.session.shapes[0].contains_point(Point::new(290.0, 200.0)));
}

#[test]
fn test_keep_selection_on_tool_switch_and_bucket_fill_selected_region_only() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    app.tool.brush_color = red;

    // x: 100..300, y: 200 の赤いストロークを描画
    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        25.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, stroke);
    assert_eq!(app.session.shapes.len(), 1);

    // 1. 選択ツールで左半分 (x: 70..200) を矩形選択
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Select });
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 1);

    // 2. バケツツールに切り替えても選択状態が解除されないこと
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Bucket });
    assert_eq!(
        app.session.selected_indices().len(),
        1,
        "バケツツールに切り替えても選択が維持されること"
    );

    // 3. 選択されていない右半分 (260, 200) をバケツでクリックしても塗られないこと
    app.tool.brush_color = blue;
    assert!(
        !tool_paint::bucket_fill_at(&mut app, Point::new(260.0, 200.0)),
        "選択外領域はバケツで塗られないこと"
    );

    // 4. 選択されている左半分 (130, 200) をバケツでクリックすると、選択部分だけが青に塗られること
    assert!(
        tool_paint::bucket_fill_at(&mut app, Point::new(130.0, 200.0)),
        "選択領域内をクリックするとバケツで色が流し込まれること"
    );
    assert!(
        app.session
            .shapes
            .iter()
            .any(|s| s.color == blue && s.contains_point(Point::new(130.0, 200.0))),
        "選択された左半分が青になっていること"
    );
    assert!(
        app.session
            .shapes
            .iter()
            .any(|s| s.color == red && s.contains_point(Point::new(260.0, 200.0))),
        "非選択の右半分は赤のままであること"
    );
}

#[test]
fn test_free_transform_rotate_scale_and_flip() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    app.tool.brush_color = red;

    // 中心 (200, 200) の横長シェイプ (x: 100..300, y: 200, 半径 20px) を描画
    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        20.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, stroke);
    assert_eq!(app.session.shapes.len(), 1);
    app.session.set_selected_indices(vec![0]);
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Transform });

    // 1. +90° 回転 -> 横長だったシェイプが中心 (200, 200) を軸に縦長 (x: 200, y: 100..300) になること
    assert!(tool_transform::rotate_selected_shapes(
        &mut app,
        std::f64::consts::FRAC_PI_2
    ));
    assert!(
        app.session.shapes[0].contains_point(Point::new(200.0, 110.0))
            && app.session.shapes[0].contains_point(Point::new(200.0, 290.0)),
        "+90度回転で縦長シェイプになること"
    );
    assert!(
        !app.session.shapes[0].contains_point(Point::new(110.0, 200.0)),
        "元の左右端にはシェイプが存在しないこと"
    );

    // 2. 縮小 (0.5倍) -> 上下端 (200, 110) は範囲外になり、(200, 155) 付近に縮小されること
    assert!(tool_transform::scale_selected_shapes(&mut app, 0.5, 0.5));
    assert!(!app.session.shapes[0].contains_point(Point::new(200.0, 110.0)));
    assert!(app.session.shapes[0].contains_point(Point::new(200.0, 155.0)));

    // 3. カスタムピボット点 (200, 200) に対して、左側の点 (150, 200) にあるシェイプを左右反転 -> (250, 200) に移り、ワインディングも正常に保たれること
    tool_select::clear_selection(&mut app);
    app.session.shapes.clear();
    let left_piece = make_stroke(
        Point::new(130.0, 200.0),
        Point::new(170.0, 200.0),
        18.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, left_piece);
    app.session.set_selected_indices(vec![0]);
    app.session.transform_pivot = Some(Point::new(200.0, 200.0));

    assert!(tool_transform::flip_selected_shapes(&mut app, true, false));
    assert!(
        !app.session.shapes[0].contains_point(Point::new(150.0, 200.0)),
        "反転前の位置には存在しないこと"
    );
    assert!(
        app.session.shapes[0].contains_point(Point::new(250.0, 200.0)),
        "ピボット (200, 200) を基準に左右対称の (250, 200) へ反転移動していること"
    );
    assert!(
        app.session.shapes[0].exterior.signed_area() > 0.0,
        "反転時にベジェ輪郭の向き（ワインディング）が自動補正され正の面積を保つこと"
    );
}

#[test]
fn test_eyedropper_picks_topmost_shape_color() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(65, 195, 105);

    // 背面 (Layer 1) に赤シェイプ (x: 100..260, y: 200) を配置し、
    // 前面グループとして同一座標 (200, 200) に重なる青シェイプ (x: 180..320, y: 200) を重ねる
    let s_red = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(260.0, 200.0),
        20.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, s_red);

    let mut s_blue = make_stroke(
        Point::new(180.0, 200.0),
        Point::new(320.0, 200.0),
        20.0,
        blue,
    );
    for sh in &mut s_blue {
        sh.group_path = vec![1];
    }
    app.session.shapes.extend(s_blue);

    app.tool.brush_color = green;
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Eyedropper });

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
    assert!(app.session.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(app.session.shapes[1].contains_point(Point::new(200.0, 200.0)));
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(200.0, 200.0)),
        Some(blue),
        "同一座標で2つの図形が重なっている場合、最前面の図形色が優先されること"
    );
    assert_eq!(app.tool.brush_color, blue);
}

#[test]
fn test_group_selection_bucket_protection_and_non_destructive_overlap() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(65, 195, 105);

    let face = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        30.0,
        red,
    );
    let eye_a = make_stroke(
        Point::new(90.0, 100.0),
        Point::new(110.0, 100.0),
        15.0,
        blue,
    );
    let eye_b = make_stroke(
        Point::new(135.0, 100.0),
        Point::new(155.0, 100.0),
        12.0,
        green,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, face);
    merge_shapes_into_canvas(&mut app.session.shapes, eye_a);
    merge_shapes_into_canvas(&mut app.session.shapes, eye_b);
    assert_eq!(app.session.shapes.len(), 3);

    app.session.set_selected_indices(vec![1, 2]);
    let eye_gid =
        tool_select::group_selected_shapes(&mut app).expect("グループ化が成功すること");
    assert_eq!(app.session.shapes[1].group_path, vec![eye_gid]);
    assert_eq!(app.session.shapes[2].group_path, vec![eye_gid]);

    // 1つクリックするだけでグループ全体の2シェイプが同時選択されること
    tool_select::clear_selection(&mut app);
    assert!(tool_select::select_single_shape_at(
        &mut app,
        Point::new(100.0, 100.0)
    ));
    assert_eq!(app.session.selected_indices().len(), 2);

    // グループ選択状態でバケツツールを使っても無効であること
    app.tool.brush_color = red;
    assert!(!tool_paint::bucket_fill_at(&mut app, Point::new(100.0, 100.0)));
    assert_eq!(app.session.shapes[1].color, blue);
    assert_eq!(app.session.shapes[2].color, green);

    // グループを赤い顔シェイプの上へ移動して選択解除しても下の生シェイプを切り抜かないこと
    let snap = app.session.shapes.clone();
    app.session.translate_selected(KVec2::new(0.0, 100.0));
    tool_select::commit_moved_selection(&mut app, snap);
    tool_select::clear_selection(&mut app);
    assert_eq!(app.session.shapes.len(), 3);
    assert!(app.session.shapes[0].contains_point(Point::new(100.0, 200.0)));
}

#[test]
fn test_nested_grouping_edit_in_place_and_single_level_ungroup() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(65, 195, 105);

    let face = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 30.0, red);
    let eye_a = make_stroke(Point::new(90.0, 200.0), Point::new(110.0, 200.0), 15.0, blue);
    let eye_b = make_stroke(Point::new(135.0, 200.0), Point::new(155.0, 200.0), 12.0, green);
    merge_shapes_into_canvas(&mut app.session.shapes, face);
    app.session.shapes.extend(eye_a);
    app.session.shapes.extend(eye_b);

    app.session.set_selected_indices(vec![1, 2]);
    let eye_gid = tool_select::group_selected_shapes(&mut app).unwrap();
    app.session.set_selected_indices(vec![0, 1, 2]);
    let head_gid = tool_select::group_selected_shapes(&mut app).unwrap();
    assert_eq!(app.session.shapes[0].group_path, vec![head_gid]);
    assert_eq!(app.session.shapes[1].group_path, vec![head_gid, eye_gid]);
    assert_eq!(app.session.shapes[2].group_path, vec![head_gid, eye_gid]);

    assert!(tool_select::enter_group_at(&mut app, Point::new(200.0, 200.0)));
    assert_eq!(app.session.editing_group_path, vec![head_gid]);
    assert!(app.session.shapes[0].is_raw_at_scope(&app.session.editing_group_path));
    assert_eq!(
        app.session.shapes[1].active_group_id_at_scope(&app.session.editing_group_path),
        Some(eye_gid)
    );

    assert!(tool_select::enter_group_at(&mut app, Point::new(100.0, 200.0)));
    assert_eq!(app.session.editing_group_path, vec![head_gid, eye_gid]);
    assert!(app.session.shapes[1].is_raw_at_scope(&app.session.editing_group_path));

    assert!(tool_select::exit_current_group(&mut app));
    assert_eq!(app.session.editing_group_path, vec![head_gid]);
    assert!(tool_select::exit_current_group(&mut app));
    assert!(app.session.editing_group_path.is_empty());
    assert_eq!(app.session.selected_indices().len(), 3);

    assert!(tool_select::ungroup_selected_shapes(&mut app));
    assert!(app.session.shapes[0].group_path.is_empty());
    assert_eq!(app.session.shapes[1].group_path, vec![eye_gid]);
    assert_eq!(app.session.shapes[2].group_path, vec![eye_gid]);
}

#[test]
fn test_zoom_around_cursor_preserves_world_point_and_clamps_bounds() {
    let mut app = Is2dApp::default();
    let cursor = Pos2::new(320.0, 240.0);

    let world_before = app.view.screen_to_world(cursor);
    app.view.zoom_around_screen_pos(cursor, 4.0);
    app.view.zoom_around_screen_pos(cursor, 2.0);
    assert!((app.view.zoom - 8.0).abs() < 1e-5);
    let world_after = app.view.screen_to_world(cursor);
    assert!(
        (world_before - world_after).hypot() < 1e-4,
        "カーソル中心ズームの前後でカーソル直下のワールド座標が一致すること"
    );
    let screen_roundtrip = app.view.world_to_screen(world_after);
    assert!((screen_roundtrip - cursor).length() < 1e-3);

    // 上限 (MAX_VIEW_ZOOM = 16.0) と下限 (MIN_VIEW_ZOOM = 0.15) でクランプされ、かつカーソル下の座標が保たれること
    app.view.zoom_around_screen_pos(cursor, 100.0);
    assert_eq!(app.view.zoom, crate::vector_engine::MAX_VIEW_ZOOM);
    assert!((app.view.screen_to_world(cursor) - world_before).hypot() < 1e-3);

    app.view.zoom_around_screen_pos(cursor, 0.001);
    assert_eq!(app.view.zoom, crate::vector_engine::MIN_VIEW_ZOOM);
    assert!((app.view.screen_to_world(cursor) - world_before).hypot() < 1e-3);

    app.view.reset_view();
    assert_eq!(app.view.zoom, 1.0);
    assert_eq!(app.view.pan, Vec2::ZERO);
}

#[test]
fn test_fine_brush_and_zoom_invariant_stroke_smoothing() {
    let mut app = Is2dApp::default();
    app.view.zoom = 8.0;
    app.tool.brush_radius = 0.5;
    let fine_raw = generate_brush_outline_at_zoom(
        &[Point::new(320.0, 240.0), Point::new(323.0, 240.0)],
        app.tool.brush_radius as f64,
        app.view.zoom as f64,
    );
    tool_paint::commit_raw_outline(&mut app, &fine_raw);
    assert_eq!(app.session.shapes.len(), 1);
    assert!(app.session.shapes[0].contains_point(Point::new(321.5, 240.0)));

    let screen_pts: Vec<Point> = (0..=40)
        .map(|i| {
            let t = i as f64 / 40.0;
            Point::new(
                100.0 + 200.0 * t,
                200.0 + 40.0 * (t * std::f64::consts::PI).sin(),
            )
        })
        .collect();
    let raw_1x = generate_brush_outline_at_zoom(&screen_pts, 8.0, 1.0);
    let shapes_1x = build_smoothed_stroke_shapes_at_zoom(&raw_1x, Color32::BLACK, 1.0);

    let world_pts_4x: Vec<Point> = screen_pts
        .iter()
        .map(|p| Point::new(p.x / 4.0, p.y / 4.0))
        .collect();
    let raw_4x = generate_brush_outline_at_zoom(&world_pts_4x, 2.0, 4.0);
    let shapes_4x = build_smoothed_stroke_shapes_at_zoom(&raw_4x, Color32::BLACK, 4.0);

    assert_eq!(shapes_1x.len(), 1);
    assert_eq!(shapes_4x.len(), 1);
    assert_eq!(
        shapes_1x[0].exterior.segments.len(),
        shapes_4x[0].exterior.segments.len(),
    );
    let area_ratio = shapes_1x[0].exterior.signed_area() / shapes_4x[0].exterior.signed_area();
    assert!((area_ratio - 16.0).abs() < 0.05);
}

#[test]
fn test_cut_and_paste_in_place_seamlessly_remerges_raw_shape() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);

    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        25.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, stroke);
    assert_eq!(app.session.shapes.len(), 1);

    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert!(tool_select::cut_selected_shapes(&mut app));
    assert_eq!(app.session.shapes.len(), 1);
    assert!(!app.session.shapes[0].contains_point(Point::new(120.0, 200.0)));
    assert!(app.session.shapes[0].contains_point(Point::new(260.0, 200.0)));

    assert!(tool_select::paste_clipboard(&mut app, true));
    assert_eq!(app.session.selected_indices().len(), 1);
    assert!(
        app.session.shapes[app.session.selected_indices()[0]].contains_point(Point::new(120.0, 200.0))
    );
    tool_select::clear_selection(&mut app);
    assert_eq!(app.session.shapes.len(), 1);
    assert!(app.session.shapes[0].contains_point(Point::new(120.0, 200.0)));
    assert!(app.session.shapes[0].contains_point(Point::new(260.0, 200.0)));
}

#[test]
fn test_copy_paste_group_assigns_new_gid_and_alt_drag_symbol_undo() {
    let mut app = Is2dApp::default();
    let blue = Color32::from_rgb(55, 135, 245);

    let g_part = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(140.0, 100.0),
        15.0,
        blue,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, g_part);
    app.session.set_selected_indices(vec![0]);
    let orig_gid = tool_select::group_selected_shapes(&mut app).unwrap();

    assert!(tool_select::copy_selected_shapes(&mut app));
    // 高ズーム時でもワールド座標上の一定オフセット (+16, +16) で貼り付けられること
    app.view.zoom = 4.0;
    assert!(tool_select::paste_clipboard(&mut app, false));
    assert_eq!(app.session.shapes.len(), 2);
    let pasted_idx = app.session.selected_indices()[0];
    let pasted_gid = app.session.shapes[pasted_idx].group_path[0];
    assert_ne!(pasted_gid, orig_gid);
    assert!(
        app.session.shapes[pasted_idx].contains_point(Point::new(120.0 + 16.0, 100.0 + 16.0)),
        "Cmd+V ではズーム倍率によらずワールド座標 (+16, +16) ずれた位置に貼り付けられること"
    );

    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    let orig_inst_gid = tool_transform::selected_single_symbol_info(&app)
        .unwrap()
        .0
        .instance_gid;
    let count_before = app.session.shapes.len();

    let undo_snapshot = tool_select::duplicate_selected_shapes_for_alt_drag(&mut app);
    assert_eq!(app.session.shapes.len(), count_before + 1);
    app.session.translate_selected(KVec2::new(60.0, 40.0));
    tool_select::finish_move_selection_drag(&mut app, KVec2::new(60.0, 40.0), undo_snapshot);

    let (dup_binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
    assert_eq!(dup_binding.symbol_id, sym_id);
    assert_ne!(dup_binding.instance_gid, orig_inst_gid);
    assert!(
        app.session.shapes[app.session.selected_indices()[0]]
            .contains_point(Point::new(120.0 + 16.0 + 60.0, 100.0 + 16.0 + 40.0)),
    );
    app.undo();
    assert_eq!(app.session.shapes.len(), count_before);
}

#[test]
fn test_timeline_layer_keyframe_insertion_and_cel_lookup() {
    let mut timeline = Timeline::default();
    assert_eq!(timeline.fps, 24);
    assert_eq!(timeline.total_frames, 1);
    assert_eq!(timeline.layers.len(), 1);
    assert_eq!(timeline.layers[0].frame_count, 1);
    assert_eq!(timeline.current_frame, 0);

    let red = Color32::from_rgb(235, 75, 75);
    let stroke = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    if let Some(cel) = timeline.layers[0].cel_at_mut(0) {
        merge_shapes_into_canvas(cel.shapes_mut(), stroke);
    }
    assert_eq!(timeline.active_cel_shapes().len(), 1);

    timeline.layers[0].insert_keyframe(4, false);
    assert_eq!(timeline.layers[0].frame_count, 5);
    assert_eq!(timeline.layers[0].keyframe_index_at(3), Some(0));
    assert_eq!(timeline.layers[0].keyframe_index_at(4), Some(1));
    assert_eq!(timeline.layers[0].cel_at(2).unwrap().shapes.len(), 1);
    assert!(timeline.layers[0].cel_at(4).unwrap().is_empty());

    timeline.layers[0].insert_keyframe(3, true);
    assert_eq!(timeline.layers[0].cel_at(3).unwrap().shapes.len(), 1);
}

#[test]
fn test_timeline_f5_f6_f7_and_delete_keyframe_undo_redo() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let s0 = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(150.0, 100.0),
        10.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, s0);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert_eq!(app.doc.root.current_frame, 1);
    assert_eq!(app.session.shapes.len(), 1);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 2);
    assert!(app.session.shapes.is_empty());

    let prev_total = app.doc.root.total_frames;
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.total_frames, prev_total + 1);
    assert!(timeline_ui::timeline_remove_frame(&mut app));
    assert_eq!(app.doc.root.total_frames, prev_total);

    timeline_ui::select_timeline_cells(&mut app, 0, 9, 0, 9);
    assert_eq!(app.doc.root.current_frame, 9);
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.layers[0].frame_count, 10);
    assert_eq!(app.doc.root.total_frames, 10);
    app.undo();
    assert_eq!(app.doc.root.total_frames, prev_total);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);

    assert!(timeline_ui::timeline_delete_keyframe(&mut app));
    assert_eq!(app.session.shapes.len(), 1);
    app.undo();
    assert!(app.session.shapes.is_empty());
    app.redo();
    assert_eq!(app.session.shapes.len(), 1);
}

#[test]
fn test_multilayer_isolation_cross_layer_marquee_and_lock_protection() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let s0 = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(150.0, 100.0),
        10.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.session.shapes, s0);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.doc.root.layers.len(), 2);
    assert_eq!(app.doc.root.active_layer, 1);
    assert!(app.doc.root.active_cel_shapes().is_empty());
    assert_eq!(app.session.shapes.len(), 1);
    app.tool.brush_color = blue;
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(125.0, 100.0)),
        Some(red)
    );

    app.tool.brush_color = blue;
    tool_paint::commit_raw_outline(
        &mut app,
        &[
            Point::new(120.0, 70.0),
            Point::new(120.0, 130.0),
            Point::new(135.0, 130.0),
            Point::new(135.0, 70.0),
        ],
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert_eq!(app.doc.root.layers[0].cel_at(0).unwrap().shapes.len(), 1);
    assert_eq!(app.doc.root.layers[1].cel_at(0).unwrap().shapes.len(), 1);
    assert_eq!(app.session.shapes.len(), 2);

    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(200.0, 200.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 2);
    let undo_snap = app.session.shapes.clone();
    app.session.translate_selected(KVec2::new(30.0, 0.0));
    tool_select::commit_moved_selection(&mut app, undo_snap);
    tool_select::clear_selection(&mut app);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert!(app.doc.root.layers[0].cel_at(0).unwrap().shapes[0]
        .contains_point(Point::new(155.0, 100.0)));
    assert!(app.doc.root.layers[1].cel_at(0).unwrap().shapes[0]
        .contains_point(Point::new(155.0, 100.0)));

    timeline_ui::set_layer_locked(&mut app, 0, true);
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(250.0, 200.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 1);
    tool_select::clear_selection(&mut app);
    timeline_ui::set_layer_locked(&mut app, 0, false);
    assert_eq!(app.session.shapes.len(), 2);
}

#[test]
fn test_timeline_layer_reorder_move_and_delete() {
    let mut app = Is2dApp::default();
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.doc.root.layers.len(), 3);
    assert_eq!(
        app.doc.root
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 1", "Layer 2", "Layer 3"]
    );

    assert!(timeline_ui::timeline_reorder_layer(&mut app, 2, 0));
    assert_eq!(app.doc.root.active_layer, 0);
    assert_eq!(
        app.doc.root
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 3", "Layer 1", "Layer 2"]
    );
    app.undo();
    assert_eq!(
        app.doc.root
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 1", "Layer 2", "Layer 3"]
    );
    app.redo();
    assert_eq!(
        app.doc.root
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 3", "Layer 1", "Layer 2"]
    );

    assert!(timeline_ui::timeline_move_active_layer(&mut app, true));
    assert_eq!(app.doc.root.active_layer, 1);
    assert!(timeline_ui::timeline_delete_active_layer(&mut app));
    assert_eq!(app.doc.root.layers.len(), 2);
    assert!(timeline_ui::timeline_delete_active_layer(&mut app));
    assert_eq!(app.doc.root.layers.len(), 1);
}

#[test]
fn test_timeline_cell_selection_sync_with_canvas_selection() {
    let mut app = Is2dApp::default();
    let blue = Color32::from_rgb(55, 135, 245);
    tool_paint::commit_raw_outline(
        &mut app,
        &[
            Point::new(40.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(80.0, 80.0),
            Point::new(40.0, 80.0),
        ],
    );
    timeline_ui::timeline_add_layer(&mut app);
    app.tool.brush_color = blue;
    tool_paint::commit_raw_outline(
        &mut app,
        &[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ],
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    let base_len = app.doc.root.layers[0].frame_count;

    timeline_ui::select_timeline_cells(&mut app, 1, 0, 1, 0);
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.layers[0].frame_count, base_len);
    assert_eq!(app.doc.root.layers[1].frame_count, base_len + 1);

    timeline_ui::scrub_ruler_to_frame(&mut app, 0);
    assert!(app.doc.root.cell_selection.is_none());
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.layers[0].frame_count, base_len + 1);
    assert_eq!(app.doc.root.layers[1].frame_count, base_len + 2);

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert_eq!(app.session.selected_indices().len(), 1);
    assert_eq!(
        app.session.shapes[app.session.selected_indices()[0]].layer_id,
        app.doc.root.layers[0].id
    );

    tool_select::clear_selection(&mut app);
    timeline_ui::scrub_ruler_to_frame(&mut app, 0);
    assert!(tool_select::select_single_shape_at(
        &mut app,
        Point::new(160.0, 60.0)
    ));
    assert_eq!(app.doc.root.active_layer, 1);
    assert_eq!(
        app.doc.root
            .cell_selection
            .map(|s| (s.min_layer, s.max_layer)),
        Some((1, 1))
    );

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 1, 0);
    assert_eq!(app.session.selected_indices().len(), 2);
}

#[test]
fn test_timeline_move_and_copy_frames_and_shift_f5_keyframe_removal() {
    let mut app = Is2dApp::default();
    let blue = Color32::from_rgb(55, 135, 245);
    tool_paint::commit_raw_outline(
        &mut app,
        &[
            Point::new(40.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(80.0, 80.0),
            Point::new(40.0, 80.0),
        ],
    );
    timeline_ui::timeline_add_layer(&mut app);
    app.tool.brush_color = blue;
    tool_paint::commit_raw_outline(
        &mut app,
        &[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ],
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::scrub_ruler_to_frame(&mut app, 0);
    timeline_ui::timeline_extend_frame(&mut app);

    let sel_l1_f0 = timeline::TimelineCellSelection::single(1, 0);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut app, sel_l1_f0, 0, 3, true
    ));
    assert!(!app.doc.root.layers[1].cel_at(0).unwrap().is_empty());
    assert_eq!(app.doc.root.layers[1].keyframe_index_at(3), Some(1));
    assert!(!app.doc.root.layers[1].cel_at(3).unwrap().is_empty());

    let sel_l1_f3 = timeline::TimelineCellSelection::single(1, 3);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut app, sel_l1_f3, 0, 2, false
    ));
    assert!(app.doc.root.layers[1].cel_at(3).unwrap().is_empty());
    assert_eq!(app.doc.root.layers[1].keyframes.last().unwrap().start_frame, 5);
    assert!(!app.doc.root.layers[1].cel_at(5).unwrap().is_empty());

    let sel_l0_f0 = timeline::TimelineCellSelection::single(0, 0);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut app, sel_l0_f0, 0, 2, false
    ));
    assert!(app.doc.root.layers[0].cel_at(0).unwrap().is_empty());
    assert!(!app.doc.root.layers[0].cel_at(1).unwrap().is_empty());
    assert!(!app.doc.root.layers[0].cel_at(2).unwrap().is_empty());

    app.doc.root.layers[1].insert_keyframe(3, false);
    app.doc.root.layers[1].insert_keyframe(4, false);
    let kf_count_before = app.doc.root.layers[1].keyframes.len();
    timeline_ui::select_timeline_cells(&mut app, 1, 0, 1, 0);
    assert!(timeline_ui::timeline_remove_frame(&mut app));
    assert_eq!(app.doc.root.layers[1].keyframes.len(), kf_count_before);

    timeline_ui::select_timeline_cells(&mut app, 1, 2, 1, 2);
    assert!(timeline_ui::timeline_remove_frame(&mut app));
    assert_eq!(app.doc.root.layers[1].keyframes.len(), kf_count_before - 1);

    let cel = app.doc.root.layers[1].cel_at(0).unwrap();
    let m1 = cel.cached_screen_mesh(1.0, Vec2::ZERO, false);
    let m2 = cel.cached_screen_mesh(1.0, Vec2::ZERO, false);
    assert!(!m1.is_empty());
    assert!(std::sync::Arc::ptr_eq(&m1, &m2));
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    app.doc.root.current_frame = 0;
    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing);
    assert_eq!(app.session.shapes.len(), 1);
}

#[test]
fn test_adobe_animate_click_selects_edge_and_selected_shape_disables_deformation() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Select });

    // 細い 2px ブラシストロークを描画（ほぼ全域が境界線の吸着範囲内になる細い線）
    let thin_stroke = make_stroke(Point::new(100.0, 200.0), Point::new(220.0, 200.0), 2.0, red);
    merge_shapes_into_canvas(&mut app.session.shapes, thin_stroke);
    assert_eq!(app.session.shapes.len(), 1);

    let vertex_pt = app.session.shapes[0].exterior.segments[0].p0;
    let edge_pt = app.session.shapes[0].exterior.segments[0].eval(0.5);

    // 1. 未選択状態では、頂点や境界線にホバーすると Vertex / Edge（変形対象）になること
    assert!(matches!(
        tool_select::compute_hover_target(
            app.scene(),
            &app.session,
            vertex_pt,
            app.view.zoom,
            app.tool.select_sub_mode
        ),
        HoverTarget::Vertex { shape_idx: 0, .. }
    ));
    assert!(matches!(
        tool_select::compute_hover_target(
            app.scene(),
            &app.session,
            edge_pt,
            app.view.zoom,
            app.tool.select_sub_mode
        ),
        HoverTarget::Edge { shape_idx: 0, .. } | HoverTarget::Vertex { shape_idx: 0, .. }
    ));

    // 2. 細い線の境界線や頂点の上でクリック（select_single_shape_at）した際にも、確実に選択状態になること
    assert!(
        tool_select::select_single_shape_at(&mut app, vertex_pt),
        "頂点や境界線上をクリックしてもシェイプが選択状態になること"
    );
    assert_eq!(app.session.selected_indices(), vec![0]);

    // 3. 選択状態（網点表示）になった後は、頂点上・境界線上・少し外側のマージンのどこを掴んでも
    //    変形（Vertex / Edge）は無効化され、常に移動（HoverTarget::Fill）になること
    assert_eq!(
        tool_select::compute_hover_target(
            app.scene(),
            &app.session,
            vertex_pt,
            app.view.zoom,
            app.tool.select_sub_mode
        ),
        HoverTarget::Fill { shape_idx: 0 },
        "選択状態のシェイプの頂点上では変形ではなく移動（Fill）になること"
    );
    assert_eq!(
        tool_select::compute_hover_target(
            app.scene(),
            &app.session,
            edge_pt,
            app.view.zoom,
            app.tool.select_sub_mode
        ),
        HoverTarget::Fill { shape_idx: 0 },
        "選択状態のシェイプの境界線上では変形ではなく移動（Fill）になること"
    );
    assert_eq!(
        tool_select::compute_hover_target(
            app.scene(),
            &app.session,
            Point::new(160.0, 204.5),
            app.view.zoom,
            app.tool.select_sub_mode
        ),
        HoverTarget::Fill { shape_idx: 0 },
        "選択状態の細い線の外周マージン内でも移動（Fill）として掴めること"
    );
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

    let raw = generate_brush_outline(&pts, brush_r);
    let shapes = build_smoothed_stroke_shapes(&raw, Color32::BLACK);
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];

    for (c_idx, contour) in std::iter::once(&shape.exterior)
        .chain(shape.holes.iter())
        .enumerate()
    {
        let k = contour.segments.len();
        for j in 0..k {
            let prev = (j + k - 1) % k;
            let t_in = normalize_vec(contour.segments[prev].p2 - contour.segments[prev].p1);
            let t_out = normalize_vec(contour.segments[j].p1 - contour.segments[j].p0);
            let dot = t_in.dot(t_out);
            assert!(
                dot > 0.9999,
                "contour {c_idx} アンカー {j}/{k} で前後のベジェ曲線の接線が G1 連続に揃っていること (dot = {dot})"
            );
        }
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
    let v_raw = generate_brush_outline(&v_pts, r_v);
    let v_shapes = build_smoothed_stroke_shapes(&v_raw, Color32::BLACK);
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
    let l_raw = generate_brush_outline(&l_pts, r_l);
    let l_shapes = build_smoothed_stroke_shapes(&l_raw, Color32::BLACK);
    assert_eq!(l_shapes.len(), 1);
    let l_shape = &l_shapes[0];
    assert!(l_shape.contains_point(Point::new(91.5, 198.5)));
    assert!(!l_shape.contains_point(Point::new(88.0, 185.0)));
    assert!(!l_shape.contains_point(Point::new(112.5, 187.5)));
}

#[test]
fn test_self_intersecting_tip_protrusion_preserves_concave_notch() {
    for tip_protrusion in [5.0_f64, 6.0, 9.0, 12.0, 15.0, 19.0, 24.0, 30.0] {
        let r_a = 9.0;
        let cross_pt = Point::new(200.0, 130.0);
        let dir_down_left = normalize_vec(KVec2::new(-0.32, 0.95));
        let tip_start = cross_pt - dir_down_left * tip_protrusion;
        let a_fixed = [
            tip_start,
            cross_pt,
            Point::new(190.0, 160.0),
            Point::new(172.0, 198.0),
            Point::new(145.0, 210.0),
            Point::new(125.0, 190.0),
            Point::new(128.0, 152.0),
            Point::new(155.0, 130.0),
            Point::new(182.0, 126.0),
            cross_pt,
            Point::new(218.0, 140.0),
            Point::new(230.0, 165.0),
            Point::new(225.0, 205.0),
        ];
        let mut a_pts = Vec::new();
        for w in a_fixed.windows(2) {
            let seg_len = (w[1] - w[0]).hypot();
            let steps = ((seg_len / 2.5).ceil() as usize).max(4);
            for k in 0..steps {
                a_pts.push(w[0].lerp(w[1], k as f64 / steps as f64));
            }
        }
        a_pts.push(*a_fixed.last().unwrap());

        let a_raw = generate_brush_outline(&a_pts, r_a);
        let a_shapes = build_smoothed_stroke_shapes(&a_raw, Color32::BLACK);
        assert_eq!(a_shapes.len(), 1);
        let a_shape = &a_shapes[0];

        let tip_apex = tip_start - dir_down_left * (r_a * 0.80);
        assert!(a_shape.contains_point(tip_apex));

        let valley_outside_probe = Point::new(cross_pt.x + r_a + 3.2, cross_pt.y - 5.5);
        assert!(!a_shape.contains_point(valley_outside_probe));
    }
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
    let wave_raw = generate_brush_outline(&wave_pts, r_wave);
    let wave_shapes = build_smoothed_stroke_shapes(&wave_raw, Color32::BLACK);
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
    let small_raw = generate_brush_outline(&small_pts, r_small_brush);
    let small_shapes = build_smoothed_stroke_shapes(&small_raw, Color32::BLACK);
    assert_eq!(small_shapes.len(), 1);
    let small_shape = &small_shapes[0];
    assert_eq!(small_shape.holes.len(), 1);
    assert!(small_shape.contains_point(Point::new(322.2, 300.0)));
    assert!(!small_shape.contains_point(Point::new(300.0, 319.2)));
    assert!(!small_shape.contains_point(Point::new(309.8, 300.0)));
}

#[test]
fn test_symbol_conversion_library_placement_and_inplace_sync() {
    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(200.0, 150.0),
        16.0,
        Color32::from_rgb(230, 80, 80),
    );
    app.session.set_selected_indices(vec![0]);

    let sym_id =
        tool_transform::convert_selection_to_symbol(&mut app).expect("シンボル変換が成功すること");
    assert_eq!(app.doc.library.len(), 1);
    assert_eq!(app.doc.library[0].id, sym_id);
    assert_eq!(app.doc.library[0].name, "Symbol 1");
    assert!(tool_transform::selected_single_symbol_info(&app).is_some());

    assert!(tool_transform::place_symbol_instance_from_library(
        &mut app, sym_id
    ));
    assert_eq!(app.session.shapes.len(), 2);
    assert_ne!(
        app.session.shapes[0].symbol_instance.unwrap().instance_gid,
        app.session.shapes[1].symbol_instance.unwrap().instance_gid
    );

    assert!(tool_select::enter_group_at(
        &mut app,
        Point::new(150.0, 150.0)
    ));
    assert_eq!(app.session.edit_stack.len(), 1);
    assert_eq!(app.doc.root.total_frames, 1);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert_eq!(app.doc.root.total_frames, 2);
    assert_eq!(app.doc.root.current_frame, 1);

    let hat = make_stroke(
        Point::new(120.0, 80.0),
        Point::new(180.0, 80.0),
        10.0,
        Color32::from_rgb(50, 120, 240),
    )
    .remove(0);
    app.session.shapes.push(hat);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    assert!(tool_select::exit_current_group(&mut app));
    assert!(app.session.edit_stack.is_empty());
    assert_eq!(app.doc.library[0].timeline.total_frames, 2);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.total_frames, 3);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.session.shapes.len(), 2);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(app.session.shapes.len(), 4);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(app.session.shapes.len(), 2);
}

#[test]
fn test_classic_tween_requires_single_symbol_instance() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    assert!(!tool_transform::timeline_toggle_tween(&mut app));

    app.session.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(200.0, 150.0),
        16.0,
        Color32::from_rgb(230, 80, 80),
    );
    assert!(!tool_transform::timeline_toggle_tween(&mut app));

    app.session.set_selected_indices(vec![0]);
    assert!(tool_select::group_selected_shapes(&mut app).is_some());
    assert!(!tool_transform::timeline_toggle_tween(&mut app));
    assert!(tool_select::ungroup_selected_shapes(&mut app));

    app.session.set_selected_indices(vec![0]);
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert!(tool_transform::timeline_toggle_tween(&mut app));
    assert!(app.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete,
    );

    assert!(tool_transform::timeline_toggle_tween(&mut app));
    assert!(!app.doc.root.layers[0].keyframes[0].tween);

    assert!(tool_transform::place_symbol_instance_from_library(
        &mut app, sym_id
    ));
    assert!(!tool_transform::timeline_toggle_tween(&mut app));
    assert!(tool_select::delete_selected_shapes(&mut app));

    let raw_extra = make_stroke(
        Point::new(300.0, 300.0),
        Point::new(340.0, 300.0),
        8.0,
        Color32::BLACK,
    )
    .remove(0);
    app.session.shapes.push(raw_extra);
    assert!(!tool_transform::timeline_toggle_tween(&mut app));
}

#[test]
fn test_classic_tween_translation_and_custom_pivot_ccw_rotation() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(200.0, 150.0),
        16.0,
        Color32::from_rgb(230, 80, 80),
    );
    app.session.set_selected_indices(vec![0]);
    tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert!(tool_transform::timeline_toggle_tween(&mut app));
    for _ in 0..4 {
        assert!(timeline_ui::timeline_extend_frame(&mut app));
    }
    assert_eq!(app.doc.root.total_frames, 5);
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete
    );

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert!(matches!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Complete {
            start_frame: 0,
            end_frame: 4,
            ..
        }
    ));

    app.session.set_selected_indices(vec![0]);
    let snap = app.session.shapes.clone();
    app.session.translate_selected(KVec2::new(200.0, 80.0));
    tool_select::commit_moved_selection(&mut app, snap);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_bbox = app.session.shapes[0].bounding_box();
    let mid_cx = (mid_bbox.x0 + mid_bbox.x1) * 0.5;
    let mid_cy = (mid_bbox.y0 + mid_bbox.y1) * 0.5;
    assert!((mid_cx - 250.0).abs() < 1.0);
    assert!((mid_cy - 190.0).abs() < 1.0);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    let start_bbox = app.session.shapes[0].bounding_box();
    let start_cx = (start_bbox.x0 + start_bbox.x1) * 0.5;
    let start_cy = (start_bbox.y0 + start_bbox.y1) * 0.5;
    assert!((start_cx - 150.0).abs() < 1.0);
    assert!((start_cy - 150.0).abs() < 1.0);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    app.session.set_selected_indices(vec![0]);
    let snap_reset = app.session.shapes.clone();
    app.session.translate_selected(KVec2::new(-200.0, -80.0));
    tool_select::commit_moved_selection(&mut app, snap_reset);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    let joint_world = Point::new(100.0, 150.0);
    assert!(tool_transform::set_selected_symbol_world_pivot(
        &mut app,
        Some(joint_world),
        true
    ));
    tool_select::clear_selection(&mut app);
    app.session.set_selected_indices(vec![0]);
    let bbox_f4 = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();
    assert!(
        (app.scene()
            .active_transform_pivot(&app.session.selection, app.session.transform_pivot, bbox_f4)
            - joint_world)
            .hypot()
            < 1e-4
    );
    assert!(tool_transform::rotate_selected_shapes(
        &mut app,
        std::f64::consts::FRAC_PI_2
    ));
    canvas_tool::sync_shapes_to_timeline(&mut app);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_binding = app.session.shapes[0].symbol_instance.unwrap();
    assert!((mid_binding.world_pivot() - joint_world).hypot() < 1e-4);
    let expected_mid_center = Point::new(
        100.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        150.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
    );
    assert!(app.session.shapes[0].contains_point(expected_mid_center));

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert!(app.view.inspect_timeline_frame);
    tool_select::select_single_shape_at(&mut app, Point::new(150.0, 150.0));
    assert!(!app.view.inspect_timeline_frame);

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert!(tool_transform::set_active_keyframe_tween_rotate(
        &mut app,
        crate::symbol::TweenRotate::CounterClockwise
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let expected_ccw_mid_center = Point::new(
        100.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        150.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
    );
    assert!(app.session.shapes[0].contains_point(expected_ccw_mid_center));
}

#[test]
fn test_symbol_playback_modes_loop_play_once_and_single_frame() {
    use crate::symbol::SymbolLoopMode;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(60, 190, 90);
    let blue = Color32::from_rgb(60, 110, 230);
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    app.session.set_selected_indices(vec![0]);
    assert!(tool_transform::convert_selection_to_symbol(&mut app).is_some());

    let (binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
    assert!(tool_transform::enter_symbol_timeline(&mut app, binding));

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        green,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        blue,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert_eq!(app.doc.root.total_frames, 3);

    assert!(tool_transform::exit_symbol_timeline(&mut app));
    for _ in 0..4 {
        assert!(timeline_ui::timeline_extend_frame(&mut app));
    }
    assert_eq!(app.doc.root.total_frames, 5);

    let expected_loop = [red, green, blue, red, green];
    for (f, &col) in expected_loop.iter().enumerate() {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(app.session.shapes[0].color, col, "Loop mode at frame {f}");
    }

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.session.set_selected_indices(vec![0]);
    assert!(tool_transform::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::PlayOnce,
        0
    ));
    let expected_once = [red, green, blue, blue, blue];
    for (f, &col) in expected_once.iter().enumerate() {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(app.session.shapes[0].color, col, "PlayOnce mode at frame {f}");
    }

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.session.set_selected_indices(vec![0]);
    assert!(tool_transform::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::SingleFrame,
        1
    ));
    assert_eq!(app.session.shapes[0].color, green);
    assert_eq!(app.session.selected_indices(), vec![0]);
    for f in 0..5 {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(app.session.shapes[0].color, green);
    }
}

#[test]
fn test_nested_symbol_recursive_frame_evaluation_and_blank_frame_marker() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 110, 230);

    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    app.session.set_selected_indices(vec![0]);
    let child_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    let (child_binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
    assert!(tool_transform::enter_symbol_timeline(
        &mut app,
        child_binding
    ));
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        blue,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert!(tool_transform::exit_symbol_timeline(&mut app));

    app.session.set_selected_indices(vec![0]);
    let parent_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert_ne!(child_sym_id, parent_sym_id);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.session.shapes[0].color, red);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(app.session.shapes[0].color, blue);

    assert!(tool_transform::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.total_frames, 3);
    assert!(tool_transform::exit_symbol_timeline(&mut app));

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(app.session.shapes.len(), 1);
    assert!(app.session.shapes[0].is_empty_symbol_marker());
    canvas_tool::select_shapes_for_timeline_selection(&mut app);
    assert_eq!(app.session.selected_indices(), vec![0]);
    assert!(app
        .scene()
        .selected_single_symbol_binding(&app.session.selection)
        .is_some());
    tool_select::clear_selection(&mut app);
    assert!(app
        .doc.root
        .resolved_cel_screen_mesh(0, 2, 1.0, Vec2::ZERO, false, &app.doc.library)
        .is_none());

    let extra = make_stroke(
        Point::new(220.0, 100.0),
        Point::new(260.0, 100.0),
        8.0,
        Color32::BLACK,
    )
    .remove(0);
    app.session.shapes.push(extra);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.session.shapes.len(), 2);
}

#[test]
fn test_symbol_rename_library_delete_detach_and_inplace_child_symbol_creation() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    app.session.set_selected_indices(vec![0]);
    let child_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    app.session.set_selected_indices(vec![0]);
    let parent_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();

    let orig_name = app.doc.find_symbol(parent_sym_id).unwrap().name.clone();
    assert!(tool_transform::rename_symbol_in_library(
        &mut app,
        parent_sym_id,
        "HeroSymbol".to_string()
    ));
    assert_eq!(
        app.doc.find_symbol(parent_sym_id).unwrap().name,
        "HeroSymbol"
    );
    app.undo();
    assert_eq!(
        app.doc.find_symbol(parent_sym_id).unwrap().name,
        orig_name
    );

    timeline_ui::set_layer_locked(&mut app, 0, true);
    assert!(app.doc.root.layers[0].locked);
    app.undo();
    assert!(!app.doc.root.layers[0].locked);

    assert!(tool_transform::delete_symbol_from_library(
        &mut app,
        parent_sym_id
    ));
    assert_eq!(app.session.shapes.len(), 1);
    assert!(app.session.shapes[0].symbol_instance.is_none());
    assert!(!app.session.shapes[0].group_path.is_empty());

    assert!(tool_transform::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.session.set_selected_indices(vec![0]);
    let lib_len_before = app.doc.library.len();
    let grandchild_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert_eq!(app.doc.library.len(), lib_len_before + 1);
    assert!(!app
        .doc
        .find_symbol(child_sym_id)
        .unwrap()
        .evaluated_local_shapes_at(0, &app.doc.library)
        .is_empty());
    app.undo();
    assert_eq!(app.doc.library.len(), lib_len_before);
    app.redo();
    assert!(app.doc.find_symbol(grandchild_sym_id).is_some());
    assert!(tool_transform::exit_symbol_timeline(&mut app));
}

#[test]
fn test_degenerate_nan_infinite_stroke_and_affine_inputs_do_not_panic() {
    use crate::vector_engine::union_bounding_boxes;
    use kurbo::{Affine, Rect as KRect};

    let red = Color32::from_rgb(220, 60, 60);
    assert!(generate_brush_outline(&[], 8.0).is_empty());
    assert!(generate_brush_outline(&[Point::new(f64::NAN, 10.0)], 8.0).is_empty());
    assert!(generate_brush_outline(&[Point::new(10.0, 10.0)], f64::NAN).is_empty());
    assert!(build_smoothed_stroke_shapes(&[Point::new(f64::INFINITY, 0.0); 4], red).is_empty());
    assert_eq!(union_bounding_boxes(&[]), KRect::ZERO);

    let dot_outline = generate_brush_outline(&[Point::new(100.0, 100.0)], 6.0);
    let dot_shapes = build_smoothed_stroke_shapes(&dot_outline, red);
    assert_eq!(dot_shapes.len(), 1);

    let mut shape = dot_shapes[0].clone();
    let orig_bbox = shape.bounding_box();
    shape.translate(KVec2::new(f64::NAN, f64::INFINITY));
    shape.transform(Affine::scale(0.0));
    shape.transform(Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]));
    assert_eq!(shape.bounding_box(), orig_bbox);
    assert!(!shape.contains_point(Point::new(f64::NAN, f64::NAN)));
}

#[test]
fn test_active_canvas_drag_state_machine_and_symbol_inplace_roundtrip_cache() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    app.session.shapes = make_stroke(Point::new(50.0, 100.0), Point::new(200.0, 100.0), 14.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(80.0, 70.0),
        Point::new(130.0, 130.0),
        false,
    );
    assert!(tool_transform::convert_selection_to_symbol(&mut app).is_some());
    let (b, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
    assert!(tool_transform::enter_symbol_timeline(&mut app, b));
    assert!(tool_transform::exit_symbol_timeline(&mut app));
    assert!(!tool_transform::exit_symbol_timeline(&mut app));
    assert!(!tool_select::exit_current_group(&mut app));
    app.undo();
    app.redo();
    timeline_ui::set_layer_visible(&mut app, 999, false);
    timeline_ui::set_layer_locked(&mut app, 999, true);
    assert!(!timeline_ui::timeline_move_or_copy_frames(
        &mut app,
        crate::timeline::TimelineCellSelection::single(999, 999),
        1,
        1,
        false
    ));

    let clean_shapes = app.session.shapes.clone();
    let clean_bbox = app.session.shapes[0].bounding_box();
    app.session.drag = crate::canvas_tool::ActiveCanvasDrag::Select(
        crate::canvas_tool::SelectDragState::MoveShape(crate::canvas_tool::MoveSelectionDrag::new(
            Point::ZERO,
            clean_shapes,
        )),
    );
    app.session.shapes[0].translate(KVec2::new(500.0, 500.0));
    tool_select::clear_selection(&mut app);
    assert_eq!(app.session.shapes[0].bounding_box(), clean_bbox);

    let stroke = vec![Point::new(10.0, 10.0), Point::new(40.0, 10.0)];
    let tail_outline = generate_brush_outline(&stroke, 8.0);
    let tail_mesh = crate::vector_engine::tessellate_raw_polygon(&tail_outline, Color32::BLACK);
    app.session.drag = crate::canvas_tool::ActiveCanvasDrag::PaintStroke {
        stroke,
        committed_world_mesh: egui::Mesh::default(),
        committed_screen_mesh: egui::Mesh::default(),
        committed_zoom_pan: (1.0, Vec2::ZERO),
        tail_outline,
        tail_mesh,
        tail_start_idx: 0,
    };
    tool_select::clear_selection(&mut app);
    assert!(matches!(
        app.session.drag,
        crate::canvas_tool::ActiveCanvasDrag::PaintStroke { .. }
    ));
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Brush });
    assert!(matches!(
        app.session.drag,
        crate::canvas_tool::ActiveCanvasDrag::None
    ));

    let b = app
        .session
        .shapes
        .iter()
        .find_map(|s| s.symbol_instance)
        .unwrap();
    let sym_id = b.symbol_id;
    let orig_local_shape = app
        .doc
        .find_symbol(sym_id)
        .unwrap()
        .timeline
        .layers[0]
        .keyframes[0]
        .cel
        .shapes[0]
        .clone();
    let orig_rev = orig_local_shape.mesh_revision;
    for _ in 0..10 {
        assert!(tool_transform::enter_symbol_timeline(&mut app, b));
        assert!(tool_transform::exit_symbol_timeline(&mut app));
    }
    let after_local_shape = &app
        .doc
        .find_symbol(sym_id)
        .unwrap()
        .timeline
        .layers[0]
        .keyframes[0]
        .cel
        .shapes[0];
    assert!(after_local_shape
        .has_same_contour_arcs(&orig_local_shape.exterior, &orig_local_shape.holes));
    assert_eq!(after_local_shape.mesh_revision, orig_rev);
}

#[test]
fn test_timeline_resize_total_frames_and_fill_shape_lazy_retessellation() {
    let mut tl = Timeline {
        total_frames: 10,
        ..Default::default()
    };
    tl.layers[0].frame_count = 10;
    tl.layers[0].insert_keyframe(8, false);
    let short_layer = crate::timeline::Layer::new_with_id_and_frames(2, "Short Layer", 4);
    tl.layers.push(short_layer);
    tl.resize_total_frames(20);
    assert_eq!(tl.layers[0].frame_count, 20);
    assert_eq!(tl.layers[1].frame_count, 4);
    tl.resize_total_frames(6);
    assert_eq!(tl.layers[0].frame_count, 6);
    assert!(tl.layers[0].keyframes.iter().all(|kf| kf.start_frame < 6));

    let small_raw = generate_brush_outline(&[Point::new(100.0, 100.0)], 4.0);
    let mut small_circle = build_smoothed_stroke_shapes(&small_raw, Color32::WHITE)
        .into_iter()
        .next()
        .unwrap();
    assert!(!small_circle.has_cached_fill_mesh());
    let verts_before = small_circle.fill_mesh().vertices.len();
    assert!(small_circle.has_cached_fill_mesh());
    small_circle.transform(kurbo::Affine::scale_about(10.0, Point::new(100.0, 100.0)));
    assert!(!small_circle.has_cached_fill_mesh());
    assert!(small_circle.fill_mesh().vertices.len() > verts_before);
}

#[test]
fn test_normalized_cel_instances_empty_symbol_revival_and_undo_cache_stripping() {
    let mut sym_app = Is2dApp::default();
    let mut two_shapes = make_stroke(
        Point::new(200.0, 200.0),
        Point::new(260.0, 200.0),
        10.0,
        Color32::from_rgb(200, 60, 60),
    );
    two_shapes.extend(make_stroke(
        Point::new(200.0, 240.0),
        Point::new(260.0, 240.0),
        10.0,
        Color32::from_rgb(60, 120, 220),
    ));
    sym_app.session.shapes = two_shapes;
    sym_app.session.set_selected_indices(vec![0, 1]);
    assert_eq!(sym_app.session.selection.shapes.len(), 2);
    assert!(sym_app.session.selection.instances.is_empty());
    let empty_test_sym_id = tool_transform::convert_selection_to_symbol(&mut sym_app).unwrap();
    assert!(sym_app.session.selection.shapes.is_empty());
    assert_eq!(sym_app.session.selection.instances.len(), 1);
    assert_eq!(sym_app.session.shapes.len(), 2);
    assert!(sym_app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes
        .is_empty());
    assert_eq!(
        sym_app.doc.root.layers[0].keyframes[0].cel.instances.len(),
        1
    );
    assert!(tool_transform::enter_symbol_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    assert!(tool_select::clear_active_frame_shapes(&mut sym_app));
    assert!(tool_transform::exit_symbol_timeline(&mut sym_app));

    assert_eq!(sym_app.session.shapes.len(), 1);
    assert!(sym_app.session.shapes[0].is_empty_symbol_marker());
    assert!(sym_app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes
        .is_empty());
    assert_eq!(
        sym_app.doc.root.layers[0].keyframes[0].cel.instances.len(),
        1
    );
    assert!(sym_app.doc.root.layers[0].keyframes[0]
        .cel
        .single_symbol_instance()
        .is_some());
    assert!(tool_transform::place_symbol_instance_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    assert_eq!(sym_app.session.shapes.len(), 2);
    assert!(sym_app.session.shapes[1].is_empty_symbol_marker());
    sym_app.undo();
    assert_eq!(sym_app.session.shapes.len(), 1);

    assert!(tool_transform::enter_symbol_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    sym_app.session.shapes = make_stroke(
        Point::new(210.0, 200.0),
        Point::new(250.0, 200.0),
        8.0,
        Color32::from_rgb(60, 180, 90),
    );
    canvas_tool::sync_shapes_to_timeline(&mut sym_app);
    assert!(tool_transform::exit_symbol_timeline(&mut sym_app));
    assert_eq!(sym_app.session.shapes.len(), 1);
    assert!(sym_app.session.shapes[0].contains_point(Point::new(230.0, 200.0)));

    let _ = sym_app.doc.library[0].timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .fill_mesh();
    assert!(sym_app.doc.library[0].timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .has_cached_fill_mesh());
    let orig_stage = sym_app.doc.stage_size;
    doc_actions::apply(
        &mut sym_app,
        doc_actions::Action::SetStageSize {
            size: Vec2::new(1920.0, 1080.0),
        },
    );
    let last_undo = sym_app.history.undo_stack.back().unwrap();
    assert!(!last_undo.library[0].timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .has_cached_fill_mesh());
    sym_app.undo();
    assert_eq!(sym_app.doc.stage_size, orig_stage);
    sym_app.redo();
    assert_eq!(sym_app.doc.stage_size, Vec2::new(1920.0, 1080.0));

    let sel_m1 = sym_app.session.shapes[0].cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
    let sel_m2 = sym_app.session.shapes[0].cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
    assert!(std::sync::Arc::ptr_eq(&sel_m1, &sel_m2));
    let orig1 = sym_app.doc.library[0]
        .preview_local_bounds(sym_app.doc.revision, &sym_app.doc.library)
        .unwrap();
    let prev_m1 = sym_app.doc.library[0]
        .cached_preview_screen_mesh(
            1.0,
            Vec2::new(100.0, 55.0),
            sym_app.doc.revision,
            &sym_app.doc.library,
        )
        .unwrap();
    let orig2 = sym_app.doc.library[0]
        .preview_local_bounds(sym_app.doc.revision, &sym_app.doc.library)
        .unwrap();
    let prev_m2 = sym_app.doc.library[0]
        .cached_preview_screen_mesh(
            1.0,
            Vec2::new(100.0, 55.0),
            sym_app.doc.revision,
            &sym_app.doc.library,
        )
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&prev_m1, &prev_m2) && orig1 == orig2);
}

#[test]
fn test_reverse_order_nested_symbol_topological_propagation_and_cycle_guard() {
    let mut nest_app = Is2dApp::default();
    nest_app.session.shapes = make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(100, 100, 100),
    );
    nest_app.session.set_selected_indices(vec![0]);
    let sym_a = tool_transform::convert_selection_to_symbol(&mut nest_app).unwrap();

    nest_app.session.shapes.extend(make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(150, 150, 150),
    ));
    let last_idx = nest_app.session.shapes.len() - 1;
    nest_app.session.set_selected_indices(vec![last_idx]);
    let sym_b = tool_transform::convert_selection_to_symbol(&mut nest_app).unwrap();
    tool_select::delete_selected_shapes(&mut nest_app);

    nest_app.session.shapes.extend(make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(200, 50, 50),
    ));
    let last_idx = nest_app.session.shapes.len() - 1;
    nest_app.session.set_selected_indices(vec![last_idx]);
    let sym_c = tool_transform::convert_selection_to_symbol(&mut nest_app).unwrap();
    tool_select::delete_selected_shapes(&mut nest_app);
    assert!(sym_a < sym_b && sym_b < sym_c);

    assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_a));
    assert!(tool_select::clear_active_frame_shapes(&mut nest_app));
    assert!(tool_transform::place_symbol_instance_from_library(
        &mut nest_app,
        sym_b
    ));
    assert!(tool_transform::exit_symbol_timeline(&mut nest_app));

    assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_b));
    assert!(tool_select::clear_active_frame_shapes(&mut nest_app));
    assert!(tool_transform::place_symbol_instance_from_library(
        &mut nest_app,
        sym_c
    ));
    assert!(tool_transform::exit_symbol_timeline(&mut nest_app));

    let updated_green = Color32::from_rgb(20, 210, 90);
    assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_c));
    for s in &mut nest_app.session.shapes {
        s.set_color(updated_green);
    }
    canvas_tool::sync_shapes_to_timeline(&mut nest_app);
    assert!(tool_transform::exit_symbol_timeline(&mut nest_app));
    assert_eq!(nest_app.session.shapes.len(), 1);
    assert_eq!(nest_app.session.shapes[0].color, updated_green);

    let scaled_binding =
        crate::symbol::SymbolInstanceBinding::new(999, sym_c, kurbo::Affine::scale(3.0));
    let sym_c_def = nest_app.doc.find_symbol(sym_c).unwrap();
    let inst_shapes = scaled_binding.instantiate_shapes(
        &sym_c_def.timeline.layers[0].keyframes[0].cel.shapes,
        1,
        &[],
    );
    assert!(inst_shapes[0].has_cached_fill_mesh());

    assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_b));
    assert!(!nest_app.can_place_symbol_in_current_context(sym_a));
    assert!(!tool_transform::place_symbol_instance_from_library(
        &mut nest_app,
        sym_a
    ));
    assert!(tool_transform::exit_symbol_timeline(&mut nest_app));
}

#[test]
fn test_eyedropper_samples_interpolated_tween_position_on_locked_layer() {
    let mut tween_eye_app = Is2dApp::default();
    let tween_color = Color32::from_rgb(240, 120, 30);
    tween_eye_app.session.shapes = make_stroke(
        Point::new(90.0, 200.0),
        Point::new(110.0, 200.0),
        12.0,
        tween_color,
    );
    tween_eye_app.session.set_selected_indices(vec![0]);
    tool_transform::convert_selection_to_symbol(&mut tween_eye_app).unwrap();
    tool_transform::timeline_toggle_tween(&mut tween_eye_app);
    timeline_ui::select_timeline_cells(&mut tween_eye_app, 0, 4, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut tween_eye_app, true);
    tool_select::nudge_selected_shapes(&mut tween_eye_app, kurbo::Vec2::new(200.0, 0.0));
    timeline_ui::set_layer_locked(&mut tween_eye_app, 0, true);
    timeline_ui::scrub_ruler_to_frame(&mut tween_eye_app, 2);
    assert_eq!(
        tool_paint::topmost_visible_color_at(&tween_eye_app, Point::new(200.0, 200.0)),
        Some(tween_color),
    );
    assert_eq!(
        tool_paint::topmost_visible_color_at(&tween_eye_app, Point::new(100.0, 200.0)),
        None,
    );
}

#[test]
fn test_blank_frame_symbol_in_parent_group_moves_rotates_and_deletes_synchronously() {
    // 空白フレーム状態のシンボルを含む親グループ（Cmd+G）の移動・回転・削除同期、および Undo 履歴の Arc 構造共有（CoW）
    let mut grp_sym_app = Is2dApp::default();
    grp_sym_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(120.0, 100.0),
        10.0,
        Color32::from_rgb(200, 40, 40),
    );
    grp_sym_app.session.set_selected_indices(vec![0]);
    let blink_sym_id = tool_transform::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
    // シンボル内部に入り、2フレーム目を空白キーフレーム（F7）にする（F0: 表示, F1: 空白）
    let blink_binding = grp_sym_app.session.shapes[0].symbol_instance.unwrap();
    assert!(tool_transform::enter_symbol_timeline(
        &mut grp_sym_app,
        blink_binding
    ));
    assert!(timeline_ui::timeline_insert_keyframe(
        &mut grp_sym_app,
        false
    ));
    assert!(tool_transform::exit_symbol_timeline(&mut grp_sym_app));

    // ルートシーンに別の生シェイプを追加し、シンボルと一緒に Cmd+G でグループ化する
    let sib_shapes = make_stroke(
        Point::new(100.0, 160.0),
        Point::new(120.0, 160.0),
        10.0,
        Color32::from_rgb(40, 120, 220),
    );
    grp_sym_app.session.shapes.extend(sib_shapes);
    grp_sym_app.session.set_selected_indices(vec![0, 1]);
    assert!(tool_select::group_selected_shapes(&mut grp_sym_app).is_some());
    // ルートシーンを3フレームに延長（F0: シンボル表示, F1: シンボル空白, F2: シンボルループ再表示）
    timeline_ui::select_timeline_cells(&mut grp_sym_app, 0, 2, 0, 2);
    assert!(timeline_ui::timeline_extend_frame(&mut grp_sym_app));

    // F1（シンボルが空白フレームで消えている状態）へ移動し、親グループを (+150, 0) 平行移動する
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    assert_eq!(
        grp_sym_app.session.shapes.len(),
        2,
        "F1では空白シンボルマーカー1個と兄弟シェイプ1個の計2個が CanvasState::shapes 内に一元展開されていること"
    );
    { let sel = grp_sym_app.scene().expand_to_group_unit(0, &grp_sym_app.session.editing_group_path); grp_sym_app.session.set_selected_indices(sel); }
    assert_eq!(grp_sym_app.session.selected_indices(), vec![0, 1]);
    assert!(tool_select::nudge_selected_shapes(
        &mut grp_sym_app,
        kurbo::Vec2::new(150.0, 0.0)
    ));

    // F2（シンボルがループして再び表示されるフレーム）へ進むと、空白フレーム中に移動した親グループに追従してシンボルも (+150, 0) 移動していること
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    assert_eq!(grp_sym_app.session.shapes.len(), 2);
    let moved_sym_shape = grp_sym_app
        .session
        .shapes
        .iter()
        .find(|s| {
            s.symbol_instance
                .is_some_and(|b| b.symbol_id == blink_sym_id)
        })
        .unwrap();
    assert!(
        moved_sym_shape.contains_point(Point::new(260.0, 100.0)),
        "空白フレーム中に親グループを移動した場合も、内部の空白フレームシンボルが置いていかれず追従移動すること"
    );

    // Undo 履歴の Arc<Vec<SymbolDef>> および未変更キーフレームの Cel::shapes (Arc<Vec<FillShape>>) が連続操作間でポインタ共有（CoW）されていること
    doc_actions::apply(
        &mut grp_sym_app,
        doc_actions::Action::SetStageSize {
            size: Vec2::new(810.0, 450.0),
        },
    );
    doc_actions::apply(&mut grp_sym_app, doc_actions::Action::EndCoalesce);
    doc_actions::apply(
        &mut grp_sym_app,
        doc_actions::Action::SetStageSize {
            size: Vec2::new(800.0, 450.0),
        },
    );
    let len_stack = grp_sym_app.history.undo_stack.len();
    let e_prev = &grp_sym_app.history.undo_stack[len_stack - 2];
    let e_last = &grp_sym_app.history.undo_stack[len_stack - 1];
    assert!(
        std::sync::Arc::ptr_eq(&e_prev.library, &e_last.library),
        "シンボル未変更時の UndoEntry 間では library がディープクローンされず Arc 共有されること"
    );
    assert!(
        std::sync::Arc::ptr_eq(
            &e_prev.timeline.layers[0].keyframes[0].cel.shapes,
            &e_last.timeline.layers[0].keyframes[0].cel.shapes
        ),
        "未変更キーフレームの Cel::shapes は連続する UndoEntry 間でディープクローンされず Arc 共有（CoW）されること"
    );

    // F1に戻って親グループを +90° 回転させても、空白フレーム中のシンボルが正確なアフィン変換で追従すること
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    { let sel = grp_sym_app.scene().expand_to_group_unit(0, &grp_sym_app.session.editing_group_path); grp_sym_app.session.set_selected_indices(sel); }
    assert!(tool_transform::rotate_selected_shapes(
        &mut grp_sym_app,
        std::f64::consts::FRAC_PI_2
    ));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    let rotated_sym_shape = grp_sym_app
        .session
        .shapes
        .iter()
        .find(|s| {
            s.symbol_instance
                .is_some_and(|b| b.symbol_id == blink_sym_id)
        })
        .unwrap();
    assert!(
        rotated_sym_shape.contains_point(Point::new(320.0, 160.0)),
        "空白フレーム中に親グループを回転した場合も、内部の空白フレームシンボルが同じ回転変換で追従すること: bbox={:?}",
        rotated_sym_shape.bounding_box()
    );

    // F1に戻って親グループを削除すると、空白フレーム状態のシンボルインスタンスも一緒に削除されること
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    { let sel = grp_sym_app.scene().expand_to_group_unit(0, &grp_sym_app.session.editing_group_path); grp_sym_app.session.set_selected_indices(sel); }
    assert!(tool_select::delete_selected_shapes(&mut grp_sym_app));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    assert!(
        grp_sym_app.session.shapes.is_empty(),
        "空白フレーム中に親グループを削除すると、内部の空白フレームシンボルも残骸にならず一緒に削除されること"
    );
}

#[test]
fn test_resolved_cel_screen_and_onion_mesh_cache_and_pan_invalidation() {
    let mut grp_sym_app = Is2dApp::default();
    grp_sym_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(120.0, 100.0),
        10.0,
        Color32::from_rgb(200, 40, 40),
    );
    grp_sym_app.session.set_selected_indices(vec![0]);
    let blink_sym_id = tool_transform::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
    let blink_binding = grp_sym_app.session.shapes[0].symbol_instance.unwrap();
    assert!(tool_transform::enter_symbol_timeline(
        &mut grp_sym_app,
        blink_binding
    ));
    assert!(timeline_ui::timeline_insert_keyframe(
        &mut grp_sym_app,
        false
    ));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(tool_select::clear_active_frame_shapes(&mut grp_sym_app));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    grp_sym_app.session.shapes = make_stroke(
        Point::new(-20.0, 0.0),
        Point::new(20.0, 0.0),
        10.0,
        Color32::from_rgb(50, 180, 100),
    );
    canvas_tool::sync_shapes_to_timeline(&mut grp_sym_app);
    assert!(tool_transform::exit_symbol_timeline(&mut grp_sym_app));

    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(tool_select::clear_active_frame_shapes(&mut grp_sym_app));
    timeline_ui::select_timeline_cells(&mut grp_sym_app, 0, 2, 0, 2);
    assert!(timeline_ui::timeline_extend_frame(&mut grp_sym_app));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(
        tool_transform::place_symbol_instance_from_library(&mut grp_sym_app, blink_sym_id),
        "Frame 0 が空白のマルチフレームシンボルでもライブラリからステージへ配置できること"
    );
    assert_eq!(
        grp_sym_app.doc.root.layers[0].keyframes[0]
            .cel
            .instances
            .len(),
        1,
        "Frame 0 時点で Cel::instances にシンボルインスタンスが登録されていること"
    );
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    assert_eq!(
        grp_sym_app.session.shapes.len(),
        1,
        "Frame 1 に進むと配置したシンボルの2コマ目の図形がステージ中央に出現すること"
    );
    let stage_center = grp_sym_app.view.stage_center_world();
    assert!(grp_sym_app.session.shapes[0].contains_point(stage_center));

    // シンボルを含むセルのスクリーンメッシュおよびオニオンスキンメッシュのキャッシュ再利用（Arcポインタ共有）検証
    let m_scr_1 = grp_sym_app
        .doc.root
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.doc.library)
        .unwrap();
    let m_scr_2 = grp_sym_app
        .doc.root
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.doc.library)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&m_scr_1, &m_scr_2),
        "キャッシュヒット時は頂点配列をディープコピーせず同一の Arc<Mesh> を O(1) 返却すること"
    );

    // パン（pan）が変化した際は resolved_cel_screen_mesh のスクリーン座標キャッシュが再計算されること (#3)
    let m_scr_panned = grp_sym_app
        .doc.root
        .resolved_cel_screen_mesh(
            0,
            1,
            1.0,
            Vec2::new(50.0, -30.0),
            false,
            &grp_sym_app.doc.library,
        )
        .unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&m_scr_2, &m_scr_panned),
        "pan が変化した際は古いスクリーン座標メッシュを返さず再変換されること"
    );
    assert!(
        (m_scr_panned.vertices[0].pos.x - (m_scr_2.vertices[0].pos.x + 50.0)).abs() < 1e-3,
        "pan の移動量が頂点スクリーン座標に正確に反映されていること"
    );

    let m_onion_1 = grp_sym_app
        .doc.root
        .resolved_cel_onion_mesh(
            0,
            1,
            1.0,
            Vec2::ZERO,
            ([35, 175, 105], 90),
            &grp_sym_app.doc.library,
        )
        .unwrap();
    let m_onion_2 = grp_sym_app
        .doc.root
        .resolved_cel_onion_mesh(
            0,
            1,
            1.0,
            Vec2::ZERO,
            ([35, 175, 105], 90),
            &grp_sym_app.doc.library,
        )
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&m_onion_1, &m_onion_2));

    // 無関係な2番目のシンボルが編集された場合はキャッシュが破棄されず維持され、
    // セルが実際に参照しているシンボルの revision が更新された場合にのみキャッシュが再評価されること
    let second_sym_shapes = make_stroke(
        Point::new(10.0, 10.0),
        Point::new(30.0, 10.0),
        6.0,
        Color32::BLUE,
    );
    let second_sym_id = grp_sym_app.doc.register_new_symbol(second_sym_shapes);
    let m_scr_before_unreferenced_edit = grp_sym_app
        .doc.root
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.doc.library)
        .unwrap();
    grp_sym_app
        .doc
        .rename_symbol(second_sym_id, "UnreferencedSymbolEdited".to_string());
    let m_scr_after_unreferenced_edit = grp_sym_app
        .doc.root
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.doc.library)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(
            &m_scr_before_unreferenced_edit,
            &m_scr_after_unreferenced_edit
        ),
        "当該セルが直接・間接に参照していない別シンボルの revision が更新されても解決済みセルキャッシュが無駄に破棄されないこと"
    );
    grp_sym_app
        .doc
        .rename_symbol(blink_sym_id, "ReferencedBlinkSymbolEdited".to_string());
    let m_scr_after_referenced_edit = grp_sym_app
        .doc.root
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.doc.library)
        .unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(
            &m_scr_after_unreferenced_edit,
            &m_scr_after_referenced_edit
        ),
        "当該セルが参照しているシンボルの revision が更新された際は解決済みセルキャッシュ署名が変化して再評価されること"
    );

    // シンボルインプレイス編集時に Frame 0 を編集しても未変更の Frame 1 の Cel::shapes は Arc ポインタ共有が維持されること
    let f1_local_arc_before = std::sync::Arc::clone(
        &grp_sym_app
            .doc
            .find_symbol(blink_sym_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[1]
            .cel
            .shapes,
    );
    let inst_b = grp_sym_app.doc.root.layers[0].keyframes[0].cel.instances[0].binding;
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    grp_sym_app.session.shapes.extend(make_stroke(
        Point::new(20.0, 20.0),
        Point::new(50.0, 20.0),
        8.0,
        Color32::GRAY,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut grp_sym_app);
    assert!(tool_transform::enter_symbol_timeline(
        &mut grp_sym_app,
        inst_b
    ));
    let session = grp_sym_app.session.edit_stack.last().unwrap();
    let bd_m1 = session
        .backdrop_cel
        .cached_backdrop_screen_mesh(0, 1.0, Vec2::ZERO, &grp_sym_app.doc.library)
        .expect("背景シェイプが存在するため backdrop メッシュが生成されること");
    let bd_m2 = session
        .backdrop_cel
        .cached_backdrop_screen_mesh(0, 1.0, Vec2::ZERO, &grp_sym_app.doc.library)
        .expect("キャッシュ済みの backdrop メッシュが返ること");
    assert!(std::sync::Arc::ptr_eq(&bd_m1, &bd_m2));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    doc_actions::apply(
        &mut grp_sym_app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(120.0, 100.0)],
            radius: 8.0,
            color: Color32::RED,
        },
    );
    assert!(tool_transform::exit_symbol_timeline(&mut grp_sym_app));
    let f1_local_arc_after = &grp_sym_app
        .doc
        .find_symbol(blink_sym_id)
        .unwrap()
        .timeline
        .layers[0]
        .keyframes[1]
        .cel
        .shapes;
    assert!(
        std::sync::Arc::ptr_eq(&f1_local_arc_before, f1_local_arc_after),
        "シンボルインプレイス編集で Undo 保存や別キーフレーム編集を挟んでも、未変更キーフレームのローカル Cel::shapes は論理IDベースの StageToLocalCache により Arc 共有（CoW）が維持されること"
    );
}

#[test]
fn test_shape_id_layer_sort_and_empty_symbol_marker_invariants() {
    // オニオンスキンやループ再生などのビュー設定（AppUiState / TimelineUiState）を変更した後に Undo / Redo を実行しても巻き戻らないこと
    let mut sort_app = Is2dApp::default();
    doc_actions::apply(&mut sort_app, doc_actions::Action::SetFps { fps: 30 });
    sort_app.view.onion_skin_enabled = true;
    sort_app.view.onion_prev_frames = 5;
    sort_app.view.onion_next_frames = 4;
    sort_app.timeline_ui.loop_playback = false;
    sort_app.undo();
    assert!(sort_app.view.onion_skin_enabled);
    assert_eq!(sort_app.view.onion_prev_frames, 5);
    assert_eq!(sort_app.view.onion_next_frames, 4);
    assert!(!sort_app.timeline_ui.loop_playback);
    sort_app.redo();
    assert!(sort_app.view.onion_skin_enabled);
    assert_eq!(sort_app.view.onion_prev_frames, 5);
    assert_eq!(sort_app.view.onion_next_frames, 4);
    assert!(!sort_app.timeline_ui.loop_playback);

    // `FillShape::shape_id` によるレイヤー順ソート後の選択状態追従と `LoadedShapeSig` の一致
    timeline_ui::timeline_add_layer(&mut sort_app);
    let lid_bottom = sort_app.doc.root.layers[0].id;
    let lid_top = sort_app.doc.root.layers[1].id;
    let shape_top = make_stroke(
        Point::new(10.0, 10.0),
        Point::new(30.0, 10.0),
        6.0,
        Color32::RED,
    )
    .remove(0)
    .with_layer_id(lid_top);
    let shape_bottom = make_stroke(
        Point::new(10.0, 40.0),
        Point::new(30.0, 40.0),
        6.0,
        Color32::BLUE,
    )
    .remove(0)
    .with_layer_id(lid_bottom);
    let top_shape_id = shape_top.shape_id;
    assert_ne!(top_shape_id, shape_bottom.shape_id);
    sort_app.session.shapes = vec![shape_top, shape_bottom];
    sort_app.session.set_selected_indices(vec![0]);
    assert!(sort_app.session.selection.shapes.contains(&top_shape_id));
    {
        let tab = sort_app.tab_mut();
        tab.session.sort_shapes_by_layer_order(&tab.doc.root);
    }
    assert_eq!(sort_app.session.shapes[0].layer_id, lid_bottom);
    assert_eq!(sort_app.session.shapes[1].layer_id, lid_top);
    assert!(sort_app.session.selection.shapes.contains(&top_shape_id));
    assert_eq!(
        sort_app.session.selected_indices(),
        vec![1],
        "sort_shapes_by_layer_order 後も shape_id に基づいて選択インデックスが新しい位置へ追従すること"
    );
    assert_eq!(sort_app.session.selected_shape_ids(), vec![top_shape_id]);
    sort_app.session.record_loaded_signatures();
    assert!(sort_app.session.is_all_shapes_unchanged());

    // 空シンボルマーカーのデータモデル不変条件（contours=0, geometry_bounding_box=None, スナップ非対象）
    let empty_marker = FillShape::new_empty_symbol_marker(
        crate::symbol::SymbolInstanceBinding::new(
            777,
            1,
            kurbo::Affine::translate((50.0, 50.0)),
        ),
        lid_bottom,
        &[],
    );
    assert_eq!(empty_marker.contours().count(), 0);
    assert!(empty_marker.get_contour(0).is_none());
    assert!(empty_marker.geometry_bounding_box().is_none());
    sort_app.session.shapes.push(empty_marker);
    assert!(
        tool_select::find_snap_vertex(
            sort_app.scene(),
            &sort_app.session.editing_group_path,
            Point::new(50.0, 50.0),
            1.0,
            usize::MAX,
            0,
            0,
        )
        .is_none(),
        "空白フレームのシンボルマーカーの基準点に対して頂点磁気スナップが発生しないこと"
    );
}

#[test]
fn test_bucket_fill_locked_walls_and_cluster_culling() {
    let red = Color32::from_rgb(220, 50, 50);
    // (a) バケツツール：アクティブレイヤーの円環を、ロックされた別レイヤーの分割線で2分割した際、分割線が優先されて片側のみ塗られること
    let mut bucket_app = Is2dApp::default();
    let center = Point::new(200.0, 200.0);
    let ring_pts: Vec<Point> = (0..=48)
        .map(|i| {
            let a = (i as f64 / 48.0) * std::f64::consts::TAU;
            Point::new(center.x + 60.0 * a.cos(), center.y + 60.0 * a.sin())
        })
        .collect();
    let raw_ring = generate_brush_outline(&ring_pts, 6.0);
    bucket_app.tool.brush_color = Color32::BLACK;
    tool_paint::commit_raw_outline(&mut bucket_app, &raw_ring);

    // Layer 2 を追加して円の中央 (x = 200) に縦の分割線を引き、Layer 2 をロック (🔒) する
    timeline_ui::timeline_add_layer(&mut bucket_app);
    let raw_div =
        generate_brush_outline(&[Point::new(200.0, 120.0), Point::new(200.0, 280.0)], 6.0);
    tool_paint::commit_raw_outline(&mut bucket_app, &raw_div);
    timeline_ui::set_layer_locked(&mut bucket_app, 1, true);
    timeline_ui::switch_to_layer_and_frame(&mut bucket_app, 0, 0);

    bucket_app.tool.brush_color = red;
    assert!(tool_paint::bucket_fill_at(
        &mut bucket_app,
        Point::new(170.0, 200.0)
    ));
    let red_fill = bucket_app
        .session
        .shapes
        .iter()
        .find(|s| s.color == red)
        .expect("赤の塗りシェイプが生成されていること");
    assert!(red_fill.contains_point(Point::new(170.0, 200.0)));
    assert!(
        !red_fill.contains_point(Point::new(230.0, 200.0)),
        "ロックされた別レイヤーの分割線が壁として機能し、反対側の右半分までは塗りつぶされないこと"
    );

    // (b) 大量の無関係な遠方シェイプが存在する場合も スイープライン＋Union-Find AABB 連結クラスタ事前カリングにより
    // クラスタ外クリックは即座に None を返し、対象閉領域内クリックは正確に抽出されること
    let mut many_walls = bucket_app.session.shapes.clone();
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
        crate::vector_boolean::find_enclosed_region_at(
            &many_walls,
            Point::new(900.0, 900.0),
            Color32::GREEN
        )
        .is_none(),
        "どの連結AABBクラスタにも含まれない空白座標はオーバーレイ構築前に None となること"
    );
    let enclosed_right = crate::vector_boolean::find_enclosed_region_at(
        &many_walls,
        Point::new(230.0, 200.0),
        Color32::GREEN,
    )
    .expect("無関係な遠方シェイプが多数存在しても対象クラスタ内の閉領域が正確に抽出されること");
    assert!(enclosed_right.contains_point(Point::new(230.0, 200.0)));
    assert!(!enclosed_right.contains_point(Point::new(170.0, 200.0)));
}

#[test]
fn test_micro_shapes_boolean_union_and_difference_at_16x_zoom() {
    let red = Color32::from_rgb(220, 50, 50);
    // 1600%（zoom = 16.0）拡大時の極小シェイプ同士のブール演算（Union / Difference）で消失しないこと
    let micro_raw_a = generate_brush_outline_at_zoom(
        &[Point::new(100.0, 100.0), Point::new(100.3, 100.0)],
        0.12,
        16.0,
    );
    let micro_raw_b = generate_brush_outline_at_zoom(
        &[Point::new(100.15, 99.9), Point::new(100.15, 100.1)],
        0.08,
        16.0,
    );
    let micro_a = build_smoothed_stroke_shapes_at_zoom(&micro_raw_a, red, 16.0);
    let micro_b = build_smoothed_stroke_shapes_at_zoom(&micro_raw_b, Color32::BLUE, 16.0);
    assert!(!micro_a.is_empty() && !micro_b.is_empty());
    let area_a: f64 = micro_a.iter().map(|s| s.exterior.signed_area()).sum();
    let area_b: f64 = micro_b.iter().map(|s| s.exterior.signed_area()).sum();
    let micro_union = crate::vector_boolean::boolean_operation(
        &micro_a,
        &micro_b,
        crate::vector_boolean::BoolOp::Union,
        red,
    );
    let area_union: f64 = micro_union.iter().map(|s| s.exterior.signed_area()).sum();
    assert!(
        !micro_union.is_empty()
            && area_union > area_a
            && area_union > area_b
            && micro_union
                .iter()
                .any(|s| s.contains_point(Point::new(100.0, 100.0)))
            && micro_union
                .iter()
                .any(|s| s.contains_point(Point::new(100.15, 100.09))),
        "1600%拡大時の極小シェイプ同士の Union が両方の領域を包含し面積が増加していること"
    );
    let micro_diff = crate::vector_boolean::boolean_operation(
        &micro_a,
        &micro_b,
        crate::vector_boolean::BoolOp::Difference,
        red,
    );
    let area_diff: f64 = micro_diff.iter().map(|s| s.exterior.signed_area()).sum();
    assert!(
        !micro_diff.is_empty()
            && area_diff < area_a * 0.95
            && micro_diff
                .iter()
                .any(|s| s.contains_point(Point::new(100.0, 100.0)))
            && !micro_diff
                .iter()
                .any(|s| s.contains_point(Point::new(100.15, 100.0))),
        "1600%拡大時の極小シェイプ同士の Difference で交差中央部が実際に切り抜かれ面積が減少していること"
    );
}

#[test]
fn test_tween_lru_cache_no_thrashing() {
    // 8フレーム超（12フレーム）のクラシックトゥイーン再生を2周ループした際、64エントリLRUキャッシュにより2周目もスラッシングせず同一 Arc<Mesh> が返ること
    let mut lru_tween_app = Is2dApp::default();
    lru_tween_app.session.shapes = make_stroke(
        Point::new(80.0, 200.0),
        Point::new(110.0, 200.0),
        10.0,
        Color32::from_rgb(220, 80, 40),
    );
    lru_tween_app.session.set_selected_indices(vec![0]);
    tool_transform::convert_selection_to_symbol(&mut lru_tween_app).unwrap();
    // シンボルインスタンスを移動確定してもキャッシュ済み fill_mesh が破棄されず、平行移動分だけ頂点がシフトすること
    let mesh_before = lru_tween_app.session.shapes[0].fill_mesh();
    assert!(!mesh_before.is_empty());
    let x0_before = mesh_before.vertices[0].pos.x;
    assert!(tool_select::nudge_selected_shapes(
        &mut lru_tween_app,
        kurbo::Vec2::new(10.0, 0.0)
    ));
    assert!(
        lru_tween_app.session.shapes[0].has_cached_fill_mesh(),
        "シンボルインスタンスの移動確定（commit_moved_selection）時にキャッシュ済み fill_mesh が無駄に破棄されないこと"
    );
    let mesh_after = lru_tween_app.session.shapes[0].fill_mesh();
    assert!(
        (mesh_after.vertices[0].pos.x - (x0_before + 10.0)).abs() < 1e-3,
        "移動確定後の fill_mesh 頂点座標が +10.0 シフトしていること"
    );
    assert!(tool_transform::timeline_toggle_tween(&mut lru_tween_app));
    timeline_ui::select_timeline_cells(&mut lru_tween_app, 0, 11, 0, 11);
    assert!(timeline_ui::timeline_insert_keyframe(
        &mut lru_tween_app,
        true
    ));
    assert!(tool_select::nudge_selected_shapes(
        &mut lru_tween_app,
        kurbo::Vec2::new(240.0, 0.0)
    ));

    let first_loop_meshes: Vec<std::sync::Arc<egui::Mesh>> = (0..12)
        .map(|f| {
            lru_tween_app
                .doc
                .root
                .resolved_cel_screen_mesh(
                    0,
                    f,
                    1.0,
                    Vec2::ZERO,
                    false,
                    &lru_tween_app.doc.library,
                )
                .unwrap()
        })
        .collect();
    for (f, first_m) in first_loop_meshes.iter().enumerate() {
        let second_m = lru_tween_app
            .doc
            .root
            .resolved_cel_screen_mesh(0, f, 1.0, Vec2::ZERO, false, &lru_tween_app.doc.library)
            .unwrap();
        assert!(
            std::sync::Arc::ptr_eq(first_m, &second_m),
            "12フレームのトゥイーンループ再生2周目（frame={f}）でもスラッシングを起こさず同一の Arc<Mesh> が O(1) キャッシュヒットすること"
        );
    }
}

#[test]
fn test_project_save_and_load_roundtrip() {
    let mut app = Is2dApp::default();
    app.doc.stage_size = Vec2::new(1280.0, 720.0);
    app.doc.root.fps = 30;

    // Layer 1: シンボル変換 + クラシックトゥイーン (Frame 0 -> Frame 5)
    app.session.shapes = make_stroke(
        Point::new(100.0, 120.0),
        Point::new(160.0, 120.0),
        12.0,
        Color32::from_rgb(240, 80, 60),
    );
    app.session.set_selected_indices(vec![0]);
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert!(tool_transform::timeline_toggle_tween(&mut app));
    assert!(tool_transform::set_active_keyframe_tween_rotate(
        &mut app,
        crate::symbol::TweenRotate::Clockwise
    ));
    timeline_ui::select_timeline_cells(&mut app, 0, 5, 0, 5);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    tool_select::nudge_selected_shapes(&mut app, kurbo::Vec2::new(150.0, 40.0));

    // Layer 2: グループ化された図形 + 生シェイプ
    timeline_ui::timeline_add_layer(&mut app);
    let mut raw_and_group = make_stroke(
        Point::new(200.0, 220.0),
        Point::new(250.0, 220.0),
        10.0,
        Color32::from_rgb(40, 160, 220),
    );
    let active_lid = app.doc.root.layers[app.doc.root.active_layer].id;
    for s in &mut raw_and_group {
        s.layer_id = active_lid;
    }
    app.session.shapes.extend(raw_and_group);
    let last_idx = app.session.shapes.len() - 1;
    app.session.set_selected_indices(vec![last_idx]);
    let gid = tool_select::group_selected_shapes(&mut app).unwrap();

    // シンボルインプレイス編集に入った状態で保存してもルートタイムラインとライブラリが正しく保存されること
    assert!(tool_transform::enter_symbol_from_library(&mut app, sym_id));
    app.session.shapes.extend(make_stroke(
        Point::new(130.0, 140.0),
        Point::new(150.0, 140.0),
        6.0,
        Color32::from_rgb(50, 200, 100),
    ));

    let tmp_path =
        std::env::temp_dir().join(format!("is2d_roundtrip_test_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).expect("保存に成功すること");
    assert_eq!(
        app.file.as_deref(),
        Some(tmp_path.as_path())
    );

    let mut loaded_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None)
        .expect("読み込みに成功すること");

    assert_eq!(loaded_app.doc.stage_size, Vec2::new(1280.0, 720.0));
    assert_eq!(loaded_app.doc.root.fps, 30);
    assert_eq!(loaded_app.doc.root.total_frames, 6);
    assert_eq!(loaded_app.doc.root.layers.len(), 2);
    assert_eq!(loaded_app.doc.library.len(), 1);
    assert_eq!(
        loaded_app
            .doc
            .find_symbol(sym_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[0]
            .cel
            .shapes
            .len(),
        2,
        "インプレイス編集中に追加した図形もシンボル定義へ保存・復元されていること"
    );
    assert!(loaded_app.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        loaded_app.doc.root.layers[0].keyframes[0].tween_rotate,
        crate::symbol::TweenRotate::Clockwise
    );
    assert!(
        loaded_app.doc.root.layers[1].keyframes[0]
            .cel
            .shapes
            .iter()
            .any(|s| s.group_path == vec![gid]),
        "グループパスも完全一致で復元されること"
    );
    assert!(loaded_app.doc.next_group_id > gid);
    assert!(loaded_app.doc.next_symbol_id > sym_id);

    let _ = std::fs::remove_file(&tmp_path);
}

#[test]
fn test_multitab_open_switch_symbol_copy_paste_and_close() {
    let mut app = Is2dApp::default();
    app.doc.stage_size = Vec2::new(1280.0, 720.0);
    app.session.shapes = make_stroke(
        Point::new(100.0, 120.0),
        Point::new(160.0, 120.0),
        12.0,
        Color32::from_rgb(240, 80, 60),
    );
    app.session.set_selected_indices(vec![0]);
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();

    let tmp_path =
        std::env::temp_dir().join(format!("is2d_multitab_test1_{}.is2d", std::process::id()));
    let tmp_path2 =
        std::env::temp_dir().join(format!("is2d_multitab_test2_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).unwrap();

    let mut app2 = Is2dApp::default();
    app2.doc.stage_size = Vec2::new(640.0, 480.0);
    crate::project_io::save_project_to_path(&mut app2, &tmp_path2)
        .expect("2つ目の保存に成功すること");

    let mut loaded_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None).unwrap();

    // Tab 0 (loaded_app) でシンボルインスタンスを選択してコピー（同時に保存時の選択状態維持も検証）
    timeline_ui::switch_to_layer_and_frame(&mut loaded_app, 0, 0);
    let sym_indices: Vec<usize> = (0..loaded_app.session.shapes.len())
        .filter(|&i| loaded_app.session.shapes[i].symbol_instance.is_some())
        .collect();
    loaded_app.session.set_selected_indices(sym_indices);
    loaded_app.session.transform_pivot = Some(Point::new(111.0, 222.0));
    assert!(!loaded_app.session.selection.is_empty());
    assert!(tool_select::copy_selected_shapes(&mut loaded_app));

    // Cmd+S（save_project_to_path）を実行しても現在の選択状態とカスタムピボットが強制解除されないこと
    crate::project_io::save_project_to_path(&mut loaded_app, &tmp_path)
        .expect("選択維持のまま上書き保存に成功すること");
    assert!(!loaded_app.session.selection.is_empty());
    assert_eq!(
        loaded_app.session.transform_pivot,
        Some(Point::new(111.0, 222.0))
    );
    assert!(!loaded_app.history.is_dirty());

    // シンボルインプレイス編集モード内で自分自身のシンボルを Cmd+V ペーストしようとした場合は循環参照防止で拒否されること
    assert!(tool_transform::enter_symbol_from_library(
        &mut loaded_app,
        sym_id
    ));
    assert!(
        !tool_select::paste_clipboard(&mut loaded_app, true),
        "編集中のシンボル自身は循環参照のためペースト拒否されること"
    );
    assert!(tool_transform::exit_symbol_timeline(&mut loaded_app));

    // 2つ目のファイルを別タブ（Tab 1）として開く
    let outcome = crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path2, None)
        .expect("別タブとして開けること");
    assert_eq!(outcome, crate::project_io::OpenProjectOutcome::Loaded);
    assert_eq!(loaded_app.tabs.len(), 2);
    assert_eq!(loaded_app.active, 1);
    assert_eq!(loaded_app.doc.stage_size, Vec2::new(640.0, 480.0));
    assert!(loaded_app.doc.library.is_empty());
    assert!(!loaded_app.history.is_dirty());

    // 現在アクティブなタブのファイルを再度開こうとした場合は AlreadyActiveTab が返ること
    let same_outcome = crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path2, None)
        .expect("同一アクティブタブ判定に成功すること");
    assert_eq!(
        same_outcome,
        crate::project_io::OpenProjectOutcome::AlreadyActiveTab
    );

    // Tab 0 からコピーしたシンボルを Tab 1 にペーストすると、Tab 1 のライブラリへシンボル定義が自動移植され Dirty 状態になること
    assert!(tool_select::paste_clipboard(&mut loaded_app, true));
    assert!(loaded_app.history.is_dirty());
    assert_eq!(
        loaded_app.doc.library.len(),
        1,
        "別タブからペーストしたシンボル定義がターゲットタブのライブラリに自動移植されること"
    );
    // Undo すれば移植されたシンボルごと元の未変更状態（!is_dirty）に戻ること
    loaded_app.undo();
    assert!(loaded_app.doc.library.is_empty());
    assert!(!loaded_app.history.is_dirty());

    // Undo 後に再度 Tab 1 でペーストした際、消去されたシンボル定義が再移植され図形が消失しないこと（指摘1-A）
    assert!(tool_select::paste_clipboard(&mut loaded_app, true));
    assert_eq!(loaded_app.doc.library.len(), 1);
    assert!(
        loaded_app
            .session
            .shapes
            .iter()
            .any(|s| s.symbol_instance.is_some()),
        "Undo後の再ペーストでもシンボルインスタンスが消失せず正しく配置されること"
    );

    // 既に開いている tmp_path (Tab 0) を開こうとした場合はタブが増えず SwitchedToExistingTab で Tab 0 に切り替わること
    let switch_outcome = crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None)
        .expect("既存タブへ切り替わること");
    assert_eq!(
        switch_outcome,
        crate::project_io::OpenProjectOutcome::SwitchedToExistingTab
    );
    assert_eq!(loaded_app.tabs.len(), 2);
    assert_eq!(loaded_app.active, 0);
    assert_eq!(loaded_app.doc.stage_size, Vec2::new(1280.0, 720.0));

    // 元タブ（Tab 0）に戻って再度ペーストした際、自分のシンボルが重複インポートされないこと（指摘1-C）
    assert!(tool_select::paste_clipboard(&mut loaded_app, true));
    assert_eq!(
        loaded_app.doc.library.len(),
        1,
        "コピー元タブに戻ってペーストしても同名・同内容のシンボルが重複増殖しないこと"
    );

    // Tab 0 を閉じると Tab 1 がアクティブになり、さらに最後のタブを閉じると新規の無題タブ（無題-1）にリセットされること
    crate::project_io::close_active_tab(&mut loaded_app, None);
    assert_eq!(loaded_app.tabs.len(), 1);
    assert_eq!(loaded_app.doc.stage_size, Vec2::new(640.0, 480.0));
    crate::project_io::close_active_tab(&mut loaded_app, None);
    assert_eq!(loaded_app.tabs.len(), 1);
    assert!(loaded_app.file.is_none());
    assert_eq!(loaded_app.tabs[0].untitled_number, 1);

    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_file(&tmp_path2);
}

#[test]
fn test_untitled_tab_numbering_and_per_tab_wireframe_state() {
    let tmp_path =
        std::env::temp_dir().join(format!("is2d_untitled_test1_{}.is2d", std::process::id()));
    let tmp_path2 =
        std::env::temp_dir().join(format!("is2d_untitled_test2_{}.is2d", std::process::id()));
    let mut app = Is2dApp::default();
    crate::project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    crate::project_io::save_project_to_path(&mut app, &tmp_path2).unwrap();

    // ファイル読込で新規タブが増えても next_untitled_number が無駄に消費されないこと
    let mut loaded_app = Is2dApp::default();
    doc_actions::apply(
        &mut loaded_app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(50.0, 50.0), Point::new(90.0, 50.0)],
            radius: 8.0,
            color: Color32::RED,
        },
    );
    let next_untitled_before = loaded_app.next_untitled_number;
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path2, None)
        .expect("ファイル読込に成功すること");
    assert_eq!(
        loaded_app.next_untitled_number, next_untitled_before,
        "既存ファイルを別タブで開いた際に無題タブ連番（next_untitled_number）が欠番スキップしないこと"
    );

    // 起動直後の白紙「無題-1」タブでファイルを開いた場合、次の新規タブが「無題-1」から採番され、show_wireframe がタブ独立保持されること
    let mut fresh_open_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut fresh_open_app, &tmp_path2, None)
        .expect("初期白紙タブへのファイル読込に成功すること");
    assert_eq!(fresh_open_app.tabs[0].untitled_number, 0);
    assert_eq!(
        fresh_open_app.alloc_untitled_number(),
        1,
        "初期の白紙無題タブがファイル読込で置き換わった場合、次の新規無題タブは 無題-1 になること"
    );
    fresh_open_app.view.show_wireframe = true;
    crate::project_io::open_project_in_tab(&mut fresh_open_app, &tmp_path, None)
        .expect("2タブ目の読込に成功すること");
    assert!(
        !fresh_open_app.view.show_wireframe,
        "新規に開いた別タブでは show_wireframe が初期値 false であること"
    );
    crate::project_io::switch_to_tab(&mut fresh_open_app, 0, None);
    assert!(
        fresh_open_app.view.show_wireframe,
        "元のタブへ戻ると show_wireframe = true が復元されること"
    );

    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_file(&tmp_path2);
}

#[test]
fn test_clipboard_symbol_evaluates_latest_symbol_def_and_shares_arc() {
    // コピー後にシンボル定義を編集して再ペーストした際、古い展開図形ではなく最新の SymbolDef が評価されること
    let mut sym_paste_app = Is2dApp::default();
    sym_paste_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(140.0, 100.0),
        10.0,
        Color32::RED,
    );
    sym_paste_app.session.set_selected_indices(vec![0]);
    let child_sym = tool_transform::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.session.set_selected_indices(vec![0]);
    let parent_sym = tool_transform::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.session.set_selected_indices(vec![0]);
    assert!(tool_select::copy_selected_shapes(&mut sym_paste_app));
    let first_clip_sym_arc = std::sync::Arc::clone(&sym_paste_app.clipboard.symbols[0]);
    assert!(tool_select::copy_selected_shapes(&mut sym_paste_app));
    assert!(
        std::sync::Arc::ptr_eq(&first_clip_sym_arc, &sym_paste_app.clipboard.symbols[0]),
        "同一シンボルリビジョンでの再コピー時は clipboard_symbols の Arc<SymbolDef> が O(1) ポインタ共有されること"
    );

    // 子シンボルをライブラリから削除（親シンボル内では通常グループへデタッチされる）
    assert!(tool_transform::delete_symbol_from_library(
        &mut sym_paste_app,
        child_sym
    ));
    assert_eq!(sym_paste_app.doc.library.len(), 1);

    // 親シンボルをインプレイス編集して青色の2シェイプに変更
    assert!(tool_transform::enter_symbol_from_library(
        &mut sym_paste_app,
        parent_sym
    ));
    sym_paste_app.doc.root.fps = 12;
    for sess in &mut sym_paste_app.session.edit_stack {
        sess.parent_timeline.fps = 12;
    }
    sym_paste_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(140.0, 100.0),
        10.0,
        Color32::BLUE,
    );
    sym_paste_app.session.shapes.extend(make_stroke(
        Point::new(100.0, 130.0),
        Point::new(140.0, 130.0),
        10.0,
        Color32::BLUE,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut sym_paste_app);
    assert!(tool_transform::exit_symbol_timeline(&mut sym_paste_app));
    assert_eq!(
        sym_paste_app.doc.root.fps, 12,
        "シンボルインプレイス編集中のFPS変更が親タイムライン復帰後も維持されること"
    );
    assert_eq!(sym_paste_app.session.shapes.len(), 2);

    // 再度 Cmd+V ペースト：古い赤1シェイプではなく最新の青2シェイプが展開され、削除済み child_sym も孤児復活しないこと
    assert!(tool_select::paste_clipboard(&mut sym_paste_app, false));
    assert_eq!(
        sym_paste_app.doc.library.len(),
        1,
        "デタッチ済みの子シンボルが孤児として再インポートされないこと"
    );
    assert_eq!(
        sym_paste_app.session.shapes.len(),
        4,
        "コピー後に編集された最新シンボル定義（2シェイプ×2インスタンス＝4シェイプ）が展開されること"
    );
    assert!(sym_paste_app
        .session
        .shapes
        .iter()
        .all(|s| s.color == Color32::BLUE));
}

#[test]
fn test_cross_tab_symbol_paste_remaps_inner_group_ids() {
    // 別タブからのシンボルペースト時に内部タイムラインの group_path が再採番され、
    // 貼り付け先タブの既存グループ（gid=1）とデタッチ時にID衝突しないこと、またタブ切り替え時に非アクティブスロットへ clipboard が残留しないこと
    let tmp_cross_tab =
        std::env::temp_dir().join(format!("is2d_cross_tab_gid_{}.is2d", std::process::id()));
    let mut empty_doc_app = Is2dApp::default();
    crate::project_io::save_project_to_path(&mut empty_doc_app, &tmp_cross_tab).unwrap();

    let mut cross_gid_app = Is2dApp::default();
    cross_gid_app.session.shapes = make_stroke(
        Point::new(80.0, 80.0),
        Point::new(120.0, 80.0),
        10.0,
        Color32::RED,
    );
    cross_gid_app.session.set_selected_indices(vec![0]);
    let tab0_inner_gid = tool_select::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab0_inner_gid, 1);
    cross_gid_app.session.set_selected_indices(vec![0]);
    let _tab0_sym_id = tool_transform::convert_selection_to_symbol(&mut cross_gid_app).unwrap();
    cross_gid_app.session.set_selected_indices(vec![0]);
    assert!(tool_select::copy_selected_shapes(&mut cross_gid_app));

    // 2つ目のタブを開き、Tab 1 側でもグループ（gid=1）を作成してから Tab 0 のシンボルをペースト＆グループ解除（デタッチ）
    crate::project_io::open_project_in_tab(&mut cross_gid_app, &tmp_cross_tab, None).unwrap();
    assert!(
        !cross_gid_app.clipboard.shapes.is_empty(),
        "タブ切り替え後もアプリ共通の clipboard が保持されていること"
    );
    cross_gid_app.session.shapes = make_stroke(
        Point::new(260.0, 200.0),
        Point::new(300.0, 200.0),
        10.0,
        Color32::BLUE,
    );
    cross_gid_app.session.set_selected_indices(vec![0]);
    let tab1_existing_gid = tool_select::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab1_existing_gid, 1);

    assert!(tool_select::paste_clipboard(&mut cross_gid_app, true));
    assert!(tool_select::ungroup_selected_shapes(&mut cross_gid_app));
    let blue_gid = cross_gid_app
        .session
        .shapes
        .iter()
        .find(|s| s.color == Color32::BLUE)
        .unwrap()
        .group_path[0];
    let red_gid = cross_gid_app
        .session
        .shapes
        .iter()
        .find(|s| s.color == Color32::RED)
        .unwrap()
        .group_path[0];
    assert_ne!(
        blue_gid, red_gid,
        "別タブから移植したシンボル内部のグループIDが再採番され、貼り付け先タブの既存グループとID衝突しないこと"
    );
    let _ = std::fs::remove_file(&tmp_cross_tab);
}

#[test]
fn test_save_while_selected_flushes_merge_drawing_and_preserves_selection() {
    let tmp_merge_path =
        std::env::temp_dir().join(format!("is2d_save_merge_test_{}.is2d", std::process::id()));

    // (1) 生シェイプ移動後の選択中保存（Cmd+S）時の Merge Drawing 確定検証
    let mut merge_app = Is2dApp::default();
    let blue_stroke = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::BLUE,
    );
    let red_stroke = make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    );
    merge_app.session.shapes = blue_stroke;
    merge_app.session.shapes.extend(red_stroke);
    canvas_tool::sync_shapes_to_timeline(&mut merge_app);
    merge_app.session.set_selected_indices(vec![1]);
    let snap = merge_app.session.snapshot_for_drag();
    merge_app.session.translate_selected(KVec2::new(0.0, -80.0));
    tool_select::commit_moved_selection(&mut merge_app, snap);
    assert!(merge_app.session.has_modified_floating());

    crate::project_io::save_project_to_path(&mut merge_app, &tmp_merge_path)
        .expect("選択中の保存に成功すること");
    assert!(!merge_app.session.has_modified_floating());
    assert!(!merge_app.history.is_dirty());
    let shapes_len_on_save = merge_app.session.shapes.len();
    tool_select::clear_selection(&mut merge_app);
    assert_eq!(merge_app.session.shapes.len(), shapes_len_on_save);
    assert!(
        !merge_app.history.is_dirty(),
        "保存後の選択解除でタイムラインが不整合更新されないこと"
    );

    // (2) 非選択の背景同色シェイプと Union 融合して保存された場合は背景を勝手に巻き込まず選択解除され、
    //     選択中のシェイプ同士のみの自己重なり Union 保存では shape_id と選択状態が維持されること
    let mut union_save_app = Is2dApp::default();
    union_save_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::RED,
    );
    union_save_app.session.shapes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut union_save_app);
    union_save_app.session.set_selected_indices(vec![1]);
    let snap_u = union_save_app.session.snapshot_for_drag();
    union_save_app
        .session
        .translate_selected(KVec2::new(0.0, -80.0));
    tool_select::commit_moved_selection(&mut union_save_app, snap_u);
    crate::project_io::save_project_to_path(&mut union_save_app, &tmp_merge_path)
        .expect("同色Union保存に成功すること");
    assert_eq!(
        union_save_app.session.shapes.len(),
        1,
        "同色シェイプが1つにUnion結合されていること"
    );
    assert!(
        union_save_app.session.selection.is_empty(),
        "非選択の背景同色シェイプとUnion融合して保存した場合は背景シェイプが勝手に選択に巻き込まれないこと"
    );

    let mut self_union_app = Is2dApp::default();
    self_union_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        16.0,
        Color32::RED,
    );
    self_union_app.session.shapes.extend(make_stroke(
        Point::new(150.0, 80.0),
        Point::new(150.0, 120.0),
        16.0,
        Color32::RED,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut self_union_app);
    self_union_app.session.set_selected_indices(vec![0, 1]);
    assert!(tool_select::copy_selected_shapes(&mut self_union_app));
    let snap_su = self_union_app.session.snapshot_for_drag();
    self_union_app
        .session
        .translate_selected(KVec2::new(10.0, 0.0));
    tool_select::commit_moved_selection(&mut self_union_app, snap_su);
    crate::project_io::save_project_to_path(&mut self_union_app, &tmp_merge_path)
        .expect("選択同士のUnion保存に成功すること");
    assert_eq!(self_union_app.session.shapes.len(), 1);
    assert_eq!(
        self_union_app.session.selected_indices(),
        vec![0],
        "選択シェイプ同士のみのUnion結合では shape_id が引き継がれ選択状態が維持されること"
    );

    let _ = std::fs::remove_file(&tmp_merge_path);
}

#[test]
fn test_save_io_failure_rollback_and_marquee_unsliced_push_undo() {
    // 全レイヤーロック中は paste_clipboard が拒否され、矩形マーキースライス直後（未編集）に push_undo() が呼ばれても分割断片が早期確定されないこと
    let mut self_union_app = Is2dApp::default();
    self_union_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        16.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut self_union_app);
    self_union_app.session.set_selected_indices(vec![0]);
    assert!(tool_select::copy_selected_shapes(&mut self_union_app));

    timeline_ui::set_layer_locked(&mut self_union_app, 0, true);
    assert!(
        !tool_select::paste_clipboard(&mut self_union_app, true),
        "ロックされたレイヤー上へのペーストが拒否されること"
    );
    timeline_ui::set_layer_locked(&mut self_union_app, 0, false);

    tool_select::clear_selection(&mut self_union_app);
    tool_select::apply_marquee_selection(
        &mut self_union_app,
        Point::new(90.0, 60.0),
        Point::new(150.0, 140.0),
        false,
    );
    assert!(self_union_app.session.has_unsliced_snapshot());
    assert!(self_union_app.session.shapes.len() >= 2);
    doc_actions::apply(
        &mut self_union_app,
        doc_actions::Action::SetStageSize {
            size: Vec2::new(810.0, 450.0),
        },
    );
    assert_eq!(
        self_union_app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes
            .len(),
        1,
        "マーキースライス未編集状態で push_undo が呼ばれてもタイムライン側の分割前1シェイプが維持されること"
    );
    tool_select::clear_selection(&mut self_union_app);
    assert_eq!(self_union_app.session.shapes.len(), 1);

    // 存在しないディレクトリへの保存（I/O失敗）時のロールバックと、Floating 保持状態からの Undo -> Redo マージ確定
    let mut undo_redo_merge_app = Is2dApp::default();
    undo_redo_merge_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::RED,
    );
    undo_redo_merge_app.session.shapes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut undo_redo_merge_app);
    undo_redo_merge_app.session.set_selected_indices(vec![1]);
    let snap_ur = undo_redo_merge_app.session.snapshot_for_drag();
    undo_redo_merge_app
        .session
        .translate_selected(KVec2::new(0.0, -80.0));
    tool_select::commit_moved_selection(&mut undo_redo_merge_app, snap_ur);
    assert!(undo_redo_merge_app.session.has_modified_floating());

    let bad_path = std::env::temp_dir()
        .join("is2d_nonexistent_dir_for_test_999999")
        .join("fail.is2d");
    assert!(crate::project_io::save_project_to_path(&mut undo_redo_merge_app, &bad_path).is_err());
    assert_eq!(
        undo_redo_merge_app.session.shapes.len(),
        2,
        "保存I/O失敗時はライブキャンバス上の未マージ図形が勝手に結合されないこと"
    );
    assert!(undo_redo_merge_app.session.has_modified_floating());

    undo_redo_merge_app.undo();
    assert_eq!(undo_redo_merge_app.session.shapes.len(), 2);
    undo_redo_merge_app.redo();
    assert_eq!(
        undo_redo_merge_app.session.shapes.len(),
        1,
        "Floating 変更済み状態からの Undo -> Redo 後もマージ描画（Union）が確定維持されること"
    );
}

#[test]
fn test_corrupt_and_truncated_json_returns_error_without_modifying_app() {
    let tmp_corrupt =
        std::env::temp_dir().join(format!("is2d_corrupt_test_{}.is2d", std::process::id()));
    let mut app = Is2dApp::default();
    app.doc.stage_size = Vec2::new(960.0, 540.0);

    // 1. 途中で切れた不正な JSON 構文
    std::fs::write(&tmp_corrupt, r#"{"version": 1, "stage_size": [800.0,"#).unwrap();
    let res1 = crate::project_io::open_project_in_tab(&mut app, &tmp_corrupt, None);
    assert!(
        res1.is_err(),
        "途中で切断された壊れた JSON はパニックせず Err を返すこと"
    );
    assert_eq!(
        app.doc.stage_size,
        Vec2::new(960.0, 540.0),
        "読み込みエラー時に既存タブの状態が破壊されないこと"
    );

    // 2. 空ファイル
    std::fs::write(&tmp_corrupt, "").unwrap();
    assert!(crate::project_io::open_project_in_tab(&mut app, &tmp_corrupt, None).is_err());

    // 3. 未来の未対応バージョン番号
    std::fs::write(
        &tmp_corrupt,
        r#"{"version": 999, "stage_size": [800.0, 450.0], "timeline": {"fps": 24, "total_frames": 1, "active_layer": 0, "current_frame": 0, "next_layer_id": 2, "layers": []}}"#,
    )
    .unwrap();
    assert!(
        crate::project_io::open_project_in_tab(&mut app, &tmp_corrupt, None).is_err(),
        "未対応の未来バージョン番号は Err を返すこと"
    );

    let _ = std::fs::remove_file(&tmp_corrupt);
}

#[test]
fn test_malformed_json_sanitization_and_cycle_breaking() {
    // 破損・外部改変された異常系 `.is2d` JSON（シンボル循環参照 A⇄B・重複レイヤーID・孤児インスタンス・逆向きワインディング・範囲外パラメータ）のサニタイズ検証
    let malformed_json = r#"{
          "version": 1,
          "stage_size": [99999.0, 10.0],
          "next_symbol_id": 1,
          "next_group_id": 1,
          "timeline": {
            "fps": 999,
            "total_frames": 2,
            "active_layer": 99,
            "current_frame": 99,
            "next_layer_id": 1,
            "layers": [
              {
                "id": 5,
                "name": "Dup A",
                "visible": true,
                "locked": false,
                "frame_count": 2,
                "keyframes": [
                  {
                    "start_frame": 0,
                    "tween": true,
                    "tween_rotate": "Auto",
                    "shapes": [
                      {
                        "color": [255, 0, 0, 255],
                        "exterior": {
                          "segments": [
                            [[100.0, 100.0], [100.0, 150.0], [100.0, 200.0]],
                            [[100.0, 200.0], [150.0, 200.0], [200.0, 200.0]],
                            [[200.0, 200.0], [150.0, 150.0], [100.0, 100.0]]
                          ]
                        },
                        "holes": [],
                        "layer_id": 999,
                        "group_path": []
                      }
                    ],
                    "instances": [
                      {
                        "binding": {
                          "instance_gid": 10,
                          "symbol_id": 1,
                          "xform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                          "pivot": [0.0, 0.0],
                          "loop_mode": "Loop",
                          "first_frame": 0
                        },
                        "layer_id": 5,
                        "parent_scope": [],
                        "shape_order_index": 0
                      },
                      {
                        "binding": {
                          "instance_gid": 11,
                          "symbol_id": 9999,
                          "xform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                          "pivot": [0.0, 0.0],
                          "loop_mode": "Loop",
                          "first_frame": 0
                        },
                        "layer_id": 5,
                        "parent_scope": [],
                        "shape_order_index": 1
                      }
                    ]
                  }
                ]
              },
              {
                "id": 5,
                "name": "Dup B",
                "visible": true,
                "locked": false,
                "frame_count": 2,
                "keyframes": []
              }
            ]
          },
          "library": [
            {
              "id": 1,
              "name": "CycleSymA",
              "timeline": {
                "fps": 24,
                "total_frames": 1,
                "active_layer": 0,
                "current_frame": 0,
                "next_layer_id": 2,
                "layers": [
                  {
                    "id": 1,
                    "name": "L1",
                    "visible": true,
                    "locked": false,
                    "frame_count": 1,
                    "keyframes": [
                      {
                        "start_frame": 0,
                        "tween": false,
                        "tween_rotate": "Auto",
                        "shapes": [],
                        "instances": [
                          {
                            "binding": {
                              "instance_gid": 20,
                              "symbol_id": 2,
                              "xform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                              "pivot": [0.0, 0.0],
                              "loop_mode": "Loop",
                              "first_frame": 0
                            },
                            "layer_id": 1,
                            "parent_scope": [],
                            "shape_order_index": 0
                          }
                        ]
                      }
                    ]
                  }
                ]
              }
            },
            {
              "id": 2,
              "name": "CycleSymB",
              "timeline": {
                "fps": 24,
                "total_frames": 1,
                "active_layer": 0,
                "current_frame": 0,
                "next_layer_id": 2,
                "layers": [
                  {
                    "id": 1,
                    "name": "L1",
                    "visible": true,
                    "locked": false,
                    "frame_count": 1,
                    "keyframes": [
                      {
                        "start_frame": 0,
                        "tween": false,
                        "tween_rotate": "Auto",
                        "shapes": [],
                        "instances": [
                          {
                            "binding": {
                              "instance_gid": 21,
                              "symbol_id": 1,
                              "xform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                              "pivot": [0.0, 0.0],
                              "loop_mode": "Loop",
                              "first_frame": 0
                            },
                            "layer_id": 1,
                            "parent_scope": [],
                            "shape_order_index": 0
                          }
                        ]
                      }
                    ]
                  }
                ]
              }
            }
          ]
        }"#;
    let tmp_malformed =
        std::env::temp_dir().join(format!("is2d_malformed_test_{}.is2d", std::process::id()));
    std::fs::write(&tmp_malformed, malformed_json).unwrap();
    let mut sanitized_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut sanitized_app, &tmp_malformed, None)
        .expect("異常系JSONでもサニタイズされてクラッシュせず読み込めること");
    assert_eq!(sanitized_app.doc.stage_size, Vec2::new(3840.0, 100.0));
    assert_eq!(sanitized_app.doc.root.fps, 60);
    assert_eq!(
        sanitized_app.doc.root.active_layer, 1,
        "範囲外の active_layer=99 が最終レイヤーインデックス 1 にクランプされていること"
    );
    assert_eq!(
        sanitized_app.doc.root.current_frame, 1,
        "範囲外の current_frame=99 が最終フレームインデックス 1 にクランプされていること"
    );
    assert_ne!(
        sanitized_app.doc.root.layers[0].id, sanitized_app.doc.root.layers[1].id,
        "重複していた layer_id が一意に再採番されていること"
    );
    assert_eq!(
        sanitized_app.doc.root.layers[0].keyframes[0].cel.shapes[0].layer_id,
        sanitized_app.doc.root.layers[0].id,
        "不正だった shape.layer_id=999 が所属レイヤーの layer_id に正規化されていること"
    );
    assert!(
        !sanitized_app.doc.root.layers[0].keyframes[0].tween,
        "生シェイプとシンボルが混在し単一シンボル条件を満たさないキーフレームの tween=true が false に正規化されていること"
    );
    assert_eq!(
        sanitized_app.doc.root.layers[1].keyframes.len(),
        1,
        "空の keyframes 配列だったレイヤーに start_frame=0 の空白キーフレームが自動補填されていること"
    );
    assert_eq!(
        sanitized_app.doc.root.layers[0].keyframes[0]
            .cel
            .instances
            .len(),
        1,
        "存在しない symbol_id=9999 の孤児インスタンスが除去されていること"
    );
    assert!(
        sanitized_app.doc.root.layers[0].keyframes[0].cel.shapes[0]
            .exterior
            .signed_area()
            > 0.0,
        "逆向き（負面積）だった外周輪郭のワインディングが正面積に正規化されていること"
    );
    assert!(
        !sanitized_app.doc.would_create_symbol_cycle(1, 2)
            || !sanitized_app.doc.would_create_symbol_cycle(2, 1),
        "シンボル間の循環参照 (1 <-> 2) が自動で切断され DAG が保証されていること"
    );

    let _ = std::fs::remove_file(&tmp_malformed);
}

#[test]
fn test_eraser_only_erases_active_layer_and_preserves_other_unlocked_layers() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 100, 220);

    // Layer 1 (index 0, id 1) に赤い横線を描画
    app.tool.tool = Tool::Brush;
    app.tool.brush_color = red;
    let raw_red =
        generate_brush_outline(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 14.0);
    tool_paint::commit_raw_outline(&mut app, &raw_red);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 1);

    // Layer 2 (index 1, id 2) を追加して同じ座標に青い横線を描画（両レイヤーとも非ロック）
    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.doc.root.active_layer, 1);
    app.tool.brush_color = blue;
    let raw_blue =
        generate_brush_outline(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 14.0);
    tool_paint::commit_raw_outline(&mut app, &raw_blue);
    assert_eq!(app.doc.root.layers[1].keyframes[0].cel.shapes.len(), 1);

    // Layer 2 がアクティブな状態で消しゴムツールを実行 → Layer 2 の青い線のみが削られ、Layer 1 の赤い線は無傷であること
    app.tool.tool = Tool::Eraser;
    let raw_erase =
        generate_brush_outline(&[Point::new(200.0, 150.0), Point::new(200.0, 250.0)], 16.0);
    tool_paint::commit_raw_outline(&mut app, &raw_erase);

    let layer1_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
    let layer2_shapes = &app.doc.root.layers[1].keyframes[0].cel.shapes;
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
fn test_multilayer_f7_advances_frame_when_any_selected_layer_has_content() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);

    // Layer 1 (index 0) の Frame 0 に図形を描画し、Layer 2 (index 1) は Frame 0 が空白のまま
    app.tool.brush_color = red;
    let raw = generate_brush_outline(&[Point::new(100.0, 100.0), Point::new(160.0, 100.0)], 10.0);
    tool_paint::commit_raw_outline(&mut app, &raw);
    timeline_ui::timeline_add_layer(&mut app);
    assert!(app.doc.root.layers[1].keyframes[0].cel.is_empty());
    assert!(!app.doc.root.layers[0].keyframes[0].cel.is_empty());

    // 両レイヤーの Frame 0 を範囲選択して F7 (空白キーフレーム挿入) を実行
    // → Layer 1 に絵があるため Frame 0 の絵を消去せず Frame 1 に進んで空白キーフレームを挿入すること
    timeline_ui::select_timeline_cells(&mut app, 0, 0, 1, 0);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 1);
    assert!(
        !app.doc.root.layers[0].keyframes[0].cel.is_empty(),
        "複数レイヤー選択時の F7 で Frame 0 の既存の絵が消去されないこと"
    );
}

#[test]
fn test_move_frames_overwrites_destination_and_cross_layer_offset() {
    let red = Color32::from_rgb(220, 50, 50);
    // 移動先に既存キーフレームがある状態でのフレーム区間移動（place_keyframes_at）の検証
    let mut move_app = Is2dApp::default();
    for _ in 0..5 {
        timeline_ui::timeline_extend_frame(&mut move_app);
    }
    // Frame 0..=2 (3コマスパン) に赤い絵を描画し、Frame 3 に空白キーフレーム、Frame 4 に青い絵のキーフレームを作成
    move_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(150.0, 100.0),
        10.0,
        red,
    );
    canvas_tool::sync_shapes_to_timeline(&mut move_app);
    timeline_ui::switch_to_layer_and_frame(&mut move_app, 0, 3);
    timeline_ui::timeline_insert_keyframe(&mut move_app, false);
    timeline_ui::switch_to_layer_and_frame(&mut move_app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut move_app, false);
    move_app.session.shapes = make_stroke(
        Point::new(200.0, 200.0),
        Point::new(250.0, 200.0),
        10.0,
        Color32::BLUE,
    );
    canvas_tool::sync_shapes_to_timeline(&mut move_app);

    // Frame 0..=2 (3コマスパン) を選択して +3 コマ右（Frame 3..=5）へ移動
    // → 移動先 Frame 4 にあった既存キーフレームがクリアされ、Frame 3..=5 全体が移動した赤いスパンになること
    let sel = crate::timeline::TimelineCellSelection::from_corners(0, 0, 0, 2);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut move_app,
        sel,
        0,
        3,
        false
    ));
    let kfs_in_dst: Vec<_> = move_app.doc.root.layers[0]
        .keyframes
        .iter()
        .filter(|kf| (3..=5).contains(&kf.start_frame))
        .collect();
    assert_eq!(
        kfs_in_dst.len(),
        1,
        "移動先範囲 [3..=5] 内の古いキーフレーム (frame 4) が残存せず、移動したキーフレーム (frame 3) のスパンが [3..=5] を保持すること"
    );
    assert_eq!(kfs_in_dst[0].start_frame, 3);
    assert_eq!(
        move_app.doc.root.layers[0].cel_at(4).unwrap().shapes[0].color,
        red
    );

    // 別レイヤー（layer_offset = +1）へのフレーム区間移動で移動先レイヤーの layer_id に更新されること
    timeline_ui::timeline_add_layer(&mut move_app);
    let dst_layer_id = move_app.doc.root.layers[1].id;
    let sel_cross = crate::timeline::TimelineCellSelection::from_corners(0, 3, 0, 5);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut move_app,
        sel_cross,
        1,
        -2,
        false
    ));
    assert!(
        move_app.doc.root.layers[0].cel_at(3).unwrap().is_empty(),
        "移動元レイヤー (Layer 1) の Frame 3..=5 は空白化されていること"
    );
    let moved_cel = move_app.doc.root.layers[1].cel_at(1).unwrap();
    assert_eq!(moved_cel.shapes.len(), 1);
    assert_eq!(moved_cel.shapes[0].color, red);
    assert_eq!(
        moved_cel.shapes[0].layer_id, dst_layer_id,
        "別レイヤーへ移動したキーフレーム内のシェイプは移動先レイヤーの layer_id に更新されていること"
    );
}

#[test]
fn test_double_click_raw_shape_does_not_exit_group_or_symbol() {
    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        14.0,
        Color32::RED,
    );
    app.session.set_selected_indices(vec![0]);
    let gid = tool_select::group_selected_shapes(&mut app).unwrap();

    // グループをダブルクリックしてインプレイス編集に入る
    assert!(tool_select::enter_group_at(
        &mut app,
        Point::new(150.0, 100.0)
    ));
    assert_eq!(app.session.editing_group_path, vec![gid]);

    // グループ内部の生シェイプ上の座標 (150, 100) をダブルクリックしても exit_current_group が呼ばれずグループ編集スコープに留まること
    let pt_on_raw = Point::new(150.0, 100.0);
    tool_select::handle_canvas_double_click(&mut app, pt_on_raw);
    assert_eq!(
        app.session.editing_group_path,
        vec![gid],
        "グループ内の生シェイプ上のダブルクリックでは親階層へ退出しないこと"
    );
    // 空白背景 (400, 400) をダブルクリックすると exit_current_group で親階層へ戻れること
    let pt_empty = Point::new(400.0, 400.0);
    tool_select::handle_canvas_double_click(&mut app, pt_empty);
    assert!(app.session.editing_group_path.is_empty());
}

#[test]
fn test_tween_state_incomplete_when_not_single_symbol_instance() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        14.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.doc.root.layers[0].keyframes[0].tween = true;
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete,
        "tween == true で単一シンボル条件を満たさないキーフレームは未完トゥイーン (TweenState::Incomplete) として扱われること"
    );
}

#[test]
fn test_marquee_shift_deselect_restores_unsliced_for_bucket_fill() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);

    // マーキースライスで1本のストロークを2つに分割選択したあと、Shift+クリック（toggle_shape_selection）で選択を空にした際、
    // 即座に未スライス状態の base が復元され、直後のバケツ塗りで古い未スライス状態が復活しないこと
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(240.0, 100.0),
        16.0,
        red,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(90.0, 70.0),
        Point::new(160.0, 130.0),
        false,
    );
    assert!(app.session.has_unsliced_snapshot());
    assert_eq!(app.session.selected_indices().len(), 1);
    let sel_idx = app.session.selected_indices()[0];
    tool_select::toggle_shape_selection(&mut app, sel_idx);
    assert!(app.session.selection.is_empty());
    assert!(
        !app.session.has_unsliced_snapshot(),
        "Shift+クリックで選択が空になった時点で未編集のマーキースライスは即座に復元されること"
    );
    assert_eq!(app.session.shapes.len(), 1);

    // 直後にバケツツールで赤シェイプを青に塗り替えても正常に1シェイプが青になること
    app.tool.brush_color = blue;
    assert!(tool_paint::bucket_fill_at(
        &mut app,
        Point::new(170.0, 100.0)
    ));
    assert_eq!(app.session.shapes.len(), 1);
    assert_eq!(app.session.shapes[0].color, blue);
}

#[test]
fn test_symbol_rename_consecutive_keystrokes_grouped_into_single_undo() {
    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(240.0, 100.0),
        16.0,
        Color32::BLUE,
    );
    app.session.set_selected_indices(vec![0]);
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    let undo_before_rename = app.history.undo_stack.len();

    // 1文字目の入力開始時のみ Undo を積み、フレーム描画（ensure_valid_selection）を挟んで2文字目を入力しても Undo が増殖しないこと
    assert!(tool_transform::rename_symbol_in_library(&mut app, sym_id, "H"));
    app.ensure_valid_library_selection();
    assert_eq!(
        app.history.last_coalesce_key,
        Some(canvas_tool::CoalesceKey::RenameSymbol(sym_id)),
        "毎フレームの ensure_valid_selection 呼び出しで RenameSymbol の CoalesceKey が消去されないこと"
    );
    assert!(tool_transform::rename_symbol_in_library(&mut app, sym_id, "Hero"));
    assert_eq!(
        app.history.undo_stack.len(),
        undo_before_rename + 1,
        "連続キー入力によるシンボル名変更で Undo スタックが1回分のみ記録されること"
    );
}

#[test]
fn test_step_frame_loop_mode_and_total_frames_over_600() {
    let mut app = Is2dApp::default();

    // 600フレーム超（1200フレーム）のタイムラインリサイズと step_frame のループ ON / OFF 挙動
    app.doc.root.resize_total_frames(1200);
    assert_eq!(app.doc.root.total_frames, 1200);
    assert_eq!(app.doc.root.layers[0].frame_count, 1200);

    // loop_playback == true のとき、先頭フレーム (0) から -1 コマ送りで最終フレーム (1199) へ折り返すこと
    app.timeline_ui.loop_playback = true;
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    timeline_ui::step_frame(&mut app, -1);
    assert_eq!(app.doc.root.current_frame, 1199);
    timeline_ui::step_frame(&mut app, 1);
    assert_eq!(app.doc.root.current_frame, 0);

    // loop_playback == false のとき、先頭フレーム (0) から -1 でも 0 で停止し、最終フレーム (1199) から +1 でも 1199 で停止すること
    app.timeline_ui.loop_playback = false;
    timeline_ui::step_frame(&mut app, -1);
    assert_eq!(app.doc.root.current_frame, 0);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1199);
    timeline_ui::step_frame(&mut app, 1);
    assert_eq!(app.doc.root.current_frame, 1199);
}

#[test]
fn test_viewport_fit_stage_to_window() {
    let mut app = Is2dApp::default();
    app.view.last_canvas_rect =
        eframe::egui::Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1600.0, 900.0));
    app.doc.stage_size = Vec2::new(800.0, 450.0);
    app.fit_stage_to_window();
    assert!(
        app.view.zoom > 1.5 && app.view.zoom <= 2.0,
        "1600x900 キャンバスに対して 800x450 ステージがマージン付きでフィット拡大されること: zoom={}",
        app.view.zoom
    );
    let center = app.view.last_canvas_rect.center();
    let world_before = app.view.screen_to_world(center);
    app.view.zoom_around_screen_pos(center, 1.5);
    let world_after = app.view.screen_to_world(center);
    assert!(
        (world_before - world_after).hypot() < 1e-3,
        "指定スクリーン座標を中心としたズーム後もその座標下のワールド座標がずれないこと"
    );
}

#[test]
fn test_nested_static_child_symbol_is_not_treated_as_dynamic_blank_frame() {
    // シンボル内 Frame 0 が空だがネストした子シンボルインスタンスのみが存在する場合の has_dynamic_animation 判定
    let mut sym_parent = crate::symbol::SymbolDef::new(1, "Parent".to_string(), Vec::new());
    let sym_child = crate::symbol::SymbolDef::new(
        2,
        "Child".to_string(),
        make_stroke(
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
            6.0,
            Color32::RED,
        ),
    );
    sym_parent.timeline.layers[0].keyframes[0].cel = crate::timeline::Cel::new_with_instances(
        Vec::new(),
        vec![crate::symbol::SymbolInstance {
            binding: crate::symbol::SymbolInstanceBinding::new(1, 2, kurbo::Affine::IDENTITY),
            layer_id: 1,
            parent_scope: Vec::new(),
            shape_order_index: 0,
        }],
    );
    let lib = vec![sym_parent, sym_child];
    assert!(
        !lib[0].has_dynamic_animation(&lib, 0),
        "Frame 0 に shapes が無くても単一フレームの子シンボルインスタンスが存在する場合は空白フレームと誤判定されないこと"
    );
}

#[test]
fn test_f7_preserves_next_keyframe_by_pushing_tail() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 100, 220);

    // Frame 0 に赤い絵、Frame 1 に青い絵のキーフレームを作成
    app.session.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 1);
    app.session.shapes = make_stroke(Point::new(200.0, 200.0), Point::new(260.0, 200.0), 10.0, blue);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // Frame 0（赤い絵）に戻って F7 を押したとき、Frame 1 の青い絵を白紙消去せず Frame 2 へ押し出して Frame 1 に空白KFを挿入すること (H4)
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 1);
    assert!(
        app.doc.root.layers[0].cel_at(1).unwrap().is_empty(),
        "F7 で自動進行した Frame 1 に空白キーフレームが挿入されること"
    );
    assert_eq!(
        app.doc.root.layers[0].cel_at(2).unwrap().shapes[0].color,
        blue,
        "Frame 1 にあった既存の青いキーフレームが消去されず Frame 2 に保持されていること"
    );
}

#[test]
fn test_shift_f6_and_shift_f5_boundary_guards() {
    let red = Color32::from_rgb(220, 40, 40);
    // 1. Frame 0 の保持フレーム（Frame 1〜）で Shift+F6 を押しても先頭キーフレーム (Frame 0) を誤って白紙化しないこと
    let mut hold_app = Is2dApp::default();
    hold_app.session.shapes =
        make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut hold_app);
    timeline_ui::timeline_extend_frame(&mut hold_app);
    timeline_ui::timeline_extend_frame(&mut hold_app);
    timeline_ui::switch_to_layer_and_frame(&mut hold_app, 0, 2);
    assert!(
        !timeline_ui::timeline_delete_keyframe(&mut hold_app),
        "先頭キーフレームの保持フレーム（Frame 2）では Shift+F6 が無効であり、Frame 0 の絵を消去しないこと"
    );
    assert!(!hold_app.doc.root.layers[0].cel_at(0).unwrap().is_empty());

    // 2. 尺の短いレイヤーが含まれる状態で後ろのフレームで Shift+F5 を押した際、範囲外の短いレイヤーの末尾を削らないこと
    timeline_ui::timeline_add_layer(&mut hold_app);
    hold_app.doc.root.layers[1].frame_count = 1;
    hold_app.doc.root.cell_selection = None;
    hold_app.doc.root.current_frame = 2;
    assert!(timeline_ui::timeline_remove_frame(&mut hold_app));
    assert_eq!(hold_app.doc.root.layers[0].frame_count, 2);
    assert_eq!(
        hold_app.doc.root.layers[1].frame_count, 1,
        "現在フレーム (2) に届いていない短いレイヤー (frame_count=1) は Shift+F5 の影響を受けないこと"
    );
}

#[test]
fn test_loop_symbol_hold_span_f6_and_tween_split_preserves_phase_and_tween_rotate() {
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 100, 220);

    // 複数フレームを持つ Loop シンボルの保持スパン途中で F6 を押した際、表示コマが先頭(0)に巻き戻らないこと
    let mut sym_f6_app = Is2dApp::default();
    sym_f6_app.session.shapes =
        make_stroke(Point::new(100.0, 100.0), Point::new(150.0, 100.0), 10.0, red);
    sym_f6_app.session.set_selected_indices(vec![0]);
    let sym_id = tool_transform::convert_selection_to_symbol(&mut sym_f6_app).unwrap();
    {
        let sym_def = sym_f6_app.doc.find_symbol_mut(sym_id).unwrap();
        sym_def.timeline.resize_total_frames(4);
        sym_def.timeline.layers[0].insert_keyframe(2, false);
        *sym_def.timeline.layers[0].cel_at_mut(2).unwrap().shapes_mut() = make_stroke(
            Point::new(-20.0, 0.0),
            Point::new(20.0, 0.0),
            10.0,
            blue,
        );
    }
    sym_f6_app.doc.root.resize_total_frames(5);
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 2);
    assert_eq!(sym_f6_app.session.shapes[0].color, blue);
    assert!(timeline_ui::timeline_insert_keyframe(&mut sym_f6_app, true));
    assert_eq!(
        sym_f6_app.session.shapes[0].color,
        blue,
        "F6 で分割された新しいキーフレームでも Loop シンボルの表示コマ (Frame 2 = 青) がそのまま引き継がれること"
    );

    // トゥイーン区間（tween_rotate = Clockwise）の途中フレームでキャンバス編集（write_shapes_to_editable_cels）により自動分割された際、
    // 新設された中間キーフレームにも tween_rotate = Clockwise が引き継がれること (#10)
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 4);
    assert!(timeline_ui::timeline_insert_keyframe(&mut sym_f6_app, true));
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 2);
    assert!(tool_transform::timeline_toggle_tween(&mut sym_f6_app));
    assert!(tool_transform::set_active_keyframe_tween_rotate(
        &mut sym_f6_app,
        crate::symbol::TweenRotate::Clockwise
    ));
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 3);
    sym_f6_app.session.set_selected_indices(vec![0]);
    assert!(tool_select::nudge_selected_shapes(
        &mut sym_f6_app,
        kurbo::Vec2::new(20.0, 0.0)
    ));
    let kf_at_3 = sym_f6_app.doc.root.layers[0]
        .keyframes
        .iter()
        .find(|kf| kf.start_frame == 3)
        .expect("Frame 3 に新キーフレームが自動分割生成されていること");
    assert!(kf_at_3.tween);
    assert_eq!(
        kf_at_3.tween_rotate,
        crate::symbol::TweenRotate::Clockwise,
        "トゥイーン区間途中で自動分割されたキーフレームに前のキーフレームの tween_rotate 設定が引き継がれること"
    );
}

#[test]
fn test_flip_tween_midpoint_and_horizontal_flip_without_spurious_rotation() {
    use crate::symbol::{interpolate_affine_with_pivot, TweenRotate};

    // 1. 垂直反転 (scale_non_uniform(1.0, -1.0)) を含むトゥイーンの中間フレーム (t = 0.5) でも特異行列にならずピボット位置へ変換されること (H5)
    let a0 = kurbo::Affine::translate((100.0, 100.0));
    let a1 = kurbo::Affine::translate((300.0, 100.0)) * kurbo::Affine::scale_non_uniform(1.0, -1.0);
    let (mid, _) = interpolate_affine_with_pivot(
        a0,
        Point::ZERO,
        a1,
        Point::ZERO,
        TweenRotate::Auto,
        0.5,
    );
    assert!(
        mid.determinant().abs() > 1e-9,
        "反転トゥイーンの中間 (t=0.5) でも行列式が非ゼロに保たれること"
    );
    let mid_origin = mid * Point::ZERO;
    assert!(
        (mid_origin.x - 200.0).abs() < 1e-3 && (mid_origin.y - 100.0).abs() < 1e-3,
        "t=0.5 でローカル原点 (0,0) に放置されず補間位置 (200, 100) に配置されること: {:?}",
        mid_origin
    );

    // 2. 水平反転 (scale_non_uniform(-1.0, 1.0)) を含むトゥイーンで不要な180°回転が発生せず、Y軸方向が反転せず X スケールのみ補間されること (#4)
    let h1 = kurbo::Affine::translate((300.0, 100.0)) * kurbo::Affine::scale_non_uniform(-1.0, 1.0);
    let (mid_h, _) = interpolate_affine_with_pivot(
        a0,
        Point::ZERO,
        h1,
        Point::ZERO,
        TweenRotate::Auto,
        0.25,
    );
    let top_pt = mid_h * Point::new(0.0, -20.0);
    assert!(
        (top_pt.y - 80.0).abs() < 1e-3,
        "水平反転 (scale_x = -1, scale_y = +1) のトゥイーン途中で 180° 回転に誤分解されて上下が揺れないこと: top_pt={:?}",
        top_pt
    );
}

#[test]
fn test_symbol_delete_mid_tween_preserves_start_keyframe_pose() {
    // トゥイーンの途中フレームでシンボルをライブラリ削除した際、開始キーフレームが途中フレームの姿勢で上書きされないこと (H6)
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 100, 220);
    app.session.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 10.0, red);
    app.session.set_selected_indices(vec![0]);
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    app.doc.root.resize_total_frames(5);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    app.session.set_selected_indices(vec![0]);
    let snap = app.session.snapshot_for_drag();
    app.session.translate_selected(KVec2::new(200.0, 0.0));
    tool_select::commit_moved_selection(&mut app, snap);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert!(tool_transform::timeline_toggle_tween(&mut app));

    // 中間フレーム (Frame 2, x ≈ 220) に移動し、別レイヤーを追加した上でシンボルを削除して別レイヤーに描画
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 1, 2);
    assert!(tool_transform::delete_symbol_from_library(&mut app, sym_id));
    let raw_blue =
        generate_brush_outline(&[Point::new(50.0, 300.0), Point::new(90.0, 300.0)], 8.0);
    app.tool.brush_color = blue;
    tool_paint::commit_raw_outline(&mut app, &raw_blue);

    let kf0_bbox = app.doc.root.layers[0].keyframes[0].cel.shapes[0].bounding_box();
    assert!(
        (kf0_bbox.center().x - 120.0).abs() < 2.0,
        "中間フレームでシンボル削除後に別レイヤーへ描画しても、Layer 1 の開始キーフレーム (Frame 0) の座標 (x≈120) が中間姿勢 (x≈220) で上書きされないこと: center={:?}",
        kf0_bbox.center()
    );
}

#[test]
fn test_noop_eraser_does_not_push_undo_and_active_layer_lock_blocks_paint_tools() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    app.session.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);

    // 図形に当たらない空振り消しゴムで Undo が増えないこと (H7)
    let undo_before_noop_erase = app.history.undo_stack.len();
    app.tool.tool = Tool::Eraser;
    let miss_erase =
        generate_brush_outline(&[Point::new(700.0, 400.0), Point::new(720.0, 400.0)], 10.0);
    tool_paint::commit_raw_outline(&mut app, &miss_erase);
    assert_eq!(
        app.history.undo_stack.len(),
        undo_before_noop_erase,
        "図形に当たらない空振り消しゴム操作では Undo 履歴が積まれないこと"
    );

    // アクティブレイヤーがロック中は消しゴム・バケツが別レイヤーへフォールバックせず拒否されること
    timeline_ui::set_layer_locked(&mut app, 1, true);
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

#[test]
fn test_compact_json_serialization_nan_rejection_and_undo_history_cap() {
    let red = Color32::from_rgb(220, 40, 40);
    let tmp_compact =
        std::env::temp_dir().join(format!("is2d_compact_test_{}.is2d", std::process::id()));
    let mut save_app = Is2dApp::default();
    save_app.session.shapes =
        make_stroke(Point::new(50.0, 50.0), Point::new(100.0, 50.0), 8.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut save_app);
    crate::project_io::save_project_to_path(&mut save_app, &tmp_compact).unwrap();
    let saved_str = std::fs::read_to_string(&tmp_compact).unwrap();
    assert!(
        !saved_str.contains('\n'),
        "保存される .is2d ファイルは膨張を防ぐコンパクト JSON（改行なし）であること"
    );
    save_app.doc.stage_size = Vec2::new(f32::NAN, 450.0);
    assert!(
        crate::project_io::save_project_to_path(&mut save_app, &tmp_compact).is_err(),
        "NaN が含まれる状態では保存前にエラーとして検出され壊れた JSON を書き出さないこと"
    );
    save_app.doc.stage_size = Vec2::new(800.0, 450.0);
    let _ = std::fs::remove_file(&tmp_compact);

    for i in 0..110 {
        doc_actions::apply(&mut save_app, doc_actions::Action::EndCoalesce);
        doc_actions::apply(
            &mut save_app,
            doc_actions::Action::SetStageSize {
                size: Vec2::new(800.0 + (i as f32), 450.0),
            },
        );
    }
    assert_eq!(
        save_app.history.undo_stack.len(),
        crate::MAX_UNDO_HISTORY,
        "Undo 履歴スタックが MAX_UNDO_HISTORY (100件) で上限管理されること"
    );
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
    assert_eq!(app.session.shapes.len(), 1);
    assert!(
        app.session.shapes[0].contains_point(expected_start_world),
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
        app.session.shapes.len(),
        1,
        "右クリックドラッグではブラシストロークが描画されないこと"
    );
}

#[test]
fn test_egui_shortcuts_clipboard_sync_and_cmd0_preserves_ui_zoom_factor() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(180.0, 100.0),
        12.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.session.set_selected_indices(vec![0]);

    // 1. コピー時に OS クリップボードを汚染せず、OS クリップボード非依存で Cmd+V が機能すること (#15)
    let out_copy = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Copy],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        out_copy.platform_output.copied_text.is_empty(),
        "図形コピー時に OS クリップボードをダミー文字列で上書きしないこと (#15)"
    );

    // Event::Paste でペーストが実行され図形が2つになること
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Paste(String::new())],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(app.session.shapes.len(), 2);

    // OS クリップボードが空で egui-winit が押下時の Event::Paste / Key(V, pressed:true) を抑止した場合でも、
    // Cmd+V のキーリリース (pressed: false) フォールバックで確実にペーストされること (#15)
    app.clipboard.paste_triggered_on_press = false;
    let cmd_mods_v = egui::Modifiers {
        command: true,
        ctrl: true,
        ..Default::default()
    };
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods_v,
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: cmd_mods_v,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(
        app.clipboard.paste_count, 2,
        "OS クリップボードが空で押下イベントが吸収されても Cmd+V リリース時にペーストされること (#15)"
    );
    assert!(
        (app.session.shapes[1].bounding_box().center().x - (140.0 + 32.0)).abs() < 2.0,
        "2回目のペースト図形が +32px オフセット位置に実際に配置されていること (#15)"
    );

    // V より先に Cmd を離した場合でも直前フレームの Cmd 猶予により V リリース時にペーストされること (#15)
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods_v,
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: egui::Modifiers::NONE,
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(
        app.clipboard.paste_count, 3,
        "V より先に Cmd を離した場合でも V キーリリースでペーストされること (#15)"
    );
    assert!(
        (app.session.shapes[1].bounding_box().center().x - (140.0 + 48.0)).abs() < 2.0,
        "3回目のペースト図形が +48px オフセット位置に実際に配置されていること (#15)"
    );

    // 2. F8 初回押下でシンボルが1つ作成され、直後のフレームで F8 長押しリピートイベントが来ても2重にシンボル化されないこと
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::F8,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(app.doc.library.len(), 1);

    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::Key {
                    key: egui::Key::F8,
                    physical_key: None,
                    pressed: true,
                    repeat: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Q,
                    physical_key: None,
                    pressed: true,
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
    assert_eq!(
        app.doc.library.len(),
        1,
        "F8 のキーリピート (repeat: true) ではシンボル化が重複暴発しないこと"
    );
    assert_eq!(app.tool.tool, Tool::Transform);

    // 3. Cmd+Plus でキャンバスのみがズームインし egui 全体の UI zoom_factor は 1.0 のまま維持され、Cmd+0 で viewport.zoom が 1.0 にリセットされること (#1)
    let cmd_mods = egui::Modifiers {
        command: true,
        ctrl: true,
        ..Default::default()
    };
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods,
            events: vec![egui::Event::Key {
                key: egui::Key::Plus,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: cmd_mods,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        (app.view.zoom - 1.25).abs() < 1e-6,
        "Cmd+Plus でキャンバスの zoom が 1.25 に拡大されること"
    );
    assert!(
        (ctx.zoom_factor() - 1.0).abs() < 1e-6,
        "zoom_with_keyboard = false により Cmd+Plus でも egui 全体の UI スケールが変更されないこと"
    );
    app.view.pan = Vec2::new(120.0, -80.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods,
            events: vec![egui::Event::Key {
                key: egui::Key::Num0,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: cmd_mods,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        (app.view.zoom - 1.0).abs() < 1e-6 && app.view.pan == Vec2::ZERO,
        "Cmd+0 でキャンバスの zoom が 1.0、pan が ZERO にリセットされること"
    );

    // 4. Esc で選択解除しても現在のツール (Tool::Transform) が勝手に Tool::Select へ切り替わらないこと
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(app.session.selection.is_empty());
    assert_eq!(
        app.tool.tool,
        Tool::Transform,
        "Esc で選択解除しても現在のツール (Tool::Transform) が勝手に Tool::Select へ切り替わらないこと"
    );
}

#[test]
fn test_egui_select_and_transform_tool_raw_input_interactions() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 初回フレームで viewport.last_canvas_rect を確定させる
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );

    // ステージ中央にシェイプを配置
    let center_world = app.view.stage_center_world();
    app.session.shapes = make_stroke(
        Point::new(center_world.x - 40.0, center_world.y),
        Point::new(center_world.x + 40.0, center_world.y),
        20.0,
        Color32::from_rgb(220, 60, 60),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // 1. 選択ツール (Tool::Select) でシェイプ中央を左クリックして選択し、右クリックドラッグでは移動しないことを検証
    app.tool.tool = Tool::Select;
    let center_screen = app.view.world_to_screen(center_world);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(center_screen),
                egui::Event::PointerButton {
                    pos: center_screen,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: center_screen,
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
        app.session.selected_indices(),
        vec![0],
        "handle_select_tool_input 経由の左クリックでシェイプが選択されること"
    );

    // 2. 自由変形ツール (Tool::Transform) でシェイプ内部（ピボットから離れた位置）を左ドラッグして平行移動できること
    app.tool.tool = Tool::Transform;
    let initial_bbox_center = app.session.shapes[0].bounding_box().center();
    let grab_world = Point::new(initial_bbox_center.x - 20.0, initial_bbox_center.y);
    let grab_screen = app.view.world_to_screen(grab_world);
    let drag_to = Pos2::new(grab_screen.x + 50.0, grab_screen.y);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(grab_screen),
                egui::Event::PointerButton {
                    pos: grab_screen,
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
            events: vec![egui::Event::PointerMoved(drag_to)],
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
                pos: drag_to,
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
    let moved_center = app.session.shapes[0].bounding_box().center();
    let expected_dx = 50.0 / (app.view.zoom as f64);
    assert!(
        (moved_center.x - (initial_bbox_center.x + expected_dx)).abs() < 2.0,
        "handle_transform_tool_input 経由の左ドラッグでシェイプが平行移動されること: moved_center={:?}, expected_x={}",
        moved_center,
        initial_bbox_center.x + expected_dx
    );
}

#[test]
fn test_hidden_layer_shapes_excluded_from_canvas_and_preserved_on_sync() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 120, 220);

    // Layer 1 に赤い図形、Layer 2 に青い図形を作成
    app.session.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::timeline_add_layer(&mut app);
    let lid2 = app.doc.root.layers[1].id;
    app.session.shapes.extend(
        make_stroke(Point::new(100.0, 200.0), Point::new(160.0, 200.0), 10.0, blue)
            .into_iter()
            .map(|s| s.with_layer_id(lid2)),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert_eq!(app.session.shapes.len(), 2);

    // Layer 1 を非表示 (visible = false) に切り替えると、CanvasState::shapes から除外されること
    timeline_ui::set_layer_visible(&mut app, 0, false);
    assert_eq!(
        app.session.shapes.len(),
        1,
        "非表示レイヤー (visible=false) の図形は load_shapes_from_timeline から除外されること"
    );
    assert_eq!(app.session.shapes[0].color, blue);

    // その状態で Layer 2 を編集して sync_shapes_to_timeline を実行しても、非表示の Layer 1 の図形が消去されないこと
    app.session.set_selected_indices(vec![0]);
    assert!(tool_select::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(20.0, 0.0)
    ));
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes.len(),
        1,
        "非表示レイヤーが除外された状態で他レイヤーを編集しても非表示レイヤーのセル内容は保護されること"
    );

    // 再び Layer 1 を表示に戻すと赤い図形が復活すること
    timeline_ui::set_layer_visible(&mut app, 0, true);
    assert_eq!(app.session.shapes.len(), 2);
}

#[test]
fn test_time_based_playback_advance_and_onion_skin_rendering() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 3フレームのアニメーションを作成し、オニオンスキンを ON にする
    app.doc.root.fps = 10; // 1コマ = 0.1秒
    app.doc.root.resize_total_frames(3);
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(150.0, 100.0),
        10.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    app.session.shapes = make_stroke(
        Point::new(150.0, 100.0),
        Point::new(200.0, 100.0),
        10.0,
        Color32::GREEN,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    app.session.shapes = make_stroke(
        Point::new(200.0, 100.0),
        Point::new(250.0, 100.0),
        10.0,
        Color32::BLUE,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // Frame 1 でオニオンスキンを有効化して update_ui を実行し、前後のオニオンスキンキャッシュが構築・描画されることを検証
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    app.view.onion_skin_enabled = true;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            time: Some(1.0),
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );

    // 非ループ再生 (loop_playback = false) で Frame 0 から再生を開始し、時刻を進めて最終フレーム (Frame 2) で自動停止することを検証
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.timeline_ui.loop_playback = false;
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);

    // t = 1.0s で再生開始基準時刻を記録
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            time: Some(1.0),
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(app.doc.root.current_frame, 0);

    // t = 1.12s (+0.12s -> 1コマ進行)
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            time: Some(1.12),
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(app.doc.root.current_frame, 1);
    assert!(app.timeline_ui.is_playing);

    // t = 1.35s (最終フレーム Frame 2 に到達して自動停止)
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            time: Some(1.35),
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert_eq!(app.doc.root.current_frame, 2);
    assert!(
        !app.timeline_ui.is_playing,
        "loop_playback == false のとき最終フレーム到達で is_playing が false になり自動停止すること"
    );
}

#[test]
fn test_pull_edge_near_endpoint_and_off_center_does_not_jump_on_drag_start() {
    use kurbo::ParamCurve;

    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        20.0,
        Color32::from_rgb(220, 60, 60),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // 最も長いセグメントを選び、端点付近 (t = 0.05 < 0.18) かつ曲線から少し外れた座標でドラッグ開始 (#5)
    let seg_idx = app.session.shapes[0]
        .exterior
        .segments
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            (a.p2 - a.p0)
                .hypot()
                .partial_cmp(&(b.p2 - b.p0).hypot())
                .unwrap()
        })
        .map(|(i, _)| i)
        .unwrap();
    let orig_seg = app.session.shapes[0].exterior.segments[seg_idx];
    let pt_near_end = orig_seg.eval(0.10) + kurbo::Vec2::new(0.0, 2.0);

    tool_select::start_reshape_drag(&mut app, pt_near_end, false, false);
    assert!(
        matches!(
            app.session.drag,
            canvas_tool::ActiveCanvasDrag::Select(canvas_tool::SelectDragState::PullEdge { .. })
        ),
        "エッジ端点付近のドラッグ開始で PullEdge 状態に入ること"
    );
    tool_select::update_reshape_drag(&mut app, pt_near_end);
    let after_zero_move = app.session.shapes[0].exterior.segments[seg_idx];
    assert!(
        (after_zero_move.p1 - orig_seg.p1).hypot() < 1e-6,
        "端点付近 (t=0.10 < 0.18) や曲線から少し離れた位置を掴んだ瞬間 (移動量0) に曲線が跳ばないこと (#5): orig={:?}, after={:?}",
        orig_seg.p1,
        after_zero_move.p1
    );

    // 微小移動 (dy = +2.0) でも跳びなく滑らかに移動すること
    let grab_t = 0.18_f64;
    let grab_before = orig_seg.eval(grab_t);
    tool_select::update_reshape_drag(&mut app, pt_near_end + kurbo::Vec2::new(0.0, 2.0));
    let grab_after = app.session.shapes[0].exterior.segments[seg_idx].eval(grab_t);
    assert!(
        ((grab_after - grab_before) - kurbo::Vec2::new(0.0, 2.0)).hypot() < 1e-5,
        "微小ドラッグ時の曲線変化量がカーソル移動量と正確に一致すること (#5)"
    );
}

#[test]
fn test_shift_partial_deselect_only_merges_deselected_raw_shape() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);

    // 背景に太い青い図形を2つ配置し、その横に小さな赤い図形を2つ配置
    let mut shapes = Vec::new();
    shapes.extend(make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        28.0,
        blue,
    ));
    shapes.extend(make_stroke(
        Point::new(100.0, 200.0),
        Point::new(160.0, 200.0),
        28.0,
        blue,
    ));
    shapes.extend(make_stroke(
        Point::new(225.0, 88.0),
        Point::new(235.0, 88.0),
        10.0,
        red,
    ));
    shapes.extend(make_stroke(
        Point::new(225.0, 188.0),
        Point::new(235.0, 188.0),
        10.0,
        red,
    ));
    app.session.shapes = shapes;
    canvas_tool::sync_shapes_to_timeline(&mut app);

    let area_blue_top_before = app.session.shapes[0].exterior.signed_area();
    let area_blue_bottom_before = app.session.shapes[1].exterior.signed_area();

    // 赤い図形2つ (index 2, 3) を選択して左へ -100px 移動し、2つの青い図形の上にそれぞれ重ねる
    app.session.set_selected_indices(vec![2, 3]);
    assert!(tool_select::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(-100.0, 0.0)
    ));
    assert!(app.session.has_modified_floating());

    // Shift+クリック相当の toggle_shape_selection で、上側の赤い図形 (index 2) だけを選択解除し、下側の赤い図形 (index 3) は選択に残す (#7)
    tool_select::toggle_shape_selection(&mut app, 2);
    assert_eq!(app.session.selected_indices().len(), 1);
    assert!(
        app.session.has_modified_floating(),
        "一部の図形だけを選択解除した際、残りの選択図形は変更済み Floating のまま維持されること (#7)"
    );

    // 選択に残した下側の赤い図形をさらに右へ +100px 戻してから全選択解除する
    let remaining_idx = app.session.selected_indices()[0];
    assert_eq!(app.session.shapes[remaining_idx].color, red);
    assert!(tool_select::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(100.0, 0.0)
    ));
    tool_select::clear_selection(&mut app);

    // 結果：先に選択解除された上側の赤い図形は上側の青い図形を切り抜いているが、
    // 選択に残して後から退かした下側の赤い図形は下側の青い図形を切り抜いていないこと！
    let blue_shapes: Vec<&FillShape> = app
        .session
        .shapes
        .iter()
        .filter(|s| s.color == blue)
        .collect();
    assert_eq!(blue_shapes.len(), 2);
    let top_blue = blue_shapes
        .iter()
        .find(|s| s.bounding_box().center().y < 150.0)
        .unwrap();
    let bottom_blue = blue_shapes
        .iter()
        .find(|s| s.bounding_box().center().y > 150.0)
        .unwrap();
    assert_eq!(
        top_blue.holes.len(),
        1,
        "選択解除された上側の赤い図形は背景の青い図形を即座に切り抜いて穴を開けていること (#7)"
    );
    assert!(
        bottom_blue.holes.is_empty()
            && (bottom_blue.exterior.signed_area() - area_blue_bottom_before).abs() < 1.0,
        "選択に残していた下側の赤い図形は途中位置で背景の青い図形を切り抜いてしまわないこと (#7)"
    );
    let _ = area_blue_top_before;
}

#[test]
fn test_pivot_commit_restores_unsliced_marquee_snapshot_and_persists() {
    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(220.0, 100.0),
        20.0,
        Color32::from_rgb(40, 150, 90),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // 1. 生シェイプを矩形マーキーで部分スライス選択した直後、変形せずにピボットだけ動かして確定した場合、
    // スライスの切れ目がタイムラインに書き込まれず、選択解除で元の1図形に戻ること (#8)
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 80.0),
        Point::new(160.0, 120.0),
        false,
    );
    assert!(app.session.has_unsliced_snapshot());
    assert_eq!(app.session.shapes.len(), 2);

    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Transform });
    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();
    let current_pivot = app.scene().active_transform_pivot(
        &app.session.selection,
        app.session.transform_pivot,
        bbox,
    );
    tool_transform::start_transform_drag(&mut app, current_pivot, false, false);
    tool_transform::update_transform_drag(&mut app, Point::new(130.0, 110.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(Point::new(130.0, 110.0)));

    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes.len(),
        1,
        "未変形のマーキースライス状態でピボットだけ動かしてもスライスの切れ目がタイムラインに書き込まれないこと (#8)"
    );

    // 2. シンボルインスタンスをマーキー選択（生シェイプも同時にスライス）した状態でピボットを変更した場合、
    // 生シェイプのスライス控えを復元しつつシンボルの新しいピボットがタイムラインへ永続化され失われないこと (#8)
    tool_select::clear_selection(&mut app);
    let sym_shapes = make_stroke(
        Point::new(100.0, 220.0),
        Point::new(180.0, 220.0),
        16.0,
        Color32::from_rgb(200, 80, 40),
    );
    app.session.shapes.extend(sym_shapes);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.session.set_selected_indices(vec![1]);
    doc_actions::convert_selection_to_symbol(&mut app).unwrap();

    // 生シェイプの一部とシンボルインスタンス全体をまたぐマーキー選択を行う
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(80.0, 80.0),
        Point::new(160.0, 260.0),
        false,
    );
    assert!(app.session.has_unsliced_snapshot());

    // シンボルインスタンスのみに選択を絞り、ピボットを変更して選択解除
    let sym_indices: Vec<usize> = app
        .session
        .shapes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.symbol_instance.is_some())
        .map(|(i, _)| i)
        .collect();
    app.session.set_selected_indices(sym_indices);
    let new_world_pivot = Point::new(155.0, 235.0);
    assert!(tool_transform::set_selected_symbol_world_pivot(
        &mut app,
        Some(new_world_pivot),
        true
    ));
    assert!(!app.session.has_unsliced_snapshot());
    tool_select::clear_selection(&mut app);

    let saved_inst = &app.doc.root.layers[0].keyframes[0].cel.instances[0];
    assert!(
        (saved_inst.binding.world_pivot() - new_world_pivot).hypot() < 1e-4,
        "ピボット変更後に選択解除しても stale な未スライススナップショットでピボットが上書き消失しないこと (#8)"
    );
}

#[test]
fn test_move_or_copy_frames_preserves_following_frames_tween_pose_and_remaps_ids() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);

    // 1. 6フレームのレイヤー (Frame 0..=1: 赤, Frame 2..=5: 青) で Frame 0..=1 を Frame 2..=3 へ複製したとき、
    // 移動先範囲直後の Frame 4..=5 の青い絵が上書きされず保持されること (#9-1)
    app.doc.root.resize_total_frames(6);
    app.session.shapes = make_stroke(Point::new(60.0, 60.0), Point::new(100.0, 60.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    app.session.shapes = make_stroke(Point::new(160.0, 60.0), Point::new(200.0, 60.0), 10.0, blue);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    let sel_0_1 = timeline::TimelineCellSelection::from_corners(0, 0, 0, 1);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut app, sel_0_1, 0, 2, true
    ));
    assert_eq!(
        app.doc.root.layers[0].cel_at(2).unwrap().shapes[0].color,
        red
    );
    assert_eq!(
        app.doc.root.layers[0].cel_at(3).unwrap().shapes[0].color,
        red
    );
    assert_eq!(
        app.doc.root.layers[0].cel_at(4).unwrap().shapes[0].color,
        blue,
        "移動先範囲 [2..=3] の直後 Frame 4..=5 の既存セルが上書きされず保持されること (#9-1)"
    );

    // 2. トゥイーン中間フレームから開始する範囲を移動/複製した際、開始フレームの中割り補間姿勢を引き継ぐこと (#9-2)
    let mut tween_app = Is2dApp::default();
    tween_app.doc.root.resize_total_frames(5);
    tween_app.session.shapes =
        make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut tween_app);
    tween_app.session.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut tween_app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut tween_app));
    timeline_ui::switch_to_layer_and_frame(&mut tween_app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut tween_app, true);
    assert!(tool_select::nudge_selected_shapes(
        &mut tween_app,
        kurbo::Vec2::new(200.0, 0.0)
    ));

    // 中間フレーム Frame 2 (t = 0.5 -> dx = +100.0) のみを選択して別レイヤーへ複製
    let mid_shapes = tween_app
        .doc.root
        .evaluate_layer_shapes_at(0, 2, &tween_app.doc.library);
    let mid_center_x = mid_shapes[0].bounding_box().center().x;
    timeline_ui::timeline_add_layer(&mut tween_app);
    let sel_mid = timeline::TimelineCellSelection::single(0, 2);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut tween_app,
        sel_mid,
        1,
        0,
        true
    ));
    let copied_shapes =
        tween_app
            .doc.root
            .evaluate_layer_shapes_at(1, 2, &tween_app.doc.library);
    assert!(
        (copied_shapes[0].bounding_box().center().x - mid_center_x).abs() < 1e-3,
        "トゥイーン途中フレームから複製したセルが中割り補間姿勢を引き継いでいること (#9-2)"
    );

    // 3. 別レイヤーへの複製で instance_gid および group_path ID が重複せず一意に再採番されること (#9-3)
    let orig_gid = tween_app.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .instance_gid;
    let copied_gid = tween_app.doc.root.layers[1]
        .cel_at(2)
        .unwrap()
        .instances[0]
        .binding
        .instance_gid;
    assert_ne!(
        orig_gid, copied_gid,
        "別レイヤーへ複製されたシンボルインスタンスの instance_gid が再採番され重複しないこと (#9-3)"
    );
    assert!(
        tween_app.doc.next_group_id > copied_gid,
        "再採番後に SymbolManager::next_group_id が更新され以降の採番と衝突しないこと (#9-3)"
    );
}

#[test]
fn test_unsaved_tab_close_modal_non_blocking_save_discard_and_cancel() {
    let ctx = eframe::egui::Context::default();
    let mut app = Is2dApp::default();

    // タブ0を編集して Dirty にし、タブ1を追加してタブ0を close_tab(Some(&ctx)) で閉じる (#11)
    doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    );
    assert!(app.history.is_dirty());

    project_io::new_project(&mut app, &ctx);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active, 1);

    // Dirty なタブ0を閉じようとすると、rfd でブロックせず PendingCloseConfirm モーダル待機状態になること
    project_io::close_tab(&mut app, 0, Some(&ctx));
    assert!(
        matches!(
            app.pending_close_confirm,
            Some(project_io::PendingCloseConfirm::SingleTab {
                tab_index: 0,
                orig_active: 1
            })
        ),
        "Dirty タブのクローズ時に OS 同期ダイアログでブロックせず非ブロッキングモーダルが開くこと (#11)"
    );

    // キャンセルを選ぶとタブは閉じられず、元のアクティブタブ (1) に戻ること
    project_io::confirm_pending_close_cancel(&mut app, &ctx);
    assert!(app.pending_close_confirm.is_none());
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active, 1);

    // 再度閉じて「保存せずに破棄」を選ぶとタブ0が閉じられること
    project_io::close_tab(&mut app, 0, Some(&ctx));
    project_io::confirm_pending_close_discard(&mut app, &ctx);
    assert!(app.pending_close_confirm.is_none());
    assert_eq!(app.tabs.len(), 1);
}

#[test]
fn test_cancel_drag_and_noop_drag_preserve_tessellated_mesh_caches() {
    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        18.0,
        Color32::from_rgb(70, 140, 230),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.session.set_selected_indices(vec![0]);

    // 事前に fill_mesh キャッシュを構築して Arc ポインタを取得 (#12)
    let mesh_arc_before = std::sync::Arc::clone(app.session.shapes[0].fill_mesh());
    assert!(app.session.shapes[0].has_cached_fill_mesh());

    // 1. 選択移動ドラッグを開始して微小移動未満で終了（noop リストア）してもキャッシュが失われないこと
    let move_drag = clipboard_ops::begin_move_selection_drag(&mut app, Point::new(150.0, 100.0), false);
    tool_select::finish_move_selection_drag(&mut app, move_drag.total_delta, move_drag.undo_snapshot);
    assert!(
        app.session.shapes[0].has_cached_fill_mesh()
            && std::sync::Arc::ptr_eq(app.session.shapes[0].fill_mesh(), &mesh_arc_before),
        "微小ドラッグのロールバック時にテッセレーション済み Arc<Mesh> が O(1) で引き継がれ再テッセレーションしないこと (#12)"
    );

    // 2. 自由変形ドラッグを開始して Esc 相当の cancel_active_drag で中断してもキャッシュが失われないこと
    app.session.set_selected_indices(vec![0]);
    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();
    tool_transform::start_transform_drag(
        &mut app,
        Point::new(bbox.x0 + 15.0, bbox.center().y),
        false,
        false,
    );
    canvas_tool::cancel_active_drag(&mut app);
    assert!(
        app.session.shapes[0].has_cached_fill_mesh()
            && std::sync::Arc::ptr_eq(app.session.shapes[0].fill_mesh(), &mesh_arc_before),
        "ドラッグのキャンセル時にテッセレーション済み Arc<Mesh> が破棄されないこと (#12)"
    );
}

#[test]
fn test_u64_max_entity_ids_in_crafted_json_remapped_without_collision() {
    // u64::MAX の layer.id, symbol.id, instance_gid, group_path を含む細工ファイル (#13)
    let crafted_json = format!(
        r#"{{
          "version": 1,
          "stage_size": [800.0, 450.0],
          "next_symbol_id": {max},
          "next_group_id": {max},
          "timeline": {{
            "fps": 24,
            "total_frames": 1,
            "active_layer": 0,
            "current_frame": 0,
            "next_layer_id": {max},
            "layers": [
              {{
                "id": {max},
                "name": "MaxLayer1",
                "visible": true,
                "locked": false,
                "frame_count": 1,
                "keyframes": [
                  {{
                    "start_frame": 0,
                    "tween": false,
                    "tween_rotate": "Auto",
                    "shapes": [
                      {{
                        "color": [255, 0, 0, 255],
                        "exterior": {{
                          "segments": [
                            [[100.0, 100.0], [150.0, 100.0], [200.0, 100.0]],
                            [[200.0, 100.0], [200.0, 150.0], [200.0, 200.0]],
                            [[200.0, 200.0], [150.0, 150.0], [100.0, 100.0]]
                          ]
                        }},
                        "holes": [],
                        "layer_id": {max},
                        "group_path": [{max}]
                      }}
                    ],
                    "instances": [
                      {{
                        "binding": {{
                          "instance_gid": {max},
                          "symbol_id": {max},
                          "xform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                          "pivot": [0.0, 0.0],
                          "loop_mode": "Loop",
                          "first_frame": 0
                        }},
                        "layer_id": {max},
                        "parent_scope": [],
                        "shape_order_index": 1
                      }}
                    ]
                  }}
                ]
              }},
              {{
                "id": {max},
                "name": "MaxLayer2",
                "visible": true,
                "locked": false,
                "frame_count": 1,
                "keyframes": []
              }}
            ]
          }},
          "library": [
            {{
              "id": {max},
              "name": "MaxSym1",
              "timeline": {{
                "fps": 24,
                "total_frames": 1,
                "active_layer": 0,
                "current_frame": 0,
                "next_layer_id": {max},
                "layers": []
              }}
            }},
            {{
              "id": {max},
              "name": "MaxSym2",
              "timeline": {{
                "fps": 24,
                "total_frames": 1,
                "active_layer": 0,
                "current_frame": 0,
                "next_layer_id": {max},
                "layers": []
              }}
            }}
          ]
        }}"#,
        max = u64::MAX
    );

    let doc: project_io::ProjectDocument = serde_json::from_str(&crafted_json).unwrap();
    let mut app = Is2dApp::default();
    doc.apply_to_app(&mut app);

    // レイヤーID・シンボルID・グループIDがすべて安全範囲内に再採番され重複していないこと
    assert_ne!(app.doc.root.layers[0].id, app.doc.root.layers[1].id);
    assert!(app.doc.root.layers[0].id <= project_io::MAX_SAFE_ENTITY_ID);
    assert!(app.doc.root.layers[1].id <= project_io::MAX_SAFE_ENTITY_ID);
    assert_ne!(app.doc.library[0].id, app.doc.library[1].id);
    assert!(app.doc.library[0].id <= project_io::MAX_SAFE_ENTITY_ID);
    assert!(app.doc.library[1].id <= project_io::MAX_SAFE_ENTITY_ID);

    // 読み込み後に新規レイヤー・新規シンボル・新規グループIDを追加しても既存IDと一切衝突しないこと (#13)
    timeline_ui::timeline_add_layer(&mut app);
    let layer_ids: std::collections::HashSet<u64> =
        app.doc.root.layers.iter().map(|l| l.id).collect();
    assert_eq!(layer_ids.len(), 3, "新規レイヤーのIDが既存レイヤーと衝突しないこと (#13)");

    let new_sym_id = app.doc.register_new_symbol(Vec::new());
    assert!(
        app.doc
            .library
            .iter()
            .filter(|s| s.id == new_sym_id)
            .count()
            == 1,
        "新規シンボルのIDが既存シンボルと衝突しないこと (#13)"
    );
    let g1 = app.doc.alloc_group_id();
    let g2 = app.doc.alloc_group_id();
    assert_ne!(g1, g2, "alloc_group_id が飽和衝突しないこと (#13)");
}

#[test]
fn test_dense_symbol_library_cycle_detection_runs_in_linear_time() {
    // 60個のシンボルが密に参照し合い、かつ複数のサイクル（後退辺・自己ループ）を含むライブラリを構築 (#14)
    let n = 60usize;
    let mut library = Vec::with_capacity(n);
    for i in 0..n {
        let sym_id = (i + 1) as u64;
        let mut instances = Vec::new();
        // 前方への密なDAG辺 (i -> j for j > i) + サイクルを作る後退辺 (i -> i, i -> 0, i -> i/2)
        let mut targets: Vec<u64> = ((i + 1)..n.min(i + 15)).map(|j| (j + 1) as u64).collect();
        targets.push(sym_id); // 自己ループ
        if i > 0 {
            targets.push(1); // ルートへの後退辺
            targets.push(((i / 2) + 1) as u64);
        }
        for (k, target_sym_id) in targets.into_iter().enumerate() {
            instances.push(project_io::SavedSymbolInstance {
                binding: project_io::SavedSymbolInstanceBinding {
                    instance_gid: ((i * 100) + k + 1) as u64,
                    symbol_id: target_sym_id,
                    xform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                    pivot: [0.0, 0.0],
                    loop_mode: project_io::SavedSymbolLoopMode::Loop,
                    first_frame: 0,
                },
                layer_id: 1,
                parent_scope: Vec::new(),
                shape_order_index: k,
            });
        }
        library.push(project_io::SavedSymbolDef {
            id: sym_id,
            name: format!("DenseSym{sym_id}"),
            timeline: project_io::SavedTimeline {
                fps: 24,
                total_frames: 1,
                current_frame: 0,
                active_layer: 0,
                next_layer_number: 2,
                next_layer_id: 2,
                layers: vec![project_io::SavedLayer {
                    id: 1,
                    name: "L1".to_owned(),
                    visible: true,
                    locked: false,
                    frame_count: 1,
                    keyframes: vec![project_io::SavedKeyframe {
                        start_frame: 0,
                        tween: false,
                        tween_rotate: project_io::SavedTweenRotate::Auto,
                        shapes: Vec::new(),
                        instances,
                    }],
                }],
            },
        });
    }

    let doc = project_io::ProjectDocument {
        version: 1,
        stage_size: [800.0, 450.0],
        timeline: (&timeline::Timeline::default()).into(),
        library,
        next_symbol_id: (n + 1) as u64,
        next_group_id: 100_000,
    };

    let start = std::time::Instant::now();
    let mut app = Is2dApp::default();
    doc.apply_to_app(&mut app);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 200,
        "密なシンボルライブラリの循環検出・除去が O(V+E) で高速に完了すること (#14): elapsed={elapsed:?}"
    );
    // 残った全インスタンス辺がサイクルを含まず、前方DAG辺は保持されていること
    for sym in &app.doc.library {
        for inst in sym.timeline.layers[0].keyframes[0].cel.instances.iter() {
            assert!(
                !app.doc.would_create_symbol_cycle(sym.id, inst.binding.symbol_id),
                "3色DFS後はいかなるサイクル辺も残存しないこと (#14)"
            );
        }
    }
    assert!(
        !app.doc.library[0].timeline.layers[0].keyframes[0]
            .cel
            .instances
            .is_empty(),
        "サイクルでない正当なDAG参照辺は保持されていること (#14)"
    );
}

#[test]
fn test_key_repeat_suppressed_for_enter_onion_tween_and_cmd3() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 1. O キー初回押下フレーム (repeat: false) でオニオンスキンが ON になり、別フレームでの長押しリピート (repeat: true) では OFF に戻らないこと (#16)
    assert!(!app.view.onion_skin_enabled);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::O,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(app.view.onion_skin_enabled);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::O,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        app.view.onion_skin_enabled,
        "別フレームの O キー長押しリピートでオニオンスキンが切り替わり続けないこと (#16)"
    );

    // 2. Cmd+3 初回押下フレームで clip_outside_stage が ON になり、別フレームの repeat: true では OFF に戻らないこと (#16)
    let cmd_mods = egui::Modifiers {
        command: true,
        ctrl: true,
        ..Default::default()
    };
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods,
            events: vec![egui::Event::Key {
                key: egui::Key::Num3,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: cmd_mods,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(app.view.clip_outside_stage);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods,
            events: vec![egui::Event::Key {
                key: egui::Key::Num3,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: cmd_mods,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        app.view.clip_outside_stage,
        "別フレームの Cmd+3 長押しリピートで枠外マスクが切り替わり続けないこと (#16)"
    );

    // 3. T キー初回押下フレームでクラシックトゥイーンが ON になり、別フレームの repeat: true では OFF に戻らないこと (#16)
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        10.0,
        Color32::RED,
    );
    app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
    assert!(!app.doc.root.layers[0].keyframes[0].tween);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::T,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(app.doc.root.layers[0].keyframes[0].tween);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::T,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        app.doc.root.layers[0].keyframes[0].tween,
        "別フレームの T キー長押しリピートでトゥイーンが切り替わり続けないこと (#16)"
    );

    // 4. Enter 初回押下フレームで再生開始し、別フレームの repeat: true では停止しないこと (#16)
    app.doc.root.resize_total_frames(4);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(app.timeline_ui.is_playing);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        app.timeline_ui.is_playing,
        "別フレームの Enter 長押しリピートで再生/停止が切り替わり続けないこと (#16)"
    );
}

#[test]
fn test_transform_rotate_zone_only_near_four_corners_and_paste_staircase_across_zoom() {
    let mut app = Is2dApp::default();
    // 横長の図形 (x: 100..300, y: 100..200) を配置して選択
    app.session.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(300.0, 150.0),
        80.0,
        Color32::from_rgb(80, 140, 220),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.session.set_selected_indices(vec![0]);
    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();

    // 1. 四隅の少し外側 (+10px, -10px) は Rotate になるが、上辺中央の少し外側 (四隅から100px以上離れた位置) は Rotate にならないこと (#17)
    let corner_outside = Point::new(bbox.x1 + 10.0, bbox.y0 - 10.0);
    assert_eq!(
        tool_transform::compute_transform_hover(app.scene(), &app.session, corner_outside, app.view.zoom),
        canvas_tool::TransformHoverTarget::Rotate,
        "四隅の外側近傍では Rotate ゾーンになること (#17)"
    );
    let top_edge_outside = Point::new(bbox.center().x, bbox.y0 - 12.0);
    assert_eq!(
        tool_transform::compute_transform_hover(app.scene(), &app.session, top_edge_outside, app.view.zoom),
        canvas_tool::TransformHoverTarget::None,
        "四隅から離れた辺中央の外側は Rotate ゾーンにならないこと (#17)"
    );

    // 2. 連続ペーストの途中にズーム倍率を変更しても、毎回前回位置から +16px (画面上) ずつ単調増加の階段状に配置されること (#18)
    app.view.zoom = 1.0;
    tool_select::copy_selected_shapes(&mut app);
    let c0 = app.session.shapes[0].bounding_box().center();

    tool_select::paste_clipboard(&mut app, false);
    let c1 = app
        .session
        .shapes
        .last()
        .unwrap()
        .bounding_box()
        .center();
    assert!((c1.x - (c0.x + 16.0)).abs() < 1e-4);

    // 途中でズームイン (zoom = 4.0 -> 1ステップ = 4.0 ワールド座標) してから2回目のペースト
    app.view.zoom = 4.0;
    tool_select::paste_clipboard(&mut app, false);
    let c2 = app
        .session
        .shapes
        .last()
        .unwrap()
        .bounding_box()
        .center();
    assert!(
        (c2.x - (c1.x + 4.0)).abs() < 1e-4 && c2.x > c1.x && c1.x > c0.x,
        "連続ペーストの途中でズームインしても位置が逆戻りせず階段状にずれること (#18): c0={c0:?}, c1={c1:?}, c2={c2:?}"
    );
}

#[test]
fn test_partial_span_frame_move_preserves_unselected_tail_and_blanks_hold_source() {
    let red = Color32::from_rgb(220, 50, 50);

    // 1. 0〜5フレーム (6コマ) のキーフレームスパンのうち先頭 0〜2 のみを Frame 8〜10 へ通常移動 (Altなし)
    // → 移動元の 0〜2 は空白になり、選択していない残りの 3〜5 は消えずに元の赤い絵を保持すること
    let mut app = Is2dApp::default();
    app.doc.root.resize_total_frames(6);
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        10.0,
        red,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    let sel_prefix = timeline::TimelineCellSelection::from_corners(0, 0, 0, 2);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut app,
        sel_prefix,
        0,
        8,
        false
    ));
    for f in 0..=2 {
        assert!(
            app.doc.root.layers[0].cel_at(f).unwrap().is_empty(),
            "移動元の先頭範囲 Frame {f} は空白化されること"
        );
    }
    for f in 3..=5 {
        let cel = app.doc.root.layers[0].cel_at(f).unwrap();
        assert_eq!(
            cel.shapes.len(),
            1,
            "選択していない後半スパン Frame {f} の絵が消失せず保持されること"
        );
        assert_eq!(cel.shapes[0].color, red);
    }
    for f in 8..=10 {
        let cel = app.doc.root.layers[0].cel_at(f).unwrap();
        assert_eq!(cel.shapes.len(), 1);
        assert_eq!(cel.shapes[0].color, red);
    }

    // 2. 0〜5フレームのキーフレームスパンのうち保持フレームのみ (2〜3) を選んで Frame 8〜9 へ通常移動
    // → 0〜1 は保持、移動元の保持フレーム 2〜3 は空白化（コピーにならない）、後ろの 4〜5 も保持されること
    let mut hold_app = Is2dApp::default();
    hold_app.doc.root.resize_total_frames(6);
    hold_app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        10.0,
        red,
    );
    canvas_tool::sync_shapes_to_timeline(&mut hold_app);

    let sel_mid_hold = timeline::TimelineCellSelection::from_corners(0, 2, 0, 3);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut hold_app,
        sel_mid_hold,
        0,
        6,
        false
    ));
    for f in 0..=1 {
        assert_eq!(hold_app.doc.root.layers[0].cel_at(f).unwrap().shapes.len(), 1);
    }
    for f in 2..=3 {
        assert!(
            hold_app.doc.root.layers[0].cel_at(f).unwrap().is_empty(),
            "保持フレームのみを選んで通常移動した際、移動元の Frame {f} が空白化されること"
        );
    }
    for f in 4..=5 {
        assert_eq!(
            hold_app.doc.root.layers[0].cel_at(f).unwrap().shapes.len(),
            1,
            "保持フレーム移動範囲直後の Frame {f} の絵も消失せず保持されること"
        );
    }
    for f in 8..=9 {
        assert_eq!(hold_app.doc.root.layers[0].cel_at(f).unwrap().shapes.len(), 1);
    }
}

#[test]
fn test_nested_symbol_expansion_memoization_and_step_cap() {
    let mut app = Is2dApp::default();

    // 1. 中身が空の葉シンボル (id=8) を起点に、各段が子シンボルを20個ずつ持つ8段入れ子階層 (20^8 = 256億経路)
    let leaf = symbol::SymbolDef::new(8, "EmptyLeaf".to_string(), Vec::new());
    app.doc.library.push(leaf);

    let mut next_gid = 1u64;
    for level in (1..=7u64).rev() {
        let child_id = level + 1;
        let mut sym = symbol::SymbolDef::new(level, format!("Level{level}"), Vec::new());
        let instances: Vec<timeline::SymbolInstance> = (0..20)
            .map(|i| timeline::SymbolInstance {
                binding: vector_engine::SymbolInstanceBinding::new(
                    {
                        let gid = next_gid;
                        next_gid += 1;
                        gid
                    },
                    child_id,
                    kurbo::Affine::translate((i as f64 * 2.0, 0.0)),
                ),
                layer_id: 1,
                parent_scope: Vec::new(),
                shape_order_index: 0,
            })
            .collect();
        sym.timeline.layers[0].keyframes[0].cel =
            timeline::Cel::new_with_instances(Vec::new(), instances);
        app.doc.library.push(sym);
    }

    let root_instances: Vec<timeline::SymbolInstance> = (0..20)
        .map(|i| timeline::SymbolInstance {
            binding: vector_engine::SymbolInstanceBinding::new(
                {
                    let gid = next_gid;
                    next_gid += 1;
                    gid
                },
                1,
                kurbo::Affine::translate((i as f64 * 5.0, 0.0)),
            ),
            layer_id: 1,
            parent_scope: Vec::new(),
            shape_order_index: 0,
        })
        .collect();
    app.doc.root.layers[0].keyframes[0].cel =
        timeline::Cel::new_with_instances(Vec::new(), root_instances);

    let t0 = std::time::Instant::now();
    canvas_tool::load_shapes_from_timeline(&mut app);
    let elapsed_memo = t0.elapsed();
    assert_eq!(
        app.session.shapes.len(),
        20,
        "ルート直下の空シンボルインスタンス20個がそれぞれ空マーカーとして即座に展開されること"
    );
    assert!(
        elapsed_memo.as_millis() < 500,
        "メモ化により 20^8 空シンボル入れ子が瞬時に完了すること: {elapsed_memo:?}"
    );

    // 2. 各インスタンスが異なる first_frame を持ちメモ化が効かない細工ケースでも、
    //    MAX_SYMBOL_EXPANSION_STEPS により無限展開せず即座に打ち切られること
    for sym in &mut app.doc.library {
        sym.timeline.total_frames = 100_000;
        if let Some(kf) = sym.timeline.layers[0].keyframes.first_mut() {
            for (idx, inst) in kf.cel.instances_mut().iter_mut().enumerate() {
                inst.binding.loop_mode = vector_engine::SymbolLoopMode::SingleFrame;
                inst.binding.first_frame = idx * 97 + (sym.id as usize) * 13;
            }
        }
        sym.revision = vector_engine::next_mesh_revision();
    }
    let t1 = std::time::Instant::now();
    canvas_tool::load_shapes_from_timeline(&mut app);
    let first_non_empty =
        app.doc.library[1].first_non_empty_local_shapes(&app.doc.library);
    let elapsed_step_cap = t1.elapsed();
    assert!(first_non_empty.is_empty());
    assert!(
        elapsed_step_cap.as_millis() < 500,
        "メモ化が効かない異なるフレーム指定の入れ子でも展開回数上限により瞬時に打ち切られること: {elapsed_step_cap:?}"
    );
}

#[test]
fn test_onion_skin_per_tab_isolation_and_shift_marquee_no_premature_merge() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();

    // 1. オニオンスキンの設定 (onion_skin_enabled / onion_prev_frames / onion_next_frames) がタブごとに独立して保持されること (#3)
    app.view.onion_skin_enabled = true;
    app.view.onion_prev_frames = 5;
    app.view.onion_next_frames = 4;

    project_io::new_project(&mut app, &ctx);
    assert_eq!(app.active, 1);
    assert!(
        !app.view.onion_skin_enabled
            && app.view.onion_prev_frames == 2
            && app.view.onion_next_frames == 2,
        "新規タブではオニオンスキン設定が初期値 (false, 2, 2) になること (#3)"
    );

    project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(
        app.view.onion_skin_enabled
            && app.view.onion_prev_frames == 5
            && app.view.onion_next_frames == 4,
        "元のタブに戻るとそのタブのオニオンスキン設定が復元されること (#3)"
    );

    // 2. Shift＋マーキーで選択を追加しても、移動済みでまだ選択中の生図形が先に結合されないこと (#4)
    let mut sel_app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);
    // Shape 0: 左の赤図形 (100..180, 100), Shape 1: 中央の赤図形 (220..300, 100), Shape 2: 右の青図形 (400..460, 100)
    sel_app.session.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(180.0, 100.0), 14.0, red);
    sel_app.session.shapes.extend(make_stroke(
        Point::new(220.0, 100.0),
        Point::new(300.0, 100.0),
        14.0,
        red,
    ));
    sel_app.session.shapes.extend(make_stroke(
        Point::new(400.0, 100.0),
        Point::new(460.0, 100.0),
        14.0,
        blue,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut sel_app);

    // 中央の赤図形 (idx=1) を選択して左へ -80px 移動し、Shape 0 と重ならせる（まだ選択中なので未結合で3図形のまま）
    sel_app.session.set_selected_indices(vec![1]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut sel_app,
        KVec2::new(-80.0, 0.0)
    ));
    assert_eq!(sel_app.session.shapes.len(), 3);
    assert!(sel_app.session.has_modified_floating());

    // Shift＋マーキー (additive = true) で右の青図形を追加選択した際、移動済みの赤図形が早期に Shape 0 と結合されないこと
    select_ops::apply_marquee_selection(
        &mut sel_app,
        Point::new(380.0, 70.0),
        Point::new(480.0, 130.0),
        true,
    );
    assert_eq!(
        sel_app.session.shapes.len(),
        3,
        "Shift＋マーキーで追加選択しても、まだ選択中の移動済み生図形は先に結合されないこと (#4)"
    );
    assert_eq!(sel_app.session.selected_indices().len(), 2);

    // その後選択解除 (clear_selection) した瞬間に同色結合が確定し、赤1個＋青1個の計2図形になること
    canvas_tool::clear_selection(&mut sel_app);
    assert_eq!(sel_app.session.shapes.len(), 2);
}

#[test]
fn test_scale_commit_and_repeated_transform_rebuild_mesh_cache() {
    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(200.0, 200.0),
        Point::new(210.0, 200.0),
        6.0,
        Color32::from_rgb(60, 130, 230),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    let initial_verts = app.session.shapes[0].fill_mesh().vertices.len();

    // 1. 自由変形の拡大ドラッグ (transform_selected_from_origins) 中は既存頂点数を再利用し、
    //    commit_moved_selection 確定時に rebuild_cache() されて曲線分割頂点数が増加すること (#5)
    app.session.set_selected_indices(vec![0]);
    let undo_snapshot = app.session.snapshot_for_drag();
    let orig_shapes = app.session.selected_shapes_snapshot();
    let pivot = app.session.shapes[0].bounding_box().center();
    let scale_xform = kurbo::Affine::translate(pivot.to_vec2())
        * kurbo::Affine::scale(4.0)
        * kurbo::Affine::translate(-pivot.to_vec2());
    canvas_tool::Session::transform_selected_from_origins(
        &mut app.session.shapes,
        &orig_shapes,
        scale_xform,
    );
    assert_eq!(
        app.session.shapes[0].fill_mesh().vertices.len(),
        initial_verts,
        "ドラッグ中は頂点バッファを再利用すること"
    );
    doc_actions::commit_moved_selection(&mut app, undo_snapshot);
    let rebuilt_verts = app.session.shapes[0].fill_mesh().vertices.len();
    assert!(
        rebuilt_verts > initial_verts,
        "拡大確定時 (commit_moved_selection) にメッシュが再構築され、拡大後の曲線に応じた滑らかな頂点数になること (#5): initial={initial_verts}, rebuilt={rebuilt_verts}"
    );

    // 2. 1.2倍 (<= 1.25倍) の FillShape::transform を繰り返した場合でもキャッシュが再構築されること (#5)
    let mut s = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(110.0, 100.0),
        6.0,
        Color32::RED,
    )
    .remove(0);
    let v0 = s.fill_mesh().vertices.len();
    let step_scale = kurbo::Affine::scale(1.2);
    for _ in 0..8 {
        s.transform(step_scale);
    }
    let v_after = s.fill_mesh().vertices.len();
    assert!(
        v_after > v0,
        "1.25倍以内の拡大 transform でもメッシュキャッシュが再構築されること (#5): v0={v0}, v_after={v_after}"
    );
}

#[test]
fn test_raw_input_transform_handles_vertex_move_wheel_zoom_pan_and_save_on_close() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 初回フレームでレイアウト確定
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );

    // 1. 中ボタンドラッグによるパン操作 & ホイールズームの RawInput 検証
    let canvas_center = app.view.last_canvas_rect.center();
    let pan_before = app.view.pan;
    let pan_to = canvas_center + Vec2::new(40.0, 25.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(canvas_center),
                egui::Event::PointerButton {
                    pos: canvas_center,
                    button: egui::PointerButton::Middle,
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
            events: vec![egui::Event::PointerMoved(pan_to)],
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
                pos: pan_to,
                button: egui::PointerButton::Middle,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        (app.view.pan - (pan_before + Vec2::new(40.0, 25.0))).length() < 1e-3,
        "中ボタンドラッグでキャンバスがパン移動すること"
    );
    app.view.reset_view();

    // 2. 選択ツールでの頂点移動ドラッグ (MoveVertex) の RawInput 検証
    app.session.shapes = make_stroke(
        Point::new(300.0, 250.0),
        Point::new(420.0, 250.0),
        18.0,
        Color32::from_rgb(220, 60, 60),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Select });
    let v0_world = app.session.shapes[0].exterior.segments[0].p0;
    let v0_screen = app.view.world_to_screen(v0_world);
    let v0_drag_to = v0_screen + Vec2::new(25.0, 30.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(v0_screen),
                egui::Event::PointerButton {
                    pos: v0_screen,
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
            events: vec![egui::Event::PointerMoved(v0_drag_to)],
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
                pos: v0_drag_to,
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
    let expected_moved_v0 = Point::new(v0_world.x + 25.0, v0_world.y + 30.0);
    assert!(
        app.session.shapes[0]
            .exterior
            .segments
            .iter()
            .any(|seg| (seg.p0 - expected_moved_v0).hypot() < 2.0),
        "選択ツールで頂点をドラッグして移動できること"
    );

    // 3. 自由変形ツールでのスケールハンドルドラッグ (Scale) & 回転ドラッグ (Rotate) の RawInput 検証
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Transform });
    app.session.set_selected_indices(vec![0]);
    let bbox_before = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();
    let br_world = canvas_tool::ScaleHandle::BottomRight.position(bbox_before);
    let br_screen = app.view.world_to_screen(br_world);
    let br_drag_to = br_screen + Vec2::new(60.0, 40.0);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(br_screen),
                egui::Event::PointerButton {
                    pos: br_screen,
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
            events: vec![egui::Event::PointerMoved(br_drag_to)],
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
                pos: br_drag_to,
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
    let bbox_scaled = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();
    assert!(
        bbox_scaled.width() > bbox_before.width() + 40.0,
        "自由変形ツールの右下スケールハンドルドラッグで拡大されること: before={bbox_before:?}, after={bbox_scaled:?}"
    );

    // 四隅外側の回転ゾーンからのドラッグで回転されること
    let rot_start_world = Point::new(bbox_scaled.x1 + 10.0, bbox_scaled.y0 - 10.0);
    let rot_start_scr = app.view.world_to_screen(rot_start_world);
    let rot_drag_scr = rot_start_scr + Vec2::new(0.0, 80.0);
    let p0_before_rot = app.session.shapes[0].exterior.segments[0].p0;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(rot_start_scr),
                egui::Event::PointerButton {
                    pos: rot_start_scr,
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
            events: vec![egui::Event::PointerMoved(rot_drag_scr)],
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
                pos: rot_drag_scr,
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
    let p0_after_rot = app.session.shapes[0].exterior.segments[0].p0;
    assert!(
        (p0_after_rot - p0_before_rot).hypot() > 5.0,
        "自由変形ツールの四隅外側ドラッグで図形が回転されること"
    );

    // マウスホイールによるカーソル中心ズームの RawInput 検証
    let zoom_before = app.view.zoom;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(canvas_center),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, 120.0),
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
        app.view.zoom > zoom_before,
        "キャンバス上のマウスホイール入力でズームインすること"
    );

    // 4. 未保存タブを閉じる際の「保存して閉じる」 (ConfirmPendingCloseSave) 検証
    let tmp_save_close =
        std::env::temp_dir().join(format!("is2d_save_on_close_{}.is2d", std::process::id()));
    project_io::new_project(&mut app, &ctx);
    assert_eq!(app.tabs.len(), 2);
    app.file = Some(tmp_save_close.clone());
    doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 12.0,
            color: Color32::BLUE,
        },
    );
    assert!(app.history.is_dirty());
    project_io::close_active_tab(&mut app, Some(&ctx));
    assert!(app.pending_close_confirm.is_some());
    app.apply_action(panels::UiAction::ConfirmPendingCloseSave, &ctx);
    assert!(
        app.pending_close_confirm.is_none() && app.tabs.len() == 1,
        "『保存して閉じる』でファイルへ保存された上で対象タブが閉じられること"
    );
    assert!(tmp_save_close.exists());
    let _ = std::fs::remove_file(&tmp_save_close);
}

#[test]
fn test_property_geometry_strokes_booleans_and_random_json_invariants() {
    let mut rng = fastrand::Rng::with_seed(0x152D_2026_1031);

    // 1. ランダムな手描き座標列に対するストローク生成・アフィン変換・ブーリアン演算の不変条件テスト (50ケース)
    for case_idx in 0..50 {
        let n_pts = rng.usize(2..=12);
        let mut pts = Vec::with_capacity(n_pts);
        let mut cur = Point::new(
            rng.f64() * 600.0 + 50.0,
            rng.f64() * 350.0 + 50.0,
        );
        pts.push(cur);
        for _ in 1..n_pts {
            cur.x = (cur.x + (rng.f64() - 0.5) * 80.0).clamp(10.0, 790.0);
            cur.y = (cur.y + (rng.f64() - 0.5) * 80.0).clamp(10.0, 440.0);
            pts.push(cur);
        }
        let radius = rng.f64() * 25.0 + 1.0;
        let col_a = Color32::from_rgb(rng.u8(..), rng.u8(..), rng.u8(..));
        let raw_outline = generate_brush_outline(&pts, radius);
        let shapes_a = build_smoothed_stroke_shapes(&raw_outline, col_a);
        assert!(
            !shapes_a.is_empty(),
            "case {case_idx}: 有効な入力点列から空でないストローク図形が生成されること"
        );
        for s in &shapes_a {
            let bbox = s.bounding_box();
            assert!(
                bbox.x0.is_finite()
                    && bbox.y0.is_finite()
                    && bbox.x1.is_finite()
                    && bbox.y1.is_finite()
                    && bbox.x0 <= bbox.x1
                    && bbox.y0 <= bbox.y1,
                "case {case_idx}: 生成されたストローク図形のバウンディングボックスが有限かつ正規化されていること"
            );
            assert!(
                !s.fill_mesh().vertices.is_empty(),
                "case {case_idx}: 生成されたストローク図形が有効なメッシュを持つこと"
            );
        }

        // 別のストロークとの Union / Difference でも全座標が有限で退化輪郭を含まないこと
        let p0 = Point::new(rng.f64() * 500.0 + 80.0, rng.f64() * 300.0 + 60.0);
        let p1 = Point::new(p0.x + rng.f64() * 60.0 + 5.0, p0.y + rng.f64() * 60.0 + 5.0);
        let shapes_b = make_stroke(p0, p1, 8.0, col_a);
        let unioned = vector_boolean::boolean_operation(
            &shapes_a,
            &shapes_b,
            vector_boolean::BoolOp::Union,
            col_a,
        );
        for u in &unioned {
            for c in u.contours() {
                assert!(c.segments.len() >= 2);
                assert!(c.segments.iter().all(|seg| {
                    seg.p0.x.is_finite()
                        && seg.p0.y.is_finite()
                        && seg.p1.x.is_finite()
                        && seg.p1.y.is_finite()
                        && seg.p2.x.is_finite()
                        && seg.p2.y.is_finite()
                }));
            }
        }
    }

    // 2. ランダムな異常値・参照構造を含む ProjectDocument JSON の読み込み・サニタイズ不変条件テスト (50ケース)
    for case_idx in 0..50 {
        let num_syms = rng.usize(1..=6);
        let mut sym_json_list = Vec::new();
        for i in 0..num_syms {
            let sid = if rng.bool() { i as u64 } else { u64::MAX - (i as u64) };
            let target_sym = rng.u64(0..=(num_syms as u64 + 2));
            let tf = rng.usize(0..=50);
            sym_json_list.push(format!(
                r#"{{
                    "id": {sid},
                    "name": "Sym_{i}",
                    "timeline": {{
                        "total_frames": {tf},
                        "fps": {},
                        "active_layer": 99,
                        "current_frame": 999,
                        "next_layer_number": 0,
                        "next_layer_id": 0,
                        "layers": [{{
                            "id": 0,
                            "name": "L",
                            "visible": true,
                            "locked": false,
                            "frame_count": {tf},
                            "keyframes": [{{
                                "start_frame": 0,
                                "tween": true,
                                "shapes": [],
                                "instances": [{{
                                    "binding": {{
                                        "symbol_id": {target_sym},
                                        "instance_gid": {},
                                        "xform": [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                                        "pivot": [0.0, 0.0],
                                        "loop_mode": "Loop",
                                        "first_frame": {}
                                    }},
                                    "layer_id": 0,
                                    "parent_scope": [],
                                    "shape_order_index": 0
                                }}]
                            }}]
                        }}]
                    }}
                }}"#,
                rng.u32(0..=120),
                rng.u64(0..=10),
                rng.usize(0..=200),
            ));
        }
        let raw_json = format!(
            r#"{{
                "version": 1,
                "stage_size": [{}, {}],
                "timeline": {{
                    "total_frames": {},
                    "fps": {},
                    "active_layer": 50,
                    "current_frame": 500,
                    "next_layer_number": 0,
                    "next_layer_id": 0,
                    "layers": []
                }},
                "library": [{}],
                "next_symbol_id": 0,
                "next_group_id": 0
            }}"#,
            rng.f32() * 5000.0 - 500.0,
            rng.f32() * 5000.0 - 500.0,
            rng.usize(0..=100),
            rng.u32(0..=200),
            sym_json_list.join(","),
        );

        let doc: project_io::ProjectDocument = serde_json::from_str(&raw_json)
            .expect("有効なJSON構文の ProjectDocument がパースできること");
        let mut app = Is2dApp::default();
        doc.apply_to_app(&mut app);

        assert!(
            !app.doc.root.layers.is_empty(),
            "case {case_idx}: 空レイヤー配列でも最低1レイヤーに補正されること"
        );
        assert!(
            (1..=60).contains(&app.doc.root.fps),
            "case {case_idx}: FPS が 1..=60 にクランプされること"
        );
        assert!(
            app.doc.root.current_frame < app.doc.root.total_frames,
            "case {case_idx}: current_frame が total_frames 未満に収まること"
        );
        for sym in &app.doc.library {
            assert!(sym.id >= 1, "case {case_idx}: シンボルIDが 1 以上に正規化されること");
            for l in &sym.timeline.layers {
                for kf in &l.keyframes {
                    for inst in kf.cel.instances.iter() {
                        assert!(
                            !app.doc
                                .would_create_symbol_cycle(sym.id, inst.binding.symbol_id),
                            "case {case_idx}: ランダム参照グラフから循環参照が完全に除去されていること"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn test_disjoint_different_color_bbox_overlap_preserves_shape_id_and_small_shape_scale_handles() {
    // 1. #6: 異なる色の斜めストローク同士で AABB は重なるが実際の幾何形状は交差していない場合、
    // merge_shapes_into_canvas で既存図形が不要に再構築されず shape_id・mesh_revision・Arc<Mesh> が維持されること
    let mut canvas = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 200.0),
        6.0,
        Color32::RED,
    );
    let orig_id = canvas[0].shape_id;
    let orig_rev = canvas[0].mesh_revision;
    let orig_mesh = std::sync::Arc::clone(canvas[0].fill_mesh());

    // AABB ([100..200, 100..200]) の右上隅 (180, 110)->(195, 125) に青ストロークを追加（赤の対角線とは交差しない）
    let non_intersecting_blue = make_stroke(
        Point::new(180.0, 110.0),
        Point::new(195.0, 125.0),
        4.0,
        Color32::BLUE,
    );
    assert!(vector_engine::rects_overlap(
        canvas[0].bounding_box(),
        non_intersecting_blue[0].bounding_box()
    ));
    vector_boolean::merge_shapes_into_canvas(&mut canvas, non_intersecting_blue);
    assert_eq!(canvas.len(), 2);
    assert_eq!(
        canvas[0].shape_id, orig_id,
        "AABBのみ重なり実交差しない異色図形の shape_id が再構築で変化しないこと (#6)"
    );
    assert_eq!(canvas[0].mesh_revision, orig_rev);
    assert!(std::sync::Arc::ptr_eq(canvas[0].fill_mesh(), &orig_mesh));

    // 2. #14: 12px 未満の極小図形でも四隅のスケールハンドル直上では Pivot ではなく Scale ハンドルが優先ヒットすること
    let mut app = Is2dApp::default();
    app.session.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(104.0, 100.0),
        2.0,
        Color32::RED,
    );
    app.session.set_selected_indices(vec![0]);
    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, 1.0)
        .unwrap();
    let top_left = tool_transform::ScaleHandle::TopLeft.position(bbox);
    let hover = tool_transform::compute_transform_hover(app.scene(), &app.session, top_left, 1.0);
    assert_eq!(
        hover,
        tool_transform::TransformHoverTarget::Scale(tool_transform::ScaleHandle::TopLeft),
        "小さな図形の四隅ハンドル座標では Pivot に遮られず Scale(TopLeft) がヒットすること (#14)"
    );

    // 3. #11: レイヤー数上限 (MAX_LAYERS_PER_TIMELINE = 1000) を超えて timeline_add_layer されないこと
    let mut tl = timeline::Timeline::default();
    for i in 1..project_io::MAX_LAYERS_PER_TIMELINE {
        tl.layers
            .push(timeline::Layer::new_with_id((i + 1) as u64, "L"));
    }
    assert_eq!(tl.layers.len(), project_io::MAX_LAYERS_PER_TIMELINE);
    tl.add_layer_above_active();
    assert_eq!(
        tl.layers.len(),
        project_io::MAX_LAYERS_PER_TIMELINE,
        "編集時も MAX_LAYERS_PER_TIMELINE (1000) を超えてレイヤーが追加されないこと (#11)"
    );

    // 4. #13: シンボリックリンク先への保存時にリンク自体を壊さずリンク先実体ファイルを更新すること
    #[cfg(unix)]
    {
        let target_path = std::env::temp_dir().join(format!(
            "is2d_symlink_target_{}.is2d",
            std::process::id()
        ));
        let link_path =
            std::env::temp_dir().join(format!("is2d_symlink_link_{}.is2d", std::process::id()));
        let _ = std::fs::remove_file(&target_path);
        let _ = std::fs::remove_file(&link_path);
        std::fs::write(&target_path, "{}").unwrap();
        std::os::unix::fs::symlink(&target_path, &link_path).unwrap();

        project_io::save_project_to_path(&mut app, &link_path).unwrap();
        let link_meta = std::fs::symlink_metadata(&link_path).unwrap();
        assert!(
            link_meta.file_type().is_symlink(),
            "シンボリックリンク経由で保存してもシンボリックリンク自体が通常ファイルに置換されないこと (#13)"
        );
        let target_content = std::fs::read_to_string(&target_path).unwrap();
        assert!(
            target_content.contains("\"version\":1"),
            "シンボリックリンクの実体ファイル側が最新の内容に更新されること (#13)"
        );
        let _ = std::fs::remove_file(&link_path);
        let _ = std::fs::remove_file(&target_path);
    }
}

// ============================================================================
// 段階0・段階1：`Action` 列（`run`）とタイムラインセル検証（`cel_summary`）による安全網テスト
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
struct InstanceSummary {
    instance_gid: u64,
    symbol_id: u64,
    xform_coeffs: [f64; 6],
    pivot: Point,
    loop_mode: crate::symbol::SymbolLoopMode,
    first_frame: usize,
}

#[derive(Clone, Debug)]
struct CelSummary {
    exists: bool,
    is_keyframe_start: bool,
    tween: bool,
    tween_rotate: crate::symbol::TweenRotate,
    shape_count: usize,
    colors: Vec<Color32>,
    total_area: f64,
    bbox: Option<kurbo::Rect>,
    total_segments: usize,
    hole_count: usize,
    group_paths: Vec<Vec<u64>>,
    instances: Vec<InstanceSummary>,
    evaluated_shape_count: usize,
    evaluated_total_area: f64,
    evaluated_bbox: Option<kurbo::Rect>,
}

/// `canvas.shapes` を一切参照せず、タイムラインのセル（およびシンボル定義の解決結果）のみから状態を要約する
fn cel_summary(doc: &symbol::Document, layer_id: u64, frame: usize) -> CelSummary {
    let Some(layer_idx) = doc.root.layer_index_by_id(layer_id) else {
        return CelSummary {
            exists: false,
            is_keyframe_start: false,
            tween: false,
            tween_rotate: crate::symbol::TweenRotate::Auto,
            shape_count: 0,
            colors: Vec::new(),
            total_area: 0.0,
            bbox: None,
            total_segments: 0,
            hole_count: 0,
            group_paths: Vec::new(),
            instances: Vec::new(),
            evaluated_shape_count: 0,
            evaluated_total_area: 0.0,
            evaluated_bbox: None,
        };
    };
    let layer = &doc.root.layers[layer_idx];
    let Some(kf_idx) = layer.keyframe_index_at(frame) else {
        return CelSummary {
            exists: false,
            is_keyframe_start: false,
            tween: false,
            tween_rotate: crate::symbol::TweenRotate::Auto,
            shape_count: 0,
            colors: Vec::new(),
            total_area: 0.0,
            bbox: None,
            total_segments: 0,
            hole_count: 0,
            group_paths: Vec::new(),
            instances: Vec::new(),
            evaluated_shape_count: 0,
            evaluated_total_area: 0.0,
            evaluated_bbox: None,
        };
    };
    let kf = &layer.keyframes[kf_idx];
    let cel = &kf.cel;
    let shapes = cel.shapes.as_slice();
    let colors = shapes.iter().map(|s| s.color).collect();
    let total_area = shapes
        .iter()
        .map(|s| {
            let ext = s.exterior.signed_area().abs();
            let holes: f64 = s.holes.iter().map(|h| h.signed_area().abs()).sum();
            (ext - holes).max(0.0)
        })
        .sum();
    let bbox = (!shapes.is_empty()).then(|| vector_engine::union_bounding_boxes(shapes));
    let total_segments = shapes
        .iter()
        .map(|s| {
            s.exterior.segments.len()
                + s.holes.iter().map(|h| h.segments.len()).sum::<usize>()
        })
        .sum();
    let hole_count = shapes.iter().map(|s| s.holes.len()).sum();
    let group_paths = shapes.iter().map(|s| s.group_path.clone()).collect();
    let instances = cel
        .instances
        .iter()
        .map(|inst| InstanceSummary {
            instance_gid: inst.binding.instance_gid,
            symbol_id: inst.binding.symbol_id,
            xform_coeffs: inst.binding.xform.as_coeffs(),
            pivot: inst.binding.pivot,
            loop_mode: inst.binding.loop_mode,
            first_frame: inst.binding.first_frame,
        })
        .collect();

    let eval_shapes = doc
        .root
        .evaluate_layer_shapes_at(layer_idx, frame, &doc.library);
    let evaluated_shape_count = eval_shapes.len();
    let evaluated_total_area = eval_shapes
        .iter()
        .map(|s| {
            let ext = s.exterior.signed_area().abs();
            let holes: f64 = s.holes.iter().map(|h| h.signed_area().abs()).sum();
            (ext - holes).max(0.0)
        })
        .sum();
    let evaluated_bbox =
        (!eval_shapes.is_empty()).then(|| vector_engine::union_bounding_boxes(&eval_shapes));

    CelSummary {
        exists: true,
        is_keyframe_start: kf.start_frame == frame,
        tween: kf.tween,
        tween_rotate: kf.tween_rotate,
        shape_count: shapes.len(),
        colors,
        total_area,
        bbox,
        total_segments,
        hole_count,
        group_paths,
        instances,
        evaluated_shape_count,
        evaluated_total_area,
        evaluated_bbox,
    }
}

/// `Action` の列を順番に `doc_actions::apply` へ流すテストヘルパー
fn run(host: &mut impl doc_actions::ActionHost, actions: &[doc_actions::Action]) {
    for action in actions {
        doc_actions::apply(host, action.clone());
    }
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

#[test]
fn test_stage01_move_deselect_merge_and_marquee_slice_restore_on_cel() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 90, 220);

    // 1. 移動と選択解除時の結合（Merge Drawing）
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
                radius: 14.0,
                color: red,
            },
            Action::PaintStroke {
                points: vec![Point::new(260.0, 100.0), Point::new(320.0, 100.0)],
                radius: 14.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(290.0, 100.0),
            },
            Action::MoveSelected {
                delta: kurbo::Vec2::new(-140.0, 0.0),
            },
            Action::Deselect,
        ],
    );
    let s_merged = cel_summary(&app.doc, lid, 0);
    assert_eq!(
        s_merged.shape_count, 1,
        "移動後に Deselect すると重なった同色生図形が1つに結合されること"
    );

    // 2. マーキーのスライスと無劣化復元、およびスライス片の移動分割
    let segs_before_marquee = s_merged.total_segments;
    let area_before_marquee = s_merged.total_area;
    run(
        &mut app,
        &[
            Action::SelectMarquee {
                p0: Point::new(80.0, 70.0),
                p1: Point::new(140.0, 130.0),
                additive: false,
            },
            Action::Deselect,
        ],
    );
    let s_restored = cel_summary(&app.doc, lid, 0);
    assert_eq!(s_restored.shape_count, 1);
    assert_eq!(
        s_restored.total_segments, segs_before_marquee,
        "マーキー選択後に移動せず解除した場合は元のセグメント数に無劣化復元されること"
    );
    assert!((s_restored.total_area - area_before_marquee).abs() < 1e-3);

    // マーキーで左半分を切り出して下へ移動し選択解除すると2図形に分割確定されること
    run(
        &mut app,
        &[
            Action::SelectMarquee {
                p0: Point::new(80.0, 70.0),
                p1: Point::new(140.0, 130.0),
                additive: false,
            },
            Action::MoveSelected {
                delta: kurbo::Vec2::new(0.0, 100.0),
            },
            Action::Deselect,
            // 異色図形も重ねて移動→解除で背面を切り抜くこと
            Action::PaintStroke {
                points: vec![Point::new(300.0, 100.0), Point::new(340.0, 100.0)],
                radius: 12.0,
                color: blue,
            },
            Action::SelectSingleAt {
                pt: Point::new(320.0, 100.0),
            },
            Action::MoveSelected {
                delta: kurbo::Vec2::new(-160.0, 0.0),
            },
            Action::Deselect,
        ],
    );
    let s_split = cel_summary(&app.doc, lid, 0);
    assert_eq!(s_split.shape_count, 3);
}

#[test]
fn test_stage01_reshape_and_anchor_edit_on_cel() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 60, 60);

    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 200.0), Point::new(300.0, 200.0)],
            radius: 20.0,
            color: red,
        }],
    );
    let s0 = cel_summary(&app.doc, lid, 0);
    let bbox0 = s0.bbox.unwrap();

    // 1. PullEdgeFromTo（上辺を上へ引き伸ばす）
    run(
        &mut app,
        &[Action::PullEdgeFromTo {
            from: Point::new(200.0, 180.0),
            to: Point::new(200.0, 120.0),
        }],
    );
    let s_pulled = cel_summary(&app.doc, lid, 0);
    assert!(
        s_pulled.bbox.unwrap().y0 < bbox0.y0 - 20.0,
        "PullEdgeFromTo により上辺が上方向へ変形していること"
    );

    // 2. AddAnchor / DeleteAnchor（輪郭上のアンカー追加と削除）
    let segs_before_add = s_pulled.total_segments;
    run(
        &mut app,
        &[Action::AddAnchor {
            pt: Point::new(200.0, 220.0),
        }],
    );
    let s_added = cel_summary(&app.doc, lid, 0);
    assert_eq!(
        s_added.total_segments,
        segs_before_add + 1,
        "AddAnchor で輪郭セグメント数が +1 増えること"
    );

    // 追加された頂点 (200, 220) を MoveVertexFromTo で下へ移動
    run(
        &mut app,
        &[Action::MoveVertexFromTo {
            from: Point::new(200.0, 220.0),
            to: Point::new(200.0, 270.0),
        }],
    );
    let s_moved_v = cel_summary(&app.doc, lid, 0);
    assert!(
        s_moved_v.bbox.unwrap().y1 > s_added.bbox.unwrap().y1 + 20.0,
        "MoveVertexFromTo で頂点が下方向へ移動していること"
    );

    // DeleteAnchor でその頂点を削除
    run(
        &mut app,
        &[Action::DeleteAnchor {
            pt: Point::new(200.0, 270.0),
        }],
    );
    let s_deleted = cel_summary(&app.doc, lid, 0);
    assert_eq!(
        s_deleted.total_segments, segs_before_add,
        "DeleteAnchor で輪郭セグメント数が元に戻ること"
    );
}

#[test]
fn test_stage01_group_ungroup_transform_and_clipboard_on_cel() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 100, 220);

    // 1. グループ化・インプレイス編集・グループ解除
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
                radius: 12.0,
                color: red,
            },
            Action::SelectAll,
            Action::Group,
        ],
    );
    let s_grp = cel_summary(&app.doc, lid, 0);
    assert_eq!(s_grp.shape_count, 1);
    assert_eq!(s_grp.group_paths[0].len(), 1);

    // グループ内へ入って青ストロークを追加し、親階層へ出る
    run(
        &mut app,
        &[
            Action::EnterGroupAt {
                pt: Point::new(140.0, 100.0),
            },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 130.0), Point::new(180.0, 130.0)],
                radius: 12.0,
                color: blue,
            },
            Action::ExitGroup,
        ],
    );
    let s_grp2 = cel_summary(&app.doc, lid, 0);
    assert_eq!(s_grp2.shape_count, 2);
    assert!(s_grp2.group_paths.iter().all(|gp| gp.len() == 1));

    // 2. 自由変形（拡大・回転・反転）
    let area_before_scale = s_grp2.total_area;
    run(
        &mut app,
        &[
            Action::ScaleSelection { sx: 2.0, sy: 1.5 },
            Action::RotateSelection {
                angle_rad: std::f64::consts::FRAC_PI_2,
            },
            Action::FlipSelection {
                flip_x: true,
                flip_y: false,
            },
        ],
    );
    let s_xform = cel_summary(&app.doc, lid, 0);
    assert!(
        (s_xform.total_area - area_before_scale * 3.0).abs() < area_before_scale * 0.15,
        "2.0x1.5 の拡大により面積が約3倍になること"
    );

    // グループ解除
    run(&mut app, &[Action::Ungroup, Action::Deselect]);
    let s_ungrp = cel_summary(&app.doc, lid, 0);
    assert!(s_ungrp.group_paths.iter().all(|gp| gp.is_empty()));

    // 3. コピー・ペースト・カット・同じ位置にペースト
    run(
        &mut app,
        &[
            Action::SelectAll,
            Action::Copy,
            Action::Paste { in_place: false },
            Action::MoveSelected {
                delta: kurbo::Vec2::new(220.0, 0.0),
            },
            Action::Deselect,
        ],
    );
    let s_pasted = cel_summary(&app.doc, lid, 0);
    assert!(
        s_pasted.total_area > s_ungrp.total_area * 1.5,
        "コピー＆ペーストでセル内の図形面積が増加すること"
    );

    run(
        &mut app,
        &[
            Action::SelectAll,
            Action::Cut,
        ],
    );
    assert_eq!(cel_summary(&app.doc, lid, 0).shape_count, 0);

    run(
        &mut app,
        &[
            Action::Paste { in_place: true },
            Action::Deselect,
        ],
    );
    assert!(cel_summary(&app.doc, lid, 0).shape_count >= 2);
}

#[test]
fn test_stage01_timeline_f5_f6_f7_and_move_copy_frames_on_cel() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 50, 50);

    // Frame 0 に描画 → F5 で延長 → Frame 2 で F6（複製キーフレーム）→ Frame 3 で F7（空白キーフレーム）
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
                radius: 12.0,
                color: red,
            },
            Action::ExtendFrame,
            Action::ExtendFrame,
            Action::ExtendFrame,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 2,
            },
            Action::InsertKeyframe { copy_prev: true },
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 3,
            },
            Action::InsertKeyframe { copy_prev: false },
        ],
    );
    assert!(cel_summary(&app.doc, lid, 0).is_keyframe_start);
    assert!(!cel_summary(&app.doc, lid, 1).is_keyframe_start);
    assert_eq!(cel_summary(&app.doc, lid, 1).shape_count, 1);
    assert!(cel_summary(&app.doc, lid, 2).is_keyframe_start);
    assert_eq!(cel_summary(&app.doc, lid, 2).shape_count, 1);
    assert!(cel_summary(&app.doc, lid, 3).is_keyframe_start);
    assert_eq!(cel_summary(&app.doc, lid, 3).shape_count, 0);

    // コマの複製（Frame 2 を Frame 5 へ複製）と移動
    let sel_f2 = crate::timeline::TimelineCellSelection::single(0, 2);
    run(
        &mut app,
        &[Action::MoveOrCopyFrames {
            sel: sel_f2,
            layer_delta: 0,
            frame_delta: 3,
            copy_mode: true,
        }],
    );
    assert!(cel_summary(&app.doc, lid, 5).is_keyframe_start);
    assert_eq!(cel_summary(&app.doc, lid, 5).shape_count, 1);

    // Shift+F6（キーフレーム削除）と Shift+F5（フレーム削除）
    run(
        &mut app,
        &[
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 2,
            },
            Action::DeleteKeyframe,
        ],
    );
    assert!(
        !cel_summary(&app.doc, lid, 2).is_keyframe_start,
        "Shift+F6 で Frame 2 のキーフレームが削除され前キーフレームの保持フレームに統合されること"
    );
    run(&mut app, &[Action::RemoveFrame]);
    assert!(
        cel_summary(&app.doc, lid, 2).is_keyframe_start
            && cel_summary(&app.doc, lid, 2).shape_count == 0,
        "Shift+F5 で保持フレームが1コマ短縮され、Frame 3 にあった空白キーフレームが Frame 2 へ前詰めされること"
    );
}

#[test]
fn test_stage01_symbol_inplace_edit_and_classic_tween_on_cel() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 100, 220);

    // 1. 図形を描いてシンボルに変換（F8）し、インプレイス編集で内部に図形を追加して戻る
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
                radius: 12.0,
                color: red,
            },
            Action::SelectAll,
            Action::ConvertToSymbol,
        ],
    );
    let s0 = cel_summary(&app.doc, lid, 0);
    assert_eq!(s0.instances.len(), 1);
    let sym_id = s0.instances[0].symbol_id;
    let area_before_inplace = s0.evaluated_total_area;

    run(
        &mut app,
        &[
            Action::EnterSymbolFromLibrary { sym_id },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 140.0), Point::new(160.0, 140.0)],
                radius: 12.0,
                color: blue,
            },
            Action::ExitSymbolTimeline,
        ],
    );
    let s_after_inplace = cel_summary(&app.doc, lid, 0);
    assert_eq!(s_after_inplace.evaluated_shape_count, 2);
    assert!(s_after_inplace.evaluated_total_area > area_before_inplace * 1.5);

    // 2. クラシックトゥイーンの設定と中間フレーム補間
    run(
        &mut app,
        &[
            Action::ExtendFrame,
            Action::ExtendFrame,
            Action::ExtendFrame,
            Action::ExtendFrame,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 4,
            },
            Action::InsertKeyframe { copy_prev: true },
            Action::SelectAll,
            Action::MoveSelected {
                delta: kurbo::Vec2::new(200.0, 0.0),
            },
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 0,
            },
            Action::ToggleTween,
            Action::SetTweenRotate {
                rotate: crate::symbol::TweenRotate::Clockwise,
            },
        ],
    );
    let s_kf0 = cel_summary(&app.doc, lid, 0);
    let s_mid = cel_summary(&app.doc, lid, 2);
    let s_kf4 = cel_summary(&app.doc, lid, 4);
    assert!(s_kf0.tween);
    assert_eq!(s_kf0.tween_rotate, crate::symbol::TweenRotate::Clockwise);
    let x0 = s_kf0.evaluated_bbox.unwrap().center().x;
    let x_mid = s_mid.evaluated_bbox.unwrap().center().x;
    let x4 = s_kf4.evaluated_bbox.unwrap().center().x;
    assert!(
        (x_mid - (x0 + 100.0)).abs() < 2.0 && (x4 - (x0 + 200.0)).abs() < 2.0,
        "中間フレーム2の補間位置が開始と終了の中央 (x0 + 100) にあること"
    );
}

#[test]
fn test_stage01_undo_redo_coalesce_and_save_load_on_cel() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 50, 50);

    // 1. 再生中に Action を流すと自動で stop_playback されること（SetFps 以外）
    app.timeline_ui.is_playing = true;
    run(&mut app, &[Action::SetFps { fps: 30 }]);
    assert!(
        app.timeline_ui.is_playing,
        "SetFps は再生を止めずに FPS を変更できること"
    );
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
            radius: 12.0,
            color: red,
        }],
    );
    assert!(
        !app.timeline_ui.is_playing,
        "描画などの Action 実行時に apply の入口で自動停止すること"
    );

    // 2. CoalesceKey による連続編集の Undo 1回統合テスト
    // (a) ステージサイズの連続ドラッグが Undo 1回で初期サイズに戻ること
    let orig_stage = app.doc.stage_size;
    let undo_base = app.history.undo_stack.len();
    run(
        &mut app,
        &[
            Action::SetStageSize {
                size: Vec2::new(900.0, 500.0),
            },
            Action::SetStageSize {
                size: Vec2::new(1000.0, 600.0),
            },
            Action::SetStageSize {
                size: Vec2::new(1280.0, 720.0),
            },
            Action::EndCoalesce,
        ],
    );
    assert_eq!(app.history.undo_stack.len(), undo_base + 1);
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.doc.stage_size, orig_stage);
    run(&mut app, &[Action::Redo]);
    assert_eq!(app.doc.stage_size, Vec2::new(1280.0, 720.0));

    // (b) 総フレーム数の連続ドラッグが Undo 1回で戻ること
    let orig_total = app.doc.root.total_frames;
    let undo_before_total = app.history.undo_stack.len();
    run(
        &mut app,
        &[
            Action::SetTotalFrames { total: 80 },
            Action::SetTotalFrames { total: 100 },
            Action::SetTotalFrames { total: 120 },
            Action::EndCoalesce,
        ],
    );
    assert_eq!(app.history.undo_stack.len(), undo_before_total + 1);
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.doc.root.total_frames, orig_total);

    // (c) 矢印キーの連続移動 (NudgeSelection) が Undo 1回で戻ること
    run(
        &mut app,
        &[Action::SelectSingleAt {
            pt: Point::new(140.0, 100.0),
        }],
    );
    let x_before_nudge = cel_summary(&app.doc, lid, 0).bbox.unwrap().center().x;
    let undo_before_nudge = app.history.undo_stack.len();
    run(
        &mut app,
        &[
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(10.0, 0.0),
            },
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(10.0, 0.0),
            },
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(10.0, 0.0),
            },
        ],
    );
    assert_eq!(app.history.undo_stack.len(), undo_before_nudge + 1);
    run(&mut app, &[Action::Undo]);
    let x_after_undo = cel_summary(&app.doc, lid, 0).bbox.unwrap().center().x;
    assert!((x_after_undo - x_before_nudge).abs() < 1e-3);

    // (d) シンボル再生フレームの連続変更とシンボル名の連続入力がそれぞれ Undo 1回で戻ること
    run(&mut app, &[Action::SelectAll, Action::ConvertToSymbol]);
    let sym_id = cel_summary(&app.doc, lid, 0).instances[0].symbol_id;
    run(
        &mut app,
        &[
            Action::EnterSymbolFromLibrary { sym_id },
            Action::ExtendFrame,
            Action::ExtendFrame,
            Action::ExitSymbolTimeline,
        ],
    );
    let undo_before_pb = app.history.undo_stack.len();
    run(
        &mut app,
        &[
            Action::SetSymbolPlayback {
                loop_mode: None,
                first_frame: Some(1),
            },
            Action::SetSymbolPlayback {
                loop_mode: None,
                first_frame: Some(2),
            },
            Action::EndCoalesce,
        ],
    );
    assert_eq!(app.history.undo_stack.len(), undo_before_pb + 1);
    assert_eq!(cel_summary(&app.doc, lid, 0).instances[0].first_frame, 2);
    run(&mut app, &[Action::Undo]);
    assert_eq!(cel_summary(&app.doc, lid, 0).instances[0].first_frame, 0);

    let orig_sym_name = app.doc.find_symbol(sym_id).unwrap().name.clone();
    let undo_before_rename = app.history.undo_stack.len();
    run(
        &mut app,
        &[
            Action::RenameSymbol {
                sym_id,
                new_name: "A".to_string(),
            },
            Action::RenameSymbol {
                sym_id,
                new_name: "AB".to_string(),
            },
            Action::RenameSymbol {
                sym_id,
                new_name: "ABC".to_string(),
            },
            Action::EndCoalesce,
        ],
    );
    assert_eq!(app.history.undo_stack.len(), undo_before_rename + 1);
    assert_eq!(app.doc.find_symbol(sym_id).unwrap().name, "ABC");
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.doc.find_symbol(sym_id).unwrap().name, orig_sym_name);

    // 3. 保存と読み込みの往復検証（cel_summary で一致を確認）
    let tmp_path = std::env::temp_dir().join(format!(
        "is2d_stage01_roundtrip_{}.is2d",
        std::process::id()
    ));
    let before_save = cel_summary(&app.doc, lid, 0);
    project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut loaded_app = Is2dApp::default();
    project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    let after_load = cel_summary(&loaded_app.doc, lid, 0);
    assert_eq!(before_save.instances, after_load.instances);
    assert_eq!(
        before_save.evaluated_shape_count,
        after_load.evaluated_shape_count
    );
    assert!((before_save.evaluated_total_area - after_load.evaluated_total_area).abs() < 1e-3);
}

#[test]
fn test_stage3_selection_preserved_across_undo_redo_frame_scrub_and_reexpansion() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(230, 70, 70);

    // 1. 図形を描画して選択し、移動 → Undo → Redo → 再展開しても ShapeId による選択が保たれること
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(80.0, 110.0), Point::new(140.0, 110.0)],
                radius: 15.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(110.0, 110.0),
            },
        ],
    );
    assert_eq!(app.session.selection.shapes.len(), 1);
    assert!(app.session.selection.instances.is_empty());
    let shape_id = *app.session.selection.shapes.iter().next().unwrap();
    assert_eq!(app.session.selected_indices(), vec![0]);

    // 移動してから Undo / Redo しても同じ ShapeId の選択が維持されること
    run(
        &mut app,
        &[Action::NudgeSelection {
            delta: KVec2::new(30.0, 15.0),
        }],
    );
    assert!(app.session.selection.shapes.contains(&shape_id));

    run(&mut app, &[Action::Undo]);
    assert!(
        app.session.selection.shapes.contains(&shape_id),
        "Undo 後も存在する ShapeId の選択が保たれること"
    );
    assert_eq!(app.session.selected_indices(), vec![0]);

    run(&mut app, &[Action::Redo]);
    assert!(
        app.session.selection.shapes.contains(&shape_id),
        "Redo 後も存在する ShapeId の選択が保たれること"
    );
    assert_eq!(app.session.selected_indices(), vec![0]);

    // タイムラインからの再展開（load_shapes_from_timeline）や同じキーフレーム内のホールドフレーム移動でも選択が保たれること
    canvas_tool::load_shapes_from_timeline(&mut app);
    assert!(app.session.selection.shapes.contains(&shape_id));

    run(
        &mut app,
        &[Action::ExtendFrame, Action::ScrubRulerToFrame { frame: 1 }],
    );
    assert_eq!(app.doc.root.current_frame, 1);
    assert!(
        app.session.selection.shapes.contains(&shape_id),
        "同じキーフレームのホールドフレーム間の移動（ScrubRulerToFrame）後も同じ ShapeId の選択が保たれること"
    );

    // F6 で複製キーフレームを作成し、さらに先頭キーフレームを F5 で延長して
    // Keyframe 0 = frame 0..=1、Keyframe 1 = frame 2 の構成にする。
    // frame 0 で図形を選び、同じキーフレーム内の frame 1 へ移動したときは選択が保たれ、
    // 別のキーフレームである frame 2 へ移動したときは（同じ shape_id を共有していても）選択が消えること
    run(
        &mut app,
        &[
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 2,
            },
            Action::InsertKeyframe { copy_prev: true },
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 0,
            },
            Action::SelectSingleAt {
                pt: Point::new(140.0, 125.0),
            },
        ],
    );
    assert!(app.session.selection.shapes.contains(&shape_id));
    run(
        &mut app,
        &[Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 1,
        }],
    );
    assert!(
        app.session.selection.shapes.contains(&shape_id),
        "同じキーフレーム内のコマ送り移動（frame 0 -> 1）では選択が残ること"
    );
    run(
        &mut app,
        &[Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2,
        }],
    );
    assert!(
        app.session.selection.is_empty(),
        "F6 で複製した別のキーフレーム（frame 1 -> 2）へ移動したときは選択が解除されること"
    );

    // 2. シンボルインスタンス（InstanceGid）に変換後、再展開で内部 FillShape の shape_id が変わっても
    //    InstanceGid による選択が Undo / Redo / 同一キーフレーム内移動 / 再展開を通して保たれること
    run(
        &mut app,
        &[
            Action::ScrubRulerToFrame { frame: 0 },
            Action::SelectSingleAt {
                pt: Point::new(140.0, 125.0),
            },
            Action::ConvertToSymbol,
        ],
    );
    assert!(app.session.selection.shapes.is_empty());
    assert_eq!(app.session.selection.instances.len(), 1);
    let inst_gid = *app.session.selection.instances.iter().next().unwrap();
    let eval_shape_id_before = app.session.shapes[0].shape_id;

    canvas_tool::load_shapes_from_timeline(&mut app);
    let eval_shape_id_after = app.session.shapes[0].shape_id;
    assert_ne!(
        eval_shape_id_before, eval_shape_id_after,
        "シンボル再展開のたびに展開後 FillShape の shape_id は新しく割り当てられること"
    );
    assert!(
        app.session.selection.instances.contains(&inst_gid),
        "展開後 FillShape の shape_id が変わっても InstanceGid による選択が保たれること"
    );
    assert_eq!(app.session.selected_indices(), vec![0]);

    run(
        &mut app,
        &[Action::NudgeSelection {
            delta: KVec2::new(20.0, 0.0),
        }],
    );
    run(&mut app, &[Action::Undo]);
    assert!(
        app.session.selection.instances.contains(&inst_gid),
        "シンボル移動の Undo 後も InstanceGid 選択が保たれること"
    );
    run(&mut app, &[Action::Redo]);
    assert!(
        app.session.selection.instances.contains(&inst_gid),
        "シンボル移動の Redo 後も InstanceGid 選択が保たれること"
    );

    // 3. シンボル移動とシンボル変換自体を Undo すると、消滅した InstanceGid は選択から除去され、
    //    古い選択が残らないこと
    run(&mut app, &[Action::Undo, Action::Undo]);
    assert!(
        !app.session.selection.instances.contains(&inst_gid),
        "Undo で消滅した InstanceGid が古い選択として残らないこと"
    );

    // 新規図形を描画して選択した状態から描画自体を Undo すると、消滅した ShapeId が選択から除去されること
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(240.0, 105.0), Point::new(290.0, 105.0)],
                radius: 15.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(265.0, 105.0),
            },
        ],
    );
    let transient_shape_id = *app.session.selection.shapes.iter().next().unwrap();
    run(&mut app, &[Action::Undo]);
    assert!(
        !app.session.selection.shapes.contains(&transient_shape_id),
        "Undo で消滅した ShapeId が古い選択として残らないこと"
    );
    assert!(app.session.selection.is_empty());
}

#[test]
fn test_stage4_scene_resolve_and_hit_testing_via_scene() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 230);

    // 1. 描画すると Tab::scene() に解決済みの読み取り専用借用ビューが返され、Scene 経由で当たり判定が動くこと
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
            radius: 15.0,
            color: red,
        }],
    );
    assert_eq!(app.scene().len(), 1);
    assert_eq!(
        app.scene()
            .topmost_shape_at(Point::new(140.0, 100.0), &app.session.editing_group_path),
        Some(0),
        "Scene::topmost_shape_at で現在フレームの図形がヒットすること"
    );
    assert_eq!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            Point::new(140.0, 100.0),
            app.view.zoom,
            app.tool.select_sub_mode,
        ),
        HoverTarget::Fill { shape_idx: 0 },
        "compute_hover_target が Scene<'_> を読み取って当たり判定を返すこと"
    );

    // 2. Scene::resolve(&app.doc, &app.session.shapes) が既存メッシュキャッシュを引き継ぎつつ解決結果を返すこと
    let mesh_arc_before = std::sync::Arc::clone(app.scene().shapes()[0].fill_mesh());
    let resolved = canvas_tool::Scene::resolve(&app.doc, &app.session.shapes);
    assert_eq!(resolved.len(), 1);
    assert!(
        std::sync::Arc::ptr_eq(resolved[0].fill_mesh(), &mesh_arc_before),
        "Scene::resolve は幾何・色が一致する図形のテッセレーション済みメッシュキャッシュを O(1) で引き継ぐこと"
    );

    // 3. 選択・自由変形ホバー・バウンディングボックス・ピボット計算がすべて Scene<'_> 経由で動作すること
    run(
        &mut app,
        &[Action::SelectSingleAt {
            pt: Point::new(140.0, 100.0),
        }],
    );
    let (mask, any_sel) = app.scene().selected_shape_mask(&app.session.selection);
    assert!(any_sel);
    assert_eq!(mask, vec![true]);
    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();
    let tl_handle = canvas_tool::ScaleHandle::TopLeft.position(bbox);
    assert_eq!(
        tool_transform::compute_transform_hover(app.scene(), &app.session, tl_handle, app.view.zoom),
        canvas_tool::TransformHoverTarget::Scale(canvas_tool::ScaleHandle::TopLeft),
        "compute_transform_hover が Scene<'_> と &Session からスケールハンドルを判定できること"
    );

    // 4. シンボル変換・インプレイス編集への出入りでも Scene が現在の編集階層に合わせて解決されること
    run(
        &mut app,
        &[
            Action::ConvertToSymbol,
            Action::EnterGroupAt {
                pt: Point::new(140.0, 100.0),
            },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 160.0), Point::new(180.0, 160.0)],
                radius: 15.0,
                color: blue,
            },
        ],
    );
    assert_eq!(
        app.scene().len(),
        2,
        "シンボル内タイムライン編集中も Scene にシンボル内の2図形が解決されていること"
    );
    assert_eq!(
        app.scene()
            .topmost_shape_at(Point::new(140.0, 160.0), &app.session.editing_group_path),
        Some(1)
    );

    run(&mut app, &[Action::ExitGroup]);
    assert_eq!(
        app.scene().len(),
        2,
        "シンボル編集から親タイムラインへ戻った後も、展開後の2図形が Scene に反映されていること"
    );
    assert!(
        app.scene()
            .shapes()
            .iter()
            .all(|s| s.symbol_instance.is_some()),
        "親タイムラインの Scene 図形にはシンボルインスタンスバインディングが付与されていること"
    );
}

#[test]
fn test_stage4_drag_2000_shapes_preserves_unmoved_mesh_caches() {
    use doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let color = Color32::from_rgb(220, 80, 60);

    // 2000個の独立した図形を配置（50列 × 40行）
    let mut shapes = Vec::with_capacity(2000);
    for idx in 0..2000usize {
        let col = (idx % 50) as f64;
        let row = (idx / 50) as f64;
        let x0 = 20.0 + col * 24.0;
        let y0 = 20.0 + row * 24.0;
        let mut stroke = make_stroke(Point::new(x0, y0), Point::new(x0 + 10.0, y0), 3.0, color);
        stroke[0].layer_id = lid;
        shapes.push(stroke.remove(0));
    }
    app.session.shapes = shapes;
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert_eq!(app.scene().len(), 2000);

    // ドラッグ前の描画（Scene 経由で全2000図形の fill_mesh() を評価・キャッシュ構築）
    let meshes_before: std::collections::HashMap<u64, std::sync::Arc<egui::epaint::Mesh>> = app
        .scene()
        .shapes()
        .iter()
        .map(|s| (s.shape_id, std::sync::Arc::clone(s.fill_mesh())))
        .collect();
    assert_eq!(meshes_before.len(), 2000);

    // 1つの図形（先頭図形: (25.0, 20.0)）を選択してドラッグ移動を開始・複数回マウス移動・確定する
    let target_pt = Point::new(25.0, 20.0);
    run(&mut app, &[Action::SelectSingleAt { pt: target_pt }]);
    assert_eq!(app.session.selection.shapes.len(), 1);
    let moved_shape_id = *app.session.selection.shapes.iter().next().unwrap();

    let move_drag = clipboard_ops::begin_move_selection_drag(&mut app, target_pt, false);
    app.session.drag = canvas_tool::ActiveCanvasDrag::Select(
        canvas_tool::SelectDragState::MoveShape(move_drag),
    );
    tool_select::update_reshape_drag(&mut app, Point::new(28.0, 22.0));

    // ドラッグ中（マウス移動1回目直後）に Scene から fill_mesh() を読んだとき、
    // 動かしていない残り1999個の図形がすべてドラッグ前と同一の Arc<Mesh> を保持していること
    let mut unmoved_during_drag = 0usize;
    for shape in app.scene().shapes() {
        if shape.shape_id == moved_shape_id {
            continue;
        }
        let before_arc = &meshes_before[&shape.shape_id];
        assert!(
            std::sync::Arc::ptr_eq(shape.fill_mesh(), before_arc),
            "ドラッグ中に動かしていない図形 (shape_id={}) の fill_mesh() キャッシュが失われず同一 Arc を返すこと",
            shape.shape_id
        );
        unmoved_during_drag += 1;
    }
    assert_eq!(unmoved_during_drag, 1999);

    // さらにマウス移動してドラッグ確定（Action::MoveSelected 相当のタイムライン同期含む）
    tool_select::update_reshape_drag(&mut app, Point::new(32.0, 25.0));
    run(
        &mut app,
        &[Action::MoveSelected {
            delta: KVec2::new(4.0, 3.0),
        }],
    );

    // ドラッグ確定・タイムライン同期後も、残り1999個の図形の fill_mesh() がドラッグ前と同一の Arc<Mesh> を返していること
    let mut unmoved_after_commit = 0usize;
    for shape in app.scene().shapes() {
        if shape.shape_id == moved_shape_id {
            continue;
        }
        let before_arc = &meshes_before[&shape.shape_id];
        assert!(
            std::sync::Arc::ptr_eq(shape.fill_mesh(), before_arc),
            "ドラッグ確定後も動かしていない図形 (shape_id={}) の fill_mesh() が再テッセレーションされず同一 Arc を返すこと",
            shape.shape_id
        );
        unmoved_after_commit += 1;
    }
    assert_eq!(unmoved_after_commit, 1999);
}

#[test]
fn test_stage5_floating_lifecycle_and_commit_floating() {
    use crate::canvas_tool::FloatingOrigin;
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let mut base_stroke = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(300.0, 100.0),
        20.0,
        Color32::RED,
    );
    base_stroke[0].layer_id = lid;
    app.session.shapes = base_stroke;
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert!(app.session.floating.is_none());

    // 1. マーキー矩形スライスで Floating(Slice, IDENTITY) が生成され、未変更で選択解除すると base がそのまま復元されること
    run(
        &mut app,
        &[Action::SelectMarquee {
            p0: Point::new(80.0, 70.0),
            p1: Point::new(200.0, 130.0),
            additive: false,
        }],
    );
    let fl = app
        .session
        .floating
        .as_ref()
        .expect("マーキー矩形スライス直後に Floating が生成されること");
    assert_eq!(fl.origin, FloatingOrigin::Slice);
    assert_eq!(fl.xform, kurbo::Affine::IDENTITY);
    assert!(fl.is_unmodified_slice());
    assert!(!fl.needs_merge_commit());
    assert_eq!(fl.base.len(), 1);
    assert_eq!(fl.pieces.len(), 1);

    run(&mut app, &[Action::Deselect]);
    assert!(app.session.floating.is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "未変更のマーキースライスは選択解除時に破棄され元の1図形に戻ること"
    );

    // 2. マーキースライス後に移動すると Floating.xform が更新され、Action::CommitFloating で1回だけ結合確定されること
    run(
        &mut app,
        &[Action::SelectMarquee {
            p0: Point::new(80.0, 70.0),
            p1: Point::new(200.0, 130.0),
            additive: false,
        }],
    );
    run(
        &mut app,
        &[Action::MoveSelected {
            delta: KVec2::new(10.0, 0.0),
        }],
    );
    let fl_moved = app
        .session
        .floating
        .as_ref()
        .expect("スライス断片の移動後も Floating が保持されること");
    assert_ne!(fl_moved.xform, kurbo::Affine::IDENTITY);
    assert!(fl_moved.needs_merge_commit());

    // 重なっている同色断片が Action::CommitFloating で1図形に Union 結合され、Floating が空になること
    run(&mut app, &[Action::CommitFloating]);
    assert!(app.session.floating.is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "CommitFloating により重なった同色断片が base と結合され1図形になること"
    );

    // 3. ペーストで FloatingOrigin::Paste が生成され、CommitFloating で重なった既存図形と結合されること
    run(
        &mut app,
        &[
            Action::SelectSingleAt {
                pt: Point::new(250.0, 100.0),
            },
            Action::Copy,
            Action::Paste { in_place: true },
        ],
    );
    let fl_paste = app
        .session
        .floating
        .as_ref()
        .expect("ペースト直後に Floating が生成されること");
    assert_eq!(fl_paste.origin, FloatingOrigin::Paste);
    assert!(fl_paste.needs_merge_commit());
    assert_eq!(app.scene().len(), 2);

    run(&mut app, &[Action::CommitFloating]);
    assert!(app.session.floating.is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "インプレースペーストした同色図形が CommitFloating で1図形に結合されること"
    );
}





