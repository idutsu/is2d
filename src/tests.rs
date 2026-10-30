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
    app.ui.brush_color = black;
    tool_paint::commit_raw_outline(&mut app, &raw_ring);
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(
        !app.canvas.shapes[0].contains_point(center),
        "初期状態では中央は空白（穴）であること"
    );

    // 1. 閉じきっていない外側の背景 (20, 20) をバケツでクリックしても塗られないこと
    app.ui.brush_color = red;
    assert!(
        !tool_paint::bucket_fill_at(&mut app, Point::new(20.0, 20.0)),
        "開いた背景は塗りつぶされないこと"
    );

    // 2. 円環の内側の空白 (200, 200) を赤でバケツ塗り -> 赤い塗りシェイプが生成されること
    assert!(
        tool_paint::bucket_fill_at(&mut app, center),
        "閉領域がバケツで塗られること"
    );
    assert_eq!(app.canvas.shapes.len(), 2);
    let inner_shape = app.canvas.shapes.iter().find(|s| s.color == red).unwrap();
    assert!(inner_shape.contains_point(center));

    // 3. 塗られた赤い領域を青でバケツクリック -> 青にリカラーされること
    app.ui.brush_color = blue;
    assert!(
        tool_paint::bucket_fill_at(&mut app, center),
        "既存の塗りシェイプが別色に塗り替わること"
    );
    assert_eq!(app.canvas.shapes.len(), 2);
    assert!(app
        .canvas
        .shapes
        .iter()
        .any(|s| s.color == blue && s.contains_point(center)));

    // 4. 内側の領域を周囲の線と同じ黒でバケツクリック -> 外周の黒リングと Union されて穴なしの1つの黒シェイプになること
    app.ui.brush_color = black;
    assert!(tool_paint::bucket_fill_at(&mut app, center));
    assert_eq!(
        app.canvas.shapes.len(),
        1,
        "同色でバケツ塗りすると周囲のシェイプと1つに結合されること"
    );
    assert!(
        app.canvas.shapes[0].holes.is_empty(),
        "穴が埋まって消えていること"
    );
    assert!(app.canvas.shapes[0].contains_point(center));
}

#[test]
fn test_marquee_select_partial_slice_and_shift_multi_select() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    app.ui.brush_color = red;

    // x: 100..300, y: 200 の太い横長ストローク（半径 25px）を描画
    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        25.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
    assert_eq!(app.canvas.shapes.len(), 1);

    // 1. 左端 (x: 70..140, y: 150..250) を矩形選択 -> 左端だけスライスされて選択状態になること
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(140.0, 250.0),
        false,
    );
    assert_eq!(
        app.canvas.shapes.len(),
        2,
        "部分矩形選択で内外2つにスライスされること"
    );
    assert_eq!(
        app.canvas.selected_shapes.len(),
        1,
        "スライスされた内側のみが選択されていること"
    );
    assert!(
        app.canvas.shapes[app.canvas.selected_shapes[0]].contains_point(Point::new(110.0, 200.0)),
        "選択された断片が左端領域を含んでいること"
    );

    // 2. 移動も削除もせずに選択解除（clear_selection）すると、無劣化で元の1つのシェイプに戻ること
    tool_select::clear_selection(&mut app);
    assert_eq!(
        app.canvas.shapes.len(),
        1,
        "未編集で選択解除すると元の1つのシェイプに復元されること"
    );
    assert!(app.canvas.selected_shapes.is_empty());

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
        app.canvas.shapes.len(),
        3,
        "左右両端のスライスと中央の非選択領域の計3つに分割されること"
    );
    assert_eq!(
        app.canvas.selected_shapes.len(),
        2,
        "Shift 連続矩形選択で左右2つの領域が同時に選択されていること"
    );

    // 4. 選択された左右両端を削除すると、中央 (x: 200, y: 200) のみが残ること
    assert!(tool_select::delete_selected_shapes(&mut app));
    assert_eq!(
        app.canvas.shapes.len(),
        1,
        "選択された左右両端が削除され中央の1つだけ残ること"
    );
    assert!(app.canvas.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(!app.canvas.shapes[0].contains_point(Point::new(110.0, 200.0)));
    assert!(!app.canvas.shapes[0].contains_point(Point::new(290.0, 200.0)));

    // 5. 1回の Undo でスライス前の元の1つのシェイプに完全復帰すること
    app.undo();
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(app.canvas.shapes[0].contains_point(Point::new(110.0, 200.0)));
    assert!(app.canvas.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(app.canvas.shapes[0].contains_point(Point::new(290.0, 200.0)));
}

#[test]
fn test_keep_selection_on_tool_switch_and_bucket_fill_selected_region_only() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    app.ui.brush_color = red;

    // x: 100..300, y: 200 の赤いストロークを描画
    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        25.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
    assert_eq!(app.canvas.shapes.len(), 1);

    // 1. 選択ツールで左半分 (x: 70..200) を矩形選択
    canvas_tool::set_tool(&mut app, Tool::Select);
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert_eq!(app.canvas.selected_shapes.len(), 1);

    // 2. バケツツールに切り替えても選択状態が解除されないこと
    canvas_tool::set_tool(&mut app, Tool::Bucket);
    assert_eq!(
        app.canvas.selected_shapes.len(),
        1,
        "バケツツールに切り替えても選択が維持されること"
    );

    // 3. 選択されていない右半分 (260, 200) をバケツでクリックしても塗られないこと
    app.ui.brush_color = blue;
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
        app.canvas
            .shapes
            .iter()
            .any(|s| s.color == blue && s.contains_point(Point::new(130.0, 200.0))),
        "選択された左半分が青になっていること"
    );
    assert!(
        app.canvas
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
    app.ui.brush_color = red;

    // 中心 (200, 200) の横長シェイプ (x: 100..300, y: 200, 半径 20px) を描画
    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        20.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
    assert_eq!(app.canvas.shapes.len(), 1);
    app.canvas.selected_shapes = vec![0];
    canvas_tool::set_tool(&mut app, Tool::Transform);

    // 1. +90° 回転 -> 横長だったシェイプが中心 (200, 200) を軸に縦長 (x: 200, y: 100..300) になること
    assert!(tool_transform::rotate_selected_shapes(
        &mut app,
        std::f64::consts::FRAC_PI_2
    ));
    assert!(
        app.canvas.shapes[0].contains_point(Point::new(200.0, 110.0))
            && app.canvas.shapes[0].contains_point(Point::new(200.0, 290.0)),
        "+90度回転で縦長シェイプになること"
    );
    assert!(
        !app.canvas.shapes[0].contains_point(Point::new(110.0, 200.0)),
        "元の左右端にはシェイプが存在しないこと"
    );

    // 2. 縮小 (0.5倍) -> 上下端 (200, 110) は範囲外になり、(200, 155) 付近に縮小されること
    assert!(tool_transform::scale_selected_shapes(&mut app, 0.5, 0.5));
    assert!(!app.canvas.shapes[0].contains_point(Point::new(200.0, 110.0)));
    assert!(app.canvas.shapes[0].contains_point(Point::new(200.0, 155.0)));

    // 3. カスタムピボット点 (200, 200) に対して、左側の点 (150, 200) にあるシェイプを左右反転 -> (250, 200) に移り、ワインディングも正常に保たれること
    tool_select::clear_selection(&mut app);
    app.canvas.shapes.clear();
    let left_piece = make_stroke(
        Point::new(130.0, 200.0),
        Point::new(170.0, 200.0),
        18.0,
        red,
    );
    merge_shapes_into_canvas(&mut app.canvas.shapes, left_piece);
    app.canvas.selected_shapes = vec![0];
    app.canvas.transform_pivot = Some(Point::new(200.0, 200.0));

    assert!(tool_transform::flip_selected_shapes(&mut app, true, false));
    assert!(
        !app.canvas.shapes[0].contains_point(Point::new(150.0, 200.0)),
        "反転前の位置には存在しないこと"
    );
    assert!(
        app.canvas.shapes[0].contains_point(Point::new(250.0, 200.0)),
        "ピボット (200, 200) を基準に左右対称の (250, 200) へ反転移動していること"
    );
    assert!(
        app.canvas.shapes[0].exterior.signed_area() > 0.0,
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
    merge_shapes_into_canvas(&mut app.canvas.shapes, s_red);

    let mut s_blue = make_stroke(
        Point::new(180.0, 200.0),
        Point::new(320.0, 200.0),
        20.0,
        blue,
    );
    for sh in &mut s_blue {
        sh.group_path = vec![1];
    }
    app.canvas.shapes.extend(s_blue);

    app.ui.brush_color = green;
    canvas_tool::set_tool(&mut app, Tool::Eyedropper);

    // 1. 空白部分をクリックしても色は変わらないこと
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(20.0, 20.0)),
        None
    );
    assert_eq!(app.ui.brush_color, green);

    // 2. 背面の赤シェイプ単独部分 (120, 200) をクリックすると赤になること
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(120.0, 200.0)),
        Some(red)
    );
    assert_eq!(app.ui.brush_color, red);

    // 3. 赤と青が重なる同一座標 (200, 200) をクリックすると最前面の青が優先して返ること
    assert!(app.canvas.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(app.canvas.shapes[1].contains_point(Point::new(200.0, 200.0)));
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(200.0, 200.0)),
        Some(blue),
        "同一座標で2つの図形が重なっている場合、最前面の図形色が優先されること"
    );
    assert_eq!(app.ui.brush_color, blue);
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
    merge_shapes_into_canvas(&mut app.canvas.shapes, face);
    merge_shapes_into_canvas(&mut app.canvas.shapes, eye_a);
    merge_shapes_into_canvas(&mut app.canvas.shapes, eye_b);
    assert_eq!(app.canvas.shapes.len(), 3);

    app.canvas.selected_shapes = vec![1, 2];
    let eye_gid =
        tool_select::group_selected_shapes(&mut app).expect("グループ化が成功すること");
    assert_eq!(app.canvas.shapes[1].group_path, vec![eye_gid]);
    assert_eq!(app.canvas.shapes[2].group_path, vec![eye_gid]);

    // 1つクリックするだけでグループ全体の2シェイプが同時選択されること
    tool_select::clear_selection(&mut app);
    assert!(tool_select::select_single_shape_at(
        &mut app,
        Point::new(100.0, 100.0)
    ));
    assert_eq!(app.canvas.selected_shapes.len(), 2);

    // グループ選択状態でバケツツールを使っても無効であること
    app.ui.brush_color = red;
    assert!(!tool_paint::bucket_fill_at(&mut app, Point::new(100.0, 100.0)));
    assert_eq!(app.canvas.shapes[1].color, blue);
    assert_eq!(app.canvas.shapes[2].color, green);

    // グループを赤い顔シェイプの上へ移動して選択解除しても下の生シェイプを切り抜かないこと
    let snap = app.canvas.shapes.clone();
    app.canvas.translate_selected(KVec2::new(0.0, 100.0));
    tool_select::commit_moved_selection(&mut app, snap);
    tool_select::clear_selection(&mut app);
    assert_eq!(app.canvas.shapes.len(), 3);
    assert!(app.canvas.shapes[0].contains_point(Point::new(100.0, 200.0)));
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
    merge_shapes_into_canvas(&mut app.canvas.shapes, face);
    app.canvas.shapes.extend(eye_a);
    app.canvas.shapes.extend(eye_b);

    app.canvas.selected_shapes = vec![1, 2];
    let eye_gid = tool_select::group_selected_shapes(&mut app).unwrap();
    app.canvas.selected_shapes = vec![0, 1, 2];
    let head_gid = tool_select::group_selected_shapes(&mut app).unwrap();
    assert_eq!(app.canvas.shapes[0].group_path, vec![head_gid]);
    assert_eq!(app.canvas.shapes[1].group_path, vec![head_gid, eye_gid]);
    assert_eq!(app.canvas.shapes[2].group_path, vec![head_gid, eye_gid]);

    assert!(tool_select::enter_group_at(&mut app, Point::new(200.0, 200.0)));
    assert_eq!(app.canvas.editing_group_path, vec![head_gid]);
    assert!(app.canvas.shapes[0].is_raw_at_scope(&app.canvas.editing_group_path));
    assert_eq!(
        app.canvas.shapes[1].active_group_id_at_scope(&app.canvas.editing_group_path),
        Some(eye_gid)
    );

    assert!(tool_select::enter_group_at(&mut app, Point::new(100.0, 200.0)));
    assert_eq!(app.canvas.editing_group_path, vec![head_gid, eye_gid]);
    assert!(app.canvas.shapes[1].is_raw_at_scope(&app.canvas.editing_group_path));

    assert!(tool_select::exit_current_group(&mut app));
    assert_eq!(app.canvas.editing_group_path, vec![head_gid]);
    assert!(tool_select::exit_current_group(&mut app));
    assert!(app.canvas.editing_group_path.is_empty());
    assert_eq!(app.canvas.selected_shapes.len(), 3);

    assert!(tool_select::ungroup_selected_shapes(&mut app));
    assert!(app.canvas.shapes[0].group_path.is_empty());
    assert_eq!(app.canvas.shapes[1].group_path, vec![eye_gid]);
    assert_eq!(app.canvas.shapes[2].group_path, vec![eye_gid]);
}

#[test]
fn test_zoom_around_cursor_preserves_world_point_and_clamps_bounds() {
    let mut app = Is2dApp::default();
    let cursor = Pos2::new(320.0, 240.0);

    let world_before = app.viewport.screen_to_world(cursor);
    app.viewport.zoom_around_screen_pos(cursor, 4.0);
    app.viewport.zoom_around_screen_pos(cursor, 2.0);
    assert!((app.viewport.zoom - 8.0).abs() < 1e-5);
    let world_after = app.viewport.screen_to_world(cursor);
    assert!(
        (world_before - world_after).hypot() < 1e-4,
        "カーソル中心ズームの前後でカーソル直下のワールド座標が一致すること"
    );
    let screen_roundtrip = app.viewport.world_to_screen(world_after);
    assert!((screen_roundtrip - cursor).length() < 1e-3);

    // 上限 (MAX_VIEW_ZOOM = 16.0) と下限 (MIN_VIEW_ZOOM = 0.15) でクランプされ、かつカーソル下の座標が保たれること
    app.viewport.zoom_around_screen_pos(cursor, 100.0);
    assert_eq!(app.viewport.zoom, crate::vector_engine::MAX_VIEW_ZOOM);
    assert!((app.viewport.screen_to_world(cursor) - world_before).hypot() < 1e-3);

    app.viewport.zoom_around_screen_pos(cursor, 0.001);
    assert_eq!(app.viewport.zoom, crate::vector_engine::MIN_VIEW_ZOOM);
    assert!((app.viewport.screen_to_world(cursor) - world_before).hypot() < 1e-3);

    app.viewport.reset_view();
    assert_eq!(app.viewport.zoom, 1.0);
    assert_eq!(app.viewport.pan, Vec2::ZERO);
}

