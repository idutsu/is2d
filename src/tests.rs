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
        let stroke1 = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 20.0, red);
        merge_shapes_into_canvas(&mut canvas, stroke1);
        assert_eq!(canvas.len(), 1);

        // 2. 交差する同色（赤）の縦ストローク -> Union で1つのシェイプに融合されること
        let stroke2 = make_stroke(Point::new(200.0, 120.0), Point::new(200.0, 280.0), 20.0, red);
        merge_shapes_into_canvas(&mut canvas, stroke2);
        assert_eq!(canvas.len(), 1);
        assert!(canvas[0].contains_point(Point::new(200.0, 200.0)));
        assert!(canvas[0].contains_point(Point::new(120.0, 200.0)));
        assert!(canvas[0].contains_point(Point::new(200.0, 140.0)));

        // 3. 異色（青）のストロークで中央を横断 -> 赤シェイプが Difference で分断され、青シェイプが追加されること
        let stroke3 = make_stroke(Point::new(80.0, 200.0), Point::new(320.0, 200.0), 26.0, blue);
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
        assert!(any_contains(Point::new(165.0, 200.0)), "左側の分割片が存在すること");
        assert!(any_contains(Point::new(235.0, 200.0)), "右側の分割片が存在すること");
        assert!(!any_contains(Point::new(200.0, 200.0)), "突っ切った中央は削られていること");
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
        assert!(tool_paint::bucket_fill_at(&mut app, center), "閉領域がバケツで塗られること");
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
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red);
        merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
        assert_eq!(app.canvas.shapes.len(), 1);

        // 1. 左端 (x: 70..140, y: 150..250) を矩形選択 -> 左端だけスライスされて選択状態になること
        tool_select::apply_marquee_selection(&mut app, Point::new(70.0, 150.0), Point::new(140.0, 250.0), false);
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
            app.canvas.shapes[app.canvas.selected_shapes[0]]
                .contains_point(Point::new(110.0, 200.0)),
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
        tool_select::apply_marquee_selection(&mut app, Point::new(70.0, 150.0), Point::new(140.0, 250.0), false);
        tool_select::apply_marquee_selection(&mut app, Point::new(260.0, 150.0), Point::new(330.0, 250.0), true);
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
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red);
        merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
        assert_eq!(app.canvas.shapes.len(), 1);

        // 1. 選択ツールで左半分 (x: 70..200) を矩形選択
        canvas_tool::set_tool(&mut app, Tool::Select);
        tool_select::apply_marquee_selection(&mut app, Point::new(70.0, 150.0), Point::new(200.0, 250.0), false);
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
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 20.0, red);
        merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
        assert_eq!(app.canvas.shapes.len(), 1);
        app.canvas.selected_shapes = vec![0];
        canvas_tool::set_tool(&mut app, Tool::Transform);

        // 1. +90° 回転 -> 横長だったシェイプが中心 (200, 200) を軸に縦長 (x: 200, y: 100..300) になること
        assert!(tool_transform::rotate_selected_shapes(&mut app, std::f64::consts::FRAC_PI_2));
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
        let left_piece =
            make_stroke(Point::new(130.0, 200.0), Point::new(170.0, 200.0), 18.0, red);
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

        // 左側に赤、右側に青のシェイプを描画
        let s_red = make_stroke(Point::new(100.0, 200.0), Point::new(180.0, 200.0), 20.0, red);
        let s_blue = make_stroke(Point::new(240.0, 200.0), Point::new(320.0, 200.0), 20.0, blue);
        merge_shapes_into_canvas(&mut app.canvas.shapes, s_red);
        merge_shapes_into_canvas(&mut app.canvas.shapes, s_blue);

        app.ui.brush_color = green;
        canvas_tool::set_tool(&mut app, Tool::Eyedropper);

        // 1. 空白部分をクリックしても色は変わらないこと
        assert_eq!(tool_paint::pick_color_at(&mut app, Point::new(20.0, 20.0)), None);
        assert_eq!(app.ui.brush_color, green);

        // 2. 赤シェイプ上をクリックすると brush_color が赤になること
        assert_eq!(tool_paint::pick_color_at(&mut app, Point::new(140.0, 200.0)), Some(red));
        assert_eq!(app.ui.brush_color, red);

        // 3. 青シェイプ上をクリックすると brush_color が青になること
        assert_eq!(tool_paint::pick_color_at(&mut app, Point::new(280.0, 200.0)), Some(blue));
        assert_eq!(app.ui.brush_color, blue);
    }

    #[test]
    fn test_nested_grouping_ungrouping_and_edit_in_place() {
        let mut app = Is2dApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        let blue = Color32::from_rgb(55, 135, 245);
        let green = Color32::from_rgb(65, 195, 105);

        // 1. ベースの赤い大シェイプ (x: 100..300, y: 200) と、別の位置に青い白目 (x: 100, y: 100) + 緑の黒目 (x: 140, y: 100) を描画
        let face = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 30.0, red);
        let eye_a = make_stroke(Point::new(90.0, 100.0), Point::new(110.0, 100.0), 15.0, blue);
        let eye_b = make_stroke(Point::new(135.0, 100.0), Point::new(155.0, 100.0), 12.0, green);
        merge_shapes_into_canvas(&mut app.canvas.shapes, face);
        merge_shapes_into_canvas(&mut app.canvas.shapes, eye_a);
        merge_shapes_into_canvas(&mut app.canvas.shapes, eye_b);
        assert_eq!(app.canvas.shapes.len(), 3);

        // 2. 青と緑の2シェイプを選択して 1段目のグループ化 (Cmd+G -> eye_gid)
        app.canvas.selected_shapes = vec![1, 2];
        let eye_gid = tool_select::group_selected_shapes(&mut app)
            .expect("1段目のグループ化が成功すること");
        assert_eq!(app.canvas.shapes[1].group_path, vec![eye_gid]);
        assert_eq!(app.canvas.shapes[2].group_path, vec![eye_gid]);

        // 3. 1つクリックするだけでグループ全体の2シェイプが同時選択されること
        tool_select::clear_selection(&mut app);
        assert!(tool_select::select_single_shape_at(&mut app, Point::new(100.0, 100.0)));
        assert_eq!(
            app.canvas.selected_shapes.len(),
            2,
            "グループの片方をクリックするとグループ塊全体が選択されること"
        );

        // 3b. グループ選択状態でバケツツールを使っても、グループ内の絵の色は変わらず無効であること
        app.ui.brush_color = red;
        assert!(
            !tool_paint::bucket_fill_at(&mut app, Point::new(100.0, 100.0)),
            "選択中のグループに対してバケツ塗りは無効であること"
        );
        assert_eq!(app.canvas.shapes[1].color, blue);
        assert_eq!(app.canvas.shapes[2].color, green);

        // 4. グループを赤い顔シェイプの上 (y: 100 -> y: 200) へ移動して選択解除しても、下の赤い顔シェイプが切り抜かれず両方共存すること
        let snap = app.canvas.shapes.clone();
        app.canvas.translate_selected(KVec2::new(0.0, 100.0));
        tool_select::commit_moved_selection(&mut app, snap);
        tool_select::clear_selection(&mut app);
        assert_eq!(
            app.canvas.shapes.len(),
            3,
            "グループ化されたパーツは選択解除しても下の生シェイプを切り抜かないこと"
        );
        assert!(
            app.canvas.shapes[0].contains_point(Point::new(100.0, 200.0)),
            "下の赤い顔シェイプが削られずに残っていること"
        );

        // 5. さらに「赤い顔シェイプ (idx 0)」と「目グループ (idx 1, 2)」をまとめて 2段目の入れ子グループ化 (Cmd+G -> head_gid)
        app.canvas.selected_shapes = vec![0, 1, 2];
        let head_gid = tool_select::group_selected_shapes(&mut app)
            .expect("2段目の入れ子グループ化が成功すること");
        assert_eq!(app.canvas.shapes[0].group_path, vec![head_gid]);
        assert_eq!(app.canvas.shapes[1].group_path, vec![head_gid, eye_gid]);
        assert_eq!(app.canvas.shapes[2].group_path, vec![head_gid, eye_gid]);

        // 6. ダブルクリック相当 (enter_group_at) で head_gid の中へインプレイス編集で潜る
        assert!(tool_select::enter_group_at(&mut app, Point::new(200.0, 200.0)));
        assert_eq!(app.canvas.editing_group_path, vec![head_gid]);
        // head_gid の中では、赤い顔 (idx 0) は生シェイプ、目 (idx 1, 2) は子グループ eye_gid として扱われること
        assert!(app.canvas.shapes[0].is_raw_at_scope(&app.canvas.editing_group_path));
        assert_eq!(
            app.canvas.shapes[1].active_group_id_at_scope(&app.canvas.editing_group_path),
            Some(eye_gid)
        );
        // さらに目グループをダブルクリックすると 2階層目 [head_gid, eye_gid] へ潜れること
        assert!(tool_select::enter_group_at(&mut app, Point::new(100.0, 200.0)));
        assert_eq!(app.canvas.editing_group_path, vec![head_gid, eye_gid]);
        assert!(app.canvas.shapes[1].is_raw_at_scope(&app.canvas.editing_group_path));

        // 2回 exit_current_group でルート階層に戻り、head_gid が選択状態になること
        assert!(tool_select::exit_current_group(&mut app));
        assert_eq!(app.canvas.editing_group_path, vec![head_gid]);
        assert!(tool_select::exit_current_group(&mut app));
        assert!(app.canvas.editing_group_path.is_empty());
        assert_eq!(app.canvas.selected_shapes.len(), 3);

        // 7. ルート階層で 1回 グループ解除 (Cmd+B / ungroup_selected_shapes) すると、
        // 外側の head_gid だけが外れ、内側の eye_gid グループはそのまま維持されること
        assert!(tool_select::ungroup_selected_shapes(&mut app));
        assert!(app.canvas.shapes[0].group_path.is_empty());
        assert_eq!(app.canvas.shapes[1].group_path, vec![eye_gid]);
        assert_eq!(app.canvas.shapes[2].group_path, vec![eye_gid]);
    }

    #[test]
    fn test_zoom_around_cursor_and_fine_brush_at_high_zoom() {
        let mut app = Is2dApp::default();
        let cursor = Pos2::new(320.0, 240.0);

        // 1. カーソル位置 (320, 240) を中心に 400% -> 800% へズームしても、カーソル直下のワールド座標が不変であること
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

        // 2. 800% ズーム状態で極細ブラシ（半径 0.5 px）のストロークを描いた際、微小面積フィルタで消えず滑らかなシェイプが生成されること
        app.ui.brush_radius = 0.5;
        let fine_raw = generate_brush_outline_at_zoom(
            &[Point::new(320.0, 240.0), Point::new(323.0, 240.0)],
            app.ui.brush_radius as f64,
            app.viewport.zoom as f64,
        );
        tool_paint::commit_raw_outline(&mut app, &fine_raw);
        assert_eq!(
            app.canvas.shapes.len(),
            1,
            "800% ズーム時の 0.5px 極細ストロークが消えずにシェイプ化されること"
        );
        assert!(app.canvas.shapes[0].contains_point(Point::new(321.5, 240.0)));

        // 3. 100% ズーム (r=8px) と 400% ズーム (r=2px、画面上では同じ8px相当) で同じ画面軌跡を描いたとき、
        //    スムーズ化後のベジェ曲線セグメント数が完全に一致し、面積も正確に 1/16 (1/4^2) スケールになること
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
            "100%表示と400%拡大表示で、画面上同一のストロークに対するスムーズ化結果のアンカー数が完全一致すること"
        );
        let area_ratio = shapes_1x[0].exterior.signed_area() / shapes_4x[0].exterior.signed_area();
        assert!(
            (area_ratio - 16.0).abs() < 0.05,
            "面積比がズーム倍率の2乗 (16.0) と一致すること: area_ratio={area_ratio}"
        );

        // 4. リセット (Cmd+0 相当) で 100%・原点パンへ戻ること
        app.viewport.reset_view();
        assert_eq!(app.viewport.zoom, 1.0);
        assert_eq!(app.viewport.pan, Vec2::ZERO);
    }

    #[test]
    fn test_copy_cut_paste_and_paste_in_place() {
        let mut app = Is2dApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        let blue = Color32::from_rgb(55, 135, 245);

        // 1. 横長の赤シェイプ (x: 100..300, y: 200) を描画し、左半分 (x: 70..200) を矩形選択してカット (Cmd+X)
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red);
        merge_shapes_into_canvas(&mut app.canvas.shapes, stroke);
        assert_eq!(app.canvas.shapes.len(), 1);

        tool_select::apply_marquee_selection(&mut app, Point::new(70.0, 150.0), Point::new(200.0, 250.0), false);
        assert!(tool_select::cut_selected_shapes(&mut app), "Cmd+X で選択領域がカットされること");
        assert_eq!(app.canvas.shapes.len(), 1, "右半分のみがキャンバスに残ること");
        assert!(!app.canvas.shapes[0].contains_point(Point::new(120.0, 200.0)));
        assert!(app.canvas.shapes[0].contains_point(Point::new(260.0, 200.0)));

        // 2. 同じ位置に貼り付け (Cmd+Shift+V) -> 元の左半分座標 (120, 200) に貼り付けられ、選択解除で右半分と1つに再結合されること
        assert!(
            tool_select::paste_clipboard(&mut app, true),
            "Cmd+Shift+V で同じ位置に貼り付けられること"
        );
        assert_eq!(app.canvas.selected_shapes.len(), 1);
        assert!(app.canvas.shapes[app.canvas.selected_shapes[0]]
            .contains_point(Point::new(120.0, 200.0)));
        tool_select::clear_selection(&mut app);
        assert_eq!(
            app.canvas.shapes.len(),
            1,
            "同じ位置に貼り付けた同色生シェイプは選択解除時に元の右半分とシームレスに結合されること"
        );
        assert!(app.canvas.shapes[0].contains_point(Point::new(120.0, 200.0)));
        assert!(app.canvas.shapes[0].contains_point(Point::new(260.0, 200.0)));

        // 3. グループ化されたオブジェクトのコピー (Cmd+C) ＆通常貼り付け (Cmd+V) で、
        //    元のグループとは独立した新しいグループIDが採番され、オフセット (+16, +16) 位置に配置されること
        app.canvas.shapes.clear();
        let g_part = make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 15.0, blue);
        merge_shapes_into_canvas(&mut app.canvas.shapes, g_part);
        app.canvas.selected_shapes = vec![0];
        let orig_gid = tool_select::group_selected_shapes(&mut app).unwrap();

        assert!(tool_select::copy_selected_shapes(&mut app), "Cmd+C でグループがコピーされること");
        assert!(
            tool_select::paste_clipboard(&mut app, false),
            "Cmd+V でオフセット付き貼り付けが成功すること"
        );
        assert_eq!(app.canvas.shapes.len(), 2);
        let pasted_idx = app.canvas.selected_shapes[0];
        let pasted_gid = app.canvas.shapes[pasted_idx].group_path[0];
        assert_ne!(
            pasted_gid, orig_gid,
            "複製されたグループには元のグループとは別の一意なグループIDが割り当てられること"
        );
        assert!(
            app.canvas.shapes[pasted_idx]
                .contains_point(Point::new(120.0 + 16.0, 100.0 + 16.0)),
            "Cmd+V では (+16, +16) ずれた位置に貼り付けられること"
        );

        // 4. Alt + ドラッグ複製 (duplicate_selected_shapes_for_alt_drag) でシンボルやグループが独立したIDで複製移動でき、
        //    1回の Undo で複製前の状態に戻ること
        let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
        let orig_inst_gid = tool_transform::selected_single_symbol_info(&app).unwrap().0.instance_gid;
        let count_before = app.canvas.shapes.len();

        let undo_snapshot = tool_select::duplicate_selected_shapes_for_alt_drag(&mut app);
        assert_eq!(app.canvas.shapes.len(), count_before + 1);
        app.canvas.translate_selected(KVec2::new(60.0, 40.0));
        tool_select::finish_move_selection_drag(&mut app, KVec2::new(60.0, 40.0), undo_snapshot);

        let (dup_binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
        assert_eq!(dup_binding.symbol_id, sym_id);
        assert_ne!(
            dup_binding.instance_gid, orig_inst_gid,
            "Alt+ドラッグで複製されたシンボルインスタンスには新しい一意な instance_gid が採番されること"
        );
        assert!(
            app.canvas.shapes[app.canvas.selected_shapes[0]]
                .contains_point(Point::new(120.0 + 16.0 + 60.0, 100.0 + 16.0 + 40.0)),
            "複製されたシンボルがドラッグ先座標に移動していること"
        );
        app.undo();
        assert_eq!(
            app.canvas.shapes.len(),
            count_before,
            "1回の Undo で Alt+ドラッグ複製前の状態へ戻ること"
        );
    }

    #[test]
    fn test_timeline_layer_keyframe_and_cel_lookup() {
        let mut timeline = Timeline::default();
        assert_eq!(timeline.fps, 24);
        assert_eq!(timeline.total_frames, 1);
        assert_eq!(timeline.layers.len(), 1);
        assert_eq!(timeline.layers[0].frame_count, 1);
        assert_eq!(timeline.current_frame, 0);

        let red = Color32::from_rgb(235, 75, 75);
        let stroke = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 12.0, red);
        if let Some(cel) = timeline.layers[0].cel_at_mut(0) {
            merge_shapes_into_canvas(cel.shapes_mut(), stroke);
        }
        assert_eq!(timeline.active_cel_shapes().len(), 1);

        // フレーム 4 に空白キーフレーム (F7: clone_prev = false) を挿入すると、
        // frame_count が 5 に拡張され、フレーム 0..=3 は元のキーフレーム、フレーム 4 は空の新しいキーフレームを参照すること
        timeline.layers[0].insert_keyframe(4, false);
        assert_eq!(timeline.layers[0].frame_count, 5);
        assert_eq!(timeline.layers[0].keyframe_index_at(3), Some(0));
        assert_eq!(timeline.layers[0].keyframe_index_at(4), Some(1));
        assert_eq!(timeline.layers[0].cel_at(2).unwrap().shapes.len(), 1);
        assert!(timeline.layers[0].cel_at(4).unwrap().is_empty());

        // フレーム 2 の位置からフレーム 3 に複製キーフレーム (F6: clone_prev = true) を挿入すると図形が引き継がれること
        timeline.layers[0].insert_keyframe(3, true);
        assert_eq!(timeline.layers[0].cel_at(3).unwrap().shapes.len(), 1);

        // Is2dApp 統合テスト：F6（複製キーフレーム進行）・F7（空白キーフレーム進行）・F5（延長）・レイヤー操作・Undo/Redo
        let mut app = Is2dApp::default();
        let blue = Color32::from_rgb(55, 135, 245);
        let s0 = make_stroke(Point::new(100.0, 100.0), Point::new(150.0, 100.0), 10.0, red);
        merge_shapes_into_canvas(&mut app.canvas.shapes, s0);
        canvas_tool::sync_shapes_to_timeline(&mut app);
        assert_eq!(app.timeline.current_frame, 0);
        assert_eq!(app.canvas.shapes.len(), 1);

        // 1. キーフレーム先頭 (frame 0) で F6 を押すと、自動的に次のフレーム (frame 1) へ進みつつ絵が複製されること
        assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
        assert_eq!(app.timeline.current_frame, 1);
        assert_eq!(app.canvas.shapes.len(), 1);

        // 2. 絵があるキーフレーム先頭 (frame 1) で F7 を押すと、次のフレーム (frame 2) に空白キーフレームが作成されること
        assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
        assert_eq!(app.timeline.current_frame, 2);
        assert!(app.canvas.shapes.is_empty());

        // 3. F5 でフレーム延長・Shift+F5 で短縮・Shift+F6 でキーフレーム削除と Undo/Redo が正確に動作すること
        let prev_total = app.timeline.total_frames;
        assert!(timeline_ui::timeline_extend_frame(&mut app));
        assert_eq!(app.timeline.total_frames, prev_total + 1);
        assert!(timeline_ui::timeline_remove_frame(&mut app));
        assert_eq!(app.timeline.total_frames, prev_total);

        // 3b. Adobe Animate 仕様：現在の尺（prev_total）より先の空マス（例: frame 9 = 10フレーム目）を選択して F5 を押すと、
        //     そのフレーム位置まで一気に保持尺が伸びること
        timeline_ui::select_timeline_cells(&mut app, 0, 9, 0, 9);
        assert_eq!(app.timeline.current_frame, 9);
        assert!(timeline_ui::timeline_extend_frame(&mut app));
        assert_eq!(app.timeline.layers[0].frame_count, 10);
        assert_eq!(app.timeline.total_frames, 10);
        app.undo();
        assert_eq!(app.timeline.total_frames, prev_total);
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);

        assert!(timeline_ui::timeline_delete_keyframe(&mut app));
        assert_eq!(
            app.canvas.shapes.len(),
            1,
            "frame 2 の空白キーフレームを削除すると直前の frame 1 の絵が保持表示されること"
        );
        app.undo();
        assert!(
            app.canvas.shapes.is_empty(),
            "Undo で削除前の空白キーフレーム状態に戻ること"
        );
        app.redo();
        assert_eq!(app.canvas.shapes.len(), 1);

        // 4. 新規レイヤー追加・複数レイヤー横断範囲選択（Adobe Animate 挙動）・ドラッグ＆ドロップ重ね順入れ替え・削除
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        assert_eq!(app.canvas.shapes.len(), 1);
        timeline_ui::timeline_add_layer(&mut app); // Layer 2 (index 1)
        assert_eq!(app.timeline.layers.len(), 2);
        assert_eq!(app.timeline.active_layer, 1);
        assert!(
            app.timeline.active_cel_shapes().is_empty(),
            "新しく追加された前面レイヤーのセルは空であること"
        );
        assert_eq!(
            app.canvas.shapes.len(),
            1,
            "作業キャンバス self.canvas.shapes には全可視・非ロックレイヤーの図形が展開されていること"
        );
        app.ui.brush_color = blue;
        assert_eq!(
            tool_paint::pick_color_at(&mut app, Point::new(125.0, 100.0)),
            Some(red),
            "空の前面レイヤー上でも、可視状態の背面レイヤーにある図形の色をスポイトで抽出できること"
        );

        // Layer 2 (index 1) に青いストロークを追加（Layer 1 の赤いストロークと重なる位置）
        app.ui.brush_color = blue;
        tool_paint::commit_raw_outline(&mut app, &[
            Point::new(120.0, 70.0),
            Point::new(120.0, 130.0),
            Point::new(135.0, 130.0),
            Point::new(135.0, 70.0),
        ]);
        canvas_tool::sync_shapes_to_timeline(&mut app);
        assert_eq!(
            app.timeline.layers[0].cel_at(0).unwrap().shapes.len(),
            1,
            "別レイヤーのストロークによって背面レイヤーの図形が削られないこと（レイヤー間の Merge Drawing 分離）"
        );
        assert_eq!(app.timeline.layers[1].cel_at(0).unwrap().shapes.len(), 1);
        assert_eq!(app.canvas.shapes.len(), 2);

        // 現在 Layer 2 を選択していても、範囲選択（Marquee Selection）で Layer 1 と Layer 2 の両方の絵を一括選択・移動できること
        tool_select::apply_marquee_selection(&mut app, Point::new(50.0, 50.0), Point::new(200.0, 200.0), false);
        assert_eq!(
            app.canvas.selected_shapes.len(),
            2,
            "Adobe Animate 同様に、範囲選択で複数レイヤーにまたがる全ての非ロック図形を一括選択できること"
        );
        let undo_snap = app.canvas.shapes.clone();
        app.canvas.translate_selected(KVec2::new(30.0, 0.0));
        tool_select::commit_moved_selection(&mut app, undo_snap);
        tool_select::clear_selection(&mut app);
        canvas_tool::sync_shapes_to_timeline(&mut app);
        assert!(
            app.timeline.layers[0].cel_at(0).unwrap().shapes[0]
                .contains_point(Point::new(155.0, 100.0)),
            "Layer 1 の図形が移動され、かつ Layer 1 に留まっていること"
        );
        assert!(
            app.timeline.layers[1].cel_at(0).unwrap().shapes[0]
                .contains_point(Point::new(155.0, 100.0)),
            "Layer 2 の図形も同時に移動され、かつ Layer 1 の図形を削らずに Layer 2 に留まっていること"
        );

        // Layer 1 をロック (🔒) すると、範囲選択では Layer 2 の図形のみが選択され、Layer 1 は保護されること
        timeline_ui::set_layer_locked(&mut app, 0, true);
        tool_select::apply_marquee_selection(&mut app, Point::new(50.0, 50.0), Point::new(250.0, 200.0), false);
        assert_eq!(
            app.canvas.selected_shapes.len(),
            1,
            "ロックされたレイヤーの図形は範囲選択から除外されて保護されること"
        );
        tool_select::clear_selection(&mut app);
        timeline_ui::set_layer_locked(&mut app, 0, false);
        assert_eq!(app.canvas.shapes.len(), 2);

        timeline_ui::timeline_add_layer(&mut app); // Layer 3 (index 2)
        assert_eq!(app.timeline.layers.len(), 3);
        assert_eq!(
            app.timeline
                .layers
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Layer 1", "Layer 2", "Layer 3"]
        );
        // Layer 3 (index 2) を最背面 (index 0) へドラッグ＆ドロップ移動
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
        // Undo / Redo でレイヤー重ね順が完全に復元されること
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

        // 5. 追加仕様6項目の検証：
        // (1) レイヤー選択状態に応じた F5 / Shift+F5 挙動（選択レイヤーのみ vs 全レイヤー）
        let mut app2 = Is2dApp::default();
        tool_paint::commit_raw_outline(&mut app2, &[
            Point::new(40.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(80.0, 80.0),
            Point::new(40.0, 80.0),
        ]);
        timeline_ui::timeline_add_layer(&mut app2); // Layer 2 (index 1)
        app2.ui.brush_color = blue;
        tool_paint::commit_raw_outline(&mut app2, &[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ]);
        canvas_tool::sync_shapes_to_timeline(&mut app2);
        let base_len = app2.timeline.layers[0].frame_count;
        assert_eq!(app2.timeline.layers[1].frame_count, base_len);

        // Layer 2 (index 1) だけを選択した状態で F5 -> Layer 2 だけが +1 伸び、Layer 1 はそのまま
        timeline_ui::select_timeline_cells(&mut app2, 1, 0, 1, 0);
        assert!(timeline_ui::timeline_extend_frame(&mut app2));
        assert_eq!(app2.timeline.layers[0].frame_count, base_len);
        assert_eq!(app2.timeline.layers[1].frame_count, base_len + 1);

        // ルーラースクラブ等でレイヤー未選択状態 (cell_selection = None) にして F5 -> 全レイヤーが +1 伸びる
        timeline_ui::scrub_ruler_to_frame(&mut app2, 0);
        assert!(app2.timeline.cell_selection.is_none());
        assert!(timeline_ui::timeline_extend_frame(&mut app2));
        assert_eq!(app2.timeline.layers[0].frame_count, base_len + 1);
        assert_eq!(app2.timeline.layers[1].frame_count, base_len + 2);

        // (2) キーフレーム（フレームセル）をクリックしたとき、そのキーフレーム内の絵が全て選択状態になること
        timeline_ui::select_timeline_cells(&mut app2, 0, 0, 0, 0);
        assert_eq!(
            app2.canvas.selected_shapes.len(),
            1,
            "Layer 1 のキーフレームをクリックすると Layer 1 の絵が全選択されること"
        );
        assert_eq!(
            app2.canvas.shapes[app2.canvas.selected_shapes[0]].layer_id,
            app2.timeline.layers[0].id
        );

        // (3) キャンバスの任意の絵を選択したとき、その絵が所属するレイヤーがタイムラインで選択状態になること
        tool_select::clear_selection(&mut app2);
        timeline_ui::scrub_ruler_to_frame(&mut app2, 0);
        assert!(tool_select::select_single_shape_at(&mut app2, Point::new(160.0, 60.0))); // Layer 2 の青い矩形をクリック
        assert_eq!(app2.timeline.active_layer, 1);
        assert_eq!(
            app2.timeline.cell_selection.map(|s| (s.min_layer, s.max_layer)),
            Some((1, 1)),
            "キャンバスの絵を選択すると、その絵が属するレイヤーがタイムラインでも選択状態になること"
        );

        // (4) タイムライン上でレイヤーをまたいでドラッグ範囲選択すると、それらのレイヤーの絵がすべて選択状態になること
        timeline_ui::select_timeline_cells(&mut app2, 0, 0, 1, 0);
        assert_eq!(
            app2.canvas.selected_shapes.len(),
            2,
            "レイヤーをまたいでタイムラインセルを範囲選択すると、対象レイヤー群の絵がすべて選択状態になること"
        );

        // (5) 選択フレームのドラッグ移動および Alt+ドラッグ複製（コピー）
        let sel_l1_f0 = timeline::TimelineCellSelection::single(1, 0);
        // Alt+ドラッグ（copy_mode = true）で Layer 2 の frame 0 を frame 3 へ複製
        assert!(timeline_ui::timeline_move_or_copy_frames(&mut app2, sel_l1_f0, 0, 3, true));
        assert!(
            !app2.timeline.layers[1].cel_at(0).unwrap().is_empty(),
            "Alt+ドラッグ複製では元の frame 0 の絵も残ること"
        );
        assert_eq!(
            app2.timeline.layers[1].keyframe_index_at(3),
            Some(1),
            "frame 3 に複製された新しいキーフレームが作成されていること"
        );
        assert!(!app2.timeline.layers[1].cel_at(3).unwrap().is_empty());

        // 複製された frame 3 のキーフレームを frame 5 へ通常ドラッグ移動（copy_mode = false）
        let sel_l1_f3 = timeline::TimelineCellSelection::single(1, 3);
        assert!(timeline_ui::timeline_move_or_copy_frames(&mut app2, sel_l1_f3, 0, 2, false));
        assert_eq!(
            app2.timeline.layers[1].keyframes[1].start_frame,
            5,
            "通常ドラッグ移動では frame 3 のキーフレームが frame 5 へ移動していること"
        );
        assert!(
            !app2.timeline.layers[1].cel_at(5).unwrap().is_empty(),
            "移動先の frame 5 に絵が移動していること"
        );
        // 先頭 frame 0 のキーフレームを frame 2 へ通常ドラッグ移動すると、frame 0 は空キーフレームになること
        let sel_l0_f0 = timeline::TimelineCellSelection::single(0, 0);
        assert!(timeline_ui::timeline_move_or_copy_frames(&mut app2, sel_l0_f0, 0, 2, false));
        assert!(
            app2.timeline.layers[0].cel_at(0).unwrap().is_empty(),
            "先頭 frame 0 のキーフレームを後続へ移動すると frame 0 は空白キーフレームになること"
        );
        assert!(
            !app2.timeline.layers[0].cel_at(2).unwrap().is_empty(),
            "移動先の frame 2 に Layer 1 の絵が移動していること"
        );

        // (6) Shift+F5（フレーム削除）がキーフレームに当たったときの Adobe Animate 準拠挙動：
        // 保持尺が 2 コマ以上あるときはキーフレームを残して尺を縮め、1 コマ単独キーフレームのときはそのキーフレーム自体を削除して後続を左へ詰める
        // Layer 2 の frame 3 と frame 4 にキーフレームを挿入し、frame 3 を 1 コマ単独キーフレームにする
        app2.timeline.layers[1].insert_keyframe(3, false);
        app2.timeline.layers[1].insert_keyframe(4, false);
        let kf_count_before = app2.timeline.layers[1].keyframes.len();
        // frame 0 は保持尺が 3 コマ (0, 1, 2) あるので、frame 0 で Shift+F5 を押すと kf[0] は消えず保持尺が 2 コマになり後続が 1 コマ左へ詰まる
        timeline_ui::select_timeline_cells(&mut app2, 1, 0, 1, 0);
        assert!(timeline_ui::timeline_remove_frame(&mut app2));
        assert_eq!(
            app2.timeline.layers[1].keyframes.len(),
            kf_count_before,
            "保持フレームがあるキーフレーム上での Shift+F5 はキーフレームを消さず保持尺のみ1コマ縮めること"
        );
        // 次に 1 コマ幅になったキーフレーム（旧 frame 3 -> 現 frame 2）の上で Shift+F5 を押すと、その1コマKF自体が削除され後続が左へ詰まる
        timeline_ui::select_timeline_cells(&mut app2, 1, 2, 1, 2);
        assert!(timeline_ui::timeline_remove_frame(&mut app2));
        assert_eq!(
            app2.timeline.layers[1].keyframes.len(),
            kf_count_before - 1,
            "保持フレームがない1コマ単独キーフレーム上での Shift+F5 はそのキーフレーム自体を削除して後続を左へ詰めること"
        );

        // (7) Cel の結合メッシュキャッシュと再生中ゼロコピーの検証
        let cel = app2.timeline.layers[1].cel_at(0).unwrap();
        let m1 = cel.cached_screen_mesh(1.0, Vec2::ZERO, false);
        let m2 = cel.cached_screen_mesh(1.0, Vec2::ZERO, false);
        assert!(!m1.is_empty());
        assert_eq!(m1.vertices.len(), m2.vertices.len());
        timeline_ui::toggle_playback(&mut app2);
        assert!(app2.timeline_ui.is_playing);
        app2.timeline.current_frame = 0;
        timeline_ui::toggle_playback(&mut app2);
        assert!(!app2.timeline_ui.is_playing);
        assert_eq!(
            app2.canvas.shapes.len(),
            1,
            "再生停止時に停止フレーム (frame 0) の図形がキャンバスに復元されること"
        );
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
    fn test_g1_tangent_continuity_and_spine_smoothing() {
        let center = Point::new(220.0, 220.0);
        let circle_r = 60.0;
        let brush_r = 10.0;

        // ピクセルジャギーの入った円ストロークを生成
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

        // 外周・内周のすべてのアンカーにおいて、前後の2次ベジェ曲線の接線方向が一直線（G1連続）に揃っていること
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

        // 短い辺（約35px）のコンパクトな V 字ストロークを描いたとき、角が勝手に丸められて削られないこと
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
        // V字の下端外角頂点 (125, 130 + r_v * 0.85) が丸められて欠けず、しっかり塗られていること
        assert!(
            v_shape.contains_point(Point::new(125.0, 130.0 + r_v * 0.85)),
            "コンパクトなV字の外角先端が勝手に丸められて削られないこと"
        );
        // V字の内角の谷間 (125, 114) が勝手に丸められて埋まらず、空白のままであること
        assert!(
            !v_shape.contains_point(Point::new(125.0, 114.0)),
            "コンパクトなV字の内角が勝手に丸められて潰れないこと"
        );

        // L字ストローク (100,100) -> (100,200) -> (200,200) で外角・内角ともに前後の直線が巻き込まれて大きく丸まらないこと
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
        assert!(
            l_shape.contains_point(Point::new(91.5, 198.5)),
            "L字の外角付近が削られて大きく丸まらないこと"
        );
        assert!(
            !l_shape.contains_point(Point::new(88.0, 185.0)),
            "L字の外角手前の直線辺が外側に膨らまないこと"
        );
        assert!(
            !l_shape.contains_point(Point::new(112.5, 187.5)),
            "L字の内角 (110, 190) が丸められて埋まらないこと"
        );

        // 「あ」の3画目のように、書き始めの先端が交差部から少しだけ（8px〜16px）突き出て自己交差するストロークで、
        // 右上の交差谷間（凹V字ノッチ）が丸められてコブになったり突起が潰れたりしないこと
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

            // 右上の突起先端 (tip_start) の外周付近が削られずに残っていること
            let tip_apex = tip_start - dir_down_left * (r_a * 0.80);
            assert!(
                a_shape.contains_point(tip_apex),
                "protrusion={tip_protrusion}: 『あ』の右上突起の先端が丸められて潰れないこと"
            );

            // 突起の右側にある交差くびれ（凹V字ノッチ）のすぐ外側が勝手に繋がってコブ状に埋まらないこと
            let valley_outside_probe = Point::new(cross_pt.x + r_a + 3.2, cross_pt.y - 5.5);
            assert!(
                !a_shape.contains_point(valley_outside_probe),
                "protrusion={tip_protrusion}: 『あ』の右上交差部の谷間が丸められてコブ状に繋がらないこと"
            );
        }

        // ゆらゆらした線（S字の波線）を描いたとき、軌道が勝手に直線化されず描いた波の山・谷が保持されること
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
        // 波の下側の山 (x=125, y=204.5) と上側の谷 (x=175, y=195.5) が平坦化されず保持されていること
        assert!(
            wave_shape.contains_point(Point::new(125.0, 211.0)),
            "ゆらゆらした波線の山 (x=125) が勝手に直線化されて削られないこと"
        );
        assert!(
            !wave_shape.contains_point(Point::new(125.0, 194.5)),
            "ゆらゆらした波線の山の反対側が勝手に直線化されて埋まらないこと"
        );
        assert!(
            wave_shape.contains_point(Point::new(175.0, 189.0)),
            "ゆらゆらした波線の谷 (x=175) が勝手に直線化されて削られないこと"
        );
        assert!(
            !wave_shape.contains_point(Point::new(175.0, 205.5)),
            "ゆらゆらした波線の谷の反対側が勝手に直線化されて埋まらないこと"
        );

        // 小さめの円（半径 15px 程度で、少し横長かつ右上が膨らんだ手描きの小円）を描いたとき、
        // 勝手に正円へ整形されず、描いた小円の非対称な輪郭と内周穴がそのまま維持されること
        let small_center = Point::new(300.0, 300.0);
        let r_small_brush = 6.0;
        let small_pts: Vec<Point> = (0..=64)
            .map(|i| {
                let theta = (i as f64 / 62.0) * std::f64::consts::TAU;
                // 横方向は半径 17px、縦方向は半径 12px に加え、右上 (theta ≈ -PI/4) に +2.2px の手描き膨らみを持つ小円
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
        assert_eq!(small_shape.holes.len(), 1, "小さめの円の中央の穴が潰れないこと");
        // 横方向 (theta = 0: r = 17, 外周 x = 300 + 17 + 6 = 323) はしっかり塗られ、
        // 縦方向 (theta = PI/2: r = 12, 外周 y = 300 + 12 + 6 = 318) は正円化で膨らまないこと
        assert!(
            small_shape.contains_point(Point::new(322.2, 300.0)),
            "小さめの円の横方向の張り出しが勝手に縮められないこと"
        );
        assert!(
            !small_shape.contains_point(Point::new(300.0, 319.2)),
            "小さめの円の縦方向が正円化によって勝手に外側へ膨らまないこと"
        );
        // 内周穴の横方向 (x = 300 + 17 - 6 = 311 の内側 309.8) が空白のままであること
        assert!(
            !small_shape.contains_point(Point::new(309.8, 300.0)),
            "小さめの円の内周穴の横幅が勝手に丸められて狭まらないこと"
        );
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

        // 1. F8 シンボル化で "Symbol 1" がライブラリに登録され、インスタンスになること
        let sym_id = tool_transform::convert_selection_to_symbol(&mut app)
            .expect("シンボル変換が成功すること");
        assert_eq!(app.symbols.library.len(), 1);
        assert_eq!(app.symbols.library[0].id, sym_id);
        assert_eq!(app.symbols.library[0].name, "Symbol 1");
        assert!(tool_transform::selected_single_symbol_info(&app).is_some());

        // 2. ライブラリからステージへ2つ目のインスタンスを配置できること
        assert!(tool_transform::place_symbol_instance_from_library(&mut app, sym_id));
        assert_eq!(app.canvas.shapes.len(), 2);
        assert_ne!(
            app.canvas.shapes[0].symbol_instance.unwrap().instance_gid,
            app.canvas.shapes[1].symbol_instance.unwrap().instance_gid
        );

        // 3. 1つ目のインスタンスをダブルクリックすると、シンボル専用のタイムラインへ切り替わること
        assert!(tool_select::enter_group_at(&mut app, Point::new(150.0, 150.0)));
        assert_eq!(app.symbols.edit_stack.len(), 1);
        assert_eq!(app.timeline.total_frames, 1);

        // シンボル内タイムラインで F6（複製キーフレーム）を押して2フレーム目を作成し、2フレーム目に帽子を描き足す
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

        // 4. シンボルから親シーン（シーン1）へ戻ると、シンボル内タイムライン（2フレーム）が保持され、
        //    親シーンのフレーム進行に合わせてループ評価されること
        assert!(tool_select::exit_current_group(&mut app));
        assert!(app.symbols.edit_stack.is_empty());
        assert_eq!(app.symbols.library[0].timeline.total_frames, 2);

        // シーン1の尺を3フレームに延長（F5 x 2）
        assert!(timeline_ui::timeline_extend_frame(&mut app));
        assert!(timeline_ui::timeline_extend_frame(&mut app));
        assert_eq!(app.timeline.total_frames, 3);

        // フレーム0（シンボル内 0F = 1シェイプ × 2インスタンス = 2シェイプ）
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        assert_eq!(app.canvas.shapes.len(), 2);

        // フレーム1（シンボル内 1F = 2シェイプ × 2インスタンス = 4シェイプ）
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
        assert_eq!(app.canvas.shapes.len(), 4);

        // フレーム2（ループしてシンボル内 0F = 1シェイプ × 2インスタンス = 2シェイプ）
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
        assert_eq!(app.canvas.shapes.len(), 2);
    }

    #[test]
    fn test_classic_tween_single_symbol_condition_and_interpolation() {
        use crate::symbol::TweenState;

        let mut app = Is2dApp::default();

        // 1. 空のキーフレームではトゥイーン (T) を設定できないこと
        assert!(!tool_transform::timeline_toggle_tween(&mut app));

        // 2. 生の絵（Raw Shape）だけのキーフレームではトゥイーンを設定できないこと
        app.canvas.shapes = make_stroke(
            Point::new(100.0, 150.0),
            Point::new(200.0, 150.0),
            16.0,
            Color32::from_rgb(230, 80, 80),
        );
        assert!(!tool_transform::timeline_toggle_tween(&mut app));

        // 3. 通常のグループ（Cmd+G）だけのキーフレームでもトゥイーンを設定できないこと
        app.canvas.selected_shapes = vec![0];
        assert!(tool_select::group_selected_shapes(&mut app).is_some());
        assert!(!tool_transform::timeline_toggle_tween(&mut app));
        assert!(tool_select::ungroup_selected_shapes(&mut app));

        // 4. F8 で単一シンボルに変換すると、トゥイーンを設定できること
        app.canvas.selected_shapes = vec![0];
        let sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
        assert!(tool_transform::timeline_toggle_tween(&mut app));
        assert!(app.timeline.layers[0].keyframes[0].tween);
        assert_eq!(
            app.timeline.layers[0].tween_state_at_keyframe(0),
            TweenState::Incomplete,
            "終了キーフレームがない状態では未完トゥイーン（横点線）になること"
        );

        // 5. もう1回 T を押すとトゥイーンを解除でき、さらに別のシンボルや生の絵を混在させると T が拒否されること
        assert!(tool_transform::timeline_toggle_tween(&mut app));
        assert!(!app.timeline.layers[0].keyframes[0].tween);

        // 2つ目のシンボルインスタンスを配置（複数シンボル混在）→ トゥイーン設定不可
        assert!(tool_transform::place_symbol_instance_from_library(&mut app, sym_id));
        assert!(!tool_transform::timeline_toggle_tween(&mut app));
        // 2つ目のインスタンスを削除して単一シンボルに戻す
        assert!(tool_select::delete_selected_shapes(&mut app));

        // 生の絵を1つ追加（シンボル＋生の絵混在）→ トゥイーン設定不可
        let raw_extra = make_stroke(
            Point::new(300.0, 300.0),
            Point::new(340.0, 300.0),
            8.0,
            Color32::BLACK,
        )
        .remove(0);
        app.canvas.shapes.push(raw_extra);
        assert!(!tool_transform::timeline_toggle_tween(&mut app));
        app.canvas.shapes.pop();

        // 6. 単一シンボルだけの状態で T を押し、F5 で 4 コマ延長（frame 0..=4）、frame 4 で F6 を押して終了キーフレームを作成
        assert!(tool_transform::timeline_toggle_tween(&mut app));
        for _ in 0..4 {
            assert!(timeline_ui::timeline_extend_frame(&mut app));
        }
        assert_eq!(app.timeline.total_frames, 5);
        assert_eq!(
            app.timeline.layers[0].tween_state_at_keyframe(0),
            TweenState::Incomplete
        );

        // frame 4 に移動して F6（複製キーフレーム作成）
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
        assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
        assert_eq!(app.timeline.current_frame, 4);
        assert!(
            matches!(
                app.timeline.layers[0].tween_state_at_keyframe(0),
                TweenState::Complete {
                    start_frame: 0,
                    end_frame: 4,
                    ..
                }
            ),
            "終了キーフレームに同一シンボルの単一インスタンスが存在すると完全トゥイーン（実線矢印）になること"
        );

        // 7. 終了キーフレーム（frame 4）でシンボルを X 方向に +200px、Y 方向に +80px 平行移動
        app.canvas.selected_shapes = vec![0];
        let snap = app.canvas.shapes.clone();
        app.canvas.translate_selected(KVec2::new(200.0, 80.0));
        tool_select::commit_moved_selection(&mut app, snap);
        canvas_tool::sync_shapes_to_timeline(&mut app);

        // 中間フレーム（frame 2, t = 0.5）へ切り替えると、中心座標が (150 + 100, 150 + 40) = (250, 190) に補間されていること
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
        let mid_bbox = app.canvas.shapes[0].bounding_box();
        let mid_cx = (mid_bbox.x0 + mid_bbox.x1) * 0.5;
        let mid_cy = (mid_bbox.y0 + mid_bbox.y1) * 0.5;
        assert!((mid_cx - 250.0).abs() < 1.0, "mid_cx = {mid_cx}");
        assert!((mid_cy - 190.0).abs() < 1.0, "mid_cy = {mid_cy}");

        // 中間フレームを表示したあとに先頭フレーム（frame 0）へ戻っても、先頭キーフレームの座標 (150, 150) が上書き破壊されていないこと
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        let start_bbox = app.canvas.shapes[0].bounding_box();
        let start_cx = (start_bbox.x0 + start_bbox.x1) * 0.5;
        let start_cy = (start_bbox.y0 + start_bbox.y1) * 0.5;
        assert!((start_cx - 150.0).abs() < 1.0, "start_cx = {start_cx}");
        assert!((start_cy - 150.0).abs() < 1.0, "start_cy = {start_cy}");

        // 8. シンボルインスタンスの変形中心（ピボット）を左端の関節位置 (100, 150) に設定し、
        //    選択解除・フレーム移動後も保持され、そのピボットを中心にクラシックトゥイーン回転補間されること
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
        app.canvas.selected_shapes = vec![0];
        // 一旦 frame 4 の位置も frame 0 と同じ (150, 150) に戻す
        let snap_reset = app.canvas.shapes.clone();
        app.canvas.translate_selected(KVec2::new(-200.0, -80.0));
        tool_select::commit_moved_selection(&mut app, snap_reset);
        canvas_tool::sync_shapes_to_timeline(&mut app);

        // frame 4 でピボットを左端 (100, 150) [ローカル座標 (-50, 0)] に移動して +90° 回転
        let joint_world = Point::new(100.0, 150.0);
        assert!(tool_transform::set_selected_symbol_world_pivot(&mut app, Some(joint_world), true));
        tool_select::clear_selection(&mut app);
        app.canvas.selected_shapes = vec![0];
        let bbox_f4 = app.canvas.selection_bounding_box(app.viewport.zoom).unwrap();
        assert!(
            (app.canvas.active_transform_pivot(bbox_f4) - joint_world).hypot() < 1e-4,
            "選択解除後もシンボルインスタンスのカスタムピボット座標が保持されること"
        );
        assert!(tool_transform::rotate_selected_shapes(&mut app, std::f64::consts::FRAC_PI_2));
        canvas_tool::sync_shapes_to_timeline(&mut app);

        // 中間フレーム（frame 2, t = 0.5 = +45° 回転）において、関節ピボット (100, 150) がぶれずに固定され、
        // 元の中心 (150, 150) [関節から距離 50px] が 45° 回転位置 (100 + 50*cos45°, 150 + 50*sin45°) を通過すること
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
        let mid_binding = app.canvas.shapes[0].symbol_instance.unwrap();
        assert!(
            (mid_binding.world_pivot() - joint_world).hypot() < 1e-4,
            "トゥイーン中間フレームでもピボット（関節位置）のワールド座標が固定されていること: {:?}",
            mid_binding.world_pivot()
        );
        let expected_mid_center = Point::new(
            100.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
            150.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        );
        assert!(
            app.canvas.shapes[0].contains_point(expected_mid_center),
            "中間フレーム (t=0.5) でピボット (100, 150) を軸とした +45° 回転位置にシェイプが存在すること"
        );

        // 9. タイムラインのキーフレーム選択時とキャンバス上のシンボル選択時で `inspect_timeline_frame` が切り替わり、
        //    トゥイーンの回転方向（時計回り CW / 反時計回り CCW）が中間フレームに反映されること
        timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
        assert!(
            app.ui.inspect_timeline_frame,
            "タイムラインのセルを選択したときはフレーム属性表示モード (inspect_timeline_frame = true) になること"
        );
        tool_select::select_single_shape_at(&mut app, Point::new(150.0, 150.0));
        assert!(
            !app.ui.inspect_timeline_frame,
            "キャンバス上のシンボルをクリックしたときはシンボル属性表示モード (inspect_timeline_frame = false) になること"
        );

        // 先頭キーフレームを選択して回転の向きを「反時計回り (CCW)」に変更すると、
        // 0° -> +90° (CW) の終了角度へ反時計回りに -270° 回転し、中間フレーム (t=0.5) では -135° 位置を通過すること
        timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
        assert!(tool_transform::set_active_keyframe_tween_rotate(&mut app, crate::symbol::TweenRotate::CounterClockwise));
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
        let expected_ccw_mid_center = Point::new(
            100.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
            150.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        );
        assert!(
            app.canvas.shapes[0].contains_point(expected_ccw_mid_center),
            "反時計回り (CCW) 指定時は中間フレーム (t=0.5) で -135° 回転位置を通過すること"
        );
    }

    #[test]
    fn test_symbol_playback_modes_loop_play_once_and_single_frame() {
        use crate::symbol::SymbolLoopMode;

        let mut app = Is2dApp::default();

        // 1. シンボルを作成し、内部を3フレーム構成にする（0F=赤、1F=緑、2F=青）
        let red = Color32::from_rgb(220, 60, 60);
        let green = Color32::from_rgb(60, 190, 90);
        let blue = Color32::from_rgb(60, 110, 230);
        app.canvas.shapes =
            make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 12.0, red);
        app.canvas.selected_shapes = vec![0];
        assert!(tool_transform::convert_selection_to_symbol(&mut app).is_some());

        let (binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
        assert!(tool_transform::enter_symbol_timeline(&mut app, binding));

        // シンボル内部 1F（空白キーフレーム → 緑シェイプ）
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

        // シンボル内部 2F（空白キーフレーム → 青シェイプ）
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

        // 親シーンへ戻る（5フレーム分の尺に延長: frame 0..=4）
        assert!(tool_transform::exit_symbol_timeline(&mut app));
        for _ in 0..4 {
            assert!(timeline_ui::timeline_extend_frame(&mut app));
        }
        assert_eq!(app.timeline.total_frames, 5);

        // 2. デフォルト（Loop, first_frame = 0）: 0F=赤, 1F=緑, 2F=青, 3F=赤, 4F=緑
        let expected_loop = [red, green, blue, red, green];
        for (f, &col) in expected_loop.iter().enumerate() {
            timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
            assert_eq!(app.canvas.shapes[0].color, col, "Loop mode at frame {f}");
        }

        // 3. 1回再生（PlayOnce, first_frame = 0）に変更: 0F=赤, 1F=緑, 2F=青, 3F=青, 4F=青
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        app.canvas.selected_shapes = vec![0];
        assert!(tool_transform::set_selected_symbol_playback(&mut app, SymbolLoopMode::PlayOnce, 0));
        let expected_once = [red, green, blue, blue, blue];
        for (f, &col) in expected_once.iter().enumerate() {
            timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
            assert_eq!(
                app.canvas.shapes[0].color, col,
                "PlayOnce mode at frame {f}"
            );
        }

        // 4. 任意のフレーム（SingleFrame, first_frame = 1 [UI上は2フレーム目=緑]）に変更: 全フレームで緑
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        app.canvas.selected_shapes = vec![0];
        assert!(tool_transform::set_selected_symbol_playback(&mut app, SymbolLoopMode::SingleFrame, 1));
        assert_eq!(app.canvas.shapes[0].color, green);
        assert_eq!(app.canvas.selected_shapes, vec![0]);
        for f in 0..5 {
            timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
            assert_eq!(
                app.canvas.shapes[0].color, green,
                "SingleFrame(1) at frame {f}"
            );
        }
    }

    #[test]
    fn test_nested_symbol_evaluation_and_library_delete_detach() {
        let mut app = Is2dApp::default();
        let red = Color32::from_rgb(220, 60, 60);
        let blue = Color32::from_rgb(60, 110, 230);

        // 1. 子シンボル（Symbol 1: 0F=赤, 1F=青 の2フレーム）を作成
        app.canvas.shapes =
            make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 12.0, red);
        app.canvas.selected_shapes = vec![0];
        let child_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
        let (child_binding, _, _) = tool_transform::selected_single_symbol_info(&app).unwrap();
        assert!(tool_transform::enter_symbol_timeline(&mut app, child_binding));
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

        // 2. 子シンボルインスタンスを選択したまま、さらに親シンボル（Symbol 2: 1フレーム）に変換（入れ子構造）
        app.canvas.selected_shapes = vec![0];
        let parent_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
        assert_ne!(child_sym_id, parent_sym_id);

        // 親シーンを2フレームに延長すると、1フレームの親シンボル越しに子シンボルの2フレームアニメが再帰評価されること
        assert!(timeline_ui::timeline_extend_frame(&mut app));
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        assert_eq!(app.canvas.shapes[0].color, red);
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
        assert_eq!(app.canvas.shapes[0].color, blue);

        // 2b. 子シンボルの3フレーム目に空白キーフレーム（F7）を追加した場合、
        //     親シーンの3フレーム目では編集時（canvas.shapes）も再生時（build_layer_resolved_world_batch_mesh）も
        //     空白（0シェイプ）として一貫評価され、かつ別フレームへ戻るとシンボルインスタンスが保持されていること
        assert!(tool_transform::enter_symbol_from_library(&mut app, child_sym_id));
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
        assert!(timeline_ui::timeline_insert_keyframe(&mut app, false)); // 2F目（index 2）に空白キーフレーム挿入
        assert_eq!(app.timeline.total_frames, 3);
        assert!(tool_transform::exit_symbol_timeline(&mut app));

        assert!(timeline_ui::timeline_extend_frame(&mut app)); // 親シーンも3フレームに延長
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
        assert_eq!(
            app.canvas.shapes.len(),
            1,
            "シンボル内の空白キーフレームは親シーンの編集キャンバスでも単一の空シンボルマーカーとして CanvasState::shapes 内に保持されること"
        );
        assert!(app.canvas.shapes[0].is_empty_symbol_marker());
        // 空白フレームのシンボルインスタンスも selected_shapes 経由で直接選択・バインディング取得できること
        canvas_tool::select_shapes_for_timeline_selection(&mut app);
        assert_eq!(app.canvas.selected_shapes, vec![0]);
        assert!(app.canvas.selected_single_symbol_binding().is_some());
        tool_select::clear_selection(&mut app);
        assert!(
            app.timeline
                .build_layer_resolved_world_batch_mesh(0, 2, &app.symbols.library)
                .is_none(),
            "再生時の解決済みメッシュでも空白キーフレームは None (描画なし) になること"
        );
        // 空白フレーム上で生の絵を追加描画しても、親キーフレームのシンボルインスタンス自体は消滅せず保持されること
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
        assert_eq!(
            app.canvas.shapes.len(),
            2,
            "フレーム0に戻るとシンボルインスタンスと追加した生シェイプの両方が存在すること"
        );
        // 生シェイプ追加を Undo して元の単一シンボル状態へ戻す
        app.canvas.shapes.retain(|s| s.symbol_instance.is_some());
        canvas_tool::sync_shapes_to_timeline(&mut app);

        // 2c. シンボル名の変更およびレイヤー表示/ロック切り替えが Undo 履歴に記録され、Undo で元に戻ること
        let orig_name = app.symbols.find_symbol(parent_sym_id).unwrap().name.clone();
        assert!(tool_transform::rename_symbol_in_library(&mut app, parent_sym_id, "HeroSymbol".to_string()));
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

        // 3. ライブラリから親シンボルを削除すると、ステージ上の図形は消滅せず通常グループへデタッチされて残ること
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        assert!(tool_transform::delete_symbol_from_library(&mut app, parent_sym_id));
        assert_eq!(
            app.canvas.shapes.len(),
            1,
            "シンボル削除時もステージ上の絵が消えないこと"
        );
        assert!(
            app.canvas.shapes[0].symbol_instance.is_none(),
            "シンボル紐付けが解除されていること"
        );
        assert!(
            !app.canvas.shapes[0].group_path.is_empty(),
            "通常のグループ図形として group_path が維持されていること"
        );

        // 4. 親シンボルのインプレイス編集中に内部図形を F8 で新規子シンボルへ変換した際、
        //    即座にライブラリ登録と親シンボル定義への同期が完了し、Undo/Redo でも整合すること
        assert!(tool_transform::enter_symbol_from_library(&mut app, child_sym_id));
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
        app.canvas.selected_shapes = vec![0];
        let lib_len_before = app.symbols.library.len();
        let grandchild_sym_id = tool_transform::convert_selection_to_symbol(&mut app).unwrap();
        assert_eq!(app.symbols.library.len(), lib_len_before + 1);
        assert!(
            !app.symbols
                .find_symbol(child_sym_id)
                .unwrap()
                .evaluated_local_shapes_at(0, &app.symbols.library)
                .is_empty(),
            "インプレイス編集中の F8 子シンボル変換直後に親シンボル定義の評価結果が空にならないこと"
        );
        app.undo();
        assert_eq!(
            app.symbols.library.len(),
            lib_len_before,
            "Undo で新規登録された子シンボルもライブラリから巻き戻されること"
        );
        app.redo();
        assert!(app.symbols.find_symbol(grandchild_sym_id).is_some());
        assert!(tool_transform::exit_symbol_timeline(&mut app));
    }

    #[test]
    fn test_crash_resilience_against_degenerate_inputs_and_rapid_state_transitions() {
        use crate::vector_engine::union_bounding_boxes;
        use kurbo::{Affine, Rect as KRect};

        let mut app = Is2dApp::default();
        let red = Color32::from_rgb(220, 60, 60);

        // 1. NaN / 無限大 / 空配列 / 長さ0ストロークを入力してもパニックせず安全に無視されること
        assert!(generate_brush_outline(&[], 8.0).is_empty());
        assert!(generate_brush_outline(&[Point::new(f64::NAN, 10.0)], 8.0).is_empty());
        assert!(generate_brush_outline(&[Point::new(10.0, 10.0)], f64::NAN).is_empty());
        assert!(build_smoothed_stroke_shapes(&[Point::new(f64::INFINITY, 0.0); 4], red).is_empty());
        assert_eq!(union_bounding_boxes(&[]), KRect::ZERO);

        // 長さ0の単一点スタンプは有効な円形シェイプになること
        let dot_outline = generate_brush_outline(&[Point::new(100.0, 100.0)], 6.0);
        let dot_shapes = build_smoothed_stroke_shapes(&dot_outline, red);
        assert_eq!(dot_shapes.len(), 1);

        // 2. 面積0（特異行列）アフィン変換や NaN 平行移動を与えても図形データが破壊されないこと
        let mut shape = dot_shapes[0].clone();
        let orig_bbox = shape.bounding_box();
        shape.translate(KVec2::new(f64::NAN, f64::INFINITY));
        shape.transform(Affine::scale(0.0));
        shape.transform(Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]));
        assert_eq!(shape.bounding_box(), orig_bbox);
        assert!(!shape.contains_point(Point::new(f64::NAN, f64::NAN)));

        // 3. スライス選択 → シンボル化 → シンボル出入り → Undo/Redo → 範囲外レイヤー操作の連続負荷でもパニックしないこと
        app.canvas.shapes =
            make_stroke(Point::new(50.0, 100.0), Point::new(200.0, 100.0), 14.0, red);
        canvas_tool::sync_shapes_to_timeline(&mut app);
        tool_select::apply_marquee_selection(&mut app, Point::new(80.0, 70.0), Point::new(130.0, 130.0), false);
        assert!(tool_transform::convert_selection_to_symbol(&mut app).is_some());
        if let Some((b, _, _)) = tool_transform::selected_single_symbol_info(&app) {
            assert!(tool_transform::enter_symbol_timeline(&mut app, b));
            assert!(tool_transform::exit_symbol_timeline(&mut app));
        }
        assert!(!tool_transform::exit_symbol_timeline(&mut app));
        assert!(!tool_select::exit_current_group(&mut app));
        app.undo();
        app.redo();
        timeline_ui::set_layer_visible(&mut app, 999, false);
        timeline_ui::set_layer_locked(&mut app, 999, true);
        assert!(!timeline_ui::timeline_move_or_copy_frames(&mut app, 
            crate::timeline::TimelineCellSelection::single(999, 999),
            1,
            1,
            false
        ));

        // 4. ドラッグ途中の一時状態（描きかけストローク・変形プレビュー）が排他ステートマシン `ActiveCanvasDrag` で安全に管理されること
        let clean_shapes = app.canvas.shapes.clone();
        let clean_bbox = app.canvas.shapes[0].bounding_box();
        app.canvas.drag = crate::canvas_tool::ActiveCanvasDrag::Select(
            crate::canvas_tool::SelectDragState::MoveShape(
                crate::canvas_tool::MoveSelectionDrag::new(Point::ZERO, clean_shapes),
            ),
        );
        app.canvas.shapes[0].translate(KVec2::new(500.0, 500.0));
        // clear_selection は未確定の形状ドラッグ（Select / Transform）のみをロールバックする
        tool_select::clear_selection(&mut app);
        assert_eq!(app.canvas.shapes[0].bounding_box(), clean_bbox);

        // PaintStroke 状態は clear_selection() に干渉されず、set_tool() でのみ消去されること
        let stroke = vec![Point::new(10.0, 10.0), Point::new(40.0, 10.0)];
        let tail_outline = generate_brush_outline(&stroke, 8.0);
        let tail_mesh =
            crate::vector_engine::tessellate_raw_polygon(&tail_outline, Color32::BLACK);
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

        // 5. シンボルインプレース編集の無劣化キャッシュ（未編集シェイプは回転・スケール下でも何度も出入りして完全一致・Arc共有を維持）
        if let Some(b) = app.canvas.shapes.iter().find_map(|s| s.symbol_instance) {
            let sym_id = b.symbol_id;
            let orig_local_shape = app
                .symbols
                .library
                .iter()
                .find(|s| s.id == sym_id)
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
                .library
                .iter()
                .find(|s| s.id == sym_id)
                .unwrap()
                .timeline
                .layers[0]
                .keyframes[0]
                .cel
                .shapes[0];
            assert!(after_local_shape.has_same_contour_arcs(
                &orig_local_shape.exterior,
                &orig_local_shape.holes
            ));
            assert_eq!(after_local_shape.mesh_revision, orig_rev);
        }

        // 6. Timeline::resize_total_frames の安全性検証（短いレイヤーの保護と孤児キーフレームの整理）
        let mut tl = Timeline {
            total_frames: 10,
            ..Default::default()
        };
        tl.layers[0].frame_count = 10;
        tl.layers[0].insert_keyframe(8, false);
        let short_layer =
            crate::timeline::Layer::new_with_id_and_frames(2, "Short Layer", 4);
        tl.layers.push(short_layer);
        // 10f -> 20f に延長した際、10f だったレイヤーは 20f に伸び、4f だった短いレイヤーは 4f のまま保護されること
        tl.resize_total_frames(20);
        assert_eq!(tl.layers[0].frame_count, 20);
        assert_eq!(tl.layers[1].frame_count, 4);
        // 20f -> 6f に短縮した際、frame 8 のキーフレームが孤児として残らず整理されること
        tl.resize_total_frames(6);
        assert_eq!(tl.layers[0].frame_count, 6);
        assert!(tl.layers[0].keyframes.iter().all(|kf| kf.start_frame < 6));

        // 7. FillShape の遅延メッシュ生成（OnceLock）と拡大アフィン変換時の再テッセレーション検証
        let small_raw = generate_brush_outline(&[Point::new(100.0, 100.0)], 4.0);
        let mut small_circle = build_smoothed_stroke_shapes(&small_raw, Color32::WHITE)
            .into_iter()
            .next()
            .unwrap();
        assert!(
            !small_circle.has_cached_fill_mesh(),
            "生成直後の FillShape は描画要求があるまで egui::Mesh をテッセレーションしないこと"
        );
        let verts_before = small_circle.fill_mesh().vertices.len();
        assert!(small_circle.has_cached_fill_mesh());
        small_circle.transform(kurbo::Affine::scale_about(10.0, Point::new(100.0, 100.0)));
        assert!(
            !small_circle.has_cached_fill_mesh(),
            "大幅な拡大時にはキャッシュが無効化され、次回 fill_mesh() 呼び出しまで遅延されること"
        );
        assert!(
            small_circle.fill_mesh().vertices.len() > verts_before,
            "大幅な拡大アフィン変換時には曲線が再テッセレーションされてメッシュ頂点数が増えること ({} -> {})",
            verts_before,
            small_circle.fill_mesh().vertices.len()
        );

        // 8. Cel::instances によるシンボルインスタンス正規化（複数構成シェイプの1エンティティ集約＆全フレーム白紙化後の復活）
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
        assert_eq!(
            sym_app.canvas.shapes.len(),
            2,
            "キャンバス上には展開された2つの構成シェイプが存在すること"
        );
        assert!(
            sym_app.timeline.layers[0].keyframes[0].cel.shapes.is_empty(),
            "正規化された Cel::shapes にはシンボル図形を重複保持しないこと"
        );
        assert_eq!(
            sym_app.timeline.layers[0].keyframes[0].cel.instances.len(),
            1,
            "Cel::instances には構成シェイプ数に関わらず正規化された1つの SymbolInstance のみが保持されること"
        );
        assert!(tool_transform::enter_symbol_from_library(&mut sym_app, empty_test_sym_id));
        assert!(tool_select::clear_active_frame_shapes(&mut sym_app));
        assert!(tool_transform::exit_symbol_timeline(&mut sym_app));
        // シンボルが空の間は Cel::shapes は0個かつ Cel::instances にエンティティが保持され、作業キャンバス CanvasState::shapes には空シンボルマーカーとして一元保持されること
        assert_eq!(sym_app.canvas.shapes.len(), 1);
        assert!(sym_app.canvas.shapes[0].is_empty_symbol_marker());
        assert!(sym_app.timeline.layers[0].keyframes[0].cel.shapes.is_empty());
        assert_eq!(
            sym_app.timeline.layers[0].keyframes[0].cel.instances.len(),
            1
        );
        assert!(
            sym_app.timeline.layers[0].keyframes[0]
                .cel
                .single_symbol_instance()
                .is_some()
        );
        // 全フレームが白紙状態のシンボルでも、ライブラリからステージ中央へ新規インスタンスを配置できること（提案2の検証）
        assert!(
            tool_transform::place_symbol_instance_from_library(&mut sym_app, empty_test_sym_id),
            "全フレームが白紙の空シンボルでもライブラリからステージへ配置できること"
        );
        assert_eq!(sym_app.canvas.shapes.len(), 2);
        assert!(sym_app.canvas.shapes[1].is_empty_symbol_marker());
        sym_app.undo();
        assert_eq!(sym_app.canvas.shapes.len(), 1);
        assert!(tool_transform::enter_symbol_from_library(&mut sym_app, empty_test_sym_id));
        sym_app.canvas.shapes = make_stroke(
            Point::new(210.0, 200.0),
            Point::new(250.0, 200.0),
            8.0,
            Color32::from_rgb(60, 180, 90),
        );
        canvas_tool::sync_shapes_to_timeline(&mut sym_app);
        assert!(tool_transform::exit_symbol_timeline(&mut sym_app));
        assert_eq!(
            sym_app.canvas.shapes.len(),
            1,
            "一度空にしたシンボルに再度絵を描くと、親ステージ上のインスタンスが元の位置に復活すること"
        );
        assert!(sym_app.canvas.shapes[0].contains_point(Point::new(230.0, 200.0)));

        // 9. ステージサイズ (stage_size) の変更が Undo / Redo で復元され、かつ UndoHistory 内の図形からは一時メッシュキャッシュが除去されていること
        let _ = sym_app.symbols.library[0].timeline.layers[0].keyframes[0].cel.shapes[0].fill_mesh();
        assert!(
            sym_app.symbols.library[0].timeline.layers[0].keyframes[0]
                .cel
                .shapes[0]
                .has_cached_fill_mesh()
        );
        let orig_stage = sym_app.viewport.stage_size;
        sym_app.push_undo();
        let last_undo = sym_app.history.undo_stack.back().unwrap();
        assert!(
            !last_undo.library[0].timeline.layers[0].keyframes[0]
                .cel
                .shapes[0]
                .has_cached_fill_mesh(),
            "UndoEntry 保存時には FillShape の egui::Mesh キャッシュが除去されていること"
        );
        sym_app.viewport.stage_size = Vec2::new(1920.0, 1080.0);
        sym_app.undo();
        assert_eq!(sym_app.viewport.stage_size, orig_stage);
        sym_app.redo();
        assert_eq!(sym_app.viewport.stage_size, Vec2::new(1920.0, 1080.0));

        // 9.5 選択ドット柄スクリーンメッシュキャッシュおよびライブラリミニプレビューキャッシュの O(1) 共有検証
        let sel_m1 = sym_app.canvas.shapes[0]
            .cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
        let sel_m2 = sym_app.canvas.shapes[0]
            .cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
        assert!(
            std::sync::Arc::ptr_eq(&sel_m1, &sel_m2),
            "同一の (zoom, pan) では選択ドット柄スクリーンメッシュの Arc が再変換なしで O(1) 共有されること"
        );
        let preview_rect = eframe::egui::Rect::from_min_max(
            Pos2::new(0.0, 0.0),
            Pos2::new(200.0, 110.0),
        );
        let (prev_m1, orig1) = sym_app.symbols.library[0]
            .cached_mini_preview(preview_rect, sym_app.symbols.revision, &sym_app.symbols.library)
            .unwrap();
        let (prev_m2, orig2) = sym_app.symbols.library[0]
            .cached_mini_preview(preview_rect, sym_app.symbols.revision, &sym_app.symbols.library)
            .unwrap();
        assert!(
            std::sync::Arc::ptr_eq(&prev_m1, &prev_m2) && orig1 == orig2,
            "ライブラリミニプレビューは同一矩形・同一リビジョンで Arc<Mesh> が O(1) キャッシュ共有されること"
        );

        // 10. 逆順ネスト（Symbol A(id小) -> Symbol B -> Symbol C(id大)）のトポロジカル更新＆拡大シンボルのメッシュキャッシュ保持
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
        // ルートから sym_b のインスタンスを消去（ライブラリには残る）
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

        // sym_a の中に sym_b を配置し、sym_b の中に sym_c を配置する（逆順3階層ネスト）
        assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_a));
        assert!(tool_select::clear_active_frame_shapes(&mut nest_app));
        assert!(tool_transform::place_symbol_instance_from_library(&mut nest_app, sym_b));
        assert!(tool_transform::exit_symbol_timeline(&mut nest_app));

        assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_b));
        assert!(tool_select::clear_active_frame_shapes(&mut nest_app));
        assert!(tool_transform::place_symbol_instance_from_library(&mut nest_app, sym_c));
        assert!(tool_transform::exit_symbol_timeline(&mut nest_app));

        // 最深部の sym_c を編集して色を緑に変更 → 1回の exit で sym_b -> sym_a -> ルートシーンまで一括反映されること
        let updated_green = Color32::from_rgb(20, 210, 90);
        assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_c));
        for s in &mut nest_app.canvas.shapes {
            s.set_color(updated_green);
        }
        canvas_tool::sync_shapes_to_timeline(&mut nest_app);
        assert!(tool_transform::exit_symbol_timeline(&mut nest_app));
        assert_eq!(nest_app.canvas.shapes.len(), 1);
        assert_eq!(
            nest_app.canvas.shapes[0].color, updated_green,
            "逆順3階層ネスト（A -> B -> C）でも C の変更がトポロジカル順で B・A・ルートへ即座に伝播すること"
        );
        // 3倍拡大のシンボルインスタンス展開でも OnceLock メッシュキャッシュが破棄されず引き継がれること
        let scaled_binding = crate::symbol::SymbolInstanceBinding::new(
            999,
            sym_c,
            kurbo::Affine::scale(3.0),
        );
        let sym_c_def = nest_app.symbols.find_symbol(sym_c).unwrap();
        let inst_shapes = scaled_binding.instantiate_shapes(
            &sym_c_def.timeline.layers[0].keyframes[0].cel.shapes,
            1,
            &[],
        );
        assert!(
            inst_shapes[0].has_cached_fill_mesh(),
            "拡大シンボルインスタンス展開時にもローカルメッシュキャッシュが頂点変換で維持され再テッセレーションが不要であること"
        );

        // 11. ロックされたレイヤー上のクラシックトゥイーン中間フレームに対するスポイト色抽出
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
        // レイヤーをロックして中間フレーム（frame = 2, x = 200.0）へ移動
        timeline_ui::set_layer_locked(&mut tween_eye_app, 0, true);
        timeline_ui::scrub_ruler_to_frame(&mut tween_eye_app, 2);
        assert_eq!(
            tool_paint::topmost_visible_color_at(&tween_eye_app, Point::new(200.0, 200.0)),
            Some(tween_color),
            "ロック中レイヤーのトゥイーン中間フレームでも補間後の現在座標からスポイトで色を拾えること"
        );
        assert_eq!(
            tool_paint::topmost_visible_color_at(&tween_eye_app, Point::new(100.0, 200.0)),
            None,
            "ロック中レイヤーのトゥイーン中間フレームで開始キーフレームの古い位置からは色を拾わないこと"
        );

        // 12. 空白フレーム状態のシンボルを含む親グループ（Cmd+G）の移動・削除同期、および Undo 履歴の Arc 構造共有（CoW）
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
        assert!(tool_transform::enter_symbol_timeline(&mut grp_sym_app, blink_binding));
        assert!(timeline_ui::timeline_insert_keyframe(&mut grp_sym_app, false));
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
        assert!(tool_select::nudge_selected_shapes(&mut grp_sym_app, kurbo::Vec2::new(150.0, 0.0)));

        // F2（シンボルがループして再び表示されるフレーム）へ進むと、空白フレーム中に移動した親グループに追従してシンボルも (+150, 0) 移動していること
        timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
        assert_eq!(grp_sym_app.canvas.shapes.len(), 2);
        let moved_sym_shape = grp_sym_app
            .canvas
            .shapes
            .iter()
            .find(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == blink_sym_id))
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
        // 兄弟シェイプの中心は (260, 160)。ここをピボットに +90° 回転すると、
        // 相対位置 (0, -60) にあったシンボル中心 (260, 100) は (260 + 60, 160) = (320, 160) へ移動する
        assert!(tool_transform::rotate_selected_shapes(
            &mut grp_sym_app,
            std::f64::consts::FRAC_PI_2
        ));
        timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
        let rotated_sym_shape = grp_sym_app
            .canvas
            .shapes
            .iter()
            .find(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == blink_sym_id))
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

        // 13. 指摘1〜3・5の検証：
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
        let raw_div = generate_brush_outline(
            &[Point::new(200.0, 120.0), Point::new(200.0, 280.0)],
            6.0,
        );
        tool_paint::commit_raw_outline(&mut bucket_app, &raw_div);
        timeline_ui::set_layer_locked(&mut bucket_app, 1, true);
        timeline_ui::switch_to_layer_and_frame(&mut bucket_app, 0, 0);

        // アクティブレイヤー（Layer 1）の左半分 (170, 200) を赤でバケツ塗り -> 左半分のみが塗られ、右半分 (230, 200) は塗られないこと
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

        // (b) シンボル入れ子の間接的な循環参照（A -> B -> A）がブロックされること
        assert!(tool_transform::enter_symbol_from_library(&mut nest_app, sym_b));
        assert!(
            !nest_app.symbols.can_place_symbol_in_current_context(sym_a),
            "sym_a -> sym_b の親子関係があるとき、sym_b の中に sym_a を配置する循環参照が禁止されること"
        );
        assert!(!tool_transform::place_symbol_instance_from_library(
            &mut nest_app,
            sym_a
        ));
        assert!(tool_transform::exit_symbol_timeline(&mut nest_app));

        // (c) 1コマ目（Frame 0）が空白キーフレームで2コマ目（Frame 1）にのみ絵があるマルチフレームシンボルをライブラリから配置できること
        assert!(tool_transform::enter_symbol_from_library(&mut grp_sym_app, blink_sym_id));
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
        assert!(
            tool_transform::place_symbol_instance_from_library(&mut grp_sym_app, blink_sym_id),
            "Frame 0 が空白のマルチフレームシンボルでもライブラリからステージへ配置できること"
        );
        assert_eq!(
            grp_sym_app.timeline.layers[0].keyframes[0].cel.instances.len(),
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

        // (d) シンボルを含むセルのスクリーンメッシュおよびオニオンスキンメッシュのキャッシュ再利用（Arcポインタ共有）検証
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
        // セルが実際に参照しているシンボルの revision が更新された場合にのみキャッシュが再評価されること（提案1の検証）
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

        // (e) 指摘4の検証：オニオンスキンやループ再生などのビュー設定（TimelineUiState）を変更した後に Undo / Redo を実行しても巻き戻らないこと
        grp_sym_app.timeline_ui.onion_skin_enabled = true;
        grp_sym_app.timeline_ui.onion_prev_frames = 5;
        grp_sym_app.timeline_ui.onion_next_frames = 4;
        grp_sym_app.timeline_ui.loop_playback = false;
        grp_sym_app.timeline.fps = 30;
        grp_sym_app.undo();
        assert!(grp_sym_app.timeline_ui.onion_skin_enabled);
        assert_eq!(grp_sym_app.timeline_ui.onion_prev_frames, 5);
        assert_eq!(grp_sym_app.timeline_ui.onion_next_frames, 4);
        assert!(!grp_sym_app.timeline_ui.loop_playback);
        grp_sym_app.redo();
        assert!(grp_sym_app.timeline_ui.onion_skin_enabled);
        assert_eq!(grp_sym_app.timeline_ui.onion_prev_frames, 5);
        assert_eq!(grp_sym_app.timeline_ui.onion_next_frames, 4);
        assert!(!grp_sym_app.timeline_ui.loop_playback);

        // (f) 指摘4の検証：1600%（zoom = 16.0）拡大時の極小シェイプ同士のブール演算（Union / Difference）で消失しないこと
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

        // (g) 指摘3の検証：大量の無関係な遠方シェイプが存在する場合も AABB 連結クラスタ事前カリングにより
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
            "どの連結AABBクラスタにも含まれない空白座標はオーバーレイ構築前に O(N) で None となること"
        );
        let enclosed_right = crate::vector_boolean::find_enclosed_region_at(
            &many_walls,
            Point::new(230.0, 200.0),
            Color32::GREEN,
        )
        .expect("無関係な遠方シェイプが多数存在しても対象クラスタ内の閉領域が正確に抽出されること");
        assert!(enclosed_right.contains_point(Point::new(230.0, 200.0)));
        assert!(!enclosed_right.contains_point(Point::new(170.0, 200.0)));

        // (h) 提案2・提案4の検証：`FillShape::shape_id` によるレイヤー順ソート後の選択状態追従と `LoadedShapeSig` の Copy 署名一致
        let mut sort_app = Is2dApp::default();
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
        // 逆順（上層レイヤーの図形が index 0、下層レイヤーの図形が index 1）で配置し、index 0 を選択
        sort_app.canvas.shapes = vec![shape_top, shape_bottom];
        sort_app.canvas.selected_shapes = vec![0];
        sort_app.canvas.sort_shapes_by_layer_order(&sort_app.timeline);
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

        // (i) 空シンボルマーカーのデータモデル不変条件（contours=0, geometry_bounding_box=None, スナップ非対象）と
        //     StageToLocalCache による未変更キーフレーム Cel の Arc CoW 共有維持の検証
        let empty_marker = FillShape::new_empty_symbol_marker(
            crate::symbol::SymbolInstanceBinding::new(
                777,
                blink_sym_id,
                Affine::translate((50.0, 50.0)),
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

        // シンボルインプレイス編集時に Frame 0 を編集しても未変更の Frame 1 の Cel::shapes は Arc ポインタ共有が維持されること
        let f1_local_arc_before = std::sync::Arc::clone(
            &grp_sym_app.symbols.find_symbol(blink_sym_id).unwrap().timeline.layers[0].keyframes[1]
                .cel
                .shapes,
        );
        let inst_b = grp_sym_app.timeline.layers[0].keyframes[0].cel.instances[0].binding;
        assert!(tool_transform::enter_symbol_timeline(&mut grp_sym_app, inst_b));
        // 背景セル（backdrop_cel）のスクリーンメッシュキャッシュが Arc::ptr_eq で O(1) 再利用されること
        let session = grp_sym_app.symbols.edit_stack.last().unwrap();
        let bd_m1 = session
            .backdrop_cel
            .cached_backdrop_screen_mesh(0, 1.0, Vec2::ZERO, &grp_sym_app.symbols.library);
        let bd_m2 = session
            .backdrop_cel
            .cached_backdrop_screen_mesh(0, 1.0, Vec2::ZERO, &grp_sym_app.symbols.library);
        if let (Some(a), Some(b)) = (bd_m1, bd_m2) {
            assert!(std::sync::Arc::ptr_eq(&a, &b));
        }
        // Frame 0 に新しいシェイプを追加し、途中で push_undo()（strip_transient_caches 実行）を挟んで同期・終了しても
        // StageToLocalCache の論理ID（Cel::revision / Contour::contour_id）により未変更 Frame 1 のローカル Cel::shapes が O(1) 維持されること
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

        // (j) 8フレーム超（12フレーム）のクラシックトゥイーン再生を2周ループした際、64エントリLRUキャッシュにより2周目もスラッシングせず同一 Arc<Mesh> が返ること
        let mut lru_tween_app = Is2dApp::default();
        lru_tween_app.canvas.shapes = make_stroke(
            Point::new(80.0, 200.0),
            Point::new(110.0, 200.0),
            10.0,
            Color32::from_rgb(220, 80, 40),
        );
        lru_tween_app.canvas.selected_shapes = vec![0];
        tool_transform::convert_selection_to_symbol(&mut lru_tween_app).unwrap();
        // シンボルインスタンスを移動確定してもキャッシュ済み fill_mesh が破棄されないこと
        let _ = lru_tween_app.canvas.shapes[0].fill_mesh();
        assert!(tool_select::nudge_selected_shapes(
            &mut lru_tween_app,
            kurbo::Vec2::new(10.0, 0.0)
        ));
        assert!(
            lru_tween_app.canvas.shapes[0].has_cached_fill_mesh(),
            "シンボルインスタンスの移動確定（commit_moved_selection）時にキャッシュ済み fill_mesh が無駄に破棄されないこと"
        );
        tool_transform::timeline_toggle_tween(&mut lru_tween_app);
        timeline_ui::select_timeline_cells(&mut lru_tween_app, 0, 11, 0, 11);
        timeline_ui::timeline_insert_keyframe(&mut lru_tween_app, true);
        tool_select::nudge_selected_shapes(&mut lru_tween_app, kurbo::Vec2::new(240.0, 0.0));
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
                .resolved_cel_screen_mesh(
                    0,
                    f,
                    1.0,
                    Vec2::ZERO,
                    false,
                    &lru_tween_app.symbols.library,
                )
                .unwrap();
            assert!(
                std::sync::Arc::ptr_eq(first_m, &second_m),
                "12フレームのトゥイーンループ再生2周目（frame={f}）でもスラッシングを起こさず同一の Arc<Mesh> が O(1) キャッシュヒットすること"
            );
        }
    }
