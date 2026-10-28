    use super::*;
    use kurbo::{ParamCurve, ParamCurveDeriv, ParamCurveNearest, QuadBez, Vec2 as KVec2};
    use vector_engine::{
        build_smoothed_stroke_shapes, build_smoothed_stroke_shapes_at_zoom, generate_brush_outline,
        generate_brush_outline_at_zoom, left_normal, merge_shapes_into_canvas, normalize_vec,
        resolve_deformed_shape,
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
        FlashBrushApp::apply_pull_edge_to_shape(
            &mut circle,
            0,
            seg_idx,
            grab_t,
            orig_seg,
            Point::new(200.0, 275.0),
            true,
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
        FlashBrushApp::apply_pull_edge_to_shape(
            &mut circle_on_edge,
            0,
            seg_idx,
            grab_t,
            orig_seg,
            Point::new(200.0, 249.0),
            true,
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
        let mut app = FlashBrushApp::default();
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
        app.brush_color = black;
        app.commit_raw_outline(&raw_ring);
        assert_eq!(app.shapes.len(), 1);
        assert!(
            !app.shapes[0].contains_point(center),
            "初期状態では中央は空白（穴）であること"
        );

        // 1. 閉じきっていない外側の背景 (20, 20) をバケツでクリックしても塗られないこと
        app.brush_color = red;
        assert!(
            !app.bucket_fill_at(Point::new(20.0, 20.0)),
            "開いた背景は塗りつぶされないこと"
        );

        // 2. 円環の内側の空白 (200, 200) を赤でバケツ塗り -> 赤い塗りシェイプが生成されること
        assert!(app.bucket_fill_at(center), "閉領域がバケツで塗られること");
        assert_eq!(app.shapes.len(), 2);
        let inner_shape = app.shapes.iter().find(|s| s.color == red).unwrap();
        assert!(inner_shape.contains_point(center));

        // 3. 塗られた赤い領域を青でバケツクリック -> 青にリカラーされること
        app.brush_color = blue;
        assert!(
            app.bucket_fill_at(center),
            "既存の塗りシェイプが別色に塗り替わること"
        );
        assert_eq!(app.shapes.len(), 2);
        assert!(app
            .shapes
            .iter()
            .any(|s| s.color == blue && s.contains_point(center)));

        // 4. 内側の領域を周囲の線と同じ黒でバケツクリック -> 外周の黒リングと Union されて穴なしの1つの黒シェイプになること
        app.brush_color = black;
        assert!(app.bucket_fill_at(center));
        assert_eq!(
            app.shapes.len(),
            1,
            "同色でバケツ塗りすると周囲のシェイプと1つに結合されること"
        );
        assert!(app.shapes[0].holes.is_empty(), "穴が埋まって消えていること");
        assert!(app.shapes[0].contains_point(center));
    }

    #[test]
    fn test_marquee_select_partial_slice_and_shift_multi_select() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        app.brush_color = red;

        // x: 100..300, y: 200 の太い横長ストローク（半径 25px）を描画
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red);
        merge_shapes_into_canvas(&mut app.shapes, stroke);
        assert_eq!(app.shapes.len(), 1);

        // 1. 左端 (x: 70..140, y: 150..250) を矩形選択 -> 左端だけスライスされて選択状態になること
        app.apply_marquee_selection(Point::new(70.0, 150.0), Point::new(140.0, 250.0), false);
        assert_eq!(app.shapes.len(), 2, "部分矩形選択で内外2つにスライスされること");
        assert_eq!(app.selected_shapes.len(), 1, "スライスされた内側のみが選択されていること");
        assert!(
            app.shapes[app.selected_shapes[0]].contains_point(Point::new(110.0, 200.0)),
            "選択された断片が左端領域を含んでいること"
        );

        // 2. 移動も削除もせずに選択解除（clear_selection）すると、無劣化で元の1つのシェイプに戻ること
        app.clear_selection();
        assert_eq!(
            app.shapes.len(),
            1,
            "未編集で選択解除すると元の1つのシェイプに復元されること"
        );
        assert!(app.selected_shapes.is_empty());

        // 3. 左端 (x: 70..140) を矩形選択し、続いて Shift キー押下（additive = true）で右端 (x: 260..330) も連続矩形選択
        app.apply_marquee_selection(Point::new(70.0, 150.0), Point::new(140.0, 250.0), false);
        app.apply_marquee_selection(Point::new(260.0, 150.0), Point::new(330.0, 250.0), true);
        assert_eq!(
            app.shapes.len(),
            3,
            "左右両端のスライスと中央の非選択領域の計3つに分割されること"
        );
        assert_eq!(
            app.selected_shapes.len(),
            2,
            "Shift 連続矩形選択で左右2つの領域が同時に選択されていること"
        );

        // 4. 選択された左右両端を削除すると、中央 (x: 200, y: 200) のみが残ること
        assert!(app.delete_selected_shapes());
        assert_eq!(
            app.shapes.len(),
            1,
            "選択された左右両端が削除され中央の1つだけ残ること"
        );
        assert!(app.shapes[0].contains_point(Point::new(200.0, 200.0)));
        assert!(!app.shapes[0].contains_point(Point::new(110.0, 200.0)));
        assert!(!app.shapes[0].contains_point(Point::new(290.0, 200.0)));

        // 5. 1回の Undo でスライス前の元の1つのシェイプに完全復帰すること
        app.undo();
        assert_eq!(app.shapes.len(), 1);
        assert!(app.shapes[0].contains_point(Point::new(110.0, 200.0)));
        assert!(app.shapes[0].contains_point(Point::new(200.0, 200.0)));
        assert!(app.shapes[0].contains_point(Point::new(290.0, 200.0)));
    }

    #[test]
    fn test_keep_selection_on_tool_switch_and_bucket_fill_selected_region_only() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        let blue = Color32::from_rgb(55, 135, 245);
        app.brush_color = red;

        // x: 100..300, y: 200 の赤いストロークを描画
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red);
        merge_shapes_into_canvas(&mut app.shapes, stroke);
        assert_eq!(app.shapes.len(), 1);

        // 1. 選択ツールで左半分 (x: 70..200) を矩形選択
        app.set_tool(Tool::Select);
        app.apply_marquee_selection(Point::new(70.0, 150.0), Point::new(200.0, 250.0), false);
        assert_eq!(app.selected_shapes.len(), 1);

        // 2. バケツツールに切り替えても選択状態が解除されないこと
        app.set_tool(Tool::Bucket);
        assert_eq!(
            app.selected_shapes.len(),
            1,
            "バケツツールに切り替えても選択が維持されること"
        );

        // 3. 選択されていない右半分 (260, 200) をバケツでクリックしても塗られないこと
        app.brush_color = blue;
        assert!(
            !app.bucket_fill_at(Point::new(260.0, 200.0)),
            "選択外領域はバケツで塗られないこと"
        );

        // 4. 選択されている左半分 (130, 200) をバケツでクリックすると、選択部分だけが青に塗られること
        assert!(
            app.bucket_fill_at(Point::new(130.0, 200.0)),
            "選択領域内をクリックするとバケツで色が流し込まれること"
        );
        assert!(
            app.shapes
                .iter()
                .any(|s| s.color == blue && s.contains_point(Point::new(130.0, 200.0))),
            "選択された左半分が青になっていること"
        );
        assert!(
            app.shapes
                .iter()
                .any(|s| s.color == red && s.contains_point(Point::new(260.0, 200.0))),
            "非選択の右半分は赤のままであること"
        );
    }

    #[test]
    fn test_free_transform_rotate_scale_and_flip() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        app.brush_color = red;

        // 中心 (200, 200) の横長シェイプ (x: 100..300, y: 200, 半径 20px) を描画
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 20.0, red);
        merge_shapes_into_canvas(&mut app.shapes, stroke);
        assert_eq!(app.shapes.len(), 1);
        app.selected_shapes = vec![0];
        app.set_tool(Tool::Transform);

        // 1. +90° 回転 -> 横長だったシェイプが中心 (200, 200) を軸に縦長 (x: 200, y: 100..300) になること
        assert!(app.rotate_selected_shapes(std::f64::consts::FRAC_PI_2));
        assert!(
            app.shapes[0].contains_point(Point::new(200.0, 110.0))
                && app.shapes[0].contains_point(Point::new(200.0, 290.0)),
            "+90度回転で縦長シェイプになること"
        );
        assert!(
            !app.shapes[0].contains_point(Point::new(110.0, 200.0)),
            "元の左右端にはシェイプが存在しないこと"
        );

        // 2. 縮小 (0.5倍) -> 上下端 (200, 110) は範囲外になり、(200, 155) 付近に縮小されること
        assert!(app.scale_selected_shapes(0.5, 0.5));
        assert!(!app.shapes[0].contains_point(Point::new(200.0, 110.0)));
        assert!(app.shapes[0].contains_point(Point::new(200.0, 155.0)));

        // 3. カスタムピボット点 (200, 200) に対して、左側の点 (150, 200) にあるシェイプを左右反転 -> (250, 200) に移り、ワインディングも正常に保たれること
        app.clear_selection();
        app.shapes.clear();
        let left_piece =
            make_stroke(Point::new(130.0, 200.0), Point::new(170.0, 200.0), 18.0, red);
        merge_shapes_into_canvas(&mut app.shapes, left_piece);
        app.selected_shapes = vec![0];
        app.transform_pivot = Some(Point::new(200.0, 200.0));

        assert!(app.flip_selected_shapes(true, false));
        assert!(
            !app.shapes[0].contains_point(Point::new(150.0, 200.0)),
            "反転前の位置には存在しないこと"
        );
        assert!(
            app.shapes[0].contains_point(Point::new(250.0, 200.0)),
            "ピボット (200, 200) を基準に左右対称の (250, 200) へ反転移動していること"
        );
        assert!(
            app.shapes[0].exterior.signed_area() > 0.0,
            "反転時にベジェ輪郭の向き（ワインディング）が自動補正され正の面積を保つこと"
        );
    }

    #[test]
    fn test_eyedropper_picks_topmost_shape_color() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        let blue = Color32::from_rgb(55, 135, 245);
        let green = Color32::from_rgb(65, 195, 105);

        // 左側に赤、右側に青のシェイプを描画
        let s_red = make_stroke(Point::new(100.0, 200.0), Point::new(180.0, 200.0), 20.0, red);
        let s_blue = make_stroke(Point::new(240.0, 200.0), Point::new(320.0, 200.0), 20.0, blue);
        merge_shapes_into_canvas(&mut app.shapes, s_red);
        merge_shapes_into_canvas(&mut app.shapes, s_blue);

        app.brush_color = green;
        app.set_tool(Tool::Eyedropper);

        // 1. 空白部分をクリックしても色は変わらないこと
        assert_eq!(app.pick_color_at(Point::new(20.0, 20.0)), None);
        assert_eq!(app.brush_color, green);

        // 2. 赤シェイプ上をクリックすると brush_color が赤になること
        assert_eq!(app.pick_color_at(Point::new(140.0, 200.0)), Some(red));
        assert_eq!(app.brush_color, red);

        // 3. 青シェイプ上をクリックすると brush_color が青になること
        assert_eq!(app.pick_color_at(Point::new(280.0, 200.0)), Some(blue));
        assert_eq!(app.brush_color, blue);
    }

    #[test]
    fn test_nested_grouping_ungrouping_and_edit_in_place() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        let blue = Color32::from_rgb(55, 135, 245);
        let green = Color32::from_rgb(65, 195, 105);

        // 1. ベースの赤い大シェイプ (x: 100..300, y: 200) と、別の位置に青い白目 (x: 100, y: 100) + 緑の黒目 (x: 140, y: 100) を描画
        let face = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 30.0, red);
        let eye_a = make_stroke(Point::new(90.0, 100.0), Point::new(110.0, 100.0), 15.0, blue);
        let eye_b = make_stroke(Point::new(135.0, 100.0), Point::new(155.0, 100.0), 12.0, green);
        merge_shapes_into_canvas(&mut app.shapes, face);
        merge_shapes_into_canvas(&mut app.shapes, eye_a);
        merge_shapes_into_canvas(&mut app.shapes, eye_b);
        assert_eq!(app.shapes.len(), 3);

        // 2. 青と緑の2シェイプを選択して 1段目のグループ化 (Cmd+G -> eye_gid)
        app.selected_shapes = vec![1, 2];
        let eye_gid = app
            .group_selected_shapes()
            .expect("1段目のグループ化が成功すること");
        assert_eq!(app.shapes[1].group_path, vec![eye_gid]);
        assert_eq!(app.shapes[2].group_path, vec![eye_gid]);

        // 3. 1つクリックするだけでグループ全体の2シェイプが同時選択されること
        app.clear_selection();
        assert!(app.select_single_shape_at(Point::new(100.0, 100.0)));
        assert_eq!(
            app.selected_shapes.len(),
            2,
            "グループの片方をクリックするとグループ塊全体が選択されること"
        );

        // 3b. グループ選択状態でバケツツールを使っても、グループ内の絵の色は変わらず無効であること
        app.brush_color = red;
        assert!(
            !app.bucket_fill_at(Point::new(100.0, 100.0)),
            "選択中のグループに対してバケツ塗りは無効であること"
        );
        assert_eq!(app.shapes[1].color, blue);
        assert_eq!(app.shapes[2].color, green);

        // 4. グループを赤い顔シェイプの上 (y: 100 -> y: 200) へ移動して選択解除しても、下の赤い顔シェイプが切り抜かれず両方共存すること
        let snap = app.shapes.clone();
        app.translate_selected_shapes(KVec2::new(0.0, 100.0));
        app.commit_moved_selection(snap);
        app.clear_selection();
        assert_eq!(
            app.shapes.len(),
            3,
            "グループ化されたパーツは選択解除しても下の生シェイプを切り抜かないこと"
        );
        assert!(
            app.shapes[0].contains_point(Point::new(100.0, 200.0)),
            "下の赤い顔シェイプが削られずに残っていること"
        );

        // 5. さらに「赤い顔シェイプ (idx 0)」と「目グループ (idx 1, 2)」をまとめて 2段目の入れ子グループ化 (Cmd+G -> head_gid)
        app.selected_shapes = vec![0, 1, 2];
        let head_gid = app
            .group_selected_shapes()
            .expect("2段目の入れ子グループ化が成功すること");
        assert_eq!(app.shapes[0].group_path, vec![head_gid]);
        assert_eq!(app.shapes[1].group_path, vec![head_gid, eye_gid]);
        assert_eq!(app.shapes[2].group_path, vec![head_gid, eye_gid]);

        // 6. ダブルクリック相当 (enter_group_at) で head_gid の中へインプレイス編集で潜る
        assert!(app.enter_group_at(Point::new(200.0, 200.0)));
        assert_eq!(app.editing_group_path, vec![head_gid]);
        // head_gid の中では、赤い顔 (idx 0) は生シェイプ、目 (idx 1, 2) は子グループ eye_gid として扱われること
        assert!(app.shapes[0].is_raw_at_scope(&app.editing_group_path));
        assert_eq!(
            app.shapes[1].active_group_id_at_scope(&app.editing_group_path),
            Some(eye_gid)
        );
        // さらに目グループをダブルクリックすると 2階層目 [head_gid, eye_gid] へ潜れること
        assert!(app.enter_group_at(Point::new(100.0, 200.0)));
        assert_eq!(app.editing_group_path, vec![head_gid, eye_gid]);
        assert!(app.shapes[1].is_raw_at_scope(&app.editing_group_path));

        // 2回 exit_current_group でルート階層に戻り、head_gid が選択状態になること
        assert!(app.exit_current_group());
        assert_eq!(app.editing_group_path, vec![head_gid]);
        assert!(app.exit_current_group());
        assert!(app.editing_group_path.is_empty());
        assert_eq!(app.selected_shapes.len(), 3);

        // 7. ルート階層で 1回 グループ解除 (Cmd+B / ungroup_selected_shapes) すると、
        // 外側の head_gid だけが外れ、内側の eye_gid グループはそのまま維持されること
        assert!(app.ungroup_selected_shapes());
        assert!(app.shapes[0].group_path.is_empty());
        assert_eq!(app.shapes[1].group_path, vec![eye_gid]);
        assert_eq!(app.shapes[2].group_path, vec![eye_gid]);
    }

    #[test]
    fn test_zoom_around_cursor_and_fine_brush_at_high_zoom() {
        let mut app = FlashBrushApp::default();
        let cursor = Pos2::new(320.0, 240.0);

        // 1. カーソル位置 (320, 240) を中心に 400% -> 800% へズームしても、カーソル直下のワールド座標が不変であること
        let world_before = app.screen_to_world(cursor);
        app.zoom_around_screen_pos(cursor, 4.0);
        app.zoom_around_screen_pos(cursor, 2.0);
        assert!((app.view_zoom - 8.0).abs() < 1e-5);
        let world_after = app.screen_to_world(cursor);
        assert!(
            (world_before - world_after).hypot() < 1e-4,
            "カーソル中心ズームの前後でカーソル直下のワールド座標が一致すること"
        );
        let screen_roundtrip = app.world_to_screen(world_after);
        assert!((screen_roundtrip - cursor).length() < 1e-3);

        // 2. 800% ズーム状態で極細ブラシ（半径 0.5 px）のストロークを描いた際、微小面積フィルタで消えず滑らかなシェイプが生成されること
        app.brush_radius = 0.5;
        let fine_raw = generate_brush_outline_at_zoom(
            &[Point::new(320.0, 240.0), Point::new(323.0, 240.0)],
            app.brush_radius as f64,
            app.view_zoom as f64,
        );
        app.commit_raw_outline(&fine_raw);
        assert_eq!(
            app.shapes.len(),
            1,
            "800% ズーム時の 0.5px 極細ストロークが消えずにシェイプ化されること"
        );
        assert!(app.shapes[0].contains_point(Point::new(321.5, 240.0)));

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
        app.reset_view();
        assert_eq!(app.view_zoom, 1.0);
        assert_eq!(app.view_pan, Vec2::ZERO);
    }

    #[test]
    fn test_copy_cut_paste_and_paste_in_place() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        let blue = Color32::from_rgb(55, 135, 245);

        // 1. 横長の赤シェイプ (x: 100..300, y: 200) を描画し、左半分 (x: 70..200) を矩形選択してカット (Cmd+X)
        let stroke = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red);
        merge_shapes_into_canvas(&mut app.shapes, stroke);
        assert_eq!(app.shapes.len(), 1);

        app.apply_marquee_selection(Point::new(70.0, 150.0), Point::new(200.0, 250.0), false);
        assert!(app.cut_selected_shapes(), "Cmd+X で選択領域がカットされること");
        assert_eq!(app.shapes.len(), 1, "右半分のみがキャンバスに残ること");
        assert!(!app.shapes[0].contains_point(Point::new(120.0, 200.0)));
        assert!(app.shapes[0].contains_point(Point::new(260.0, 200.0)));

        // 2. 同じ位置に貼り付け (Cmd+Shift+V) -> 元の左半分座標 (120, 200) に貼り付けられ、選択解除で右半分と1つに再結合されること
        assert!(
            app.paste_clipboard(true),
            "Cmd+Shift+V で同じ位置に貼り付けられること"
        );
        assert_eq!(app.selected_shapes.len(), 1);
        assert!(app.shapes[app.selected_shapes[0]].contains_point(Point::new(120.0, 200.0)));
        app.clear_selection();
        assert_eq!(
            app.shapes.len(),
            1,
            "同じ位置に貼り付けた同色生シェイプは選択解除時に元の右半分とシームレスに結合されること"
        );
        assert!(app.shapes[0].contains_point(Point::new(120.0, 200.0)));
        assert!(app.shapes[0].contains_point(Point::new(260.0, 200.0)));

        // 3. グループ化されたオブジェクトのコピー (Cmd+C) ＆通常貼り付け (Cmd+V) で、
        //    元のグループとは独立した新しいグループIDが採番され、オフセット (+16, +16) 位置に配置されること
        app.shapes.clear();
        let g_part = make_stroke(Point::new(100.0, 100.0), Point::new(140.0, 100.0), 15.0, blue);
        merge_shapes_into_canvas(&mut app.shapes, g_part);
        app.selected_shapes = vec![0];
        let orig_gid = app.group_selected_shapes().unwrap();

        assert!(app.copy_selected_shapes(), "Cmd+C でグループがコピーされること");
        assert!(
            app.paste_clipboard(false),
            "Cmd+V でオフセット付き貼り付けが成功すること"
        );
        assert_eq!(app.shapes.len(), 2);
        let pasted_idx = app.selected_shapes[0];
        let pasted_gid = app.shapes[pasted_idx].group_path[0];
        assert_ne!(
            pasted_gid, orig_gid,
            "複製されたグループには元のグループとは別の一意なグループIDが割り当てられること"
        );
        assert!(
            app.shapes[pasted_idx].contains_point(Point::new(120.0 + 16.0, 100.0 + 16.0)),
            "Cmd+V では (+16, +16) ずれた位置に貼り付けられること"
        );

        // 4. Alt + ドラッグ複製 (duplicate_selected_shapes_for_alt_drag) でシンボルやグループが独立したIDで複製移動でき、
        //    1回の Undo で複製前の状態に戻ること
        let sym_id = app.convert_selection_to_symbol().unwrap();
        let orig_inst_gid = app.selected_single_symbol_info().unwrap().0.instance_gid;
        let count_before = app.shapes.len();

        let undo_snapshot = app.duplicate_selected_shapes_for_alt_drag();
        assert_eq!(app.shapes.len(), count_before + 1);
        app.translate_selected_shapes(KVec2::new(60.0, 40.0));
        app.finish_move_selection_drag(KVec2::new(60.0, 40.0), undo_snapshot);

        let (dup_binding, _, _) = app.selected_single_symbol_info().unwrap();
        assert_eq!(dup_binding.symbol_id, sym_id);
        assert_ne!(
            dup_binding.instance_gid, orig_inst_gid,
            "Alt+ドラッグで複製されたシンボルインスタンスには新しい一意な instance_gid が採番されること"
        );
        assert!(
            app.shapes[app.selected_shapes[0]]
                .contains_point(Point::new(120.0 + 16.0 + 60.0, 100.0 + 16.0 + 40.0)),
            "複製されたシンボルがドラッグ先座標に移動していること"
        );
        app.undo();
        assert_eq!(
            app.shapes.len(),
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
            merge_shapes_into_canvas(&mut cel.shapes, stroke);
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

        // FlashBrushApp 統合テスト：F6（複製キーフレーム進行）・F7（空白キーフレーム進行）・F5（延長）・レイヤー操作・Undo/Redo
        let mut app = FlashBrushApp::default();
        let blue = Color32::from_rgb(55, 135, 245);
        let s0 = make_stroke(Point::new(100.0, 100.0), Point::new(150.0, 100.0), 10.0, red);
        merge_shapes_into_canvas(&mut app.shapes, s0);
        app.sync_shapes_to_timeline();
        assert_eq!(app.timeline.current_frame, 0);
        assert_eq!(app.shapes.len(), 1);

        // 1. キーフレーム先頭 (frame 0) で F6 を押すと、自動的に次のフレーム (frame 1) へ進みつつ絵が複製されること
        assert!(app.timeline_insert_keyframe(true));
        assert_eq!(app.timeline.current_frame, 1);
        assert_eq!(app.shapes.len(), 1);

        // 2. 絵があるキーフレーム先頭 (frame 1) で F7 を押すと、次のフレーム (frame 2) に空白キーフレームが作成されること
        assert!(app.timeline_insert_keyframe(false));
        assert_eq!(app.timeline.current_frame, 2);
        assert!(app.shapes.is_empty());

        // 3. F5 でフレーム延長・Shift+F5 で短縮・Shift+F6 でキーフレーム削除と Undo/Redo が正確に動作すること
        let prev_total = app.timeline.total_frames;
        assert!(app.timeline_extend_frame());
        assert_eq!(app.timeline.total_frames, prev_total + 1);
        assert!(app.timeline_remove_frame());
        assert_eq!(app.timeline.total_frames, prev_total);

        // 3b. Adobe Animate 仕様：現在の尺（prev_total）より先の空マス（例: frame 9 = 10フレーム目）を選択して F5 を押すと、
        //     そのフレーム位置まで一気に保持尺が伸びること
        app.select_timeline_cells(0, 9, 0, 9);
        assert_eq!(app.timeline.current_frame, 9);
        assert!(app.timeline_extend_frame());
        assert_eq!(app.timeline.layers[0].frame_count, 10);
        assert_eq!(app.timeline.total_frames, 10);
        app.undo();
        assert_eq!(app.timeline.total_frames, prev_total);
        app.switch_to_layer_and_frame(0, 2);

        assert!(app.timeline_delete_keyframe());
        assert_eq!(
            app.shapes.len(),
            1,
            "frame 2 の空白キーフレームを削除すると直前の frame 1 の絵が保持表示されること"
        );
        app.undo();
        assert!(
            app.shapes.is_empty(),
            "Undo で削除前の空白キーフレーム状態に戻ること"
        );
        app.redo();
        assert_eq!(app.shapes.len(), 1);

        // 4. 新規レイヤー追加・複数レイヤー横断範囲選択（Adobe Animate 挙動）・ドラッグ＆ドロップ重ね順入れ替え・削除
        app.switch_to_layer_and_frame(0, 0);
        assert_eq!(app.shapes.len(), 1);
        app.timeline_add_layer(); // Layer 2 (index 1)
        assert_eq!(app.timeline.layers.len(), 2);
        assert_eq!(app.timeline.active_layer, 1);
        assert!(
            app.timeline.active_cel_shapes().is_empty(),
            "新しく追加された前面レイヤーのセルは空であること"
        );
        assert_eq!(
            app.shapes.len(),
            1,
            "作業キャンバス self.shapes には全可視・非ロックレイヤーの図形が展開されていること"
        );
        app.brush_color = blue;
        assert_eq!(
            app.pick_color_at(Point::new(125.0, 100.0)),
            Some(red),
            "空の前面レイヤー上でも、可視状態の背面レイヤーにある図形の色をスポイトで抽出できること"
        );

        // Layer 2 (index 1) に青いストロークを追加（Layer 1 の赤いストロークと重なる位置）
        app.brush_color = blue;
        app.commit_raw_outline(&[
            Point::new(120.0, 70.0),
            Point::new(120.0, 130.0),
            Point::new(135.0, 130.0),
            Point::new(135.0, 70.0),
        ]);
        app.sync_shapes_to_timeline();
        assert_eq!(
            app.timeline.layers[0].cel_at(0).unwrap().shapes.len(),
            1,
            "別レイヤーのストロークによって背面レイヤーの図形が削られないこと（レイヤー間の Merge Drawing 分離）"
        );
        assert_eq!(app.timeline.layers[1].cel_at(0).unwrap().shapes.len(), 1);
        assert_eq!(app.shapes.len(), 2);

        // 現在 Layer 2 を選択していても、範囲選択（Marquee Selection）で Layer 1 と Layer 2 の両方の絵を一括選択・移動できること
        app.apply_marquee_selection(Point::new(50.0, 50.0), Point::new(200.0, 200.0), false);
        assert_eq!(
            app.selected_shapes.len(),
            2,
            "Adobe Animate 同様に、範囲選択で複数レイヤーにまたがる全ての非ロック図形を一括選択できること"
        );
        let undo_snap = app.shapes.clone();
        app.translate_selected_shapes(KVec2::new(30.0, 0.0));
        app.commit_moved_selection(undo_snap);
        app.clear_selection();
        app.sync_shapes_to_timeline();
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
        app.set_layer_locked(0, true);
        app.apply_marquee_selection(Point::new(50.0, 50.0), Point::new(250.0, 200.0), false);
        assert_eq!(
            app.selected_shapes.len(),
            1,
            "ロックされたレイヤーの図形は範囲選択から除外されて保護されること"
        );
        app.clear_selection();
        app.set_layer_locked(0, false);
        assert_eq!(app.shapes.len(), 2);

        app.timeline_add_layer(); // Layer 3 (index 2)
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
        assert!(app.timeline_reorder_layer(2, 0));
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

        assert!(app.timeline_move_active_layer(true));
        assert_eq!(app.timeline.active_layer, 1);
        assert!(app.timeline_delete_active_layer());
        assert_eq!(app.timeline.layers.len(), 2);
        assert!(app.timeline_delete_active_layer());
        assert_eq!(app.timeline.layers.len(), 1);

        // 5. 追加仕様6項目の検証：
        // (1) レイヤー選択状態に応じた F5 / Shift+F5 挙動（選択レイヤーのみ vs 全レイヤー）
        let mut app2 = FlashBrushApp::default();
        app2.commit_raw_outline(&[
            Point::new(40.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(80.0, 80.0),
            Point::new(40.0, 80.0),
        ]);
        app2.timeline_add_layer(); // Layer 2 (index 1)
        app2.brush_color = blue;
        app2.commit_raw_outline(&[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ]);
        app2.sync_shapes_to_timeline();
        let base_len = app2.timeline.layers[0].frame_count;
        assert_eq!(app2.timeline.layers[1].frame_count, base_len);

        // Layer 2 (index 1) だけを選択した状態で F5 -> Layer 2 だけが +1 伸び、Layer 1 はそのまま
        app2.select_timeline_cells(1, 0, 1, 0);
        assert!(app2.timeline_extend_frame());
        assert_eq!(app2.timeline.layers[0].frame_count, base_len);
        assert_eq!(app2.timeline.layers[1].frame_count, base_len + 1);

        // ルーラースクラブ等でレイヤー未選択状態 (cell_selection = None) にして F5 -> 全レイヤーが +1 伸びる
        app2.scrub_ruler_to_frame(0);
        assert!(app2.timeline.cell_selection.is_none());
        assert!(app2.timeline_extend_frame());
        assert_eq!(app2.timeline.layers[0].frame_count, base_len + 1);
        assert_eq!(app2.timeline.layers[1].frame_count, base_len + 2);

        // (2) キーフレーム（フレームセル）をクリックしたとき、そのキーフレーム内の絵が全て選択状態になること
        app2.select_timeline_cells(0, 0, 0, 0);
        assert_eq!(
            app2.selected_shapes.len(),
            1,
            "Layer 1 のキーフレームをクリックすると Layer 1 の絵が全選択されること"
        );
        assert_eq!(
            app2.shapes[app2.selected_shapes[0]].layer_id,
            app2.timeline.layers[0].id
        );

        // (3) キャンバスの任意の絵を選択したとき、その絵が所属するレイヤーがタイムラインで選択状態になること
        app2.clear_selection();
        app2.scrub_ruler_to_frame(0);
        assert!(app2.select_single_shape_at(Point::new(160.0, 60.0))); // Layer 2 の青い矩形をクリック
        assert_eq!(app2.timeline.active_layer, 1);
        assert_eq!(
            app2.timeline.cell_selection.map(|s| (s.min_layer, s.max_layer)),
            Some((1, 1)),
            "キャンバスの絵を選択すると、その絵が属するレイヤーがタイムラインでも選択状態になること"
        );

        // (4) タイムライン上でレイヤーをまたいでドラッグ範囲選択すると、それらのレイヤーの絵がすべて選択状態になること
        app2.select_timeline_cells(0, 0, 1, 0);
        assert_eq!(
            app2.selected_shapes.len(),
            2,
            "レイヤーをまたいでタイムラインセルを範囲選択すると、対象レイヤー群の絵がすべて選択状態になること"
        );

        // (5) 選択フレームのドラッグ移動および Alt+ドラッグ複製（コピー）
        let sel_l1_f0 = timeline::TimelineCellSelection::single(1, 0);
        // Alt+ドラッグ（copy_mode = true）で Layer 2 の frame 0 を frame 3 へ複製
        assert!(app2.timeline_move_or_copy_frames(sel_l1_f0, 0, 3, true));
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
        assert!(app2.timeline_move_or_copy_frames(sel_l1_f3, 0, 2, false));
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
        assert!(app2.timeline_move_or_copy_frames(sel_l0_f0, 0, 2, false));
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
        app2.select_timeline_cells(1, 0, 1, 0);
        assert!(app2.timeline_remove_frame());
        assert_eq!(
            app2.timeline.layers[1].keyframes.len(),
            kf_count_before,
            "保持フレームがあるキーフレーム上での Shift+F5 はキーフレームを消さず保持尺のみ1コマ縮めること"
        );
        // 次に 1 コマ幅になったキーフレーム（旧 frame 3 -> 現 frame 2）の上で Shift+F5 を押すと、その1コマKF自体が削除され後続が左へ詰まる
        app2.select_timeline_cells(1, 2, 1, 2);
        assert!(app2.timeline_remove_frame());
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
        app2.toggle_playback();
        assert!(app2.timeline.is_playing);
        app2.timeline.current_frame = 0;
        app2.toggle_playback();
        assert!(!app2.timeline.is_playing);
        assert_eq!(
            app2.shapes.len(),
            1,
            "再生停止時に停止フレーム (frame 0) の図形がキャンバスに復元されること"
        );
    }

    #[test]
    fn test_adobe_animate_click_selects_edge_and_selected_shape_disables_deformation() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(235, 75, 75);
        app.set_tool(Tool::Select);

        // 細い 2px ブラシストロークを描画（ほぼ全域が境界線の吸着範囲内になる細い線）
        let thin_stroke = make_stroke(Point::new(100.0, 200.0), Point::new(220.0, 200.0), 2.0, red);
        merge_shapes_into_canvas(&mut app.shapes, thin_stroke);
        assert_eq!(app.shapes.len(), 1);

        let vertex_pt = app.shapes[0].exterior.segments[0].p0;
        let edge_pt = app.shapes[0].exterior.segments[0].eval(0.5);

        // 1. 未選択状態では、頂点や境界線にホバーすると Vertex / Edge（変形対象）になること
        assert!(matches!(
            app.compute_hover_target(vertex_pt),
            HoverTarget::Vertex { shape_idx: 0, .. }
        ));
        assert!(matches!(
            app.compute_hover_target(edge_pt),
            HoverTarget::Edge { shape_idx: 0, .. } | HoverTarget::Vertex { shape_idx: 0, .. }
        ));

        // 2. 細い線の境界線や頂点の上でクリック（select_single_shape_at）した際にも、確実に選択状態になること
        assert!(
            app.select_single_shape_at(vertex_pt),
            "頂点や境界線上をクリックしてもシェイプが選択状態になること"
        );
        assert_eq!(app.selected_shapes, vec![0]);

        // 3. 選択状態（網点表示）になった後は、頂点上・境界線上・少し外側のマージンのどこを掴んでも
        //    変形（Vertex / Edge）は無効化され、常に移動（HoverTarget::Fill）になること
        assert_eq!(
            app.compute_hover_target(vertex_pt),
            HoverTarget::Fill { shape_idx: 0 },
            "選択状態のシェイプの頂点上では変形ではなく移動（Fill）になること"
        );
        assert_eq!(
            app.compute_hover_target(edge_pt),
            HoverTarget::Fill { shape_idx: 0 },
            "選択状態のシェイプの境界線上では変形ではなく移動（Fill）になること"
        );
        assert_eq!(
            app.compute_hover_target(Point::new(160.0, 204.5)),
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
        let mut app = FlashBrushApp {
            shapes: make_stroke(
                Point::new(100.0, 150.0),
                Point::new(200.0, 150.0),
                16.0,
                Color32::from_rgb(230, 80, 80),
            ),
            selected_shapes: vec![0],
            ..Default::default()
        };

        // 1. F8 シンボル化で "Symbol 1" がライブラリに登録され、インスタンスになること
        let sym_id = app
            .convert_selection_to_symbol()
            .expect("シンボル変換が成功すること");
        assert_eq!(app.library.len(), 1);
        assert_eq!(app.library[0].id, sym_id);
        assert_eq!(app.library[0].name, "Symbol 1");
        assert!(app.selected_single_symbol_info().is_some());

        // 2. ライブラリからステージへ2つ目のインスタンスを配置できること
        assert!(app.place_symbol_instance_from_library(sym_id));
        assert_eq!(app.shapes.len(), 2);
        assert_ne!(
            app.shapes[0].symbol_instance.unwrap().instance_gid,
            app.shapes[1].symbol_instance.unwrap().instance_gid
        );

        // 3. 1つ目のインスタンスをダブルクリックすると、シンボル専用のタイムラインへ切り替わること
        assert!(app.enter_group_at(Point::new(150.0, 150.0)));
        assert_eq!(app.symbol_edit_stack.len(), 1);
        assert_eq!(app.timeline.total_frames, 1);

        // シンボル内タイムラインで F6（複製キーフレーム）を押して2フレーム目を作成し、2フレーム目に帽子を描き足す
        assert!(app.timeline_insert_keyframe(true));
        assert_eq!(app.timeline.total_frames, 2);
        assert_eq!(app.timeline.current_frame, 1);

        let hat = make_stroke(
            Point::new(120.0, 80.0),
            Point::new(180.0, 80.0),
            10.0,
            Color32::from_rgb(50, 120, 240),
        )
        .remove(0);
        app.shapes.push(hat);
        app.sync_shapes_to_timeline();

        // 4. シンボルから親シーン（シーン1）へ戻ると、シンボル内タイムライン（2フレーム）が保持され、
        //    親シーンのフレーム進行に合わせてループ評価されること
        assert!(app.exit_current_group());
        assert!(app.symbol_edit_stack.is_empty());
        assert_eq!(app.library[0].total_frames, 2);

        // シーン1の尺を3フレームに延長（F5 x 2）
        assert!(app.timeline_extend_frame());
        assert!(app.timeline_extend_frame());
        assert_eq!(app.timeline.total_frames, 3);

        // フレーム0（シンボル内 0F = 1シェイプ × 2インスタンス = 2シェイプ）
        app.switch_to_layer_and_frame(0, 0);
        assert_eq!(app.shapes.len(), 2);

        // フレーム1（シンボル内 1F = 2シェイプ × 2インスタンス = 4シェイプ）
        app.switch_to_layer_and_frame(0, 1);
        assert_eq!(app.shapes.len(), 4);

        // フレーム2（ループしてシンボル内 0F = 1シェイプ × 2インスタンス = 2シェイプ）
        app.switch_to_layer_and_frame(0, 2);
        assert_eq!(app.shapes.len(), 2);
    }

    #[test]
    fn test_classic_tween_single_symbol_condition_and_interpolation() {
        use crate::symbol::TweenState;

        let mut app = FlashBrushApp::default();

        // 1. 空のキーフレームではトゥイーン (T) を設定できないこと
        assert!(!app.timeline_toggle_tween());

        // 2. 生の絵（Raw Shape）だけのキーフレームではトゥイーンを設定できないこと
        app.shapes = make_stroke(
            Point::new(100.0, 150.0),
            Point::new(200.0, 150.0),
            16.0,
            Color32::from_rgb(230, 80, 80),
        );
        assert!(!app.timeline_toggle_tween());

        // 3. 通常のグループ（Cmd+G）だけのキーフレームでもトゥイーンを設定できないこと
        app.selected_shapes = vec![0];
        assert!(app.group_selected_shapes().is_some());
        assert!(!app.timeline_toggle_tween());
        assert!(app.ungroup_selected_shapes());

        // 4. F8 で単一シンボルに変換すると、トゥイーンを設定できること
        app.selected_shapes = vec![0];
        let sym_id = app.convert_selection_to_symbol().unwrap();
        assert!(app.timeline_toggle_tween());
        assert!(app.timeline.layers[0].keyframes[0].tween);
        assert_eq!(
            app.timeline.layers[0].tween_state_at_keyframe(0),
            TweenState::Incomplete,
            "終了キーフレームがない状態では未完トゥイーン（横点線）になること"
        );

        // 5. もう1回 T を押すとトゥイーンを解除でき、さらに別のシンボルや生の絵を混在させると T が拒否されること
        assert!(app.timeline_toggle_tween());
        assert!(!app.timeline.layers[0].keyframes[0].tween);

        // 2つ目のシンボルインスタンスを配置（複数シンボル混在）→ トゥイーン設定不可
        assert!(app.place_symbol_instance_from_library(sym_id));
        assert!(!app.timeline_toggle_tween());
        // 2つ目のインスタンスを削除して単一シンボルに戻す
        assert!(app.delete_selected_shapes());

        // 生の絵を1つ追加（シンボル＋生の絵混在）→ トゥイーン設定不可
        let raw_extra = make_stroke(
            Point::new(300.0, 300.0),
            Point::new(340.0, 300.0),
            8.0,
            Color32::BLACK,
        )
        .remove(0);
        app.shapes.push(raw_extra);
        assert!(!app.timeline_toggle_tween());
        app.shapes.pop();

        // 6. 単一シンボルだけの状態で T を押し、F5 で 4 コマ延長（frame 0..=4）、frame 4 で F6 を押して終了キーフレームを作成
        assert!(app.timeline_toggle_tween());
        for _ in 0..4 {
            assert!(app.timeline_extend_frame());
        }
        assert_eq!(app.timeline.total_frames, 5);
        assert_eq!(
            app.timeline.layers[0].tween_state_at_keyframe(0),
            TweenState::Incomplete
        );

        // frame 4 に移動して F6（複製キーフレーム作成）
        app.switch_to_layer_and_frame(0, 4);
        assert!(app.timeline_insert_keyframe(true));
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
        app.selected_shapes = vec![0];
        let snap = app.shapes.clone();
        app.translate_selected_shapes(KVec2::new(200.0, 80.0));
        app.commit_moved_selection(snap);
        app.sync_shapes_to_timeline();

        // 中間フレーム（frame 2, t = 0.5）へ切り替えると、中心座標が (150 + 100, 150 + 40) = (250, 190) に補間されていること
        app.switch_to_layer_and_frame(0, 2);
        let mid_bbox = app.shapes[0].bounding_box();
        let mid_cx = (mid_bbox.x0 + mid_bbox.x1) * 0.5;
        let mid_cy = (mid_bbox.y0 + mid_bbox.y1) * 0.5;
        assert!((mid_cx - 250.0).abs() < 1.0, "mid_cx = {mid_cx}");
        assert!((mid_cy - 190.0).abs() < 1.0, "mid_cy = {mid_cy}");

        // 中間フレームを表示したあとに先頭フレーム（frame 0）へ戻っても、先頭キーフレームの座標 (150, 150) が上書き破壊されていないこと
        app.switch_to_layer_and_frame(0, 0);
        let start_bbox = app.shapes[0].bounding_box();
        let start_cx = (start_bbox.x0 + start_bbox.x1) * 0.5;
        let start_cy = (start_bbox.y0 + start_bbox.y1) * 0.5;
        assert!((start_cx - 150.0).abs() < 1.0, "start_cx = {start_cx}");
        assert!((start_cy - 150.0).abs() < 1.0, "start_cy = {start_cy}");

        // 8. シンボルインスタンスの変形中心（ピボット）を左端の関節位置 (100, 150) に設定し、
        //    選択解除・フレーム移動後も保持され、そのピボットを中心にクラシックトゥイーン回転補間されること
        app.switch_to_layer_and_frame(0, 4);
        app.selected_shapes = vec![0];
        // 一旦 frame 4 の位置も frame 0 と同じ (150, 150) に戻す
        let snap_reset = app.shapes.clone();
        app.translate_selected_shapes(KVec2::new(-200.0, -80.0));
        app.commit_moved_selection(snap_reset);
        app.sync_shapes_to_timeline();

        // frame 4 でピボットを左端 (100, 150) [ローカル座標 (-50, 0)] に移動して +90° 回転
        let joint_world = Point::new(100.0, 150.0);
        assert!(app.set_selected_symbol_world_pivot(Some(joint_world), true));
        app.clear_selection();
        app.selected_shapes = vec![0];
        let bbox_f4 = app.selection_bounding_box().unwrap();
        assert!(
            (app.active_transform_pivot(bbox_f4) - joint_world).hypot() < 1e-4,
            "選択解除後もシンボルインスタンスのカスタムピボット座標が保持されること"
        );
        assert!(app.rotate_selected_shapes(std::f64::consts::FRAC_PI_2));
        app.sync_shapes_to_timeline();

        // 中間フレーム（frame 2, t = 0.5 = +45° 回転）において、関節ピボット (100, 150) がぶれずに固定され、
        // 元の中心 (150, 150) [関節から距離 50px] が 45° 回転位置 (100 + 50*cos45°, 150 + 50*sin45°) を通過すること
        app.switch_to_layer_and_frame(0, 2);
        let mid_binding = app.shapes[0].symbol_instance.unwrap();
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
            app.shapes[0].contains_point(expected_mid_center),
            "中間フレーム (t=0.5) でピボット (100, 150) を軸とした +45° 回転位置にシェイプが存在すること"
        );

        // 9. タイムラインのキーフレーム選択時とキャンバス上のシンボル選択時で `inspect_timeline_frame` が切り替わり、
        //    トゥイーンの回転方向（時計回り CW / 反時計回り CCW）が中間フレームに反映されること
        app.select_timeline_cells(0, 0, 0, 0);
        assert!(
            app.inspect_timeline_frame,
            "タイムラインのセルを選択したときはフレーム属性表示モード (inspect_timeline_frame = true) になること"
        );
        app.select_single_shape_at(Point::new(150.0, 150.0));
        assert!(
            !app.inspect_timeline_frame,
            "キャンバス上のシンボルをクリックしたときはシンボル属性表示モード (inspect_timeline_frame = false) になること"
        );

        // 先頭キーフレームを選択して回転の向きを「反時計回り (CCW)」に変更すると、
        // 0° -> +90° (CW) の終了角度へ反時計回りに -270° 回転し、中間フレーム (t=0.5) では -135° 位置を通過すること
        app.select_timeline_cells(0, 0, 0, 0);
        assert!(app.set_active_keyframe_tween_rotate(crate::symbol::TweenRotate::CounterClockwise));
        app.switch_to_layer_and_frame(0, 2);
        let expected_ccw_mid_center = Point::new(
            100.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
            150.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        );
        assert!(
            app.shapes[0].contains_point(expected_ccw_mid_center),
            "反時計回り (CCW) 指定時は中間フレーム (t=0.5) で -135° 回転位置を通過すること"
        );
    }

    #[test]
    fn test_symbol_playback_modes_loop_play_once_and_single_frame() {
        use crate::symbol::SymbolLoopMode;

        let mut app = FlashBrushApp::default();

        // 1. シンボルを作成し、内部を3フレーム構成にする（0F=赤、1F=緑、2F=青）
        let red = Color32::from_rgb(220, 60, 60);
        let green = Color32::from_rgb(60, 190, 90);
        let blue = Color32::from_rgb(60, 110, 230);
        app.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 12.0, red);
        app.selected_shapes = vec![0];
        assert!(app.convert_selection_to_symbol().is_some());

        let (binding, _, _) = app.selected_single_symbol_info().unwrap();
        assert!(app.enter_symbol_timeline(binding));

        // シンボル内部 1F（空白キーフレーム → 緑シェイプ）
        assert!(app.timeline_extend_frame());
        app.switch_to_layer_and_frame(0, 1);
        assert!(app.timeline_insert_keyframe(false));
        app.shapes = make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            green,
        );
        app.sync_shapes_to_timeline();

        // シンボル内部 2F（空白キーフレーム → 青シェイプ）
        assert!(app.timeline_extend_frame());
        app.switch_to_layer_and_frame(0, 2);
        assert!(app.timeline_insert_keyframe(false));
        app.shapes = make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            blue,
        );
        app.sync_shapes_to_timeline();
        assert_eq!(app.timeline.total_frames, 3);

        // 親シーンへ戻る（5フレーム分の尺に延長: frame 0..=4）
        assert!(app.exit_symbol_timeline());
        for _ in 0..4 {
            assert!(app.timeline_extend_frame());
        }
        assert_eq!(app.timeline.total_frames, 5);

        // 2. デフォルト（Loop, first_frame = 0）: 0F=赤, 1F=緑, 2F=青, 3F=赤, 4F=緑
        let expected_loop = [red, green, blue, red, green];
        for (f, &col) in expected_loop.iter().enumerate() {
            app.switch_to_layer_and_frame(0, f);
            assert_eq!(app.shapes[0].color, col, "Loop mode at frame {f}");
        }

        // 3. 1回再生（PlayOnce, first_frame = 0）に変更: 0F=赤, 1F=緑, 2F=青, 3F=青, 4F=青
        app.switch_to_layer_and_frame(0, 0);
        app.selected_shapes = vec![0];
        assert!(app.set_selected_symbol_playback(SymbolLoopMode::PlayOnce, 0));
        let expected_once = [red, green, blue, blue, blue];
        for (f, &col) in expected_once.iter().enumerate() {
            app.switch_to_layer_and_frame(0, f);
            assert_eq!(app.shapes[0].color, col, "PlayOnce mode at frame {f}");
        }

        // 4. 任意のフレーム（SingleFrame, first_frame = 1 [UI上は2フレーム目=緑]）に変更: 全フレームで緑
        app.switch_to_layer_and_frame(0, 0);
        app.selected_shapes = vec![0];
        assert!(app.set_selected_symbol_playback(SymbolLoopMode::SingleFrame, 1));
        assert_eq!(app.shapes[0].color, green);
        assert_eq!(app.selected_shapes, vec![0]);
        for f in 0..5 {
            app.switch_to_layer_and_frame(0, f);
            assert_eq!(app.shapes[0].color, green, "SingleFrame(1) at frame {f}");
        }
    }

    #[test]
    fn test_nested_symbol_evaluation_and_library_delete_detach() {
        let mut app = FlashBrushApp::default();
        let red = Color32::from_rgb(220, 60, 60);
        let blue = Color32::from_rgb(60, 110, 230);

        // 1. 子シンボル（Symbol 1: 0F=赤, 1F=青 の2フレーム）を作成
        app.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 12.0, red);
        app.selected_shapes = vec![0];
        let child_sym_id = app.convert_selection_to_symbol().unwrap();
        let (child_binding, _, _) = app.selected_single_symbol_info().unwrap();
        assert!(app.enter_symbol_timeline(child_binding));
        assert!(app.timeline_extend_frame());
        app.switch_to_layer_and_frame(0, 1);
        assert!(app.timeline_insert_keyframe(false));
        app.shapes = make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 12.0, blue);
        app.sync_shapes_to_timeline();
        assert!(app.exit_symbol_timeline());

        // 2. 子シンボルインスタンスを選択したまま、さらに親シンボル（Symbol 2: 1フレーム）に変換（入れ子構造）
        app.selected_shapes = vec![0];
        let parent_sym_id = app.convert_selection_to_symbol().unwrap();
        assert_ne!(child_sym_id, parent_sym_id);

        // 親シーンを2フレームに延長すると、1フレームの親シンボル越しに子シンボルの2フレームアニメが再帰評価されること
        assert!(app.timeline_extend_frame());
        app.switch_to_layer_and_frame(0, 0);
        assert_eq!(app.shapes[0].color, red);
        app.switch_to_layer_and_frame(0, 1);
        assert_eq!(app.shapes[0].color, blue);

        // 3. ライブラリから親シンボルを削除すると、ステージ上の図形は消滅せず通常グループへデタッチされて残ること
        app.switch_to_layer_and_frame(0, 0);
        assert!(app.delete_symbol_from_library(parent_sym_id));
        assert_eq!(app.shapes.len(), 1, "シンボル削除時もステージ上の絵が消えないこと");
        assert!(
            app.shapes[0].symbol_instance.is_none(),
            "シンボル紐付けが解除されていること"
        );
        assert!(
            !app.shapes[0].group_path.is_empty(),
            "通常のグループ図形として group_path が維持されていること"
        );
    }

    #[test]
    fn test_crash_resilience_against_degenerate_inputs_and_rapid_state_transitions() {
        use crate::vector_engine::union_bounding_boxes;
        use kurbo::{Affine, Rect as KRect};

        let mut app = FlashBrushApp::default();
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
        app.shapes = make_stroke(Point::new(50.0, 100.0), Point::new(200.0, 100.0), 14.0, red);
        app.sync_shapes_to_timeline();
        app.apply_marquee_selection(Point::new(80.0, 70.0), Point::new(130.0, 130.0), false);
        assert!(app.convert_selection_to_symbol().is_some());
        if let Some((b, _, _)) = app.selected_single_symbol_info() {
            assert!(app.enter_symbol_timeline(b));
            assert!(app.exit_symbol_timeline());
        }
        assert!(!app.exit_symbol_timeline());
        assert!(!app.exit_current_group());
        app.undo();
        app.redo();
        app.set_layer_visible(999, false);
        app.set_layer_locked(999, true);
        assert!(!app.timeline_move_or_copy_frames(
            crate::timeline::TimelineCellSelection::single(999, 999),
            1,
            1,
            false
        ));
    }