#[test]
fn test_fine_brush_and_zoom_invariant_stroke_smoothing() {
    let mut app = Is2dApp::default();
    app.viewport.zoom = 8.0;
    app.ui.brush_radius = 0.5;
    let fine_raw = generate_brush_outline_at_zoom(
        &[Point::new(320.0, 240.0), Point::new(323.0, 240.0)],
        app.ui.brush_radius as f64,
        app.viewport.zoom as f64,
    );
    tool_paint::commit_raw_outline(&mut app, &fine_raw);
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(app.canvas.shapes[0].contains_point(Point::new(321.5, 240.0)));

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
    merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
    assert_eq!(app.canvas.shapes.len(), 1);

    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert!(tool_select::cut_selected_shapes(&mut app));
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(!app.canvas.shapes[0].contains_point(Point::new(120.0, 200.0)));
    assert!(app.canvas.shapes[0].contains_point(Point::new(260.0, 200.0)));

    assert!(tool_select::paste_clipboard(&mut app, true));
    assert_eq!(app.canvas.selected_shapes.len(), 1);
    assert!(
        app.canvas.shapes[app.canvas.selected_shapes[0]].contains_point(Point::new(120.0, 200.0))
    );
    tool_select::clear_selection(&mut app);
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(app.canvas.shapes[0].contains_point(Point::new(120.0, 200.0)));
    assert!(app.canvas.shapes[0].contains_point(Point::new(260.0, 200.0)));
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
    merge_shapes_into_canvas(&mut app.canvas.shapes, g_part);
    app.canvas.selected_shapes = vec![0];
    let orig_gid = tool_select::group_selected_shapes(&mut app).unwrap();

    assert!(tool_select::copy_selected_shapes(&mut app));
    // 高ズーム時でもワールド座標上の一定オフセット (+16, +16) で貼り付けられること
    app.viewport.zoom = 4.0;
    assert!(tool_select::paste_clipboard(&mut app, false));
    assert_eq!(app.canvas.shapes.len(), 2);
    let pasted_idx = app.canvas.selected_shapes[0];
    let pasted_gid = app.canvas.shapes[pasted_idx].group_path[0];
    assert_ne!(pasted_gid, orig_gid);
    assert!(
        app.canvas.shapes[pasted_idx].contains_point(Point::new(120.0 + 16.0, 100.0 + 16.0)),
        "Cmd+V ではズーム倍率によらずワールド座標 (+16, +16) ずれた位置に貼り付けられること"
    );

    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    let orig_inst_gid = tool_transform::selected_single_symbol_info(&app)
        .unwrap()
        .0
        .instance_gid;
    let count_before = app.canvas.shapes.len();

    let undo_snapshot = tool_select::duplicate_selected_shapes_for_alt_drag(&mut app);
    assert_eq!(app.canvas.shapes.len(), count_before + 1);
    app.canvas.translate_selected(KVec2::new(60.0, 40.0));
    tool_select::finish_move_selection_drag(&mut app, KVec2::new(60.0, 40.0), undo_snapshot);

    let (dup_binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
    assert_eq!(dup_binding.symbol_id, sym_id);
    assert_ne!(dup_binding.instance_gid, orig_inst_gid);
    assert!(
        app.canvas.shapes[app.canvas.selected_shapes[0]]
            .contains_point(Point::new(120.0 + 16.0 + 60.0, 100.0 + 16.0 + 40.0)),
    );
    app.undo();
    assert_eq!(app.canvas.shapes.len(), count_before);
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
    merge_shapes_into_canvas(&mut app.canvas.shapes, s0);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert_eq!(app.timeline.current_frame, 1);
    assert_eq!(app.canvas.shapes.len(), 1);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.timeline.current_frame, 2);
    assert!(app.canvas.shapes.is_empty());

    let prev_total = app.timeline.total_frames;
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.timeline.total_frames, prev_total + 1);
    assert!(timeline_ui::timeline_remove_frame(&mut app));
    assert_eq!(app.timeline.total_frames, prev_total);

    timeline_ui::select_timeline_cells(&mut app, 0, 9, 0, 9);
    assert_eq!(app.timeline.current_frame, 9);
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.timeline.layers[0].frame_count, 10);
    assert_eq!(app.timeline.total_frames, 10);
    app.undo();
    assert_eq!(app.timeline.total_frames, prev_total);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);

    assert!(timeline_ui::timeline_delete_keyframe(&mut app));
    assert_eq!(app.canvas.shapes.len(), 1);
    app.undo();
    assert!(app.canvas.shapes.is_empty());
    app.redo();
    assert_eq!(app.canvas.shapes.len(), 1);
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
    merge_shapes_into_canvas(&mut app.canvas.shapes, s0);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.timeline.layers.len(), 2);
    assert_eq!(app.timeline.active_layer, 1);
    assert!(app.timeline.active_cel_shapes().is_empty());
    assert_eq!(app.canvas.shapes.len(), 1);
    app.ui.brush_color = blue;
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(125.0, 100.0)),
        Some(red)
    );

    app.ui.brush_color = blue;
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
    assert_eq!(app.timeline.layers[0].cel_at(0).unwrap().shapes.len(), 1);
    assert_eq!(app.timeline.layers[1].cel_at(0).unwrap().shapes.len(), 1);
    assert_eq!(app.canvas.shapes.len(), 2);

    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(200.0, 200.0),
        false,
    );
    assert_eq!(app.canvas.selected_shapes.len(), 2);
    let undo_snap = app.canvas.shapes.clone();
    app.canvas.translate_selected(KVec2::new(30.0, 0.0));
    tool_select::commit_moved_selection(&mut app, undo_snap);
    tool_select::clear_selection(&mut app);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert!(app.timeline.layers[0].cel_at(0).unwrap().shapes[0]
        .contains_point(Point::new(155.0, 100.0)));
    assert!(app.timeline.layers[1].cel_at(0).unwrap().shapes[0]
        .contains_point(Point::new(155.0, 100.0)));

    timeline_ui::set_layer_locked(&mut app, 0, true);
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(250.0, 200.0),
        false,
    );
    assert_eq!(app.canvas.selected_shapes.len(), 1);
    tool_select::clear_selection(&mut app);
    timeline_ui::set_layer_locked(&mut app, 0, false);
    assert_eq!(app.canvas.shapes.len(), 2);
}

#[test]
fn test_timeline_layer_reorder_move_and_delete() {
    let mut app = Is2dApp::default();
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.timeline.layers.len(), 3);
    assert_eq!(
        app.timeline
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 1", "Layer 2", "Layer 3"]
    );

    assert!(timeline_ui::timeline_reorder_layer(&mut app, 2, 0));
    assert_eq!(app.timeline.active_layer, 0);
    assert_eq!(
        app.timeline
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 3", "Layer 1", "Layer 2"]
    );
    app.undo();
    assert_eq!(
        app.timeline
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 1", "Layer 2", "Layer 3"]
    );
    app.redo();
    assert_eq!(
        app.timeline
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 3", "Layer 1", "Layer 2"]
    );

    assert!(timeline_ui::timeline_move_active_layer(&mut app, true));
    assert_eq!(app.timeline.active_layer, 1);
    assert!(timeline_ui::timeline_delete_active_layer(&mut app));
    assert_eq!(app.timeline.layers.len(), 2);
    assert!(timeline_ui::timeline_delete_active_layer(&mut app));
    assert_eq!(app.timeline.layers.len(), 1);
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
    app.ui.brush_color = blue;
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
    let base_len = app.timeline.layers[0].frame_count;

    timeline_ui::select_timeline_cells(&mut app, 1, 0, 1, 0);
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.timeline.layers[0].frame_count, base_len);
    assert_eq!(app.timeline.layers[1].frame_count, base_len + 1);

    timeline_ui::scrub_ruler_to_frame(&mut app, 0);
    assert!(app.timeline.cell_selection.is_none());
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.timeline.layers[0].frame_count, base_len + 1);
    assert_eq!(app.timeline.layers[1].frame_count, base_len + 2);

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert_eq!(app.canvas.selected_shapes.len(), 1);
    assert_eq!(
        app.canvas.shapes[app.canvas.selected_shapes[0]].layer_id,
        app.timeline.layers[0].id
    );

    tool_select::clear_selection(&mut app);
    timeline_ui::scrub_ruler_to_frame(&mut app, 0);
    assert!(tool_select::select_single_shape_at(
        &mut app,
        Point::new(160.0, 60.0)
    ));
    assert_eq!(app.timeline.active_layer, 1);
    assert_eq!(
        app.timeline
            .cell_selection
            .map(|s| (s.min_layer, s.max_layer)),
        Some((1, 1))
    );

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 1, 0);
    assert_eq!(app.canvas.selected_shapes.len(), 2);
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
    app.ui.brush_color = blue;
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
    assert!(!app.timeline.layers[1].cel_at(0).unwrap().is_empty());
    assert_eq!(app.timeline.layers[1].keyframe_index_at(3), Some(1));
    assert!(!app.timeline.layers[1].cel_at(3).unwrap().is_empty());

    let sel_l1_f3 = timeline::TimelineCellSelection::single(1, 3);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut app, sel_l1_f3, 0, 2, false
    ));
    assert_eq!(app.timeline.layers[1].keyframes[1].start_frame, 5);
    assert!(!app.timeline.layers[1].cel_at(5).unwrap().is_empty());

    let sel_l0_f0 = timeline::TimelineCellSelection::single(0, 0);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut app, sel_l0_f0, 0, 2, false
    ));
    assert!(app.timeline.layers[0].cel_at(0).unwrap().is_empty());
    assert!(!app.timeline.layers[0].cel_at(2).unwrap().is_empty());

    app.timeline.layers[1].insert_keyframe(3, false);
    app.timeline.layers[1].insert_keyframe(4, false);
    let kf_count_before = app.timeline.layers[1].keyframes.len();
    timeline_ui::select_timeline_cells(&mut app, 1, 0, 1, 0);
    assert!(timeline_ui::timeline_remove_frame(&mut app));
    assert_eq!(app.timeline.layers[1].keyframes.len(), kf_count_before);

    timeline_ui::select_timeline_cells(&mut app, 1, 2, 1, 2);
    assert!(timeline_ui::timeline_remove_frame(&mut app));
    assert_eq!(app.timeline.layers[1].keyframes.len(), kf_count_before - 1);

    let cel = app.timeline.layers[1].cel_at(0).unwrap();
    let m1 = cel.cached_screen_mesh(1.0, Vec2::ZERO, false);
    let m2 = cel.cached_screen_mesh(1.0, Vec2::ZERO, false);
    assert!(!m1.is_empty());
    assert!(std::sync::Arc::ptr_eq(&m1, &m2));
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    app.timeline.current_frame = 0;
    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing);
    assert_eq!(app.canvas.shapes.len(), 1);
}

#[test]
fn test_adobe_animate_click_selects_edge_and_selected_shape_disables_deformation() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    canvas_tool::set_tool(&mut app, Tool::Select);

    // 細い 2px ブラシストロークを描画（ほぼ全域が境界線の吸着範囲内になる細い線）
    let thin_stroke = make_stroke(Point::new(100.0, 200.0), Point::new(220.0, 200.0), 2.0, red);
    merge_shapes_into_canvas(&mut app.canvas.shapes, thin_stroke);
    assert_eq!(app.canvas.shapes.len(), 1);

    let vertex_pt = app.canvas.shapes[0].exterior.segments[0].p0;
    let edge_pt = app.canvas.shapes[0].exterior.segments[0].eval(0.5);

    // 1. 未選択状態では、頂点や境界線にホバーすると Vertex / Edge（変形対象）になること
    assert!(matches!(
        tool_select::compute_hover_target(
            &app.canvas,
            vertex_pt,
            app.viewport.zoom,
            app.ui.select_sub_mode
        ),
        HoverTarget::Vertex { shape_idx: 0, .. }
    ));
    assert!(matches!(
        tool_select::compute_hover_target(
            &app.canvas,
            edge_pt,
            app.viewport.zoom,
            app.ui.select_sub_mode
        ),
        HoverTarget::Edge { shape_idx: 0, .. } | HoverTarget::Vertex { shape_idx: 0, .. }
    ));

    // 2. 細い線の境界線や頂点の上でクリック（select_single_shape_at）した際にも、確実に選択状態になること
    assert!(
        tool_select::select_single_shape_at(&mut app, vertex_pt),
        "頂点や境界線上をクリックしてもシェイプが選択状態になること"
    );
    assert_eq!(app.canvas.selected_shapes, vec![0]);

    // 3. 選択状態（網点表示）になった後は、頂点上・境界線上・少し外側のマージンのどこを掴んでも
    //    変形（Vertex / Edge）は無効化され、常に移動（HoverTarget::Fill）になること
    assert_eq!(
        tool_select::compute_hover_target(
            &app.canvas,
            vertex_pt,
            app.viewport.zoom,
            app.ui.select_sub_mode
        ),
        HoverTarget::Fill { shape_idx: 0 },
        "選択状態のシェイプの頂点上では変形ではなく移動（Fill）になること"
    );
    assert_eq!(
        tool_select::compute_hover_target(
            &app.canvas,
            edge_pt,
            app.viewport.zoom,
            app.ui.select_sub_mode
        ),
        HoverTarget::Fill { shape_idx: 0 },
        "選択状態のシェイプの境界線上では変形ではなく移動（Fill）になること"
    );
    assert_eq!(
        tool_select::compute_hover_target(
            &app.canvas,
            Point::new(160.0, 204.5),
            app.viewport.zoom,
            app.ui.select_sub_mode
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
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(200.0, 150.0),
        16.0,
        Color32::from_rgb(230, 80, 80),
    );
    app.canvas.selected_shapes = vec![0];

    let sym_id =
        tool_transform::convert_selection_to_symbol(&mut app).expect("シンボル変換が成功すること");
    assert_eq!(app.symbols.library.len(), 1);
    assert_eq!(app.symbols.library[0].id, sym_id);
    assert_eq!(app.symbols.library[0].name, "Symbol 1");
    assert!(tool_transform::selected_single_symbol_info(&app).is_some());

    assert!(tool_transform::place_symbol_instance_from_library(
        &mut app, sym_id
    ));
    assert_eq!(app.canvas.shapes.len(), 2);
    assert_ne!(
        app.canvas.shapes[0].symbol_instance.unwrap().instance_gid,
        app.canvas.shapes[1].symbol_instance.unwrap().instance_gid
    );

    assert!(tool_select::enter_group_at(
        &mut app,
        Point::new(150.0, 150.0)
    ));
    assert_eq!(app.symbols.edit_stack.len(), 1);
    assert_eq!(app.timeline.total_frames, 1);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert_eq!(app.timeline.total_frames, 2);
    assert_eq!(app.timeline.current_frame, 1);

    let hat = make_stroke(
        Point::new(120.0, 80.0),
        Point::new(180.0, 80.0),
        10.0,
        Color32::from_rgb(50, 120, 240),
    )
    .remove(0);
    app.canvas.shapes.push(hat);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    assert!(tool_select::exit_current_group(&mut app));
    assert!(app.symbols.edit_stack.is_empty());
    assert_eq!(app.symbols.library[0].timeline.total_frames, 2);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.timeline.total_frames, 3);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.canvas.shapes.len(), 2);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(app.canvas.shapes.len(), 4);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(app.canvas.shapes.len(), 2);
}

#[test]
fn test_classic_tween_requires_single_symbol_instance() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    assert!(!tool_transform::timeline_toggle_tween(&mut app));

    app.canvas.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(200.0, 150.0),
        16.0,
        Color32::from_rgb(230, 80, 80),
    );
    assert!(!tool_transform::timeline_toggle_tween(&mut app));

    app.canvas.selected_shapes = vec![0];
    assert!(tool_select::group_selected_shapes(&mut app).is_some());
    assert!(!tool_transform::timeline_toggle_tween(&mut app));
    assert!(tool_select::ungroup_selected_shapes(&mut app));

    app.canvas.selected_shapes = vec![0];
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert!(tool_transform::timeline_toggle_tween(&mut app));
    assert!(app.timeline.layers[0].keyframes[0].tween);
    assert_eq!(
        app.timeline.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete,
    );

    assert!(tool_transform::timeline_toggle_tween(&mut app));
    assert!(!app.timeline.layers[0].keyframes[0].tween);

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
    app.canvas.shapes.push(raw_extra);
    assert!(!tool_transform::timeline_toggle_tween(&mut app));
}

#[test]
fn test_classic_tween_translation_and_custom_pivot_ccw_rotation() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(200.0, 150.0),
        16.0,
        Color32::from_rgb(230, 80, 80),
    );
    app.canvas.selected_shapes = vec![0];
    tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert!(tool_transform::timeline_toggle_tween(&mut app));
    for _ in 0..4 {
        assert!(timeline_ui::timeline_extend_frame(&mut app));
    }
    assert_eq!(app.timeline.total_frames, 5);
    assert_eq!(
        app.timeline.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete
    );

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert!(matches!(
        app.timeline.layers[0].tween_state_at_keyframe(0),
        TweenState::Complete {
            start_frame: 0,
            end_frame: 4,
            ..
        }
    ));

    app.canvas.selected_shapes = vec![0];
    let snap = app.canvas.shapes.clone();
    app.canvas.translate_selected(KVec2::new(200.0, 80.0));
    tool_select::commit_moved_selection(&mut app, snap);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_bbox = app.canvas.shapes[0].bounding_box();
    let mid_cx = (mid_bbox.x0 + mid_bbox.x1) * 0.5;
    let mid_cy = (mid_bbox.y0 + mid_bbox.y1) * 0.5;
    assert!((mid_cx - 250.0).abs() < 1.0);
    assert!((mid_cy - 190.0).abs() < 1.0);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    let start_bbox = app.canvas.shapes[0].bounding_box();
    let start_cx = (start_bbox.x0 + start_bbox.x1) * 0.5;
    let start_cy = (start_bbox.y0 + start_bbox.y1) * 0.5;
    assert!((start_cx - 150.0).abs() < 1.0);
    assert!((start_cy - 150.0).abs() < 1.0);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    app.canvas.selected_shapes = vec![0];
    let snap_reset = app.canvas.shapes.clone();
    app.canvas.translate_selected(KVec2::new(-200.0, -80.0));
    tool_select::commit_moved_selection(&mut app, snap_reset);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    let joint_world = Point::new(100.0, 150.0);
    assert!(tool_transform::set_selected_symbol_world_pivot(
        &mut app,
        Some(joint_world),
        true
    ));
    tool_select::clear_selection(&mut app);
    app.canvas.selected_shapes = vec![0];
    let bbox_f4 = app
        .canvas
        .selection_bounding_box(app.viewport.zoom)
        .unwrap();
    assert!((app.canvas.active_transform_pivot(bbox_f4) - joint_world).hypot() < 1e-4);
    assert!(tool_transform::rotate_selected_shapes(
        &mut app,
        std::f64::consts::FRAC_PI_2
    ));
    canvas_tool::sync_shapes_to_timeline(&mut app);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_binding = app.canvas.shapes[0].symbol_instance.unwrap();
    assert!((mid_binding.world_pivot() - joint_world).hypot() < 1e-4);
    let expected_mid_center = Point::new(
        100.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        150.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
    );
    assert!(app.canvas.shapes[0].contains_point(expected_mid_center));

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert!(app.ui.inspect_timeline_frame);
    tool_select::select_single_shape_at(&mut app, Point::new(150.0, 150.0));
    assert!(!app.ui.inspect_timeline_frame);

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
    assert!(app.canvas.shapes[0].contains_point(expected_ccw_mid_center));
}

#[test]
fn test_symbol_playback_modes_loop_play_once_and_single_frame() {
    use crate::symbol::SymbolLoopMode;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(60, 190, 90);
    let blue = Color32::from_rgb(60, 110, 230);
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    app.canvas.selected_shapes = vec![0];
    assert!(tool_transform::convert_selection_to_symbol(&mut app).is_some());

    let (binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
    assert!(tool_transform::enter_symbol_timeline(&mut app, binding));

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        green,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        blue,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert_eq!(app.timeline.total_frames, 3);

    assert!(tool_transform::exit_symbol_timeline(&mut app));
    for _ in 0..4 {
        assert!(timeline_ui::timeline_extend_frame(&mut app));
    }
    assert_eq!(app.timeline.total_frames, 5);

    let expected_loop = [red, green, blue, red, green];
    for (f, &col) in expected_loop.iter().enumerate() {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(app.canvas.shapes[0].color, col, "Loop mode at frame {f}");
    }

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.canvas.selected_shapes = vec![0];
    assert!(tool_transform::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::PlayOnce,
        0
    ));
    let expected_once = [red, green, blue, blue, blue];
    for (f, &col) in expected_once.iter().enumerate() {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(app.canvas.shapes[0].color, col, "PlayOnce mode at frame {f}");
    }

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.canvas.selected_shapes = vec![0];
    assert!(tool_transform::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::SingleFrame,
        1
    ));
    assert_eq!(app.canvas.shapes[0].color, green);
    assert_eq!(app.canvas.selected_shapes, vec![0]);
    for f in 0..5 {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(app.canvas.shapes[0].color, green);
    }
}

#[test]
fn test_nested_symbol_recursive_frame_evaluation_and_blank_frame_marker() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 110, 230);

    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    app.canvas.selected_shapes = vec![0];
    let child_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    let (child_binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
    assert!(tool_transform::enter_symbol_timeline(
        &mut app,
        child_binding
    ));
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        blue,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert!(tool_transform::exit_symbol_timeline(&mut app));

    app.canvas.selected_shapes = vec![0];
    let parent_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert_ne!(child_sym_id, parent_sym_id);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.canvas.shapes[0].color, red);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(app.canvas.shapes[0].color, blue);

    assert!(tool_transform::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.timeline.total_frames, 3);
    assert!(tool_transform::exit_symbol_timeline(&mut app));

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(app.canvas.shapes[0].is_empty_symbol_marker());
    canvas_tool::select_shapes_for_timeline_selection(&mut app);
    assert_eq!(app.canvas.selected_shapes, vec![0]);
    assert!(app.canvas.selected_single_symbol_binding().is_some());
    tool_select::clear_selection(&mut app);
    assert!(app
        .timeline
        .resolved_cel_screen_mesh(0, 2, 1.0, Vec2::ZERO, false, &app.symbols.library)
        .is_none());

    let extra = make_stroke(
        Point::new(220.0, 100.0),
        Point::new(260.0, 100.0),
        8.0,
        Color32::BLACK,
    )
    .remove(0);
    app.canvas.shapes.push(extra);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.canvas.shapes.len(), 2);
}

#[test]
fn test_symbol_rename_library_delete_detach_and_inplace_child_symbol_creation() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    app.canvas.selected_shapes = vec![0];
    let child_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    app.canvas.selected_shapes = vec![0];
    let parent_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();

    let orig_name = app.symbols.find_symbol(parent_sym_id).unwrap().name.clone();
    assert!(tool_transform::rename_symbol_in_library(
        &mut app,
        parent_sym_id,
        "HeroSymbol".to_string()
    ));
    assert_eq!(
        app.symbols.find_symbol(parent_sym_id).unwrap().name,
        "HeroSymbol"
    );
    app.undo();
    assert_eq!(
        app.symbols.find_symbol(parent_sym_id).unwrap().name,
        orig_name
    );

    timeline_ui::set_layer_locked(&mut app, 0, true);
    assert!(app.timeline.layers[0].locked);
    app.undo();
    assert!(!app.timeline.layers[0].locked);

    assert!(tool_transform::delete_symbol_from_library(
        &mut app,
        parent_sym_id
    ));
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(app.canvas.shapes[0].symbol_instance.is_none());
    assert!(!app.canvas.shapes[0].group_path.is_empty());

    assert!(tool_transform::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.canvas.selected_shapes = vec![0];
    let lib_len_before = app.symbols.library.len();
    let grandchild_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    assert_eq!(app.symbols.library.len(), lib_len_before + 1);
    assert!(!app
        .symbols
        .find_symbol(child_sym_id)
        .unwrap()
        .evaluated_local_shapes_at(0, &app.symbols.library)
        .is_empty());
    app.undo();
    assert_eq!(app.symbols.library.len(), lib_len_before);
    app.redo();
    assert!(app.symbols.find_symbol(grandchild_sym_id).is_some());
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
    app.canvas.shapes = make_stroke(Point::new(50.0, 100.0), Point::new(200.0, 100.0), 14.0, red);
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

    let clean_shapes = app.canvas.shapes.clone();
    let clean_bbox = app.canvas.shapes[0].bounding_box();
    app.canvas.drag = crate::canvas_tool::ActiveCanvasDrag::Select(
        crate::canvas_tool::SelectDragState::MoveShape(crate::canvas_tool::MoveSelectionDrag::new(
            Point::ZERO,
            clean_shapes,
        )),
    );
    app.canvas.shapes[0].translate(KVec2::new(500.0, 500.0));
    tool_select::clear_selection(&mut app);
    assert_eq!(app.canvas.shapes[0].bounding_box(), clean_bbox);

    let stroke = vec![Point::new(10.0, 10.0), Point::new(40.0, 10.0)];
    let tail_outline = generate_brush_outline(&stroke, 8.0);
    let tail_mesh = crate::vector_engine::tessellate_raw_polygon(&tail_outline, Color32::BLACK);
    app.canvas.drag = crate::canvas_tool::ActiveCanvasDrag::PaintStroke {
        stroke,
        committed_mesh: egui::Mesh::default(),
        tail_outline,
        tail_mesh,
        tail_start_idx: 0,
    };
    tool_select::clear_selection(&mut app);
    assert!(matches!(
        app.canvas.drag,
        crate::canvas_tool::ActiveCanvasDrag::PaintStroke { .. }
    ));
    canvas_tool::set_tool(&mut app, Tool::Brush);
    assert!(matches!(
        app.canvas.drag,
        crate::canvas_tool::ActiveCanvasDrag::None
    ));

    let b = app
        .canvas
        .shapes
        .iter()
        .find_map(|s| s.symbol_instance)
        .unwrap();
    let sym_id = b.symbol_id;
    let orig_local_shape = app
        .symbols
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
        .symbols
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
    sym_app.canvas.shapes = two_shapes;
    sym_app.canvas.selected_shapes = vec![0, 1];
    let empty_test_sym_id = tool_transform::convert_selection_to_symbol(&mut sym_app).unwrap();
    assert_eq!(sym_app.canvas.shapes.len(), 2);
    assert!(sym_app.timeline.layers[0].keyframes[0]
        .cel
        .shapes
        .is_empty());
    assert_eq!(
        sym_app.timeline.layers[0].keyframes[0].cel.instances.len(),
        1
    );
    assert!(tool_transform::enter_symbol_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    assert!(tool_select::clear_active_frame_shapes(&mut sym_app));
    assert!(tool_transform::exit_symbol_timeline(&mut sym_app));

    assert_eq!(sym_app.canvas.shapes.len(), 1);
    assert!(sym_app.canvas.shapes[0].is_empty_symbol_marker());
    assert!(sym_app.timeline.layers[0].keyframes[0]
        .cel
        .shapes
        .is_empty());
    assert_eq!(
        sym_app.timeline.layers[0].keyframes[0].cel.instances.len(),
        1
    );
    assert!(sym_app.timeline.layers[0].keyframes[0]
        .cel
        .single_symbol_instance()
        .is_some());
    assert!(tool_transform::place_symbol_instance_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    assert_eq!(sym_app.canvas.shapes.len(), 2);
    assert!(sym_app.canvas.shapes[1].is_empty_symbol_marker());
    sym_app.undo();
    assert_eq!(sym_app.canvas.shapes.len(), 1);

    assert!(tool_transform::enter_symbol_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    sym_app.canvas.shapes = make_stroke(
        Point::new(210.0, 200.0),
        Point::new(250.0, 200.0),
        8.0,
        Color32::from_rgb(60, 180, 90),
    );
    canvas_tool::sync_shapes_to_timeline(&mut sym_app);
    assert!(tool_transform::exit_symbol_timeline(&mut sym_app));
    assert_eq!(sym_app.canvas.shapes.len(), 1);
    assert!(sym_app.canvas.shapes[0].contains_point(Point::new(230.0, 200.0)));

    let _ = sym_app.symbols.library[0].timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .fill_mesh();
    assert!(sym_app.symbols.library[0].timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .has_cached_fill_mesh());
    let orig_stage = sym_app.viewport.stage_size;
    sym_app.push_undo();
    let last_undo = sym_app.history.undo_stack.back().unwrap();
    assert!(!last_undo.library[0].timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .has_cached_fill_mesh());
    sym_app.viewport.stage_size = Vec2::new(1920.0, 1080.0);
    sym_app.undo();
    assert_eq!(sym_app.viewport.stage_size, orig_stage);
    sym_app.redo();
    assert_eq!(sym_app.viewport.stage_size, Vec2::new(1920.0, 1080.0));

    let sel_m1 = sym_app.canvas.shapes[0].cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
    let sel_m2 = sym_app.canvas.shapes[0].cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
    assert!(std::sync::Arc::ptr_eq(&sel_m1, &sel_m2));
    let preview_rect =
        eframe::egui::Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(200.0, 110.0));
    let (prev_m1, orig1) = sym_app.symbols.library[0]
        .cached_mini_preview(
            preview_rect,
            sym_app.symbols.revision,
            &sym_app.symbols.library,
        )
        .unwrap();
    let (prev_m2, orig2) = sym_app.symbols.library[0]
        .cached_mini_preview(
            preview_rect,
            sym_app.symbols.revision,
            &sym_app.symbols.library,
        )
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&prev_m1, &prev_m2) && orig1 == orig2);
}

#[test]
fn test_reverse_order_nested_symbol_topological_propagation_and_cycle_guard() {
    let mut nest_app = Is2dApp::default();
    nest_app.canvas.shapes = make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(100, 100, 100),
    );
    nest_app.canvas.selected_shapes = vec![0];
    let sym_a = tool_transform::convert_selection_to_symbol(&mut nest_app).unwrap();

    nest_app.canvas.shapes.extend(make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(150, 150, 150),
    ));
    let last_idx = nest_app.canvas.shapes.len() - 1;
    nest_app.canvas.selected_shapes = vec![last_idx];
    let sym_b = tool_transform::convert_selection_to_symbol(&mut nest_app).unwrap();
    tool_select::delete_selected_shapes(&mut nest_app);

    nest_app.canvas.shapes.extend(make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(200, 50, 50),
    ));
    let last_idx = nest_app.canvas.shapes.len() - 1;
    nest_app.canvas.selected_shapes = vec![last_idx];
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
    for s in &mut nest_app.canvas.shapes {
        s.set_color(updated_green);
    }
    canvas_tool::sync_shapes_to_timeline(&mut nest_app);
    assert!(tool_transform::exit_symbol_timeline(&mut nest_app));
    assert_eq!(nest_app.canvas.shapes.len(), 1);
    assert_eq!(nest_app.canvas.shapes[0].color, updated_green);

    let scaled_binding =
        crate::symbol::SymbolInstanceBinding::new(999, sym_c, kurbo::Affine::scale(3.0));
    let sym_c_def = nest_app.symbols.find_symbol(sym_c).unwrap();
    let inst_shapes = scaled_binding.instantiate_shapes(
        &sym_c_def.timeline.layers[0].keyframes[0].cel.shapes,
        1,
        &[],
    );
    assert!(inst_shapes[0].has_cached_fill_mesh());

    assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_b));
    assert!(!nest_app.symbols.can_place_symbol_in_current_context(sym_a));
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
    tween_eye_app.canvas.shapes = make_stroke(
        Point::new(90.0, 200.0),
        Point::new(110.0, 200.0),
        12.0,
        tween_color,
    );
    tween_eye_app.canvas.selected_shapes = vec![0];
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
    grp_sym_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(120.0, 100.0),
        10.0,
        Color32::from_rgb(200, 40, 40),
    );
    grp_sym_app.canvas.selected_shapes = vec![0];
    let blink_sym_id = tool_transform::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
    // シンボル内部に入り、2フレーム目を空白キーフレーム（F7）にする（F0: 表示, F1: 空白）
    let blink_binding = grp_sym_app.canvas.shapes[0].symbol_instance.unwrap();
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
    grp_sym_app.canvas.shapes.extend(sib_shapes);
    grp_sym_app.canvas.selected_shapes = vec![0, 1];
    assert!(tool_select::group_selected_shapes(&mut grp_sym_app).is_some());
    // ルートシーンを3フレームに延長（F0: シンボル表示, F1: シンボル空白, F2: シンボルループ再表示）
    timeline_ui::select_timeline_cells(&mut grp_sym_app, 0, 2, 0, 2);
    assert!(timeline_ui::timeline_extend_frame(&mut grp_sym_app));

    // F1（シンボルが空白フレームで消えている状態）へ移動し、親グループを (+150, 0) 平行移動する
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    assert_eq!(
        grp_sym_app.canvas.shapes.len(),
        2,
        "F1では空白シンボルマーカー1個と兄弟シェイプ1個の計2個が CanvasState::shapes 内に一元展開されていること"
    );
    grp_sym_app.canvas.selected_shapes = grp_sym_app.canvas.expand_to_group_unit(0);
    assert_eq!(grp_sym_app.canvas.selected_shapes, vec![0, 1]);
    assert!(tool_select::nudge_selected_shapes(
        &mut grp_sym_app,
        kurbo::Vec2::new(150.0, 0.0)
    ));

    // F2（シンボルがループして再び表示されるフレーム）へ進むと、空白フレーム中に移動した親グループに追従してシンボルも (+150, 0) 移動していること
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    assert_eq!(grp_sym_app.canvas.shapes.len(), 2);
    let moved_sym_shape = grp_sym_app
        .canvas
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
    grp_sym_app.push_undo();
    grp_sym_app.push_undo();
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
    grp_sym_app.canvas.selected_shapes = grp_sym_app.canvas.expand_to_group_unit(0);
    assert!(tool_transform::rotate_selected_shapes(
        &mut grp_sym_app,
        std::f64::consts::FRAC_PI_2
    ));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    let rotated_sym_shape = grp_sym_app
        .canvas
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
    grp_sym_app.canvas.selected_shapes = grp_sym_app.canvas.expand_to_group_unit(0);
    assert!(tool_select::delete_selected_shapes(&mut grp_sym_app));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    assert!(
        grp_sym_app.canvas.shapes.is_empty(),
        "空白フレーム中に親グループを削除すると、内部の空白フレームシンボルも残骸にならず一緒に削除されること"
    );
}

#[test]
fn test_resolved_cel_screen_and_onion_mesh_cache_and_pan_invalidation() {
    let mut grp_sym_app = Is2dApp::default();
    grp_sym_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(120.0, 100.0),
        10.0,
        Color32::from_rgb(200, 40, 40),
    );
    grp_sym_app.canvas.selected_shapes = vec![0];
    let blink_sym_id = tool_transform::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
    let blink_binding = grp_sym_app.canvas.shapes[0].symbol_instance.unwrap();
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
    grp_sym_app.canvas.shapes = make_stroke(
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
        grp_sym_app.timeline.layers[0].keyframes[0]
            .cel
            .instances
            .len(),
        1,
        "Frame 0 時点で Cel::instances にシンボルインスタンスが登録されていること"
    );
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    assert_eq!(
        grp_sym_app.canvas.shapes.len(),
        1,
        "Frame 1 に進むと配置したシンボルの2コマ目の図形がステージ中央に出現すること"
    );
    let stage_center = grp_sym_app.viewport.stage_center_world();
    assert!(grp_sym_app.canvas.shapes[0].contains_point(stage_center));

    // シンボルを含むセルのスクリーンメッシュおよびオニオンスキンメッシュのキャッシュ再利用（Arcポインタ共有）検証
    let m_scr_1 = grp_sym_app
        .timeline
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.symbols.library)
        .unwrap();
    let m_scr_2 = grp_sym_app
        .timeline
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.symbols.library)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&m_scr_1, &m_scr_2),
        "キャッシュヒット時は頂点配列をディープコピーせず同一の Arc<Mesh> を O(1) 返却すること"
    );

    // パン（pan）が変化した際は resolved_cel_screen_mesh のスクリーン座標キャッシュが再計算されること (#3)
    let m_scr_panned = grp_sym_app
        .timeline
        .resolved_cel_screen_mesh(
            0,
            1,
            1.0,
            Vec2::new(50.0, -30.0),
            false,
            &grp_sym_app.symbols.library,
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
        .timeline
        .resolved_cel_onion_mesh(
            0,
            1,
            1.0,
            Vec2::ZERO,
            ([35, 175, 105], 90),
            &grp_sym_app.symbols.library,
        )
        .unwrap();
    let m_onion_2 = grp_sym_app
        .timeline
        .resolved_cel_onion_mesh(
            0,
            1,
            1.0,
            Vec2::ZERO,
            ([35, 175, 105], 90),
            &grp_sym_app.symbols.library,
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
    let second_sym_id = grp_sym_app.symbols.register_new_symbol(second_sym_shapes);
    let m_scr_before_unreferenced_edit = grp_sym_app
        .timeline
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.symbols.library)
        .unwrap();
    grp_sym_app
        .symbols
        .rename_symbol(second_sym_id, "UnreferencedSymbolEdited".to_string());
    let m_scr_after_unreferenced_edit = grp_sym_app
        .timeline
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.symbols.library)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(
            &m_scr_before_unreferenced_edit,
            &m_scr_after_unreferenced_edit
        ),
        "当該セルが直接・間接に参照していない別シンボルの revision が更新されても解決済みセルキャッシュが無駄に破棄されないこと"
    );
    grp_sym_app
        .symbols
        .rename_symbol(blink_sym_id, "ReferencedBlinkSymbolEdited".to_string());
    let m_scr_after_referenced_edit = grp_sym_app
        .timeline
        .resolved_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, false, &grp_sym_app.symbols.library)
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
            .symbols
            .find_symbol(blink_sym_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[1]
            .cel
            .shapes,
    );
    let inst_b = grp_sym_app.timeline.layers[0].keyframes[0].cel.instances[0].binding;
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    grp_sym_app.canvas.shapes.extend(make_stroke(
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
    let session = grp_sym_app.symbols.edit_stack.last().unwrap();
    let bd_m1 = session
        .backdrop_cel
        .cached_backdrop_screen_mesh(0, 1.0, Vec2::ZERO, &grp_sym_app.symbols.library)
        .expect("背景シェイプが存在するため backdrop メッシュが生成されること");
    let bd_m2 = session
        .backdrop_cel
        .cached_backdrop_screen_mesh(0, 1.0, Vec2::ZERO, &grp_sym_app.symbols.library)
        .expect("キャッシュ済みの backdrop メッシュが返ること");
    assert!(std::sync::Arc::ptr_eq(&bd_m1, &bd_m2));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    grp_sym_app.push_undo();
    grp_sym_app.canvas.shapes.extend(make_stroke(
        Point::new(100.0, 100.0),
        Point::new(120.0, 100.0),
        8.0,
        Color32::RED,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut grp_sym_app);
    assert!(tool_transform::exit_symbol_timeline(&mut grp_sym_app));
    let f1_local_arc_after = &grp_sym_app
        .symbols
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
    // オニオンスキンやループ再生などのビュー設定（TimelineUiState）を変更した後に Undo / Redo を実行しても巻き戻らないこと
    let mut sort_app = Is2dApp::default();
    sort_app.push_undo();
    sort_app.timeline_ui.onion_skin_enabled = true;
    sort_app.timeline_ui.onion_prev_frames = 5;
    sort_app.timeline_ui.onion_next_frames = 4;
    sort_app.timeline_ui.loop_playback = false;
    sort_app.timeline.fps = 30;
    sort_app.undo();
    assert!(sort_app.timeline_ui.onion_skin_enabled);
    assert_eq!(sort_app.timeline_ui.onion_prev_frames, 5);
    assert_eq!(sort_app.timeline_ui.onion_next_frames, 4);
    assert!(!sort_app.timeline_ui.loop_playback);
    sort_app.redo();
    assert!(sort_app.timeline_ui.onion_skin_enabled);
    assert_eq!(sort_app.timeline_ui.onion_prev_frames, 5);
    assert_eq!(sort_app.timeline_ui.onion_next_frames, 4);
    assert!(!sort_app.timeline_ui.loop_playback);

    // `FillShape::shape_id` によるレイヤー順ソート後の選択状態追従と `LoadedShapeSig` の一致
    timeline_ui::timeline_add_layer(&mut sort_app);
    let lid_bottom = sort_app.timeline.layers[0].id;
    let lid_top = sort_app.timeline.layers[1].id;
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
    sort_app.canvas.shapes = vec![shape_top, shape_bottom];
    sort_app.canvas.selected_shapes = vec![0];
    sort_app
        .doc
        .canvas
        .sort_shapes_by_layer_order(&sort_app.doc.timeline);
    assert_eq!(sort_app.canvas.shapes[0].layer_id, lid_bottom);
    assert_eq!(sort_app.canvas.shapes[1].layer_id, lid_top);
    assert_eq!(
        sort_app.canvas.selected_shapes,
        vec![1],
        "sort_shapes_by_layer_order 後も shape_id に基づいて選択インデックスが新しい位置へ追従すること"
    );
    assert_eq!(sort_app.canvas.selected_shape_ids(), vec![top_shape_id]);
    sort_app.canvas.record_loaded_signatures();
    assert!(sort_app.canvas.is_all_shapes_unchanged());

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
    sort_app.canvas.shapes.push(empty_marker);
    assert!(
        tool_select::find_snap_vertex(
            &sort_app.canvas,
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
    bucket_app.ui.brush_color = Color32::BLACK;
    tool_paint::commit_raw_outline(&mut bucket_app, &raw_ring);

    // Layer 2 を追加して円の中央 (x = 200) に縦の分割線を引き、Layer 2 をロック (🔒) する
    timeline_ui::timeline_add_layer(&mut bucket_app);
    let raw_div =
        generate_brush_outline(&[Point::new(200.0, 120.0), Point::new(200.0, 280.0)], 6.0);
    tool_paint::commit_raw_outline(&mut bucket_app, &raw_div);
    timeline_ui::set_layer_locked(&mut bucket_app, 1, true);
    timeline_ui::switch_to_layer_and_frame(&mut bucket_app, 0, 0);

    bucket_app.ui.brush_color = red;
    assert!(tool_paint::bucket_fill_at(
        &mut bucket_app,
        Point::new(170.0, 200.0)
    ));
    let red_fill = bucket_app
        .canvas
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
    let mut many_walls = bucket_app.canvas.shapes.clone();
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
    let micro_union = crate::vector_boolean::boolean_operation(
        &micro_a,
        &micro_b,
        crate::vector_boolean::BoolOp::Union,
        red,
    );
    assert!(
        !micro_union.is_empty(),
        "1600%拡大時の極小シェイプ同士の Union が min_area フィルタで消失しないこと"
    );
    let micro_diff = crate::vector_boolean::boolean_operation(
        &micro_a,
        &micro_b,
        crate::vector_boolean::BoolOp::Difference,
        red,
    );
    assert!(
        !micro_diff.is_empty(),
        "1600%拡大時の極小シェイプ同士の Difference が消失せず正確に切り抜かれること"
    );
}

#[test]
fn test_tween_lru_cache_no_thrashing() {
    // 8フレーム超（12フレーム）のクラシックトゥイーン再生を2周ループした際、64エントリLRUキャッシュにより2周目もスラッシングせず同一 Arc<Mesh> が返ること
    let mut lru_tween_app = Is2dApp::default();
    lru_tween_app.canvas.shapes = make_stroke(
        Point::new(80.0, 200.0),
        Point::new(110.0, 200.0),
        10.0,
        Color32::from_rgb(220, 80, 40),
    );
    lru_tween_app.canvas.selected_shapes = vec![0];
    tool_transform::convert_selection_to_symbol(&mut lru_tween_app).unwrap();
    // シンボルインスタンスを移動確定してもキャッシュ済み fill_mesh が破棄されず、平行移動分だけ頂点がシフトすること
    let mesh_before = lru_tween_app.canvas.shapes[0].fill_mesh();
    assert!(!mesh_before.is_empty());
    let x0_before = mesh_before.vertices[0].pos.x;
    assert!(tool_select::nudge_selected_shapes(
        &mut lru_tween_app,
        kurbo::Vec2::new(10.0, 0.0)
    ));
    assert!(
        lru_tween_app.canvas.shapes[0].has_cached_fill_mesh(),
        "シンボルインスタンスの移動確定（commit_moved_selection）時にキャッシュ済み fill_mesh が無駄に破棄されないこと"
    );
    let mesh_after = lru_tween_app.canvas.shapes[0].fill_mesh();
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
                .timeline
                .resolved_cel_screen_mesh(
                    0,
                    f,
                    1.0,
                    Vec2::ZERO,
                    false,
                    &lru_tween_app.symbols.library,
                )
                .unwrap()
        })
        .collect();
    for (f, first_m) in first_loop_meshes.iter().enumerate() {
        let second_m = lru_tween_app
            .timeline
            .resolved_cel_screen_mesh(0, f, 1.0, Vec2::ZERO, false, &lru_tween_app.symbols.library)
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
    app.viewport.stage_size = Vec2::new(1280.0, 720.0);
    app.timeline.fps = 30;

    // Layer 1: シンボル変換 + クラシックトゥイーン (Frame 0 -> Frame 5)
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 120.0),
        Point::new(160.0, 120.0),
        12.0,
        Color32::from_rgb(240, 80, 60),
    );
    app.canvas.selected_shapes = vec![0];
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
    let active_lid = app.timeline.layers[app.timeline.active_layer].id;
    for s in &mut raw_and_group {
        s.layer_id = active_lid;
    }
    app.canvas.shapes.extend(raw_and_group);
    let last_idx = app.canvas.shapes.len() - 1;
    app.canvas.selected_shapes = vec![last_idx];
    let gid = tool_select::group_selected_shapes(&mut app).unwrap();

    // シンボルインプレイス編集に入った状態で保存してもルートタイムラインとライブラリが正しく保存されること
    assert!(tool_transform::enter_symbol_from_library(&mut app, sym_id));
    app.canvas.shapes.extend(make_stroke(
        Point::new(130.0, 140.0),
        Point::new(150.0, 140.0),
        6.0,
        Color32::from_rgb(50, 200, 100),
    ));

    let tmp_path =
        std::env::temp_dir().join(format!("is2d_roundtrip_test_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).expect("保存に成功すること");
    assert_eq!(
        app.ui.current_file_path.as_deref(),
        Some(tmp_path.as_path())
    );

    let mut loaded_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None)
        .expect("読み込みに成功すること");

    assert_eq!(loaded_app.viewport.stage_size, Vec2::new(1280.0, 720.0));
    assert_eq!(loaded_app.timeline.fps, 30);
    assert_eq!(loaded_app.timeline.total_frames, 6);
    assert_eq!(loaded_app.timeline.layers.len(), 2);
    assert_eq!(loaded_app.symbols.library.len(), 1);
    assert_eq!(
        loaded_app
            .symbols
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
    assert!(loaded_app.timeline.layers[0].keyframes[0].tween);
    assert_eq!(
        loaded_app.timeline.layers[0].keyframes[0].tween_rotate,
        crate::symbol::TweenRotate::Clockwise
    );
    assert!(
        loaded_app.timeline.layers[1].keyframes[0]
            .cel
            .shapes
            .iter()
            .any(|s| s.group_path == vec![gid]),
        "グループパスも完全一致で復元されること"
    );
    assert!(loaded_app.symbols.next_group_id > gid);
    assert!(loaded_app.symbols.next_symbol_id > sym_id);

    let _ = std::fs::remove_file(&tmp_path);
}

#[test]
fn test_multitab_open_switch_symbol_copy_paste_and_close() {
    let mut app = Is2dApp::default();
    app.viewport.stage_size = Vec2::new(1280.0, 720.0);
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 120.0),
        Point::new(160.0, 120.0),
        12.0,
        Color32::from_rgb(240, 80, 60),
    );
    app.canvas.selected_shapes = vec![0];
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();

    let tmp_path =
        std::env::temp_dir().join(format!("is2d_multitab_test1_{}.is2d", std::process::id()));
    let tmp_path2 =
        std::env::temp_dir().join(format!("is2d_multitab_test2_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).unwrap();

    let mut app2 = Is2dApp::default();
    app2.viewport.stage_size = Vec2::new(640.0, 480.0);
    crate::project_io::save_project_to_path(&mut app2, &tmp_path2)
        .expect("2つ目の保存に成功すること");

    let mut loaded_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None).unwrap();

    // Tab 0 (loaded_app) でシンボルインスタンスを選択してコピー（同時に保存時の選択状態維持も検証）
    timeline_ui::switch_to_layer_and_frame(&mut loaded_app, 0, 0);
    loaded_app.canvas.selected_shapes = (0..loaded_app.canvas.shapes.len())
        .filter(|&i| loaded_app.canvas.shapes[i].symbol_instance.is_some())
        .collect();
    loaded_app.canvas.transform_pivot = Some(Point::new(111.0, 222.0));
    assert!(!loaded_app.canvas.selected_shapes.is_empty());
    assert!(tool_select::copy_selected_shapes(&mut loaded_app));

    // Cmd+S（save_project_to_path）を実行しても現在の選択状態とカスタムピボットが強制解除されないこと
    crate::project_io::save_project_to_path(&mut loaded_app, &tmp_path)
        .expect("選択維持のまま上書き保存に成功すること");
    assert!(!loaded_app.canvas.selected_shapes.is_empty());
    assert_eq!(
        loaded_app.canvas.transform_pivot,
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
    assert_eq!(loaded_app.tabs.tabs.len(), 2);
    assert_eq!(loaded_app.tabs.active_index, 1);
    assert_eq!(loaded_app.viewport.stage_size, Vec2::new(640.0, 480.0));
    assert!(loaded_app.symbols.library.is_empty());
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
        loaded_app.symbols.library.len(),
        1,
        "別タブからペーストしたシンボル定義がターゲットタブのライブラリに自動移植されること"
    );
    // Undo すれば移植されたシンボルごと元の未変更状態（!is_dirty）に戻ること
    loaded_app.undo();
    assert!(loaded_app.symbols.library.is_empty());
    assert!(!loaded_app.history.is_dirty());

    // Undo 後に再度 Tab 1 でペーストした際、消去されたシンボル定義が再移植され図形が消失しないこと（指摘1-A）
    assert!(tool_select::paste_clipboard(&mut loaded_app, true));
    assert_eq!(loaded_app.symbols.library.len(), 1);
    assert!(
        loaded_app
            .canvas
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
    assert_eq!(loaded_app.tabs.tabs.len(), 2);
    assert_eq!(loaded_app.tabs.active_index, 0);
    assert_eq!(loaded_app.viewport.stage_size, Vec2::new(1280.0, 720.0));

    // 元タブ（Tab 0）に戻って再度ペーストした際、自分のシンボルが重複インポートされないこと（指摘1-C）
    assert!(tool_select::paste_clipboard(&mut loaded_app, true));
    assert_eq!(
        loaded_app.symbols.library.len(),
        1,
        "コピー元タブに戻ってペーストしても同名・同内容のシンボルが重複増殖しないこと"
    );

    // Tab 0 を閉じると Tab 1 がアクティブになり、さらに最後のタブを閉じると新規の無題タブ（無題-1）にリセットされること
    crate::project_io::close_active_tab(&mut loaded_app, None);
    assert_eq!(loaded_app.tabs.tabs.len(), 1);
    assert_eq!(loaded_app.viewport.stage_size, Vec2::new(640.0, 480.0));
    crate::project_io::close_active_tab(&mut loaded_app, None);
    assert_eq!(loaded_app.tabs.tabs.len(), 1);
    assert!(loaded_app.ui.current_file_path.is_none());
    assert_eq!(loaded_app.tabs.tabs[0].untitled_number, 1);

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
    loaded_app.push_undo();
    loaded_app.canvas.shapes = make_stroke(
        Point::new(50.0, 50.0),
        Point::new(90.0, 50.0),
        8.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut loaded_app);
    let next_untitled_before = loaded_app.tabs.next_untitled_number;
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path2, None)
        .expect("ファイル読込に成功すること");
    assert_eq!(
        loaded_app.tabs.next_untitled_number, next_untitled_before,
        "既存ファイルを別タブで開いた際に無題タブ連番（next_untitled_number）が欠番スキップしないこと"
    );

    // 起動直後の白紙「無題-1」タブでファイルを開いた場合、次の新規タブが「無題-1」から採番され、show_wireframe がタブ独立保持されること
    let mut fresh_open_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut fresh_open_app, &tmp_path2, None)
        .expect("初期白紙タブへのファイル読込に成功すること");
    assert_eq!(fresh_open_app.tabs.tabs[0].untitled_number, 0);
    assert_eq!(
        fresh_open_app.tabs.alloc_untitled_number(),
        1,
        "初期の白紙無題タブがファイル読込で置き換わった場合、次の新規無題タブは 無題-1 になること"
    );
    fresh_open_app.ui.show_wireframe = true;
    crate::project_io::open_project_in_tab(&mut fresh_open_app, &tmp_path, None)
        .expect("2タブ目の読込に成功すること");
    assert!(
        !fresh_open_app.ui.show_wireframe,
        "新規に開いた別タブでは show_wireframe が初期値 false であること"
    );
    crate::project_io::switch_to_tab(&mut fresh_open_app, 0, None);
    assert!(
        fresh_open_app.ui.show_wireframe,
        "元のタブへ戻ると show_wireframe = true が復元されること"
    );

    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_file(&tmp_path2);
}

#[test]
fn test_clipboard_symbol_evaluates_latest_symbol_def_and_shares_arc() {
    // コピー後にシンボル定義を編集して再ペーストした際、古い展開図形ではなく最新の SymbolDef が評価されること
    let mut sym_paste_app = Is2dApp::default();
    sym_paste_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(140.0, 100.0),
        10.0,
        Color32::RED,
    );
    sym_paste_app.canvas.selected_shapes = vec![0];
    let child_sym = tool_transform::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.canvas.selected_shapes = vec![0];
    let parent_sym = tool_transform::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.canvas.selected_shapes = vec![0];
    assert!(tool_select::copy_selected_shapes(&mut sym_paste_app));
    let first_clip_sym_arc = std::sync::Arc::clone(&sym_paste_app.canvas.clipboard_symbols[0]);
    assert!(tool_select::copy_selected_shapes(&mut sym_paste_app));
    assert!(
        std::sync::Arc::ptr_eq(&first_clip_sym_arc, &sym_paste_app.canvas.clipboard_symbols[0]),
        "同一シンボルリビジョンでの再コピー時は clipboard_symbols の Arc<SymbolDef> が O(1) ポインタ共有されること"
    );

    // 子シンボルをライブラリから削除（親シンボル内では通常グループへデタッチされる）
    assert!(tool_transform::delete_symbol_from_library(
        &mut sym_paste_app,
        child_sym
    ));
    assert_eq!(sym_paste_app.symbols.library.len(), 1);

    // 親シンボルをインプレイス編集して青色の2シェイプに変更
    assert!(tool_transform::enter_symbol_from_library(
        &mut sym_paste_app,
        parent_sym
    ));
    sym_paste_app.timeline.fps = 12;
    for sess in &mut sym_paste_app.symbols.edit_stack {
        sess.parent_timeline.fps = 12;
    }
    sym_paste_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(140.0, 100.0),
        10.0,
        Color32::BLUE,
    );
    sym_paste_app.canvas.shapes.extend(make_stroke(
        Point::new(100.0, 130.0),
        Point::new(140.0, 130.0),
        10.0,
        Color32::BLUE,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut sym_paste_app);
    assert!(tool_transform::exit_symbol_timeline(&mut sym_paste_app));
    assert_eq!(
        sym_paste_app.timeline.fps, 12,
        "シンボルインプレイス編集中のFPS変更が親タイムライン復帰後も維持されること"
    );
    assert_eq!(sym_paste_app.canvas.shapes.len(), 2);

    // 再度 Cmd+V ペースト：古い赤1シェイプではなく最新の青2シェイプが展開され、削除済み child_sym も孤児復活しないこと
    assert!(tool_select::paste_clipboard(&mut sym_paste_app, false));
    assert_eq!(
        sym_paste_app.symbols.library.len(),
        1,
        "デタッチ済みの子シンボルが孤児として再インポートされないこと"
    );
    assert_eq!(
        sym_paste_app.canvas.shapes.len(),
        4,
        "コピー後に編集された最新シンボル定義（2シェイプ×2インスタンス＝4シェイプ）が展開されること"
    );
    assert!(sym_paste_app
        .canvas
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
    cross_gid_app.canvas.shapes = make_stroke(
        Point::new(80.0, 80.0),
        Point::new(120.0, 80.0),
        10.0,
        Color32::RED,
    );
    cross_gid_app.canvas.selected_shapes = vec![0];
    let tab0_inner_gid = tool_select::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab0_inner_gid, 1);
    cross_gid_app.canvas.selected_shapes = vec![0];
    let _tab0_sym_id = tool_transform::convert_selection_to_symbol(&mut cross_gid_app).unwrap();
    cross_gid_app.canvas.selected_shapes = vec![0];
    assert!(tool_select::copy_selected_shapes(&mut cross_gid_app));

    // 2つ目のタブを開き、Tab 1 側でもグループ（gid=1）を作成してから Tab 0 のシンボルをペースト＆グループ解除（デタッチ）
    crate::project_io::open_project_in_tab(&mut cross_gid_app, &tmp_cross_tab, None).unwrap();
    assert!(
        cross_gid_app.tabs.tabs[0].canvas.clipboard.is_empty(),
        "タブ切り替え時に非アクティブスロットの clipboard が空（std::mem::take）になっていること"
    );
    cross_gid_app.canvas.shapes = make_stroke(
        Point::new(260.0, 200.0),
        Point::new(300.0, 200.0),
        10.0,
        Color32::BLUE,
    );
    cross_gid_app.canvas.selected_shapes = vec![0];
    let tab1_existing_gid = tool_select::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab1_existing_gid, 1);

    assert!(tool_select::paste_clipboard(&mut cross_gid_app, true));
    assert!(tool_select::ungroup_selected_shapes(&mut cross_gid_app));
    let blue_gid = cross_gid_app
        .canvas
        .shapes
        .iter()
        .find(|s| s.color == Color32::BLUE)
        .unwrap()
        .group_path[0];
    let red_gid = cross_gid_app
        .canvas
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
    merge_app.canvas.shapes = blue_stroke;
    merge_app.canvas.shapes.extend(red_stroke);
    canvas_tool::sync_shapes_to_timeline(&mut merge_app);
    merge_app.canvas.selected_shapes = vec![1];
    let snap = merge_app.canvas.snapshot_for_drag();
    merge_app.canvas.translate_selected(KVec2::new(0.0, -80.0));
    tool_select::commit_moved_selection(&mut merge_app, snap);
    assert!(merge_app.canvas.selection_modified);

    crate::project_io::save_project_to_path(&mut merge_app, &tmp_merge_path)
        .expect("選択中の保存に成功すること");
    assert!(!merge_app.canvas.selection_modified);
    assert!(!merge_app.history.is_dirty());
    let shapes_len_on_save = merge_app.canvas.shapes.len();
    tool_select::clear_selection(&mut merge_app);
    assert_eq!(merge_app.canvas.shapes.len(), shapes_len_on_save);
    assert!(
        !merge_app.history.is_dirty(),
        "保存後の選択解除でタイムラインが不整合更新されないこと"
    );

    // (2) 非選択の背景同色シェイプと Union 融合して保存された場合は背景を勝手に巻き込まず選択解除され、
    //     選択中のシェイプ同士のみの自己重なり Union 保存では shape_id と選択状態が維持されること
    let mut union_save_app = Is2dApp::default();
    union_save_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::RED,
    );
    union_save_app.canvas.shapes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut union_save_app);
    union_save_app.canvas.selected_shapes = vec![1];
    let snap_u = union_save_app.canvas.snapshot_for_drag();
    union_save_app
        .canvas
        .translate_selected(KVec2::new(0.0, -80.0));
    tool_select::commit_moved_selection(&mut union_save_app, snap_u);
    crate::project_io::save_project_to_path(&mut union_save_app, &tmp_merge_path)
        .expect("同色Union保存に成功すること");
    assert_eq!(
        union_save_app.canvas.shapes.len(),
        1,
        "同色シェイプが1つにUnion結合されていること"
    );
    assert!(
        union_save_app.canvas.selected_shapes.is_empty(),
        "非選択の背景同色シェイプとUnion融合して保存した場合は背景シェイプが勝手に選択に巻き込まれないこと"
    );

    let mut self_union_app = Is2dApp::default();
    self_union_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        16.0,
        Color32::RED,
    );
    self_union_app.canvas.shapes.extend(make_stroke(
        Point::new(150.0, 80.0),
        Point::new(150.0, 120.0),
        16.0,
        Color32::RED,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut self_union_app);
    self_union_app.canvas.selected_shapes = vec![0, 1];
    assert!(tool_select::copy_selected_shapes(&mut self_union_app));
    let snap_su = self_union_app.canvas.snapshot_for_drag();
    self_union_app
        .canvas
        .translate_selected(KVec2::new(10.0, 0.0));
    tool_select::commit_moved_selection(&mut self_union_app, snap_su);
    crate::project_io::save_project_to_path(&mut self_union_app, &tmp_merge_path)
        .expect("選択同士のUnion保存に成功すること");
    assert_eq!(self_union_app.canvas.shapes.len(), 1);
    assert_eq!(
        self_union_app.canvas.selected_shapes,
        vec![0],
        "選択シェイプ同士のみのUnion結合では shape_id が引き継がれ選択状態が維持されること"
    );

    let _ = std::fs::remove_file(&tmp_merge_path);
}

#[test]
fn test_save_io_failure_rollback_and_marquee_unsliced_push_undo() {
    // 全レイヤーロック中は paste_clipboard が拒否され、矩形マーキースライス直後（未編集）に push_undo() が呼ばれても分割断片が早期確定されないこと
    let mut self_union_app = Is2dApp::default();
    self_union_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        16.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut self_union_app);
    self_union_app.canvas.selected_shapes = vec![0];
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
    assert!(self_union_app.canvas.has_unsliced_snapshot());
    assert!(self_union_app.canvas.shapes.len() >= 2);
    self_union_app.push_undo();
    assert_eq!(
        self_union_app.timeline.layers[0].keyframes[0]
            .cel
            .shapes
            .len(),
        1,
        "マーキースライス未編集状態で push_undo が呼ばれてもタイムライン側の分割前1シェイプが維持されること"
    );
    tool_select::clear_selection(&mut self_union_app);
    assert_eq!(self_union_app.canvas.shapes.len(), 1);

    // 存在しないディレクトリへの保存（I/O失敗）時のロールバックと、selection_modified からの Undo -> Redo マージ確定
    let mut undo_redo_merge_app = Is2dApp::default();
    undo_redo_merge_app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::RED,
    );
    undo_redo_merge_app.canvas.shapes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    canvas_tool::sync_shapes_to_timeline(&mut undo_redo_merge_app);
    undo_redo_merge_app.canvas.selected_shapes = vec![1];
    let snap_ur = undo_redo_merge_app.canvas.snapshot_for_drag();
    undo_redo_merge_app
        .canvas
        .translate_selected(KVec2::new(0.0, -80.0));
    tool_select::commit_moved_selection(&mut undo_redo_merge_app, snap_ur);
    assert!(undo_redo_merge_app.canvas.selection_modified);

    let bad_path = std::env::temp_dir()
        .join("is2d_nonexistent_dir_for_test_999999")
        .join("fail.is2d");
    assert!(crate::project_io::save_project_to_path(&mut undo_redo_merge_app, &bad_path).is_err());
    assert_eq!(
        undo_redo_merge_app.canvas.shapes.len(),
        2,
        "保存I/O失敗時はライブキャンバス上の未マージ図形が勝手に結合されないこと"
    );
    assert!(undo_redo_merge_app.canvas.selection_modified);

    undo_redo_merge_app.undo();
    assert_eq!(undo_redo_merge_app.canvas.shapes.len(), 2);
    undo_redo_merge_app.redo();
    assert_eq!(
        undo_redo_merge_app.canvas.shapes.len(),
        1,
        "selection_modified 状態からの Undo -> Redo 後もマージ描画（Union）が確定維持されること"
    );
}

#[test]
fn test_corrupt_and_truncated_json_returns_error_without_modifying_app() {
    let tmp_corrupt =
        std::env::temp_dir().join(format!("is2d_corrupt_test_{}.is2d", std::process::id()));
    let mut app = Is2dApp::default();
    app.viewport.stage_size = Vec2::new(960.0, 540.0);

    // 1. 途中で切れた不正な JSON 構文
    std::fs::write(&tmp_corrupt, r#"{"version": 1, "stage_size": [800.0,"#).unwrap();
    let res1 = crate::project_io::open_project_in_tab(&mut app, &tmp_corrupt, None);
    assert!(
        res1.is_err(),
        "途中で切断された壊れた JSON はパニックせず Err を返すこと"
    );
    assert_eq!(
        app.viewport.stage_size,
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
    assert_eq!(sanitized_app.viewport.stage_size, Vec2::new(3840.0, 100.0));
    assert_eq!(sanitized_app.timeline.fps, 60);
    assert_eq!(
        sanitized_app.timeline.active_layer, 1,
        "範囲外の active_layer=99 が最終レイヤーインデックス 1 にクランプされていること"
    );
    assert_eq!(
        sanitized_app.timeline.current_frame, 1,
        "範囲外の current_frame=99 が最終フレームインデックス 1 にクランプされていること"
    );
    assert_ne!(
        sanitized_app.timeline.layers[0].id, sanitized_app.timeline.layers[1].id,
        "重複していた layer_id が一意に再採番されていること"
    );
    assert_eq!(
        sanitized_app.timeline.layers[0].keyframes[0].cel.shapes[0].layer_id,
        sanitized_app.timeline.layers[0].id,
        "不正だった shape.layer_id=999 が所属レイヤーの layer_id に正規化されていること"
    );
    assert!(
        !sanitized_app.timeline.layers[0].keyframes[0].tween,
        "生シェイプとシンボルが混在し単一シンボル条件を満たさないキーフレームの tween=true が false に正規化されていること"
    );
    assert_eq!(
        sanitized_app.timeline.layers[1].keyframes.len(),
        1,
        "空の keyframes 配列だったレイヤーに start_frame=0 の空白キーフレームが自動補填されていること"
    );
    assert_eq!(
        sanitized_app.timeline.layers[0].keyframes[0]
            .cel
            .instances
            .len(),
        1,
        "存在しない symbol_id=9999 の孤児インスタンスが除去されていること"
    );
    assert!(
        sanitized_app.timeline.layers[0].keyframes[0].cel.shapes[0]
            .exterior
            .signed_area()
            > 0.0,
        "逆向き（負面積）だった外周輪郭のワインディングが正面積に正規化されていること"
    );
    assert!(
        !sanitized_app.symbols.would_create_symbol_cycle(1, 2)
            || !sanitized_app.symbols.would_create_symbol_cycle(2, 1),
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
    app.ui.tool = Tool::Brush;
    app.ui.brush_color = red;
    let raw_red =
        generate_brush_outline(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 14.0);
    tool_paint::commit_raw_outline(&mut app, &raw_red);
    assert_eq!(app.timeline.layers[0].keyframes[0].cel.shapes.len(), 1);

    // Layer 2 (index 1, id 2) を追加して同じ座標に青い横線を描画（両レイヤーとも非ロック）
    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.timeline.active_layer, 1);
    app.ui.brush_color = blue;
    let raw_blue =
        generate_brush_outline(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 14.0);
    tool_paint::commit_raw_outline(&mut app, &raw_blue);
    assert_eq!(app.timeline.layers[1].keyframes[0].cel.shapes.len(), 1);

    // Layer 2 がアクティブな状態で消しゴムツールを実行 → Layer 2 の青い線のみが削られ、Layer 1 の赤い線は無傷であること
    app.ui.tool = Tool::Eraser;
    let raw_erase =
        generate_brush_outline(&[Point::new(200.0, 150.0), Point::new(200.0, 250.0)], 16.0);
    tool_paint::commit_raw_outline(&mut app, &raw_erase);

    let layer1_shapes = &app.timeline.layers[0].keyframes[0].cel.shapes;
    let layer2_shapes = &app.timeline.layers[1].keyframes[0].cel.shapes;
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
    app.ui.brush_color = red;
    let raw = generate_brush_outline(&[Point::new(100.0, 100.0), Point::new(160.0, 100.0)], 10.0);
    tool_paint::commit_raw_outline(&mut app, &raw);
    timeline_ui::timeline_add_layer(&mut app);
    assert!(app.timeline.layers[1].keyframes[0].cel.is_empty());
    assert!(!app.timeline.layers[0].keyframes[0].cel.is_empty());

    // 両レイヤーの Frame 0 を範囲選択して F7 (空白キーフレーム挿入) を実行
    // → Layer 1 に絵があるため Frame 0 の絵を消去せず Frame 1 に進んで空白キーフレームを挿入すること
    timeline_ui::select_timeline_cells(&mut app, 0, 0, 1, 0);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.timeline.current_frame, 1);
    assert!(
        !app.timeline.layers[0].keyframes[0].cel.is_empty(),
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
    move_app.canvas.shapes = make_stroke(
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
    move_app.canvas.shapes = make_stroke(
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
    let kfs_in_dst: Vec<_> = move_app.timeline.layers[0]
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
        move_app.timeline.layers[0].cel_at(4).unwrap().shapes[0].color,
        red
    );

    // 別レイヤー（layer_offset = +1）へのフレーム区間移動で移動先レイヤーの layer_id に更新されること
    timeline_ui::timeline_add_layer(&mut move_app);
    let dst_layer_id = move_app.timeline.layers[1].id;
    let sel_cross = crate::timeline::TimelineCellSelection::from_corners(0, 3, 0, 5);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut move_app,
        sel_cross,
        1,
        -2,
        false
    ));
    assert!(
        move_app.timeline.layers[0].cel_at(3).unwrap().is_empty(),
        "移動元レイヤー (Layer 1) の Frame 3..=5 は空白化されていること"
    );
    let moved_cel = move_app.timeline.layers[1].cel_at(1).unwrap();
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
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        14.0,
        Color32::RED,
    );
    app.canvas.selected_shapes = vec![0];
    let gid = tool_select::group_selected_shapes(&mut app).unwrap();

    // グループをダブルクリックしてインプレイス編集に入る
    assert!(tool_select::enter_group_at(
        &mut app,
        Point::new(150.0, 100.0)
    ));
    assert_eq!(app.canvas.editing_group_path, vec![gid]);

    // グループ内部の生シェイプ上の座標 (150, 100) をダブルクリックしても exit_current_group が呼ばれずグループ編集スコープに留まること
    let pt_on_raw = Point::new(150.0, 100.0);
    tool_select::handle_canvas_double_click(&mut app, pt_on_raw);
    assert_eq!(
        app.canvas.editing_group_path,
        vec![gid],
        "グループ内の生シェイプ上のダブルクリックでは親階層へ退出しないこと"
    );
    // 空白背景 (400, 400) をダブルクリックすると exit_current_group で親階層へ戻れること
    let pt_empty = Point::new(400.0, 400.0);
    tool_select::handle_canvas_double_click(&mut app, pt_empty);
    assert!(app.canvas.editing_group_path.is_empty());
}

#[test]
fn test_tween_state_incomplete_when_not_single_symbol_instance() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        14.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.timeline.layers[0].keyframes[0].tween = true;
    assert_eq!(
        app.timeline.layers[0].tween_state_at_keyframe(0),
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
    // 即座に unsliced_canvas が復元され、直後のバケツ塗りで古い unsliced_canvas が復活しないこと
    app.canvas.shapes = make_stroke(
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
    assert!(app.canvas.has_unsliced_snapshot());
    assert_eq!(app.canvas.selected_shapes.len(), 1);
    let sel_idx = app.canvas.selected_shapes[0];
    tool_select::toggle_shape_selection(&mut app, sel_idx);
    assert!(app.canvas.selected_shapes.is_empty());
    assert!(
        !app.canvas.has_unsliced_snapshot(),
        "Shift+クリックで選択が空になった時点で未編集のマーキースライスは即座に復元されること"
    );
    assert_eq!(app.canvas.shapes.len(), 1);

    // 直後にバケツツールで赤シェイプを青に塗り替えても正常に1シェイプが青になること
    app.ui.brush_color = blue;
    assert!(tool_paint::bucket_fill_at(
        &mut app,
        Point::new(170.0, 100.0)
    ));
    assert_eq!(app.canvas.shapes.len(), 1);
    assert_eq!(app.canvas.shapes[0].color, blue);
}

#[test]
fn test_symbol_rename_consecutive_keystrokes_grouped_into_single_undo() {
    let mut app = Is2dApp::default();
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(240.0, 100.0),
        16.0,
        Color32::BLUE,
    );
    app.canvas.selected_shapes = vec![0];
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    let undo_before_rename = app.history.undo_stack.len();

    // 1文字目の入力開始時のみ Undo を積み、フレーム描画（ensure_valid_selection）を挟んで2文字目を入力しても Undo が増殖しないこと
    assert!(tool_transform::rename_symbol_in_library(&mut app, sym_id, "H"));
    app.symbols.ensure_valid_selection();
    assert_eq!(
        app.symbols.actively_renaming_symbol,
        Some(sym_id),
        "毎フレームの ensure_valid_selection 呼び出しで actively_renaming_symbol が消去されないこと"
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
    app.timeline.resize_total_frames(1200);
    assert_eq!(app.timeline.total_frames, 1200);
    assert_eq!(app.timeline.layers[0].frame_count, 1200);

    // loop_playback == true のとき、先頭フレーム (0) から -1 コマ送りで最終フレーム (1199) へ折り返すこと
    app.timeline_ui.loop_playback = true;
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    timeline_ui::step_frame(&mut app, -1);
    assert_eq!(app.timeline.current_frame, 1199);
    timeline_ui::step_frame(&mut app, 1);
    assert_eq!(app.timeline.current_frame, 0);

    // loop_playback == false のとき、先頭フレーム (0) から -1 でも 0 で停止し、最終フレーム (1199) から +1 でも 1199 で停止すること
    app.timeline_ui.loop_playback = false;
    timeline_ui::step_frame(&mut app, -1);
    assert_eq!(app.timeline.current_frame, 0);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1199);
    timeline_ui::step_frame(&mut app, 1);
    assert_eq!(app.timeline.current_frame, 1199);
}

#[test]
fn test_viewport_fit_stage_to_window() {
    let mut app = Is2dApp::default();
    app.viewport.last_canvas_rect =
        eframe::egui::Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1600.0, 900.0));
    app.viewport.stage_size = Vec2::new(800.0, 450.0);
    app.viewport.fit_stage_to_window();
    assert!(
        app.viewport.zoom > 1.5 && app.viewport.zoom <= 2.0,
        "1600x900 キャンバスに対して 800x450 ステージがマージン付きでフィット拡大されること: zoom={}",
        app.viewport.zoom
    );
    let center = app.viewport.last_canvas_rect.center();
    let world_before = app.viewport.screen_to_world(center);
    app.viewport.zoom_around_screen_pos(center, 1.5);
    let world_after = app.viewport.screen_to_world(center);
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
    app.canvas.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.timeline.current_frame, 1);
    app.canvas.shapes = make_stroke(Point::new(200.0, 200.0), Point::new(260.0, 200.0), 10.0, blue);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // Frame 0（赤い絵）に戻って F7 を押したとき、Frame 1 の青い絵を白紙消去せず Frame 2 へ押し出して Frame 1 に空白KFを挿入すること (H4)
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.timeline.current_frame, 1);
    assert!(
        app.timeline.layers[0].cel_at(1).unwrap().is_empty(),
        "F7 で自動進行した Frame 1 に空白キーフレームが挿入されること"
    );
    assert_eq!(
        app.timeline.layers[0].cel_at(2).unwrap().shapes[0].color,
        blue,
        "Frame 1 にあった既存の青いキーフレームが消去されず Frame 2 に保持されていること"
    );
}

#[test]
fn test_shift_f6_and_shift_f5_boundary_guards() {
    let red = Color32::from_rgb(220, 40, 40);
    // 1. Frame 0 の保持フレーム（Frame 1〜）で Shift+F6 を押しても先頭キーフレーム (Frame 0) を誤って白紙化しないこと
    let mut hold_app = Is2dApp::default();
    hold_app.canvas.shapes =
        make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut hold_app);
    timeline_ui::timeline_extend_frame(&mut hold_app);
    timeline_ui::timeline_extend_frame(&mut hold_app);
    timeline_ui::switch_to_layer_and_frame(&mut hold_app, 0, 2);
    assert!(
        !timeline_ui::timeline_delete_keyframe(&mut hold_app),
        "先頭キーフレームの保持フレーム（Frame 2）では Shift+F6 が無効であり、Frame 0 の絵を消去しないこと"
    );
    assert!(!hold_app.timeline.layers[0].cel_at(0).unwrap().is_empty());

    // 2. 尺の短いレイヤーが含まれる状態で後ろのフレームで Shift+F5 を押した際、範囲外の短いレイヤーの末尾を削らないこと
    timeline_ui::timeline_add_layer(&mut hold_app);
    hold_app.timeline.layers[1].frame_count = 1;
    hold_app.timeline.cell_selection = None;
    hold_app.timeline.current_frame = 2;
    assert!(timeline_ui::timeline_remove_frame(&mut hold_app));
    assert_eq!(hold_app.timeline.layers[0].frame_count, 2);
    assert_eq!(
        hold_app.timeline.layers[1].frame_count, 1,
        "現在フレーム (2) に届いていない短いレイヤー (frame_count=1) は Shift+F5 の影響を受けないこと"
    );
}

#[test]
fn test_loop_symbol_hold_span_f6_and_tween_split_preserves_phase_and_tween_rotate() {
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 100, 220);

    // 複数フレームを持つ Loop シンボルの保持スパン途中で F6 を押した際、表示コマが先頭(0)に巻き戻らないこと
    let mut sym_f6_app = Is2dApp::default();
    sym_f6_app.canvas.shapes =
        make_stroke(Point::new(100.0, 100.0), Point::new(150.0, 100.0), 10.0, red);
    sym_f6_app.canvas.selected_shapes = vec![0];
    let sym_id = tool_transform::convert_selection_to_symbol(&mut sym_f6_app).unwrap();
    {
        let sym_def = sym_f6_app.symbols.find_symbol_mut(sym_id).unwrap();
        sym_def.timeline.resize_total_frames(4);
        sym_def.timeline.layers[0].insert_keyframe(2, false);
        *sym_def.timeline.layers[0].cel_at_mut(2).unwrap().shapes_mut() = make_stroke(
            Point::new(-20.0, 0.0),
            Point::new(20.0, 0.0),
            10.0,
            blue,
        );
    }
    sym_f6_app.timeline.resize_total_frames(5);
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 2);
    assert_eq!(sym_f6_app.canvas.shapes[0].color, blue);
    assert!(timeline_ui::timeline_insert_keyframe(&mut sym_f6_app, true));
    assert_eq!(
        sym_f6_app.canvas.shapes[0].color,
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
    sym_f6_app.canvas.selected_shapes = vec![0];
    assert!(tool_select::nudge_selected_shapes(
        &mut sym_f6_app,
        kurbo::Vec2::new(20.0, 0.0)
    ));
    let kf_at_3 = sym_f6_app.timeline.layers[0]
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
    app.canvas.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 10.0, red);
    app.canvas.selected_shapes = vec![0];
    let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
    app.timeline.resize_total_frames(5);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    app.canvas.selected_shapes = vec![0];
    let snap = app.canvas.snapshot_for_drag();
    app.canvas.translate_selected(KVec2::new(200.0, 0.0));
    tool_select::commit_moved_selection(&mut app, snap);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert!(tool_transform::timeline_toggle_tween(&mut app));

    // 中間フレーム (Frame 2, x ≈ 220) に移動し、別レイヤーを追加した上でシンボルを削除して別レイヤーに描画
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 1, 2);
    assert!(tool_transform::delete_symbol_from_library(&mut app, sym_id));
    let raw_blue =
        generate_brush_outline(&[Point::new(50.0, 300.0), Point::new(90.0, 300.0)], 8.0);
    app.ui.brush_color = blue;
    tool_paint::commit_raw_outline(&mut app, &raw_blue);

    let kf0_bbox = app.timeline.layers[0].keyframes[0].cel.shapes[0].bounding_box();
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
    app.canvas.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);

    // 図形に当たらない空振り消しゴムで Undo が増えないこと (H7)
    let undo_before_noop_erase = app.history.undo_stack.len();
    app.ui.tool = Tool::Eraser;
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
    app.timeline.active_layer = 1;
    app.ui.tool = Tool::Eraser;
    assert!(
        !app.is_current_tool_allowed(),
        "アクティブレイヤーがロック中は他レイヤーが非ロックでも消しゴムが禁止判定になること"
    );
    app.ui.tool = Tool::Bucket;
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
    save_app.canvas.shapes =
        make_stroke(Point::new(50.0, 50.0), Point::new(100.0, 50.0), 8.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut save_app);
    crate::project_io::save_project_to_path(&mut save_app, &tmp_compact).unwrap();
    let saved_str = std::fs::read_to_string(&tmp_compact).unwrap();
    assert!(
        !saved_str.contains('\n'),
        "保存される .is2d ファイルは膨張を防ぐコンパクト JSON（改行なし）であること"
    );
    save_app.viewport.stage_size = Vec2::new(f32::NAN, 450.0);
    assert!(
        crate::project_io::save_project_to_path(&mut save_app, &tmp_compact).is_err(),
        "NaN が含まれる状態では保存前にエラーとして検出され壊れた JSON を書き出さないこと"
    );
    save_app.viewport.stage_size = Vec2::new(800.0, 450.0);
    let _ = std::fs::remove_file(&tmp_compact);

    for _ in 0..110 {
        save_app.push_undo();
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
    let expected_start_world = app.viewport.screen_to_world(press_pos);
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
    assert_eq!(app.canvas.shapes.len(), 1);
    assert!(
        app.canvas.shapes[0].contains_point(expected_start_world),
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
        app.canvas.shapes.len(),
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

    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(180.0, 100.0),
        12.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.canvas.selected_shapes = vec![0];

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
    assert_eq!(app.canvas.shapes.len(), 2);

    // OS クリップボードが空で egui-winit が押下時の Event::Paste / Key(V, pressed:true) を抑止した場合でも、
    // Cmd+V のキーリリース (pressed: false) フォールバックで確実にペーストされること (#15)
    app.canvas.paste_triggered_on_press = false;
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
        app.canvas.paste_count, 2,
        "OS クリップボードが空で押下イベントが吸収されても Cmd+V リリース時にペーストされること (#15)"
    );
    assert_eq!(app.canvas.shapes.len(), 2);

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
    assert_eq!(app.symbols.library.len(), 1);

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
        app.symbols.library.len(),
        1,
        "F8 のキーリピート (repeat: true) ではシンボル化が重複暴発しないこと"
    );
    assert_eq!(app.ui.tool, Tool::Transform);

    // 3. Cmd+0 でキャンバスの viewport.zoom が 1.0 (100%) にリセットされ、egui 全体の UI zoom_factor が変更されないこと (#1)
    app.viewport.zoom = 2.5;
    app.viewport.pan = Vec2::new(120.0, -80.0);
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
        (app.viewport.zoom - 1.0).abs() < 1e-6 && app.viewport.pan == Vec2::ZERO,
        "Cmd+0 でキャンバスの zoom が 1.0、pan が ZERO にリセットされること"
    );
    assert!(
        (ctx.zoom_factor() - 1.0).abs() < 1e-6,
        "zoom_with_keyboard = false により egui 全体の UI スケールが変更されないこと"
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
    assert!(app.canvas.selected_shapes.is_empty());
    assert_eq!(
        app.ui.tool,
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
    let center_world = app.viewport.stage_center_world();
    app.canvas.shapes = make_stroke(
        Point::new(center_world.x - 40.0, center_world.y),
        Point::new(center_world.x + 40.0, center_world.y),
        20.0,
        Color32::from_rgb(220, 60, 60),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // 1. 選択ツール (Tool::Select) でシェイプ中央を左クリックして選択し、右クリックドラッグでは移動しないことを検証
    app.ui.tool = Tool::Select;
    let center_screen = app.viewport.world_to_screen(center_world);
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
        app.canvas.selected_shapes,
        vec![0],
        "handle_select_tool_input 経由の左クリックでシェイプが選択されること"
    );

    // 2. 自由変形ツール (Tool::Transform) でシェイプ内部（ピボットから離れた位置）を左ドラッグして平行移動できること
    app.ui.tool = Tool::Transform;
    let initial_bbox_center = app.canvas.shapes[0].bounding_box().center();
    let grab_world = Point::new(initial_bbox_center.x - 20.0, initial_bbox_center.y);
    let grab_screen = app.viewport.world_to_screen(grab_world);
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
    let moved_center = app.canvas.shapes[0].bounding_box().center();
    let expected_dx = 50.0 / (app.viewport.zoom as f64);
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
    app.canvas.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::timeline_add_layer(&mut app);
    let lid2 = app.timeline.layers[1].id;
    app.canvas.shapes.extend(
        make_stroke(Point::new(100.0, 200.0), Point::new(160.0, 200.0), 10.0, blue)
            .into_iter()
            .map(|s| s.with_layer_id(lid2)),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    assert_eq!(app.canvas.shapes.len(), 2);

    // Layer 1 を非表示 (visible = false) に切り替えると、CanvasState::shapes から除外されること
    timeline_ui::set_layer_visible(&mut app, 0, false);
    assert_eq!(
        app.canvas.shapes.len(),
        1,
        "非表示レイヤー (visible=false) の図形は load_shapes_from_timeline から除外されること"
    );
    assert_eq!(app.canvas.shapes[0].color, blue);

    // その状態で Layer 2 を編集して sync_shapes_to_timeline を実行しても、非表示の Layer 1 の図形が消去されないこと
    app.canvas.selected_shapes = vec![0];
    assert!(tool_select::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(20.0, 0.0)
    ));
    assert_eq!(
        app.timeline.layers[0].keyframes[0].cel.shapes.len(),
        1,
        "非表示レイヤーが除外された状態で他レイヤーを編集しても非表示レイヤーのセル内容は保護されること"
    );

    // 再び Layer 1 を表示に戻すと赤い図形が復活すること
    timeline_ui::set_layer_visible(&mut app, 0, true);
    assert_eq!(app.canvas.shapes.len(), 2);
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
    app.timeline.fps = 10; // 1コマ = 0.1秒
    app.timeline.resize_total_frames(3);
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(150.0, 100.0),
        10.0,
        Color32::RED,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    app.canvas.shapes = make_stroke(
        Point::new(150.0, 100.0),
        Point::new(200.0, 100.0),
        10.0,
        Color32::GREEN,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    app.canvas.shapes = make_stroke(
        Point::new(200.0, 100.0),
        Point::new(250.0, 100.0),
        10.0,
        Color32::BLUE,
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // Frame 1 でオニオンスキンを有効化して update_ui を実行し、前後のオニオンスキンキャッシュが構築・描画されることを検証
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    app.timeline_ui.onion_skin_enabled = true;
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
    assert_eq!(app.timeline.current_frame, 0);

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
    assert_eq!(app.timeline.current_frame, 1);
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
    assert_eq!(app.timeline.current_frame, 2);
    assert!(
        !app.timeline_ui.is_playing,
        "loop_playback == false のとき最終フレーム到達で is_playing が false になり自動停止すること"
    );
}

#[test]
fn test_pull_edge_near_endpoint_and_off_center_does_not_jump_on_drag_start() {
    use kurbo::ParamCurve;

    let mut app = Is2dApp::default();
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        20.0,
        Color32::from_rgb(220, 60, 60),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);

    // 最も長いセグメントを選び、端点付近 (t = 0.05 < 0.18) かつ曲線から少し外れた座標でドラッグ開始 (#5)
    let seg_idx = app.canvas.shapes[0]
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
    let orig_seg = app.canvas.shapes[0].exterior.segments[seg_idx];
    let pt_near_end = orig_seg.eval(0.10) + kurbo::Vec2::new(0.0, 2.0);

    tool_select::start_reshape_drag(&mut app, pt_near_end, false, false);
    assert!(
        matches!(
            app.canvas.drag,
            canvas_tool::ActiveCanvasDrag::Select(canvas_tool::SelectDragState::PullEdge { .. })
        ),
        "エッジ端点付近のドラッグ開始で PullEdge 状態に入ること"
    );
    tool_select::update_reshape_drag(&mut app, pt_near_end);
    let after_zero_move = app.canvas.shapes[0].exterior.segments[seg_idx];
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
    let grab_after = app.canvas.shapes[0].exterior.segments[seg_idx].eval(grab_t);
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
    app.canvas.shapes = shapes;
    canvas_tool::sync_shapes_to_timeline(&mut app);

    let area_blue_top_before = app.canvas.shapes[0].exterior.signed_area();
    let area_blue_bottom_before = app.canvas.shapes[1].exterior.signed_area();

    // 赤い図形2つ (index 2, 3) を選択して左へ -100px 移動し、2つの青い図形の上にそれぞれ重ねる
    app.canvas.selected_shapes = vec![2, 3];
    assert!(tool_select::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(-100.0, 0.0)
    ));
    assert!(app.canvas.selection_modified);

    // Shift+クリック相当の toggle_shape_selection で、上側の赤い図形 (index 2) だけを選択解除し、下側の赤い図形 (index 3) は選択に残す (#7)
    tool_select::toggle_shape_selection(&mut app, 2);
    assert_eq!(app.canvas.selected_shapes.len(), 1);
    assert!(
        app.canvas.selection_modified,
        "一部の図形だけを選択解除した際、残りの選択図形は selection_modified=true のまま維持されること (#7)"
    );

    // 選択に残した下側の赤い図形をさらに右へ +100px 戻してから全選択解除する
    let remaining_idx = app.canvas.selected_shapes[0];
    assert_eq!(app.canvas.shapes[remaining_idx].color, red);
    assert!(tool_select::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(100.0, 0.0)
    ));
    tool_select::clear_selection(&mut app);

    // 結果：先に選択解除された上側の赤い図形は上側の青い図形を切り抜いているが、
    // 選択に残して後から退かした下側の赤い図形は下側の青い図形を切り抜いていないこと！
    let blue_shapes: Vec<&FillShape> = app
        .canvas
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
    app.canvas.shapes = make_stroke(
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
    assert!(app.canvas.has_unsliced_snapshot());
    assert_eq!(app.canvas.shapes.len(), 2);

    canvas_tool::set_tool(&mut app, Tool::Transform);
    let bbox = app.canvas.selection_bounding_box(app.viewport.zoom).unwrap();
    let current_pivot = app.canvas.active_transform_pivot(bbox);
    tool_transform::start_transform_drag(&mut app, current_pivot, false, false);
    tool_transform::update_transform_drag(&mut app, Point::new(130.0, 110.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(Point::new(130.0, 110.0)));

    assert_eq!(
        app.timeline.layers[0].keyframes[0].cel.shapes.len(),
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
    app.canvas.shapes.extend(sym_shapes);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.canvas.selected_shapes = vec![1];
    symbol_ops::convert_selection_to_symbol(&mut app).unwrap();

    // 生シェイプの一部とシンボルインスタンス全体をまたぐマーキー選択を行う
    tool_select::apply_marquee_selection(
        &mut app,
        Point::new(80.0, 80.0),
        Point::new(160.0, 260.0),
        false,
    );
    assert!(app.canvas.has_unsliced_snapshot());

    // シンボルインスタンスのみに選択を絞り、ピボットを変更して選択解除
    let sym_indices: Vec<usize> = app
        .canvas
        .shapes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.symbol_instance.is_some())
        .map(|(i, _)| i)
        .collect();
    app.canvas.selected_shapes = sym_indices;
    let new_world_pivot = Point::new(155.0, 235.0);
    assert!(tool_transform::set_selected_symbol_world_pivot(
        &mut app,
        Some(new_world_pivot),
        true
    ));
    assert!(!app.canvas.has_unsliced_snapshot());
    tool_select::clear_selection(&mut app);

    let saved_inst = &app.timeline.layers[0].keyframes[0].cel.instances[0];
    assert!(
        (saved_inst.binding.world_pivot() - new_world_pivot).hypot() < 1e-4,
        "ピボット変更後に選択解除しても stale な unsliced_canvas でピボットが上書き消失しないこと (#8)"
    );
}

#[test]
fn test_move_or_copy_frames_preserves_following_frames_tween_pose_and_remaps_ids() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);

    // 1. 6フレームのレイヤー (Frame 0..=1: 赤, Frame 2..=5: 青) で Frame 0..=1 を Frame 2..=3 へ複製したとき、
    // 移動先範囲直後の Frame 4..=5 の青い絵が上書きされず保持されること (#9-1)
    app.timeline.resize_total_frames(6);
    app.canvas.shapes = make_stroke(Point::new(60.0, 60.0), Point::new(100.0, 60.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    app.canvas.shapes = make_stroke(Point::new(160.0, 60.0), Point::new(200.0, 60.0), 10.0, blue);
    canvas_tool::sync_shapes_to_timeline(&mut app);

    let sel_0_1 = timeline::TimelineCellSelection::from_corners(0, 0, 0, 1);
    assert!(timeline_ui::timeline_move_or_copy_frames(
        &mut app, sel_0_1, 0, 2, true
    ));
    assert_eq!(
        app.timeline.layers[0].cel_at(2).unwrap().shapes[0].color,
        red
    );
    assert_eq!(
        app.timeline.layers[0].cel_at(3).unwrap().shapes[0].color,
        red
    );
    assert_eq!(
        app.timeline.layers[0].cel_at(4).unwrap().shapes[0].color,
        blue,
        "移動先範囲 [2..=3] の直後 Frame 4..=5 の既存セルが上書きされず保持されること (#9-1)"
    );

    // 2. トゥイーン中間フレームから開始する範囲を移動/複製した際、開始フレームの中割り補間姿勢を引き継ぐこと (#9-2)
    let mut tween_app = Is2dApp::default();
    tween_app.timeline.resize_total_frames(5);
    tween_app.canvas.shapes =
        make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 10.0, red);
    canvas_tool::sync_shapes_to_timeline(&mut tween_app);
    tween_app.canvas.selected_shapes = vec![0];
    symbol_ops::convert_selection_to_symbol(&mut tween_app).unwrap();
    assert!(symbol_ops::timeline_toggle_tween(&mut tween_app));
    timeline_ui::switch_to_layer_and_frame(&mut tween_app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut tween_app, true);
    assert!(tool_select::nudge_selected_shapes(
        &mut tween_app,
        kurbo::Vec2::new(200.0, 0.0)
    ));

    // 中間フレーム Frame 2 (t = 0.5 -> dx = +100.0) のみを選択して別レイヤーへ複製
    let mid_shapes = tween_app
        .timeline
        .evaluate_layer_shapes_at(0, 2, &tween_app.symbols.library);
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
            .timeline
            .evaluate_layer_shapes_at(1, 2, &tween_app.symbols.library);
    assert!(
        (copied_shapes[0].bounding_box().center().x - mid_center_x).abs() < 1e-3,
        "トゥイーン途中フレームから複製したセルが中割り補間姿勢を引き継いでいること (#9-2)"
    );

    // 3. 別レイヤーへの複製で instance_gid および group_path ID が重複せず一意に再採番されること (#9-3)
    let orig_gid = tween_app.timeline.layers[0].keyframes[0].cel.instances[0]
        .binding
        .instance_gid;
    let copied_gid = tween_app.timeline.layers[1]
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
        tween_app.symbols.next_group_id > copied_gid,
        "再採番後に SymbolManager::next_group_id が更新され以降の採番と衝突しないこと (#9-3)"
    );
}

#[test]
fn test_unsaved_tab_close_modal_non_blocking_save_discard_and_cancel() {
    let ctx = eframe::egui::Context::default();
    let mut app = Is2dApp::default();

    // タブ0を編集して Dirty にし、タブ1を追加してタブ0を close_tab(Some(&ctx)) で閉じる (#11)
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        10.0,
        Color32::RED,
    );
    app.commit_canvas_mutation(Vec::new());
    assert!(app.history.is_dirty());

    project_io::new_project(&mut app, &ctx);
    assert_eq!(app.tabs.tabs.len(), 2);
    assert_eq!(app.tabs.active_index, 1);

    // Dirty なタブ0を閉じようとすると、rfd でブロックせず PendingCloseConfirm モーダル待機状態になること
    project_io::close_tab(&mut app, 0, Some(&ctx));
    assert!(
        matches!(
            app.tabs.pending_close_confirm,
            Some(project_io::PendingCloseConfirm::SingleTab {
                tab_index: 0,
                orig_active: 1
            })
        ),
        "Dirty タブのクローズ時に OS 同期ダイアログでブロックせず非ブロッキングモーダルが開くこと (#11)"
    );

    // キャンセルを選ぶとタブは閉じられず、元のアクティブタブ (1) に戻ること
    project_io::confirm_pending_close_cancel(&mut app, &ctx);
    assert!(app.tabs.pending_close_confirm.is_none());
    assert_eq!(app.tabs.tabs.len(), 2);
    assert_eq!(app.tabs.active_index, 1);

    // 再度閉じて「保存せずに破棄」を選ぶとタブ0が閉じられること
    project_io::close_tab(&mut app, 0, Some(&ctx));
    project_io::confirm_pending_close_discard(&mut app, &ctx);
    assert!(app.tabs.pending_close_confirm.is_none());
    assert_eq!(app.tabs.tabs.len(), 1);
}

#[test]
fn test_cancel_drag_and_noop_drag_preserve_tessellated_mesh_caches() {
    let mut app = Is2dApp::default();
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        18.0,
        Color32::from_rgb(70, 140, 230),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.canvas.selected_shapes = vec![0];

    // 事前に fill_mesh キャッシュを構築して Arc ポインタを取得 (#12)
    let mesh_arc_before = std::sync::Arc::clone(app.canvas.shapes[0].fill_mesh());
    assert!(app.canvas.shapes[0].has_cached_fill_mesh());

    // 1. 選択移動ドラッグを開始して微小移動未満で終了（noop リストア）してもキャッシュが失われないこと
    let move_drag = clipboard_ops::begin_move_selection_drag(&mut app, Point::new(150.0, 100.0), false);
    tool_select::finish_move_selection_drag(&mut app, move_drag.total_delta, move_drag.undo_snapshot);
    assert!(
        app.canvas.shapes[0].has_cached_fill_mesh()
            && std::sync::Arc::ptr_eq(app.canvas.shapes[0].fill_mesh(), &mesh_arc_before),
        "微小ドラッグのロールバック時にテッセレーション済み Arc<Mesh> が O(1) で引き継がれ再テッセレーションしないこと (#12)"
    );

    // 2. 自由変形ドラッグを開始して Esc 相当の cancel_active_drag で中断してもキャッシュが失われないこと
    app.canvas.selected_shapes = vec![0];
    let bbox = app.canvas.selection_bounding_box(app.viewport.zoom).unwrap();
    tool_transform::start_transform_drag(
        &mut app,
        Point::new(bbox.x0 + 15.0, bbox.center().y),
        false,
        false,
    );
    canvas_tool::cancel_active_drag(&mut app);
    assert!(
        app.canvas.shapes[0].has_cached_fill_mesh()
            && std::sync::Arc::ptr_eq(app.canvas.shapes[0].fill_mesh(), &mesh_arc_before),
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
    assert_ne!(app.timeline.layers[0].id, app.timeline.layers[1].id);
    assert!(app.timeline.layers[0].id <= project_io::MAX_SAFE_ENTITY_ID);
    assert!(app.timeline.layers[1].id <= project_io::MAX_SAFE_ENTITY_ID);
    assert_ne!(app.symbols.library[0].id, app.symbols.library[1].id);
    assert!(app.symbols.library[0].id <= project_io::MAX_SAFE_ENTITY_ID);
    assert!(app.symbols.library[1].id <= project_io::MAX_SAFE_ENTITY_ID);

    // 読み込み後に新規レイヤー・新規シンボル・新規グループIDを追加しても既存IDと一切衝突しないこと (#13)
    timeline_ui::timeline_add_layer(&mut app);
    let layer_ids: std::collections::HashSet<u64> =
        app.timeline.layers.iter().map(|l| l.id).collect();
    assert_eq!(layer_ids.len(), 3, "新規レイヤーのIDが既存レイヤーと衝突しないこと (#13)");

    let new_sym_id = app.symbols.register_new_symbol(Vec::new());
    assert!(
        app.symbols
            .library
            .iter()
            .filter(|s| s.id == new_sym_id)
            .count()
            == 1,
        "新規シンボルのIDが既存シンボルと衝突しないこと (#13)"
    );
    let g1 = app.symbols.alloc_group_id();
    let g2 = app.symbols.alloc_group_id();
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
    for sym in &app.symbols.library {
        for inst in sym.timeline.layers[0].keyframes[0].cel.instances.iter() {
            assert!(
                !app.symbols.would_create_symbol_cycle(sym.id, inst.binding.symbol_id),
                "3色DFS後はいかなるサイクル辺も残存しないこと (#14)"
            );
        }
    }
    assert!(
        !app.symbols.library[0].timeline.layers[0].keyframes[0]
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

    // O キー初回押下でオニオンスキンが ON になり、直後の repeat: true では OFF に戻らないこと (#16)
    assert!(!app.timeline_ui.onion_skin_enabled);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::Key {
                    key: egui::Key::O,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::O,
                    physical_key: None,
                    pressed: true,
                    repeat: true,
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
        app.timeline_ui.onion_skin_enabled,
        "O キー長押しリピートでオニオンスキンが切り替わり続けないこと (#16)"
    );

    // Cmd+3 初回押下で clip_outside_stage が ON になり、repeat: true では OFF に戻らないこと (#16)
    let cmd_mods = egui::Modifiers {
        command: true,
        ctrl: true,
        ..Default::default()
    };
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods,
            events: vec![
                egui::Event::Key {
                    key: egui::Key::Num3,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: cmd_mods,
                },
                egui::Event::Key {
                    key: egui::Key::Num3,
                    physical_key: None,
                    pressed: true,
                    repeat: true,
                    modifiers: cmd_mods,
                },
            ],
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(
        app.viewport.clip_outside_stage,
        "Cmd+3 長押しリピートで枠外マスクが切り替わり続けないこと (#16)"
    );

    // Enter 初回押下で再生開始し、repeat: true では停止しないこと (#16)
    app.timeline.resize_total_frames(4);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: true,
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
        app.timeline_ui.is_playing,
        "Enter 長押しリピートで再生/停止が切り替わり続けないこと (#16)"
    );
}

#[test]
fn test_transform_rotate_zone_only_near_four_corners_and_paste_staircase_across_zoom() {
    let mut app = Is2dApp::default();
    // 横長の図形 (x: 100..300, y: 100..200) を配置して選択
    app.canvas.shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(300.0, 150.0),
        80.0,
        Color32::from_rgb(80, 140, 220),
    );
    canvas_tool::sync_shapes_to_timeline(&mut app);
    app.canvas.selected_shapes = vec![0];
    let bbox = app.canvas.selection_bounding_box(app.viewport.zoom).unwrap();

    // 1. 四隅の少し外側 (+10px, -10px) は Rotate になるが、上辺中央の少し外側 (四隅から100px以上離れた位置) は Rotate にならないこと (#17)
    let corner_outside = Point::new(bbox.x1 + 10.0, bbox.y0 - 10.0);
    assert_eq!(
        tool_transform::compute_transform_hover(&app.canvas, corner_outside, app.viewport.zoom),
        canvas_tool::TransformHoverTarget::Rotate,
        "四隅の外側近傍では Rotate ゾーンになること (#17)"
    );
    let top_edge_outside = Point::new(bbox.center().x, bbox.y0 - 12.0);
    assert_eq!(
        tool_transform::compute_transform_hover(&app.canvas, top_edge_outside, app.viewport.zoom),
        canvas_tool::TransformHoverTarget::None,
        "四隅から離れた辺中央の外側は Rotate ゾーンにならないこと (#17)"
    );

    // 2. 連続ペーストの途中にズーム倍率を変更しても、毎回前回位置から +16px (画面上) ずつ単調増加の階段状に配置されること (#18)
    app.viewport.zoom = 1.0;
    tool_select::copy_selected_shapes(&mut app);
    let c0 = app.canvas.shapes[0].bounding_box().center();

    tool_select::paste_clipboard(&mut app, false);
    let c1 = app
        .canvas
        .shapes
        .last()
        .unwrap()
        .bounding_box()
        .center();
    assert!((c1.x - (c0.x + 16.0)).abs() < 1e-4);

    // 途中でズームイン (zoom = 4.0 -> 1ステップ = 4.0 ワールド座標) してから2回目のペースト
    app.viewport.zoom = 4.0;
    tool_select::paste_clipboard(&mut app, false);
    let c2 = app
        .canvas
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

