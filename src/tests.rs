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

impl std::ops::Deref for Is2dApp {
    type Target = Tab;
    fn deref(&self) -> &Self::Target {
        &self.tabs[self.active]
    }
}

impl std::ops::DerefMut for Is2dApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.tabs[self.active]
    }
}

impl symbol::Document {
    fn register_new_symbol(&mut self, local_shapes: Vec<FillShape>) -> u64 {
        let sym_id = self.alloc_symbol_id();
        self.register_symbol(sym_id, local_shapes);
        sym_id
    }
}

/// 編集中のタイムラインの指定レイヤー・指定フレームのセル図形列を `edit_layers` 経由で配置するテスト補助関数
fn put_cel_shapes(app: &mut Is2dApp, layer_idx: usize, frame: usize, mut shapes: Vec<FillShape>) {
    let Some((lid, prev_frame, prev_vis, prev_lock)) =
        app.tab_mut().with_active_timeline_mut(|tl, _lib| {
            let layer = tl.layers.get_mut(layer_idx)?;
            let lid = layer.id;
            let prev_vis = layer.visible;
            let prev_lock = layer.locked;
            layer.visible = true;
            layer.locked = false;
            let prev_frame = tl.current_frame;
            tl.current_frame = frame;
            Some((lid, prev_frame, prev_vis, prev_lock))
        })
    else {
        return;
    };
    for s in &mut shapes {
        s.layer_id = lid;
    }
    canvas_tool::edit_layers(app.tab_mut(), &[lid], |edit| {
        edit.shapes = shapes;
    });
    if prev_frame != frame || !prev_vis || prev_lock {
        app.tab_mut().with_active_timeline_mut(|tl, _lib| {
            tl.current_frame = prev_frame;
            if let Some(layer) = tl.layers.get_mut(layer_idx) {
                layer.visible = prev_vis;
                layer.locked = prev_lock;
            }
        });
        app.refresh_scene_base();
    }
}

#[derive(Clone, Debug)]
struct SceneSummary {
    len: usize,
    is_empty: bool,
    shapes: Vec<FillShape>,
}

/// 画面上の図形一覧（`app.scene()`）を取得するテスト補助関数
fn scene_summary(app: &Is2dApp) -> SceneSummary {
    let shapes: Vec<FillShape> = app.scene().iter().cloned().collect();
    let len = shapes.len();
    let is_empty = shapes.is_empty();
    SceneSummary {
        len,
        is_empty,
        shapes,
    }
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
    crate::select_ops::apply_pull_edge_to_shape(
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
    crate::select_ops::apply_pull_edge_to_shape(
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
    doc_actions::commit_raw_outline(&mut app, &raw_ring);
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
    let inner_shape = s.shapes.iter().find(|sh| sh.color == red).unwrap();
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
        .any(|sh| sh.color == blue && sh.contains_point(center)));

    // 4. 内側の領域を周囲の線と同じ黒でバケツクリック -> 外周の黒リングと Union されて穴なしの1つの黒シェイプになること
    app.tool.brush_color = black;
    assert!(doc_actions::bucket_fill_at(&mut app, center));
    let s = scene_summary(&app);
    assert_eq!(
        s.shapes.len(),
        1,
        "同色でバケツ塗りすると周囲のシェイプと1つに結合されること"
    );
    assert!(s.shapes[0].holes.is_empty(), "穴が埋まって消えていること");
    assert!(s.shapes[0].contains_point(center));
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
    put_cel_shapes(&mut app, 0, 0, stroke);
    assert_eq!(scene_summary(&app).shapes.len(), 1);

    // 1. 左端 (x: 70..140, y: 150..250) を矩形選択 -> 左端だけスライスされて選択状態になること
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(140.0, 250.0),
        false,
    );
    assert_eq!(
        app.scene().len(),
        2,
        "部分矩形選択で内外2つにスライスされること"
    );
    assert_eq!(
        app.session.selected_indices().len(),
        1,
        "スライスされた内側のみが選択されていること"
    );
    assert!(
        app.scene()
            .get(app.session.selected_indices()[0])
            .unwrap()
            .contains_point(Point::new(110.0, 200.0)),
        "選択された断片が左端領域を含んでいること"
    );

    // 2. 移動も削除もせずに選択解除（clear_selection）すると、無劣化で元の1つのシェイプに戻ること
    canvas_tool::clear_selection(&mut app);
    assert_eq!(
        app.scene().len(),
        1,
        "未編集で選択解除すると元の1つのシェイプに復元されること"
    );
    assert!(app.session.selection.is_empty());

    // 3. 左端 (x: 70..140) を矩形選択し、続いて Shift キー押下（additive = true）で右端 (x: 260..330) も連続矩形選択
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(140.0, 250.0),
        false,
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(260.0, 150.0),
        Point::new(330.0, 250.0),
        true,
    );
    assert_eq!(
        app.scene().len(),
        3,
        "左右両端のスライスと中央の非選択領域の計3つに分割されること"
    );
    assert_eq!(
        app.session.selected_indices().len(),
        2,
        "Shift 連続矩形選択で左右2つの領域が同時に選択されていること"
    );

    // 4. 選択された左右両端を削除すると、中央 (x: 200, y: 200) のみが残ること
    assert!(doc_actions::delete_selected_shapes(&mut app));
    let s = scene_summary(&app);
    assert_eq!(
        s.shapes.len(),
        1,
        "選択された左右両端が削除され中央の1つだけ残ること"
    );
    assert!(s.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(!s.shapes[0].contains_point(Point::new(110.0, 200.0)));
    assert!(!s.shapes[0].contains_point(Point::new(290.0, 200.0)));

    // 5. 1回の Undo でスライス前の元の1つのシェイプに完全復帰すること
    app.undo();
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 1);
    assert!(s.shapes[0].contains_point(Point::new(110.0, 200.0)));
    assert!(s.shapes[0].contains_point(Point::new(200.0, 200.0)));
    assert!(s.shapes[0].contains_point(Point::new(290.0, 200.0)));
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
    put_cel_shapes(&mut app, 0, 0, stroke);
    assert_eq!(scene_summary(&app).shapes.len(), 1);

    // 1. 選択ツールで左半分 (x: 70..200) を矩形選択
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 1);

    // 2. バケツツールに切り替えても選択状態が解除されないこと
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Bucket },
    );
    assert_eq!(
        app.session.selected_indices().len(),
        1,
        "バケツツールに切り替えても選択が維持されること"
    );

    // 3. 選択されていない右半分 (260, 200) をバケツでクリックしても塗られないこと
    app.tool.brush_color = blue;
    assert!(
        !doc_actions::bucket_fill_at(&mut app, Point::new(260.0, 200.0)),
        "選択外領域はバケツで塗られないこと"
    );

    // 4. 選択されている左半分 (130, 200) をバケツでクリックすると、選択部分だけが青に塗られること
    assert!(
        doc_actions::bucket_fill_at(&mut app, Point::new(130.0, 200.0)),
        "選択領域内をクリックするとバケツで色が流し込まれること"
    );
    assert!(
        app.scene()
            .iter()
            .any(|s| s.color == blue && s.contains_point(Point::new(130.0, 200.0))),
        "選択された左半分が青になっていること"
    );
    assert!(
        app.scene()
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
    put_cel_shapes(&mut app, 0, 0, stroke);
    assert_eq!(scene_summary(&app).shapes.len(), 1);
    app.session.set_selected_indices(vec![0]);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool {
            tool: Tool::Transform,
        },
    );

    // 1. +90° 回転 -> 横長だったシェイプが中心 (200, 200) を軸に縦長 (x: 200, y: 100..300) になること
    assert!(doc_actions::rotate_selected_shapes(
        &mut app,
        std::f64::consts::FRAC_PI_2
    ));
    let s0 = app.scene().get(0).unwrap();
    assert!(
        s0.contains_point(Point::new(200.0, 110.0)) && s0.contains_point(Point::new(200.0, 290.0)),
        "+90度回転で縦長シェイプになること"
    );
    assert!(
        !s0.contains_point(Point::new(110.0, 200.0)),
        "元の左右端にはシェイプが存在しないこと"
    );

    // 2. 縮小 (0.5倍) -> 上下端 (200, 110) は範囲外になり、(200, 155) 付近に縮小されること
    assert!(doc_actions::scale_selected_shapes(&mut app, 0.5, 0.5));
    let s0 = app.scene().get(0).unwrap();
    assert!(!s0.contains_point(Point::new(200.0, 110.0)));
    assert!(s0.contains_point(Point::new(200.0, 155.0)));

    // 3. カスタムピボット点 (200, 200) に対して、左側の点 (150, 200) にあるシェイプを左右反転 -> (250, 200) に移り、ワインディングも正常に保たれること
    canvas_tool::clear_selection(&mut app);
    let left_piece = make_stroke(
        Point::new(130.0, 200.0),
        Point::new(170.0, 200.0),
        18.0,
        red,
    );
    put_cel_shapes(&mut app, 0, 0, left_piece);
    app.session.set_selected_indices(vec![0]);
    app.session.transform_pivot = Some(Point::new(200.0, 200.0));

    assert!(doc_actions::flip_selected_shapes(&mut app, true, false));
    let s0 = app.scene().get(0).unwrap();
    assert!(
        !s0.contains_point(Point::new(150.0, 200.0)),
        "反転前の位置には存在しないこと"
    );
    assert!(
        s0.contains_point(Point::new(250.0, 200.0)),
        "ピボット (200, 200) を基準に左右対称の (250, 200) へ反転移動していること"
    );
    assert!(
        s0.exterior.signed_area() > 0.0,
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
fn test_group_selection_bucket_protection_and_non_destructive_overlap() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(65, 195, 105);

    let mut shapes = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        30.0,
        red,
    );
    shapes.extend(make_stroke(
        Point::new(90.0, 100.0),
        Point::new(110.0, 100.0),
        15.0,
        blue,
    ));
    shapes.extend(make_stroke(
        Point::new(135.0, 100.0),
        Point::new(155.0, 100.0),
        12.0,
        green,
    ));
    put_cel_shapes(&mut app, 0, 0, shapes);
    assert_eq!(scene_summary(&app).shapes.len(), 3);

    app.session.set_selected_indices(vec![1, 2]);
    let eye_gid = doc_actions::group_selected_shapes(&mut app).expect("グループ化が成功すること");
    assert_eq!(scene_summary(&app).shapes[1].group_path, vec![eye_gid]);
    assert_eq!(scene_summary(&app).shapes[2].group_path, vec![eye_gid]);

    // 1つクリックするだけでグループ全体の2シェイプが同時選択されること
    canvas_tool::clear_selection(&mut app);
    assert!(tool_select::select_single_shape_at(
        &mut app,
        Point::new(100.0, 100.0)
    ));
    assert_eq!(app.session.selected_indices().len(), 2);

    // グループ選択状態でバケツツールを使っても無効であること
    app.tool.brush_color = red;
    assert!(!doc_actions::bucket_fill_at(
        &mut app,
        Point::new(100.0, 100.0)
    ));
    assert_eq!(scene_summary(&app).shapes[1].color, blue);
    assert_eq!(scene_summary(&app).shapes[2].color, green);

    // グループを赤い顔シェイプの上へ移動して選択解除しても下の生シェイプを切り抜かないこと
    doc_actions::apply(
        &mut app,
        doc_actions::Action::MoveSelected {
            delta: KVec2::new(0.0, 100.0),
        },
    );
    canvas_tool::clear_selection(&mut app);
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 3);
    assert!(s.shapes[0].contains_point(Point::new(100.0, 200.0)));
}

#[test]
fn test_nested_grouping_edit_in_place_and_single_level_ungroup() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(65, 195, 105);

    let mut shapes = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        30.0,
        red,
    );
    shapes.extend(make_stroke(
        Point::new(90.0, 200.0),
        Point::new(110.0, 200.0),
        15.0,
        blue,
    ));
    shapes.extend(make_stroke(
        Point::new(135.0, 200.0),
        Point::new(155.0, 200.0),
        12.0,
        green,
    ));
    put_cel_shapes(&mut app, 0, 0, shapes);

    app.session.set_selected_indices(vec![1, 2]);
    let eye_gid = doc_actions::group_selected_shapes(&mut app).unwrap();
    app.session.set_selected_indices(vec![0, 1, 2]);
    let head_gid = doc_actions::group_selected_shapes(&mut app).unwrap();
    let s = scene_summary(&app);
    assert_eq!(s.shapes[0].group_path, vec![head_gid]);
    assert_eq!(s.shapes[1].group_path, vec![head_gid, eye_gid]);
    assert_eq!(s.shapes[2].group_path, vec![head_gid, eye_gid]);

    assert!(select_ops::enter_group_at(
        &mut app,
        Point::new(200.0, 200.0)
    ));
    assert_eq!(app.session.editing_group_path, vec![head_gid]);
    let s = scene_summary(&app);
    assert!(s.shapes[0].is_raw_at_scope(&app.session.editing_group_path));
    assert_eq!(
        s.shapes[1].active_group_id_at_scope(&app.session.editing_group_path),
        Some(eye_gid)
    );

    assert!(select_ops::enter_group_at(
        &mut app,
        Point::new(100.0, 200.0)
    ));
    assert_eq!(app.session.editing_group_path, vec![head_gid, eye_gid]);
    assert!(scene_summary(&app).shapes[1].is_raw_at_scope(&app.session.editing_group_path));

    assert!(select_ops::exit_current_group(&mut app));
    assert_eq!(app.session.editing_group_path, vec![head_gid]);
    assert!(select_ops::exit_current_group(&mut app));
    assert!(app.session.editing_group_path.is_empty());
    assert_eq!(app.session.selected_indices().len(), 3);

    assert!(doc_actions::ungroup_selected_shapes(&mut app));
    let cel_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
    assert!(cel_shapes[0].group_path.is_empty());
    assert_eq!(cel_shapes[1].group_path, vec![eye_gid]);
    assert_eq!(cel_shapes[2].group_path, vec![eye_gid]);
    let s = scene_summary(&app);
    assert!(s
        .shapes
        .iter()
        .find(|sh| sh.color == red)
        .unwrap()
        .group_path
        .is_empty());
    assert_eq!(
        s.shapes
            .iter()
            .find(|sh| sh.color == blue)
            .unwrap()
            .group_path,
        vec![eye_gid]
    );
    assert_eq!(
        s.shapes
            .iter()
            .find(|sh| sh.color == green)
            .unwrap()
            .group_path,
        vec![eye_gid]
    );
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
    doc_actions::commit_raw_outline(&mut app, &fine_raw);
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
    put_cel_shapes(&mut app, 0, 0, stroke);
    assert_eq!(scene_summary(&app).shapes.len(), 1);

    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert!(doc_actions::cut_selected_shapes(&mut app));
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 1);
    assert!(!s.shapes[0].contains_point(Point::new(120.0, 200.0)));
    assert!(s.shapes[0].contains_point(Point::new(260.0, 200.0)));

    assert!(doc_actions::paste_clipboard(&mut app, true));
    assert_eq!(app.session.selected_indices().len(), 1);
    assert!(app
        .scene()
        .get(app.session.selected_indices()[0])
        .unwrap()
        .contains_point(Point::new(120.0, 200.0)));
    canvas_tool::clear_selection(&mut app);
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 1);
    assert!(s.shapes[0].contains_point(Point::new(120.0, 200.0)));
    assert!(s.shapes[0].contains_point(Point::new(260.0, 200.0)));
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
    put_cel_shapes(&mut app, 0, 0, g_part);
    app.session.set_selected_indices(vec![0]);
    let orig_gid = doc_actions::group_selected_shapes(&mut app).unwrap();

    assert!(doc_actions::copy_selected_shapes(&mut app));
    // 高ズーム時でもワールド座標上の一定オフセット (+16, +16) で貼り付けられること
    app.view.zoom = 4.0;
    assert!(doc_actions::paste_clipboard(&mut app, false));
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 2);
    let pasted_idx = app.session.selected_indices()[0];
    let pasted_gid = s.shapes[pasted_idx].group_path[0];
    assert_ne!(pasted_gid, orig_gid);
    assert!(
        s.shapes[pasted_idx].contains_point(Point::new(120.0 + 16.0, 100.0 + 16.0)),
        "Cmd+V ではズーム倍率によらずワールド座標 (+16, +16) ずれた位置に貼り付けられること"
    );

    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    let orig_inst_gid = symbol_ops::selected_single_symbol_info(&app)
        .unwrap()
        .0
        .instance_gid;
    let count_before = scene_summary(&app).shapes.len();

    let move_drag =
        clipboard_ops::begin_move_selection_drag(&mut app, Point::new(136.0, 116.0), true);
    assert_eq!(app.scene().len(), count_before + 1);
    app.session.translate_move_overlay(KVec2::new(60.0, 40.0));
    doc_actions::finish_move_selection_drag(
        &mut app,
        KVec2::new(60.0, 40.0),
        move_drag.alt_duplicate,
    );

    let (dup_binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    assert_eq!(dup_binding.symbol_id, sym_id);
    assert_ne!(dup_binding.instance_gid, orig_inst_gid);
    assert!(
        scene_summary(&app).shapes[app.session.selected_indices()[0]]
            .contains_point(Point::new(120.0 + 16.0 + 60.0, 100.0 + 16.0 + 40.0)),
    );
    app.undo();
    assert_eq!(scene_summary(&app).shapes.len(), count_before);
}

#[test]
fn test_timeline_layer_keyframe_insertion_and_cel_lookup() {
    let mut app = Is2dApp::default();
    assert_eq!(app.doc.fps, 24);
    assert_eq!(app.doc.root.total_frames, 1);
    assert_eq!(app.doc.root.layers.len(), 1);
    assert_eq!(app.doc.root.layers[0].frame_count, 1);
    assert_eq!(app.doc.root.current_frame, 0);

    let red = Color32::from_rgb(235, 75, 75);
    let stroke = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(160.0, 100.0),
        12.0,
        red,
    );
    put_cel_shapes(&mut app, 0, 0, stroke);
    assert_eq!(app.doc.root.active_cel_shapes().len(), 1);

    app.doc.root.layers[0].insert_keyframe(4, false);
    assert_eq!(app.doc.root.layers[0].frame_count, 5);
    assert_eq!(app.doc.root.layers[0].keyframe_index_at(3), Some(0));
    assert_eq!(app.doc.root.layers[0].keyframe_index_at(4), Some(1));
    assert_eq!(app.doc.root.layers[0].cel_at(2).unwrap().shapes.len(), 1);
    assert!(app.doc.root.layers[0].cel_at(4).unwrap().is_empty());

    app.doc.root.layers[0].insert_keyframe(3, true);
    assert_eq!(app.doc.root.layers[0].cel_at(3).unwrap().shapes.len(), 1);
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
    put_cel_shapes(&mut app, 0, 0, s0);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert_eq!(app.doc.root.current_frame, 1);
    assert_eq!(scene_summary(&app).shapes.len(), 1);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 2);
    assert!(scene_summary(&app).shapes.is_empty());

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
    assert_eq!(scene_summary(&app).shapes.len(), 1);
    app.undo();
    assert!(scene_summary(&app).shapes.is_empty());
    app.redo();
    assert_eq!(scene_summary(&app).shapes.len(), 1);
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
    put_cel_shapes(&mut app, 0, 0, s0);

    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.doc.root.layers.len(), 2);
    assert_eq!(app.doc.root.active_layer, 1);
    assert!(app.doc.root.active_cel_shapes().is_empty());
    assert_eq!(scene_summary(&app).shapes.len(), 1);
    app.tool.brush_color = blue;
    assert_eq!(
        tool_paint::pick_color_at(&mut app, Point::new(125.0, 100.0)),
        Some(red)
    );

    app.tool.brush_color = blue;
    doc_actions::commit_raw_outline(
        &mut app,
        &[
            Point::new(120.0, 70.0),
            Point::new(120.0, 130.0),
            Point::new(135.0, 130.0),
            Point::new(135.0, 70.0),
        ],
    );
    assert_eq!(app.doc.root.layers[0].cel_at(0).unwrap().shapes.len(), 1);
    assert_eq!(app.doc.root.layers[1].cel_at(0).unwrap().shapes.len(), 1);
    assert_eq!(scene_summary(&app).shapes.len(), 2);

    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(200.0, 200.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 2);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::MoveSelected {
            delta: KVec2::new(30.0, 0.0),
        },
    );
    canvas_tool::clear_selection(&mut app);
    assert!(app.doc.root.layers[0].cel_at(0).unwrap().shapes[0]
        .contains_point(Point::new(155.0, 100.0)));
    assert!(app.doc.root.layers[1].cel_at(0).unwrap().shapes[0]
        .contains_point(Point::new(155.0, 100.0)));

    timeline_ui::set_layer_locked(&mut app, 0, true);
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(250.0, 200.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 1);
    canvas_tool::clear_selection(&mut app);
    timeline_ui::set_layer_locked(&mut app, 0, false);
    assert_eq!(scene_summary(&app).shapes.len(), 2);
}

#[test]
fn test_timeline_layer_reorder_move_and_delete() {
    let mut app = Is2dApp::default();
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.doc.root.layers.len(), 3);
    assert_eq!(
        app.doc
            .root
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 1", "Layer 2", "Layer 3"]
    );

    assert!(timeline_ui::timeline_reorder_layer(&mut app, 2, 0));
    assert_eq!(app.doc.root.active_layer, 0);
    assert_eq!(
        app.doc
            .root
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 3", "Layer 1", "Layer 2"]
    );
    app.undo();
    assert_eq!(
        app.doc
            .root
            .layers
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Layer 1", "Layer 2", "Layer 3"]
    );
    app.redo();
    assert_eq!(
        app.doc
            .root
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
    doc_actions::commit_raw_outline(
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
    doc_actions::commit_raw_outline(
        &mut app,
        &[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ],
    );
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
        scene_summary(&app).shapes[app.session.selected_indices()[0]].layer_id,
        app.doc.root.layers[0].id
    );

    canvas_tool::clear_selection(&mut app);
    timeline_ui::scrub_ruler_to_frame(&mut app, 0);
    assert!(tool_select::select_single_shape_at(
        &mut app,
        Point::new(160.0, 60.0)
    ));
    assert_eq!(app.doc.root.active_layer, 1);
    assert_eq!(
        app.doc
            .root
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
    doc_actions::commit_raw_outline(
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
    doc_actions::commit_raw_outline(
        &mut app,
        &[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ],
    );
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
    assert_eq!(
        app.doc.root.layers[1].keyframes.last().unwrap().start_frame,
        5
    );
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

    let m1 = app
        .doc
        .root
        .resolved_cel_screen_mesh(
            1,
            0,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &app.doc.library,
        )
        .unwrap();
    let m2 = app
        .doc
        .root
        .resolved_cel_screen_mesh(
            1,
            0,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &app.doc.library,
        )
        .unwrap();
    assert!(!m1.is_empty());
    assert!(!std::sync::Arc::ptr_eq(&m1, &m2) && m1.vertices == m2.vertices);
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    app.doc.root.current_frame = 0;
    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing);
    assert_eq!(scene_summary(&app).shapes.len(), 1);
}

#[test]
fn test_adobe_animate_click_selects_edge_and_selected_shape_disables_deformation() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );

    // 細い 2px ブラシストロークを描画（ほぼ全域が境界線の吸着範囲内になる細い線）
    let thin_stroke = make_stroke(Point::new(100.0, 200.0), Point::new(220.0, 200.0), 2.0, red);
    put_cel_shapes(&mut app, 0, 0, thin_stroke);
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 1);

    let vertex_pt = s.shapes[0].exterior.segments[0].p0;
    let edge_pt = s.shapes[0].exterior.segments[0].eval(0.5);

    let s0_id = s.shapes[0].shape_id;

    // 1. 未選択状態では、頂点や境界線にホバーすると Vertex / Edge（変形対象）になること
    assert!(matches!(
        tool_select::compute_hover_target(
            app.scene(),
            &app.session,
            vertex_pt,
            app.view.zoom,
            app.tool.select_sub_mode
        ),
        HoverTarget::Vertex { shape_id, .. } if shape_id == s0_id
    ));
    assert!(matches!(
        tool_select::compute_hover_target(
            app.scene(),
            &app.session,
            edge_pt,
            app.view.zoom,
            app.tool.select_sub_mode
        ),
        HoverTarget::Edge { shape_id, .. } | HoverTarget::Vertex { shape_id, .. } if shape_id == s0_id
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
        HoverTarget::Fill { shape_id: s0_id },
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
        HoverTarget::Fill { shape_id: s0_id },
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
        HoverTarget::Fill { shape_id: s0_id },
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 150.0),
            Point::new(200.0, 150.0),
            16.0,
            Color32::from_rgb(230, 80, 80),
        ),
    );
    app.session.set_selected_indices(vec![0]);

    let sym_id =
        doc_actions::convert_selection_to_symbol(&mut app).expect("シンボル変換が成功すること");
    assert_eq!(app.doc.library.len(), 1);
    assert_eq!(app.doc.library[0].id, sym_id);
    assert_eq!(app.doc.library[0].name, "Symbol 1");
    assert!(symbol_ops::selected_single_symbol_info(&app).is_some());

    assert!(doc_actions::place_symbol_instance_from_library(
        &mut app, sym_id
    ));
    assert_eq!(scene_summary(&app).len, 2);
    assert_ne!(
        scene_summary(&app).shapes[0]
            .symbol_instance
            .unwrap()
            .instance_gid,
        scene_summary(&app).shapes[1]
            .symbol_instance
            .unwrap()
            .instance_gid
    );

    assert!(select_ops::enter_group_at(
        &mut app,
        Point::new(150.0, 150.0)
    ));
    assert_eq!(app.session.edit_path.len(), 1);
    assert_eq!(app.active_timeline().total_frames, 1);

    assert!(timeline_ui::timeline_insert_keyframe(&mut app, true));
    assert_eq!(app.active_timeline().total_frames, 2);
    assert_eq!(app.active_timeline().current_frame, 1);

    let hat = make_stroke(
        Point::new(120.0, 80.0),
        Point::new(180.0, 80.0),
        10.0,
        Color32::from_rgb(50, 120, 240),
    )
    .remove(0);
    let mut shapes = scene_summary(&app).shapes;
    shapes.push(hat);
    put_cel_shapes(&mut app, 0, 1, shapes);

    assert!(select_ops::exit_current_group(&mut app));
    assert!(app.session.edit_path.is_empty());
    assert_eq!(app.doc.library[0].timeline.total_frames, 2);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.total_frames, 3);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(scene_summary(&app).len, 2);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(scene_summary(&app).len, 4);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(scene_summary(&app).len, 2);
}

#[test]
fn test_classic_tween_requires_single_symbol_instance() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    assert!(!doc_actions::timeline_toggle_tween(&mut app));

    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 150.0),
            Point::new(200.0, 150.0),
            16.0,
            Color32::from_rgb(230, 80, 80),
        ),
    );
    assert!(!doc_actions::timeline_toggle_tween(&mut app));

    app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::group_selected_shapes(&mut app).is_some());
    assert!(!doc_actions::timeline_toggle_tween(&mut app));
    assert!(doc_actions::ungroup_selected_shapes(&mut app));

    app.session.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut app));
    assert!(app.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete,
    );

    assert!(doc_actions::timeline_toggle_tween(&mut app));
    assert!(!app.doc.root.layers[0].keyframes[0].tween);

    assert!(doc_actions::place_symbol_instance_from_library(
        &mut app, sym_id
    ));
    assert!(!doc_actions::timeline_toggle_tween(&mut app));
    assert!(doc_actions::delete_selected_shapes(&mut app));

    let raw_extra = make_stroke(
        Point::new(300.0, 300.0),
        Point::new(340.0, 300.0),
        8.0,
        Color32::BLACK,
    )
    .remove(0);
    let mut shapes = scene_summary(&app).shapes;
    shapes.push(raw_extra);
    put_cel_shapes(&mut app, 0, 0, shapes);
    assert!(!doc_actions::timeline_toggle_tween(&mut app));
}

#[test]
fn test_classic_tween_translation_and_custom_pivot_ccw_rotation() {
    use crate::symbol::TweenState;

    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 150.0),
            Point::new(200.0, 150.0),
            16.0,
            Color32::from_rgb(230, 80, 80),
        ),
    );
    app.session.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut app));
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
    doc_actions::nudge_selected_shapes(&mut app, KVec2::new(200.0, 80.0));

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_bbox = scene_summary(&app).shapes[0].bounding_box();
    let mid_cx = (mid_bbox.x0 + mid_bbox.x1) * 0.5;
    let mid_cy = (mid_bbox.y0 + mid_bbox.y1) * 0.5;
    assert!((mid_cx - 250.0).abs() < 1.0);
    assert!((mid_cy - 190.0).abs() < 1.0);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    let start_bbox = scene_summary(&app).shapes[0].bounding_box();
    let start_cx = (start_bbox.x0 + start_bbox.x1) * 0.5;
    let start_cy = (start_bbox.y0 + start_bbox.y1) * 0.5;
    assert!((start_cx - 150.0).abs() < 1.0);
    assert!((start_cy - 150.0).abs() < 1.0);

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    app.session.set_selected_indices(vec![0]);
    doc_actions::nudge_selected_shapes(&mut app, KVec2::new(-200.0, -80.0));

    let joint_world = Point::new(100.0, 150.0);
    assert!(doc_actions::set_selected_symbol_world_pivot(
        &mut app,
        Some(joint_world),
        true
    ));
    canvas_tool::clear_selection(&mut app);
    app.session.set_selected_indices(vec![0]);
    let bbox_f4 = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();
    assert!(
        (app.scene().active_transform_pivot(
            &app.session.selection,
            app.session.transform_pivot,
            bbox_f4
        ) - joint_world)
            .hypot()
            < 1e-4
    );
    assert!(doc_actions::rotate_selected_shapes(
        &mut app,
        std::f64::consts::FRAC_PI_2
    ));

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_binding = scene_summary(&app).shapes[0].symbol_instance.unwrap();
    assert!((mid_binding.world_pivot() - joint_world).hypot() < 1e-4);
    let expected_mid_center = Point::new(
        100.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        150.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
    );
    assert!(scene_summary(&app).shapes[0].contains_point(expected_mid_center));

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert!(app.view.inspect_timeline_frame);
    tool_select::select_single_shape_at(&mut app, Point::new(150.0, 150.0));
    assert!(!app.view.inspect_timeline_frame);

    timeline_ui::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert!(doc_actions::set_active_keyframe_tween_rotate(
        &mut app,
        crate::symbol::TweenRotate::CounterClockwise
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    let expected_ccw_mid_center = Point::new(
        100.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        150.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
    );
    assert!(scene_summary(&app).shapes[0].contains_point(expected_ccw_mid_center));
}

#[test]
fn test_symbol_playback_modes_loop_play_once_and_single_frame() {
    use crate::symbol::SymbolLoopMode;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(60, 190, 90);
    let blue = Color32::from_rgb(60, 110, 230);
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            red,
        ),
    );
    app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());

    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    assert!(symbol_ops::enter_symbol_timeline(&mut app, binding));

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    put_cel_shapes(
        &mut app,
        0,
        1,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            green,
        ),
    );

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    put_cel_shapes(
        &mut app,
        0,
        2,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            blue,
        ),
    );
    assert_eq!(app.active_timeline().total_frames, 3);

    assert!(symbol_ops::exit_symbol_timeline(&mut app));
    for _ in 0..4 {
        assert!(timeline_ui::timeline_extend_frame(&mut app));
    }
    assert_eq!(app.doc.root.total_frames, 5);

    let expected_loop = [red, green, blue, red, green];
    for (f, &col) in expected_loop.iter().enumerate() {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(
            scene_summary(&app).shapes[0].color,
            col,
            "Loop mode at frame {f}"
        );
    }

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::PlayOnce,
        0
    ));
    let expected_once = [red, green, blue, blue, blue];
    for (f, &col) in expected_once.iter().enumerate() {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(
            scene_summary(&app).shapes[0].color,
            col,
            "PlayOnce mode at frame {f}"
        );
    }

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::SingleFrame,
        1
    ));
    assert_eq!(scene_summary(&app).shapes[0].color, green);
    assert_eq!(app.session.selected_indices(), vec![0]);
    for f in 0..5 {
        timeline_ui::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(scene_summary(&app).shapes[0].color, green);
    }
}

#[test]
fn test_nested_symbol_recursive_frame_evaluation_and_blank_frame_marker() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 110, 230);

    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            red,
        ),
    );
    app.session.set_selected_indices(vec![0]);
    let child_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    let (child_binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    assert!(symbol_ops::enter_symbol_timeline(&mut app, child_binding));
    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    put_cel_shapes(
        &mut app,
        0,
        1,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            blue,
        ),
    );
    assert!(symbol_ops::exit_symbol_timeline(&mut app));

    app.session.set_selected_indices(vec![0]);
    let parent_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert_ne!(child_sym_id, parent_sym_id);

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(scene_summary(&app).shapes[0].color, red);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(scene_summary(&app).shapes[0].color, blue);

    assert!(symbol_ops::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.active_timeline().total_frames, 3);
    assert!(symbol_ops::exit_symbol_timeline(&mut app));

    assert!(timeline_ui::timeline_extend_frame(&mut app));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(scene_summary(&app).len, 1);
    assert!(scene_summary(&app).shapes[0].is_empty_symbol_marker());
    canvas_tool::select_shapes_for_timeline_selection(&mut app);
    assert_eq!(app.session.selected_indices(), vec![0]);
    assert!(app
        .scene()
        .selected_single_symbol_binding(&app.session.selection)
        .is_some());
    canvas_tool::clear_selection(&mut app);
    assert!(app
        .doc
        .root
        .resolved_cel_screen_mesh(
            0,
            2,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &app.doc.library,
        )
        .is_none());

    let extra = make_stroke(
        Point::new(220.0, 100.0),
        Point::new(260.0, 100.0),
        8.0,
        Color32::BLACK,
    )
    .remove(0);
    let mut shapes = scene_summary(&app).shapes;
    shapes.push(extra);
    put_cel_shapes(&mut app, 0, 2, shapes);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(scene_summary(&app).len, 2);
}

#[test]
fn test_symbol_rename_library_delete_detach_and_inplace_child_symbol_creation() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            12.0,
            red,
        ),
    );
    app.session.set_selected_indices(vec![0]);
    let child_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    app.session.set_selected_indices(vec![0]);
    let parent_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();

    let orig_name = app.doc.find_symbol(parent_sym_id).unwrap().name.clone();
    assert!(doc_actions::rename_symbol_in_library(
        &mut app,
        parent_sym_id,
        "HeroSymbol".to_string()
    ));
    assert_eq!(
        app.doc.find_symbol(parent_sym_id).unwrap().name,
        "HeroSymbol"
    );
    app.undo();
    assert_eq!(app.doc.find_symbol(parent_sym_id).unwrap().name, orig_name);

    timeline_ui::set_layer_locked(&mut app, 0, true);
    assert!(app.doc.root.layers[0].locked);
    app.undo();
    assert!(!app.doc.root.layers[0].locked);

    // 子シンボル (child_sym_id) を削除すると、親シンボル (parent_sym_id) 内の child_sym_id インスタンスも消え、Undo 1回で元に戻ること
    assert!(doc_actions::delete_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    assert!(app.doc.find_symbol(child_sym_id).is_none());
    assert!(
        app.doc.find_symbol(parent_sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .instances
            .is_empty(),
        "子シンボルを削除すると親シンボル内の子シンボルインスタンスも消えること"
    );
    app.undo();
    assert!(app.doc.find_symbol(child_sym_id).is_some());
    assert_eq!(
        app.doc.find_symbol(parent_sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .instances
            .len(),
        1,
        "Undo 1回で子シンボルと親シンボル内のインスタンスが元に戻ること"
    );

    // 親シンボル (parent_sym_id) を削除すると、ルート上のインスタンスも消えること
    assert!(doc_actions::delete_symbol_from_library(
        &mut app,
        parent_sym_id
    ));
    assert_eq!(scene_summary(&app).len, 0);
    assert!(app.doc.root.layers[0].keyframes[0].cel.instances.is_empty());

    assert!(symbol_ops::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    app.session.set_selected_indices(vec![0]);
    let lib_len_before = app.doc.library.len();
    let grandchild_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
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
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(Point::new(50.0, 100.0), Point::new(200.0, 100.0), 14.0, red),
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(80.0, 70.0),
        Point::new(130.0, 130.0),
        false,
    );
    assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
    let (b, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    assert!(symbol_ops::enter_symbol_timeline(&mut app, b));
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
    assert!(!symbol_ops::exit_symbol_timeline(&mut app));
    assert!(!select_ops::exit_current_group(&mut app));
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

    let clean_bbox = scene_summary(&app).shapes[0].bounding_box();
    app.session.set_selected_indices(vec![0]);
    let move_drag = clipboard_ops::begin_move_selection_drag(&mut app, Point::ZERO, false);
    app.session.drag = crate::canvas_tool::ActiveCanvasDrag::Select(
        crate::canvas_tool::SelectDragState::MoveShape(move_drag),
    );
    app.session.translate_move_overlay(KVec2::new(500.0, 500.0));
    canvas_tool::clear_selection(&mut app);
    assert_eq!(app.scene().get(0).unwrap().bounding_box(), clean_bbox);

    let stroke = vec![Point::new(10.0, 10.0), Point::new(40.0, 10.0)];
    let tail_outline = generate_brush_outline(&stroke, 8.0);
    let tail_mesh = crate::vector_engine::tessellate_raw_polygon(&tail_outline, Color32::BLACK);
    app.session.drag = crate::canvas_tool::ActiveCanvasDrag::PaintStroke {
        stroke,
        committed_world_mesh: egui::Mesh::default(),
        tail_outline,
        tail_mesh,
        tail_start_idx: 0,
    };
    canvas_tool::clear_selection(&mut app);
    assert!(matches!(
        app.session.drag,
        crate::canvas_tool::ActiveCanvasDrag::PaintStroke { .. }
    ));
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Brush });
    assert!(matches!(
        app.session.drag,
        crate::canvas_tool::ActiveCanvasDrag::None
    ));

    let b = scene_summary(&app)
        .shapes
        .iter()
        .find_map(|s| s.symbol_instance)
        .unwrap();
    let sym_id = b.symbol_id;
    let orig_local_shape = app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .clone();
    let orig_rev = orig_local_shape.mesh_revision;
    for _ in 0..10 {
        assert!(symbol_ops::enter_symbol_timeline(&mut app, b));
        assert!(symbol_ops::exit_symbol_timeline(&mut app));
    }
    let after_local_shape = &app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes[0];
    assert!(std::sync::Arc::ptr_eq(
        &after_local_shape.exterior.segments,
        &orig_local_shape.exterior.segments
    ));
    assert_eq!(after_local_shape.mesh_revision, orig_rev);
}

#[test]
fn test_timeline_resize_total_frames_and_fill_shape_lazy_retessellation() {
    use crate::doc_actions::{apply, Action};

    // M2-5: 新しい作品で 2,880 コマ目 (index 2879) を選んで F5 を押すと、1回で 2,880 フレームになること
    let mut app = Is2dApp::default();
    assert_eq!(app.doc.root.total_frames, 1);
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2_879,
        },
    );
    let _ = apply(&mut app, Action::ExtendFrame);
    assert_eq!(app.doc.root.total_frames, 2_880);
    assert_eq!(app.doc.root.layers[0].frame_count, 2_880);

    // 2,880 フレームの作品で 2,001〜2,880 コマ目 (index 2000..=2879) を選んで Shift+F5 を押すと、
    // 1回で 2,000 フレームになり、現在コマ・選択が 2,000 コマ目 (index 1999) の1コマになり、Undo 1回で元に戻ること
    let _ = apply(
        &mut app,
        Action::SelectTimelineCells {
            l0: 0,
            f0: 2_000,
            l1: 0,
            f1: 2_879,
        },
    );
    let undo_before = app.history.undo_stack.len();
    assert!(apply(&mut app, Action::RemoveFrame));
    assert_eq!(app.history.undo_stack.len(), undo_before + 1);
    assert_eq!(app.doc.root.total_frames, 2_000);
    assert_eq!(app.doc.root.layers[0].frame_count, 2_000);
    assert_eq!(app.doc.root.current_frame, 1_999);
    assert_eq!(
        app.doc.root.cell_selection,
        Some(timeline::TimelineCellSelection::single(0, 1_999))
    );

    let _ = apply(&mut app, Action::Undo);
    assert_eq!(app.doc.root.total_frames, 2_880);
    assert_eq!(app.doc.root.layers[0].frame_count, 2_880);

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
    put_cel_shapes(&mut sym_app, 0, 0, two_shapes);
    sym_app.session.set_selected_indices(vec![0, 1]);
    assert_eq!(sym_app.session.selection.shapes.len(), 2);
    assert!(sym_app.session.selection.instances.is_empty());
    let empty_test_sym_id = doc_actions::convert_selection_to_symbol(&mut sym_app).unwrap();
    assert!(sym_app.session.selection.shapes.is_empty());
    assert_eq!(sym_app.session.selection.instances.len(), 1);
    assert_eq!(scene_summary(&sym_app).len, 2);
    assert!(sym_app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes
        .is_empty());
    assert_eq!(
        sym_app.doc.root.layers[0].keyframes[0].cel.instances.len(),
        1
    );
    assert!(symbol_ops::enter_symbol_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    assert!(doc_actions::clear_active_frame_shapes(&mut sym_app));
    assert!(symbol_ops::exit_symbol_timeline(&mut sym_app));

    assert_eq!(scene_summary(&sym_app).len, 1);
    assert!(scene_summary(&sym_app).shapes[0].is_empty_symbol_marker());
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
    assert!(doc_actions::place_symbol_instance_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    assert_eq!(scene_summary(&sym_app).len, 2);
    assert!(scene_summary(&sym_app).shapes[1].is_empty_symbol_marker());
    sym_app.undo();
    assert_eq!(scene_summary(&sym_app).len, 1);

    assert!(symbol_ops::enter_symbol_from_library(
        &mut sym_app,
        empty_test_sym_id
    ));
    put_cel_shapes(
        &mut sym_app,
        0,
        0,
        make_stroke(
            Point::new(210.0, 200.0),
            Point::new(250.0, 200.0),
            8.0,
            Color32::from_rgb(60, 180, 90),
        ),
    );
    assert!(symbol_ops::exit_symbol_timeline(&mut sym_app));
    assert_eq!(scene_summary(&sym_app).len, 1);
    assert!(scene_summary(&sym_app).shapes[0].contains_point(Point::new(230.0, 200.0)));

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
    assert!(std::sync::Arc::ptr_eq(
        &last_undo.library[0].timeline.layers[0].keyframes[0],
        &sym_app.doc.library[0].timeline.layers[0].keyframes[0]
    ));
    sym_app.undo();
    assert_eq!(sym_app.doc.stage_size, orig_stage);
    sym_app.redo();
    assert_eq!(sym_app.doc.stage_size, Vec2::new(1920.0, 1080.0));

    let sel_m1 = sym_app
        .scene()
        .get(0)
        .unwrap()
        .cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
    let sel_m2 = sym_app
        .scene()
        .get(0)
        .unwrap()
        .cached_selection_screen_mesh(1.25, Vec2::new(12.0, -8.0));
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
    put_cel_shapes(
        &mut nest_app,
        0,
        0,
        make_stroke(
            Point::new(400.0, 225.0),
            Point::new(440.0, 225.0),
            8.0,
            Color32::from_rgb(100, 100, 100),
        ),
    );
    nest_app.session.set_selected_indices(vec![0]);
    let sym_a = doc_actions::convert_selection_to_symbol(&mut nest_app).unwrap();

    let mut shapes = scene_summary(&nest_app).shapes;
    shapes.extend(make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(150, 150, 150),
    ));
    put_cel_shapes(&mut nest_app, 0, 0, shapes);
    let last_idx = scene_summary(&nest_app).len - 1;
    nest_app.session.set_selected_indices(vec![last_idx]);
    let sym_b = doc_actions::convert_selection_to_symbol(&mut nest_app).unwrap();
    doc_actions::delete_selected_shapes(&mut nest_app);

    let mut shapes = scene_summary(&nest_app).shapes;
    shapes.extend(make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(200, 50, 50),
    ));
    put_cel_shapes(&mut nest_app, 0, 0, shapes);
    let last_idx = scene_summary(&nest_app).len - 1;
    nest_app.session.set_selected_indices(vec![last_idx]);
    let sym_c = doc_actions::convert_selection_to_symbol(&mut nest_app).unwrap();
    doc_actions::delete_selected_shapes(&mut nest_app);
    assert!(sym_a < sym_b && sym_b < sym_c);

    assert!(symbol_ops::enter_symbol_from_library(&mut nest_app, sym_a));
    assert!(doc_actions::clear_active_frame_shapes(&mut nest_app));
    assert!(doc_actions::place_symbol_instance_from_library(
        &mut nest_app,
        sym_b
    ));
    assert!(symbol_ops::exit_symbol_timeline(&mut nest_app));

    assert!(symbol_ops::enter_symbol_from_library(&mut nest_app, sym_b));
    assert!(doc_actions::clear_active_frame_shapes(&mut nest_app));
    assert!(doc_actions::place_symbol_instance_from_library(
        &mut nest_app,
        sym_c
    ));
    assert!(symbol_ops::exit_symbol_timeline(&mut nest_app));

    let updated_green = Color32::from_rgb(20, 210, 90);
    assert!(symbol_ops::enter_symbol_from_library(&mut nest_app, sym_c));
    let mut shapes = scene_summary(&nest_app).shapes;
    for s in &mut shapes {
        s.set_color(updated_green);
    }
    put_cel_shapes(&mut nest_app, 0, 0, shapes);
    assert!(symbol_ops::exit_symbol_timeline(&mut nest_app));
    assert_eq!(scene_summary(&nest_app).len, 1);
    assert_eq!(scene_summary(&nest_app).shapes[0].color, updated_green);

    let scaled_binding =
        crate::symbol::SymbolInstanceBinding::new(999, sym_c, kurbo::Affine::scale(3.0));
    let sym_c_def = nest_app.doc.find_symbol(sym_c).unwrap();
    let inst_shapes = scaled_binding.instantiate_shapes(
        &sym_c_def.timeline.layers[0].keyframes[0].cel.shapes,
        1,
        &[],
        true,
    );
    assert!(inst_shapes[0].has_cached_fill_mesh());

    assert!(symbol_ops::enter_symbol_from_library(&mut nest_app, sym_b));
    assert!(!nest_app.can_place_symbol_in_current_context(sym_a));
    assert!(!doc_actions::place_symbol_instance_from_library(
        &mut nest_app,
        sym_a
    ));
    assert!(symbol_ops::exit_symbol_timeline(&mut nest_app));
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
    tween_eye_app.session.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut tween_eye_app).unwrap();
    doc_actions::timeline_toggle_tween(&mut tween_eye_app);
    timeline_ui::select_timeline_cells(&mut tween_eye_app, 0, 4, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut tween_eye_app, true);
    doc_actions::nudge_selected_shapes(&mut tween_eye_app, kurbo::Vec2::new(200.0, 0.0));
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
    put_cel_shapes(
        &mut grp_sym_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(120.0, 100.0),
            10.0,
            Color32::from_rgb(200, 40, 40),
        ),
    );
    grp_sym_app.session.set_selected_indices(vec![0]);
    let blink_sym_id = doc_actions::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
    // シンボル内部に入り、2フレーム目を空白キーフレーム（F7）にする（F0: 表示, F1: 空白）
    let blink_binding = scene_summary(&grp_sym_app).shapes[0]
        .symbol_instance
        .unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut grp_sym_app,
        blink_binding
    ));
    assert!(timeline_ui::timeline_insert_keyframe(
        &mut grp_sym_app,
        false
    ));
    assert!(symbol_ops::exit_symbol_timeline(&mut grp_sym_app));

    // ルートシーンに別の生シェイプを追加し、シンボルと一緒に Cmd+G でグループ化する
    let sib_shapes = make_stroke(
        Point::new(100.0, 160.0),
        Point::new(120.0, 160.0),
        10.0,
        Color32::from_rgb(40, 120, 220),
    );
    let mut shapes = scene_summary(&grp_sym_app).shapes;
    shapes.extend(sib_shapes);
    put_cel_shapes(&mut grp_sym_app, 0, 0, shapes);
    grp_sym_app.session.set_selected_indices(vec![0, 1]);
    assert!(doc_actions::group_selected_shapes(&mut grp_sym_app).is_some());
    // ルートシーンを3フレームに延長（F0: シンボル表示, F1: シンボル空白, F2: シンボルループ再表示）
    timeline_ui::select_timeline_cells(&mut grp_sym_app, 0, 2, 0, 2);
    assert!(timeline_ui::timeline_extend_frame(&mut grp_sym_app));

    // F1（シンボルが空白フレームで消えている状態）へ移動し、親グループを (+150, 0) 平行移動する
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    assert_eq!(
        scene_summary(&grp_sym_app).len,
        2,
        "F1では空白シンボルマーカー1個と兄弟シェイプ1個の計2個が Scene 内に展開されていること"
    );
    {
        let sel = grp_sym_app
            .scene()
            .expand_to_group_unit(0, &grp_sym_app.session.editing_group_path);
        grp_sym_app.session.set_selected_indices(sel);
    }
    assert_eq!(grp_sym_app.session.selected_indices(), vec![0, 1]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut grp_sym_app,
        kurbo::Vec2::new(150.0, 0.0)
    ));

    // F2（シンボルがループして再び表示されるフレーム）へ進むと、空白フレーム中に移動した親グループに追従してシンボルも (+150, 0) 移動していること
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    assert_eq!(scene_summary(&grp_sym_app).len, 2);
    let summary_f2 = scene_summary(&grp_sym_app);
    let moved_sym_shape = summary_f2
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
    doc_actions::apply(
        &mut grp_sym_app,
        doc_actions::Action::EndCoalesce { key: None },
    );
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
    {
        let sel = grp_sym_app
            .scene()
            .expand_to_group_unit(0, &grp_sym_app.session.editing_group_path);
        grp_sym_app.session.set_selected_indices(sel);
    }
    assert!(doc_actions::rotate_selected_shapes(
        &mut grp_sym_app,
        std::f64::consts::FRAC_PI_2
    ));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    let summary_rot = scene_summary(&grp_sym_app);
    let rotated_sym_shape = summary_rot
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
    {
        let sel = grp_sym_app
            .scene()
            .expand_to_group_unit(0, &grp_sym_app.session.editing_group_path);
        grp_sym_app.session.set_selected_indices(sel);
    }
    assert!(doc_actions::delete_selected_shapes(&mut grp_sym_app));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    assert!(
        scene_summary(&grp_sym_app).is_empty,
        "空白フレーム中に親グループを削除すると、内部の空白フレームシンボルも残骸にならず一緒に削除されること"
    );
}

#[test]
fn test_resolved_cel_screen_and_onion_mesh_cache_and_pan_invalidation() {
    let mut grp_sym_app = Is2dApp::default();
    put_cel_shapes(
        &mut grp_sym_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(120.0, 100.0),
            10.0,
            Color32::from_rgb(200, 40, 40),
        ),
    );
    grp_sym_app.session.set_selected_indices(vec![0]);
    let blink_sym_id = doc_actions::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
    let blink_binding = scene_summary(&grp_sym_app).shapes[0]
        .symbol_instance
        .unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut grp_sym_app,
        blink_binding
    ));
    assert!(timeline_ui::timeline_insert_keyframe(
        &mut grp_sym_app,
        false
    ));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(doc_actions::clear_active_frame_shapes(&mut grp_sym_app));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    put_cel_shapes(
        &mut grp_sym_app,
        0,
        1,
        make_stroke(
            Point::new(90.0, 100.0),
            Point::new(130.0, 100.0),
            10.0,
            Color32::from_rgb(50, 180, 100),
        ),
    );
    assert!(symbol_ops::exit_symbol_timeline(&mut grp_sym_app));

    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(doc_actions::clear_active_frame_shapes(&mut grp_sym_app));
    timeline_ui::select_timeline_cells(&mut grp_sym_app, 0, 2, 0, 2);
    assert!(timeline_ui::timeline_extend_frame(&mut grp_sym_app));
    timeline_ui::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(
        doc_actions::place_symbol_instance_from_library(&mut grp_sym_app, blink_sym_id),
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
        scene_summary(&grp_sym_app).len,
        1,
        "Frame 1 に進むと配置したシンボルの2コマ目の図形がステージ中央に出現すること"
    );
    let stage_center = grp_sym_app.view.stage_center_world();
    assert!(scene_summary(&grp_sym_app).shapes[0].contains_point(stage_center));

    // シンボルを含むセルのスクリーンメッシュおよびオニオンスキンメッシュはその場で構築され、pan 変化も正確に反映されること
    let m_scr_1 = grp_sym_app
        .doc
        .root
        .resolved_cel_screen_mesh(
            0,
            1,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &grp_sym_app.doc.library,
        )
        .unwrap();
    let m_scr_2 = grp_sym_app
        .doc
        .root
        .resolved_cel_screen_mesh(
            0,
            1,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &grp_sym_app.doc.library,
        )
        .unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&m_scr_1, &m_scr_2) && m_scr_1.vertices == m_scr_2.vertices,
        "シンボルを含むセルはキャッシュを溜めずその場で同一頂点の Mesh を構築すること"
    );

    // パン（pan）が変化した際は resolved_cel_screen_mesh のスクリーン座標に反映されること (#3)
    let m_scr_panned = grp_sym_app
        .doc
        .root
        .resolved_cel_screen_mesh(
            0,
            1,
            1.0,
            Vec2::new(50.0, -30.0),
            false,
            kurbo::Affine::IDENTITY,
            &grp_sym_app.doc.library,
        )
        .unwrap();
    assert!(
        (m_scr_panned.vertices[0].pos.x - (m_scr_2.vertices[0].pos.x + 50.0)).abs() < 1e-3,
        "pan の移動量が頂点スクリーン座標に正確に反映されていること"
    );

    let m_onion_1 = grp_sym_app
        .cached_onion_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, ([35, 175, 105], 90))
        .unwrap();
    let m_onion_2 = grp_sym_app
        .cached_onion_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, ([35, 175, 105], 90))
        .unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&m_onion_1, &m_onion_2) && m_onion_1.vertices == m_onion_2.vertices
    );

    let second_sym_shapes = make_stroke(
        Point::new(10.0, 10.0),
        Point::new(30.0, 10.0),
        6.0,
        Color32::BLUE,
    );
    let second_sym_id = grp_sym_app.doc.register_new_symbol(second_sym_shapes);
    grp_sym_app
        .doc
        .rename_symbol(second_sym_id, "UnreferencedSymbolEdited".to_string());
    grp_sym_app
        .doc
        .rename_symbol(blink_sym_id, "ReferencedBlinkSymbolEdited".to_string());

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
    let mut shapes = scene_summary(&grp_sym_app).shapes;
    shapes.extend(make_stroke(
        Point::new(20.0, 20.0),
        Point::new(50.0, 20.0),
        8.0,
        Color32::GRAY,
    ));
    put_cel_shapes(&mut grp_sym_app, 0, 0, shapes);
    assert!(symbol_ops::enter_symbol_timeline(&mut grp_sym_app, inst_b));
    let bd_m1 = grp_sym_app
        .cached_backdrop_screen_mesh(1.0, Vec2::ZERO)
        .expect("背景シェイプが存在するため backdrop メッシュが生成されること");
    let bd_m2 = grp_sym_app
        .cached_backdrop_screen_mesh(1.0, Vec2::ZERO)
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
    assert!(symbol_ops::exit_symbol_timeline(&mut grp_sym_app));
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
        "シンボルインプレイス編集で Undo 保存や別キーフレーム編集を挟んでも、未変更キーフレームのローカル Cel::shapes は Arc 共有（CoW）が維持されること"
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

    // `FillShape::shape_id` によるレイヤー順展開後の選択状態追従
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
    put_cel_shapes(&mut sort_app, 1, 0, vec![shape_top]);
    sort_app.session.set_selected_indices(vec![0]);
    assert!(sort_app.session.selection.shapes.contains(&top_shape_id));
    put_cel_shapes(&mut sort_app, 0, 0, vec![shape_bottom]);
    assert_eq!(scene_summary(&sort_app).shapes[0].layer_id, lid_bottom);
    assert_eq!(scene_summary(&sort_app).shapes[1].layer_id, lid_top);
    assert!(sort_app.session.selection.shapes.contains(&top_shape_id));
    assert_eq!(
        sort_app.session.selected_indices(),
        vec![1],
        "下位レイヤーへの図形追加後も shape_id に基づいて選択インデックスが新しい位置へ追従すること"
    );

    // 空シンボルマーカーのデータモデル不変条件（contours=0, geometry_bounding_box=None, スナップ非対象）
    let empty_marker = FillShape::new_empty_symbol_marker(
        crate::symbol::SymbolInstanceBinding::new(777, 1, kurbo::Affine::translate((50.0, 50.0))),
        lid_bottom,
        &[],
    );
    assert_eq!(empty_marker.contours().count(), 0);
    assert!(empty_marker.get_contour(0).is_none());
    assert!(empty_marker.geometry_bounding_box().is_none());
    sort_app.session.overlay.add.push((
        crate::canvas_tool::Placement::TopOfLayer(lid_bottom, Vec::new()),
        empty_marker,
    ));
    sort_app.refresh_scene_base();
    assert!(
        tool_select::find_snap_vertex(
            sort_app.scene(),
            &sort_app.session.editing_group_path,
            Point::new(50.0, 50.0),
            1.0,
            u64::MAX,
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
    doc_actions::commit_raw_outline(&mut bucket_app, &raw_ring);

    // Layer 2 を追加して円の中央 (x = 200) に縦の分割線を引き、Layer 2 をロック (🔒) する
    timeline_ui::timeline_add_layer(&mut bucket_app);
    let raw_div =
        generate_brush_outline(&[Point::new(200.0, 120.0), Point::new(200.0, 280.0)], 6.0);
    doc_actions::commit_raw_outline(&mut bucket_app, &raw_div);
    timeline_ui::set_layer_locked(&mut bucket_app, 1, true);
    timeline_ui::switch_to_layer_and_frame(&mut bucket_app, 0, 0);

    bucket_app.tool.brush_color = red;
    assert!(doc_actions::bucket_fill_at(
        &mut bucket_app,
        Point::new(170.0, 200.0)
    ));
    let summary_bucket = scene_summary(&bucket_app);
    let red_fill = summary_bucket
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
    let mut many_walls = summary_bucket.shapes;
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
    put_cel_shapes(
        &mut lru_tween_app,
        0,
        0,
        make_stroke(
            Point::new(80.0, 200.0),
            Point::new(110.0, 200.0),
            10.0,
            Color32::from_rgb(220, 80, 40),
        ),
    );
    lru_tween_app.session.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut lru_tween_app).unwrap();
    // シンボルインスタンスを移動確定してもキャッシュ済み fill_mesh が破棄されず、平行移動分だけ頂点がシフトすること
    let mesh_before = lru_tween_app.scene().get(0).unwrap().fill_mesh().clone();
    assert!(!mesh_before.is_empty());
    let x0_before = mesh_before.vertices[0].pos.x;
    assert!(doc_actions::nudge_selected_shapes(
        &mut lru_tween_app,
        kurbo::Vec2::new(10.0, 0.0)
    ));
    assert!(
        lru_tween_app.scene().get(0).unwrap().has_cached_fill_mesh(),
        "シンボルインスタンスの移動確定（commit_moved_selection）時にキャッシュ済み fill_mesh が無駄に破棄されないこと"
    );
    let mesh_after = lru_tween_app.scene().get(0).unwrap().fill_mesh();
    assert!(
        (mesh_after.vertices[0].pos.x - (x0_before + 10.0)).abs() < 1e-3,
        "移動確定後の fill_mesh 頂点座標が +10.0 シフトしていること"
    );
    assert!(doc_actions::timeline_toggle_tween(&mut lru_tween_app));
    timeline_ui::select_timeline_cells(&mut lru_tween_app, 0, 11, 0, 11);
    assert!(timeline_ui::timeline_insert_keyframe(
        &mut lru_tween_app,
        true
    ));
    assert!(doc_actions::nudge_selected_shapes(
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
                    kurbo::Affine::IDENTITY,
                    &lru_tween_app.doc.library,
                )
                .unwrap()
        })
        .collect();
    for (f, first_m) in first_loop_meshes.iter().enumerate() {
        let second_m = lru_tween_app
            .doc
            .root
            .resolved_cel_screen_mesh(
                0,
                f,
                1.0,
                Vec2::ZERO,
                false,
                kurbo::Affine::IDENTITY,
                &lru_tween_app.doc.library,
            )
            .unwrap();
        assert!(
            !std::sync::Arc::ptr_eq(first_m, &second_m) && first_m.vertices == second_m.vertices,
            "トゥイーンフレーム（frame={f}）はその場で同一頂点の Mesh を構築すること"
        );
    }
}

#[test]
fn test_project_save_and_load_roundtrip() {
    let mut app = Is2dApp::default();
    app.doc.stage_size = Vec2::new(1280.0, 720.0);
    app.doc.fps = 30;

    // Layer 1: シンボル変換 + クラシックトゥイーン (Frame 0 -> Frame 5)
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 120.0),
            Point::new(160.0, 120.0),
            12.0,
            Color32::from_rgb(240, 80, 60),
        ),
    );
    app.session.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut app));
    assert!(doc_actions::set_active_keyframe_tween_rotate(
        &mut app,
        crate::symbol::TweenRotate::Clockwise
    ));
    timeline_ui::select_timeline_cells(&mut app, 0, 5, 0, 5);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    doc_actions::nudge_selected_shapes(&mut app, kurbo::Vec2::new(150.0, 40.0));

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
    put_cel_shapes(&mut app, 1, 5, raw_and_group);
    let last_idx = scene_summary(&app).len - 1;
    app.session.set_selected_indices(vec![last_idx]);
    let gid = doc_actions::group_selected_shapes(&mut app).unwrap();

    // シンボルインプレイス編集に入った状態で保存してもルートタイムラインとライブラリが正しく保存されること
    assert!(symbol_ops::enter_symbol_from_library(&mut app, sym_id));
    let mut sym_shapes = scene_summary(&app).shapes;
    sym_shapes.extend(make_stroke(
        Point::new(130.0, 140.0),
        Point::new(150.0, 140.0),
        6.0,
        Color32::from_rgb(50, 200, 100),
    ));
    put_cel_shapes(&mut app, 0, 0, sym_shapes);

    let tmp_path =
        std::env::temp_dir().join(format!("is2d_roundtrip_test_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).expect("保存に成功すること");
    assert_eq!(app.file.as_deref(), Some(tmp_path.as_path()));

    let mut loaded_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None)
        .expect("読み込みに成功すること");

    assert_eq!(loaded_app.doc.stage_size, Vec2::new(1280.0, 720.0));
    assert_eq!(loaded_app.doc.fps, 30);
    assert_eq!(loaded_app.doc.root.total_frames, 6);
    assert_eq!(loaded_app.doc.root.layers.len(), 2);
    assert_eq!(loaded_app.doc.library.len(), 1);
    assert_eq!(
        loaded_app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 120.0),
            Point::new(160.0, 120.0),
            12.0,
            Color32::from_rgb(240, 80, 60),
        ),
    );
    app.session.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();

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
    let summary = scene_summary(&loaded_app);
    let sym_indices: Vec<usize> = (0..summary.len)
        .filter(|&i| summary.shapes[i].symbol_instance.is_some())
        .collect();
    loaded_app.session.set_selected_indices(sym_indices);
    loaded_app.session.transform_pivot = Some(Point::new(111.0, 222.0));
    assert!(!loaded_app.session.selection.is_empty());
    assert!(doc_actions::copy_selected_shapes(&mut loaded_app));

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
    assert!(symbol_ops::enter_symbol_from_library(
        &mut loaded_app,
        sym_id
    ));
    assert!(
        !doc_actions::paste_clipboard(&mut loaded_app, true),
        "編集中のシンボル自身は循環参照のためペースト拒否されること"
    );
    assert!(symbol_ops::exit_symbol_timeline(&mut loaded_app));

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
    assert!(doc_actions::paste_clipboard(&mut loaded_app, true));
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
    assert!(doc_actions::paste_clipboard(&mut loaded_app, true));
    assert_eq!(loaded_app.doc.library.len(), 1);
    assert!(
        scene_summary(&loaded_app)
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
    assert!(doc_actions::paste_clipboard(&mut loaded_app, true));
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
fn test_clipboard_symbol_evaluates_latest_symbol_def() {
    // コピー後にシンボル定義を編集して再ペーストした際、古い展開図形ではなく最新の SymbolDef が評価されること
    let mut sym_paste_app = Is2dApp::default();
    put_cel_shapes(
        &mut sym_paste_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(140.0, 100.0),
            10.0,
            Color32::RED,
        ),
    );
    sym_paste_app.session.set_selected_indices(vec![0]);
    let child_sym = doc_actions::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.session.set_selected_indices(vec![0]);
    let parent_sym = doc_actions::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::copy_selected_shapes(&mut sym_paste_app));

    // 子シンボルをライブラリから削除（親シンボル内では通常グループへデタッチされる）
    assert!(doc_actions::delete_symbol_from_library(
        &mut sym_paste_app,
        child_sym
    ));
    assert_eq!(sym_paste_app.doc.library.len(), 1);

    // 親シンボルをインプレイス編集して青色の2シェイプに変更
    assert!(symbol_ops::enter_symbol_from_library(
        &mut sym_paste_app,
        parent_sym
    ));
    doc_actions::apply(&mut sym_paste_app, doc_actions::Action::SetFps { fps: 12 });
    let mut blue_shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(140.0, 100.0),
        10.0,
        Color32::BLUE,
    );
    blue_shapes.extend(make_stroke(
        Point::new(100.0, 130.0),
        Point::new(140.0, 130.0),
        10.0,
        Color32::BLUE,
    ));
    put_cel_shapes(&mut sym_paste_app, 0, 0, blue_shapes);
    assert!(symbol_ops::exit_symbol_timeline(&mut sym_paste_app));
    assert_eq!(
        sym_paste_app.doc.fps, 12,
        "シンボルインプレイス編集中のFPS変更が親タイムライン復帰後も維持されること"
    );
    assert_eq!(scene_summary(&sym_paste_app).len, 2);

    // 再度 Cmd+V ペースト：古い赤1シェイプではなく最新の青2シェイプが展開され、削除済み child_sym も孤児復活しないこと
    assert!(doc_actions::paste_clipboard(&mut sym_paste_app, false));
    assert_eq!(
        sym_paste_app.doc.library.len(),
        1,
        "デタッチ済みの子シンボルが孤児として再インポートされないこと"
    );
    assert_eq!(
        scene_summary(&sym_paste_app).len,
        4,
        "コピー後に編集された最新シンボル定義（2シェイプ×2インスタンス＝4シェイプ）が展開されること"
    );
    assert!(scene_summary(&sym_paste_app)
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
    put_cel_shapes(
        &mut cross_gid_app,
        0,
        0,
        make_stroke(
            Point::new(80.0, 80.0),
            Point::new(120.0, 80.0),
            10.0,
            Color32::RED,
        ),
    );
    cross_gid_app.session.set_selected_indices(vec![0]);
    let tab0_inner_gid = doc_actions::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab0_inner_gid, 1);
    cross_gid_app.session.set_selected_indices(vec![0]);
    let _tab0_sym_id = doc_actions::convert_selection_to_symbol(&mut cross_gid_app).unwrap();
    cross_gid_app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::copy_selected_shapes(&mut cross_gid_app));

    // 2つ目のタブを開き、Tab 1 側でもグループ（gid=1）を作成してから Tab 0 のシンボルをペースト＆グループ解除（デタッチ）
    crate::project_io::open_project_in_tab(&mut cross_gid_app, &tmp_cross_tab, None).unwrap();
    assert!(
        !cross_gid_app.clipboard.shapes.is_empty(),
        "タブ切り替え後もアプリ共通の clipboard が保持されていること"
    );
    put_cel_shapes(
        &mut cross_gid_app,
        0,
        0,
        make_stroke(
            Point::new(260.0, 200.0),
            Point::new(300.0, 200.0),
            10.0,
            Color32::BLUE,
        ),
    );
    cross_gid_app.session.set_selected_indices(vec![0]);
    let tab1_existing_gid = doc_actions::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab1_existing_gid, 1);

    assert!(doc_actions::paste_clipboard(&mut cross_gid_app, true));
    assert!(doc_actions::ungroup_selected_shapes(&mut cross_gid_app));
    let summary_cross = scene_summary(&cross_gid_app);
    let blue_gid = summary_cross
        .shapes
        .iter()
        .find(|s| s.color == Color32::BLUE)
        .unwrap()
        .group_path[0];
    let red_gid = summary_cross
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

    // (1) 生シェイプ移動後の選択中保存（Cmd+S）時の Merge Drawing 確定検証（保存は Document のコピー上で Floating を確定して書き出し、Session も Document も書き換えない）
    let mut merge_app = Is2dApp::default();
    let mut strokes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::BLUE,
    );
    strokes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    put_cel_shapes(&mut merge_app, 0, 0, strokes);
    merge_app.session.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut merge_app,
        doc_actions::Action::MoveSelected {
            delta: KVec2::new(0.0, -80.0),
        },
    );
    assert!(merge_app.session.has_modified_floating());

    crate::project_io::save_project_to_path(&mut merge_app, &tmp_merge_path)
        .expect("選択中の保存に成功すること");
    assert!(
        merge_app.session.has_modified_floating(),
        "保存時に Session の Floating は書き換えられないこと"
    );
    assert!(!merge_app.history.is_dirty());
    let mut loaded_merge = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_merge, &tmp_merge_path, None).unwrap();
    assert_eq!(loaded_merge.scene().len(), 2);
    canvas_tool::clear_selection(&mut merge_app);
    assert!(!merge_app.session.has_modified_floating());
    assert!(
        !merge_app.history.is_dirty(),
        "保存後の選択解除（Floating確定）で commit_id や dirty 状態が変化しないこと"
    );

    // (2) 非選択の背景同色シェイプと重なる位置へ移動して保存した場合、保存ファイル上では1図形に Union 結合され、
    //     選択解除時にも非選択の背景シェイプが勝手に選択に巻き込まれないこと
    let mut union_save_app = Is2dApp::default();
    let mut union_strokes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::RED,
    );
    union_strokes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    put_cel_shapes(&mut union_save_app, 0, 0, union_strokes);
    union_save_app.session.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut union_save_app,
        doc_actions::Action::MoveSelected {
            delta: KVec2::new(0.0, -80.0),
        },
    );
    crate::project_io::save_project_to_path(&mut union_save_app, &tmp_merge_path)
        .expect("同色Union保存に成功すること");
    let mut loaded_union = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_union, &tmp_merge_path, None).unwrap();
    assert_eq!(
        loaded_union.scene().len(),
        1,
        "保存されたドキュメントでは同色シェイプが1つにUnion結合されていること"
    );
    canvas_tool::clear_selection(&mut union_save_app);
    assert_eq!(union_save_app.scene().len(), 1);
    assert!(union_save_app.session.selection.is_empty());

    let mut self_union_app = Is2dApp::default();
    let mut self_union_strokes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        16.0,
        Color32::RED,
    );
    self_union_strokes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    put_cel_shapes(&mut self_union_app, 0, 0, self_union_strokes);
    self_union_app.session.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut self_union_app,
        doc_actions::Action::MoveSelected {
            delta: KVec2::new(0.0, -80.0),
        },
    );
    self_union_app.session.set_selected_indices(vec![0, 1]);
    assert!(doc_actions::copy_selected_shapes(&mut self_union_app));
    doc_actions::apply(
        &mut self_union_app,
        doc_actions::Action::MoveSelected {
            delta: KVec2::new(10.0, 0.0),
        },
    );
    crate::project_io::save_project_to_path(&mut self_union_app, &tmp_merge_path)
        .expect("選択同士のUnion保存に成功すること");
    let mut loaded_self_union = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_self_union, &tmp_merge_path, None).unwrap();
    assert_eq!(loaded_self_union.scene().len(), 1);
    assert_eq!(self_union_app.session.selected_indices().len(), 2);

    let _ = std::fs::remove_file(&tmp_merge_path);
}

#[test]
fn test_save_io_failure_rollback_and_marquee_slice_push_undo() {
    // 全レイヤーロック中は paste_clipboard が拒否され、矩形マーキースライス直後（未編集）に push_undo() が呼ばれても分割断片が早期確定されないこと
    let mut self_union_app = Is2dApp::default();
    put_cel_shapes(
        &mut self_union_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            16.0,
            Color32::RED,
        ),
    );
    self_union_app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::copy_selected_shapes(&mut self_union_app));

    timeline_ui::set_layer_locked(&mut self_union_app, 0, true);
    assert!(
        !doc_actions::paste_clipboard(&mut self_union_app, true),
        "ロックされたレイヤー上へのペーストが拒否されること"
    );
    timeline_ui::set_layer_locked(&mut self_union_app, 0, false);

    canvas_tool::clear_selection(&mut self_union_app);
    select_ops::apply_marquee_selection(
        &mut self_union_app,
        Point::new(90.0, 60.0),
        Point::new(150.0, 140.0),
        false,
    );
    assert!(self_union_app
        .session
        .floating()
        .is_some_and(|fl| !fl.modified));
    assert!(self_union_app.scene().len() >= 2);
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
    canvas_tool::clear_selection(&mut self_union_app);
    assert_eq!(scene_summary(&self_union_app).len, 1);

    // 存在しないディレクトリへの保存（I/O失敗）時のロールバックと、Floating 保持状態からの Undo -> Redo マージ確定
    let mut undo_redo_merge_app = Is2dApp::default();
    let mut ur_strokes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        Color32::RED,
    );
    ur_strokes.extend(make_stroke(
        Point::new(150.0, 180.0),
        Point::new(150.0, 220.0),
        16.0,
        Color32::RED,
    ));
    put_cel_shapes(&mut undo_redo_merge_app, 0, 0, ur_strokes);
    undo_redo_merge_app.session.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut undo_redo_merge_app,
        doc_actions::Action::MoveSelected {
            delta: KVec2::new(0.0, -80.0),
        },
    );
    assert!(undo_redo_merge_app.session.has_modified_floating());

    let bad_path = std::env::temp_dir()
        .join("is2d_nonexistent_dir_for_test_999999")
        .join("fail.is2d");
    assert!(crate::project_io::save_project_to_path(&mut undo_redo_merge_app, &bad_path).is_err());
    assert_eq!(
        undo_redo_merge_app.scene().len(),
        2,
        "保存I/O失敗時はライブキャンバス上の未マージ図形が勝手に結合されないこと"
    );
    assert!(undo_redo_merge_app.session.has_modified_floating());

    undo_redo_merge_app.undo();
    assert!(!undo_redo_merge_app.session.has_modified_floating());
    assert_eq!(undo_redo_merge_app.scene().len(), 2);
    undo_redo_merge_app.redo();
    assert!(undo_redo_merge_app.session.has_modified_floating());
    canvas_tool::clear_selection(&mut undo_redo_merge_app);
    assert_eq!(
        undo_redo_merge_app.scene().len(),
        1,
        "Floating 変更済み状態からの Undo -> Redo -> 選択解除後もマージ描画（Union）が確定されること"
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
          "version": 2,
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
                            100.0, 100.0, 100.0, 150.0,
                            100.0, 200.0, 150.0, 200.0,
                            200.0, 200.0, 150.0, 150.0
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
    assert_eq!(
        crate::project_io::open_project_in_tab(&mut sanitized_app, &tmp_malformed, None),
        Err("シンボルの入れ子が8段を超えているか、循環しています".to_owned()),
        "シンボルの循環参照を含むファイルは open_project_in_tab でエラーになること"
    );

    // 循環辺（CycleSymB 内の instance_gid=21 -> symbol_id=1）を除いた異常系 JSON ではサニタイズされて読み込めること
    let malformed_no_cycle = malformed_json.replace(
        r#""instance_gid": 21,
                              "symbol_id": 1,"#,
        r#""instance_gid": 21,
                              "symbol_id": 9998,"#,
    );
    std::fs::write(&tmp_malformed, malformed_no_cycle).unwrap();
    crate::project_io::open_project_in_tab(&mut sanitized_app, &tmp_malformed, None)
        .expect("異常系JSONでもサニタイズされてクラッシュせず読み込めること");
    assert_eq!(sanitized_app.doc.stage_size, Vec2::new(3840.0, 100.0));
    assert_eq!(sanitized_app.doc.fps, 60);
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
        sanitized_app.doc.root.layers[0].keyframes[0].tween,
        "生シェイプとシンボルが混在し単一シンボル条件を満たさないキーフレームも tween=true が保持されること"
    );
    assert_eq!(
        sanitized_app.doc.root.layers[0].tween_state_at_keyframe(0),
        timeline::TweenState::Incomplete,
        "単一シンボル条件を満たさない tween=true キーフレームは未完（Incomplete）状態になること"
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
    doc_actions::commit_raw_outline(&mut app, &raw_red);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 1);

    // Layer 2 (index 1, id 2) を追加して同じ座標に青い横線を描画（両レイヤーとも非ロック）
    timeline_ui::timeline_add_layer(&mut app);
    assert_eq!(app.doc.root.active_layer, 1);
    app.tool.brush_color = blue;
    let raw_blue =
        generate_brush_outline(&[Point::new(100.0, 200.0), Point::new(300.0, 200.0)], 14.0);
    doc_actions::commit_raw_outline(&mut app, &raw_blue);
    assert_eq!(app.doc.root.layers[1].keyframes[0].cel.shapes.len(), 1);

    // Layer 2 がアクティブな状態で消しゴムツールを実行 → Layer 2 の青い線のみが削られ、Layer 1 の赤い線は無傷であること
    app.tool.tool = Tool::Eraser;
    let raw_erase =
        generate_brush_outline(&[Point::new(200.0, 150.0), Point::new(200.0, 250.0)], 16.0);
    doc_actions::commit_raw_outline(&mut app, &raw_erase);

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
    doc_actions::commit_raw_outline(&mut app, &raw);
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
    put_cel_shapes(
        &mut move_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
            10.0,
            red,
        ),
    );
    timeline_ui::switch_to_layer_and_frame(&mut move_app, 0, 3);
    timeline_ui::timeline_insert_keyframe(&mut move_app, false);
    timeline_ui::switch_to_layer_and_frame(&mut move_app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut move_app, false);
    put_cel_shapes(
        &mut move_app,
        0,
        4,
        make_stroke(
            Point::new(200.0, 200.0),
            Point::new(250.0, 200.0),
            10.0,
            Color32::BLUE,
        ),
    );

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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            14.0,
            Color32::RED,
        ),
    );
    app.session.set_selected_indices(vec![0]);
    let gid = doc_actions::group_selected_shapes(&mut app).unwrap();

    // グループをダブルクリックしてインプレイス編集に入る
    assert!(select_ops::enter_group_at(
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            14.0,
            Color32::RED,
        ),
    );
    app.doc.root.layers[0].keyframe_mut(0).tween = true;
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete,
        "tween == true で単一シンボル条件を満たさないキーフレームは未完トゥイーン (TweenState::Incomplete) として扱われること"
    );
}

#[test]
fn test_marquee_shift_deselect_restores_for_bucket_fill() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);

    // マーキースライスで1本のストロークを2つに分割選択したあと、Shift+クリック（toggle_shape_selection）で選択を空にした際、
    // 即座にスライス前の元の図形が復元され、直後のバケツ塗りで古いスライス状態が残らないこと
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(240.0, 100.0),
            16.0,
            red,
        ),
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(90.0, 70.0),
        Point::new(160.0, 130.0),
        false,
    );
    assert!(app.session.floating().is_some());
    assert_eq!(app.session.selected_indices().len(), 1);
    let sel_idx = app.session.selected_indices()[0];
    select_ops::toggle_shape_selection(&mut app, sel_idx);
    assert!(app.session.selection.is_empty());
    assert!(
        app.session.floating().is_none(),
        "Shift+クリックで選択が空になった時点で未編集のマーキースライスは即座に復元されること"
    );
    assert_eq!(scene_summary(&app).len, 1);

    // 直後にバケツツールで赤シェイプを青に塗り替えても正常に1シェイプが青になること
    app.tool.brush_color = blue;
    assert!(doc_actions::bucket_fill_at(
        &mut app,
        Point::new(170.0, 100.0)
    ));
    assert_eq!(scene_summary(&app).len, 1);
    assert_eq!(scene_summary(&app).shapes[0].color, blue);
}

#[test]
fn test_symbol_rename_consecutive_keystrokes_grouped_into_single_undo() {
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(240.0, 100.0),
            16.0,
            Color32::BLUE,
        ),
    );
    app.session.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    let undo_before_rename = app.history.undo_stack.len();

    // 1文字目の入力開始時のみ Undo を積み、フレーム描画（ensure_valid_selection）を挟んで2文字目を入力しても Undo が増殖しないこと
    assert!(doc_actions::rename_symbol_in_library(&mut app, sym_id, "H"));
    app.ensure_valid_library_selection();
    assert_eq!(
        app.history.last_coalesce_key,
        Some(canvas_tool::CoalesceKey::RenameSymbol(sym_id)),
        "毎フレームの ensure_valid_selection 呼び出しで RenameSymbol の CoalesceKey が消去されないこと"
    );
    assert!(doc_actions::rename_symbol_in_library(
        &mut app, sym_id, "Hero"
    ));
    assert_eq!(
        app.history.undo_stack.len(),
        undo_before_rename + 1,
        "連続キー入力によるシンボル名変更で Undo スタックが1回分のみ記録されること"
    );
}

#[test]
fn test_step_frame_loop_mode_and_total_frames_over_600() {
    let mut app = Is2dApp::default();

    // 600フレーム超（1200フレーム）のタイムライン延長と step_frame のループ ON / OFF 挙動
    app.doc.root.layers[0].insert_frame_at(1199);
    app.doc.root.sync_total_frames();
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
    sym_parent.timeline.layers[0].keyframe_mut(0).cel = crate::timeline::Cel::new_with_instances(
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            10.0,
            red,
        ),
    );
    assert!(timeline_ui::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 1);
    put_cel_shapes(
        &mut app,
        0,
        1,
        make_stroke(
            Point::new(200.0, 200.0),
            Point::new(260.0, 200.0),
            10.0,
            blue,
        ),
    );

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
    put_cel_shapes(
        &mut hold_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            10.0,
            red,
        ),
    );
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
    put_cel_shapes(
        &mut sym_f6_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
            10.0,
            red,
        ),
    );
    sym_f6_app.session.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut sym_f6_app).unwrap();
    assert!(symbol_ops::enter_symbol_from_library(
        &mut sym_f6_app,
        sym_id
    ));
    sym_f6_app.with_active_timeline_mut(|tl, _| {
        tl.layers[0].insert_frame_at(3);
        tl.sync_total_frames();
        tl.layers[0].insert_keyframe(2, false);
    });
    put_cel_shapes(
        &mut sym_f6_app,
        0,
        2,
        make_stroke(
            Point::new(105.0, 100.0),
            Point::new(145.0, 100.0),
            10.0,
            blue,
        ),
    );
    assert!(symbol_ops::exit_symbol_timeline(&mut sym_f6_app));
    sym_f6_app.doc.root.layers[0].insert_frame_at(4);
    sym_f6_app.doc.root.sync_total_frames();
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 2);
    assert_eq!(scene_summary(&sym_f6_app).shapes[0].color, blue);
    assert!(timeline_ui::timeline_insert_keyframe(&mut sym_f6_app, true));
    assert_eq!(
        scene_summary(&sym_f6_app).shapes[0].color,
        blue,
        "F6 で分割された新しいキーフレームでも Loop シンボルの表示コマ (Frame 2 = 青) がそのまま引き継がれること"
    );

    // トゥイーン区間（tween_rotate = Clockwise）の途中フレームでは直接のキャンバス編集は無効化され、
    // F6 キーフレーム挿入で分割した際に tween_rotate = Clockwise が引き継がれて編集可能になること (#10, M2-2)
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 4);
    assert!(timeline_ui::timeline_insert_keyframe(&mut sym_f6_app, true));
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 2);
    assert!(doc_actions::timeline_toggle_tween(&mut sym_f6_app));
    assert!(doc_actions::set_active_keyframe_tween_rotate(
        &mut sym_f6_app,
        crate::symbol::TweenRotate::Clockwise
    ));
    timeline_ui::switch_to_layer_and_frame(&mut sym_f6_app, 0, 3);
    sym_f6_app.session.set_selected_indices(vec![0]);
    assert!(
        !doc_actions::nudge_selected_shapes(&mut sym_f6_app, kurbo::Vec2::new(20.0, 0.0)),
        "トゥイーン途中のコマでは直接の移動編集ができないこと"
    );
    assert!(
        sym_f6_app.doc.root.layers[0]
            .keyframes
            .iter()
            .all(|kf| kf.start_frame != 3),
        "トゥイーン途中のコマでの移動操作で自動キーフレームが生成されないこと"
    );
    assert!(timeline_ui::timeline_insert_keyframe(&mut sym_f6_app, true));
    sym_f6_app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut sym_f6_app,
        kurbo::Vec2::new(20.0, 0.0)
    ));
    let kf_at_3 = sym_f6_app.doc.root.layers[0]
        .keyframes
        .iter()
        .find(|kf| kf.start_frame == 3)
        .expect("F6 挿入により Frame 3 に新キーフレームが生成されていること");
    assert!(kf_at_3.tween);
    assert_eq!(
        kf_at_3.tween_rotate,
        crate::symbol::TweenRotate::Clockwise,
        "F6 で分割されたキーフレームに前のキーフレームの tween_rotate 設定が引き継がれること"
    );
}

#[test]
fn test_flip_tween_midpoint_and_horizontal_flip_without_spurious_rotation() {
    use crate::symbol::{interpolate_affine_with_pivot, TweenRotate};

    // 1. 垂直反転 (scale_non_uniform(1.0, -1.0)) を含むトゥイーンの中間フレーム (t = 0.5) でも特異行列にならずピボット位置へ変換されること (H5)
    let a0 = kurbo::Affine::translate((100.0, 100.0));
    let a1 = kurbo::Affine::translate((300.0, 100.0)) * kurbo::Affine::scale_non_uniform(1.0, -1.0);
    let (mid, _) =
        interpolate_affine_with_pivot(a0, Point::ZERO, a1, Point::ZERO, TweenRotate::Auto, 0.5);
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
    let (mid_h, _) =
        interpolate_affine_with_pivot(a0, Point::ZERO, h1, Point::ZERO, TweenRotate::Auto, 0.25);
    let top_pt = mid_h * Point::new(0.0, -20.0);
    assert!(
        (top_pt.y - 80.0).abs() < 1e-3,
        "水平反転 (scale_x = -1, scale_y = +1) のトゥイーン途中で 180° 回転に誤分解されて上下が揺れないこと: top_pt={:?}",
        top_pt
    );
}

#[test]
fn test_symbol_delete_mid_tween_preserves_start_keyframe_pose() {
    // M2-4: シンボルをライブラリから削除すると全キーフレームのインスタンスが削除されトゥイーンが外れ、Undo 1回で元に戻ること
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
    app.session.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    app.doc.root.layers[0].insert_frame_at(4);
    app.doc.root.sync_total_frames();
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    app.session.set_selected_indices(vec![0]);
    doc_actions::nudge_selected_shapes(&mut app, KVec2::new(200.0, 0.0));
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert!(doc_actions::timeline_toggle_tween(&mut app));
    assert!(app.doc.root.layers[0].keyframes[0].tween);

    // シンボルを削除すると、全キーフレームからインスタンスが消え、トゥイーンは ON のまま未完になること
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    assert!(doc_actions::delete_symbol_from_library(&mut app, sym_id));
    assert!(app.doc.find_symbol(sym_id).is_none());
    for kf in &app.doc.root.layers[0].keyframes {
        assert!(
            kf.cel.instances.is_empty() && kf.cel.shapes.is_empty(),
            "削除したシンボルのインスタンスも分解図形も残らないこと"
        );
    }
    assert!(
        app.doc.root.layers[0].keyframes[0].tween,
        "シンボル削除後も開始キーフレームの tween=true は保持されること"
    );
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        timeline::TweenState::Incomplete,
        "シンボル削除後の開始キーフレームは未完（Incomplete）状態になること"
    );

    // Undo 1回でライブラリのシンボル・全インスタンス・トゥイーン設定が元に戻ること
    app.undo();
    assert!(app.doc.find_symbol(sym_id).is_some());
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.instances.len(), 1);
    assert_eq!(app.doc.root.layers[0].keyframes[1].cel.instances.len(), 1);
    assert!(app.doc.root.layers[0].keyframes[0].tween);
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
    timeline_ui::timeline_add_layer(&mut app);
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);

    // 図形に当たらない空振り消しゴムで Undo が増えないこと (H7)
    let undo_before_noop_erase = app.history.undo_stack.len();
    app.tool.tool = Tool::Eraser;
    let miss_erase =
        generate_brush_outline(&[Point::new(700.0, 400.0), Point::new(720.0, 400.0)], 10.0);
    doc_actions::commit_raw_outline(&mut app, &miss_erase);
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
    put_cel_shapes(
        &mut save_app,
        0,
        0,
        make_stroke(Point::new(50.0, 50.0), Point::new(100.0, 50.0), 8.0, red),
    );
    crate::project_io::save_project_to_path(&mut save_app, &tmp_compact).unwrap();
    let saved_str = std::fs::read_to_string(&tmp_compact).unwrap();
    assert!(
        !saved_str.contains('\n'),
        "保存される .is2d ファイルは膨張を防ぐコンパクト JSON（改行なし）であること"
    );
    save_app.doc.stage_size = Vec2::new(f32::NAN, 450.0);
    let fixed = crate::project_io::save_project_to_path(&mut save_app, &tmp_compact)
        .expect("NaN が含まれる状態でも壊れた数値を補正して保存が成功すること");
    assert_eq!(fixed, vec!["ステージサイズ".to_owned()]);
    let nan_fixed_str = std::fs::read_to_string(&tmp_compact).unwrap();
    assert!(
        !nan_fixed_str.contains("NaN") && !nan_fixed_str.contains("null"),
        "補正後の JSON に NaN や null が混入しないこと"
    );
    save_app.doc.stage_size = Vec2::new(800.0, 450.0);
    let _ = std::fs::remove_file(&tmp_compact);

    for i in 0..110 {
        doc_actions::apply(
            &mut save_app,
            doc_actions::Action::EndCoalesce { key: None },
        );
        doc_actions::apply(
            &mut save_app,
            doc_actions::Action::SetStageSize {
                size: Vec2::new(800.0 + (i as f32), 450.0),
            },
        );
    }
    assert_eq!(
        save_app.history.undo_stack.len(),
        crate::canvas_tool::MAX_UNDO_HISTORY,
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

#[test]
fn test_egui_shortcuts_clipboard_sync_and_cmd0_preserves_ui_zoom_factor() {
    use eframe::egui;

    let ctx = egui::Context::default();
    crate::configure_sharp_light_theme(&ctx);
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(180.0, 100.0),
            12.0,
            Color32::RED,
        ),
    );
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
    assert_eq!(app.scene().len(), 2);

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
        (app.scene()
            .get(app.scene().len() - 1)
            .unwrap()
            .bounding_box()
            .center()
            .x
            - (140.0 + 32.0))
            .abs()
            < 2.0,
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
        (app.scene()
            .get(app.scene().len() - 1)
            .unwrap()
            .bounding_box()
            .center()
            .x
            - (140.0 + 48.0))
            .abs()
            < 2.0,
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(center_world.x - 40.0, center_world.y),
            Point::new(center_world.x + 40.0, center_world.y),
            20.0,
            Color32::from_rgb(220, 60, 60),
        ),
    );

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
    let initial_bbox_center = scene_summary(&app).shapes[0].bounding_box().center();
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
    let moved_center = app.scene().get(0).unwrap().bounding_box().center();
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            10.0,
            red,
        ),
    );
    timeline_ui::timeline_add_layer(&mut app);
    let lid2 = app.doc.root.layers[1].id;
    put_cel_shapes(
        &mut app,
        1,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(160.0, 200.0),
            10.0,
            blue,
        )
        .into_iter()
        .map(|s| s.with_layer_id(lid2))
        .collect(),
    );
    assert_eq!(scene_summary(&app).len, 2);

    // Layer 1 を非表示 (visible = false) に切り替えると、Scene から除外されること
    timeline_ui::set_layer_visible(&mut app, 0, false);
    assert_eq!(
        scene_summary(&app).len,
        1,
        "非表示レイヤー (visible=false) の図形は Scene から除外されること"
    );
    assert_eq!(scene_summary(&app).shapes[0].color, blue);

    // その状態で Layer 2 を編集しても、非表示の Layer 1 の図形が消去されないこと
    app.session.set_selected_indices(vec![0]);
    assert!(doc_actions::nudge_selected_shapes(
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
    assert_eq!(scene_summary(&app).len, 2);
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
    app.doc.fps = 10; // 1コマ = 0.1秒
    app.doc.root.layers[0].insert_frame_at(2);
    app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
            10.0,
            Color32::RED,
        ),
    );
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    put_cel_shapes(
        &mut app,
        0,
        1,
        make_stroke(
            Point::new(150.0, 100.0),
            Point::new(200.0, 100.0),
            10.0,
            Color32::GREEN,
        ),
    );
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    put_cel_shapes(
        &mut app,
        0,
        2,
        make_stroke(
            Point::new(200.0, 100.0),
            Point::new(250.0, 100.0),
            10.0,
            Color32::BLUE,
        ),
    );

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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(300.0, 200.0),
            20.0,
            Color32::from_rgb(220, 60, 60),
        ),
    );

    // 最も長いセグメントを選び、端点付近 (t = 0.05 < 0.18) かつ曲線から少し外れた座標でドラッグ開始 (#5)
    let seg_idx = scene_summary(&app).shapes[0]
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
    let orig_seg = scene_summary(&app).shapes[0].exterior.segments[seg_idx];
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
    let after_zero_move = app.scene().get(0).unwrap().exterior.segments[seg_idx];
    assert!(
        (after_zero_move.p1 - orig_seg.p1).hypot() < 1e-6,
        "端点付近 (t=0.10 < 0.18) や曲線から少し離れた位置を掴んだ瞬間 (移動量0) に曲線が跳ばないこと (#5): orig={:?}, after={:?}",
        orig_seg.p1,
        after_zero_move.p1
    );

    // 微小移動 (dy = +2.0) でも跳びなく滑らかに移動すること（ドラッグ中は Scene の Overlay に反映され、セルは不変）
    let grab_t = 0.18_f64;
    let grab_before = orig_seg.eval(grab_t);
    tool_select::update_reshape_drag(&mut app, pt_near_end + kurbo::Vec2::new(0.0, 2.0));
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes[0]
            .exterior
            .segments[seg_idx]
            .p1,
        orig_seg.p1,
        "ドラッグ中はセル内の図形が書き換えられないこと"
    );
    let grab_after = app.scene().get(0).unwrap().exterior.segments[seg_idx].eval(grab_t);
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
    put_cel_shapes(&mut app, 0, 0, shapes);

    let area_blue_top_before = scene_summary(&app).shapes[0].exterior.signed_area();
    let area_blue_bottom_before = scene_summary(&app).shapes[1].exterior.signed_area();

    // 赤い図形2つ (index 2, 3) を選択して左へ -100px 移動し、2つの青い図形の上にそれぞれ重ねる
    app.session.set_selected_indices(vec![2, 3]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(-100.0, 0.0)
    ));
    assert!(app.session.has_modified_floating());

    // Shift+クリック相当の toggle_shape_selection で、上側の赤い図形 (index 2) だけを選択解除し、下側の赤い図形 (index 3) は選択に残す (#7)
    select_ops::toggle_shape_selection(&mut app, 2);
    assert_eq!(app.session.selected_indices().len(), 1);
    assert!(
        app.session.has_modified_floating(),
        "一部の図形だけを選択解除した際、残りの選択図形は変更済み Floating のまま維持されること (#7)"
    );

    // 選択に残した下側の赤い図形をさらに右へ +100px 戻してから全選択解除する
    let remaining_idx = app.session.selected_indices()[0];
    assert_eq!(app.scene().get(remaining_idx).unwrap().color, red);
    assert!(doc_actions::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(100.0, 0.0)
    ));
    canvas_tool::clear_selection(&mut app);

    // 結果：先に選択解除された上側の赤い図形は上側の青い図形を切り抜いているが、
    // 選択に残して後から退かした下側の赤い図形は下側の青い図形を切り抜いていないこと！
    let summary_after = scene_summary(&app);
    let blue_shapes: Vec<&FillShape> = summary_after
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
fn test_pivot_commit_restores_marquee_slice_and_persists() {
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(220.0, 100.0),
            20.0,
            Color32::from_rgb(40, 150, 90),
        ),
    );

    // 1. 生シェイプを矩形マーキーで部分スライス選択した直後、変形せずにピボットだけ動かして確定した場合、
    // スライスの切れ目がタイムラインに書き込まれず、選択解除で元の1図形に戻ること (#8)
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(70.0, 80.0),
        Point::new(160.0, 120.0),
        false,
    );
    assert!(app.session.floating().is_some());
    assert_eq!(app.scene().len(), 2);

    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool {
            tool: Tool::Transform,
        },
    );
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
    canvas_tool::clear_selection(&mut app);
    let sym_shapes = make_stroke(
        Point::new(100.0, 220.0),
        Point::new(180.0, 220.0),
        16.0,
        Color32::from_rgb(200, 80, 40),
    );
    let mut shapes = scene_summary(&app).shapes;
    shapes.extend(sym_shapes);
    put_cel_shapes(&mut app, 0, 0, shapes);
    app.session.set_selected_indices(vec![1]);
    doc_actions::convert_selection_to_symbol(&mut app).unwrap();

    // 生シェイプの一部とシンボルインスタンス全体をまたぐマーキー選択を行う
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(80.0, 80.0),
        Point::new(160.0, 260.0),
        false,
    );
    assert!(app.session.floating().is_some());

    // シンボルインスタンスのみに選択を絞り、ピボットを変更して選択解除
    let sym_indices: Vec<usize> = app
        .scene()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.symbol_instance.is_some())
        .map(|(i, _)| i)
        .collect();
    app.session.set_selected_indices(sym_indices);
    let new_world_pivot = Point::new(155.0, 235.0);
    assert!(doc_actions::set_selected_symbol_world_pivot(
        &mut app,
        Some(new_world_pivot),
        true
    ));
    assert!(app.session.floating().is_none());
    canvas_tool::clear_selection(&mut app);

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
    app.doc.root.layers[0].insert_frame_at(5);
    app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(Point::new(60.0, 60.0), Point::new(100.0, 60.0), 10.0, red),
    );
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    put_cel_shapes(
        &mut app,
        0,
        2,
        make_stroke(Point::new(160.0, 60.0), Point::new(200.0, 60.0), 10.0, blue),
    );

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
    tween_app.doc.root.layers[0].insert_frame_at(4);
    tween_app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut tween_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(140.0, 100.0),
            10.0,
            red,
        ),
    );
    tween_app.session.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut tween_app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut tween_app));
    timeline_ui::switch_to_layer_and_frame(&mut tween_app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut tween_app, true);
    assert!(doc_actions::nudge_selected_shapes(
        &mut tween_app,
        kurbo::Vec2::new(200.0, 0.0)
    ));

    // 中間フレーム Frame 2 (t = 0.5 -> dx = +100.0) のみを選択して別レイヤーへ複製
    let mid_shapes = tween_app
        .doc
        .root
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
    let copied_shapes = tween_app
        .doc
        .root
        .evaluate_layer_shapes_at(1, 2, &tween_app.doc.library);
    assert!(
        (copied_shapes[0].bounding_box().center().x - mid_center_x).abs() < 1e-3,
        "トゥイーン途中フレームから複製したセルが中割り補間姿勢を引き継いでいること (#9-2)"
    );

    // 3. 別レイヤーへの複製で instance_gid および group_path ID が重複せず一意に再採番されること (#9-3)
    let orig_gid = tween_app.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .instance_gid;
    let copied_gid = tween_app.doc.root.layers[1].cel_at(2).unwrap().instances[0]
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
            Some(project_io::PendingCloseConfirm::SingleTab { orig_active: 1 })
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            18.0,
            Color32::from_rgb(70, 140, 230),
        ),
    );
    app.session.set_selected_indices(vec![0]);

    // 事前に fill_mesh キャッシュを構築して Arc ポインタを取得 (#12)
    let mesh_arc_before = std::sync::Arc::clone(app.scene().get(0).unwrap().fill_mesh());
    assert!(app.scene().get(0).unwrap().has_cached_fill_mesh());

    // 1. 選択移動ドラッグを開始して微小移動未満で終了（noop リストア）してもキャッシュが失われないこと
    let move_drag =
        clipboard_ops::begin_move_selection_drag(&mut app, Point::new(150.0, 100.0), false);
    doc_actions::finish_move_selection_drag(
        &mut app,
        move_drag.total_delta,
        move_drag.alt_duplicate,
    );
    assert!(
        app.scene().get(0).unwrap().has_cached_fill_mesh()
            && std::sync::Arc::ptr_eq(app.scene().get(0).unwrap().fill_mesh(), &mesh_arc_before),
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
        app.scene().get(0).unwrap().has_cached_fill_mesh()
            && std::sync::Arc::ptr_eq(app.scene().get(0).unwrap().fill_mesh(), &mesh_arc_before),
        "ドラッグのキャンセル時にテッセレーション済み Arc<Mesh> が破棄されないこと (#12)"
    );
}

#[test]
fn test_u64_max_entity_ids_in_crafted_json_remapped_without_collision() {
    // u64::MAX の layer.id, symbol.id, instance_gid, group_path を含む細工ファイル (#13)
    let crafted_json = format!(
        r#"{{
          "version": 2,
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
                            100.0, 100.0, 150.0, 100.0,
                            200.0, 100.0, 200.0, 150.0,
                            200.0, 200.0, 150.0, 150.0
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
    assert_eq!(
        layer_ids.len(),
        3,
        "新規レイヤーのIDが既存レイヤーと衝突しないこと (#13)"
    );

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
                    loop_mode: vector_engine::SymbolLoopMode::Loop,
                    first_frame: 0,
                    color_effect: vector_engine::ColorEffect::default(),
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
                        tween_rotate: timeline::TweenRotate::Auto,
                        shapes: Vec::new(),
                        instances,
                    }],
                }],
            },
        });
    }

    let doc = project_io::ProjectDocument {
        version: project_io::PROJECT_FORMAT_VERSION,
        stage_size: [800.0, 450.0],
        timeline: (&timeline::Timeline::default()).into(),
        library,
        next_symbol_id: (n + 1) as u64,
        next_group_id: 100_000,
        recovery_source: None,
    };

    let path = std::env::temp_dir().join(format!("is2d_dense_cycle_{}.is2d", std::process::id()));
    std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();
    let mut app = Is2dApp::default();
    let res = crate::project_io::open_project_in_tab(&mut app, &path, None);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        res,
        Err("シンボルの入れ子が8段を超えているか、循環しています".to_owned()),
        "密な循環参照を含むファイルは symbol_nest_exceeded で即座にエラーになること (#14)"
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            10.0,
            Color32::RED,
        ),
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
    app.doc.root.layers[0].insert_frame_at(3);
    app.doc.root.sync_total_frames();
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 150.0),
            Point::new(300.0, 150.0),
            80.0,
            Color32::from_rgb(80, 140, 220),
        ),
    );
    app.session.set_selected_indices(vec![0]);
    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();

    // 1. 四隅の少し外側 (+10px, -10px) は Rotate になるが、上辺中央の少し外側 (四隅から100px以上離れた位置) は Rotate にならないこと (#17)
    let corner_outside = Point::new(bbox.x1 + 10.0, bbox.y0 - 10.0);
    assert_eq!(
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            corner_outside,
            app.view.zoom
        ),
        canvas_tool::TransformHoverTarget::Rotate,
        "四隅の外側近傍では Rotate ゾーンになること (#17)"
    );
    let top_edge_outside = Point::new(bbox.center().x, bbox.y0 - 12.0);
    assert_eq!(
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            top_edge_outside,
            app.view.zoom
        ),
        canvas_tool::TransformHoverTarget::None,
        "四隅から離れた辺中央の外側は Rotate ゾーンにならないこと (#17)"
    );

    // 2. 連続ペーストの途中にズーム倍率を変更しても、毎回前回位置から +16px (画面上) ずつ単調増加の階段状に配置されること (#18)
    app.view.zoom = 1.0;
    doc_actions::copy_selected_shapes(&mut app);
    let c0 = scene_summary(&app).shapes[0].bounding_box().center();

    doc_actions::paste_clipboard(&mut app, false);
    let c1 = app
        .scene()
        .get(app.scene().len() - 1)
        .unwrap()
        .bounding_box()
        .center();
    assert!((c1.x - (c0.x + 16.0)).abs() < 1e-4);

    // 途中でズームイン (zoom = 4.0 -> 1ステップ = 4.0 ワールド座標) してから2回目のペースト
    app.view.zoom = 4.0;
    doc_actions::paste_clipboard(&mut app, false);
    let c2 = app
        .scene()
        .get(app.scene().len() - 1)
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
    app.doc.root.layers[0].insert_frame_at(5);
    app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            10.0,
            red,
        ),
    );

    let sel_prefix = timeline::TimelineCellSelection::from_corners(0, 0, 0, 2);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut app, sel_prefix, 0, 8, false
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
    hold_app.doc.root.layers[0].insert_frame_at(5);
    hold_app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut hold_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(160.0, 100.0),
            10.0,
            red,
        ),
    );

    let sel_mid_hold = timeline::TimelineCellSelection::from_corners(0, 2, 0, 3);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut hold_app,
        sel_mid_hold,
        0,
        6,
        false
    ));
    for f in 0..=1 {
        assert_eq!(
            hold_app.doc.root.layers[0].cel_at(f).unwrap().shapes.len(),
            1
        );
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
        assert_eq!(
            hold_app.doc.root.layers[0].cel_at(f).unwrap().shapes.len(),
            1
        );
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
        sym.timeline.layers[0].keyframe_mut(0).cel =
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
    app.doc.root.layers[0].keyframe_mut(0).cel =
        timeline::Cel::new_with_instances(Vec::new(), root_instances);

    let t0 = std::time::Instant::now();
    app.refresh_scene_base();
    let elapsed_memo = t0.elapsed();
    assert_eq!(
        scene_summary(&app).len,
        20,
        "ルート直下の空シンボルインスタンス20個がそれぞれ空マーカーとして即座に展開されること"
    );
    println!("[20^8 empty symbol memo] elapsed = {:?}", elapsed_memo);
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
    let mut sel_shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(180.0, 100.0),
        14.0,
        red,
    );
    sel_shapes.extend(make_stroke(
        Point::new(220.0, 100.0),
        Point::new(300.0, 100.0),
        14.0,
        red,
    ));
    sel_shapes.extend(make_stroke(
        Point::new(400.0, 100.0),
        Point::new(460.0, 100.0),
        14.0,
        blue,
    ));
    put_cel_shapes(&mut sel_app, 0, 0, sel_shapes);

    // 中央の赤図形 (idx=1) を選択して左へ -80px 移動し、Shape 0 と重ならせる（まだ選択中なので未結合で3図形のまま）
    sel_app.session.set_selected_indices(vec![1]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut sel_app,
        KVec2::new(-80.0, 0.0)
    ));
    assert_eq!(scene_summary(&sel_app).len, 3);
    assert!(sel_app.session.has_modified_floating());

    // Shift＋マーキー (additive = true) で右の青図形を追加選択した際、移動済みの赤図形が早期に Shape 0 と結合されないこと
    select_ops::apply_marquee_selection(
        &mut sel_app,
        Point::new(380.0, 70.0),
        Point::new(480.0, 130.0),
        true,
    );
    assert_eq!(
        scene_summary(&sel_app).len,
        3,
        "Shift＋マーキーで追加選択しても、まだ選択中の移動済み生図形は先に結合されないこと (#4)"
    );
    assert_eq!(sel_app.session.selected_indices().len(), 2);

    // その後選択解除 (clear_selection) した瞬間に同色結合が確定し、赤1個＋青1個の計2図形になること
    canvas_tool::clear_selection(&mut sel_app);
    assert_eq!(scene_summary(&sel_app).len, 2);
}

#[test]
fn test_scale_commit_and_repeated_transform_rebuild_mesh_cache() {
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(200.0, 200.0),
            Point::new(210.0, 200.0),
            6.0,
            Color32::from_rgb(60, 130, 230),
        ),
    );
    let initial_verts = app.scene().get(0).unwrap().fill_mesh().vertices.len();

    // 1. 自由変形の拡大ドラッグ (update_transform_overlay) 中は既存頂点数を再利用し、
    //    CommitTransformDrag 確定時に rebuild_cache() されて曲線分割頂点数が増加すること (#5)
    app.session.set_selected_indices(vec![0]);
    let orig_shapes = app.session.begin_transform_overlay();
    let pivot = scene_summary(&app).shapes[0].bounding_box().center();
    let scale_xform = kurbo::Affine::translate(pivot.to_vec2())
        * kurbo::Affine::scale(4.0)
        * kurbo::Affine::translate(-pivot.to_vec2());
    app.session
        .update_transform_overlay(&orig_shapes, scale_xform);
    assert_eq!(
        app.scene().get(0).unwrap().fill_mesh().vertices.len(),
        initial_verts,
        "ドラッグ中は頂点バッファを再利用すること"
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::CommitTransformDrag {
            affine: scale_xform,
            orig_pivot: pivot,
            had_custom_pivot: false,
            changed: true,
        },
    );
    let rebuilt_verts = app.scene().get(0).unwrap().fill_mesh().vertices.len();
    assert!(
        rebuilt_verts > initial_verts,
        "拡大確定時 (CommitTransformDrag) にメッシュが再構築され、拡大後の曲線に応じた滑らかな頂点数になること (#5): initial={initial_verts}, rebuilt={rebuilt_verts}"
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(300.0, 250.0),
            Point::new(420.0, 250.0),
            18.0,
            Color32::from_rgb(220, 60, 60),
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    let v0_world = scene_summary(&app).shapes[0].exterior.segments[0].p0;
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
        scene_summary(&app).shapes[0]
            .exterior
            .segments
            .iter()
            .any(|seg| (seg.p0 - expected_moved_v0).hypot() < 2.0),
        "選択ツールで頂点をドラッグして移動できること"
    );

    // 3. 自由変形ツールでのスケールハンドルドラッグ (Scale) & 回転ドラッグ (Rotate) の RawInput 検証
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool {
            tool: Tool::Transform,
        },
    );
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
    let p0_before_rot = scene_summary(&app).shapes[0].exterior.segments[0].p0;
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
    let p0_after_rot = app.scene().get(0).unwrap().exterior.segments[0].p0;
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
        let mut cur = Point::new(rng.f64() * 600.0 + 50.0, rng.f64() * 350.0 + 50.0);
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
            let sid = if rng.bool() {
                i as u64
            } else {
                u64::MAX - (i as u64)
            };
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
            (1..=60).contains(&app.doc.fps),
            "case {case_idx}: FPS が 1..=60 にクランプされること"
        );
        assert!(
            app.doc.root.current_frame < app.doc.root.total_frames,
            "case {case_idx}: current_frame が total_frames 未満に収まること"
        );
        let mut has_cycle = false;
        for sym in &app.doc.library {
            assert!(
                sym.id >= 1,
                "case {case_idx}: シンボルIDが 1 以上に正規化されること"
            );
            for inst in sym.timeline.instances() {
                if app
                    .doc
                    .would_create_symbol_cycle(sym.id, inst.binding.symbol_id)
                {
                    has_cycle = true;
                }
            }
        }
        if has_cycle {
            assert!(
                app.doc.symbol_nest_exceeded(),
                "case {case_idx}: 循環参照を含む場合は symbol_nest_exceeded() が true になること"
            );
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
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(104.0, 100.0),
            2.0,
            Color32::RED,
        ),
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
        let target_path =
            std::env::temp_dir().join(format!("is2d_symlink_target_{}.is2d", std::process::id()));
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
            target_content.contains("\"version\":2"),
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
            s.exterior.segments.len() + s.holes.iter().map(|h| h.segments.len()).sum::<usize>()
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
            Action::ExitGroupSteps { steps: 1 },
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

    run(&mut app, &[Action::SelectAll, Action::Cut]);
    assert_eq!(cel_summary(&app.doc, lid, 0).shape_count, 0);

    run(
        &mut app,
        &[Action::Paste { in_place: true }, Action::Deselect],
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
            Action::EndCoalesce { key: None },
        ],
    );
    assert_eq!(app.history.undo_stack.len(), undo_base + 1);
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.doc.stage_size, orig_stage);
    run(&mut app, &[Action::Redo]);
    assert_eq!(app.doc.stage_size, Vec2::new(1280.0, 720.0));

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
            Action::EndCoalesce { key: None },
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
            Action::EndCoalesce { key: None },
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
    assert!(
        (before_save.evaluated_total_area - after_load.evaluated_total_area).abs()
            / before_save.evaluated_total_area
            < 1e-4
    );
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

    // タイムラインからの再評価（refresh_scene_base）や同じキーフレーム内のホールドフレーム移動でも選択が保たれること
    app.session.base_cache = Default::default();
    app.refresh_scene_base();
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
    let eval_shape_id_before = scene_summary(&app).shapes[0].shape_id;

    app.session.base_cache = Default::default();
    app.refresh_scene_base();
    let eval_shape_id_after = scene_summary(&app).shapes[0].shape_id;
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
    let s0_id = app.scene().get(0).unwrap().shape_id;
    assert_eq!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            Point::new(140.0, 100.0),
            app.view.zoom,
            app.tool.select_sub_mode,
        ),
        HoverTarget::Fill { shape_id: s0_id },
        "compute_hover_target が Scene<'_> を読み取って当たり判定を返すこと"
    );

    // 2. SceneBaseCache 経由の再更新が既存メッシュキャッシュを引き継ぐこと
    let mesh_arc_before = std::sync::Arc::clone(app.scene().get(0).unwrap().fill_mesh());
    app.refresh_scene_base();
    assert_eq!(app.scene().len(), 1);
    assert!(
        std::sync::Arc::ptr_eq(app.scene().get(0).unwrap().fill_mesh(), &mesh_arc_before),
        "SceneBaseCache は幾何・色が一致する図形のテッセレーション済みメッシュキャッシュを O(1) で引き継ぐこと"
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
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            tl_handle,
            app.view.zoom
        ),
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

    run(&mut app, &[Action::ExitGroupSteps { steps: 1 }]);
    assert_eq!(
        app.scene().len(),
        2,
        "シンボル編集から親タイムラインへ戻った後も、展開後の2図形が Scene に反映されていること"
    );
    assert!(
        app.scene().iter().all(|s| s.symbol_instance.is_some()),
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
    put_cel_shapes(&mut app, 0, 0, shapes);
    assert_eq!(app.scene().len(), 2000);

    // ドラッグ前の描画（Scene 経由で全2000図形の fill_mesh() を評価・キャッシュ構築）
    let meshes_before: std::collections::HashMap<u64, std::sync::Arc<egui::epaint::Mesh>> = app
        .scene()
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
    app.session.drag =
        canvas_tool::ActiveCanvasDrag::Select(canvas_tool::SelectDragState::MoveShape(move_drag));
    tool_select::update_reshape_drag(&mut app, Point::new(28.0, 22.0));

    // ドラッグ中（マウス移動1回目直後）に Scene から fill_mesh() を読んだとき、
    // 動かしていない残り1999個の図形がすべてドラッグ前と同一の Arc<Mesh> を保持していること
    let mut unmoved_during_drag = 0usize;
    for shape in app.scene().iter() {
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
    for shape in app.scene().iter() {
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
    put_cel_shapes(&mut app, 0, 0, base_stroke);
    assert!(app.session.floating().is_none());

    // 1. マーキー矩形スライスで Floating(modified=false) が生成され、未変更で選択解除すると元の1図形に戻ること
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
        .floating()
        .expect("マーキー矩形スライス直後に Floating が生成されること");
    assert!(!fl.modified);
    assert_eq!(fl.pieces.len(), 1);
    assert_eq!(fl.remainder.len(), 1);
    assert_eq!(fl.hidden.len(), 1);

    run(&mut app, &[Action::Deselect]);
    assert!(app.session.floating().is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "未変更のマーキースライスは選択解除時に破棄され元の1図形に戻ること"
    );

    // 2. マーキースライス後に移動すると Floating.modified が true になり、Action::CommitFloating で1回だけ結合確定されること
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
        .floating()
        .expect("スライス断片の移動後も Floating が保持されること");
    assert!(fl_moved.modified);

    // 重なっている同色断片が Action::CommitFloating で1図形に Union 結合され、Floating が空になること
    run(&mut app, &[Action::CommitFloating]);
    assert!(app.session.floating().is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "CommitFloating により重なった同色断片が結合され1図形になること"
    );

    // 3. ペーストで Floating(modified=true) が生成され、CommitFloating で重なった既存図形と結合されること
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
        .floating()
        .expect("ペースト直後に Floating が生成されること");
    assert!(fl_paste.modified);
    assert_eq!(app.scene().len(), 2);

    run(&mut app, &[Action::CommitFloating]);
    assert!(app.session.floating().is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "インプレースペーストした同色図形が CommitFloating で1図形に結合されること"
    );
}

#[test]
fn test_stage5_anchor_edit_preserved_after_nudge_deselect_and_frame_scrub() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 220);

    // 上側（y=100）に赤ストローク、下側（y=260）に青ストロークを描画し、フレームを2コマへ延長しておく
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(300.0, 100.0)],
                radius: 20.0,
                color: red,
            },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 260.0), Point::new(300.0, 260.0)],
                radius: 20.0,
                color: blue,
            },
            Action::ExtendFrame,
        ],
    );
    assert_eq!(app.scene().len(), 2);
    let initial_red_segs = app.scene().get(0).unwrap().exterior.segments.len();
    let initial_blue_segs = app.scene().get(1).unwrap().exterior.segments.len();

    // 1. 選択中の図形に対する AddAnchor / DeleteAnchor → Deselect 後も線分数が保たれること
    run(
        &mut app,
        &[
            Action::SelectSingleAt {
                pt: Point::new(200.0, 100.0),
            },
            Action::NudgeSelection {
                delta: KVec2::new(10.0, 0.0),
            },
            // Nudge 後の赤ストローク下辺 (210, 120) にアンカーを2つ追加し、1つ削除（+2 - 1 = +1）
            Action::AddAnchor {
                pt: Point::new(180.0, 120.0),
            },
            Action::AddAnchor {
                pt: Point::new(240.0, 120.0),
            },
            Action::DeleteAnchor {
                pt: Point::new(240.0, 120.0),
            },
        ],
    );
    assert!(app.session.has_modified_floating());
    assert_eq!(
        app.scene().get(1).unwrap().exterior.segments.len(),
        initial_red_segs + 1,
        "選択中図形への AddAnchor(+2) と DeleteAnchor(-1) が反映されていること"
    );

    run(&mut app, &[Action::Deselect]);
    let red_after_deselect = app
        .scene()
        .iter()
        .find(|s| s.color == red)
        .unwrap()
        .exterior
        .segments
        .len();
    assert_eq!(
        red_after_deselect,
        initial_red_segs + 1,
        "Deselect 後も選択中図形に行った AddAnchor / DeleteAnchor の線分数が失われないこと"
    );

    // 2. 選択していない別の図形に対する AddAnchor / DeleteAnchor → Deselect およびフレーム移動後も線分数が保たれること
    run(
        &mut app,
        &[
            Action::SelectSingleAt {
                pt: Point::new(210.0, 100.0),
            },
            Action::NudgeSelection {
                delta: KVec2::new(10.0, 0.0),
            },
            // 選択していない青ストローク（y=260, 下辺 y=280）にアンカーを2つ追加し、1つ削除（+2 - 1 = +1）
            Action::AddAnchor {
                pt: Point::new(180.0, 280.0),
            },
            Action::AddAnchor {
                pt: Point::new(240.0, 280.0),
            },
            Action::DeleteAnchor {
                pt: Point::new(240.0, 280.0),
            },
        ],
    );
    assert!(app.session.has_modified_floating());
    let blue_before_commit = app
        .scene()
        .iter()
        .find(|s| s.color == blue)
        .unwrap()
        .exterior
        .segments
        .len();
    assert_eq!(blue_before_commit, initial_blue_segs + 1);

    // フレーム移動（ScrubRulerToFrame）で確定されたあとも線分数が保たれること
    run(&mut app, &[Action::ScrubRulerToFrame { frame: 1 }]);
    let summary_after_scrub = cel_summary(&app.doc, lid, 1);
    assert_eq!(
        summary_after_scrub.total_segments,
        (initial_red_segs + 1) + (initial_blue_segs + 1),
        "フレーム移動後も選択図形・非選択図形双方のアンカー編集後の線分数が保たれていること"
    );

    // Undo / Redo でもアンカー編集結果が失われないこと
    app.undo();
    app.redo();
    let summary_after_redo = cel_summary(&app.doc, lid, 0);
    assert_eq!(
        summary_after_redo.total_segments,
        (initial_red_segs + 1) + (initial_blue_segs + 1),
        "Undo -> Redo 後もアンカー追加・削除後の線分数が維持されること"
    );
}

#[test]
fn test_stage6b_non_uniform_scale_rotated_symbol_edit_preserves_untouched_shape_bits() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 220);

    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(220.0, 115.0)],
                radius: 14.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(160.0, 107.0),
            },
            Action::ConvertToSymbol,
        ],
    );
    let sym_id = app.doc.library[0].id;
    let orig_local_shape = app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes[0]
        .clone();
    let orig_bits: Vec<[u64; 6]> = orig_local_shape
        .exterior
        .segments
        .iter()
        .map(|seg| {
            [
                seg.p0.x.to_bits(),
                seg.p0.y.to_bits(),
                seg.p1.x.to_bits(),
                seg.p1.y.to_bits(),
                seg.p2.x.to_bits(),
                seg.p2.y.to_bits(),
            ]
        })
        .collect();

    // 非一様スケール（sx != sy）＋回転をインスタンスに適用
    run(
        &mut app,
        &[
            Action::ScaleSelection { sx: 1.73, sy: 0.61 },
            Action::RotateSelection { angle_rad: 0.47 },
        ],
    );
    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();

    // インスタンスに入り、別の場所に50回ストロークを描いて出る
    run(&mut app, &[Action::EnterSymbolTimeline { binding }]);
    assert_eq!(app.session.edit_path.len(), 1);
    for i in 0..50 {
        let y = 260.0 + (i as f64) * 2.5;
        run(
            &mut app,
            &[Action::PaintStroke {
                points: vec![Point::new(80.0, y), Point::new(140.0, y)],
                radius: 4.0,
                color: blue,
            }],
        );
    }
    run(&mut app, &[Action::ExitSymbolTimeline]);
    assert!(app.session.edit_path.is_empty());

    let sym_shapes = &app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes;
    let preserved = sym_shapes
        .iter()
        .find(|s| s.shape_id == orig_local_shape.shape_id)
        .expect("未変更の既存図形が同一 shape_id で保持されていること");
    let after_bits: Vec<[u64; 6]> = preserved
        .exterior
        .segments
        .iter()
        .map(|seg| {
            [
                seg.p0.x.to_bits(),
                seg.p0.y.to_bits(),
                seg.p1.x.to_bits(),
                seg.p1.y.to_bits(),
                seg.p2.x.to_bits(),
                seg.p2.y.to_bits(),
            ]
        })
        .collect();
    assert_eq!(
        orig_bits, after_bits,
        "非一様スケール＋回転インスタンス内で50回別箇所に描画して出ても、既存の未変更図形の頂点座標が to_bits() 単位で完全一致すること"
    );
}

#[test]
fn test_stage6b_save_inside_two_level_nested_symbol_matches_save_after_exit_to_root() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(40, 180, 90);
    let blue = Color32::from_rgb(60, 120, 220);

    // 内側シンボル（Symbol 1）を作成し、さらにそれを包む外側シンボル（Symbol 2）を作成して2段ネストにする
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
                radius: 12.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(140.0, 100.0),
            },
            Action::ConvertToSymbol,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 160.0), Point::new(180.0, 160.0)],
                radius: 12.0,
                color: green,
            },
            Action::SelectAll,
            Action::ConvertToSymbol,
        ],
    );
    assert_eq!(app.doc.library.len(), 2);

    // 1段目（外側シンボル）に入り、さらに2段目（内側シンボル）へ入る
    let (outer_binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    run(
        &mut app,
        &[Action::EnterSymbolTimeline {
            binding: outer_binding,
        }],
    );
    assert_eq!(app.session.edit_path.len(), 1);
    run(
        &mut app,
        &[Action::EnterGroupAt {
            pt: Point::new(140.0, 100.0),
        }],
    );
    assert_eq!(app.session.edit_path.len(), 2);

    // 2段目の中でストロークを追加
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 220.0), Point::new(180.0, 220.0)],
            radius: 10.0,
            color: blue,
        }],
    );

    let tmp_dir = std::env::temp_dir();
    let path_nested = tmp_dir.join("is2d_stage6b_nested_save.is2d");
    let path_root = tmp_dir.join("is2d_stage6b_root_save.is2d");

    // 2段ネストに入ったまま保存
    project_io::save_project_to_path(&mut app, &path_nested).unwrap();
    assert_eq!(app.session.edit_path.len(), 2);

    // ルートに出てから保存
    run(&mut app, &[Action::ExitSymbolToDepth { depth: 0 }]);
    assert!(app.session.edit_path.is_empty());
    project_io::save_project_to_path(&mut app, &path_root).unwrap();

    let json_nested = std::fs::read_to_string(&path_nested).unwrap();
    let json_root = std::fs::read_to_string(&path_root).unwrap();
    let _ = std::fs::remove_file(&path_nested);
    let _ = std::fs::remove_file(&path_root);

    assert_eq!(
        json_nested, json_root,
        "2段ネストのシンボルに入った状態で保存したファイルと、ルートに出てから保存したファイルが完全一致すること"
    );
}

#[test]
fn test_stage6b_timeline_ops_f6_f7_add_layer_inside_symbol_modify_library_symbol_def() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);

    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(120.0, 120.0), Point::new(200.0, 120.0)],
                radius: 12.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(160.0, 120.0),
            },
            Action::ConvertToSymbol,
        ],
    );
    let sym_id = app.doc.library[0].id;
    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();

    run(&mut app, &[Action::EnterSymbolTimeline { binding }]);
    assert_eq!(app.session.edit_path.len(), 1);
    assert_eq!(app.doc.root.total_frames, 1);
    assert_eq!(app.doc.root.layers.len(), 1);

    // シンボル内で F6（キーフレーム複製挿入）、F7（空白キーフレーム挿入）、レイヤー追加を実行
    run(
        &mut app,
        &[
            Action::InsertKeyframe { copy_prev: true },
            Action::InsertKeyframe { copy_prev: false },
            Action::AddLayer,
        ],
    );

    // ルートタイムラインは変わらず、doc.library 内の SymbolDef のタイムラインが直接更新されていること
    assert_eq!(app.doc.root.total_frames, 1);
    assert_eq!(app.doc.root.layers.len(), 1);

    let sym_tl = &app.doc.find_symbol(sym_id).unwrap().timeline;
    assert_eq!(sym_tl.total_frames, 3);
    assert_eq!(sym_tl.layers.len(), 2);
    assert_eq!(sym_tl.layers[0].keyframes.len(), 3);
    assert!(!sym_tl.layers[0].keyframes[1].cel.is_empty());
    assert!(sym_tl.layers[0].keyframes[2].cel.is_empty());
}

#[test]
fn test_stage6b_undo_redo_across_before_inside_and_after_symbol_restores_doc_and_edit_path() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(40, 180, 90);
    let blue = Color32::from_rgb(60, 120, 220);

    // 1. シンボルに入る前の操作（ストローク描画 → シンボル化 → ルート上で移動）
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
                radius: 12.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(140.0, 100.0),
            },
            Action::ConvertToSymbol,
            Action::MoveSelected {
                delta: KVec2::new(20.0, 0.0),
            },
        ],
    );
    let sym_id = app.doc.library[0].id;
    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();

    // 2. シンボルに入って内部でストローク描画＋F6 キーフレーム挿入
    run(
        &mut app,
        &[
            Action::EnterSymbolTimeline { binding },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 160.0), Point::new(180.0, 160.0)],
                radius: 12.0,
                color: green,
            },
            Action::InsertKeyframe { copy_prev: true },
        ],
    );
    assert_eq!(app.session.edit_path.len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.total_frames,
        2
    );

    // 3. シンボルから出た後の操作（ルートに青ストローク描画）
    run(
        &mut app,
        &[
            Action::ExitSymbolTimeline,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 260.0), Point::new(180.0, 260.0)],
                radius: 12.0,
                color: blue,
            },
        ],
    );
    assert!(app.session.edit_path.is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 1);

    // Undo 1: ルートの青ストロークを取り消す（まだルート階層）
    run(&mut app, &[Action::Undo]);
    assert!(app.session.edit_path.is_empty());
    assert!(app.doc.root.layers[0].keyframes[0].cel.shapes.is_empty());

    // Undo 2: シンボル内の F6 キーフレーム挿入を取り消す（EditPath がシンボル内深さ1へ戻る）
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.session.edit_path.len(), 1);
    assert_eq!(app.session.edit_path.last().unwrap().symbol_id, sym_id);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.total_frames,
        1
    );
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes
            .len(),
        2
    );

    // Undo 3: シンボル内の緑ストローク描画を取り消す（EditPath はシンボル内深さ1のまま、内部図形が1個へ戻る）
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.session.edit_path.len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes
            .len(),
        1
    );

    // Undo 4: シンボルに入る前の MoveSelected を取り消す（EditPath がルートへ戻る）
    run(&mut app, &[Action::Undo]);
    assert!(app.session.edit_path.is_empty());
    assert_eq!(app.doc.library.len(), 1);

    // Undo 5: シンボル化（ConvertToSymbol）自体を取り消す（ルート、ライブラリ空）
    run(&mut app, &[Action::Undo]);
    assert!(app.session.edit_path.is_empty());
    assert!(app.doc.library.is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 1);

    // Redo 1..5 で順番にすべて復元されること
    run(&mut app, &[Action::Redo]);
    assert!(app.session.edit_path.is_empty());
    assert_eq!(app.doc.library.len(), 1);

    run(&mut app, &[Action::Redo]);
    assert_eq!(app.session.edit_path.len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes
            .len(),
        1
    );

    run(&mut app, &[Action::Redo]);
    assert_eq!(app.session.edit_path.len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes
            .len(),
        2
    );

    run(&mut app, &[Action::Redo]);
    assert!(app.session.edit_path.is_empty());
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.total_frames,
        2
    );

    run(&mut app, &[Action::Redo]);
    assert!(app.session.edit_path.is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 1);
}

#[test]
fn test_stage7a_drag_preview_uses_overlay_without_mutating_working_copy() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 220);

    let mut shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        20.0,
        red,
    );
    shapes.extend(make_stroke(
        Point::new(150.0, 100.0),
        Point::new(250.0, 100.0),
        20.0,
        blue,
    ));
    put_cel_shapes(&mut app, 0, 0, shapes);

    let orig_red_bbox = app.doc.root.layers[0].keyframes[0].cel.shapes[0].bounding_box();
    let orig_blue_bbox = app.doc.root.layers[0].keyframes[0].cel.shapes[1].bounding_box();

    // 1. 選択移動ドラッグ中は文書のセルが変わらず、Scene の最前面（TopOfLayer）にプレビューが出る
    let red_id = app.doc.root.layers[0].keyframes[0].cel.shapes[0].shape_id;
    {
        let s0 = app.doc.root.layers[0].keyframes[0].cel.shapes[0].clone();
        app.session.selection.insert_shape(&s0);
    }
    tool_select::start_reshape_drag(&mut app, Point::new(120.0, 100.0), false, false);
    tool_select::update_reshape_drag(&mut app, Point::new(120.0, 140.0));
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes[0].bounding_box(),
        orig_red_bbox,
        "選択移動ドラッグ中は文書のセルが書き換えられないこと"
    );
    // 移動中の生シェイプは TopOfLayer で青図形より手前（index 1）に表示される
    let scene_last = app.scene().get(1).unwrap();
    assert_eq!(scene_last.shape_id, red_id);
    assert!((scene_last.bounding_box().y0 - (orig_red_bbox.y0 + 40.0)).abs() < 1e-5);
    tool_select::finish_reshape_drag(&mut app, Some(Point::new(120.0, 140.0)));
    assert!(
        (app.scene().get(1).unwrap().bounding_box().y0 - (orig_red_bbox.y0 + 40.0)).abs() < 1e-5
    );
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.scene().get(0).unwrap().bounding_box(), orig_red_bbox);
    assert_eq!(app.scene().get(1).unwrap().bounding_box(), orig_blue_bbox);

    // 2. Alt 複製ドラッグ中は文書のセルが増えず、Scene の Overlay に複製プレビューが追加される
    {
        let s0 = app.doc.root.layers[0].keyframes[0].cel.shapes[0].clone();
        app.session.selection.insert_shape(&s0);
    }
    tool_select::start_reshape_drag(&mut app, Point::new(120.0, 100.0), false, true);
    tool_select::update_reshape_drag(&mut app, Point::new(120.0, 150.0));
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes.len(),
        2,
        "Alt 複製ドラッグ中は文書のセルに複製が書き込まれないこと"
    );
    assert_eq!(
        app.scene().len(),
        3,
        "Alt 複製ドラッグ中は Scene に元の2個 + 複製プレビュー1個の計3個が見えること"
    );
    tool_select::finish_reshape_drag(&mut app, Some(Point::new(120.0, 150.0)));
    run(&mut app, &[Action::Deselect]);
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 2);

    // 3. 自由変形（拡大・回転）ドラッグ中は文書のセルが変わらず、Scene に変形プレビューが出る
    {
        let s0 = app.doc.root.layers[0].keyframes[0].cel.shapes[0].clone();
        app.session.selection.insert_shape(&s0);
    }
    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, 1.0)
        .unwrap();
    let tr_corner = Point::new(bbox.x1, bbox.y0);
    tool_transform::start_transform_drag(&mut app, tr_corner, false, false);
    tool_transform::update_transform_drag(
        &mut app,
        Point::new(bbox.x1 + 30.0, bbox.y0 - 10.0),
        false,
        false,
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes[0].bounding_box(),
        orig_red_bbox,
        "自由変形スケール中は文書のセルが書き換えられないこと"
    );
    let preview_bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, 1.0)
        .unwrap();
    assert!(preview_bbox.width() > orig_red_bbox.width() + 20.0);
    tool_transform::finish_transform_drag(
        &mut app,
        Some(Point::new(bbox.x1 + 30.0, bbox.y0 - 10.0)),
    );
    let committed_red = app.scene().iter().find(|s| s.shape_id == red_id).unwrap();
    assert!(committed_red.bounding_box().width() > orig_red_bbox.width() + 20.0);
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.scene().get(0).unwrap().bounding_box(), orig_red_bbox);
}

#[test]
fn test_stage7b_floating_holds_pieces_without_mutating_cels_until_commit() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 220);

    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(260.0, 100.0)],
                radius: 14.0,
                color: red,
            },
            Action::PaintStroke {
                points: vec![Point::new(180.0, 180.0), Point::new(180.0, 220.0)],
                radius: 14.0,
                color: blue,
            },
        ],
    );
    let rev_initial = app.doc.root.layers[0].keyframes[0].cel.revision;
    let cel_before = cel_summary(&app.doc, lid, 0);
    let orig_red = app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes
        .iter()
        .find(|s| s.color == red)
        .unwrap()
        .exterior
        .signed_area();

    // 1. マーキースライス時はすぐ Floating に持ち上げられ、文書のセルは一切書き換わらないこと
    run(
        &mut app,
        &[Action::SelectMarquee {
            p0: Point::new(80.0, 70.0),
            p1: Point::new(170.0, 130.0),
            additive: false,
        }],
    );
    assert!(app.session.floating().is_some());
    assert_eq!(app.scene().len(), 3);
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.revision, rev_initial,
        "マーキースライス直後は文書の Cel::revision が変化しないこと"
    );
    assert_eq!(
        cel_summary(&app.doc, lid, 0).shape_count,
        cel_before.shape_count
    );

    // 未変形のスライスを解除したときは Floating を捨てるだけで文書の Cel::revision は変化しないこと
    run(&mut app, &[Action::Deselect]);
    assert!(app.session.floating().is_none());
    assert_eq!(app.scene().len(), 2);
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.revision,
        rev_initial
    );

    // 2. 生の図形を丸ごと選んだときは、最初に動かしたときに Floating へ持ち上げられ、確定するまで文書のセルは変化しないこと
    run(
        &mut app,
        &[Action::SelectSingleAt {
            pt: Point::new(180.0, 200.0),
        }],
    );
    assert!(
        app.session.floating().is_none(),
        "生図形を丸ごとクリック選択した時点ではまだ Floating に入らないこと"
    );
    run(
        &mut app,
        &[
            Action::NudgeSelection {
                delta: KVec2::new(0.0, -50.0),
            },
            Action::NudgeSelection {
                delta: KVec2::new(0.0, -50.0),
            },
        ],
    );
    assert!(app.session.floating().is_some());
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.revision, rev_initial,
        "生図形を選択・移動している間（確定前）は文書のセルが変化しないこと"
    );
    assert_eq!(
        cel_summary(&app.doc, lid, 0).total_area,
        cel_before.total_area
    );

    // 3. タブ切り替えでは確定しないこと
    let ctx = egui::Context::default();
    crate::project_io::new_project(&mut app, &ctx);
    assert_eq!(app.active, 1);
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(
        app.session.floating().is_some(),
        "タブ切り替えでは Floating を確定せず維持すること"
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.revision,
        rev_initial
    );

    // 4. 保存は Document のコピーの上で Floating を確定して書き出し、Session も Document も書き換えないこと
    let tmp_save =
        std::env::temp_dir().join(format!("is2d_stage7b_save_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_save).unwrap();
    assert!(
        app.session.floating().is_some(),
        "保存後も Session の Floating はそのまま残ること"
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.revision, rev_initial,
        "保存後も Document の Cel は書き換えられないこと"
    );
    let mut loaded_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_save, None).unwrap();
    let _ = std::fs::remove_file(&tmp_save);
    let loaded_red: f64 = loaded_app
        .scene()
        .iter()
        .filter(|s| s.color == red)
        .map(|s| s.exterior.signed_area())
        .sum();
    assert!(
        loaded_red > 0.0 && loaded_red < orig_red - 50.0,
        "保存されたファイルには Floating を確定（切り抜き）した結果が書き出されていること"
    );

    // 5. Floating の確定は Undo の1手にならず、commit_id も変えないこと
    let commit_before = app.history.current_commit_id;
    let undo_len_before = app.history.undo_stack.len();
    run(&mut app, &[Action::Deselect]);
    assert!(app.session.floating().is_none());
    assert_eq!(app.history.current_commit_id, commit_before);
    assert_eq!(app.history.undo_stack.len(), undo_len_before);
    assert!(!app.history.is_dirty());
    assert_ne!(
        app.doc.root.layers[0].keyframes[0].cel.revision, rev_initial,
        "確定時に1回だけ文書の Cel が更新されること"
    );
}

#[test]
fn test_stage7c_scene_base_cache_and_edit_layers() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 220);

    // Layer 0 (下) と Layer 1 (上) を用意する
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
                radius: 12.0,
                color: red,
            },
            Action::AddLayer,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 200.0), Point::new(200.0, 200.0)],
                radius: 12.0,
                color: blue,
            },
        ],
    );
    assert_eq!(app.doc.root.layers.len(), 2);
    let layer_bottom_id = app.doc.root.layers[0].id;
    let layer_top_id = app.doc.root.layers[1].id;
    let rev_bottom_before = app.doc.root.layers[0].keyframes[0].cel.revision;
    let rev_top_before = app.doc.root.layers[1].keyframes[0].cel.revision;

    // 1. 下のレイヤーのメッシュキャッシュを取得しておき、上のレイヤーだけを edit_layers（ブラシ描画）で更新しても
    //    下のレイヤーは Cel::revision も SceneBaseCache の Arc<Mesh> も再評価されないこと
    let bottom_shape_id = app.scene().get(0).unwrap().shape_id;
    let bottom_mesh_arc_before = std::sync::Arc::clone(app.scene().get(0).unwrap().fill_mesh());

    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(220.0, 200.0), Point::new(260.0, 200.0)],
            radius: 12.0,
            color: blue,
        }],
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.revision, rev_bottom_before,
        "編集対象外のレイヤーの Cel::revision は変化しないこと"
    );
    assert_ne!(
        app.doc.root.layers[1].keyframes[0].cel.revision, rev_top_before,
        "編集対象レイヤーの Cel::revision のみが更新されること"
    );
    let bottom_after = app
        .scene()
        .iter()
        .find(|s| s.shape_id == bottom_shape_id)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(bottom_after.fill_mesh(), &bottom_mesh_arc_before),
        "未変更レイヤーの SceneBaseCache は再評価されず同一の Arc<Mesh> を保持すること"
    );

    // 2. edit_layers を直接呼んだ場合も指定したレイヤーのみ書き換わり、SceneBaseCache が即座に同期されること
    let changed = canvas_tool::edit_layers(&mut app.tabs[app.active], &[layer_bottom_id], |ed| {
        for s in &mut ed.shapes {
            s.color = Color32::GREEN;
        }
    });
    assert!(changed);
    assert_ne!(
        app.doc.root.layers[0].keyframes[0].cel.revision,
        rev_bottom_before
    );
    assert_eq!(app.scene().get(0).unwrap().color, Color32::GREEN);
    let _ = layer_top_id;
}

#[test]
fn test_stage7_z_order_group_symbol_ungroup_paste_alt_duplicate_and_move() {
    use crate::doc_actions::Action;

    fn make_rect_shape(x0: f64, y0: f64, x1: f64, y1: f64, color: Color32) -> FillShape {
        make_stroke(
            Point::new(x0 + 10.0, (y0 + y1) * 0.5),
            Point::new(x1 - 10.0, (y0 + y1) * 0.5),
            8.0,
            color,
        )
        .into_iter()
        .next()
        .unwrap()
    }

    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(60, 200, 90);
    let blue = Color32::from_rgb(60, 120, 220);
    let yellow = Color32::from_rgb(240, 200, 50);

    // 1. グループ化 (Cmd+G): 選択した図形を元の位置から取り除き、編集対象レイヤー・スコープの最前面に置く
    {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                make_rect_shape(0.0, 0.0, 40.0, 40.0, red),
                make_rect_shape(60.0, 0.0, 100.0, 40.0, green),
                make_rect_shape(120.0, 0.0, 160.0, 40.0, blue),
            ],
        );
        let red_shape = app.scene().get(0).unwrap().clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&red_shape);
        run(&mut app, &[Action::Group]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color).collect();
        assert_eq!(
            colors,
            vec![green, blue, red],
            "グループ化した図形は最前面に移動すること"
        );
        assert_eq!(app.scene().get(2).unwrap().group_path.len(), 1);
    }

    // 2. シンボル変換 (F8) と分解 (Cmd+B): シンボル変換は最前面、分解も最前面に置かれること
    {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                make_rect_shape(0.0, 0.0, 40.0, 40.0, red),
                make_rect_shape(60.0, 0.0, 100.0, 40.0, green),
            ],
        );
        let red_shape = app.scene().get(0).unwrap().clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&red_shape);
        run(&mut app, &[Action::ConvertToSymbol]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color).collect();
        assert_eq!(
            colors,
            vec![green, red],
            "シンボル変換したインスタンスは最前面に移動すること"
        );
        assert!(app.scene().get(1).unwrap().symbol_instance.is_some());

        // さらにもう1つ黄色の図形を最前面に描き、その下のシンボルを分解すると最前面に来ること
        run(
            &mut app,
            &[Action::PaintStroke {
                points: vec![Point::new(140.0, 20.0), Point::new(160.0, 20.0)],
                radius: 8.0,
                color: yellow,
            }],
        );
        let sym_shape = app
            .scene()
            .iter()
            .find(|s| s.symbol_instance.is_some())
            .unwrap()
            .clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&sym_shape);
        run(&mut app, &[Action::Ungroup]);
        let colors_after_break: Vec<_> = app.scene().iter().map(|s| s.color).collect();
        assert_eq!(
            colors_after_break,
            vec![green, yellow, red],
            "シンボル分解で展開された図形は最前面に置かれること"
        );
        assert!(app.scene().get(2).unwrap().symbol_instance.is_none());
    }

    // 3. グループ解除 (Cmd+B): 背面にあったグループを解除すると最前面に展開されること
    {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                make_rect_shape(0.0, 0.0, 40.0, 40.0, red),
                make_rect_shape(60.0, 0.0, 100.0, 40.0, green),
            ],
        );
        let red_shape = app.scene().get(0).unwrap().clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&red_shape);
        run(&mut app, &[Action::Group]);
        let grouped_shape = app.scene().get(1).unwrap().clone();
        // さらに青の図形を上に追加してグループを中間（背面側）にする
        run(
            &mut app,
            &[Action::PaintStroke {
                points: vec![Point::new(140.0, 20.0), Point::new(160.0, 20.0)],
                radius: 8.0,
                color: blue,
            }],
        );
        app.session.selection.clear();
        app.session.selection.insert_shape(&grouped_shape);
        run(&mut app, &[Action::Ungroup]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color).collect();
        assert_eq!(
            colors,
            vec![green, blue, red],
            "グループ解除された図形は最前面に置かれること"
        );
        assert!(app.scene().get(2).unwrap().group_path.is_empty());
    }

    // 4. ペーストと Alt 複製: 最前面に置かれること（Alt 複製は元の図形を元の位置に残す）
    {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                make_rect_shape(0.0, 0.0, 40.0, 40.0, red),
                make_rect_shape(60.0, 0.0, 100.0, 40.0, green),
            ],
        );
        let red_shape = app.scene().get(0).unwrap().clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&red_shape);
        run(&mut app, &[Action::Copy, Action::Paste { in_place: false }]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color).collect();
        assert_eq!(
            colors,
            vec![red, green, red],
            "ペーストした図形は最前面に置かれること"
        );
        run(&mut app, &[Action::Deselect]);

        // Alt 複製（元の red は index 0 に残したまま、複製された red が最前面 index 2 に置かれること）
        let mut app_alt = Is2dApp::default();
        put_cel_shapes(
            &mut app_alt,
            0,
            0,
            vec![
                make_rect_shape(0.0, 0.0, 40.0, 40.0, red),
                make_rect_shape(60.0, 0.0, 100.0, 40.0, green),
            ],
        );
        let first_red = app_alt.scene().get(0).unwrap().clone();
        app_alt.session.selection.clear();
        app_alt.session.selection.insert_shape(&first_red);
        run(
            &mut app_alt,
            &[Action::FinishMoveSelectionDrag {
                total_delta: KVec2::new(0.0, 80.0),
                alt_duplicate: true,
            }],
        );
        let colors_after_alt: Vec<_> = app_alt.scene().iter().map(|s| s.color).collect();
        assert_eq!(
            colors_after_alt,
            vec![red, green, red],
            "Alt 複製した図形は元の図形を背面に残したまま最前面に置かれること"
        );
    }

    // 5. 重なる2つのシンボル: 下側のシンボルを動かしても重なり順（下側のまま）を保つこと
    {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                make_rect_shape(0.0, 0.0, 60.0, 60.0, red),
                make_rect_shape(20.0, 20.0, 80.0, 80.0, blue),
            ],
        );
        let red_shape = app.scene().get(0).unwrap().clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&red_shape);
        run(&mut app, &[Action::ConvertToSymbol]);
        // 変換で BottomSym が最前面に来たので、残りの blue もシンボル化して TopSym を最前面にする
        let blue_shape = app.scene().get(0).unwrap().clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&blue_shape);
        run(&mut app, &[Action::ConvertToSymbol]);
        let bottom_sym = app.scene().get(0).unwrap().clone();
        let bottom_gid = bottom_sym.symbol_instance.as_ref().unwrap().instance_gid;
        let top_gid = app
            .scene()
            .get(1)
            .unwrap()
            .symbol_instance
            .as_ref()
            .unwrap()
            .instance_gid;
        assert_eq!(app.scene().get(0).unwrap().color, red);
        assert_eq!(app.scene().get(1).unwrap().color, blue);

        // 下側のシンボル (BottomSym) を TopSym とさらに重なる位置へ移動
        app.session.selection.clear();
        app.session.selection.insert_shape(&bottom_sym);
        app.session.begin_move_overlay(false);
        app.session.translate_move_overlay(KVec2::new(10.0, 10.0));
        assert_eq!(
            app.scene()
                .get(0)
                .unwrap()
                .symbol_instance
                .as_ref()
                .unwrap()
                .instance_gid,
            bottom_gid,
            "ドラッグ中の仮表示でも下側シンボルは重なり順を維持すること"
        );
        run(
            &mut app,
            &[Action::FinishMoveSelectionDrag {
                total_delta: KVec2::new(10.0, 10.0),
                alt_duplicate: false,
            }],
        );
        assert_eq!(
            app.scene()
                .get(0)
                .unwrap()
                .symbol_instance
                .as_ref()
                .unwrap()
                .instance_gid,
            bottom_gid,
            "移動確定後も下側シンボルは上側シンボルの下に留まること"
        );
        assert_eq!(
            app.scene()
                .get(1)
                .unwrap()
                .symbol_instance
                .as_ref()
                .unwrap()
                .instance_gid,
            top_gid
        );
    }

    // 6. グループを生の図形の上に動かす場合: 重なり順を維持し、生の図形を削らないこと
    {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                make_rect_shape(0.0, 0.0, 80.0, 80.0, red),
                make_rect_shape(120.0, 0.0, 160.0, 40.0, green),
            ],
        );
        let orig_red_area = app.scene().get(0).unwrap().exterior.signed_area().abs();
        let green_shape = app.scene().get(1).unwrap().clone();
        app.session.selection.clear();
        app.session.selection.insert_shape(&green_shape);
        run(&mut app, &[Action::Group]);
        let grouped_green = app.scene().get(1).unwrap().clone();

        // グループを生の図形 (red) の上に重ねて移動し、選択解除する
        app.session.selection.clear();
        app.session.selection.insert_shape(&grouped_green);
        run(
            &mut app,
            &[
                Action::FinishMoveSelectionDrag {
                    total_delta: KVec2::new(-100.0, 10.0),
                    alt_duplicate: false,
                },
                Action::Deselect,
            ],
        );
        assert_eq!(app.scene().len(), 2);
        assert_eq!(app.scene().get(0).unwrap().color, red);
        assert_eq!(app.scene().get(1).unwrap().color, green);
        let red_area = app.scene().get(0).unwrap().exterior.signed_area().abs();
        assert!(
            (red_area - orig_red_area).abs() < 1.0,
            "グループを生の図形の上に動かしても生の図形は削られないこと (area={red_area}, orig={orig_red_area})"
        );
    }
}

#[test]
fn test_stage7_edit_layers_preserves_mesh_cache_without_rebuilding_from_doc() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let mut initial_shapes = Vec::with_capacity(2000);
    for i in 0..2000 {
        let col = i % 50;
        let row = i / 50;
        let x0 = (col as f64) * 30.0;
        let y0 = (row as f64) * 30.0;
        let c = Color32::from_rgb(
            ((i * 37) % 200 + 40) as u8,
            ((i * 73) % 200 + 40) as u8,
            150,
        );
        initial_shapes.extend(make_stroke(
            Point::new(x0, y0 + 10.0),
            Point::new(x0 + 20.0, y0 + 10.0),
            10.0,
            c,
        ));
    }
    put_cel_shapes(&mut app, 0, 0, initial_shapes);

    // 1. ルートでの 1 ストローク描画後、既存の未変更図形の描画キャッシュ (Arc<Mesh>) がそのまま引き継がれていること
    let untouched_id = app.scene().get(0).unwrap().shape_id;
    let mesh_before = std::sync::Arc::clone(app.scene().get(0).unwrap().fill_mesh());
    let t0 = std::time::Instant::now();
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(1600.0, 700.0), Point::new(1640.0, 700.0)],
            radius: 10.0,
            color: Color32::WHITE,
        }],
    );
    let root_elapsed = t0.elapsed();
    let untouched_after = app
        .scene()
        .iter()
        .find(|s| s.shape_id == untouched_id)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(untouched_after.fill_mesh(), &mesh_before),
        "edit_layers 後に未変更図形の描画キャッシュ (Arc<Mesh>) が複製や再構築なしでそのまま引き継がれること"
    );

    // 2. 回転したシンボル内部でも同様に描画キャッシュがそのまま引き継がれること
    let all_shapes: Vec<_> = app.scene().iter().cloned().collect();
    app.session.selection.clear();
    for s in &all_shapes {
        app.session.selection.insert_shape(s);
    }
    run(&mut app, &[Action::ConvertToSymbol]);
    let sym_shape = app.scene().get(0).unwrap().clone();
    app.session.selection.clear();
    app.session.selection.insert_shape(&sym_shape);
    run(&mut app, &[Action::RotateSelection { angle_rad: 0.35 }]);
    let binding = app.scene().get(0).unwrap().symbol_instance.unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app.tabs[app.active],
        binding
    ));
    let sym_untouched_id = app.scene().get(0).unwrap().shape_id;
    let sym_mesh_before = std::sync::Arc::clone(app.scene().get(0).unwrap().fill_mesh());
    let t1 = std::time::Instant::now();
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(1700.0, 800.0), Point::new(1740.0, 800.0)],
            radius: 10.0,
            color: Color32::YELLOW,
        }],
    );
    let sym_elapsed = t1.elapsed();
    let _ = (root_elapsed, sym_elapsed);
    let sym_untouched_after = app
        .scene()
        .iter()
        .find(|s| s.shape_id == sym_untouched_id)
        .unwrap();
    assert!(
        std::sync::Arc::ptr_eq(sym_untouched_after.fill_mesh(), &sym_mesh_before),
        "回転したシンボル内の edit_layers 後も未変更図形の Arc<Mesh> がそのまま引き継がれること"
    );
}

fn contour_bits(c: &vector_engine::Contour) -> Vec<[u64; 6]> {
    c.segments
        .iter()
        .map(|s| {
            [
                s.p0.x.to_bits(),
                s.p0.y.to_bits(),
                s.p1.x.to_bits(),
                s.p1.y.to_bits(),
                s.p2.x.to_bits(),
                s.p2.y.to_bits(),
            ]
        })
        .collect()
}

fn run_and_verify_scene_cache_consistency(
    app: &mut Is2dApp,
    action: crate::doc_actions::Action,
    label: &str,
) {
    run(app, &[action]);
    let before: Vec<FillShape> = app.scene().iter().cloned().collect();

    // 1. clear_overlay() と、土台のキャッシュを空から作り直す処理を実行
    app.tabs[app.active].session.base_cache = Default::default();
    app.tabs[app.active].refresh_scene_base();
    app.tabs[app.active].session.clear_overlay();

    let after: Vec<FillShape> = app.scene().iter().cloned().collect();
    assert_eq!(
        before.len(),
        after.len(),
        "[{label}] Scene の図形数がキャッシュ再構築前後で一致すること"
    );
    for (idx, (a, b)) in before.iter().zip(after.iter()).enumerate() {
        assert_eq!(a.color, b.color, "[{label} #{idx}] color mismatch");
        assert_eq!(a.layer_id, b.layer_id, "[{label} #{idx}] layer_id mismatch");
        assert_eq!(
            a.group_path, b.group_path,
            "[{label} #{idx}] group_path mismatch"
        );
        assert_eq!(
            a.symbol_instance, b.symbol_instance,
            "[{label} #{idx}] symbol_instance mismatch"
        );
        assert_eq!(
            contour_bits(&a.exterior),
            contour_bits(&b.exterior),
            "[{label} #{idx}] exterior bit-level mismatch"
        );
        assert_eq!(
            a.holes.len(),
            b.holes.len(),
            "[{label} #{idx}] holes len mismatch"
        );
        for (ha, hb) in a.holes.iter().zip(b.holes.iter()) {
            assert_eq!(
                contour_bits(ha),
                contour_bits(hb),
                "[{label} #{idx}] hole bit-level mismatch"
            );
        }
        let ba = a.bounding_box();
        let bb = b.bounding_box();
        assert_eq!(
            (
                ba.x0.to_bits(),
                ba.y0.to_bits(),
                ba.x1.to_bits(),
                ba.y1.to_bits()
            ),
            (
                bb.x0.to_bits(),
                bb.y0.to_bits(),
                bb.x1.to_bits(),
                bb.y1.to_bits()
            ),
            "[{label} #{idx}] bbox bit-level mismatch"
        );
        if a.symbol_instance.is_none() {
            assert_eq!(a.shape_id, b.shape_id, "[{label} #{idx}] shape_id mismatch");
            assert_eq!(
                a.mesh_revision, b.mesh_revision,
                "[{label} #{idx}] mesh_revision mismatch"
            );
        }
    }
}

#[test]
fn test_stage7_scene_unchanged_after_rebuild_overlay_and_base_cache_on_every_action() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 120, 220);
    let green = Color32::from_rgb(60, 200, 90);

    // ブラシ描画・消しゴム・バケツ塗り
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(50.0, 50.0), Point::new(150.0, 50.0)],
            radius: 20.0,
            color: red,
        },
        "PaintStroke 1",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(50.0, 120.0), Point::new(150.0, 120.0)],
            radius: 20.0,
            color: blue,
        },
        "PaintStroke 2",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::EraseStroke {
            points: vec![Point::new(100.0, 40.0), Point::new(100.0, 60.0)],
            radius: 6.0,
        },
        "EraseStroke",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::BucketFill {
            pt: Point::new(60.0, 50.0),
            color: green,
        },
        "BucketFill",
    );

    // 矩形スライス選択 → 移動 → 色変更 → 自由変形 → 選択解除
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::SelectMarquee {
            p0: Point::new(40.0, 100.0),
            p1: Point::new(100.0, 140.0),
            additive: false,
        },
        "SelectMarquee slice",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(15.0, -10.0),
        },
        "NudgeSelection on sliced floating",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::FlipSelection {
            flip_x: true,
            flip_y: false,
        },
        "FlipSelection on floating",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::RotateSelection { angle_rad: 0.2 },
        "RotateSelection on floating",
    );
    run_and_verify_scene_cache_consistency(&mut app, Action::Deselect, "Deselect after slice edit");

    // グループ化・グループ解除・シンボル変換・コピー・ペースト・タイムライン操作・Undo/Redo
    let first_shape = app.scene().get(0).unwrap().clone();
    app.session.selection.clear();
    app.session.selection.insert_shape(&first_shape);
    run_and_verify_scene_cache_consistency(&mut app, Action::Group, "Group");
    run_and_verify_scene_cache_consistency(&mut app, Action::Ungroup, "Ungroup group");

    let sym_target = app.scene().get(0).unwrap().clone();
    app.session.selection.clear();
    app.session.selection.insert_shape(&sym_target);
    run_and_verify_scene_cache_consistency(&mut app, Action::ConvertToSymbol, "ConvertToSymbol");
    run_and_verify_scene_cache_consistency(&mut app, Action::Copy, "Copy symbol");
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::Paste { in_place: false },
        "Paste symbol",
    );
    run_and_verify_scene_cache_consistency(&mut app, Action::ExtendFrame, "F5");
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::InsertKeyframe { copy_prev: true },
        "F6",
    );
    run_and_verify_scene_cache_consistency(&mut app, Action::AddLayer, "AddLayer");
    run_and_verify_scene_cache_consistency(&mut app, Action::Undo, "Undo 1");
    run_and_verify_scene_cache_consistency(&mut app, Action::Undo, "Undo 2");
    run_and_verify_scene_cache_consistency(&mut app, Action::Undo, "Undo 3");
    run_and_verify_scene_cache_consistency(&mut app, Action::Redo, "Redo 1");
    run_and_verify_scene_cache_consistency(&mut app, Action::Redo, "Redo 2");

    // 回転と縦横で違う拡大率を持つシンボルの中で編集するケース（ビット単位で一致すること）
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 0,
        },
        "SwitchLayerAndFrame to symbol layer",
    );
    let sym_inst_shape = app
        .scene()
        .iter()
        .find(|s| s.symbol_instance.is_some())
        .unwrap()
        .clone();
    app.session.selection.clear();
    app.session.selection.insert_shape(&sym_inst_shape);
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::ScaleSelection { sx: 1.35, sy: 0.72 },
        "ScaleSelection non-uniform on symbol",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::RotateSelection { angle_rad: 0.43 },
        "RotateSelection on symbol",
    );
    let transformed_binding = app.scene().iter().find_map(|s| s.symbol_instance).unwrap();
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::EnterSymbolTimeline {
            binding: transformed_binding,
        },
        "EnterSymbolTimeline rotated+non-uniform-scaled",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(70.0, 60.0), Point::new(140.0, 85.0)],
            radius: 14.0,
            color: blue,
        },
        "PaintStroke inside rotated+scaled symbol",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::EraseStroke {
            points: vec![Point::new(90.0, 50.0), Point::new(90.0, 90.0)],
            radius: 5.0,
        },
        "EraseStroke inside rotated+scaled symbol",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::SelectMarquee {
            p0: Point::new(50.0, 40.0),
            p1: Point::new(110.0, 95.0),
            additive: false,
        },
        "SelectMarquee inside rotated+scaled symbol",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(12.0, -8.0),
        },
        "NudgeSelection inside rotated+scaled symbol",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::Deselect,
        "Deselect inside rotated+scaled symbol",
    );
    let inner_first = app.scene().get(0).unwrap().clone();
    app.session.selection.clear();
    app.session.selection.insert_shape(&inner_first);
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::Group,
        "Group inside rotated+scaled symbol",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::Ungroup,
        "Ungroup inside rotated+scaled symbol",
    );
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::ExitSymbolTimeline,
        "ExitSymbolTimeline from rotated+scaled symbol",
    );
}

#[test]
fn test_stage7_set_active_frame_and_per_layer_backdrop_cache() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let mut parent_shapes = Vec::with_capacity(2000);
    for i in 0..2000 {
        let col = i % 50;
        let row = i / 50;
        let x0 = (col as f64) * 30.0;
        let y0 = (row as f64) * 30.0;
        let c = Color32::from_rgb(
            ((i * 37) % 200 + 40) as u8,
            ((i * 73) % 200 + 40) as u8,
            150,
        );
        parent_shapes.extend(make_stroke(
            Point::new(x0, y0 + 10.0),
            Point::new(x0 + 20.0, y0 + 10.0),
            10.0,
            c,
        ));
    }
    put_cel_shapes(&mut app, 0, 0, parent_shapes);

    // 別レイヤーにシンボルを作成して進入する
    run(
        &mut app,
        &[
            Action::AddLayer,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
                radius: 14.0,
                color: Color32::RED,
            },
        ],
    );
    let red_shape = app.scene().iter().last().unwrap().clone();
    app.session.selection.clear();
    app.session.selection.insert_shape(&red_shape);
    run(&mut app, &[Action::ConvertToSymbol]);
    let sym_binding = app.scene().iter().find_map(|s| s.symbol_instance).unwrap();
    // 同じレイヤーにもう1つ同じシンボルのインスタンスを配置しておく（背景に兄弟インスタンスが映る状態）
    run(
        &mut app,
        &[Action::PlaceSymbol {
            sym_id: sym_binding.symbol_id,
        }],
    );
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app.tabs[app.active],
        sym_binding
    ));

    // シンボル内で3フレームに伸ばす
    run(
        &mut app,
        &[
            Action::ExtendFrame,
            Action::ExtendFrame,
            Action::InsertKeyframe { copy_prev: true },
        ],
    );

    // 1. シンボルの中で set_active_frame / set_active_layer / set_active_cell_selection を呼んでも doc.revision が変わらないこと
    let doc_rev_before = app.doc.revision;
    let sym_rev_before = app.doc.library[0].revision;
    app.tabs[app.active].set_active_frame(0);
    app.tabs[app.active].set_active_frame(1);
    app.tabs[app.active].set_active_frame(2);
    app.tabs[app.active].set_active_layer(0);
    app.tabs[app.active].set_active_cell_selection(None);
    assert_eq!(
        app.doc.revision, doc_rev_before,
        "シンボル内で set_active_frame 等を呼んでも doc.revision が変わらないこと"
    );
    assert_eq!(
        app.doc.library[0].revision, sym_rev_before,
        "シンボル内で set_active_frame 等を呼んでもシンボルの revision が変わらないこと"
    );

    // 2. 親シーン背景キャッシュを初回構築したあと、シンボル内で再生フレーム送りしても背景メッシュが再構築されずそのまま返ること
    let bd_initial = app.tabs[app.active]
        .cached_backdrop_screen_mesh(1.0, Vec2::ZERO)
        .expect("親シーン背景が存在すること");
    let t_play = std::time::Instant::now();
    app.tabs[app.active].set_active_frame(1);
    app.tabs[app.active].refresh_scene_base();
    let bd_after_step = app.tabs[app.active]
        .cached_backdrop_screen_mesh(1.0, Vec2::ZERO)
        .unwrap();
    for s in app.scene().iter() {
        let _ = s.cached_screen_mesh(1.0, Vec2::ZERO, false);
    }
    let play_step_elapsed = t_play.elapsed();
    assert!(
        std::sync::Arc::ptr_eq(&bd_initial, &bd_after_step),
        "シンボル内での再生フレーム送りでは親シーン背景メッシュが再計算されないこと"
    );

    // 3. シンボルの中で（親シーンが参照している先頭フレーム 0 に戻って）1本描いても、
    //    編集中のシンボルのインスタンスを含まない親レイヤー（2000図形）は作り直されず、
    //    兄弟インスタンスを含むレイヤーのみ更新されて背景メッシュに反映されること
    app.tabs[app.active].set_active_frame(0);
    app.tabs[app.active].refresh_scene_base();
    let initial_vert_count = bd_initial.vertices.len();
    let t_draw = std::time::Instant::now();
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 140.0), Point::new(180.0, 140.0)],
            radius: 12.0,
            color: Color32::YELLOW,
        }],
    );
    let bd_after_draw = app.tabs[app.active]
        .cached_backdrop_screen_mesh(1.0, Vec2::ZERO)
        .unwrap();
    for s in app.scene().iter() {
        let _ = s.cached_screen_mesh(1.0, Vec2::ZERO, false);
    }
    let draw_and_render_elapsed = t_draw.elapsed();
    assert!(
        bd_after_draw.vertices.len() > initial_vert_count,
        "兄弟インスタンスを含む親レイヤーの背景のみ更新され頂点数が増えること"
    );
    // 2回目以降の同フレーム描画では combined_screen_cache もそのまま再利用されること
    let bd_after_draw_2 = app.tabs[app.active]
        .cached_backdrop_screen_mesh(1.0, Vec2::ZERO)
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&bd_after_draw, &bd_after_draw_2));
    println!(
        "[symbol backdrop] play_step = {:.4} ms, draw_and_render = {:.4} ms",
        play_step_elapsed.as_secs_f64() * 1000.0,
        draw_and_render_elapsed.as_secs_f64() * 1000.0,
    );
}

#[test]
fn test_collapsed_symbol_instance_blocks_enter_and_timeline_mut_bumps_revision() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 16.0,
            color: red,
        }],
    );
    let first = app.scene().get(0).unwrap().clone();
    app.session.selection.clear();
    app.session.selection.insert_shape(&first);
    run(&mut app, &[Action::ConvertToSymbol]);

    let normal_binding = app.scene().get(0).unwrap().symbol_instance.unwrap();

    // 1. 行列式がほぼ0（潰れたインスタンス）には入れないこと
    let mut collapsed_binding = normal_binding;
    collapsed_binding.xform = kurbo::Affine::scale_non_uniform(1.0, 0.0);
    assert!(
        !symbol_ops::enter_symbol_timeline(&mut app.tabs[app.active], collapsed_binding),
        "行列式が0の潰れたインスタンスには入れないこと"
    );
    assert!(app.session.edit_path.is_empty());

    // 2. 通常のインスタンスに入り、with_active_timeline_mut を呼ぶと必ずリビジョンが上がること
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app.tabs[app.active],
        normal_binding
    ));
    assert_eq!(app.session.edit_path.levels.len(), 1);
    let sym_rev_before = app.doc.library[0].revision;
    let doc_rev_before = app.doc.revision;
    app.tabs[app.active].with_active_timeline_mut(|_tl, _| {});
    assert_ne!(
        app.doc.library[0].revision, sym_rev_before,
        "with_active_timeline_mut を呼んだら手書きハッシュ判定なしで必ずシンボルのリビジョンが上がること"
    );
    assert_ne!(
        app.doc.revision, doc_rev_before,
        "with_active_timeline_mut を呼んだら文書の revision も上がること"
    );
}

#[test]
fn test_g5_1_save_and_reopen_60_brush_strokes_f6_480_frames_over_100mb() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();
    let mut rng = fastrand::Rng::with_seed(42);
    for i in 0..60 {
        let color = Color32::from_rgb(
            ((i * 37) % 200 + 30) as u8,
            ((i * 73) % 200 + 30) as u8,
            ((i * 113) % 200 + 30) as u8,
        );
        let mut pts = Vec::with_capacity(36);
        let mut x = 50.0 + (i % 10) as f64 * 70.0;
        let mut y = 50.0 + (i / 10) as f64 * 60.0;
        for step in 0..36 {
            x += 12.0 + rng.f64() * 8.0;
            y += ((step as f64 * 0.7).sin() * 18.0) + (rng.f64() - 0.5) * 6.0;
            pts.push(Point::new(x, y));
        }
        doc_actions::apply(
            &mut app,
            Action::PaintStroke {
                points: pts,
                radius: 6.0,
                color,
            },
        );
    }

    let first_cel_shapes = app.doc.root.layers[0].keyframes[0].cel.shapes.len();
    let first_cel_segs: usize = app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes
        .iter()
        .map(|s| {
            s.exterior.segments.len() + s.holes.iter().map(|h| h.segments.len()).sum::<usize>()
        })
        .sum();

    for _ in 1..480 {
        assert!(doc_actions::apply(
            &mut app,
            Action::InsertKeyframe { copy_prev: true }
        ));
    }
    assert_eq!(app.doc.root.total_frames, 480);
    assert_eq!(app.doc.root.layers[0].keyframes.len(), 480);

    let tmp_path = std::env::temp_dir().join(format!(
        "is2d_g5_1_480kf_over_100mb_{}.is2d",
        std::process::id()
    ));

    let t_save = std::time::Instant::now();
    crate::project_io::save_project_to_path(&mut app, &tmp_path)
        .expect("100MB を超える文書でも保存に成功すること (G5-1)");
    let save_elapsed = t_save.elapsed();

    let file_bytes = std::fs::metadata(&tmp_path).unwrap().len();
    println!(
        "[G5-1] 60 strokes ({first_cel_shapes} shapes, {first_cel_segs} segs/cel) x 480 KF: size = {:.2} MB ({file_bytes} bytes), save = {save_elapsed:?}",
        file_bytes as f64 / (1024.0 * 1024.0)
    );
    assert!(
        file_bytes > 100 * 1024 * 1024,
        "テスト文書が旧上限 100MB を超えていること: {file_bytes} bytes"
    );

    let mut reopened = Is2dApp::default();
    let t_open = std::time::Instant::now();
    crate::project_io::open_project_in_tab(&mut reopened, &tmp_path, None)
        .expect("100MB を超える保存済みファイルを開き直せること (G5-1)");
    let open_elapsed = t_open.elapsed();
    let _ = std::fs::remove_file(&tmp_path);

    println!("[G5-1] reopen = {open_elapsed:?}");
    assert_eq!(reopened.doc.root.total_frames, 480);
    assert_eq!(reopened.doc.root.layers[0].keyframes.len(), 480);
    assert_eq!(
        reopened.doc.root.layers[0].keyframes[479].cel.shapes.len(),
        first_cel_shapes
    );
}

#[test]
fn test_g5_2_symbol_nest_depth_limit_and_file_rejection() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();

    // 1段ずつシンボル化（F8）を8回繰り返し、各段に1つずつ生図形を持つ8段の連鎖を作る
    let mut chain_sym_ids = Vec::new();
    for lvl in 0..8 {
        let x = 40.0 + (lvl as f64) * 30.0;
        let _ = apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(x, 100.0), Point::new(x + 15.0, 100.0)],
                radius: 6.0,
                color: Color32::from_rgb(20 + (lvl as u8) * 25, 80, 160),
            },
        );
        let _ = apply(&mut app, Action::SelectAll);
        assert!(
            apply(&mut app, Action::ConvertToSymbol),
            "8段目までのシンボル化は成功すること"
        );
        let id = app.doc.library.last().unwrap().id;
        chain_sym_ids.push(id);
    }
    assert_eq!(app.doc.library.len(), 8);
    assert!(!app.doc.symbol_nest_exceeded());

    // 完了条件1: 8段の連鎖が、ルートからすべて描かれる（各段の生図形8個すべてが展開される）
    let root_eval = app
        .doc
        .root
        .evaluate_layer_shapes_at(0, 0, &app.doc.library);
    assert_eq!(
        root_eval.len(),
        8,
        "8段の連鎖のすべての段の図形がルートから描かれること"
    );
    assert_eq!(app.scene().len(), 8);

    // 完了条件2: 高さ8のシンボルを含む選択で F8 -> 文書は変わらず、警告が出て、Undo履歴も増えない
    let undo_before_f8 = app.history.undo_stack.len();
    let lib_len_before = app.doc.library.len();
    let root_inst_before = app.doc.root.layers[0].keyframes[0].cel.instances.clone();
    app.toast = None;
    let _ = apply(&mut app, Action::SelectAll);
    let res_f8 = apply(&mut app, Action::ConvertToSymbol);
    assert!(!res_f8, "9段目になるF8は拒否されること");
    assert_eq!(app.doc.library.len(), lib_len_before);
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.instances,
        root_inst_before
    );
    assert_eq!(app.history.undo_stack.len(), undo_before_f8);
    assert_eq!(
        app.toast.as_ref().map(|t| t.message.as_str()),
        Some("シンボルの入れ子は8段までです")
    );
    assert!(!app.doc.symbol_nest_exceeded());

    // 完了条件3: シンボル編集中のペースト、ライブラリからの配置で9段になる場合も同じ
    // ルートに高さ8のシンボルインスタンスをコピーしておく
    let top_sym_id = *chain_sym_ids.last().unwrap();
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::Copy);

    // 別の単段シンボル `leaf_sym_id` を作り、その内部編集に入って高さ8のシンボルを Paste / PlaceSymbol する
    let _ = apply(&mut app, Action::Deselect);
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(350.0, 200.0), Point::new(380.0, 200.0)],
            radius: 6.0,
            color: Color32::GREEN,
        },
    );
    let pt_hit = Point::new(365.0, 200.0);
    let _ = apply(&mut app, Action::SelectSingleAt { pt: pt_hit });
    assert!(
        apply(&mut app, Action::ConvertToSymbol),
        "単段シンボルの作成は成功すること"
    );
    let host_sym_id = app.doc.library.last().unwrap().id;
    assert_eq!(app.doc.library.len(), 9);
    let _ = apply(
        &mut app,
        Action::EnterSymbolFromLibrary {
            sym_id: host_sym_id,
        },
    );
    assert_eq!(app.session.edit_path.len(), 1);

    // 3a. シンボル編集中のペーストで9段になる場合
    let undo_before_paste = app.history.undo_stack.len();
    let host_inst_before = app.doc.find_symbol(host_sym_id).unwrap().timeline.layers[0].keyframes
        [0]
    .cel
    .instances
    .len();
    app.toast = None;
    let res_paste = apply(&mut app, Action::Paste { in_place: true });
    assert!(!res_paste, "9段になるペーストは拒否されること");
    assert_eq!(app.history.undo_stack.len(), undo_before_paste);
    assert_eq!(
        app.doc.find_symbol(host_sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .instances
            .len(),
        host_inst_before
    );
    assert_eq!(
        app.toast.as_ref().map(|t| t.message.as_str()),
        Some("シンボルの入れ子は8段までです")
    );

    // 3b. ライブラリからの配置で9段になる場合
    let undo_before_place = app.history.undo_stack.len();
    app.toast = None;
    let res_place = apply(&mut app, Action::PlaceSymbol { sym_id: top_sym_id });
    assert!(!res_place, "9段になるライブラリからの配置は拒否されること");
    assert_eq!(app.history.undo_stack.len(), undo_before_place);
    assert_eq!(
        app.doc.find_symbol(host_sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .instances
            .len(),
        host_inst_before
    );
    assert_eq!(
        app.toast.as_ref().map(|t| t.message.as_str()),
        Some("シンボルの入れ子は8段までです")
    );
    let _ = apply(&mut app, Action::ExitSymbolTimeline);

    // 完了条件4 & 5: 9段のファイル・1000段の連鎖のファイルは開けずにエラーになり、タブは増えない
    let make_chain_project = |depth: usize| -> project_io::ProjectDocument {
        let mut library = Vec::with_capacity(depth);
        for i in 0..depth {
            let sym_id = (i + 1) as u64;
            let instances = if i + 1 < depth {
                vec![project_io::SavedSymbolInstance {
                    binding: project_io::SavedSymbolInstanceBinding {
                        instance_gid: (i + 1) as u64,
                        symbol_id: (i + 2) as u64,
                        xform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                        pivot: [0.0, 0.0],
                        loop_mode: vector_engine::SymbolLoopMode::Loop,
                        first_frame: 0,
                        color_effect: vector_engine::ColorEffect::default(),
                    },
                    layer_id: 1,
                    parent_scope: Vec::new(),
                    shape_order_index: 0,
                }]
            } else {
                Vec::new()
            };
            library.push(project_io::SavedSymbolDef {
                id: sym_id,
                name: format!("Chain{sym_id}"),
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
                            tween_rotate: timeline::TweenRotate::Auto,
                            shapes: Vec::new(),
                            instances,
                        }],
                    }],
                },
            });
        }
        project_io::ProjectDocument {
            version: project_io::PROJECT_FORMAT_VERSION,
            stage_size: [800.0, 450.0],
            timeline: (&timeline::Timeline::default()).into(),
            library,
            next_symbol_id: (depth + 1) as u64,
            next_group_id: (depth + 1) as u64,
            recovery_source: None,
        }
    };

    let tabs_before = app.tabs.len();
    for depth in [9usize, 1000usize] {
        let path = std::env::temp_dir().join(format!(
            "is2d_g5_2_chain_{depth}_{}.is2d",
            std::process::id()
        ));
        let json = serde_json::to_string(&make_chain_project(depth)).unwrap();
        std::fs::write(&path, json.as_bytes()).unwrap();
        let res = crate::project_io::open_project_in_tab(&mut app, &path, None);
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            res,
            Err("シンボルの入れ子が8段を超えているか、循環しています".to_owned()),
            "{depth}段のファイルはエラーで拒否されること"
        );
        assert_eq!(
            app.tabs.len(),
            tabs_before,
            "{depth}段のファイル読み込み失敗時にタブが増えないこと"
        );
    }
}

#[test]
fn test_g1_2_truncation_flag_and_warning_above_10000_shapes() {
    let mut app = Is2dApp::default();

    // 100個の図形を持つシンボルを作成
    let mut local_shapes = Vec::with_capacity(100);
    for i in 0..100 {
        let x = (i % 10) as f64 * 12.0;
        let y = (i / 10) as f64 * 12.0;
        let mut s = make_stroke(
            Point::new(x, y),
            Point::new(x + 8.0, y),
            2.0,
            Color32::from_rgb(40 + i as u8, 100, 200),
        )
        .remove(0);
        s.layer_id = 1;
        local_shapes.push(s);
    }
    let sym_id = app.doc.register_new_symbol(local_shapes);

    // 120インスタンス（展開後 12,000 図形）を配置 -> 打ち切られず 12,000 個すべて土台キャッシュに載ること
    let instances_120: Vec<timeline::SymbolInstance> = (0..120)
        .map(|k| timeline::SymbolInstance {
            binding: symbol::SymbolInstanceBinding::new(
                (k + 1) as u64,
                sym_id,
                kurbo::Affine::IDENTITY,
            ),
            layer_id: 1,
            parent_scope: Vec::new(),
            shape_order_index: 0,
        })
        .collect();
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_parts(Vec::new(), instances_120);
    app.refresh_scene_base();
    assert_eq!(
        app.session.base_cache.shapes().len(),
        12_000,
        "100図形のシンボルを120個置いたレイヤーで、土台キャッシュに12,000個の図形がすべて載ること"
    );
    assert_eq!(app.scene().len(), 12_000);
}

#[test]
fn test_g1_1_writeback_preserves_unexpanded_and_orphan_elements() {
    use crate::doc_actions::Action;

    let mut app = Is2dApp::default();

    // 100図形のシンボルを作成
    let mut local_shapes = Vec::with_capacity(100);
    for i in 0..100 {
        let x = (i % 10) as f64 * 10.0;
        let y = (i / 10) as f64 * 10.0;
        let mut s = make_stroke(
            Point::new(x, y),
            Point::new(x + 6.0, y),
            2.0,
            Color32::from_rgb(30 + i as u8, 90, 180),
        )
        .remove(0);
        s.layer_id = 1;
        local_shapes.push(s);
    }
    let sym_id = app.doc.register_new_symbol(local_shapes);

    // 120個のシンボルインスタンス（展開後 12,000 図形）をセルに設定
    let instances: Vec<timeline::SymbolInstance> = (0..120)
        .map(|k| timeline::SymbolInstance {
            binding: symbol::SymbolInstanceBinding::new(
                (k + 1) as u64,
                sym_id,
                kurbo::Affine::IDENTITY,
            ),
            layer_id: 1,
            parent_scope: Vec::new(),
            shape_order_index: 0,
        })
        .collect();
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_parts(Vec::new(), instances);
    app.refresh_scene_base();
    assert_eq!(app.session.base_cache.shapes().len(), 12_000);
    assert_eq!(app.scene().len(), 12_000);

    // ブラシで1回描く
    run_and_verify_scene_cache_consistency(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(500.0, 300.0), Point::new(540.0, 300.0)],
            radius: 5.0,
            color: Color32::RED,
        },
        "M2-3 PaintStroke on 120-instance cel",
    );

    // 文書のインスタンスは120個のまま残ること
    let cel = &app.doc.root.layers[0].keyframes[0].cel;
    assert_eq!(
        cel.instances.len(),
        120,
        "ブラシで1回描いても文書のインスタンスは120個のまま残ること"
    );
    assert_eq!(cel.shapes.len(), 1, "描いたブラシ図形が保存されていること");

    // 保存して読み込んでも120個のインスタンスが残ること
    let tmp_path = std::env::temp_dir().join(format!("is2d_m2_3_test_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut reopened = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut reopened, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    assert_eq!(
        reopened.doc.root.layers[0].keyframes[0].cel.instances.len(),
        120,
        "保存して読み込んでもインスタンスが120個残ること"
    );
    reopened.refresh_scene_base();
    assert_eq!(reopened.session.base_cache.shapes().len(), 12_001);
}

#[cfg(target_os = "macos")]
fn current_process_rss_bytes() -> usize {
    #[repr(C)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [u32; 2],
        system_time: [u32; 2],
        policy: i32,
        suspend_count: i32,
    }
    const MACH_TASK_BASIC_INFO: u32 = 20;
    const MACH_TASK_BASIC_INFO_COUNT: u32 =
        (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;
    extern "C" {
        static mach_task_self_: u32;
        fn task_info(
            target_task: u32,
            flavor: u32,
            task_info_out: *mut MachTaskBasicInfo,
            task_info_out_cnt: *mut u32,
        ) -> i32;
    }
    let mut info = std::mem::MaybeUninit::<MachTaskBasicInfo>::zeroed();
    let mut count = MACH_TASK_BASIC_INFO_COUNT;
    let kr = unsafe {
        task_info(
            mach_task_self_,
            MACH_TASK_BASIC_INFO,
            info.as_mut_ptr(),
            &mut count,
        )
    };
    assert_eq!(kr, 0, "task_info failed: {kr}");
    unsafe { info.assume_init().resident_size as usize }
}

#[cfg(target_os = "linux")]
fn current_process_rss_bytes() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|kb| kb.parse::<usize>().ok())
                .map(|kb| kb * 1024)
        })
        .unwrap_or(0)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn current_process_rss_bytes() -> usize {
    0
}

#[test]
fn test_g4_1_shared_keyframes_fast_undo_and_rss_bound() {
    use crate::doc_actions::{apply, Action};
    use std::sync::Arc;
    use std::time::Instant;

    // 1. 10レイヤー × 3,000キーフレームのタイムラインで、1ストロークの描画と1回の Undo がそれぞれ 5ms 以内
    {
        let mut app = Is2dApp::default();
        let sample_shape = make_stroke(
            Point::new(50.0, 50.0),
            Point::new(90.0, 50.0),
            4.0,
            Color32::from_rgb(80, 120, 200),
        );
        app.doc.root.total_frames = 3_000;
        app.doc.root.layers.clear();
        for l in 0..10 {
            let lid = (l + 1) as u64;
            let mut kfs = Vec::with_capacity(3_000);
            for f in 0..3_000 {
                let mut shapes = sample_shape.clone();
                shapes[0].layer_id = lid;
                kfs.push(Arc::new(timeline::Keyframe {
                    start_frame: f,
                    cel: timeline::Cel::new(shapes),
                    tween: false,
                    tween_rotate: timeline::TweenRotate::Auto,
                }));
            }
            app.doc.root.layers.push(timeline::Layer {
                id: lid,
                name: format!("Layer {}", l + 1),
                visible: true,
                locked: false,
                frame_count: 3_000,
                keyframes: kfs,
            });
        }
        app.doc.root.next_layer_id = 11;
        app.doc.root.next_layer_number = 11;
        app.doc.root.current_frame = 1_500;
        app.doc.root.active_layer = 0;
        app.refresh_scene_base();

        let t_stroke = Instant::now();
        let _ = apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(200.0, 200.0), Point::new(240.0, 200.0)],
                radius: 4.0,
                color: Color32::RED,
            },
        );
        let stroke_elapsed = t_stroke.elapsed();

        let t_undo = Instant::now();
        app.undo();
        let undo_elapsed = t_undo.elapsed();

        println!(
            "[G4-1] 10L x 3000KF: 1 stroke = {:.3} ms, 1 undo = {:.3} ms",
            stroke_elapsed.as_secs_f64() * 1000.0,
            undo_elapsed.as_secs_f64() * 1000.0,
        );
    }

    // 2. 20レイヤー × 600キーフレームのタイムラインで、ブラシ描画を100回繰り返した前後の RSS 増加が 50MB 以内
    #[cfg(target_os = "macos")]
    {
        let mut app = Is2dApp::default();
        let sample_shape = make_stroke(
            Point::new(50.0, 50.0),
            Point::new(90.0, 50.0),
            4.0,
            Color32::from_rgb(100, 160, 90),
        );
        app.doc.root.total_frames = 600;
        app.doc.root.layers.clear();
        for l in 0..20 {
            let lid = (l + 1) as u64;
            let mut kfs = Vec::with_capacity(600);
            for f in 0..600 {
                let mut shapes = sample_shape.clone();
                shapes[0].layer_id = lid;
                kfs.push(Arc::new(timeline::Keyframe {
                    start_frame: f,
                    cel: timeline::Cel::new(shapes),
                    tween: false,
                    tween_rotate: timeline::TweenRotate::Auto,
                }));
            }
            app.doc.root.layers.push(timeline::Layer {
                id: lid,
                name: format!("Layer {}", l + 1),
                visible: true,
                locked: false,
                frame_count: 600,
                keyframes: kfs,
            });
        }
        app.doc.root.next_layer_id = 21;
        app.doc.root.next_layer_number = 21;
        app.doc.root.current_frame = 0;
        app.doc.root.active_layer = 0;
        app.refresh_scene_base();

        let rss_before = current_process_rss_bytes();
        for i in 0..100 {
            let y = 120.0 + (i % 10) as f64 * 12.0;
            let x = 100.0 + (i / 10) as f64 * 30.0;
            let _ = apply(
                &mut app,
                Action::PaintStroke {
                    points: vec![Point::new(x, y), Point::new(x + 20.0, y)],
                    radius: 3.0,
                    color: Color32::from_rgb(200, (i * 2) as u8, 80),
                },
            );
        }
        let rss_after = current_process_rss_bytes();
        let rss_delta_bytes = rss_after.saturating_sub(rss_before);
        let rss_delta_mb = rss_delta_bytes as f64 / (1024.0 * 1024.0);
        println!(
            "[G4-1] 20L x 600KF x 100 strokes: RSS before = {:.2} MB, after = {:.2} MB, delta = {:.2} MB",
            rss_before as f64 / (1024.0 * 1024.0),
            rss_after as f64 / (1024.0 * 1024.0),
            rss_delta_mb,
        );
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
                    binding: symbol::SymbolInstanceBinding::new(
                        gid,
                        prev_sym_id,
                        kurbo::Affine::translate((i as f64 * 0.5, 0.0)),
                    ),
                    layer_id: 1,
                    parent_scope: Vec::new(),
                    shape_order_index: 0,
                }
            })
            .collect();
        let sym = app.doc.find_symbol_mut(sym_id).unwrap();
        sym.timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .set_parts(Vec::new(), instances);
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
                binding: symbol::SymbolInstanceBinding::new(
                    gid,
                    prev_sym_id,
                    kurbo::Affine::translate((i as f64 * 0.5, 0.0)),
                ),
                layer_id: 1,
                parent_scope: Vec::new(),
                shape_order_index: 1,
            }
        })
        .collect();
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_parts(bg_shape, root_instances);
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

#[test]
fn test_g1_4_playback_advance_skips_base_cache_rebuild_and_stop_refreshes() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    app.doc.fps = 10; // 1コマ = 0.1秒
    app.doc.root.layers[0].insert_frame_at(2);
    app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(150.0, 100.0),
            10.0,
            Color32::RED,
        ),
    );
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 1);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    put_cel_shapes(
        &mut app,
        0,
        1,
        make_stroke(
            Point::new(150.0, 100.0),
            Point::new(200.0, 100.0),
            10.0,
            Color32::GREEN,
        ),
    );
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, false);
    put_cel_shapes(
        &mut app,
        0,
        2,
        make_stroke(
            Point::new(200.0, 100.0),
            Point::new(250.0, 100.0),
            10.0,
            Color32::BLUE,
        ),
    );

    // Frame 0 から再生開始（非ループ）
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.session.base_cache.shapes()[0].color, Color32::RED);
    app.timeline_ui.loop_playback = false;
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);

    // t = 1.0s（基準時刻）
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
    // t = 1.12s（Frame 1 へ進行、まだ再生中）
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
    assert!(app.timeline_ui.is_playing);
    assert_eq!(app.doc.root.current_frame, 1);
    assert_eq!(
        app.session.base_cache.shapes()[0].color,
        Color32::RED,
        "再生中のフレーム進行では SceneBaseCache の再構築が走らないこと"
    );

    // 手動停止すると Frame 1 の土台キャッシュが再構築されること
    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing);
    assert_eq!(
        app.session.base_cache.shapes()[0].color,
        Color32::GREEN,
        "再生停止時点で現在フレーム (Frame 1) の土台キャッシュが揃うこと"
    );

    // 再び再生を開始し、最終フレーム (Frame 2) で自動停止した際にも stop_playback 経由で土台キャッシュが揃うこと
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            time: Some(2.0),
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            time: Some(2.15),
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(!app.timeline_ui.is_playing);
    assert_eq!(app.doc.root.current_frame, 2);
    assert_eq!(
        app.session.base_cache.shapes()[0].color,
        Color32::BLUE,
        "最終フレームでの自動停止でも stop_playback により Frame 2 の土台キャッシュが揃うこと"
    );
}

#[test]
fn test_g9_1_transform_click_on_rotate_or_scale_handle_preserves_selection() {
    use crate::canvas_tool::{Tool, TransformHoverTarget};
    use crate::doc_actions::{apply, Action};
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(300.0, 300.0), Point::new(400.0, 300.0)],
            radius: 15.0,
            color: Color32::RED,
        },
    );
    app.tool.tool = Tool::Transform;
    let _ = apply(
        &mut app,
        Action::SelectSingleAt {
            pt: Point::new(350.0, 300.0),
        },
    );
    assert!(!app.session.selection.is_empty());

    // 初期レイアウトの確定
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );

    let bbox = app
        .scene()
        .selection_bounding_box(&app.session.selection, app.view.zoom)
        .unwrap();

    // 1. 四隅の回転ゾーン（図形外・境界枠の左上コーナー外側）の座標を確認し、クリックしても選択が外れないこと
    let rotate_pt = Point::new(bbox.x0 - 12.0, bbox.y0 - 12.0);
    assert!(
        !app.scene().get(0).unwrap().contains_point(rotate_pt),
        "回転ゾーン座標は図形の外側であること"
    );
    assert_eq!(
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            rotate_pt,
            app.view.zoom
        ),
        TransformHoverTarget::Rotate
    );
    let rotate_scr = app.view.world_to_screen(rotate_pt);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(rotate_scr),
                egui::Event::PointerButton {
                    pos: rotate_scr,
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
            events: vec![egui::Event::PointerButton {
                pos: rotate_scr,
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
    assert!(
        !app.session.selection.is_empty(),
        "自由変形ツールで回転ゾーンをクリックしたとき選択が外れないこと"
    );

    // 2. 拡縮ハンドルの上（図形外・境界枠の左上角の少し外側）をクリックしても選択が外れないこと
    let scale_pt = Point::new(bbox.x0 - 3.0, bbox.y0 - 3.0);
    assert!(
        !app.scene().get(0).unwrap().contains_point(scale_pt),
        "拡縮ハンドル上の座標は図形の外側であること"
    );
    assert!(matches!(
        tool_transform::compute_transform_hover(app.scene(), &app.session, scale_pt, app.view.zoom),
        TransformHoverTarget::Scale(_)
    ));
    let scale_scr = app.view.world_to_screen(scale_pt);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(scale_scr),
                egui::Event::PointerButton {
                    pos: scale_scr,
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
            events: vec![egui::Event::PointerButton {
                pos: scale_scr,
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
    assert!(
        !app.session.selection.is_empty(),
        "自由変形ツールで拡縮ハンドルの上をクリックしたとき選択が外れないこと"
    );

    // 3. 何もない場所をクリックしたときは選択が外れること
    let empty_pt = Point::new(bbox.x0 - 80.0, bbox.y0 - 80.0);
    assert_eq!(
        tool_transform::compute_transform_hover(app.scene(), &app.session, empty_pt, app.view.zoom),
        TransformHoverTarget::None
    );
    let empty_scr = app.view.world_to_screen(empty_pt);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![
                egui::Event::PointerMoved(empty_scr),
                egui::Event::PointerButton {
                    pos: empty_scr,
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
            events: vec![egui::Event::PointerButton {
                pos: empty_scr,
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
    assert!(
        app.session.selection.is_empty(),
        "何もない場所をクリックしたときは選択が外れること"
    );
}

#[test]
fn test_other_1_insert_keyframe_via_snapshot_keyframe_at() {
    use crate::doc_actions::{apply, Action};
    use crate::symbol::{SymbolLoopMode, TweenRotate};

    let mut app = Is2dApp::default();

    // 3フレームのループシンボルを作成
    let mut s0 = make_stroke(
        Point::new(0.0, 0.0),
        Point::new(20.0, 0.0),
        4.0,
        Color32::RED,
    );
    s0[0].layer_id = 1;
    let sym_id = app.doc.register_new_symbol(s0);
    {
        let sym = app.doc.find_symbol_mut(sym_id).unwrap();
        sym.timeline.layers[0].insert_frame_at(2);
        sym.timeline.sync_total_frames();
    }

    // ルートにシンボルを配置し、Frame 0..4 のクラシックトゥイーンを作成
    app.doc.root.layers[0].insert_frame_at(7);
    app.doc.root.sync_total_frames();
    let inst0 = timeline::SymbolInstance {
        binding: symbol::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((100.0, 100.0)),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
            color_effect: vector_engine::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        shape_order_index: 0,
    };
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_parts(Vec::new(), vec![inst0]);
    let _ = apply(&mut app, Action::ToggleTween);
    let _ = apply(
        &mut app,
        Action::SetTweenRotate {
            rotate: TweenRotate::Clockwise,
        },
    );

    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 4);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    let inst4 = timeline::SymbolInstance {
        binding: symbol::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((300.0, 100.0)),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 1,
            color_effect: vector_engine::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        shape_order_index: 0,
    };
    app.doc.root.layers[0]
        .keyframe_mut(1)
        .cel
        .set_parts(Vec::new(), vec![inst4]);

    // 1. トゥイーン途中 (Frame 2) で F6 キーフレーム挿入：中割り座標 (200, 100)・first_frame=2・tween=true・Clockwise が引き継がれること
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 2);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    let kf_mid = &app.doc.root.layers[0].keyframes[1];
    assert_eq!(kf_mid.start_frame, 2);
    assert!(kf_mid.tween);
    assert_eq!(kf_mid.tween_rotate, TweenRotate::Clockwise);
    let b_mid = kf_mid.cel.instances[0].binding;
    assert!((b_mid.xform.as_coeffs()[4] - 200.0).abs() < 1e-6);
    assert_eq!(b_mid.first_frame, 2);

    // 2. 保持フレーム上 (Frame 6、Frame 4 キーフレームからの経過2コマ) で F6 キーフレーム挿入：first_frame が +2 進んだ値になること
    timeline_ui::switch_to_layer_and_frame(&mut app, 0, 6);
    timeline_ui::timeline_insert_keyframe(&mut app, true);
    let kf_hold = &app.doc.root.layers[0].keyframes[3];
    assert_eq!(kf_hold.start_frame, 6);
    assert_eq!(kf_hold.cel.instances[0].binding.first_frame, 3);

    // 3. フレーム範囲の複製がこれまでどおり動くこと
    app.doc.root.cell_selection = Some(timeline::TimelineCellSelection::from_corners(0, 1, 0, 3));
    let _ = apply(
        &mut app,
        Action::MoveOrCopyFrames {
            sel: timeline::TimelineCellSelection::from_corners(0, 1, 0, 3),
            layer_delta: 0,
            frame_delta: 4,
            copy_mode: true,
        },
    );
    assert!(app.doc.root.layers[0]
        .keyframes
        .iter()
        .any(|kf| kf.start_frame == 5));
}

#[test]
fn test_g3_5_document_single_fps_inside_symbol_undo_redo_and_save() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    assert_eq!(app.doc.fps, 24);

    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 5.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    let sym_id = app.doc.library[0].id;

    // シンボルの中に入る
    assert!(symbol_ops::enter_symbol_from_library(&mut app, sym_id));
    assert!(!app.session.edit_path.is_empty());

    // シンボル内で FPS を 30 に変更
    let _ = apply(&mut app, Action::SetFps { fps: 30 });
    assert_eq!(app.doc.fps, 30);

    // Undo / Redo で FPS が正しく巻き戻り・やり直されること
    app.undo();
    assert_eq!(app.doc.fps, 24);
    app.redo();
    assert_eq!(app.doc.fps, 30);

    // シンボルから出ても FPS が 30 のまま保たれること
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
    assert_eq!(app.doc.fps, 30);

    // 保存して開き直しても FPS が 30 のまま保たれること
    let tmp_path = std::env::temp_dir().join(format!("is2d_g3_5_test_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut reopened = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut reopened, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    assert_eq!(reopened.doc.fps, 30);
}

#[test]
fn test_g3_1_symbol_enter_exit_preserves_symbol_def_and_load_normalizes() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 5.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    let sym_id = app.doc.library[0].id;
    let rev_before = app.doc.library[0].revision;
    let doc_rev_before = app.doc.revision;
    let total_before = app.doc.library[0].timeline.total_frames;
    let next_num_before = app.doc.library[0].timeline.next_layer_number;
    let next_id_before = app.doc.library[0].timeline.next_layer_id;
    assert_eq!(total_before, 1);
    assert_eq!(next_num_before, 2);
    assert_eq!(next_id_before, 2);

    // シンボルへの出入りで SymbolDef の内容やリビジョンが変わらないこと
    assert!(symbol_ops::enter_symbol_from_library(&mut app, sym_id));
    assert_eq!(app.doc.library[0].revision, rev_before);
    assert_eq!(app.doc.revision, doc_rev_before);
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
    assert_eq!(app.doc.library[0].revision, rev_before);
    assert_eq!(app.doc.revision, doc_rev_before);
    assert_eq!(app.doc.library[0].timeline.total_frames, total_before);
    assert_eq!(
        app.doc.library[0].timeline.next_layer_number,
        next_num_before
    );
    assert_eq!(app.doc.library[0].timeline.next_layer_id, next_id_before);

    // 不正な total_frames=0, next_layer_number=0, next_layer_id=0 を持つファイルを読み込んだ時点で正規化されていること
    let mut pd = project_io::ProjectDocument::from_app(&app);
    pd.library[0].timeline.total_frames = 0;
    pd.library[0].timeline.next_layer_number = 0;
    pd.library[0].timeline.next_layer_id = 0;
    pd.library[0].timeline.layers[0].frame_count = 0;
    let loaded_doc = pd.into_document();
    assert!(loaded_doc.library[0].timeline.total_frames >= 1);
    assert!(loaded_doc.library[0].timeline.next_layer_number >= 2);
    assert!(loaded_doc.library[0].timeline.next_layer_id >= 2);
}

#[test]
fn test_g3_2_end_coalesce_via_apply_preserves_playback_and_coalesce_boundaries() {
    use crate::canvas_tool::CoalesceKey;
    use crate::doc_actions::{apply, Action};

    // 1. UI 層（panels.rs, timeline_ui.rs, main.rs）に last_coalesce_key と end_coalesce が出てこないこと
    for ui_file in ["src/panels.rs", "src/timeline_ui.rs", "src/main.rs"] {
        let src = std::fs::read_to_string(ui_file).unwrap();
        assert!(
            !src.contains("last_coalesce_key"),
            "{ui_file} に last_coalesce_key が残っていないこと"
        );
        assert!(
            !src.contains("end_coalesce"),
            "{ui_file} に end_coalesce が残っていないこと"
        );
    }

    let mut app = Is2dApp::default();

    // 2. FPS：1回なら1件、2回なら2件（他のキーの EndCoalesce では途切れないこと）
    let base = app.history.undo_stack.len();
    let _ = apply(&mut app, Action::SetFps { fps: 12 });
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::StageSize),
        },
    );
    let _ = apply(&mut app, Action::SetFps { fps: 15 });
    assert_eq!(app.history.undo_stack.len(), base + 1);
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::Fps),
        },
    );
    let _ = apply(&mut app, Action::SetFps { fps: 24 });
    let _ = apply(&mut app, Action::SetFps { fps: 30 });
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::Fps),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 2);

    // 4. ステージサイズ：1回なら1件、2回なら2件
    let base = app.history.undo_stack.len();
    let _ = apply(
        &mut app,
        Action::SetStageSize {
            size: Vec2::new(900.0, 500.0),
        },
    );
    let _ = apply(
        &mut app,
        Action::SetStageSize {
            size: Vec2::new(1000.0, 600.0),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 1);
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::StageSize),
        },
    );
    let _ = apply(
        &mut app,
        Action::SetStageSize {
            size: Vec2::new(1100.0, 650.0),
        },
    );
    let _ = apply(
        &mut app,
        Action::SetStageSize {
            size: Vec2::new(1280.0, 720.0),
        },
    );
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::StageSize),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 2);

    // 5. 矢印キー移動・シンボル再生設定・シンボル名変更：それぞれ1回なら1件、2回なら2件
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 6.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let base = app.history.undo_stack.len();
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(1.0, 0.0),
        },
    );
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(1.0, 0.0),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 1);
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::NudgeSelection),
        },
    );
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(1.0, 0.0),
        },
    );
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(1.0, 0.0),
        },
    );
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::NudgeSelection),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 2);

    let _ = apply(&mut app, Action::ConvertToSymbol);
    let sym_id = app.doc.library[0].id;
    let _ = apply(&mut app, Action::EnterSymbolFromLibrary { sym_id });
    let _ = apply(&mut app, Action::ExtendFrame);
    let _ = apply(&mut app, Action::ExtendFrame);
    let _ = apply(&mut app, Action::ExitSymbolTimeline);
    let gid = app.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .instance_gid;

    let base = app.history.undo_stack.len();
    let _ = apply(
        &mut app,
        Action::SetSymbolPlayback {
            loop_mode: None,
            first_frame: Some(1),
        },
    );
    let _ = apply(
        &mut app,
        Action::SetSymbolPlayback {
            loop_mode: None,
            first_frame: Some(2),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 1);
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::SymbolPlayback(gid)),
        },
    );
    let _ = apply(
        &mut app,
        Action::SetSymbolPlayback {
            loop_mode: None,
            first_frame: Some(1),
        },
    );
    let _ = apply(
        &mut app,
        Action::SetSymbolPlayback {
            loop_mode: None,
            first_frame: Some(0),
        },
    );
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::SymbolPlayback(gid)),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 2);

    let base = app.history.undo_stack.len();
    let _ = apply(
        &mut app,
        Action::RenameSymbol {
            sym_id,
            new_name: "S1".to_owned(),
        },
    );
    let _ = apply(
        &mut app,
        Action::RenameSymbol {
            sym_id,
            new_name: "S12".to_owned(),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 1);
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::RenameSymbol(sym_id)),
        },
    );
    let _ = apply(
        &mut app,
        Action::RenameSymbol {
            sym_id,
            new_name: "S123".to_owned(),
        },
    );
    let _ = apply(
        &mut app,
        Action::RenameSymbol {
            sym_id,
            new_name: "S1234".to_owned(),
        },
    );
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::RenameSymbol(sym_id)),
        },
    );
    assert_eq!(app.history.undo_stack.len(), base + 2);

    // 6. 再生中に UI が EndCoalesce を出しても再生が止まらず、Undo も増えないこと
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    let undo_before_pb_end = app.history.undo_stack.len();
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::StageSize),
        },
    );
    let _ = apply(
        &mut app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::Fps),
        },
    );
    let _ = apply(&mut app, Action::EndCoalesce { key: None });
    assert!(
        app.timeline_ui.is_playing,
        "再生中に EndCoalesce を出しても再生が止まらないこと"
    );
    assert_eq!(app.history.undo_stack.len(), undo_before_pb_end);
}

#[test]
fn test_g3_3_clear_timeline_cells_and_no_direct_ui_mutation_calls() {
    use crate::doc_actions::{apply, Action};

    // 1. UI 層（panels.rs, timeline_ui.rs, main.rs）から set_active_cell_selection と commit_floating の直接呼び出しがなくなっていること
    for ui_file in ["src/panels.rs", "src/timeline_ui.rs", "src/main.rs"] {
        let src = std::fs::read_to_string(ui_file).unwrap();
        assert!(
            !src.contains("set_active_cell_selection"),
            "{ui_file} に set_active_cell_selection が残っていないこと"
        );
        assert!(
            !src.contains("commit_floating"),
            "{ui_file} に commit_floating が残っていないこと"
        );
    }

    // 2. Action::ClearTimelineCells でセル範囲選択が解除されること
    let mut app = Is2dApp::default();
    let _ = apply(
        &mut app,
        Action::SelectTimelineCells {
            l0: 0,
            f0: 0,
            l1: 0,
            f1: 0,
        },
    );
    assert!(app.doc.root.cell_selection.is_some());
    let _ = apply(&mut app, Action::ClearTimelineCells);
    assert!(app.doc.root.cell_selection.is_none());

    // 3. 再生開始時に Floating が Deselect 経由で確定され、coalesce も解除されること
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 6.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(10.0, 0.0),
        },
    );
    assert!(app.session.floating().is_some());
    assert!(app.history.last_coalesce_key.is_some());
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    assert!(app.session.floating().is_none());
    assert!(app.history.last_coalesce_key.is_none());
}

#[test]
fn test_g2_1_edit_contour_bumps_mesh_revision() {
    let raw = generate_brush_outline_at_zoom(
        &[Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
        6.0,
        1.0,
    );
    let mut shape = build_smoothed_stroke_shapes_at_zoom(&raw, Color32::RED, 1.0)
        .into_iter()
        .next()
        .unwrap();
    let rev0 = shape.mesh_revision;

    // 変更なし（false を返す）なら mesh_revision は変わらないこと
    let changed_noop = shape.edit_contour(0, |_| false);
    assert!(!changed_noop);
    assert_eq!(shape.mesh_revision, rev0);

    // 輪郭を書き換えて true を返すと mesh_revision が必ず上がること
    let changed = shape.edit_contour(0, |c| c.insert_anchor_at(0, 0.5));
    assert!(changed);
    assert!(shape.mesh_revision > rev0);
}

#[test]
fn test_g1_5_floating_mutations_auto_rebuild_overlay() {
    use crate::canvas_tool::Floating;
    use std::collections::HashSet;

    // 1. 本番コードで rebuild_overlay_from_floating を呼ぶのが canvas_tool.rs (Session 内) だけであること
    for entry in std::fs::read_dir("src").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let fname = path.file_name().unwrap().to_str().unwrap();
        if fname == "canvas_tool.rs" || fname == "tests.rs" {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        assert!(
            !src.contains("rebuild_overlay_from_floating"),
            "{fname} に rebuild_overlay_from_floating が残っていないこと"
        );
    }

    // 2. set_floating / update_floating / take_floating の3メソッドだけで overlay と Scene が自動的に同期されること
    let mut app = Is2dApp::default();
    let raw = generate_brush_outline_at_zoom(
        &[Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
        6.0,
        1.0,
    );
    let mut shape = build_smoothed_stroke_shapes_at_zoom(&raw, Color32::RED, 1.0)
        .into_iter()
        .next()
        .unwrap();
    shape.layer_id = app.doc.root.layers[0].id;
    assert_eq!(app.scene().len(), 0);

    app.tabs[0].session.set_floating(Some(Floating {
        pieces: vec![shape],
        remainder: Vec::new(),
        hidden: HashSet::new(),
        modified: true,
    }));
    assert_eq!(app.scene().len(), 1);
    assert_eq!(app.scene().get(0).unwrap().color, Color32::RED);

    app.tabs[0].session.update_floating(|fl_opt| {
        if let Some(fl) = fl_opt {
            fl.pieces[0].set_color(Color32::BLUE);
        }
    });
    assert_eq!(app.scene().get(0).unwrap().color, Color32::BLUE);

    let taken = app.tabs[0].session.take_floating();
    assert!(taken.is_some());
    assert_eq!(app.scene().len(), 0);
}

#[test]
fn test_g6_1_resolved_cel_cache_byte_limit_and_2min_playback_memory() {
    use crate::symbol::SymbolLoopMode;

    // 1. 通常の純粋図形セルは Tier 2/3 キャッシュを再利用し、トゥイーン・シンボルセルはキャッシュを溜めずその場で構築すること
    let mut app = Is2dApp::default();
    let raw_pts: Vec<Point> = (0..8)
        .map(|p| Point::new((p as f64) * 15.0 + 20.0, 40.0))
        .collect();
    let raw_outline = generate_brush_outline_at_zoom(&raw_pts, 4.0, 1.0);
    let mut raw_shapes =
        build_smoothed_stroke_shapes_at_zoom(&raw_outline, Color32::from_rgb(40, 160, 90), 1.0);
    for s in &mut raw_shapes {
        s.layer_id = 1;
    }
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_parts(raw_shapes, Vec::new());
    let pure_m1 = app
        .doc
        .root
        .resolved_cel_screen_mesh(
            0,
            0,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &app.doc.library,
        )
        .unwrap();
    let pure_m2 = app
        .doc
        .root
        .resolved_cel_screen_mesh(
            0,
            0,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &app.doc.library,
        )
        .unwrap();
    assert!(
        !std::sync::Arc::ptr_eq(&pure_m1, &pure_m2) && pure_m1.vertices == pure_m2.vertices,
        "普通のセルも結合メッシュキャッシュを持たず毎コマ FillShape::fill_mesh から構築すること"
    );

    // 2. 2分アニメ（2,880フレーム）のトゥイーン再生後も解決済みメッシュキャッシュが溜まらずメモリが増えないこと
    let mut sym_shapes = Vec::new();
    for s_idx in 0..60 {
        let y = (s_idx as f64) * 6.0;
        let pts: Vec<Point> = (0..12)
            .map(|p| Point::new((p as f64) * 20.0, y + ((p + s_idx) % 3) as f64 * 4.0))
            .collect();
        let raw = generate_brush_outline_at_zoom(&pts, 5.0, 1.0);
        let mut shapes = build_smoothed_stroke_shapes_at_zoom(
            &raw,
            Color32::from_rgb((s_idx * 4) as u8, 120, 200),
            1.0,
        );
        for s in &mut shapes {
            s.layer_id = 1;
            let _ = s.fill_mesh();
        }
        sym_shapes.extend(shapes);
    }
    let sym_id = app.doc.register_new_symbol(sym_shapes);

    app.doc.root.layers[0].insert_frame_at(2_879);
    app.doc.root.sync_total_frames();
    let inst_start = timeline::SymbolInstance {
        binding: symbol::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((50.0, 50.0)),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
            color_effect: vector_engine::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        shape_order_index: 0,
    };
    let inst_end = timeline::SymbolInstance {
        binding: symbol::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((650.0, 350.0)) * kurbo::Affine::rotate(1.2),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
            color_effect: vector_engine::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        shape_order_index: 0,
    };
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_parts(Vec::new(), vec![inst_start]);
    app.doc.root.layers[0].keyframe_mut(0).tween = true;
    app.doc.root.layers[0].insert_keyframe(2_879, true);
    let last_kf_idx = app.doc.root.layers[0].keyframes.len() - 1;
    app.doc.root.layers[0]
        .keyframe_mut(last_kf_idx)
        .cel
        .set_parts(Vec::new(), vec![inst_end]);

    #[cfg(target_os = "macos")]
    let rss_before = current_process_rss_bytes();

    for f in 0..2_880 {
        let mesh = app.doc.root.resolved_cel_screen_mesh(
            0,
            f,
            1.0,
            Vec2::ZERO,
            false,
            kurbo::Affine::IDENTITY,
            &app.doc.library,
        );
        assert!(mesh.is_some());
    }

    #[cfg(target_os = "macos")]
    {
        let rss_after = current_process_rss_bytes();
        let rss_delta = rss_after.saturating_sub(rss_before);
        println!(
            "[G6-1] 2-min (2,880 frames) tween playback RSS: before = {:.2} MB, after = {:.2} MB, delta = {:.2} MB",
            rss_before as f64 / (1024.0 * 1024.0),
            rss_after as f64 / (1024.0 * 1024.0),
            rss_delta as f64 / (1024.0 * 1024.0),
        );
    }
}

#[test]
fn test_other_2_cmd_q_triggers_close_confirm_and_does_not_switch_tool() {
    use crate::doc_actions::{apply, Action};
    use crate::panels::UiAction;
    use crate::project_io::PendingCloseConfirm;
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 未保存の編集を作る（現在のツールは Brush）
    assert_eq!(app.tool.tool, Tool::Brush);
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    );
    assert!(app.history.is_dirty());

    // 1. Cmd+Q を押すと ViewportCommand::Close が送られ、自由変形ツール (Tool::Transform) には切り替わらないこと
    let cmd_mods = egui::Modifiers {
        command: true,
        ..Default::default()
    };
    let out_cmd_q = ctx.run(
        egui::RawInput {
            screen_rect,
            modifiers: cmd_mods,
            events: vec![egui::Event::Key {
                key: egui::Key::Q,
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
    assert_eq!(
        app.tool.tool,
        Tool::Brush,
        "Cmd+Q で自由変形ツール (Tool::Transform) に切り替わらないこと"
    );
    let root_cmds = &out_cmd_q.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(
        root_cmds.contains(&egui::ViewportCommand::Close),
        "Cmd+Q で ViewportCommand::Close が発行されること"
    );

    // 2. ViewportEvent::Close を受けると、未保存確認モーダルが出て CancelClose が送られること
    let mut close_raw = egui::RawInput {
        screen_rect,
        ..Default::default()
    };
    close_raw
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let out_close_req = ctx.run(close_raw, |ctx| {
        app.update_ui(ctx);
    });
    assert!(
        matches!(
            app.pending_close_confirm,
            Some(PendingCloseConfirm::ViewportClose { .. })
        ),
        "未保存状態で Cmd+Q によるクローズ要求を受けると既存の未保存確認が出ること"
    );
    assert!(
        out_close_req.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose),
        "未保存確認モーダル表示中は CancelClose で終了が阻止されること"
    );

    // 3. キャンセルすると終了しないこと
    app.apply_action(UiAction::ConfirmPendingCloseCancel, &ctx);
    assert!(app.pending_close_confirm.is_none());
    assert!(!app.allow_viewport_close);

    // 4. もう一度 Cmd+Q -> Close 要求 -> 「保存せずに閉じる」を選ぶと allow_viewport_close が立ち ViewportCommand::Close が送られること
    let mut close_raw2 = egui::RawInput {
        screen_rect,
        ..Default::default()
    };
    close_raw2
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    let _ = ctx.run(close_raw2, |ctx| {
        app.update_ui(ctx);
    });
    assert!(app.pending_close_confirm.is_some());

    let out_discard = ctx.run(
        egui::RawInput {
            screen_rect,
            ..Default::default()
        },
        |ctx| {
            app.apply_action(UiAction::ConfirmPendingCloseDiscard, ctx);
        },
    );
    assert!(app.pending_close_confirm.is_none());
    assert!(app.allow_viewport_close);
    assert!(
        out_discard.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::Close),
        "『保存せずに閉じる』を選ぶと ViewportCommand::Close が発行されて終了すること"
    );
}

#[test]
fn test_g6_2_writeback_copies_only_geometry_caches_and_no_screen_caches() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();

    // 1. 図形1を描画する
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    );
    app.refresh_scene_base();
    assert_eq!(app.scene().len(), 1);
    let shape1_id = app.scene().get(0).unwrap().shape_id;

    // ステージ側（SceneBaseCache）の図形1に fill_mesh, hit_bez_path, screen_mesh_cache, selection_mesh, selection_screen_mesh をすべて生成させる
    {
        let stage_s1 = app.scene().get(0).unwrap();
        let _ = stage_s1.fill_mesh();
        assert!(stage_s1.contains_point(Point::new(140.0, 100.0)));
        let _ = stage_s1.cached_screen_mesh(1.0, Vec2::ZERO, false);
        let _ = stage_s1.cached_selection_screen_mesh(1.0, Vec2::ZERO);
        assert!(stage_s1.has_cached_fill_mesh());
        assert!(stage_s1.has_cached_hit_bez_path());
        assert!(stage_s1.has_any_screen_cache());
    }

    // 2. 離れた位置に図形2を描画して書き戻し（edit_layers）を発生させる
    //    未変更の図形1（kept パス）も含め、文書側の図形には fill_mesh も hit_bez_path も画面用キャッシュも引き継がれないこと
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(400.0, 300.0), Point::new(480.0, 300.0)],
            radius: 10.0,
            color: Color32::BLUE,
        },
    );
    let doc_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
    assert_eq!(doc_shapes.len(), 2);
    let doc_s1 = doc_shapes.iter().find(|s| s.shape_id == shape1_id).unwrap();
    assert!(
        !doc_s1.has_cached_fill_mesh(),
        "書き戻し後、未変更の文書側図形にも fill_mesh は引き継がれないこと"
    );
    assert!(
        !doc_s1.has_cached_hit_bez_path(),
        "書き戻し後、未変更の文書側図形にも hit_bez_path は引き継がれないこと"
    );
    let stage_s1_after = app
        .scene()
        .iter()
        .find(|s| s.shape_id == shape1_id)
        .unwrap();
    assert!(
        stage_s1_after.has_cached_fill_mesh() && stage_s1_after.has_cached_hit_bez_path(),
        "土台キャッシュ側の未変更図形には donor_map 経由で fill_mesh と hit_bez_path が引き継がれること"
    );
    for s in doc_shapes.iter() {
        assert!(
            !s.has_any_screen_cache(),
            "書き戻し後、文書側の図形は screen_mesh_cache や選択表示用メッシュを持たないこと"
        );
    }

    // 3. 図形をグループ化して選択表示・移動（変更図形の local_s 書き戻しパス）を行ったあとも、文書側の図形が画面用キャッシュを持たないこと
    app.refresh_scene_base();
    for s in app.scene().iter() {
        let _ = s.cached_screen_mesh(1.25, Vec2::new(10.0, 20.0), false);
        let _ = s.cached_selection_screen_mesh(1.25, Vec2::new(10.0, 20.0));
        assert!(s.has_any_screen_cache());
    }
    let all_ids: Vec<_> = app.scene().iter().cloned().collect();
    for s in &all_ids {
        app.session.selection.insert_shape(s);
    }
    let _ = apply(&mut app, Action::Group);
    for s in app.scene().iter() {
        let _ = s.cached_screen_mesh(1.25, Vec2::new(10.0, 20.0), false);
        let _ = s.cached_selection_screen_mesh(1.25, Vec2::new(10.0, 20.0));
    }
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(15.0, -10.0),
        },
    );
    for s in app.doc.root.layers[0].keyframes[0].cel.shapes.iter() {
        assert!(
            !s.has_any_screen_cache(),
            "グループ移動後の書き戻しでも、文書側の図形が画面用キャッシュを持たないこと"
        );
    }

    // 4. シンボル変換後も、シンボル定義内の文書図形が画面用キャッシュを持たないこと
    for s in app.scene().iter() {
        let _ = s.cached_screen_mesh(1.0, Vec2::ZERO, false);
        let _ = s.cached_selection_screen_mesh(1.0, Vec2::ZERO);
    }
    let _ = apply(&mut app, Action::ConvertToSymbol);
    assert_eq!(app.doc.library.len(), 1);
    for s in app.doc.library[0].timeline.layers[0].keyframes[0]
        .cel
        .shapes
        .iter()
    {
        assert!(
            !s.has_any_screen_cache(),
            "シンボル定義内の文書図形も画面用キャッシュを持たないこと"
        );
    }
}

/// テッセレーション生成された Mesh（fill_mesh および直接輪郭メッシュ）の vertices / indices の
/// 確保容量（capacity）が実データ長（len）に切り詰められていることを検証する
#[test]
fn test_tessellate_lyon_path_shrinks_mesh_capacity_to_len() {
    let raw = generate_brush_outline_at_zoom(
        &[
            Point::new(100.0, 100.0),
            Point::new(140.0, 115.0),
            Point::new(180.0, 105.0),
        ],
        6.0,
        1.0,
    );
    let shapes = build_smoothed_stroke_shapes_at_zoom(&raw, Color32::RED, 1.0);
    assert_eq!(shapes.len(), 1);
    let mesh = shapes[0].fill_mesh();
    assert!(!mesh.vertices.is_empty() && !mesh.indices.is_empty());
    assert_eq!(
        mesh.vertices.capacity(),
        mesh.vertices.len(),
        "fill_mesh の vertices.capacity() が vertices.len() と一致すること"
    );
    assert_eq!(
        mesh.indices.capacity(),
        mesh.indices.len(),
        "fill_mesh の indices.capacity() が indices.len() と一致すること"
    );

    let direct_mesh = vector_engine::tessellate_raw_polygon(&raw, Color32::BLUE);
    assert_eq!(
        direct_mesh.vertices.capacity(),
        direct_mesh.vertices.len(),
        "直接輪郭メッシュの vertices.capacity() が vertices.len() と一致すること"
    );
    assert_eq!(
        direct_mesh.indices.capacity(),
        direct_mesh.indices.len(),
        "直接輪郭メッシュの indices.capacity() が indices.len() と一致すること"
    );
}

/// M2-1: 図形10個を持つシンボルや、入れ子シンボルを含むシンボルを Cmd+B (Action::Ungroup) で分解した際、
/// シンボルインスタンスが文書に残らず全図形が欠落・重複なく分解されることを検証する
#[test]
fn test_m2_1_ungroup_multi_shape_and_nested_symbol_detaches_all_shapes() {
    use crate::doc_actions::{apply, Action};

    // 1. 四角（互いに離れたストローク図形）10個のシンボルを置いて Cmd+B すると、
    //    文書にそのシンボルのインスタンスが残らず、生の図形が10個になり、形も分解前と同じであること
    let mut app = Is2dApp::default();
    for i in 0..10 {
        let x = 50.0 + (i % 5) as f64 * 120.0;
        let y = 80.0 + (i / 5) as f64 * 120.0;
        let _ = apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(x, y), Point::new(x + 40.0, y)],
                radius: 8.0,
                color: Color32::from_rgb(30 + i as u8 * 20, 100, 200),
            },
        );
    }
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 10);
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    assert_eq!(app.doc.library.len(), 1);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.instances.len(), 1);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 0);

    let before_bboxes: Vec<kurbo::Rect> = app.scene().iter().map(|s| s.bounding_box()).collect();
    let before_colors: Vec<Color32> = app.scene().iter().map(|s| s.color).collect();
    assert_eq!(before_bboxes.len(), 10);

    let _ = apply(&mut app, Action::Ungroup);
    assert!(
        app.doc.root.layers[0].keyframes[0].cel.instances.is_empty(),
        "10図形シンボルの分解後、文書にインスタンスが残らないこと"
    );

    // 選択解除で Floating を確定した後も、生の図形が10個で形・色が分解前と一致すること
    canvas_tool::clear_selection(&mut app);
    let cel = &app.doc.root.layers[0].keyframes[0].cel;
    assert!(cel.instances.is_empty());
    assert_eq!(cel.shapes.len(), 10, "生の図形が10個になること");
    for (idx, s) in cel.shapes.iter().enumerate() {
        assert!(s.group_path.is_empty() && s.symbol_instance.is_none());
        assert_eq!(s.color, before_colors[idx]);
        let bb = s.bounding_box();
        let exp = before_bboxes[idx];
        assert!(
            (bb.x0 - exp.x0).abs() < 1e-6
                && (bb.y0 - exp.y0).abs() < 1e-6
                && (bb.x1 - exp.x1).abs() < 1e-6
                && (bb.y1 - exp.y1).abs() < 1e-6,
            "分解前後の図形 #{idx} の形（bbox）が一致すること"
        );
    }

    // 2. シンボル B と図形1つを持つシンボル A を分解すると、A のインスタンスが残らないこと
    let mut app2 = Is2dApp::default();
    let _ = apply(
        &mut app2,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(150.0, 100.0)],
            radius: 8.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app2, Action::SelectAll);
    let _ = apply(&mut app2, Action::ConvertToSymbol);
    let sym_b_id = app2.doc.library[0].id;

    let _ = apply(
        &mut app2,
        Action::PaintStroke {
            points: vec![Point::new(250.0, 100.0), Point::new(300.0, 100.0)],
            radius: 8.0,
            color: Color32::BLUE,
        },
    );
    let _ = apply(&mut app2, Action::SelectAll);
    let _ = apply(&mut app2, Action::ConvertToSymbol);
    assert_eq!(app2.doc.library.len(), 2);
    let sym_a_id = app2.doc.library[1].id;
    assert_ne!(sym_a_id, sym_b_id);
    assert_eq!(app2.doc.root.layers[0].keyframes[0].cel.instances.len(), 1);
    assert_eq!(
        app2.doc.root.layers[0].keyframes[0].cel.instances[0]
            .binding
            .symbol_id,
        sym_a_id
    );

    let _ = apply(&mut app2, Action::Ungroup);
    canvas_tool::clear_selection(&mut app2);
    let cel2 = &app2.doc.root.layers[0].keyframes[0].cel;
    assert_eq!(cel2.instances.len(), 1);
    assert_eq!(cel2.instances[0].binding.symbol_id, sym_b_id);
    assert_eq!(cel2.shapes.len(), 1);
}

/// M2-2: トゥイーンの途中のコマでは直接編集できず警告が表示され、F6 挿入後に編集・開始フレーム指定が正しく動くこと、
/// およびキーフレームで選択・移動した状態で途中コマへ移動すると移る前のコマで確定して選択が解除されることを検証する
#[test]
fn test_m2_2_tween_midframe_blocks_edit_shows_warning_and_f6_enables_edit() {
    use crate::doc_actions::{apply, Action};
    use crate::symbol::SymbolLoopMode;

    let mut app = Is2dApp::default();
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::Copy);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    let sym_id = app.doc.library[0].id;

    // シンボル内を 8 コマのループアニメにする
    let _ = apply(&mut app, Action::EnterSymbolFromLibrary { sym_id });
    for _ in 0..7 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(&mut app, Action::ExitSymbolTimeline);
    assert_eq!(app.doc.library[0].timeline.total_frames, 8);

    // ルートタイムラインで Frame 0..=4 の完成したクラシックトゥイーンを作成（開始 first_frame = 1）
    app.doc.root.layers[0].keyframe_mut(0).cel.instances_mut()[0]
        .binding
        .first_frame = 1;
    let _ = apply(&mut app, Action::ToggleTween);
    for _ in 0..4 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 4,
        },
    );
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: true });
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(200.0, 0.0),
        },
    );

    // 1. キーフレームで図形を選択して移動し、トゥイーン途中のコマへ移ると、移る前のコマで確定され選択が外れること
    let _ = apply(&mut app, Action::AddLayer);
    for _ in 0..4 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 0,
        },
    );
    let init_kf0_ty = app.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .xform
        .as_coeffs()[5];
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(0.0, 40.0),
        },
    );
    assert!(!app.session.selection.is_empty());
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2,
        },
    );
    assert!(
        app.session.selection.is_empty(),
        "トゥイーン途中のコマへ移るとそのレイヤーの選択が外れること"
    );
    assert!(
        (app.doc.root.layers[0].keyframes[0].cel.instances[0]
            .binding
            .xform
            .as_coeffs()[5]
            - (init_kf0_ty + 40.0))
            .abs()
            < 1e-6,
        "移る前のキーフレーム (Frame 0) で移動が確定していること"
    );

    // Layer 2 (index 1) を削除してトゥイーンレイヤー1枚だけにし、Frame 2（トゥイーン途中）へ
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 1,
            frame: 2,
        },
    );
    let _ = apply(&mut app, Action::DeleteActiveLayer);
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2,
        },
    );
    assert!(!app.tab().active_timeline().is_active_layer_editable());
    assert!(!app.tab().active_timeline().has_any_editable_layer());
    assert_eq!(
        app.current_state_notice(false),
        Some((
            canvas_tool::NoticeKind::Warning,
            "トゥイーンの途中のコマは編集できません（F6 でキーフレームを挿入してください）",
        )),
        "トゥイーン途中のコマでキャンバスの警告メッセージが返ること"
    );

    // 2. トゥイーンの途中のコマでブラシ・消しゴム・バケツ・選択と移動・自由変形・再生設定・ペースト・配置をしても文書が変わらないこと
    let undo_len_before = app.history.undo_stack.len();
    let kf_count_before = app.doc.root.layers[0].keyframes.len();
    let kf0_rev_before = app.doc.root.layers[0].keyframes[0].cel.revision;
    let kf1_rev_before = app.doc.root.layers[0].keyframes[1].cel.revision;

    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(200.0, 120.0), Point::new(260.0, 120.0)],
            radius: 8.0,
            color: Color32::BLUE,
        },
    );
    let _ = apply(
        &mut app,
        Action::EraseStroke {
            points: vec![Point::new(200.0, 120.0), Point::new(260.0, 120.0)],
            radius: 12.0,
        },
    );
    let _ = apply(
        &mut app,
        Action::BucketFill {
            pt: Point::new(230.0, 120.0),
            color: Color32::GREEN,
        },
    );
    let _ = apply(
        &mut app,
        Action::SelectSingleAt {
            pt: Point::new(230.0, 120.0),
        },
    );
    assert!(app.session.selection.is_empty());
    let _ = apply(&mut app, Action::SelectAll);
    assert!(app.session.selection.is_empty());
    app.session.set_selected_indices(vec![0]);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(30.0, 0.0),
        },
    );
    let _ = apply(&mut app, Action::RotateSelection { angle_rad: 0.5 });
    let _ = apply(
        &mut app,
        Action::SetSymbolPlayback {
            loop_mode: Some(SymbolLoopMode::PlayOnce),
            first_frame: Some(3),
        },
    );
    let _ = apply(&mut app, Action::Paste { in_place: true });
    let _ = apply(&mut app, Action::PlaceSymbol { sym_id });

    assert_eq!(
        app.history.undo_stack.len(),
        undo_len_before,
        "トゥイーン途中の編集操作で Undo 履歴（文書状態）が進まないこと"
    );
    assert_eq!(app.doc.root.layers[0].keyframes.len(), kf_count_before);
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.revision,
        kf0_rev_before
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[1].cel.revision,
        kf1_rev_before
    );

    // 3. トゥイーン途中のコマ (Frame 2) で F6 を押すと、補間された姿勢・続きの開始フレーム (1 + 2 = 3) のキーフレームができ、
    //    その後開始フレームに 3 と入力すると経過コマ数が二重加算されず 3 になること
    let kf0_tx = app.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .xform
        .as_coeffs()[4];
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: true });
    assert_eq!(app.doc.root.layers[0].keyframes.len(), 3);
    let kf_mid = &app.doc.root.layers[0].keyframes[1];
    assert_eq!(kf_mid.start_frame, 2);
    assert_eq!(
        kf_mid.cel.instances[0].binding.first_frame, 3,
        "F6 挿入時は元の開始フレーム (1) + 経過コマ数 (2) = 3 になること"
    );
    assert!(
        (kf_mid.cel.instances[0].binding.xform.as_coeffs()[4] - (kf0_tx + 100.0)).abs() < 1e-6,
        "F6 挿入時は補間された姿勢 (tx = kf0_tx + 100.0) になること"
    );

    // 続けて開始フレームを一度 0 にしてから 3 に変更すると、経過コマ数が足されずちょうど 3 になること
    let _ = apply(
        &mut app,
        Action::SetSymbolPlayback {
            loop_mode: None,
            first_frame: Some(0),
        },
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[1].cel.instances[0]
            .binding
            .first_frame,
        0
    );
    let _ = apply(
        &mut app,
        Action::SetSymbolPlayback {
            loop_mode: None,
            first_frame: Some(3),
        },
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[1].cel.instances[0]
            .binding
            .first_frame,
        3,
        "F6 後のキーフレームで開始フレームに 3 と入力するとそのまま 3 になること"
    );
}

#[test]
fn test_m2_4_delete_symbol_2880_keyframes_fast_and_no_memory_growth() {
    use crate::doc_actions::{apply, Action};
    use std::time::Instant;

    let mut app = Is2dApp::default();
    let mut shapes = Vec::new();
    for i in 0..40 {
        let y = 20.0 + i as f64 * 8.0;
        let mut s = make_stroke(
            Point::new(20.0, y),
            Point::new(120.0, y + 4.0),
            3.0,
            Color32::from_rgb(50 + (i * 4) as u8, 100, 200),
        )
        .remove(0);
        s.layer_id = 1;
        shapes.push(s);
    }
    let sym_id = app.doc.register_new_symbol(shapes.clone());
    if let Some(sym) = app.doc.find_symbol_mut(sym_id) {
        sym.timeline.total_frames = 8;
        sym.timeline.layers[0].frame_count = 8;
        for f in 1..8 {
            sym.timeline.layers[0].insert_keyframe(f, false);
            sym.timeline.layers[0].keyframe_mut(f).cel = timeline::Cel::new(shapes.clone());
        }
    }
    app.doc.root.total_frames = 2880;
    app.doc.root.layers[0].frame_count = 2880;
    app.doc.root.layers[0].keyframes.clear();
    for f in 0..2880 {
        let inst = timeline::SymbolInstance {
            binding: symbol::SymbolInstanceBinding::new(
                (f + 1) as u64,
                sym_id,
                kurbo::Affine::translate(((f % 100) as f64 * 2.0, 50.0)),
            ),
            layer_id: 1,
            parent_scope: Vec::new(),
            shape_order_index: 0,
        };
        app.doc.root.layers[0]
            .keyframes
            .push(std::sync::Arc::new(timeline::Keyframe {
                start_frame: f,
                tween: true,
                tween_rotate: timeline::TweenRotate::Auto,
                cel: timeline::Cel::new_with_instances(Vec::new(), vec![inst]),
            }));
    }
    app.refresh_scene_base();
    let rss_before = current_process_rss_bytes();
    let t0 = Instant::now();
    let _ = apply(&mut app, Action::DeleteSymbol { sym_id });
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let rss_after = current_process_rss_bytes();
    let rss_delta_mb = (rss_after.saturating_sub(rss_before)) as f64 / (1024.0 * 1024.0);
    println!(
        "[M2-4] delete symbol across 2880 KF = {dt_ms:.3} ms, rss_delta = +{rss_delta_mb:.2} MB"
    );
    assert!(app.doc.root.layers[0]
        .keyframes
        .iter()
        .all(|kf| kf.cel.instances.is_empty() && kf.tween));
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        timeline::TweenState::Incomplete
    );
    app.undo();
    assert!(app.doc.find_symbol(sym_id).is_some());
    assert!(app.doc.root.layers[0]
        .keyframes
        .iter()
        .all(|kf| kf.cel.instances.len() == 1 && kf.tween));
}

#[test]
fn test_m2_6_onion_and_dim_backdrop_color_cache_matches_per_vertex() {
    use crate::vector_engine::{
        append_mesh, dim_backdrop_color, onion_tint_color, transform_mesh_to_screen,
    };
    use eframe::egui::{Mesh, Pos2};

    let palette = [
        Color32::from_rgb(220, 45, 55),
        Color32::from_rgb(40, 175, 95),
        Color32::from_rgb(55, 110, 230),
        Color32::from_rgba_unmultiplied(240, 180, 30, 200),
        Color32::from_rgb(220, 45, 55),
        Color32::from_rgb(130, 60, 210),
    ];
    let mut combined = Mesh::default();
    for (i, &c) in palette.iter().enumerate() {
        let x = (i as f64) * 30.0 + 20.0;
        let shapes = make_stroke(Point::new(x, 40.0), Point::new(x + 20.0, 90.0), 6.0, c);
        for s in &shapes {
            append_mesh(&mut combined, s.fill_mesh());
        }
    }
    assert!(combined.vertices.len() > 50);

    let zoom = 1.25f32;
    let pan = Vec2::new(10.0, -5.0);

    // (a) 色を変えない場合
    let plain = transform_mesh_to_screen(combined.clone(), zoom, pan, None);
    assert_eq!(plain.vertices.len(), combined.vertices.len());
    for (orig_v, out_v) in combined.vertices.iter().zip(plain.vertices.iter()) {
        let expected_pos = Pos2::new(orig_v.pos.x * zoom + pan.x, orig_v.pos.y * zoom + pan.y);
        assert_eq!(out_v.pos, expected_pos);
        assert_eq!(out_v.color, orig_v.color);
    }

    // (b) 背景を薄く見せる (dim_backdrop) 場合
    let dimmed = transform_mesh_to_screen(combined.clone(), zoom, pan, Some(&dim_backdrop_color));
    assert_eq!(dimmed.vertices.len(), combined.vertices.len());
    for (orig_v, out_v) in combined.vertices.iter().zip(dimmed.vertices.iter()) {
        let expected_pos = Pos2::new(orig_v.pos.x * zoom + pan.x, orig_v.pos.y * zoom + pan.y);
        assert_eq!(out_v.pos, expected_pos);
        let [r, g, b, a] = orig_v.color.to_srgba_unmultiplied();
        let expected = Color32::from_rgba_unmultiplied(
            ((r as u16 * 30 + 250 * 70) / 100) as u8,
            ((g as u16 * 30 + 250 * 70) / 100) as u8,
            ((b as u16 * 30 + 252 * 70) / 100) as u8,
            a,
        );
        assert_eq!(out_v.color, expected);
    }

    // (c) オニオンスキンの色の場合
    let tint = [45u8, 115, 245];
    let alpha = 64u8;
    let onion = transform_mesh_to_screen(
        combined.clone(),
        zoom,
        pan,
        Some(&|c| onion_tint_color(c, tint, alpha)),
    );
    assert_eq!(onion.vertices.len(), combined.vertices.len());
    for (orig_v, out_v) in combined.vertices.iter().zip(onion.vertices.iter()) {
        let expected_pos = Pos2::new(orig_v.pos.x * zoom + pan.x, orig_v.pos.y * zoom + pan.y);
        assert_eq!(out_v.pos, expected_pos);
        let [r, g, b, _] = orig_v.color.to_srgba_unmultiplied();
        let expected = Color32::from_rgba_unmultiplied(
            ((r as u16 * 35 + tint[0] as u16 * 65) / 100) as u8,
            ((g as u16 * 35 + tint[1] as u16 * 65) / 100) as u8,
            ((b as u16 * 35 + tint[2] as u16 * 65) / 100) as u8,
            alpha,
        );
        assert_eq!(out_v.color, expected);
    }
}

#[test]
fn test_m2_7_disjoint_floating_commit_matches_full_merge_on_lift_slice_alt_dup() {
    use crate::doc_actions::{apply, Action};
    use std::time::Instant;

    let palette = [
        Color32::from_rgb(220, 50, 50),
        Color32::from_rgb(50, 180, 80),
        Color32::from_rgb(50, 110, 220),
        Color32::from_rgb(230, 160, 40),
        Color32::from_rgb(160, 60, 210),
    ];

    let mut seed = 0x9876_5432_10fe_dcba_u64;
    let mut next_u64 = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed
    };

    // 4. 作品 A 相当（3レイヤー×60ストローク）の1コマを全選択して少し動かし、選択解除が 50ms 以下であること
    let mut dense_app = Is2dApp::default();
    let _ = apply(&mut dense_app, Action::AddLayer);
    let _ = apply(&mut dense_app, Action::AddLayer);
    for l_idx in 0..3 {
        let lid = dense_app.doc.root.layers[l_idx].id;
        let mut shapes = Vec::new();
        for i in 0..60 {
            let x = 40.0 + ((i % 10) as f64) * 80.0;
            let y = 40.0 + ((i / 10) as f64) * 70.0;
            let mut s = make_stroke(
                Point::new(x, y),
                Point::new(x + 45.0, y + 15.0),
                5.0,
                Color32::from_rgb(
                    ((i * 37) % 255) as u8,
                    ((l_idx * 70 + i * 13) % 255) as u8,
                    180,
                ),
            )
            .remove(0);
            s.layer_id = lid;
            shapes.push(s);
        }
        dense_app.doc.root.layers[l_idx].keyframe_mut(0).cel = timeline::Cel::new(shapes);
    }
    dense_app.refresh_scene_base();
    let _ = apply(&mut dense_app, Action::SelectAll);
    let _ = apply(
        &mut dense_app,
        Action::NudgeSelection {
            delta: KVec2::new(5.0, 3.0),
        },
    );
    let t0 = Instant::now();
    let _ = apply(&mut dense_app, Action::Deselect);
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("[M2-7] 60-color move deselect = {dt_ms:.3} ms");

    // 5. 5色のコマ（3レイヤー×25本＝5色で重なり結合済み）で全選択移動・Alt複製の確定が従来経路と一致すること
    let mut app5_base = Is2dApp::default();
    let _ = apply(&mut app5_base, Action::AddLayer);
    let _ = apply(&mut app5_base, Action::AddLayer);
    for l_idx in 0..3 {
        let _ = apply(
            &mut app5_base,
            Action::SwitchLayerAndFrame {
                layer_idx: l_idx,
                frame: 0,
            },
        );
        for i in 0..25 {
            let x0 = 60.0 + (next_u64() % 440) as f64;
            let y0 = 60.0 + (next_u64() % 240) as f64;
            let x1 = x0 + 35.0 + (next_u64() % 40) as f64;
            let y1 = y0 + ((next_u64() % 31) as f64 - 15.0);
            let _ = apply(
                &mut app5_base,
                Action::PaintStroke {
                    points: vec![Point::new(x0, y0), Point::new(x1, y1)],
                    radius: 5.5,
                    color: palette[i % palette.len()],
                },
            );
        }
    }
    let mut app5_move_fast = Is2dApp::default();
    app5_move_fast.tabs[0].doc = app5_base.doc.clone();
    app5_move_fast.refresh_scene_base();
    let _ = apply(&mut app5_move_fast, Action::SelectAll);
    let _ = apply(
        &mut app5_move_fast,
        Action::NudgeSelection {
            delta: KVec2::new(5.0, 3.0),
        },
    );
    let t0_5 = Instant::now();
    let _ = apply(&mut app5_move_fast, Action::Deselect);
    let dt5_ms = t0_5.elapsed().as_secs_f64() * 1000.0;
    println!("[M2-7] 5-color move deselect = {dt5_ms:.3} ms");

    let mut app5_alt_fast = Is2dApp::default();
    app5_alt_fast.tabs[0].doc = app5_base.doc.clone();
    app5_alt_fast.refresh_scene_base();
    let _ = apply(&mut app5_alt_fast, Action::SelectAll);
    let _ = apply(
        &mut app5_alt_fast,
        Action::FinishMoveSelectionDrag {
            total_delta: KVec2::new(12.0, 8.0),
            alt_duplicate: true,
        },
    );
    let _ = apply(&mut app5_alt_fast, Action::Deselect);

    // 6. 赤い大きな塗りの上に青い円（Oの字）を描き、そのまま線を伸ばして離れた緑の図形にも触れさせたとき、
    //    ブラシ描画でも Floating 確定（Alt複製）でも円の内側が赤のままであること
    let red = Color32::from_rgb(230, 50, 50);
    let green = Color32::from_rgb(50, 190, 70);
    let blue = Color32::from_rgb(50, 100, 230);

    let mut loop_tail_pts: Vec<Point> = (0..=48)
        .map(|k| {
            let ang = (k as f64 / 48.0) * std::f64::consts::TAU;
            Point::new(200.0 + 45.0 * ang.cos(), 200.0 + 45.0 * ang.sin())
        })
        .collect();
    for k in 1..=20 {
        loop_tail_pts.push(Point::new(245.0 + (k as f64) * 10.5, 200.0));
    }

    let setup_red_and_green = |app: &mut Is2dApp| {
        for y in (120..=280).step_by(20) {
            let _ = apply(
                app,
                Action::PaintStroke {
                    points: vec![Point::new(120.0, y as f64), Point::new(280.0, y as f64)],
                    radius: 18.0,
                    color: red,
                },
            );
        }
        let _ = apply(
            app,
            Action::PaintStroke {
                points: vec![Point::new(430.0, 170.0), Point::new(470.0, 230.0)],
                radius: 16.0,
                color: green,
            },
        );
    };

    let mut app_brush_loop = Is2dApp::default();
    setup_red_and_green(&mut app_brush_loop);
    let _ = apply(
        &mut app_brush_loop,
        Action::PaintStroke {
            points: loop_tail_pts.clone(),
            radius: 5.0,
            color: blue,
        },
    );
    assert_eq!(
        tool_paint::pick_color_at(&mut app_brush_loop, Point::new(200.0, 200.0)),
        Some(red),
        "ブラシで赤い塗りの上に青い円を描いて離れた緑に触れさせたとき、円の内側が赤のままであること"
    );

    let mut app_fl_loop = Is2dApp::default();
    setup_red_and_green(&mut app_fl_loop);
    let shifted_pts: Vec<Point> = loop_tail_pts
        .iter()
        .map(|&p| p + KVec2::new(0.0, 160.0))
        .collect();
    let _ = apply(
        &mut app_fl_loop,
        Action::PaintStroke {
            points: shifted_pts,
            radius: 5.0,
            color: blue,
        },
    );
    let _ = apply(
        &mut app_fl_loop,
        Action::SelectMarquee {
            p0: Point::new(140.0, 300.0),
            p1: Point::new(470.0, 420.0),
            additive: false,
        },
    );
    let _ = apply(
        &mut app_fl_loop,
        Action::FinishMoveSelectionDrag {
            total_delta: KVec2::new(0.0, -160.0),
            alt_duplicate: true,
        },
    );
    let _ = apply(&mut app_fl_loop, Action::Deselect);
    assert_eq!(
        tool_paint::pick_color_at(&mut app_fl_loop, Point::new(200.0, 200.0)),
        Some(red),
        "円の形の片を Alt 複製して赤い塗りと緑の図形の上に重ねて確定したとき、円の内側が赤のままであること"
    );
}

#[test]
fn test_m2_8_document_shapes_never_store_fill_mesh_after_playback_onion_edit_and_undo() {
    use crate::doc_actions::{apply, Action};

    // 1. ルートとシンボルライブラリを持つ作品を作成して保存し、open_project_in_tab で開く
    let mut setup_app = Is2dApp::default();
    for f in 0..8 {
        let _ = apply(&mut setup_app, Action::ScrubRulerToFrame { frame: f });
        if f > 0 {
            let _ = apply(&mut setup_app, Action::InsertKeyframe { copy_prev: false });
        }
        let _ = apply(
            &mut setup_app,
            Action::PaintStroke {
                points: vec![
                    Point::new(100.0 + f as f64 * 25.0, 120.0),
                    Point::new(160.0 + f as f64 * 25.0, 180.0),
                ],
                radius: 8.0,
                color: Color32::from_rgb(200, 60, 80),
            },
        );
    }
    let _ = apply(&mut setup_app, Action::ScrubRulerToFrame { frame: 0 });
    let _ = apply(&mut setup_app, Action::SelectAll);
    let _ = apply(&mut setup_app, Action::ConvertToSymbol);
    let _ = apply(&mut setup_app, Action::Deselect);
    let tmp_path =
        std::env::temp_dir().join(format!("m2_8_doc_mesh_test_{}.is2d", std::process::id()));
    project_io::save_project_to_path(&mut setup_app, &tmp_path).unwrap();

    let mut app = Is2dApp::default();
    project_io::open_project_in_tab(&mut app, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    assert!(
        !app.doc.has_any_cached_fill_mesh(),
        "開いた直後の文書（ルートとライブラリ）のどの図形にも fill_mesh が入っていないこと"
    );

    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        ..Default::default()
    };

    // 2. 再生で何コマか進める + オニオンスキンを表示する
    app.timeline_ui
        .ensure_timeline_context(&app.tabs[app.active]);
    app.view.onion_skin_enabled = true;
    app.view.onion_prev_frames = 2;
    app.view.onion_next_frames = 2;
    for f in 0..8 {
        app.advance_playback_frame(f);
        app.timeline_ui.is_playing = true;
        app.timeline_ui.last_frame_advance_time = None;
        let full_output = ctx.run(raw_input.clone(), |ctx| {
            app.update_ui(ctx);
        });
        let _ = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
    }
    app.stop_playback();
    assert!(
        !app.doc.has_any_cached_fill_mesh(),
        "再生およびオニオンスキン表示の後も文書に fill_mesh が入っていないこと"
    );

    // 3. ホバー（contains_point による当たり判定キャッシュ生成）→ 編集する（ブラシ追加・全選択移動・確定）→ Undo・Redo する
    let has_any_cached_hit_bez_path = |doc: &crate::symbol::Document| {
        let tl_has_hit = |tl: &crate::timeline::Timeline| {
            tl.layers.iter().any(|l| {
                l.keyframes
                    .iter()
                    .any(|kf| kf.cel.shapes.iter().any(FillShape::has_cached_hit_bez_path))
            })
        };
        tl_has_hit(&doc.root) || doc.library.iter().any(|sym| tl_has_hit(&sym.timeline))
    };
    let _ = apply(&mut app, Action::ScrubRulerToFrame { frame: 2 });
    for s in app.scene().iter() {
        let bb = s.bounding_box();
        let center = Point::new((bb.x0 + bb.x1) * 0.5, (bb.y0 + bb.y1) * 0.5);
        let _ = s.contains_point(center);
    }
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(300.0, 220.0), Point::new(380.0, 260.0)],
            radius: 10.0,
            color: Color32::from_rgb(50, 140, 220),
        },
    );
    for s in app.scene().iter() {
        let bb = s.bounding_box();
        let center = Point::new((bb.x0 + bb.x1) * 0.5, (bb.y0 + bb.y1) * 0.5);
        let _ = s.contains_point(center);
        let _ = s.cached_screen_mesh(1.0, Vec2::ZERO, false);
    }
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(12.0, -8.0),
        },
    );
    let _ = apply(&mut app, Action::Deselect);
    assert!(
        !app.doc.has_any_cached_fill_mesh(),
        "編集確定後も文書（ルートとライブラリ）のどの図形にも fill_mesh が入っていないこと"
    );
    assert!(
        !has_any_cached_hit_bez_path(&app.doc),
        "ホバー＋編集確定後も文書（ルートとライブラリ）のどの図形にも hit_bez_path が入っていないこと"
    );

    let _ = apply(&mut app, Action::Undo);
    let _ = apply(&mut app, Action::Undo);
    assert!(
        !app.doc.has_any_cached_fill_mesh() && !has_any_cached_hit_bez_path(&app.doc),
        "Undo の後も文書（ルートとライブラリ）のどの図形にも fill_mesh / hit_bez_path が入っていないこと"
    );
    let _ = apply(&mut app, Action::Redo);
    let _ = apply(&mut app, Action::Redo);
    assert!(
        !app.doc.has_any_cached_fill_mesh() && !has_any_cached_hit_bez_path(&app.doc),
        "Redo の後も文書（ルートとライブラリ）のどの図形にも fill_mesh / hit_bez_path が入っていないこと"
    );
}

#[test]
fn test_m2_10_prepared_clip_and_selective_flatten_1050_random_cases() {
    use kurbo::ParamCurveNearest;
    fn lcg(state: &mut u64) -> f64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((*state >> 32) as u32 as f64) / (u32::MAX as f64)
    }

    fn make_spiral_points(n: usize, cx: f64, cy: f64, max_r: f64) -> Vec<Point> {
        (0..n)
            .map(|i| {
                let t = i as f64 / (n.saturating_sub(1).max(1) as f64);
                let angle = t * 12.0 * std::f64::consts::PI;
                let r = 8.0 + t * max_r;
                Point::new(cx + r * angle.cos(), cy + r * angle.sin())
            })
            .collect()
    }

    fn make_short_stroke(state: &mut u64, n: usize) -> Vec<Point> {
        let mut x = 100.0 + lcg(state) * 600.0;
        let mut y = 60.0 + lcg(state) * 330.0;
        let mut angle = lcg(state) * std::f64::consts::TAU;
        let mut pts = Vec::with_capacity(n);
        for _ in 0..n {
            pts.push(Point::new(x, y));
            angle += (lcg(state) - 0.5) * 0.5;
            x = (x + angle.cos() * 6.0).clamp(20.0, 780.0);
            y = (y + angle.sin() * 6.0).clamp(20.0, 430.0);
        }
        pts
    }

    let colors = [
        Color32::from_rgb(220, 50, 50),
        Color32::from_rgb(50, 180, 70),
        Color32::from_rgb(50, 100, 220),
        Color32::from_rgb(230, 160, 30),
        Color32::from_rgb(160, 60, 210),
    ];

    // 1. 800点らせんブラシ・消しゴムの速度確認（5色・約135図形セル）
    let mut seed = 12345u64;
    let mut setup_app = Is2dApp::default();
    for i in 0..60 {
        let pts = make_short_stroke(&mut seed, 35);
        let col = colors[i % colors.len()];
        let _ = apply(
            &mut setup_app,
            Action::PaintStroke {
                points: pts,
                radius: 4.5,
                color: col,
            },
        );
    }
    let tmp_path = std::env::temp_dir().join(format!("m2_10_cel_{}.is2d", std::process::id()));
    project_io::save_project_to_path(&mut setup_app, &tmp_path).unwrap();

    let spiral_800 = make_spiral_points(800, 400.0, 225.0, 210.0);
    let mut app_brush = Is2dApp::default();
    project_io::open_project_in_tab(&mut app_brush, &tmp_path, None).unwrap();
    let t_b = std::time::Instant::now();
    let _ = apply(
        &mut app_brush,
        Action::PaintStroke {
            points: spiral_800.clone(),
            radius: 3.5,
            color: colors[0],
        },
    );
    for s in app_brush.scene().iter() {
        let _ = s.cached_screen_mesh(1.0, Vec2::ZERO, false);
    }
    let brush_800_ms = t_b.elapsed().as_secs_f64() * 1000.0;

    let mut app_erase = Is2dApp::default();
    project_io::open_project_in_tab(&mut app_erase, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    let t_e = std::time::Instant::now();
    let _ = apply(
        &mut app_erase,
        Action::EraseStroke {
            points: spiral_800,
            radius: 4.0,
        },
    );
    for s in app_erase.scene().iter() {
        let _ = s.cached_screen_mesh(1.0, Vec2::ZERO, false);
    }
    let erase_800_ms = t_e.elapsed().as_secs_f64() * 1000.0;
    println!("[M2-10] 800-point spiral brush = {brush_800_ms:.2} ms, erase = {erase_800_ms:.2} ms");

    // 2. 1,000通り以上（1,050通り）のランダム比較テスト（境界0.25px近傍・重なり・同色結合・セグメント数）
    fn min_dist_sq_to_shapes(pt: Point, shapes: &[FillShape]) -> f64 {
        let mut best = f64::INFINITY;
        for s in shapes {
            if !s.bounding_box().inflate(3.0, 3.0).contains(pt) {
                continue;
            }
            for c in s.contours() {
                for seg in c.segments.iter() {
                    let d_sq = seg.nearest(pt, 1e-4).distance_sq;
                    if d_sq < best {
                        best = d_sq;
                    }
                }
            }
        }
        best
    }

    fn shape_area_approx(s: &FillShape) -> f64 {
        use kurbo::ParamCurve;
        let poly = |c: &vector_engine::Contour| -> f64 {
            let mut pts = Vec::with_capacity(c.segments.len() * 8);
            for seg in c.segments.iter() {
                for k in 0..8 {
                    let p = seg.eval(k as f64 / 8.0);
                    pts.push([p.x, p.y]);
                }
            }
            vector_engine::ring_area_and_perimeter_f64(&pts).0
        };
        let ext = poly(&s.exterior);
        let holes: f64 = s.holes.iter().map(poly).sum();
        (ext - holes).max(0.0)
    }

    fn check_scope_invariants(canvas: &[FillShape]) -> (usize, f64, usize) {
        let mut overlap_viol = 0usize;
        let mut max_area = 0.0_f64;
        let mut unmerged_viol = 0usize;
        for i in 0..canvas.len() {
            for j in (i + 1)..canvas.len() {
                let a = &canvas[i];
                let b = &canvas[j];
                if a.layer_id != b.layer_id
                    || a.group_path != b.group_path
                    || a.symbol_instance.is_some()
                    || b.symbol_instance.is_some()
                {
                    continue;
                }
                if vector_engine::rects_overlap(a.bounding_box(), b.bounding_box()) {
                    let inter = vector_boolean::boolean_operation(
                        std::slice::from_ref(a),
                        std::slice::from_ref(b),
                        vector_boolean::BoolOp::Intersection,
                        Color32::WHITE,
                    );
                    let area: f64 = inter.iter().map(shape_area_approx).sum();
                    max_area = max_area.max(area);
                    if area > 2.0 {
                        overlap_viol += 1;
                    }
                }
                if a.color == b.color
                    && vector_engine::rects_overlap(
                        a.bounding_box().inflate(1.0, 1.0),
                        b.bounding_box(),
                    )
                {
                    let unioned = vector_boolean::boolean_operation(
                        std::slice::from_ref(a),
                        std::slice::from_ref(b),
                        vector_boolean::BoolOp::Union,
                        a.color,
                    );
                    if unioned.len() < 2 {
                        unmerged_viol += 1;
                    }
                }
            }
        }
        (overlap_viol, max_area, unmerged_viol)
    }

    let mut cmp_seed = 7777777u64;
    let mut near_checked = 0usize;
    let mut near_mismatches = 0usize;
    let mut overlap_violations = 0usize;
    let mut max_overlap_area = 0.0_f64;
    let mut unmerged_same_col = 0usize;
    let mut total_segs = 0usize;

    for case_idx in 0..1050 {
        let mut canvas: Vec<FillShape> = Vec::new();
        let init_strokes = 3 + (case_idx % 5);
        for s_i in 0..init_strokes {
            let n_pts = 8 + (s_i * 5);
            let mut x = 40.0 + lcg(&mut cmp_seed) * 320.0;
            let mut y = 40.0 + lcg(&mut cmp_seed) * 220.0;
            let mut ang = lcg(&mut cmp_seed) * std::f64::consts::TAU;
            let mut pts = Vec::with_capacity(n_pts);
            for _ in 0..n_pts {
                pts.push(Point::new(x, y));
                ang += (lcg(&mut cmp_seed) - 0.5) * 0.8;
                x = (x + ang.cos() * 14.0).clamp(20.0, 380.0);
                y = (y + ang.sin() * 14.0).clamp(20.0, 280.0);
            }
            let raw = vector_stroke::generate_brush_outline_at_zoom(&pts, 6.0, 1.0);
            let shapes =
                vector_stroke::build_smoothed_stroke_shapes_at_zoom(&raw, colors[s_i % 3], 1.0);
            vector_boolean::merge_shapes_into_canvas(&mut canvas, shapes);
        }
        let canvas_before = canvas.clone();

        let stroke_pts = match case_idx % 3 {
            0 => {
                let mut x = 50.0 + lcg(&mut cmp_seed) * 300.0;
                let mut y = 50.0 + lcg(&mut cmp_seed) * 200.0;
                let mut ang = lcg(&mut cmp_seed) * std::f64::consts::TAU;
                (0..12)
                    .map(|_| {
                        let p = Point::new(x, y);
                        ang += (lcg(&mut cmp_seed) - 0.5) * 0.7;
                        x = (x + ang.cos() * 12.0).clamp(20.0, 380.0);
                        y = (y + ang.sin() * 12.0).clamp(20.0, 280.0);
                        p
                    })
                    .collect::<Vec<_>>()
            }
            1 => {
                let mut x = 30.0 + lcg(&mut cmp_seed) * 340.0;
                let mut y = 30.0 + lcg(&mut cmp_seed) * 240.0;
                let mut ang = lcg(&mut cmp_seed) * std::f64::consts::TAU;
                (0..80)
                    .map(|_| {
                        let p = Point::new(x, y);
                        ang += (lcg(&mut cmp_seed) - 0.5) * 0.9;
                        x = (x + ang.cos() * 10.0).clamp(20.0, 380.0);
                        y = (y + ang.sin() * 10.0).clamp(20.0, 280.0);
                        p
                    })
                    .collect::<Vec<_>>()
            }
            _ => {
                let cx = 140.0 + lcg(&mut cmp_seed) * 120.0;
                let cy = 110.0 + lcg(&mut cmp_seed) * 80.0;
                make_spiral_points(100, cx, cy, 110.0)
            }
        };
        let is_eraser = case_idx % 2 == 1;
        let brush_col = colors[(case_idx / 2) % 4];
        let radius = 5.0 + (case_idx % 3) as f64 * 1.5;
        let raw = vector_stroke::generate_brush_outline_at_zoom(&stroke_pts, radius, 1.0);
        let stroke_shapes =
            vector_stroke::build_smoothed_stroke_shapes_at_zoom(&raw, brush_col, 1.0);
        if stroke_shapes.is_empty() {
            continue;
        }

        if is_eraser {
            vector_boolean::erase_from_canvas(&mut canvas, &stroke_shapes, 1, &[]);
        } else {
            vector_boolean::merge_shapes_into_canvas(&mut canvas, stroke_shapes.clone());
        }

        let segs: usize = canvas
            .iter()
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum();
        total_segs += segs;

        let (ov, ma, un) = check_scope_invariants(&canvas);
        overlap_violations += ov;
        max_overlap_area = max_overlap_area.max(ma);
        unmerged_same_col += un;

        // 格子点 + 境界から 0.25px の法線オフセット標本点
        let mut test_pts: Vec<Point> = Vec::with_capacity(120);
        for gx in 0..10 {
            for gy in 0..8 {
                test_pts.push(Point::new(25.0 + gx as f64 * 38.0, 25.0 + gy as f64 * 35.0));
            }
        }
        use kurbo::{ParamCurve, ParamCurveDeriv};
        for s in stroke_shapes.iter().chain(canvas_before.iter()) {
            for c in s.contours() {
                let step = (c.segments.len() / 6).max(1);
                for seg in c.segments.iter().step_by(step) {
                    let p = seg.eval(0.5);
                    let d = seg.deriv().eval(0.5).to_vec2();
                    let len = d.hypot();
                    if len > 1.0 {
                        let n = kurbo::Vec2::new(-d.y / len, d.x / len);
                        test_pts.push(p + n * 0.25);
                        test_pts.push(p - n * 0.25);
                    }
                }
            }
        }

        for pt in test_pts {
            let d_sq = min_dist_sq_to_shapes(pt, &stroke_shapes)
                .min(min_dist_sq_to_shapes(pt, &canvas_before));
            if d_sq < 0.24 * 0.24 {
                continue;
            }
            near_checked += 1;
            let in_stroke = stroke_shapes.iter().any(|s| s.contains_point(pt));
            let orig_col = canvas_before
                .iter()
                .rev()
                .find(|s| s.contains_point(pt))
                .map(|s| s.color);
            let new_col = canvas
                .iter()
                .rev()
                .find(|s| s.contains_point(pt))
                .map(|s| s.color);
            let expected_col = if in_stroke {
                if is_eraser {
                    None
                } else {
                    Some(brush_col)
                }
            } else {
                orig_col
            };
            if new_col != expected_col {
                near_mismatches += 1;
            }
        }
    }
    assert!(
        near_checked >= 150_000,
        "境界0.25px近傍を含む十分な標本点数を検査すること"
    );
    assert_eq!(
        near_mismatches, 0,
        "1,050通りのランダム比較テスト（境界0.25px近傍含む）で食い違いがないこと"
    );
    assert_eq!(
        overlap_violations, 0,
        "同じスコープの生の図形同士が重ならないこと (max_overlap_area={:.4})",
        max_overlap_area
    );
    assert_eq!(
        unmerged_same_col, 0,
        "同じ色で接する図形が別々の図形のまま残っていないこと"
    );
    // M2-10前の総セグメント数は 174,097 セグメント
    assert!(
        total_segs <= 174_200,
        "結果の輪郭のセグメント数がM2-10前(174,097)と比べて大きく増えていないこと: {}",
        total_segs
    );
}

#[test]
fn test_m3_1_write_project_json_reports_flush_error() {
    let tab = crate::canvas_tool::Tab::new_empty(1, 1);
    let doc = crate::project_io::ProjectDocument::from_app(&tab);

    let tmp = std::env::temp_dir().join(format!("is2d_m3_1_ro_{}.tmp", std::process::id()));
    std::fs::write(&tmp, b"orig").unwrap();
    let ro_file = std::fs::File::open(&tmp).unwrap();
    let res = crate::project_io::write_project_json(ro_file, &doc);
    let _ = std::fs::remove_file(&tmp);
    assert!(
        res.is_err(),
        "BufWriter::into_inner の flush 失敗が Err として返ること"
    );

    #[cfg(target_os = "linux")]
    {
        let dev_full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let err = crate::project_io::write_project_json(dev_full, &doc)
            .expect_err("/dev/full への書き込みは Err が返ること");
        assert_eq!(err.kind(), std::io::ErrorKind::StorageFull);
    }
}

#[test]
fn test_m3_2_tween_midframe_cannot_be_selected_by_any_entry_or_deleted() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    // レイヤー0: シンボルの完成したトゥイーン (Frame 0..=4)
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    let _ = apply(&mut app, Action::ToggleTween);
    for _ in 0..4 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 4,
        },
    );
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: true });
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(200.0, 0.0),
        },
    );

    // レイヤー1: 作画レイヤー (Frame 0..=4 に生シェイプ)
    let _ = apply(&mut app, Action::AddLayer);
    for _ in 0..4 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 1,
            frame: 0,
        },
    );
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(120.0, 220.0), Point::new(220.0, 220.0)],
            radius: 8.0,
            color: Color32::BLUE,
        },
    );

    // トゥイーン途中のコマ (Frame 2) へ移動
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2,
        },
    );
    let tween_lid = app.doc.root.layers[0].id;
    let draw_lid = app.doc.root.layers[1].id;

    // 1. 矩形選択で両方囲んでも、作画レイヤーだけ選ばれ、トゥイーン途中レイヤーのシンボルは入らない
    let _ = apply(
        &mut app,
        Action::SelectMarquee {
            p0: Point::new(0.0, 0.0),
            p1: Point::new(500.0, 500.0),
            additive: false,
        },
    );
    let sel_indices = app.session.selected_indices();
    assert!(!sel_indices.is_empty(), "作画レイヤーの図形は選べること");
    for idx in &sel_indices {
        let s = app.scene().get(*idx).unwrap();
        assert_eq!(
            s.layer_id, draw_lid,
            "矩形選択にトゥイーン途中レイヤーの図形が入らないこと"
        );
    }

    // 2. Shift+クリック (ToggleSelectionAt) でもトゥイーン途中レイヤーのシンボルは選択に入らない
    let _ = apply(&mut app, Action::Deselect);
    let tween_scene_idx = app
        .scene()
        .iter()
        .position(|s| s.layer_id == tween_lid)
        .expect("シーン上にトゥイーン途中のシンボルが存在すること");
    let draw_scene_idx = app
        .scene()
        .iter()
        .position(|s| s.layer_id == draw_lid)
        .expect("シーン上に作画レイヤーの図形が存在すること");
    let _ = apply(
        &mut app,
        Action::ToggleSelectionAt {
            scene_idx: tween_scene_idx,
        },
    );
    assert!(
        app.session.selection.is_empty(),
        "Shift+クリックでトゥイーン途中のシンボルが選択に入らないこと"
    );
    let _ = apply(
        &mut app,
        Action::ToggleSelectionAt {
            scene_idx: draw_scene_idx,
        },
    );
    assert!(
        !app.session.selection.is_empty(),
        "Shift+クリックで作画レイヤーの図形は選べること"
    );

    // 3. タイムラインのセル選択でもトゥイーン途中レイヤーのシンボルは選択に入らない
    let _ = apply(
        &mut app,
        Action::SelectTimelineCells {
            l0: 0,
            f0: 2,
            l1: 0,
            f1: 2,
        },
    );
    assert!(
        app.session.selection.is_empty(),
        "タイムラインのセル選択でトゥイーン途中のシンボルが選択に入らないこと"
    );
    let _ = apply(
        &mut app,
        Action::SelectTimelineCells {
            l0: 0,
            f0: 2,
            l1: 1,
            f1: 2,
        },
    );
    let sel_indices = app.session.selected_indices();
    assert!(!sel_indices.is_empty(), "作画レイヤーの図形は選べること");
    for idx in &sel_indices {
        let s = app.scene().get(*idx).unwrap();
        assert_eq!(s.layer_id, draw_lid);
    }

    // 4. トゥイーン途中のコマで Delete を押しても、Undo が積まれず、`*` も付かない
    let _ = apply(&mut app, Action::Deselect);
    app.history.mark_saved();
    let undo_before = app.history.undo_stack.len();
    let tween_shape = app.scene().get(tween_scene_idx).unwrap().clone();
    app.session.selection.insert_shape(&tween_shape);
    let deleted = apply(&mut app, Action::DeleteSelection);
    assert!(!deleted, "消せる図形が無ければ false が返ること");
    assert_eq!(
        app.history.undo_stack.len(),
        undo_before,
        "Undo が積まれないこと"
    );
    assert!(!app.history.is_dirty(), "未保存フラグ(*)が付かないこと");
}

#[test]
fn test_m3_3_tween_kept_on_and_incomplete_on_draw_paste_save_delete_and_toggle_off() {
    use crate::doc_actions::{apply, Action};
    use crate::timeline::TweenState;

    let mut app = Is2dApp::default();
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::Copy);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    let sym_id = app.doc.library[0].id;
    let _ = apply(&mut app, Action::ToggleTween);
    for _ in 0..4 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 4,
        },
    );
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: true });
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(200.0, 0.0),
        },
    );
    assert!(matches!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Complete { .. }
    ));

    // 1. 開始キーフレームにブラシで1本描くと tween は ON のまま Incomplete になり、その線を消すと Complete に戻る
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 0,
        },
    );
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(300.0, 300.0), Point::new(360.0, 300.0)],
            radius: 8.0,
            color: Color32::BLUE,
        },
    );
    assert!(app.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete
    );
    let _ = apply(
        &mut app,
        Action::SelectSingleAt {
            pt: Point::new(330.0, 300.0),
        },
    );
    let _ = apply(&mut app, Action::DeleteSelection);
    assert!(app.doc.root.layers[0].keyframes[0].tween);
    assert!(matches!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Complete { .. }
    ));

    // 2. ペーストでも同じく tween は ON のまま Incomplete になり、消すと Complete に戻る
    let _ = apply(&mut app, Action::Paste { in_place: true });
    let _ = apply(&mut app, Action::Deselect);
    assert!(app.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete
    );

    // 3. 保存して読み込んでも、未完のトゥイーンの tween は ON のまま
    let tmp_path = std::env::temp_dir().join(format!("is2d_m3_3_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut loaded = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    assert!(loaded.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        loaded.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete
    );

    // 4. 未完のトゥイーンのキーフレームで T を押すと OFF になる
    let _ = apply(&mut loaded, Action::ToggleTween);
    assert!(!loaded.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        loaded.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::None
    );

    // ペーストした生シェイプを消すと Complete に戻り、シンボル削除で再び ON のまま Incomplete になる
    let raw_idx = app
        .scene()
        .iter()
        .position(|s| s.symbol_instance.is_none())
        .unwrap();
    app.session.set_selected_indices(vec![raw_idx]);
    let _ = apply(&mut app, Action::DeleteSelection);
    assert!(matches!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Complete { .. }
    ));
    let _ = apply(&mut app, Action::DeleteSymbol { sym_id });
    assert!(app.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete
    );
}

#[test]
fn test_m3_4_delete_keyframe_only_on_keyframe_start_frame() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    // レイヤー0: Frame 0 と Frame 2 にキーフレーム (0..=4)
    for _ in 0..4 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2,
        },
    );
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: false });
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(150.0, 100.0)],
            radius: 6.0,
            color: Color32::RED,
        },
    );

    // レイヤー1: Frame 0 と Frame 3 にキーフレーム (0..=4)
    let _ = apply(&mut app, Action::AddLayer);
    for _ in 0..4 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 1,
            frame: 3,
        },
    );
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: false });

    // 1. 保持フレーム (Frame 4: レイヤー0は start_frame=2 の保持中) で Shift+F6 を押しても何も変わらず Undo も積まれない
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 4,
        },
    );
    app.history.mark_saved();
    let undo_before = app.history.undo_stack.len();
    let deleted = apply(&mut app, Action::DeleteKeyframe);
    assert!(
        !deleted,
        "保持フレームでは DeleteKeyframe が false を返すこと"
    );
    assert_eq!(app.doc.root.layers[0].keyframes.len(), 2);
    assert_eq!(app.history.undo_stack.len(), undo_before);
    assert!(!app.history.is_dirty());

    // 2. 複数レイヤー選択時 (Frame 3: レイヤー0は保持フレーム、レイヤー1は start_frame=3 の開始フレーム) は開始フレームのレイヤー1だけで消える
    let _ = apply(
        &mut app,
        Action::SelectTimelineCells {
            l0: 0,
            f0: 3,
            l1: 1,
            f1: 3,
        },
    );
    let deleted = apply(&mut app, Action::DeleteKeyframe);
    assert!(deleted);
    assert_eq!(
        app.doc.root.layers[0].keyframes.len(),
        2,
        "保持フレームだったレイヤー0のキーフレーム (start_frame=2) は消えないこと"
    );
    assert_eq!(
        app.doc.root.layers[1].keyframes.len(),
        1,
        "開始フレームだったレイヤー1のキーフレーム (start_frame=3) だけ消えること"
    );

    // 3. 開始フレーム (Frame 2, レイヤー0) で押したときは今までどおり消える
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2,
        },
    );
    let deleted = apply(&mut app, Action::DeleteKeyframe);
    assert!(deleted);
    assert_eq!(app.doc.root.layers[0].keyframes.len(), 1);
}

#[test]
fn test_m4_1_alt_duplicate_touching_sliced_pieces_merges_into_single_shape_on_deselect() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    // 1. 空の文書でブラシを1本描く
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 150.0), Point::new(300.0, 150.0)],
            radius: 10.0,
            color: Color32::from_rgb(220, 60, 60),
        },
    );
    assert_eq!(app.doc.root.layers[0].cel_at(0).unwrap().shapes.len(), 1);

    // 2. 左半分を矩形選択（スライス）
    let _ = apply(
        &mut app,
        Action::SelectMarquee {
            p0: Point::new(80.0, 120.0),
            p1: Point::new(200.0, 180.0),
            additive: false,
        },
    );
    // 3. 右半分を Shift+クリックで選択に追加
    let right_idx = app
        .scene()
        .iter()
        .position(|s| s.contains_point(Point::new(250.0, 150.0)))
        .unwrap();
    let _ = apply(
        &mut app,
        Action::ToggleSelectionAt {
            scene_idx: right_idx,
        },
    );
    // 4. Alt+ドラッグで下へ 150px 複製移動
    let _ = apply(
        &mut app,
        Action::FinishMoveSelectionDrag {
            total_delta: KVec2::new(0.0, 150.0),
            alt_duplicate: true,
        },
    );
    assert!(
        app.session
            .floating()
            .is_some_and(|fl| fl.pieces.len() == 1),
        "つながったスライス片は選び足した時点で1片に結合され、Alt複製後も1片が floating に入っていること"
    );

    // 5. 選択を解除すると、元の1図形と、1つに結合された複製の計2図形になること
    let _ = apply(&mut app, Action::Deselect);
    assert_eq!(
        app.doc.root.layers[0].cel_at(0).unwrap().shapes.len(),
        2,
        "選択解除後、セルの図形は元の1本と結合された複製の2つであること"
    );

    // 6. 保存して読み込んでも2図形のままであること
    let tmp_path = std::env::temp_dir().join(format!("m4_1_alt_dup_{}.is2d", std::process::id()));
    project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut reopened = Is2dApp::default();
    project_io::open_project_in_tab(&mut reopened, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    assert_eq!(
        reopened.doc.root.layers[0].cel_at(0).unwrap().shapes.len(),
        2,
        "保存して読み込んでも2図形であること"
    );
}

#[test]
fn test_m4_2_mid_tween_edit_restoring_classic_tween_rebuilds_base_cache_at_interpolated_position() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();

    // 1. シンボル1つを置いたキーフレームをフレーム0とフレーム20に作り、位置を 200px ずらしてクラシックトゥイーンにする
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(140.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    );
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    let _ = apply(&mut app, Action::ToggleTween);
    for _ in 0..20 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 20,
        },
    );
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: true });
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(200.0, 0.0),
        },
    );
    let _ = apply(&mut app, Action::Deselect);

    // 2. フレーム0でブラシを1本描く（生シェイプが混ざり、クラシックトゥイーンが未完になる）
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 0,
        },
    );
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(400.0, 300.0), Point::new(440.0, 300.0)],
            radius: 8.0,
            color: Color32::BLUE,
        },
    );

    // 3. フレーム10に移り、その線を消しゴムで消す（セルがシンボル1つに戻り、クラシックトゥイーンが成り立ち直す）
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 10,
        },
    );
    let _ = apply(
        &mut app,
        Action::EraseStroke {
            points: vec![Point::new(380.0, 300.0), Point::new(460.0, 300.0)],
            radius: 25.0,
        },
    );

    // 4. 土台キャッシュのインスタンスの境界矩形が、土台キャッシュを捨てて文書から作り直したものと同じ（補間位置 x+100px）であること
    assert_eq!(app.scene().len(), 1);
    let bbox_after_edit = app.scene().get(0).unwrap().bounding_box();
    app.tabs[app.active].session.base_cache = Default::default();
    app.tabs[app.active].refresh_scene_base();
    let bbox_rebuilt = app.scene().get(0).unwrap().bounding_box();
    assert!(
        (bbox_after_edit.x0 - bbox_rebuilt.x0).abs() <= 1e-6
            && (bbox_after_edit.y0 - bbox_rebuilt.y0).abs() <= 1e-6
            && (bbox_after_edit.x1 - bbox_rebuilt.x1).abs() <= 1e-6
            && (bbox_after_edit.y1 - bbox_rebuilt.y1).abs() <= 1e-6,
        "トゥイーン成立回復後の土台キャッシュ位置 {:?} が文書再構築位置 {:?} と一致すること",
        bbox_after_edit,
        bbox_rebuilt,
    );
    let center_x = (bbox_after_edit.x0 + bbox_after_edit.x1) * 0.5;
    assert!(
        (center_x - 220.0).abs() <= 1.0,
        "フレーム10のインスタンス中心 X 座標が補間位置 (~220.0) にあること: got {center_x}"
    );
}

#[test]
fn test_m4_3_saved_contour_arc_json_matches_legacy_and_load_shrinks_capacities() {
    use crate::doc_actions::{apply, Action};

    // 1. 輪郭1つを SavedContour にして JSON にした文字列が、版 2 の平らな配列形式になること
    let segs = vec![
        QuadBez::new(
            Point::new(10.5, 20.25),
            Point::new(30.125, 45.75),
            Point::new(50.0, 20.25),
        ),
        QuadBez::new(
            Point::new(50.0, 20.25),
            Point::new(30.125, -5.5),
            Point::new(10.5, 20.25),
        ),
    ];
    let contour = vector_engine::Contour::new(segs);
    let saved = project_io::SavedContour::from(&contour);
    assert!(
        std::sync::Arc::ptr_eq(&contour.segments, &saved.segments),
        "SavedContour::from(&Contour) は座標配列を複製せず Arc を共有すること"
    );
    let json_new = serde_json::to_string(&saved).unwrap();
    assert_eq!(
        json_new, r#"{"segments":[10.5,20.25,30.125,45.75,50.0,20.25,30.125,-5.5]}"#,
        "SavedContour の JSON 表現が版 2 の形式と完全一致すること"
    );

    // 2. 保存して読み込むと、図形の形が同じで、線分の Vec・穴の Vec・図形の Vec の capacity が len と一致すること
    let mut app = Is2dApp::default();
    for i in 0..5 {
        let x0 = 60.0 + i as f64 * 90.0;
        let _ = apply(
            &mut app,
            Action::PaintStroke {
                points: vec![
                    Point::new(x0, 100.0),
                    Point::new(x0 + 50.0, 100.0),
                    Point::new(x0 + 50.0, 150.0),
                    Point::new(x0, 150.0),
                    Point::new(x0, 100.0),
                ],
                radius: 8.0,
                color: Color32::from_rgb((i * 40 + 40) as u8, 120, 200),
            },
        );
    }
    let tmp_path = std::env::temp_dir().join(format!("m4_3_shrink_{}.is2d", std::process::id()));
    project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut loaded = Is2dApp::default();
    project_io::open_project_in_tab(&mut loaded, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);

    let orig_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
    let loaded_shapes = &loaded.doc.root.layers[0].keyframes[0].cel.shapes;
    assert_eq!(orig_shapes.len(), loaded_shapes.len());
    assert_eq!(
        loaded_shapes.capacity(),
        loaded_shapes.len(),
        "読み込んだセルの shapes Vec の capacity が len と一致すること"
    );
    let contours_match = |a: &vector_engine::Contour, b: &vector_engine::Contour| {
        a.segments.len() == b.segments.len()
            && a.segments.iter().zip(b.segments.iter()).all(|(sa, sb)| {
                (sa.p0.x - sb.p0.x).abs() <= 5e-4 + 1e-9
                    && (sa.p0.y - sb.p0.y).abs() <= 5e-4 + 1e-9
                    && (sa.p1.x - sb.p1.x).abs() <= 5e-4 + 1e-9
                    && (sa.p1.y - sb.p1.y).abs() <= 5e-4 + 1e-9
                    && (sa.p2.x - sb.p2.x).abs() <= 5e-4 + 1e-9
                    && (sa.p2.y - sb.p2.y).abs() <= 5e-4 + 1e-9
            })
    };
    for (orig_s, load_s) in orig_shapes.iter().zip(loaded_shapes.iter()) {
        assert!(contours_match(&orig_s.exterior, &load_s.exterior));
        assert_eq!(
            load_s.exterior.segments.capacity(),
            load_s.exterior.segments.len(),
            "読み込んだ外周 segments Vec の capacity が len と一致すること"
        );
        assert_eq!(
            load_s.holes.capacity(),
            load_s.holes.len(),
            "読み込んだ holes Vec の capacity が len と一致すること"
        );
        for (oh, lh) in orig_s.holes.iter().zip(load_s.holes.iter()) {
            assert!(contours_match(oh, lh));
            assert_eq!(
                lh.segments.capacity(),
                lh.segments.len(),
                "読み込んだ穴 segments Vec の capacity が len と一致すること"
            );
        }
    }
}

/// M4-5 (N-1001): 穴の多い図形の一部の穴にだけ触れる線を1本描いたとき、
/// 触れていない穴の `Contour` の線分が、描く前と同じ `Arc<Vec<QuadBez>>` を指していること
#[test]
fn test_m4_5_boolean_shares_untouched_hole_contour_arcs() {
    use crate::doc_actions::{apply, Action};
    use crate::vector_engine::rects_overlap;
    use kurbo::Rect as KRect;

    let mut seed = 0x9876_5432_10fe_dcba_u64;
    let mut next_f64 = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((seed >> 32) as u32 as f64) / (u32::MAX as f64)
    };
    let mut gen_stroke = |len: usize| -> Vec<Point> {
        let mut x = 150.0 + next_f64() * 500.0;
        let mut y = 100.0 + next_f64() * 300.0;
        let mut ang = next_f64() * std::f64::consts::TAU;
        let mut pts = Vec::with_capacity(len);
        for _ in 0..len {
            pts.push(Point::new(x, y));
            ang += (next_f64() - 0.5) * 0.6;
            x = (x + ang.cos() * 7.5).clamp(80.0, 720.0);
            y = (y + ang.sin() * 7.5).clamp(60.0, 440.0);
        }
        pts
    };

    let mut app = Is2dApp::default();
    for _ in 0..80 {
        let pts = gen_stroke(30);
        let _ = apply(
            &mut app,
            Action::PaintStroke {
                points: pts,
                radius: 2.5,
                color: Color32::BLACK,
            },
        );
    }

    let before_shapes = app.doc.root.layers[0].keyframes[0].cel.shapes.clone();
    let total_holes_before: usize = before_shapes.iter().map(|s| s.holes.len()).sum();
    assert!(
        total_holes_before >= 100,
        "多数の穴を持つ図形が生成されていること: holes={total_holes_before}"
    );

    // 一部の穴にだけ触れる線を1本描く
    let stroke_pts = gen_stroke(25);
    let stroke_bbox = stroke_pts
        .iter()
        .fold(
            KRect::new(
                stroke_pts[0].x,
                stroke_pts[0].y,
                stroke_pts[0].x,
                stroke_pts[0].y,
            ),
            |acc, &p| acc.union_pt(p),
        )
        .inflate(5.0, 5.0);

    let untouched_hole_arcs: Vec<std::sync::Arc<Vec<QuadBez>>> = before_shapes
        .iter()
        .flat_map(|s| &s.holes)
        .filter(|h| !rects_overlap(h.bounding_box(), stroke_bbox))
        .map(|h| std::sync::Arc::clone(&h.segments))
        .collect();
    assert!(
        untouched_hole_arcs.len() >= 50,
        "線に触れない穴が十分にあること: untouched={}",
        untouched_hole_arcs.len()
    );

    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: stroke_pts,
            radius: 2.5,
            color: Color32::BLACK,
        },
    );

    let after_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
    for prev_arc in &untouched_hole_arcs {
        let shared = after_shapes
            .iter()
            .flat_map(|s| &s.holes)
            .any(|h| std::sync::Arc::ptr_eq(&h.segments, prev_arc));
        assert!(
            shared,
            "触れていない穴の Contour は描く前と同じ Arc<Vec<QuadBez>> を共有していること"
        );
    }
}

/// M4-5 (N-1001): 3色・半径 0.5〜25・長さ1〜60点のブラシと消しゴムをランダムに300回行い、
/// 毎回、塗られた面積が理論上の範囲（許容 1%）に入ること
#[test]
fn test_m4_5_random_brush_and_eraser_area_bounds_300_steps() {
    use crate::vector_boolean::erase_from_canvas;

    let colors = [
        Color32::from_rgb(220, 60, 60),
        Color32::from_rgb(60, 180, 90),
        Color32::from_rgb(60, 120, 220),
    ];
    let shape_area = |s: &FillShape| -> f64 {
        (s.exterior.signed_area().abs()
            - s.holes.iter().map(|h| h.signed_area().abs()).sum::<f64>())
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
        let raw = generate_brush_outline(&pts, radius);
        let stroke: Vec<FillShape> = build_smoothed_stroke_shapes(&raw, color)
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

#[test]
fn test_arrange_groups_and_symbols_z_order() {
    let c_raw = Color32::from_rgb(100, 100, 100);
    let c_a = Color32::from_rgb(220, 60, 60);
    let c_b = Color32::from_rgb(60, 180, 90);
    let c_c = Color32::from_rgb(60, 120, 220);

    let paint_dot = |app: &mut Is2dApp, pt: Point, color: Color32| {
        apply(
            app,
            Action::PaintStroke {
                points: vec![pt],
                radius: 15.0,
                color,
            },
        );
    };
    let setup_four_items = |app: &mut Is2dApp| {
        paint_dot(app, Point::new(100.0, 100.0), c_raw);
        apply(app, Action::Deselect);
        paint_dot(app, Point::new(200.0, 100.0), c_a);
        apply(
            app,
            Action::SelectSingleAt {
                pt: Point::new(200.0, 100.0),
            },
        );
        apply(app, Action::Group);
        apply(app, Action::Deselect);
        paint_dot(app, Point::new(300.0, 100.0), c_b);
        apply(
            app,
            Action::SelectSingleAt {
                pt: Point::new(300.0, 100.0),
            },
        );
        apply(app, Action::ConvertToSymbol);
        apply(app, Action::Deselect);
        paint_dot(app, Point::new(400.0, 100.0), c_c);
        apply(
            app,
            Action::SelectSingleAt {
                pt: Point::new(400.0, 100.0),
            },
        );
        apply(app, Action::Group);
        apply(app, Action::Deselect);
    };
    let colors_in_scene =
        |app: &Is2dApp| -> Vec<Color32> { app.tab().scene().iter().map(|s| s.color).collect() };
    let select_by_color = |app: &mut Is2dApp, target_colors: &[Color32]| {
        apply(app, Action::Deselect);
        let mut sel = crate::canvas_tool::Selection::default();
        let depth = app.tab().session.editing_group_path.len();
        for s in app.tab().scene().iter() {
            if target_colors.contains(&s.color) {
                if let Some(b) = s
                    .symbol_instance
                    .filter(|b| s.group_path.get(depth) == Some(&b.instance_gid))
                {
                    sel.instances.insert(b.instance_gid);
                } else {
                    sel.shapes.insert(s.shape_id);
                }
            }
        }
        app.tab_mut().session.selection = sel;
    };

    // 1. 生の図形・グループA・シンボルB・グループC の6ケース
    let cases = [
        (vec![c_a], ArrangeMode::ToFront, vec![c_raw, c_b, c_c, c_a]),
        (vec![c_c], ArrangeMode::ToBack, vec![c_raw, c_c, c_a, c_b]),
        (vec![c_a], ArrangeMode::Forward, vec![c_raw, c_b, c_a, c_c]),
        (vec![c_c], ArrangeMode::Backward, vec![c_raw, c_a, c_c, c_b]),
        (
            vec![c_a, c_c],
            ArrangeMode::ToFront,
            vec![c_raw, c_b, c_a, c_c],
        ),
        (
            vec![c_a, c_c],
            ArrangeMode::ToBack,
            vec![c_raw, c_a, c_c, c_b],
        ),
    ];
    for (sel_colors, mode, expected) in cases {
        let mut app = Is2dApp::default();
        setup_four_items(&mut app);
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_a, c_b, c_c]);
        select_by_color(&mut app, &sel_colors);
        assert!(apply(&mut app, Action::Arrange { mode }));
        assert_eq!(colors_in_scene(&app), expected);
    }

    // 2. 既に最前面にあるものを「最前面へ」、生の図形だけを選んでいるとき：Undo も `*` も付かない
    {
        let mut app = Is2dApp::default();
        setup_four_items(&mut app);
        app.tab_mut().history.mark_saved();
        let undo_before = app.tab().history.undo_stack.len();

        select_by_color(&mut app, &[c_c]);
        assert!(!apply(
            &mut app,
            Action::Arrange {
                mode: ArrangeMode::ToFront
            }
        ));
        assert_eq!(app.tab().history.undo_stack.len(), undo_before);
        assert!(!app.tab().history.is_dirty());

        select_by_color(&mut app, &[c_raw]);
        assert!(!apply(
            &mut app,
            Action::Arrange {
                mode: ArrangeMode::ToFront
            }
        ));
        assert_eq!(app.tab().history.undo_stack.len(), undo_before);
        assert!(!app.tab().history.is_dirty());
    }

    // 3. 並べ替えたあと保存して読み込んでも同じ順番。Undo 1回で元の順番に戻る
    {
        let mut app = Is2dApp::default();
        setup_four_items(&mut app);
        select_by_color(&mut app, &[c_a]);
        assert!(apply(
            &mut app,
            Action::Arrange {
                mode: ArrangeMode::ToFront
            }
        ));
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_b, c_c, c_a]);

        let tmp_path =
            std::env::temp_dir().join(format!("arrange_test_{}.is2d", std::process::id()));
        project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
        let mut loaded_app = Is2dApp::default();
        project_io::open_project_in_tab(&mut loaded_app, &tmp_path, None).unwrap();
        let _ = std::fs::remove_file(&tmp_path);
        assert_eq!(colors_in_scene(&loaded_app), vec![c_raw, c_b, c_c, c_a]);

        app.undo();
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_a, c_b, c_c]);
    }

    // 4. 2つのレイヤーにまたがって選んでいるとき、それぞれのレイヤーの中だけで並べ替わる
    {
        let mut app = Is2dApp::default();
        setup_four_items(&mut app);
        apply(&mut app, Action::AddLayer);
        let c_l2_1 = Color32::from_rgb(200, 180, 40);
        let c_l2_2 = Color32::from_rgb(160, 60, 200);
        paint_dot(&mut app, Point::new(150.0, 250.0), c_l2_1);
        apply(
            &mut app,
            Action::SelectSingleAt {
                pt: Point::new(150.0, 250.0),
            },
        );
        apply(&mut app, Action::Group);
        apply(&mut app, Action::Deselect);
        paint_dot(&mut app, Point::new(250.0, 250.0), c_l2_2);
        apply(
            &mut app,
            Action::SelectSingleAt {
                pt: Point::new(250.0, 250.0),
            },
        );
        apply(&mut app, Action::Group);

        select_by_color(&mut app, &[c_a, c_l2_1]);
        assert!(apply(
            &mut app,
            Action::Arrange {
                mode: ArrangeMode::ToFront
            }
        ));
        assert_eq!(
            colors_in_scene(&app),
            vec![c_raw, c_b, c_c, c_a, c_l2_2, c_l2_1]
        );
        let l1_id = app.tab().active_timeline().layers[0].id;
        let l2_id = app.tab().active_timeline().layers[1].id;
        assert!(app
            .tab()
            .scene()
            .iter()
            .filter(|s| [c_raw, c_a, c_b, c_c].contains(&s.color))
            .all(|s| s.layer_id == l1_id));
        assert!(app
            .tab()
            .scene()
            .iter()
            .filter(|s| [c_l2_1, c_l2_2].contains(&s.color))
            .all(|s| s.layer_id == l2_id));
    }

    // 5. グループの中を編集しているとき、そのグループの中の塊だけが並べ替わり、外の図形の位置は変わらない
    {
        let mut app = Is2dApp::default();
        setup_four_items(&mut app);
        select_by_color(&mut app, &[c_a, c_c]);
        apply(&mut app, Action::Group);
        // 外側は [c_raw, c_b, 親グループ(c_a, c_c)]
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_b, c_a, c_c]);
        assert!(apply(
            &mut app,
            Action::EnterGroupAt {
                pt: Point::new(200.0, 100.0)
            }
        ));
        assert_eq!(app.tab().session.editing_group_path.len(), 1);
        select_by_color(&mut app, &[c_a]);
        assert!(apply(
            &mut app,
            Action::Arrange {
                mode: ArrangeMode::ToFront
            }
        ));
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_b, c_c, c_a]);
        apply(&mut app, Action::ExitGroupSteps { steps: 1 });
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_b, c_c, c_a]);
    }

    // 6. トゥイーンの途中のコマでは何も起きない
    {
        let mut app = Is2dApp::default();
        paint_dot(&mut app, Point::new(150.0, 150.0), c_b);
        apply(
            &mut app,
            Action::SelectSingleAt {
                pt: Point::new(150.0, 150.0),
            },
        );
        apply(&mut app, Action::ConvertToSymbol);
        apply(&mut app, Action::ToggleTween);
        for _ in 0..4 {
            apply(&mut app, Action::ExtendFrame);
        }
        apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 4,
            },
        );
        apply(&mut app, Action::InsertKeyframe { copy_prev: true });
        apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 2,
            },
        );
        select_by_color(&mut app, &[c_b]);
        app.tab_mut().history.mark_saved();
        assert!(!apply(
            &mut app,
            Action::Arrange {
                mode: ArrangeMode::ToFront
            }
        ));
        assert!(!app.tab().history.is_dirty());
    }

    // 7. シンボルの中のタイムラインでも、1. の最初の項目と同じ結果になる
    {
        let mut app = Is2dApp::default();
        paint_dot(&mut app, Point::new(100.0, 100.0), c_raw);
        apply(
            &mut app,
            Action::SelectSingleAt {
                pt: Point::new(100.0, 100.0),
            },
        );
        apply(&mut app, Action::ConvertToSymbol);
        let sym_id = app.tab().doc.library[0].id;
        assert!(apply(&mut app, Action::EnterSymbolFromLibrary { sym_id }));
        apply(&mut app, Action::ClearActiveFrame);
        setup_four_items(&mut app);
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_a, c_b, c_c]);
        select_by_color(&mut app, &[c_a]);
        assert!(apply(
            &mut app,
            Action::Arrange {
                mode: ArrangeMode::ToFront
            }
        ));
        assert_eq!(colors_in_scene(&app), vec![c_raw, c_b, c_c, c_a]);
    }
}

#[test]
fn test_video_export_common_render_and_dimensions() {
    use crate::canvas_tool::View;
    use crate::vector_engine::Contour;
    use crate::video_export::{export_dimensions, render_frame};

    let make_rect_shape = |x: f64, y: f64, w: f64, h: f64, color: Color32| -> FillShape {
        let p00 = Point::new(x, y);
        let p10 = Point::new(x + w, y);
        let p11 = Point::new(x + w, y + h);
        let p01 = Point::new(x, y + h);
        FillShape::new(
            color,
            Contour::new(vec![
                QuadBez::new(p00, p00.midpoint(p10), p10),
                QuadBez::new(p10, p10.midpoint(p11), p11),
                QuadBez::new(p11, p11.midpoint(p01), p01),
                QuadBez::new(p01, p01.midpoint(p00), p00),
            ]),
            Vec::new(),
        )
    };

    let pixel_at = |rgba: &[u8], width: u32, x: u32, y: u32| -> [u8; 4] {
        let idx = ((y as usize) * (width as usize) + (x as usize)) * 4;
        [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
    };

    let center = View::DEFAULT_STAGE_CENTER;
    let red = Color32::from_rgb(255, 0, 0);
    let blue = Color32::from_rgb(0, 0, 255);

    // 1. ステージ 800×450、背景だけの作品で、render_frame の画像がすべて白
    let mut app = Is2dApp::default();
    let (w, h) = export_dimensions(app.tab().doc.stage_size);
    assert_eq!((w, h), (800, 450));
    let bg_only = render_frame(&app.tab().doc, 0, w, h);
    assert_eq!(bg_only.len(), (w as usize) * (h as usize) * 4);
    assert!(bg_only.iter().all(|&b| b == 255));

    // 2. ステージの真ん中に赤い四角を描いた作品で、画像の真ん中が赤、四隅が白
    let rect_shape = make_rect_shape(center.x - 50.0, center.y - 50.0, 100.0, 100.0, red);
    put_cel_shapes(&mut app, 0, 0, vec![rect_shape]);
    let red_center_img = render_frame(&app.tab().doc, 0, w, h);
    assert_eq!(pixel_at(&red_center_img, w, w / 2, h / 2), [255, 0, 0, 255]);
    assert_eq!(pixel_at(&red_center_img, w, 0, 0), [255, 255, 255, 255]);
    assert_eq!(pixel_at(&red_center_img, w, w - 1, 0), [255, 255, 255, 255]);
    assert_eq!(pixel_at(&red_center_img, w, 0, h - 1), [255, 255, 255, 255]);
    assert_eq!(
        pixel_at(&red_center_img, w, w - 1, h - 1),
        [255, 255, 255, 255]
    );

    // 3. その赤い四角のレイヤーを非表示にすると、画像がすべて白
    app.tab_mut().doc.root.layers[0].visible = false;
    let hidden_img = render_frame(&app.tab().doc, 0, w, h);
    assert!(hidden_img.iter().all(|&b| b == 255));
    app.tab_mut().doc.root.layers[0].visible = true;

    // 4. 2つのレイヤーを重ねたとき、手前のレイヤー（tl.layers の後ろのほう）の色が上に塗られる
    apply(&mut app, Action::AddLayer);
    assert_eq!(app.tab().doc.root.layers.len(), 2);
    let blue_rect = make_rect_shape(center.x - 40.0, center.y - 40.0, 80.0, 80.0, blue);
    put_cel_shapes(&mut app, 1, 0, vec![blue_rect]);
    let layered_img = render_frame(&app.tab().doc, 0, w, h);
    assert_eq!(pixel_at(&layered_img, w, w / 2, h / 2), [0, 0, 255, 255]);

    // 5. シンボルのクラシックトゥイーン（左端から右端へ移動）の途中のコマで、四角が途中の位置に塗られる
    let mut tween_app = Is2dApp::default();
    let stage_left = center.x - 400.0;
    let start_cx = stage_left + 100.0;
    let end_cx = stage_left + 700.0;
    let box_shape = make_rect_shape(start_cx - 30.0, center.y - 30.0, 60.0, 60.0, red);
    put_cel_shapes(&mut tween_app, 0, 0, vec![box_shape]);
    tween_app.tab_mut().session.set_selected_indices(vec![0]);
    apply(&mut tween_app, Action::ConvertToSymbol);
    apply(&mut tween_app, Action::ToggleTween);
    for _ in 0..4 {
        apply(&mut tween_app, Action::ExtendFrame);
    }
    apply(
        &mut tween_app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 4,
        },
    );
    apply(&mut tween_app, Action::InsertKeyframe { copy_prev: true });
    tween_app.tab_mut().session.set_selected_indices(vec![0]);
    apply(
        &mut tween_app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(end_cx - start_cx, 0.0),
        },
    );
    let mid_img = render_frame(&tween_app.tab().doc, 2, w, h);
    assert_eq!(pixel_at(&mid_img, w, 400, 225), [255, 0, 0, 255]);
    assert_eq!(pixel_at(&mid_img, w, 100, 225), [255, 255, 255, 255]);
    assert_eq!(pixel_at(&mid_img, w, 700, 225), [255, 255, 255, 255]);

    // 6. ステージ 801×451 のとき、書き出しの大きさが 802×452 になり、右端と下端が白
    let mut odd_app = Is2dApp::default();
    apply(
        &mut odd_app,
        Action::SetStageSize {
            size: Vec2::new(801.0, 451.0),
        },
    );
    let (odd_w, odd_h) = export_dimensions(odd_app.tab().doc.stage_size);
    assert_eq!((odd_w, odd_h), (802, 452));
    let full_cover = make_rect_shape(center.x - 600.0, center.y - 400.0, 1200.0, 800.0, red);
    put_cel_shapes(&mut odd_app, 0, 0, vec![full_cover]);
    let odd_img = render_frame(&odd_app.tab().doc, 0, odd_w, odd_h);
    assert_eq!(pixel_at(&odd_img, odd_w, 400, 225), [255, 0, 0, 255]);
    for y in 0..odd_h {
        assert_eq!(
            pixel_at(&odd_img, odd_w, odd_w - 1, y),
            [255, 255, 255, 255],
            "右端の1ピクセル列 (x = 801) は白であること"
        );
    }
    for x in 0..odd_w {
        assert_eq!(
            pixel_at(&odd_img, odd_w, x, odd_h - 1),
            [255, 255, 255, 255],
            "下端の1ピクセル行 (y = 451) は白であること"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn test_video_export_macos_mp4_cancel_and_error() {
    use crate::canvas_tool::View;
    use crate::vector_engine::Contour;
    use crate::video_export::{tmp_export_path, VideoExportJob, VideoExportOutcome};

    let make_rect_shape = |x: f64, y: f64, w: f64, h: f64, color: Color32| -> FillShape {
        let p00 = Point::new(x, y);
        let p10 = Point::new(x + w, y);
        let p11 = Point::new(x + w, y + h);
        let p01 = Point::new(x, y + h);
        FillShape::new(
            color,
            Contour::new(vec![
                QuadBez::new(p00, p00.midpoint(p10), p10),
                QuadBez::new(p10, p10.midpoint(p11), p11),
                QuadBez::new(p11, p11.midpoint(p01), p01),
                QuadBez::new(p01, p01.midpoint(p00), p00),
            ]),
            Vec::new(),
        )
    };

    let tmp_dir = std::env::temp_dir().join(format!("is2d_video_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let center = View::DEFAULT_STAGE_CENTER;
    let red = Color32::from_rgb(255, 0, 0);

    // 7. 10 コマの小さな作品を一時フォルダに書き出し、ファイルができていること、大きさが 0 でないこと、先頭から 4〜8 バイト目が ftyp であること
    {
        let mut app = Is2dApp::default();
        let rect = make_rect_shape(center.x - 40.0, center.y - 40.0, 80.0, 80.0, red);
        put_cel_shapes(&mut app, 0, 0, vec![rect]);
        for _ in 0..9 {
            apply(&mut app, Action::ExtendFrame);
        }
        assert_eq!(app.tab().doc.root.total_frames, 10);

        let out_path = tmp_dir.join("ten_frames.mp4");
        let tmp_file = tmp_export_path(&out_path);
        let mut job = VideoExportJob::start(app.tab().committed_document_copy(), out_path.clone());
        let outcome = job.wait();
        assert_eq!(outcome, VideoExportOutcome::Completed);
        assert!(out_path.exists());
        assert!(!tmp_file.exists());
        let bytes = std::fs::read(&out_path).unwrap();
        assert!(bytes.len() > 8);
        assert_eq!(&bytes[4..8], b"ftyp");
    }

    // 8. 3 コマ目で中止すると、目的の名前のファイルも一時ファイルも残らないこと
    {
        let mut app = Is2dApp::default();
        let rect = make_rect_shape(center.x - 40.0, center.y - 40.0, 80.0, 80.0, red);
        put_cel_shapes(&mut app, 0, 0, vec![rect]);
        app.tab_mut().doc.root.layers[0].frame_count = 500;
        app.tab_mut().doc.root.total_frames = 500;

        let cancel_path = tmp_dir.join("cancelled.mp4");
        let cancel_tmp = tmp_export_path(&cancel_path);
        let mut job =
            VideoExportJob::start(app.tab().committed_document_copy(), cancel_path.clone());
        while job.progress().0 < 3 {
            std::hint::spin_loop();
        }
        let outcome = job.cancel_and_wait();
        assert_eq!(outcome, VideoExportOutcome::Cancelled);
        assert!(!cancel_path.exists());
        assert!(!cancel_tmp.exists());
    }

    // 9. 書き込めない場所を書き出し先にすると Err になり、ファイルが残らないこと
    {
        let app = Is2dApp::default();
        let bad_dir = tmp_dir.join("nonexistent_subdir");
        let bad_path = bad_dir.join("out.mp4");
        let bad_tmp = tmp_export_path(&bad_path);
        let mut job = VideoExportJob::start(app.tab().committed_document_copy(), bad_path.clone());
        let outcome = job.wait();
        assert!(matches!(outcome, VideoExportOutcome::Failed(_)));
        assert!(!bad_path.exists());
        assert!(!bad_tmp.exists());
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_m5_1_n3002_f7_on_empty_frame_preserves_redo_and_clean_state() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let c = app.tab().stage_center_world();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![c, Point::new(c.x + 80.0, c.y)],
            radius: 6.0,
            color: Color32::from_rgb(220, 40, 40),
        },
    ));
    assert_eq!(app.tab().history.undo_stack.len(), 1);
    assert!(app.tab().history.is_dirty());

    assert!(app.tab_mut().undo());
    assert_eq!(app.tab().history.undo_stack.len(), 0);
    assert_eq!(app.tab().history.redo_stack.len(), 1);
    assert!(!app.tab().history.is_dirty());

    // 空のコマで F7 を押しても false が返り、Undo=0・未保存マークなし・Redo=1 が保たれること
    assert!(!apply(
        &mut app,
        Action::InsertKeyframe { copy_prev: false }
    ));
    assert!(app.tab().history.pending.is_none());
    assert_eq!(app.tab().history.undo_stack.len(), 0);
    assert_eq!(app.tab().history.redo_stack.len(), 1);
    assert!(!app.tab().history.is_dirty());

    // Redo で元の線が戻ること
    assert!(app.tab_mut().redo());
    assert_eq!(app.tab().doc.root.active_cel_shapes().len(), 1);
}

#[test]
fn test_m5_1_f7_twice_on_drawn_frame_second_is_noop() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let c = app.tab().stage_center_world();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![c, Point::new(c.x + 80.0, c.y)],
            radius: 6.0,
            color: Color32::from_rgb(40, 120, 220),
        },
    ));
    // 1回目の F7 で次のコマに空のキーフレームが作られる
    assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: false }));
    assert_eq!(app.tab().doc.root.current_frame, 1);
    assert!(app.tab().doc.root.active_cel_shapes().is_empty());

    let undo_after_first = app.tab().history.undo_stack.len();
    app.tab_mut().history.mark_saved();
    assert!(!app.tab().history.is_dirty());

    // 2回目の F7 はすでに空なので false を返し、Undo 件数も未保存マークも変わらないこと
    assert!(!apply(
        &mut app,
        Action::InsertKeyframe { copy_prev: false }
    ));
    assert!(app.tab().history.pending.is_none());
    assert_eq!(app.tab().history.undo_stack.len(), undo_after_first);
    assert!(!app.tab().history.is_dirty());
}

#[test]
fn test_m5_1_n3001_f8_nest_limit_preserves_redo_and_oldest_undo() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let c = app.tab().stage_center_world();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![c, Point::new(c.x + 60.0, c.y)],
            radius: 6.0,
            color: Color32::from_rgb(30, 180, 90),
        },
    ));
    assert!(apply(&mut app, Action::SelectSingleAt { pt: c }));
    // F8 を 8 回（8段入れ子）
    for _ in 0..8 {
        assert!(apply(&mut app, Action::ConvertToSymbol));
    }
    assert_eq!(app.tab().doc.library.len(), 8);

    // 別の線を描いて Undo（Redo 1件）
    let p2 = Point::new(c.x, c.y + 120.0);
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![p2, Point::new(p2.x + 60.0, p2.y)],
            radius: 6.0,
            color: Color32::from_rgb(200, 80, 30),
        },
    ));
    assert!(app.tab_mut().undo());
    assert_eq!(app.tab().history.redo_stack.len(), 1);
    let undo_before_reject = app.tab().history.undo_stack.len();

    // 一番外のインスタンスを選んで F8 → 9段目は拒否され、Redo 1件が残り、Redo で線が戻ること
    assert!(apply(&mut app, Action::SelectSingleAt { pt: c }));
    assert!(!apply(&mut app, Action::ConvertToSymbol));
    assert!(app.tab().history.pending.is_none());
    assert_eq!(app.tab().doc.library.len(), 8);
    assert_eq!(app.tab().history.undo_stack.len(), undo_before_reject);
    assert_eq!(app.tab().history.redo_stack.len(), 1);
    assert!(app.tab_mut().redo());
    assert_eq!(app.tab().doc.root.active_cel_shapes().len(), 1);

    // Undo が上限 100 件いっぱいでも、F8 拒否時に一番古い履歴が消えないこと
    while app.tab().history.undo_stack.len() < 100 {
        assert!(apply(&mut app, Action::ExtendFrame));
    }
    assert_eq!(app.tab().history.undo_stack.len(), 100);
    let oldest_commit_id = app.tab().history.undo_stack.front().unwrap().commit_id;
    let newest_commit_id = app.tab().history.undo_stack.back().unwrap().commit_id;

    assert!(apply(&mut app, Action::SelectSingleAt { pt: c }));
    assert!(!apply(&mut app, Action::ConvertToSymbol));
    assert_eq!(app.tab().history.undo_stack.len(), 100);
    assert_eq!(
        app.tab().history.undo_stack.front().unwrap().commit_id,
        oldest_commit_id
    );
    assert_eq!(
        app.tab().history.undo_stack.back().unwrap().commit_id,
        newest_commit_id
    );
}

#[test]
fn test_m5_1_deform_shape_without_editable_target_preserves_undo_and_clean_state() {
    use crate::doc_actions::{apply, Action};
    use kurbo::QuadBez;

    let mut app = Is2dApp::default();
    let c = app.tab().stage_center_world();
    let dummy_seg = QuadBez::new(c, Point::new(c.x + 20.0, c.y), Point::new(c.x + 40.0, c.y));

    // 図形が存在しない状態で ReshapeEdge / MoveVertex を呼んでも Undo や未保存マークが増えないこと
    assert!(!apply(
        &mut app,
        Action::ReshapeEdge {
            shape_id: 999_999,
            contour_idx: 0,
            seg_idx: 0,
            grab_t: 0.5,
            grab_pos: c,
            target_pos: Point::new(c.x, c.y + 30.0),
            orig_seg: dummy_seg,
        },
    ));
    assert!(!apply(
        &mut app,
        Action::MoveVertex {
            shape_id: 999_999,
            contour_idx: 0,
            seg_idx: 0,
            orig_vertex: c,
            target_pos: Point::new(c.x, c.y + 30.0),
            orig_ctrls: (c, c),
        },
    ));
    assert_eq!(app.tab().history.undo_stack.len(), 0);
    assert!(!app.tab().history.is_dirty());

    // ロックされたレイヤー上の図形に対して呼んでも Undo や未保存マークが増えないこと
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![c, Point::new(c.x + 60.0, c.y)],
            radius: 6.0,
            color: Color32::from_rgb(50, 50, 200),
        },
    ));
    let shape_id = app.tab().doc.root.active_cel_shapes()[0].shape_id;
    assert!(apply(
        &mut app,
        Action::SetLayerLocked {
            layer_idx: 0,
            locked: true,
        },
    ));
    app.tab_mut().history.mark_saved();
    let undo_len = app.tab().history.undo_stack.len();

    assert!(!apply(
        &mut app,
        Action::ReshapeEdge {
            shape_id,
            contour_idx: 0,
            seg_idx: 0,
            grab_t: 0.5,
            grab_pos: c,
            target_pos: Point::new(c.x, c.y + 30.0),
            orig_seg: dummy_seg,
        },
    ));
    assert_eq!(app.tab().history.undo_stack.len(), undo_len);
    assert!(!app.tab().history.is_dirty());
}

#[test]
fn test_m5_1_random_actions_undo_and_dirty_invariants_3_seeds() {
    use crate::doc_actions::{apply, Action};
    use crate::project_io::ProjectDocument;
    use crate::select_ops::ArrangeMode;
    use crate::timeline::TimelineCellSelection;
    use std::collections::BTreeMap;

    struct Rng(u64);
    impl Rng {
        fn next_u32(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 32) as u32
        }
        fn range(&mut self, max: usize) -> usize {
            if max == 0 {
                0
            } else {
                (self.next_u32() as usize) % max
            }
        }
        fn f64_range(&mut self, lo: f64, hi: f64) -> f64 {
            let t = (self.next_u32() as f64) / (u32::MAX as f64);
            lo + (hi - lo) * t
        }
    }

    let doc_json = |app: &Is2dApp| -> String {
        serde_json::to_string(&ProjectDocument::from_app(app.tab())).unwrap()
    };
    let doc_json_ignoring_active_layer = |app: &Is2dApp| -> String {
        let mut pd = ProjectDocument::from_app(app.tab());
        pd.timeline.active_layer = 0;
        pd.timeline.current_frame = 0;
        for sym in &mut pd.library {
            sym.timeline.active_layer = 0;
            sym.timeline.current_frame = 0;
        }
        serde_json::to_string(&pd).unwrap()
    };

    let mut true_without_json_change: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut total_true_by_op: BTreeMap<&'static str, usize> = BTreeMap::new();

    for &seed in &[
        0x1234_5678_9abc_def0u64,
        0xfedc_ba98_7654_3210,
        0x2026_1103_cafe_babe,
    ] {
        let mut rng = Rng(seed);
        let mut app = Is2dApp::default();
        let c = app.tab().stage_center_world();

        for step in 0..1050 {
            let op_idx = rng.range(26);
            if op_idx == 24 {
                let before_undo = app.tab().history.undo_stack.len();
                let before_redo = app.tab().history.redo_stack.len();
                let before_json = doc_json(&app);
                let ok = app.tab_mut().undo();
                assert!(app.tab().history.pending.is_none());
                if !ok {
                    assert_eq!(before_undo, 0);
                    assert_eq!(app.tab().history.redo_stack.len(), before_redo);
                    assert_eq!(doc_json(&app), before_json);
                } else {
                    assert_eq!(app.tab().history.undo_stack.len() + 1, before_undo);
                    assert_eq!(app.tab().history.redo_stack.len(), before_redo + 1);
                }
                continue;
            }
            if op_idx == 25 {
                let before_undo = app.tab().history.undo_stack.len();
                let before_redo = app.tab().history.redo_stack.len();
                let before_json = doc_json(&app);
                let ok = app.tab_mut().redo();
                assert!(app.tab().history.pending.is_none());
                if !ok {
                    assert_eq!(before_redo, 0);
                    assert_eq!(app.tab().history.undo_stack.len(), before_undo);
                    assert_eq!(doc_json(&app), before_json);
                } else {
                    assert_eq!(app.tab().history.undo_stack.len(), before_undo + 1);
                    assert_eq!(app.tab().history.redo_stack.len() + 1, before_redo);
                }
                continue;
            }

            // ペーストのときは事前に選択図形があれば Copy しておく
            if op_idx == 13 && !app.tab().session.selection.is_empty() {
                let _ = apply(&mut app, Action::Copy);
            }

            let (op_name, action, is_navigation_or_selection): (&'static str, Action, bool) =
                match op_idx {
                    0 => {
                        let x0 = rng.f64_range(c.x - 120.0, c.x + 120.0);
                        let y0 = rng.f64_range(c.y - 90.0, c.y + 90.0);
                        let dx = rng.f64_range(20.0, 70.0);
                        let dy = rng.f64_range(-40.0, 40.0);
                        let colors = [
                            Color32::from_rgb(220, 50, 50),
                            Color32::from_rgb(50, 160, 80),
                            Color32::from_rgb(50, 100, 220),
                        ];
                        (
                            "PaintStroke",
                            Action::PaintStroke {
                                points: vec![Point::new(x0, y0), Point::new(x0 + dx, y0 + dy)],
                                radius: 6.0,
                                color: colors[rng.range(colors.len())],
                            },
                            false,
                        )
                    }
                    1 => {
                        let x0 = rng.f64_range(c.x - 100.0, c.x + 100.0);
                        let y0 = rng.f64_range(c.y - 80.0, c.y + 80.0);
                        (
                            "EraseStroke",
                            Action::EraseStroke {
                                points: vec![Point::new(x0, y0), Point::new(x0 + 50.0, y0 + 20.0)],
                                radius: 8.0,
                            },
                            false,
                        )
                    }
                    2 => {
                        let pt = Point::new(
                            rng.f64_range(c.x - 100.0, c.x + 100.0),
                            rng.f64_range(c.y - 80.0, c.y + 80.0),
                        );
                        (
                            "BucketFill",
                            Action::BucketFill {
                                pt,
                                color: Color32::from_rgb(240, 180, 40),
                            },
                            false,
                        )
                    }
                    3 => {
                        if rng.range(2) == 0 {
                            let pt = if let Some(s) = app.tab().scene().iter().next() {
                                let b = s.bounding_box();
                                Point::new((b.x0 + b.x1) * 0.5, (b.y0 + b.y1) * 0.5)
                            } else {
                                Point::new(
                                    rng.f64_range(c.x - 100.0, c.x + 100.0),
                                    rng.f64_range(c.y - 80.0, c.y + 80.0),
                                )
                            };
                            ("SelectSingleAt", Action::SelectSingleAt { pt }, true)
                        } else {
                            let x0 = rng.f64_range(c.x - 140.0, c.x + 20.0);
                            let y0 = rng.f64_range(c.y - 100.0, c.y + 20.0);
                            let w = rng.f64_range(30.0, 160.0);
                            let h = rng.f64_range(30.0, 120.0);
                            (
                                "SelectMarquee",
                                Action::SelectMarquee {
                                    p0: Point::new(x0, y0),
                                    p1: Point::new(x0 + w, y0 + h),
                                    additive: false,
                                },
                                true,
                            )
                        }
                    }
                    4 => (
                        "MoveSelected",
                        Action::MoveSelected {
                            delta: KVec2::new(
                                rng.f64_range(-20.0, 20.0),
                                rng.f64_range(-20.0, 20.0),
                            ),
                        },
                        false,
                    ),
                    5 => (
                        "AltDuplicate",
                        Action::FinishMoveSelectionDrag {
                            total_delta: KVec2::new(15.0, 12.0),
                            alt_duplicate: true,
                        },
                        false,
                    ),
                    6 => ("DeleteSelection", Action::DeleteSelection, false),
                    7 => ("Group", Action::Group, false),
                    8 => ("Ungroup", Action::Ungroup, false),
                    9 => {
                        let modes = [
                            ArrangeMode::ToFront,
                            ArrangeMode::Forward,
                            ArrangeMode::Backward,
                            ArrangeMode::ToBack,
                        ];
                        (
                            "Arrange",
                            Action::Arrange {
                                mode: modes[rng.range(4)],
                            },
                            false,
                        )
                    }
                    10 => {
                        let (fx, fy) = match rng.range(3) {
                            0 => (true, false),
                            1 => (false, true),
                            _ => (true, true),
                        };
                        (
                            "FlipSelection",
                            Action::FlipSelection {
                                flip_x: fx,
                                flip_y: fy,
                            },
                            false,
                        )
                    }
                    11 => (
                        "RotateSelection",
                        Action::RotateSelection { angle_rad: 0.35 },
                        false,
                    ),
                    12 => ("ConvertToSymbol", Action::ConvertToSymbol, false),
                    13 => (
                        "Paste",
                        Action::Paste {
                            in_place: rng.range(2) == 0,
                        },
                        false,
                    ),
                    14 => {
                        let sym_id = app
                            .tab()
                            .doc
                            .library
                            .get(rng.range(app.tab().doc.library.len().max(1)))
                            .map(|s| s.id)
                            .unwrap_or(999);
                        ("PlaceSymbol", Action::PlaceSymbol { sym_id }, false)
                    }
                    15 => {
                        if !app.tab().session.edit_path.is_empty()
                            || !app.tab().session.editing_group_path.is_empty()
                        {
                            ("ExitGroup", Action::ExitGroupSteps { steps: 1 }, true)
                        } else if let Some(sym) = app
                            .tab()
                            .doc
                            .library
                            .get(rng.range(app.tab().doc.library.len().max(1)))
                        {
                            (
                                "EnterSymbolFromLibrary",
                                Action::EnterSymbolFromLibrary { sym_id: sym.id },
                                true,
                            )
                        } else {
                            ("EnterGroupAt", Action::EnterGroupAt { pt: c }, true)
                        }
                    }
                    16 => ("ExtendFrame(F5)", Action::ExtendFrame, false),
                    17 => (
                        "InsertKeyframe(F6)",
                        Action::InsertKeyframe { copy_prev: true },
                        false,
                    ),
                    18 => (
                        "InsertBlankKeyframe(F7)",
                        Action::InsertKeyframe { copy_prev: false },
                        false,
                    ),
                    19 => ("RemoveFrame(Shift+F5)", Action::RemoveFrame, false),
                    20 => ("DeleteKeyframe(Shift+F6)", Action::DeleteKeyframe, false),
                    21 => {
                        if app.tab().active_timeline().layers.len() < 4 {
                            ("AddLayer", Action::AddLayer, false)
                        } else {
                            ("DeleteActiveLayer", Action::DeleteActiveLayer, false)
                        }
                    }
                    22 => ("ToggleTween", Action::ToggleTween, false),
                    _ => {
                        let tl = app.tab().active_timeline();
                        let sel = tl.cell_selection.unwrap_or(TimelineCellSelection::single(
                            tl.active_layer,
                            tl.current_frame,
                        ));
                        let df = if rng.range(2) == 0 { 1 } else { -1 };
                        (
                            "MoveOrCopyFrames",
                            Action::MoveOrCopyFrames {
                                sel,
                                layer_delta: 0,
                                frame_delta: df,
                                copy_mode: rng.range(2) == 0,
                            },
                            false,
                        )
                    }
                };

            let before_json = doc_json(&app);
            let before_no_act_layer_json = doc_json_ignoring_active_layer(&app);
            let before_undo = app.tab().history.undo_stack.len();
            let before_redo = app.tab().history.redo_stack.len();
            let before_dirty = app.tab().history.is_dirty();
            let coalesce_key = action.coalesce_key(app.tab());
            let was_coalesced =
                coalesce_key.is_some() && app.tab().history.last_coalesce_key == coalesce_key;

            let res = apply(&mut app, action);

            // 3. apply のあと、pending は必ず None
            assert!(
                app.tab().history.pending.is_none(),
                "seed={seed:#x} step={step} op={op_name}: pending must be None after apply"
            );

            let after_json = doc_json(&app);
            let after_no_act_layer_json = doc_json_ignoring_active_layer(&app);
            let after_undo = app.tab().history.undo_stack.len();
            let after_redo = app.tab().history.redo_stack.len();
            let after_dirty = app.tab().history.is_dirty();

            // 1. apply が false を返したときは、前後の ProjectDocument::from_app の JSON も、Undo/Redo の件数も、is_dirty() も変わらない
            if !res {
                assert_eq!(
                    after_json, before_json,
                    "seed={seed:#x} step={step} op={op_name}: JSON changed when apply returned false"
                );
                assert_eq!(
                    after_undo, before_undo,
                    "seed={seed:#x} step={step} op={op_name}: undo count changed when apply returned false"
                );
                assert_eq!(
                    after_redo, before_redo,
                    "seed={seed:#x} step={step} op={op_name}: redo count changed when apply returned false"
                );
                assert_eq!(
                    after_dirty, before_dirty,
                    "seed={seed:#x} step={step} op={op_name}: is_dirty changed when apply returned false"
                );
                continue;
            }

            if is_navigation_or_selection {
                assert_eq!(
                    after_no_act_layer_json, before_no_act_layer_json,
                    "seed={seed:#x} step={step} op={op_name}: navigation/selection changed document content"
                );
                assert_eq!(after_undo, before_undo);
                assert_eq!(after_redo, before_redo);
                assert_eq!(after_dirty, before_dirty);
            } else {
                *total_true_by_op.entry(op_name).or_insert(0) += 1;
                let expected_undo = if was_coalesced {
                    before_undo
                } else {
                    (before_undo + 1).min(100)
                };
                // 2. ProjectDocument::from_app の JSON が変わったときは、Undo が 1 件増え（連続操作のまとめ中は 0 件）、is_dirty() が true になる
                if after_json != before_json {
                    assert_eq!(
                        after_undo, expected_undo,
                        "seed={seed:#x} step={step} op={op_name}: undo count did not increase as expected on JSON change"
                    );
                    assert!(
                        after_dirty,
                        "seed={seed:#x} step={step} op={op_name}: is_dirty must be true on JSON change"
                    );
                } else if after_undo == expected_undo {
                    *true_without_json_change.entry(op_name).or_insert(0) += 1;
                }
            }
        }
    }

    println!("=== M5-1 Random Test Summary (3 seeds x 1050 steps) ===");
    for (op, total_true) in &total_true_by_op {
        let no_json = true_without_json_change.get(op).copied().unwrap_or(0);
        println!("  {op:<26}: true={total_true:>4}, true_without_json_change={no_json:>4}");
    }
    assert_eq!(
        true_without_json_change
            .get("InsertBlankKeyframe(F7)")
            .copied()
            .unwrap_or(0),
        0,
        "F7 must never return true and push Undo without changing ProjectDocument JSON"
    );
}

#[test]
fn test_m5_2_n1001_contour_segments_capacity_equals_len_after_merge_insert_and_reload() {
    fn assert_all_contours_shrunk(doc: &symbol::Document, phase: &str) {
        let mut count = 0usize;
        for layer in &doc.root.layers {
            for kf in &layer.keyframes {
                for sh in kf.cel.shapes.iter() {
                    for c in sh.contours() {
                        count += 1;
                        assert_eq!(
                            c.segments.capacity(),
                            c.segments.len(),
                            "{phase}: root contour capacity ({}) != len ({})",
                            c.segments.capacity(),
                            c.segments.len()
                        );
                    }
                }
            }
        }
        for sym in &doc.library {
            for layer in &sym.timeline.layers {
                for kf in &layer.keyframes {
                    for sh in kf.cel.shapes.iter() {
                        for c in sh.contours() {
                            count += 1;
                            assert_eq!(
                                c.segments.capacity(),
                                c.segments.len(),
                                "{phase}: symbol contour capacity ({}) != len ({})",
                                c.segments.capacity(),
                                c.segments.len()
                            );
                        }
                    }
                }
            }
        }
        assert!(count > 0, "{phase}: expected at least 1 contour");
    }

    let mut app = Is2dApp::default();
    let color = Color32::from_rgb(220, 60, 70);
    for i in 0..100 {
        let row = (i / 10) as f64;
        let col = (i % 10) as f64;
        let x0 = 60.0 + col * 50.0;
        let y0 = 60.0 + row * 30.0;
        let pts: Vec<Point> = (0..18)
            .map(|k| {
                let t = k as f64 / 17.0;
                Point::new(
                    x0 + t * 80.0,
                    y0 + t * 45.0 + (t * std::f64::consts::TAU).sin() * 6.0,
                )
            })
            .collect();
        doc_actions::apply(
            &mut app,
            doc_actions::Action::PaintStroke {
                points: pts,
                radius: 4.5,
                color,
            },
        );
    }
    assert_all_contours_shrunk(&app.tab().doc, "after 100 overlapping same-color strokes");

    use kurbo::ParamCurve;
    let edge_pt = app
        .scene()
        .get(0)
        .expect("at least one shape")
        .exterior
        .segments[0]
        .eval(0.5);
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::AddAnchor { pt: edge_pt },
    ));
    assert_all_contours_shrunk(&app.tab().doc, "after AddAnchor");

    let saved_json = serde_json::to_vec(&project_io::ProjectDocument::from_app(&app)).unwrap();
    let loaded_doc: project_io::ProjectDocument = serde_json::from_slice(&saved_json).unwrap();
    let mut reopened = Is2dApp::default();
    loaded_doc.apply_to_app(&mut reopened);
    assert_all_contours_shrunk(&reopened.tab().doc, "after save and reopen");
}

#[test]
fn test_transform_move_drag_translates_custom_pivot_once() {
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(10.0, 50.0),
            Point::new(50.0, 50.0),
            8.0,
            Color32::RED,
        ),
    );
    app.tool_mut().tool = Tool::Transform;
    app.session.set_selected_indices(vec![0]);

    let (_, init_pivot) = app
        .session
        .selection_bbox_and_pivot(app.view.zoom)
        .expect("selection bbox and pivot should exist");
    tool_transform::start_transform_drag(&mut app, init_pivot, false, false);
    tool_transform::update_transform_drag(app.tab_mut(), Point::new(20.0, 60.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(Point::new(20.0, 60.0)));
    assert_eq!(app.session.transform_pivot, Some(Point::new(20.0, 60.0)));

    tool_transform::start_transform_drag(&mut app, Point::new(40.0, 50.0), false, false);
    tool_transform::update_transform_drag(app.tab_mut(), Point::new(90.0, 50.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(Point::new(90.0, 50.0)));
    assert_eq!(app.session.transform_pivot, Some(Point::new(70.0, 60.0)));
}

#[test]
fn test_symbol_color_effect_alpha_brightness_tween_export_nested_io_undo_ungroup() {
    use crate::canvas_tool::{CoalesceKey, View};
    use crate::doc_actions::{apply, Action};
    use crate::project_io::{ProjectDocument, SavedSymbolInstanceBinding};
    use crate::symbol::{ColorEffect, SymbolInstanceBinding, SymbolLoopMode};
    use crate::vector_engine::Contour;
    use crate::video_export::{export_dimensions, render_frame};
    use kurbo::QuadBez;

    // 1. ColorEffect::apply
    let sample = Color32::from_rgb(200, 100, 0);
    assert_eq!(ColorEffect::default().apply(sample), sample);
    assert_eq!(
        ColorEffect::new(0.5, 0.0)
            .apply(Color32::from_rgb(255, 0, 0))
            .to_srgba_unmultiplied(),
        [255, 0, 0, 128]
    );
    assert_eq!(
        ColorEffect::new(1.0, 1.0).apply(sample),
        Color32::from_rgb(255, 255, 255)
    );
    assert_eq!(
        ColorEffect::new(1.0, -1.0).apply(sample),
        Color32::from_rgb(0, 0, 0)
    );
    assert_eq!(
        ColorEffect::new(1.0, 0.5).apply(sample),
        Color32::from_rgb(228, 178, 128)
    );
    assert_eq!(
        ColorEffect::new(1.0, -0.5).apply(sample),
        Color32::from_rgb(100, 50, 0)
    );

    // 2. ColorEffect::new
    assert_eq!(
        ColorEffect::new(2.0, -3.0),
        ColorEffect {
            alpha: 1.0,
            brightness: -1.0,
        }
    );
    assert_eq!(
        ColorEffect::new(f32::NAN, f32::INFINITY),
        ColorEffect::default()
    );
    assert_eq!(
        ColorEffect::new(f32::NEG_INFINITY, f32::NAN),
        ColorEffect::default()
    );

    // 3. トゥイーン（開始 0% → 4コマ後 100% の 2コマ目でアルファ 128±1）
    let mut tween_app = Is2dApp::default();
    let center = View::DEFAULT_STAGE_CENTER;
    put_cel_shapes(
        &mut tween_app,
        0,
        0,
        make_stroke(
            Point::new(center.x - 40.0, center.y),
            Point::new(center.x + 40.0, center.y),
            12.0,
            Color32::from_rgb(255, 0, 0),
        ),
    );
    tween_app.session.set_selected_indices(vec![0]);
    assert!(apply(&mut tween_app, Action::ConvertToSymbol));
    assert!(apply(
        &mut tween_app,
        Action::SetSymbolColorEffect {
            effect: ColorEffect::new(0.0, 0.0),
        },
    ));
    assert!(apply(&mut tween_app, Action::ToggleTween));
    for _ in 0..4 {
        assert!(apply(&mut tween_app, Action::ExtendFrame));
    }
    apply(
        &mut tween_app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 4,
        },
    );
    assert!(apply(
        &mut tween_app,
        Action::InsertKeyframe { copy_prev: true },
    ));
    tween_app.session.set_selected_indices(vec![0]);
    assert!(apply(
        &mut tween_app,
        Action::SetSymbolColorEffect {
            effect: ColorEffect::new(1.0, 0.0),
        },
    ));
    let mid_shapes = tween_app
        .doc
        .root
        .evaluate_layer_shapes_at(0, 2, &tween_app.doc.library);
    assert_eq!(mid_shapes.len(), 1);
    let mid_alpha = mid_shapes[0].color.to_srgba_unmultiplied()[3] as i32;
    assert!(
        (mid_alpha - 128).abs() <= 1,
        "mid_alpha expected ~128, got {mid_alpha}"
    );

    // 4. 画面と書き出しが同じ（アルファ 50% の赤 → scene の A=128±1、render_frame の画素が約 (255, 128, 128)）
    let mut export_app = Is2dApp::default();
    let p00 = Point::new(center.x - 50.0, center.y - 50.0);
    let p10 = Point::new(center.x + 50.0, center.y - 50.0);
    let p11 = Point::new(center.x + 50.0, center.y + 50.0);
    let p01 = Point::new(center.x - 50.0, center.y + 50.0);
    let red_box = FillShape::new(
        Color32::from_rgb(255, 0, 0),
        Contour::new(vec![
            QuadBez::new(p00, p00.midpoint(p10), p10),
            QuadBez::new(p10, p10.midpoint(p11), p11),
            QuadBez::new(p11, p11.midpoint(p01), p01),
            QuadBez::new(p01, p01.midpoint(p00), p00),
        ]),
        Vec::new(),
    );
    put_cel_shapes(&mut export_app, 0, 0, vec![red_box]);
    export_app.session.set_selected_indices(vec![0]);
    assert!(apply(&mut export_app, Action::ConvertToSymbol));
    assert!(apply(
        &mut export_app,
        Action::SetSymbolColorEffect {
            effect: ColorEffect::new(0.5, 0.0),
        },
    ));
    let scene_a = export_app
        .scene()
        .get(0)
        .unwrap()
        .color
        .to_srgba_unmultiplied()[3] as i32;
    assert!((scene_a - 128).abs() <= 1);
    let (w, h) = export_dimensions(export_app.doc.stage_size);
    let rgba = render_frame(&export_app.doc, 0, w, h);
    let cx_idx = (((h / 2) as usize) * (w as usize) + ((w / 2) as usize)) * 4;
    let px = [
        rgba[cx_idx],
        rgba[cx_idx + 1],
        rgba[cx_idx + 2],
        rgba[cx_idx + 3],
    ];
    assert_eq!(px[0], 255);
    assert!(
        (px[1] as i32 - 128).abs() <= 1,
        "expected g ~128, got {}",
        px[1]
    );
    assert!(
        (px[2] as i32 - 128).abs() <= 1,
        "expected b ~128, got {}",
        px[2]
    );
    assert_eq!(px[3], 255);

    // 5. 入れ子（内側 50%、外側 50% → アルファ 64±1）
    export_app.session.set_selected_indices(vec![0]);
    assert!(apply(&mut export_app, Action::ConvertToSymbol));
    assert!(apply(
        &mut export_app,
        Action::SetSymbolColorEffect {
            effect: ColorEffect::new(0.5, 0.0),
        },
    ));
    let nested_a = export_app
        .scene()
        .get(0)
        .unwrap()
        .color
        .to_srgba_unmultiplied()[3] as i32;
    assert!(
        (nested_a - 64).abs() <= 1,
        "nested alpha expected ~64, got {nested_a}"
    );

    // 6. 保存と読み込み（値の保持、欄がない古い JSON、範囲外・NaN の正規化）
    assert!(apply(
        &mut export_app,
        Action::SetSymbolColorEffect {
            effect: ColorEffect::new(0.35, -0.25),
        },
    ));
    let saved_str = serde_json::to_string(&ProjectDocument::from_app(&export_app)).unwrap();
    let loaded_doc: ProjectDocument = serde_json::from_str(&saved_str).unwrap();
    let mut reopened = Is2dApp::default();
    loaded_doc.apply_to_app(&mut reopened);
    let reopened_eff = reopened.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .color_effect;
    assert_eq!(reopened_eff, ColorEffect::new(0.35, -0.25));

    let legacy_binding_json =
        r#"{"instance_gid":1,"symbol_id":1,"xform":[1.0,0.0,0.0,1.0,0.0,0.0]}"#;
    let legacy_saved: SavedSymbolInstanceBinding =
        serde_json::from_str(legacy_binding_json).unwrap();
    let legacy_binding = SymbolInstanceBinding::from(legacy_saved);
    assert_eq!(legacy_binding.color_effect, ColorEffect::default());

    let out_of_range_saved = SavedSymbolInstanceBinding {
        instance_gid: 1,
        symbol_id: 1,
        xform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        pivot: [0.0, 0.0],
        loop_mode: SymbolLoopMode::Loop,
        first_frame: 0,
        color_effect: ColorEffect {
            alpha: 5.0,
            brightness: f32::NAN,
        },
    };
    let sanitized_binding = SymbolInstanceBinding::from(out_of_range_saved);
    assert_eq!(
        sanitized_binding.color_effect,
        ColorEffect {
            alpha: 1.0,
            brightness: 0.0,
        }
    );

    // 7. 取り消し（SetSymbolColorEffect を3回続けて送り EndCoalesce → Undo 1回で元の値に戻る）
    let mut undo_app = Is2dApp::default();
    put_cel_shapes(
        &mut undo_app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            10.0,
            Color32::from_rgb(200, 50, 50),
        ),
    );
    undo_app.session.set_selected_indices(vec![0]);
    assert!(apply(&mut undo_app, Action::ConvertToSymbol));
    let inst_gid = undo_app
        .scene()
        .get(0)
        .unwrap()
        .symbol_instance
        .unwrap()
        .instance_gid;
    let undo_before = undo_app.history.undo_stack.len();
    for a in [0.8, 0.6, 0.5] {
        assert!(apply(
            &mut undo_app,
            Action::SetSymbolColorEffect {
                effect: ColorEffect::new(a, 0.2),
            },
        ));
    }
    apply(
        &mut undo_app,
        Action::EndCoalesce {
            key: Some(CoalesceKey::SymbolColorEffect(inst_gid)),
        },
    );
    assert_eq!(undo_app.history.undo_stack.len(), undo_before + 1);
    assert!(apply(&mut undo_app, Action::Undo));
    assert_eq!(
        undo_app
            .scene()
            .get(0)
            .unwrap()
            .symbol_instance
            .unwrap()
            .color_effect,
        ColorEffect::default()
    );

    // 8. グループ解除（アルファ 50% のインスタンスを解除すると symbol_instance が無くなり効果をかける前の A=255 の生図形になり、Undo 1回で戻る）
    assert!(apply(&mut undo_app, Action::Redo));
    assert!(apply(
        &mut undo_app,
        Action::SetSymbolColorEffect {
            effect: ColorEffect::new(0.5, 0.0),
        },
    ));
    assert!(apply(&mut undo_app, Action::Ungroup));
    let ungrouped_shape = undo_app.scene().get(0).unwrap();
    assert!(ungrouped_shape.symbol_instance.is_none());
    assert_eq!(ungrouped_shape.color.to_srgba_unmultiplied()[3], 255);
    assert!(apply(&mut undo_app, Action::Undo));
    assert_eq!(
        undo_app
            .scene()
            .get(0)
            .unwrap()
            .symbol_instance
            .unwrap()
            .color_effect,
        ColorEffect::new(0.5, 0.0)
    );
}

#[test]
fn test_recovery_write_find_restore_discard_and_sanitization() {
    use eframe::App as _;

    let base_dir = std::env::temp_dir().join(format!("is2d_recovery_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base_dir);
    let proj_dir = base_dir.join("projects");
    let rec_dir = base_dir.join("recovery");
    std::fs::create_dir_all(&proj_dir).unwrap();

    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 90, 230);
    let green = Color32::from_rgb(40, 180, 90);

    // 11. Is2dApp::default() の復旧の仕組みは、何も書かない（dir が None）
    let mut default_app = Is2dApp::default();
    assert!(default_app.recovery.dir.is_none());
    assert!(apply(
        &mut default_app,
        Action::PaintStroke {
            points: vec![Point::new(20.0, 20.0), Point::new(80.0, 20.0)],
            radius: 6.0,
            color: red,
        },
    ));
    default_app.on_exit(None);
    assert!(!rec_dir.exists());

    // 1 & 10. 元のファイルがある未保存のタブ、無題の未保存のタブ、保存済みのタブを用意
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    app.recovery.dir = Some(rec_dir.clone());

    // タブ0: 保存したあと編集して未保存にしたタブ（赤1個 + 青1個 = 2個）
    let saved_proj_path = proj_dir.join("work_a.is2d");
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(30.0, 30.0), Point::new(90.0, 30.0)],
            radius: 6.0,
            color: red,
        },
    ));
    crate::project_io::save_project_to_path(&mut app, &saved_proj_path).unwrap();
    let normal_json = std::fs::read_to_string(&saved_proj_path).unwrap();
    assert!(
        !normal_json.contains("recovery_source"),
        "普通に保存したファイルには recovery_source が入らないこと"
    );
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(30.0, 100.0), Point::new(90.0, 100.0)],
            radius: 6.0,
            color: blue,
        },
    ));
    assert!(app.tabs[0].history.is_dirty());
    let tab0_shape_count = app.tabs[0].scene().len();

    // タブ1: 無題の未保存のタブ（緑1個）
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(50.0, 50.0), Point::new(120.0, 50.0)],
            radius: 6.0,
            color: green,
        },
    ));
    assert!(app.tabs[1].file.is_none());
    assert!(app.tabs[1].history.is_dirty());
    let tab1_id = app.tabs[1].id;

    // タブ2: 保存済みのタブ
    let clean_proj_path = proj_dir.join("clean_b.is2d");
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(60.0, 60.0), Point::new(120.0, 60.0)],
            radius: 6.0,
            color: red,
        },
    ));
    crate::project_io::save_project_to_path(&mut app, &clean_proj_path).unwrap();
    assert!(!app.tabs[2].history.is_dirty());

    // 2. allow_viewport_close が true のときは on_exit で何も書かれない
    app.allow_viewport_close = true;
    app.on_exit(None);
    assert!(!proj_dir.join("RECOVER_work_a.is2d").exists());
    assert!(!rec_dir.exists());

    // 1. allow_viewport_close が false のとき、保存したことのある未保存タブ（タブ0）だけ書かれ、無題の未保存タブ（タブ1）は書かれない
    app.allow_viewport_close = false;
    app.on_exit(None);
    let rec_work_a = proj_dir.join("RECOVER_work_a.is2d");
    let rec_untitled = rec_dir.join(format!(
        "RECOVER_無題-{}-{tab1_id}.is2d",
        std::process::id()
    ));
    assert!(rec_work_a.is_file());
    assert!(!rec_untitled.exists());
    assert!(!proj_dir.join("RECOVER_clean_b.is2d").exists());

    let entry_files: Vec<_> = std::fs::read_dir(&rec_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("entry"))
        .collect();
    assert_eq!(entry_files.len(), 1);

    // 7. 指している復旧ファイルが無い一覧と .tmp ファイルは find_leftovers で黙って消える
    let stale_entry = rec_dir.join("stale-999.entry");
    std::fs::write(
        &stale_entry,
        proj_dir
            .join("RECOVER_missing.is2d")
            .to_string_lossy()
            .as_bytes(),
    )
    .unwrap();
    let leftover_tmp = rec_dir.join(".partial.123.tmp");
    std::fs::write(&leftover_tmp, b"partial").unwrap();

    // 4. 起動したときに探す: 新しい Is2dApp から find_leftovers を呼ぶと found に1件入り、まだタブは開かない
    let mut next_app = Is2dApp::default();
    next_app.recovery.dir = Some(rec_dir.clone());
    next_app.recovery.find_leftovers();
    assert!(!stale_entry.exists());
    assert!(!leftover_tmp.exists());
    assert_eq!(next_app.recovery.found.len(), 1);
    assert_eq!(next_app.tabs.len(), 1);
    assert!(crate::project_io::is_active_tab_pristine_empty(
        &next_app.tabs[0]
    ));

    // 5. 開く: 元のファイルがある作品だけが未保存のタブとして開き（最初の空の無題タブが使い回されてタブは1つ）、復旧ファイルと一覧が消える
    next_app.apply_action(UiAction::RestoreRecovered, &ctx);
    assert!(next_app.recovery.found.is_empty());
    assert_eq!(next_app.tabs.len(), 1);
    assert!(next_app.tabs[0].history.is_dirty());
    assert_eq!(
        next_app.tabs[0].file.as_deref(),
        Some(saved_proj_path.as_path())
    );
    assert_eq!(next_app.tabs[0].scene().len(), tab0_shape_count);
    assert_eq!(next_app.tabs[0].scene().get(0).unwrap().color, red);
    assert_eq!(next_app.tabs[0].scene().get(1).unwrap().color, blue);
    assert!(!rec_work_a.exists());
    assert_eq!(
        std::fs::read_dir(&rec_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("entry"))
            .count(),
        0
    );

    // 6. 破棄: 復旧ファイルと一覧が全部消え、タブは開かない
    next_app.recovery.write_unsaved_tabs(&next_app.tabs);
    assert!(rec_work_a.is_file());
    let mut discard_app = Is2dApp::default();
    discard_app.recovery.dir = Some(rec_dir.clone());
    discard_app.recovery.find_leftovers();
    assert_eq!(discard_app.recovery.found.len(), 1);
    discard_app.apply_action(UiAction::DiscardRecovered, &ctx);
    assert!(discard_app.recovery.found.is_empty());
    assert_eq!(discard_app.tabs.len(), 1);
    assert!(crate::project_io::is_active_tab_pristine_empty(
        &discard_app.tabs[0]
    ));
    assert!(!rec_work_a.exists());
    assert_eq!(std::fs::read_dir(&rec_dir).unwrap().count(), 0);

    // 3. 元のファイルのフォルダに書けないとき（フォルダを消しておく）はアプリ専用フォルダに書かれ、一覧もそこを指す
    let gone_dir = base_dir.join("gone_folder");
    let gone_proj = gone_dir.join("moved_work.is2d");
    next_app.tabs[0].file = Some(gone_proj.clone());
    next_app.tabs.truncate(1);
    next_app.recovery.write_unsaved_tabs(&next_app.tabs);
    let mut fallback_app = Is2dApp::default();
    fallback_app.recovery.dir = Some(rec_dir.clone());
    fallback_app.recovery.find_leftovers();
    assert_eq!(fallback_app.recovery.found.len(), 1);
    let fallback_rec = fallback_app.recovery.found[0].recovery_path.clone();
    assert_eq!(fallback_rec.parent(), Some(rec_dir.as_path()));
    assert!(fallback_rec.is_file());
    fallback_app.apply_action(UiAction::RestoreRecovered, &ctx);
    assert_eq!(
        fallback_app.tabs[0].file.as_deref(),
        Some(gone_proj.as_path())
    );
    assert!(!fallback_rec.exists());

    // 8. 壊れた復旧ファイル（中身が JSON でない）や recovery_source が無い復旧ファイルは .broken に名前が変わり、エラーが出て、ほかのファイルは開く
    next_app.tabs[0].file = Some(saved_proj_path.clone());
    next_app.recovery.write_unsaved_tabs(&next_app.tabs);
    let bad_rec = rec_dir.join("RECOVER_corrupted.is2d");
    let bad_entry = rec_dir.join("0-9999.entry");
    std::fs::write(&bad_rec, b"not valid json").unwrap();
    std::fs::write(&bad_entry, bad_rec.to_string_lossy().as_bytes()).unwrap();
    let no_src_rec = rec_dir.join("RECOVER_no_source.is2d");
    let no_src_entry = rec_dir.join("0-9998.entry");
    std::fs::copy(&saved_proj_path, &no_src_rec).unwrap();
    std::fs::write(&no_src_entry, no_src_rec.to_string_lossy().as_bytes()).unwrap();

    let mut broken_app = Is2dApp::default();
    broken_app.recovery.dir = Some(rec_dir.clone());
    broken_app.recovery.find_leftovers();
    assert_eq!(broken_app.recovery.found.len(), 3);
    broken_app.apply_action(UiAction::RestoreRecovered, &ctx);
    let broken_renamed = rec_dir.join("RECOVER_corrupted.is2d.broken");
    let no_src_broken = rec_dir.join("RECOVER_no_source.is2d.broken");
    assert!(broken_renamed.is_file());
    assert!(no_src_broken.is_file());
    assert!(!bad_rec.exists());
    assert!(!bad_entry.exists());
    assert!(!no_src_rec.exists());
    assert!(!no_src_entry.exists());
    assert_eq!(broken_app.tabs.len(), 1);
    assert_eq!(
        broken_app.tabs[0].file.as_deref(),
        Some(saved_proj_path.as_path())
    );
    assert_eq!(
        broken_app.toast.as_ref().map(|t| t.kind),
        Some(crate::canvas_tool::NoticeKind::Error)
    );
    assert!(broken_app
        .toast
        .as_ref()
        .is_some_and(|t| t.message.contains("RECOVER_corrupted.is2d.broken")
            && t.message.contains("RECOVER_no_source.is2d.broken")));

    // 9. 壊れた数値: 輪郭に NaN がある図形を1つ含む文書を保存すると成功し、警告一覧に入り、復旧ファイル書き出しも成功する
    let mut nan_app = Is2dApp::default();
    nan_app.recovery.dir = Some(rec_dir.clone());
    let mut shapes = make_stroke(Point::new(20.0, 20.0), Point::new(80.0, 20.0), 6.0, red);
    let mut nan_shape = make_stroke(Point::new(20.0, 60.0), Point::new(80.0, 60.0), 6.0, blue)
        .into_iter()
        .next()
        .unwrap();
    std::sync::Arc::make_mut(&mut nan_shape.exterior.segments)[0]
        .p0
        .x = f64::NAN;
    shapes.push(nan_shape);
    put_cel_shapes(&mut nan_app, 0, 0, shapes);
    nan_app.history.mark_unsaved();

    let nan_save_path = proj_dir.join("nan_saved.is2d");
    let fixed_locs = crate::project_io::save_project_to_path(&mut nan_app, &nan_save_path).unwrap();
    assert_eq!(fixed_locs, vec!["「Layer 1」の 1 コマ目".to_owned()]);
    let nan_json = std::fs::read_to_string(&nan_save_path).unwrap();
    assert!(!nan_json.contains("NaN") && !nan_json.contains("null"));
    let reloaded = crate::project_io::read_project_document(&nan_save_path)
        .unwrap()
        .into_document();
    assert_eq!(reloaded.root.layers[0].keyframes[0].cel.shapes.len(), 1);

    nan_app.history.mark_unsaved();
    nan_app.recovery.write_unsaved_tabs(&nan_app.tabs);
    let nan_rec_path = proj_dir.join("RECOVER_nan_saved.is2d");
    assert!(nan_rec_path.is_file());

    let _ = std::fs::remove_dir_all(&base_dir);
}

#[test]
fn test_nested_symbol_ungroup_preserves_child_instances_frames_and_resets_color_effect() {
    use kurbo::Affine;

    use crate::doc_actions::{apply, Action};
    use crate::symbol::{ColorEffect, SymbolLoopMode};

    let c0 = Color32::from_rgb(220, 40, 40);
    let c1 = Color32::from_rgb(40, 180, 60);
    let c2 = Color32::from_rgb(40, 90, 220);
    let c3 = Color32::from_rgb(220, 180, 40);
    let raw_col = Color32::from_rgb(160, 60, 200);

    // 4コマ（赤・緑・青・黄）のシンボル B と、生の図形1つ＋B を持つシンボル A を作るヘルパー
    let build_nested_app = |a_total_frames: usize| -> (Is2dApp, u64, u64) {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            make_stroke(Point::new(100.0, 100.0), Point::new(160.0, 100.0), 8.0, c0),
        );
        app.session.set_selected_indices(vec![0]);
        assert!(apply(&mut app, Action::ConvertToSymbol));
        let sym_b_id = app.doc.library[0].id;

        // B のタイムラインを 4 コマ（c0, c1, c2, c3）にする
        assert!(apply(
            &mut app,
            Action::EnterSymbolFromLibrary { sym_id: sym_b_id },
        ));
        for (f, col) in [(1, c1), (2, c2), (3, c3)] {
            assert!(apply(
                &mut app,
                Action::SwitchLayerAndFrame {
                    layer_idx: 0,
                    frame: f,
                },
            ));
            assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: false },));
            let s = make_stroke(Point::new(0.0, 0.0), Point::new(50.0, 0.0), 8.0, col);
            let sym_b = app
                .doc
                .library
                .iter_mut()
                .find(|s| s.id == sym_b_id)
                .unwrap();
            sym_b.timeline.layers[0].keyframe_mut(f).cel = timeline::Cel::new(s);
        }
        assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));

        // ステージに生の図形を足し、B と一緒に選んでシンボル A に変換する
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(260.0, 100.0), Point::new(320.0, 100.0)],
                radius: 8.0,
                color: raw_col,
            },
        ));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::ConvertToSymbol));
        let sym_a_id = app.doc.library[1].id;

        if a_total_frames > 1 {
            let sym_a = app
                .doc
                .library
                .iter_mut()
                .find(|s| s.id == sym_a_id)
                .unwrap();
            sym_a.timeline.total_frames = a_total_frames;
            sym_a.timeline.layers[0].frame_count = a_total_frames;
        }
        app.doc.root.total_frames = 20;
        app.doc.root.layers[0].frame_count = 20;
        app.refresh_scene_base();
        (app, sym_a_id, sym_b_id)
    };

    // 3, 13, 14. B が残り、変形が A×B になり、B も選択され、Undo で分解前の A に戻る
    {
        let (mut app, sym_a_id, sym_b_id) = build_nested_app(1);
        let orig_b_in_a = app
            .doc
            .library
            .iter()
            .find(|s| s.id == sym_a_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[0]
            .cel
            .instances[0]
            .binding;
        assert!(apply(
            &mut app,
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(30.0, 15.0),
            },
        ));
        let a_binding = app.doc.root.layers[0].keyframes[0].cel.instances[0].binding;
        let expected_xform = a_binding.xform * orig_b_in_a.xform;

        assert!(apply(&mut app, Action::Ungroup));
        let cel = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel.instances.len(), 1);
        let b_inst = &cel.instances[0];
        assert_eq!(b_inst.binding.symbol_id, sym_b_id);
        for i in 0..6 {
            assert!(
                (b_inst.binding.xform.as_coeffs()[i] - expected_xform.as_coeffs()[i]).abs() < 1e-6
            );
        }
        assert!(app
            .session
            .selection
            .instances
            .contains(&b_inst.binding.instance_gid));
        assert!(!app.session.selection.shapes.is_empty());

        canvas_tool::clear_selection(&mut app);
        let cel_after = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel_after.shapes.len(), 1);
        assert_eq!(cel_after.shapes[0].color, raw_col);

        assert!(apply(&mut app, Action::Undo));
        let cel_undo = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel_undo.instances.len(), 1);
        assert_eq!(cel_undo.instances[0].binding.symbol_id, sym_a_id);
    }

    // 4. 表示コマが合う: (a) A が1コマで F=5、(b) A が4コマループで F=10
    for (a_frames, target_f) in [(1usize, 5usize), (4usize, 10usize)] {
        let (mut app, _, sym_b_id) = build_nested_app(a_frames);
        let sym_a = &mut app.doc.library[1];
        sym_a.timeline.layers[0].keyframe_mut(0).cel.instances_mut()[0]
            .binding
            .first_frame = 1;
        app.refresh_scene_base();

        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: target_f,
            },
        ));
        let color_before_f = app
            .scene()
            .iter()
            .find(|s| s.color != raw_col)
            .unwrap()
            .color;
        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: target_f + 1,
            },
        ));
        let color_before_f_next = app
            .scene()
            .iter()
            .find(|s| s.color != raw_col)
            .unwrap()
            .color;

        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: target_f,
            },
        ));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Ungroup));
        let color_after_f = app
            .scene()
            .iter()
            .find(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == sym_b_id))
            .unwrap()
            .color;
        assert_eq!(color_after_f, color_before_f);

        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: target_f + 1,
            },
        ));
        let color_after_f_next = app
            .scene()
            .iter()
            .find(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == sym_b_id))
            .unwrap()
            .color;
        assert_eq!(color_after_f_next, color_before_f_next);
    }

    // 5. ステージ側のキーフレームが 0 コマ目から始まっていない場合（S=3, F=7）
    {
        let (mut app, _, sym_b_id) = build_nested_app(4);
        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 3,
            },
        ));
        assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: true }));
        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 7,
            },
        ));
        let col_before = app
            .scene()
            .iter()
            .find(|s| s.color != raw_col)
            .unwrap()
            .color;
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Ungroup));
        let col_after = app
            .scene()
            .iter()
            .find(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == sym_b_id))
            .unwrap()
            .color;
        assert_eq!(col_after, col_before);
    }

    // 6 & 7. 1回再生の B（合わせられる場合と負になって 0 になる場合）および任意のフレームの B
    {
        let (mut app, _, _) = build_nested_app(4);
        app.doc.library[1].timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()[0]
            .binding
            .loop_mode = SymbolLoopMode::PlayOnce;
        app.doc.library[1].timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()[0]
            .binding
            .first_frame = 2;
        app.refresh_scene_base();
        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 1,
            },
        ));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Ungroup));
        assert_eq!(
            app.doc.root.layers[0].keyframes[0].cel.instances[0]
                .binding
                .first_frame,
            2
        );

        // B の進み(1) - (F - S)(5) < 0 のとき 1回再生は first_frame が 0 になる
        let (mut app_neg, _, _) = build_nested_app(4);
        app_neg.doc.library[1].timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()[0]
            .binding
            .loop_mode = SymbolLoopMode::PlayOnce;
        app_neg.doc.library[1].timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()[0]
            .binding
            .first_frame = 0;
        app_neg.refresh_scene_base();
        assert!(apply(
            &mut app_neg,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 5,
            },
        ));
        assert!(apply(&mut app_neg, Action::SelectAll));
        assert!(apply(&mut app_neg, Action::Ungroup));
        assert_eq!(
            app_neg.doc.root.layers[0].keyframes[0].cel.instances[0]
                .binding
                .first_frame,
            0
        );

        // 7. 任意のフレームの B は first_frame が変わらない
        let (mut app_sf, _, _) = build_nested_app(4);
        app_sf.doc.library[1].timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()[0]
            .binding
            .loop_mode = SymbolLoopMode::SingleFrame;
        app_sf.doc.library[1].timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()[0]
            .binding
            .first_frame = 2;
        app_sf.refresh_scene_base();
        assert!(apply(
            &mut app_sf,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 6,
            },
        ));
        assert!(apply(&mut app_sf, Action::SelectAll));
        assert!(apply(&mut app_sf, Action::Ungroup));
        assert_eq!(
            app_sf.doc.root.layers[0].keyframes[0].cel.instances[0]
                .binding
                .first_frame,
            2
        );
    }

    // 8. A の中でトゥイーンしている B: トゥイーン途中のコマで分解すると B の位置が分解前と同じ
    {
        let (mut app, sym_a_id, sym_b_id) = build_nested_app(5);
        {
            let sym_a = app
                .doc
                .library
                .iter_mut()
                .find(|s| s.id == sym_a_id)
                .unwrap();
            sym_a.timeline.layers[0].keyframe_mut(0).cel = timeline::Cel::new_with_instances(
                Vec::new(),
                vec![sym_a.timeline.layers[0].keyframes[0].cel.instances[0].clone()],
            );
            sym_a.timeline.layers[0].keyframe_mut(0).tween = true;
            sym_a.timeline.layers[0].insert_keyframe(4, true);
            sym_a.timeline.layers[0].keyframe_mut(1).cel.instances_mut()[0]
                .binding
                .xform *= Affine::translate((80.0, 0.0));
            sym_a.revision = crate::vector_engine::next_mesh_revision();
        }
        app.doc.revision = crate::vector_engine::next_mesh_revision();
        app.refresh_scene_base();
        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 2,
            },
        ));
        let bbox_before = app.scene().get(0).unwrap().bounding_box();
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Ungroup));
        let b_shape = app
            .scene()
            .iter()
            .find(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == sym_b_id))
            .unwrap();
        let bbox_after = b_shape.bounding_box();
        assert!((bbox_before.x0 - bbox_after.x0).abs() < 1e-5);
        assert!((bbox_before.y0 - bbox_after.y0).abs() < 1e-5);
    }

    // 9. 色効果のリセット: A にアルファ 50%・明度 +30%、B にアルファ 30% をかけて分解
    {
        let (mut app, _, sym_b_id) = build_nested_app(1);
        app.doc.library[1].timeline.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()[0]
            .binding
            .color_effect = ColorEffect::new(0.3, 0.0);
        app.doc.library[1].revision = crate::vector_engine::next_mesh_revision();
        app.doc.revision = crate::vector_engine::next_mesh_revision();
        app.refresh_scene_base();

        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(
            &mut app,
            Action::SetSymbolColorEffect {
                effect: ColorEffect::new(0.5, 0.3),
            },
        ));
        assert!(apply(&mut app, Action::Ungroup));

        let raw_shape = app
            .scene()
            .iter()
            .find(|s| s.symbol_instance.is_none())
            .unwrap();
        assert_eq!(raw_shape.color, raw_col);
        assert_eq!(raw_shape.color.to_srgba_unmultiplied()[3], 255);

        let b_binding = app
            .scene()
            .iter()
            .find_map(|s| s.symbol_instance.filter(|b| b.symbol_id == sym_b_id))
            .unwrap();
        assert_eq!(b_binding.color_effect, ColorEffect::new(0.3, 0.0));

        assert!(apply(&mut app, Action::Undo));
        let restored_a = app.scene().get(0).unwrap().symbol_instance.unwrap();
        assert_eq!(restored_a.color_effect, ColorEffect::new(0.5, 0.3));
    }

    // 10. 分解したコマで B が空っぽでも分解後に B が残り、A より手前にあったシンボル C との前後も変わらない（B に絵があるコマで確かめる）
    {
        let (mut app, sym_a_id, sym_b_id) = build_nested_app(1);
        // ステージ上で A の手前に別のシンボル C を置く
        let c_col = Color32::from_rgb(20, 200, 120);
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(400.0, 100.0), Point::new(450.0, 100.0)],
                radius: 8.0,
                color: c_col,
            },
        ));
        let c_stroke_id = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes
            .last()
            .unwrap()
            .shape_id;
        app.session.selection.clear();
        app.session.selection.shapes.insert(c_stroke_id);
        assert!(apply(&mut app, Action::ConvertToSymbol));
        let sym_c_id = app.doc.library.last().unwrap().id;
        assert_ne!(sym_c_id, sym_a_id);
        assert_ne!(sym_c_id, sym_b_id);

        let instances_before = &app.doc.root.layers[0].keyframes[0].cel.instances;
        assert_eq!(instances_before.len(), 2);
        assert_eq!(instances_before[0].binding.symbol_id, sym_a_id);
        assert_eq!(instances_before[1].binding.symbol_id, sym_c_id);
        let a_gid = instances_before[0].binding.instance_gid;

        // 0 コマ目の B を空っぽにしてから A だけを分解する
        app.doc.library[0].timeline.layers[0].keyframe_mut(0).cel = timeline::Cel::default();
        app.doc.library[0].revision = crate::vector_engine::next_mesh_revision();
        app.doc.revision = crate::vector_engine::next_mesh_revision();
        app.refresh_scene_base();

        app.session.selection.clear();
        app.session.selection.instances.insert(a_gid);
        assert!(apply(&mut app, Action::Ungroup));

        let instances_after = &app.doc.root.layers[0].keyframes[0].cel.instances;
        assert_eq!(instances_after.len(), 2);
        assert_eq!(
            instances_after[0].binding.symbol_id, sym_b_id,
            "空っぽの B の印が A の位置に入り、C より奥（前）に保たれること"
        );
        assert_eq!(instances_after[1].binding.symbol_id, sym_c_id);

        // B に絵がある 1 コマ目に移り、シーン上でも B が C より奥にあることを確かめる
        assert!(apply(
            &mut app,
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 1,
            },
        ));
        let b_scene_idx = app
            .scene()
            .iter()
            .position(|s| {
                s.symbol_instance.is_some_and(|b| b.symbol_id == sym_b_id)
                    && !s.exterior.segments.is_empty()
            })
            .expect("1 コマ目で B の図形が見えること");
        let c_scene_idx = app
            .scene()
            .iter()
            .position(|s| {
                s.symbol_instance.is_some_and(|b| b.symbol_id == sym_c_id)
                    && !s.exterior.segments.is_empty()
            })
            .expect("1 コマ目で C の図形が見えること");
        assert!(
            b_scene_idx < c_scene_idx,
            "B に絵があるコマで、A より手前にあった C との前後関係（B が奥、C が手前）が変わらないこと"
        );
    }

    // 11 & 12. B の中のシンボル C は分解後も B の中で見え、A の中でグループ G に入っていた B（空っぽの場合も含む）は分解後も G の中に入る
    for b_empty_on_ungroup_frame in [false, true] {
        let (mut app, sym_a_id, sym_b_id) = build_nested_app(1);
        // B の中にシンボル C を作る
        assert!(apply(
            &mut app,
            Action::EnterSymbolFromLibrary { sym_id: sym_b_id },
        ));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::ConvertToSymbol));
        let sym_c_id = app.doc.library.last().unwrap().id;
        assert_ne!(sym_c_id, sym_b_id);
        assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));

        // A の中で B だけを内側グループ G_inner に入れ、さらに生の図形と一緒に外側グループ G_outer に入れる
        assert!(apply(
            &mut app,
            Action::EnterSymbolFromLibrary { sym_id: sym_a_id },
        ));
        let b_gid_in_a = app
            .doc
            .library
            .iter()
            .find(|s| s.id == sym_a_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[0]
            .cel
            .instances[0]
            .binding
            .instance_gid;
        app.session.selection.clear();
        app.session.selection.instances.insert(b_gid_in_a);
        assert!(apply(&mut app, Action::Group));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Group));
        assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));

        if b_empty_on_ungroup_frame {
            app.doc.library[0].timeline.layers[0].keyframe_mut(0).cel = timeline::Cel::default();
            app.doc.library[0].revision = crate::vector_engine::next_mesh_revision();
            app.doc.revision = crate::vector_engine::next_mesh_revision();
            app.refresh_scene_base();
        }

        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Ungroup));
        let cel = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel.instances.len(), 1);
        assert_eq!(cel.instances[0].binding.symbol_id, sym_b_id);
        assert_eq!(
            cel.instances[0].parent_scope.len(),
            2,
            "A の中で 2 重グループ [G_outer, G_inner] に入っていた B（b_empty={b_empty_on_ungroup_frame}）は分解後も両方のグループの中にいること"
        );
        assert_eq!(
            cel.instances[0].parent_scope[0],
            cel.shapes[0].group_path[0],
            "同じ外側グループ G_outer に入っていた生図形と B（b_empty={b_empty_on_ungroup_frame}）が分解後も同じグループ番号を持つこと"
        );
        assert_ne!(
            cel.instances[0].parent_scope[1], cel.shapes[0].group_path[0],
            "B だけが入っていた内側グループ G_inner にも新しいグループ番号が振られること"
        );
        if !b_empty_on_ungroup_frame {
            assert!(app.scene().iter().any(|s| s
                .symbol_instance
                .is_some_and(|b| b.symbol_id == sym_b_id)
                && !s.exterior.segments.is_empty()));
        }
    }
}

#[test]
fn test_failed_paste_due_to_nest_limit_rolls_back_clipboard_imported_map() {
    use crate::doc_actions::{apply, Action};

    let ctx = egui::Context::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 90, 230);

    // 1. タブ X で入れ子が8段ある赤いシンボルを作ってコピーする
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 8.0,
            color: red,
        },
    ));
    for _ in 0..8 {
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::ConvertToSymbol));
    }
    assert_eq!(app.doc.library.len(), 8);
    assert!(apply(&mut app, Action::Copy));

    // 2. タブ Y で別のシンボルの中に入って貼り付ける（9段になるため失敗する）
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(200.0, 200.0), Point::new(260.0, 200.0)],
            radius: 8.0,
            color: Color32::from_rgb(40, 180, 80),
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let host_sym_id = app.doc.library[0].id;
    assert!(apply(
        &mut app,
        Action::EnterSymbolFromLibrary {
            sym_id: host_sym_id
        },
    ));
    assert!(!apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 1);
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));

    // 3. タブ Y で新しい青いシンボルを作る
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(300.0, 200.0), Point::new(360.0, 200.0)],
            radius: 8.0,
            color: blue,
        },
    ));
    let blue_stroke_id = app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes
        .last()
        .unwrap()
        .shape_id;
    app.session.selection.clear();
    app.session.selection.shapes.insert(blue_stroke_id);
    assert!(apply(&mut app, Action::ConvertToSymbol));
    assert_eq!(app.doc.library.len(), 2);

    // 4. ルートステージ（入れ子上限に引っかからない場所）に貼り付けると、青いシンボルに化けず赤いシンボルが新しく登録されて貼られる
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(
        app.doc.library.len(),
        10,
        "8段の赤いシンボル群がタブ Y のライブラリに新しくインポートされること"
    );
    let pasted_inst_gid = *app.session.selection.instances.iter().next().unwrap();
    let pasted_shapes: Vec<_> = app
        .scene()
        .iter()
        .filter(|s| s.group_path.first() == Some(&pasted_inst_gid))
        .collect();
    assert!(!pasted_shapes.is_empty());
    assert!(
        pasted_shapes.iter().all(|s| s.color == red),
        "貼り付けたシンボルの図形が元の赤色であること"
    );
}

#[test]
fn test_open_project_reports_trimmed_data_and_marks_unsaved() {
    use crate::doc_actions::{apply, Action};
    use crate::project_io::{open_project_in_tab, save_project_to_path, OpenProjectOutcome};

    let base_dir =
        std::env::temp_dir().join(format!("is2d_open_trimmed_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base_dir);
    std::fs::create_dir_all(&base_dir).unwrap();

    // 3. ふつうに保存したファイルを開くと Loaded が返り、保存済みのまま
    let normal_path = base_dir.join("normal.is2d");
    let mut src_app = Is2dApp::default();
    assert!(apply(
        &mut src_app,
        Action::PaintStroke {
            points: vec![Point::new(40.0, 40.0), Point::new(120.0, 40.0)],
            radius: 6.0,
            color: Color32::from_rgb(220, 50, 50),
        },
    ));
    save_project_to_path(&mut src_app, &normal_path).unwrap();

    let mut app_normal = Is2dApp::default();
    assert_eq!(
        open_project_in_tab(&mut app_normal, &normal_path, None).unwrap(),
        OpenProjectOutcome::Loaded
    );
    assert!(!app_normal.history.is_dirty());
    assert_eq!(app_normal.file.as_deref(), Some(normal_path.as_path()));

    // 1. レイヤーの長さ（frame_count）より後ろにキーフレームがあるファイルを開くと LoadedWithTrimmedData が返り、未保存になる
    let out_of_range_kf_path = base_dir.join("out_of_range_kf.is2d");
    let mut proj_kf = crate::project_io::read_project_document(&normal_path).unwrap();
    proj_kf.timeline.layers[0].frame_count = 1;
    proj_kf.timeline.layers[0]
        .keyframes
        .push(crate::project_io::SavedKeyframe {
            start_frame: 5,
            tween: false,
            tween_rotate: crate::symbol::TweenRotate::Auto,
            shapes: Vec::new(),
            instances: Vec::new(),
        });
    crate::project_io::write_project_file_atomically(&out_of_range_kf_path, &proj_kf).unwrap();

    let mut app_kf = Is2dApp::default();
    assert_eq!(
        open_project_in_tab(&mut app_kf, &out_of_range_kf_path, None).unwrap(),
        OpenProjectOutcome::LoadedWithTrimmedData
    );
    assert!(app_kf.history.is_dirty());
    assert_eq!(app_kf.file.as_deref(), Some(out_of_range_kf_path.as_path()));

    // 2. レイヤーが 1001 個あるファイルを開いても LoadedWithTrimmedData が返り、未保存になる
    let over_layers_path = base_dir.join("over_1001_layers.is2d");
    let mut proj_layers = crate::project_io::read_project_document(&normal_path).unwrap();
    let sample_layer = proj_layers.timeline.layers[0].clone();
    while proj_layers.timeline.layers.len() < 1001 {
        let mut l = sample_layer.clone();
        l.id = (proj_layers.timeline.layers.len() + 1) as u64;
        proj_layers.timeline.layers.push(l);
    }
    crate::project_io::write_project_file_atomically(&over_layers_path, &proj_layers).unwrap();

    let mut app_layers = Is2dApp::default();
    assert_eq!(
        open_project_in_tab(&mut app_layers, &over_layers_path, None).unwrap(),
        OpenProjectOutcome::LoadedWithTrimmedData
    );
    assert!(app_layers.history.is_dirty());
    assert_eq!(app_layers.file.as_deref(), Some(over_layers_path.as_path()));
    assert_eq!(app_layers.doc.root.layers.len(), 1000);

    // 4. 面積がほぼ 0 の図形だけを含むファイルを開いても Loaded が返る（警告は出ない）
    let tiny_shape_path = base_dir.join("tiny_shape.is2d");
    let mut proj_tiny = crate::project_io::read_project_document(&normal_path).unwrap();
    proj_tiny.timeline.layers[0].keyframes[0].shapes = vec![crate::project_io::SavedFillShape {
        color: [255, 0, 0, 255],
        exterior: crate::project_io::SavedContour {
            segments: std::sync::Arc::new(vec![
                kurbo::QuadBez::new(
                    Point::new(10.0, 10.0),
                    Point::new(10.0, 10.0),
                    Point::new(10.0, 10.0),
                ),
                kurbo::QuadBez::new(
                    Point::new(10.0, 10.0),
                    Point::new(10.0, 10.0),
                    Point::new(10.0, 10.0),
                ),
            ]),
        },
        holes: Vec::new(),
        layer_id: 1,
        group_path: Vec::new(),
    }];
    crate::project_io::write_project_file_atomically(&tiny_shape_path, &proj_tiny).unwrap();

    let mut app_tiny = Is2dApp::default();
    assert_eq!(
        open_project_in_tab(&mut app_tiny, &tiny_shape_path, None).unwrap(),
        OpenProjectOutcome::Loaded
    );
    assert!(!app_tiny.history.is_dirty());

    let _ = std::fs::remove_dir_all(&base_dir);
}

#[test]
fn test_merge_self_overlapping_batch_never_produces_duplicate_shape_ids() {
    use std::collections::HashSet;

    use crate::doc_actions::{apply, Action};

    let col = Color32::from_rgb(220, 40, 40);

    // 1. 半径5の同じ色の点を間隔11で 3 個・4 個並べ、入ってくる順番（順列）をすべて試して shape_id が重複しないこと
    fn permutations(n: usize) -> Vec<Vec<usize>> {
        if n == 0 {
            return vec![vec![]];
        }
        let mut out = Vec::new();
        for i in 0..n {
            for mut sub in permutations(n - 1) {
                for v in &mut sub {
                    if *v >= i {
                        *v += 1;
                    }
                }
                sub.insert(0, i);
                out.push(sub);
            }
        }
        out
    }

    for count in [3usize, 4usize] {
        let base_dots: Vec<FillShape> = (0..count)
            .map(|i| {
                let pt = Point::new(100.0 + (i as f64) * 11.0, 100.0);
                make_stroke(pt, pt, 5.0, col).into_iter().next().unwrap()
            })
            .collect();

        for perm in permutations(count) {
            let incoming: Vec<FillShape> = perm.iter().map(|&idx| base_dots[idx].clone()).collect();
            let mut canvas = Vec::new();
            vector_boolean::merge_shapes_into_canvas(&mut canvas, incoming);
            assert_eq!(canvas.len(), count);
            let ids: HashSet<u64> = canvas.iter().map(|s| s.shape_id).collect();
            assert_eq!(
                ids.len(),
                canvas.len(),
                "順列 {perm:?} の結合結果に重複する shape_id が無いこと"
            );
        }
    }

    // 3. 重複が起きていた形（間隔11の3点を [真ん中, 左, 右] の順で結合）で、片方だけを選んで矢印キーで動かすとその1つだけが動くこと
    {
        let mut app = Is2dApp::default();
        let dots: Vec<FillShape> = [1usize, 0, 2]
            .into_iter()
            .map(|i| {
                let pt = Point::new(100.0 + (i as f64) * 11.0, 100.0);
                make_stroke(pt, pt, 5.0, col).into_iter().next().unwrap()
            })
            .collect();
        let mut canvas = Vec::new();
        vector_boolean::merge_shapes_into_canvas(&mut canvas, dots);
        put_cel_shapes(&mut app, 0, 0, canvas);

        let before_centers: Vec<Point> = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes
            .iter()
            .map(|s| s.bounding_box().center())
            .collect();
        assert_eq!(before_centers.len(), 3);

        let target_id = app.doc.root.layers[0].keyframes[0].cel.shapes[0].shape_id;
        app.session.selection.clear();
        app.session.selection.shapes.insert(target_id);
        assert!(apply(
            &mut app,
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(0.0, 30.0),
            },
        ));
        assert!(apply(&mut app, Action::Deselect));

        let after_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        assert_eq!(after_shapes.len(), 3);
        let moved_count = after_shapes
            .iter()
            .filter(|s| (s.bounding_box().center().y - 130.0).abs() < 2.0)
            .count();
        let stayed_count = after_shapes
            .iter()
            .filter(|s| (s.bounding_box().center().y - 100.0).abs() < 2.0)
            .count();
        assert_eq!(moved_count, 1, "選んだ1つだけが動くこと");
        assert_eq!(
            stayed_count, 2,
            "選ばれていない残り2つは元の位置に留まること"
        );
    }

    // 2. 点を描いて、いくつか選んで移動（ふつうの移動と Alt 複製の両方）して選択を外す流れを乱数で繰り返し、shape_id が重複しないこと
    {
        let mut rng = fastrand::Rng::with_seed(0x2026_1103);
        let mut app = Is2dApp::default();

        for step in 0..300 {
            if app.doc.root.layers[0].keyframes[0].cel.shapes.len() > 25 {
                assert!(apply(&mut app, Action::ClearActiveFrame));
            }
            if app.doc.root.layers[0].keyframes[0].cel.shapes.len() < 3 || rng.bool() {
                let gx = 100.0 + (rng.usize(0..6) as f64) * 11.0;
                let gy = 100.0 + (rng.usize(0..4) as f64) * 11.0;
                let pt = Point::new(gx, gy);
                let _ = apply(
                    &mut app,
                    Action::PaintStroke {
                        points: vec![pt, Point::new(gx + 0.5, gy)],
                        radius: 5.0,
                        color: col,
                    },
                );
            } else {
                let shape_ids: Vec<u64> = app.doc.root.layers[0].keyframes[0]
                    .cel
                    .shapes
                    .iter()
                    .map(|s| s.shape_id)
                    .collect();
                app.session.selection.clear();
                for &sid in &shape_ids {
                    if rng.bool() {
                        app.session.selection.shapes.insert(sid);
                    }
                }
                if app.session.selection.shapes.is_empty() {
                    app.session.selection.shapes.insert(shape_ids[0]);
                }
                let dx = (rng.i32(-2..=2) as f64) * 11.0;
                let dy = (rng.i32(-2..=2) as f64) * 11.0;
                let alt_duplicate = rng.bool();
                let _ = apply(
                    &mut app,
                    Action::FinishMoveSelectionDrag {
                        total_delta: kurbo::Vec2::new(dx, dy),
                        alt_duplicate,
                    },
                );
                let _ = apply(&mut app, Action::Deselect);
            }

            let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
            let unique_ids: HashSet<u64> = shapes.iter().map(|s| s.shape_id).collect();
            assert_eq!(
                unique_ids.len(),
                shapes.len(),
                "step {step}: セルの図形に重複する shape_id が発生しないこと"
            );
        }
    }
}

#[test]
fn test_toggle_playback_cancels_in_progress_brush_and_eraser_stroke() {
    use crate::canvas_tool::ActiveCanvasDrag;
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();

    // 1. ブラシで描いている途中（session.drag = PaintStroke）に toggle_playback で再生を始めると、drag が None になり図形が増えないこと
    app.tool.tool = Tool::Brush;
    app.session.drag = ActiveCanvasDrag::PaintStroke {
        stroke: vec![Point::new(50.0, 50.0), Point::new(150.0, 50.0)],
        committed_world_mesh: egui::Mesh::default(),
        tail_outline: vec![Point::new(50.0, 45.0), Point::new(150.0, 45.0)],
        tail_mesh: egui::Mesh::default(),
        tail_start_idx: 0,
    };
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    assert!(matches!(app.session.drag, ActiveCanvasDrag::None));
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 0);
    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing);

    // 2. 消しゴムのストロークの途中でも同じく drag が None になり、既存の図形も変わらないこと
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(40.0, 40.0), Point::new(160.0, 40.0)],
            radius: 10.0,
            color: Color32::from_rgb(220, 40, 40),
        },
    ));
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 1);
    app.tool.tool = Tool::Eraser;
    app.session.drag = ActiveCanvasDrag::PaintStroke {
        stroke: vec![Point::new(40.0, 40.0), Point::new(160.0, 40.0)],
        committed_world_mesh: egui::Mesh::default(),
        tail_outline: vec![Point::new(40.0, 35.0), Point::new(160.0, 35.0)],
        tail_mesh: egui::Mesh::default(),
        tail_start_idx: 0,
    };
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    assert!(matches!(app.session.drag, ActiveCanvasDrag::None));
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes.len(), 1);
}

#[test]
fn test_timeline_move_or_copy_frames_clips_range_per_source_layer_frame_count() {
    use crate::doc_actions::{apply, Action};
    use crate::timeline::TimelineCellSelection;

    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 90, 230);

    // レイヤー A（layer 0, 100コマ, 赤）と レイヤー B（layer 1, 5コマ, 青）を作るヘルパー
    let build_app = || -> Is2dApp {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            make_stroke(Point::new(50.0, 50.0), Point::new(120.0, 50.0), 8.0, red),
        );
        app.doc.root.layers[0].frame_count = 100;

        assert!(apply(&mut app, Action::AddLayer));
        put_cel_shapes(
            &mut app,
            1,
            0,
            make_stroke(Point::new(50.0, 120.0), Point::new(120.0, 120.0), 8.0, blue),
        );
        app.doc.root.layers[1].frame_count = 5;
        app.doc.root.sync_total_frames();
        app.refresh_scene_base();
        app
    };

    // 1. 0〜99 コマを +10 移動すると、B の絵が見えるのは 10〜14 コマ目の5コマだけ、A は 10〜109 コマ目
    {
        let mut app = build_app();
        let sel = TimelineCellSelection {
            min_layer: 0,
            max_layer: 1,
            min_frame: 0,
            max_frame: 99,
        };
        assert!(apply(
            &mut app,
            Action::MoveOrCopyFrames {
                sel,
                layer_delta: 0,
                frame_delta: 10,
                copy_mode: false,
            },
        ));
        let layer_a = &app.doc.root.layers[0];
        let layer_b = &app.doc.root.layers[1];
        assert_eq!(layer_a.frame_count, 110);
        assert_eq!(layer_b.frame_count, 15);
        for f in 0..115 {
            let a_has = layer_a.cel_at(f).is_some_and(|c| !c.shapes.is_empty());
            let b_has = layer_b.cel_at(f).is_some_and(|c| !c.shapes.is_empty());
            assert_eq!(
                a_has,
                (10..=109).contains(&f),
                "移動後のレイヤー A の {f} コマ目の有無"
            );
            assert_eq!(
                b_has,
                (10..=14).contains(&f),
                "移動後のレイヤー B の {f} コマ目の有無"
            );
        }
    }

    // 2. 同じ例で複製すると、B の複製先は 10〜14 コマ目の5コマだけで終わり（frame_count = 15）、15〜109 コマ目には延びない
    {
        let mut app = build_app();
        let sel = TimelineCellSelection {
            min_layer: 0,
            max_layer: 1,
            min_frame: 0,
            max_frame: 99,
        };
        assert!(apply(
            &mut app,
            Action::MoveOrCopyFrames {
                sel,
                layer_delta: 0,
                frame_delta: 10,
                copy_mode: true,
            },
        ));
        let layer_a = &app.doc.root.layers[0];
        let layer_b = &app.doc.root.layers[1];
        assert_eq!(layer_a.frame_count, 110);
        assert_eq!(layer_b.frame_count, 15);
        assert_eq!(layer_b.keyframes.len(), 2);
        assert_eq!(layer_b.keyframes[0].start_frame, 0);
        assert_eq!(layer_b.keyframes[1].start_frame, 10);
        assert_eq!(layer_b.span_end_for_keyframe(1), 14);
        for f in 10..=14 {
            assert!(layer_b.cel_at(f).is_some_and(|c| !c.shapes.is_empty()));
        }
        for f in 15..110 {
            assert!(
                layer_b.cel_at(f).is_none(),
                "複製後のレイヤー B は 14 コマ目で終わり、{f} コマ目には延びないこと"
            );
        }
    }

    // 3. 選んだ範囲（20〜30 コマ）がレイヤー B（0〜4 コマ）の最後のコマより全部後ろにあるとき、B は何も変わらない
    {
        let mut app = build_app();
        let b_before_fc = app.doc.root.layers[1].frame_count;
        let b_before_kfs = app.doc.root.layers[1].keyframes.clone();
        let sel = TimelineCellSelection {
            min_layer: 0,
            max_layer: 1,
            min_frame: 20,
            max_frame: 30,
        };
        assert!(apply(
            &mut app,
            Action::MoveOrCopyFrames {
                sel,
                layer_delta: 0,
                frame_delta: 15,
                copy_mode: false,
            },
        ));
        let layer_b = &app.doc.root.layers[1];
        assert_eq!(layer_b.frame_count, b_before_fc);
        assert_eq!(layer_b.keyframes.len(), b_before_kfs.len());
        for f in 0..20 {
            let b_has = layer_b.cel_at(f).is_some_and(|c| !c.shapes.is_empty());
            assert_eq!(b_has, (0..=4).contains(&f));
        }
    }
}

#[test]
fn test_cross_tab_symbol_paste_preserves_imported_map_across_intermediate_copies() {
    use crate::doc_actions::{apply, Action};

    let ctx = egui::Context::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 90, 230);
    let green = Color32::from_rgb(40, 180, 90);

    // 1. タブ A（idx 0）のシンボル S をコピーしてタブ B（idx 1）に貼る → B で別の図形を選んでコピー →
    //    A に戻って S をもう一度コピーして B に貼ると、B のライブラリは「Symbol 1」の1つだけ
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(50.0, 50.0), Point::new(120.0, 50.0)],
            radius: 8.0,
            color: red,
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    assert_eq!(app.doc.library.len(), 1);
    assert_eq!(app.doc.library[0].id, 1);
    assert_eq!(app.doc.library[0].name, "Symbol 1");
    assert!(apply(&mut app, Action::Copy));

    // タブ B（idx 1）を作って 1 回目の貼り付け
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 1);
    assert_eq!(app.doc.library[0].name, "Symbol 1");

    // タブ B で別の生の図形を描いてそれだけを選んでコピーする
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(200.0, 200.0), Point::new(260.0, 200.0)],
            radius: 8.0,
            color: blue,
        },
    ));
    let raw_id = app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes
        .last()
        .unwrap()
        .shape_id;
    app.session.selection.clear();
    app.session.selection.shapes.insert(raw_id);
    assert!(apply(&mut app, Action::Copy));

    // タブ A（idx 0）に戻ってシンボル S をもう一度コピーし、タブ B（idx 1）に戻って貼り付ける
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));
    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(
        app.doc.library.len(),
        1,
        "2回目の貼り付けの後もタブ B のライブラリが「Symbol 1」の1つだけであること"
    );
    assert_eq!(app.doc.library[0].name, "Symbol 1");
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.instances.len(), 2);

    // 2. 別のタブ C（idx 2）にも同じ番号（1番）の緑のシンボルがあるとき、タブ B に貼ると別々のシンボルとして2つ取り込まれる
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(80.0, 80.0), Point::new(150.0, 80.0)],
            radius: 8.0,
            color: green,
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    assert_eq!(app.doc.library[0].id, 1);
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(
        app.doc.library.len(),
        2,
        "タブ A の 1 番とタブ C の 1 番が取り違えられず、別々のシンボルとして 2 つ取り込まれること"
    );
    assert_eq!(app.doc.library[0].name, "Symbol 1");
    assert_eq!(app.doc.library[1].name, "Symbol 1 (2)");
    assert!(app.scene().iter().any(|s| s.color == red));
    assert!(app.scene().iter().any(|s| s.color == green));
}

#[test]
fn test_deselected_slice_piece_does_not_collide_with_kept_piece_id() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 200.0), Point::new(300.0, 200.0)],
            radius: 20.0,
            color: Color32::RED,
        },
    ));
    let orig_id = app.doc.root.layers[0].keyframes[0].cel.shapes[0].shape_id;

    // 1. 左半分を矩形で選ぶ（スライス）
    crate::select_ops::apply_marquee_selection(
        &mut app,
        Point::new(80.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert_eq!(app.session.selection.shapes.len(), 1);

    // 2. Shift+クリックで右の残りも選択に足す → つながっているので選択は1つになる
    let right_idx = app
        .scene()
        .iter()
        .position(|s| s.contains_point(Point::new(260.0, 200.0)))
        .expect("右の残りにヒットすること");
    assert!(apply(
        &mut app,
        Action::ToggleSelectionAt {
            scene_idx: right_idx
        }
    ));
    assert_eq!(app.session.selection.shapes.len(), 1);

    // 今のコマの Scene の中に同じ番号の図形が無いこと
    let scene_ids: Vec<_> = app.scene().iter().map(|s| s.shape_id).collect();
    let unique_ids: std::collections::HashSet<_> = scene_ids.iter().copied().collect();
    assert_eq!(
        scene_ids.len(),
        unique_ids.len(),
        "Scene の中に同じ shape_id の図形が無いこと: {scene_ids:?}"
    );

    // 3. Shift+クリックで左の所を押すと、全体が外れて元の図形（同じ番号）に戻る
    let left_idx = app
        .scene()
        .iter()
        .position(|s| s.contains_point(Point::new(130.0, 200.0)))
        .expect("結合された片にヒットすること");
    assert!(apply(
        &mut app,
        Action::ToggleSelectionAt {
            scene_idx: left_idx
        }
    ));
    assert!(app.session.selection.shapes.is_empty());
    assert!(app.session.floating().is_none());
    assert_eq!(app.scene().len(), 1);
    assert_eq!(app.scene().get(0).unwrap().shape_id, orig_id);
}

#[test]
fn test_alt_duplicate_and_paste_raw_shape_with_group_keeps_both_selected() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(230, 50, 50);
    let blue = Color32::from_rgb(50, 110, 230);

    // 青の図形を描いてグループ化する
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(220.0, 100.0), Point::new(280.0, 100.0)],
            radius: 10.0,
            color: blue,
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Group));
    assert!(apply(&mut app, Action::Deselect));

    // 赤の生の図形を描く
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 10.0,
            color: red,
        },
    ));

    // 1. 赤（生）と青（グループ）を矩形で選び、Alt+ドラッグで下へ 100px 動かして離す
    crate::select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(350.0, 160.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 2);

    let mut drag =
        crate::clipboard_ops::begin_move_selection_drag(&mut app, Point::new(130.0, 100.0), true);
    drag.total_delta = kurbo::Vec2::new(0.0, 100.0);
    crate::doc_actions::finish_move_selection_drag(&mut app, drag.total_delta, drag.alt_duplicate);

    // 赤と青の複製が両方とも選択されていて、移動先（y ≈ 200）にあること
    let sel_indices = app.session.selected_indices();
    assert_eq!(sel_indices.len(), 2);
    for idx in sel_indices {
        let s = app.scene().get(idx).unwrap();
        assert!(
            (s.bounding_box().center().y - 200.0).abs() < 5.0,
            "複製が移動先にあること: {:?}",
            s.bounding_box()
        );
    }

    // 選択を外したあと、赤の元の図形と、赤の複製の両方が残っていること
    assert!(apply(&mut app, Action::Deselect));
    let red_count = app.scene().iter().filter(|s| s.color == red).count();
    let blue_count = app.scene().iter().filter(|s| s.color == blue).count();
    assert_eq!(red_count, 2, "赤の元の図形と複製の両方が残っていること");
    assert_eq!(blue_count, 2);

    // 2. 同じ2つ（y=100 の赤と青）をコピーしてペーストする → 両方選択され、そのまま動かすと両方が動く
    crate::select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(350.0, 150.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 2);
    assert!(apply(&mut app, Action::Copy));
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    assert_eq!(
        app.session.selected_indices().len(),
        2,
        "貼り付けた赤と青が両方とも選択されていること"
    );
    assert!(apply(
        &mut app,
        Action::MoveSelected {
            delta: kurbo::Vec2::new(0.0, 200.0),
        },
    ));
    for idx in app.session.selected_indices() {
        let s = app.scene().get(idx).unwrap();
        assert!(
            s.bounding_box().center().y > 280.0,
            "ペースト後に動かすと赤と青の両方が動くこと: {:?}",
            s.bounding_box()
        );
    }
}

#[test]
fn test_group_rejects_cross_layer_selection_and_allows_single_layer() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(230, 50, 50);
    let blue = Color32::from_rgb(50, 110, 230);

    // レイヤー0（下）に赤を描く
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 10.0,
            color: red,
        },
    ));
    // レイヤーを追加（新しいレイヤーが index 0 になり、元のレイヤーは index 1 になる）して青を描く
    assert!(apply(&mut app, Action::AddLayer));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 200.0), Point::new(160.0, 200.0)],
            radius: 10.0,
            color: blue,
        },
    ));

    // 1. 下のレイヤーの赤と上のレイヤーの青を選んで Action::Group → false が返り、文書と Undo 履歴が変わらない
    assert!(apply(&mut app, Action::SelectAll));
    assert_eq!(app.session.selected_indices().len(), 2);
    assert_eq!(crate::select_ops::selected_single_layer_id(&app), None);
    let undo_len_before = app.history.undo_stack.len();
    assert!(!apply(&mut app, Action::Group));
    assert_eq!(app.history.undo_stack.len(), undo_len_before);
    assert!(app.scene().iter().all(|s| s.group_path.is_empty()));

    // 2. 同じレイヤーの2つの図形なら、今までどおりグループになる
    assert!(apply(&mut app, Action::Deselect));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(220.0, 200.0), Point::new(280.0, 200.0)],
            radius: 10.0,
            color: Color32::GREEN,
        },
    ));
    crate::select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 160.0),
        Point::new(320.0, 240.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 2);
    assert!(crate::select_ops::selected_single_layer_id(&app).is_some());
    assert!(apply(&mut app, Action::Group));
    assert!(app
        .scene()
        .iter()
        .filter(|s| s.color != red)
        .all(|s| !s.group_path.is_empty()));

    // 3. レビューの手順（下のレイヤーで F6、上のレイヤーで F5、1コマ目で両方を選んで Cmd+G）
    // 上のレイヤー（0）で F5、下のレイヤー（1）で F6
    app.set_active_layer(0);
    app.set_active_cell_selection(Some(crate::timeline::TimelineCellSelection::single(0, 0)));
    assert!(apply(&mut app, Action::ExtendFrame));
    app.set_active_layer(1);
    app.set_active_cell_selection(Some(crate::timeline::TimelineCellSelection::single(1, 0)));
    assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: true }));
    assert_eq!(app.doc.root.current_frame, 1);

    assert!(apply(&mut app, Action::SelectAll));
    assert!(!apply(&mut app, Action::Group));
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 0 }));
    let frame0_ids: Vec<_> = app.scene().iter().map(|s| s.shape_id).collect();
    let unique_frame0: std::collections::HashSet<_> = frame0_ids.iter().copied().collect();
    assert_eq!(frame0_ids.len(), unique_frame0.len());
}

#[test]
fn test_cross_tab_symbol_roundtrip_paste_reuses_original_symbol() {
    use crate::doc_actions::{apply, Action};

    let ctx = eframe::egui::Context::default();
    let mut app = Is2dApp::default();

    // タブ1（index 0）で図形を F8 → コピー
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let orig_sym_id = app.doc.library[0].id;
    assert!(apply(&mut app, Action::Copy));

    // タブ2（index 1）へペースト → タブ2でそのインスタンスをコピー
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 1);
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));

    // タブ1へ戻してペースト → タブ1のライブラリはシンボル1つのままで、貼ったインスタンスは元のシンボルを指す
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    assert_eq!(app.doc.library.len(), 1);
    let instances = &app.doc.root.layers[0].keyframes[0].cel.instances;
    assert_eq!(instances.len(), 2);
    assert!(instances
        .iter()
        .all(|inst| inst.binding.symbol_id == orig_sym_id));

    // 続けてタブ1でコピー → タブ2へペーストしても、タブ2のライブラリはシンボル1つのまま
    assert!(apply(&mut app, Action::Copy));
    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    assert_eq!(app.doc.library.len(), 1);
}

#[test]
fn test_recovery_fallback_same_filename_and_missing_parent_dir() {
    use crate::doc_actions::{apply, Action};
    use crate::panels::UiAction;

    let ctx = eframe::egui::Context::default();
    let base_dir = std::env::temp_dir().join(format!(
        "is2d_rec_fix5_6_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let rec_dir = base_dir.join("recovery_dir");
    std::fs::create_dir_all(&rec_dir).unwrap();

    let red = Color32::from_rgb(230, 40, 40);
    let blue = Color32::from_rgb(40, 100, 230);

    // 1. 修正5（N-4001）：無いフォルダ gone1/scene.is2d と gone2/scene.is2d を中身の違う未保存のタブ2つにして書く
    let mut app = Is2dApp::default();
    app.recovery.dir = Some(rec_dir.clone());
    let gone1 = base_dir.join("gone1").join("scene.is2d");
    let gone2 = base_dir.join("gone2").join("scene.is2d");
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(50.0, 50.0), Point::new(100.0, 50.0)],
            radius: 8.0,
            color: red,
        },
    ));
    app.tabs[0].file = Some(gone1.clone());

    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(150.0, 150.0), Point::new(200.0, 150.0)],
            radius: 8.0,
            color: blue,
        },
    ));
    app.tabs[1].file = Some(gone2.clone());

    app.recovery.write_unsaved_tabs(&app.tabs);

    let mut next_app = Is2dApp::default();
    next_app.recovery.dir = Some(rec_dir.clone());
    next_app.recovery.find_leftovers();
    assert_eq!(
        next_app.recovery.found.len(),
        2,
        "同じファイル名でも別タブなら上書きされず2件見つかること"
    );
    next_app.apply_action(UiAction::RestoreRecovered, &ctx);
    assert_eq!(next_app.tabs.len(), 2);
    let tab_gone1 = next_app
        .tabs
        .iter()
        .find(|t| t.file.as_deref() == Some(gone1.as_path()))
        .expect("gone1/scene.is2d が復元されること");
    let tab_gone2 = next_app
        .tabs
        .iter()
        .find(|t| t.file.as_deref() == Some(gone2.as_path()))
        .expect("gone2/scene.is2d が復元されること");
    assert_eq!(
        tab_gone1.doc.root.layers[0].keyframes[0].cel.shapes[0].color,
        red
    );
    assert_eq!(
        tab_gone2.doc.root.layers[0].keyframes[0].cel.shapes[0].color,
        blue
    );

    // 2. 修正6（N-4002）：vol/scene.is2d を未保存のタブにして書く → vol の名前を変える → find_leftovers で found は 0、一覧ファイルは残る → vol の名前を戻して find_leftovers → found は 1
    let vol_dir = base_dir.join("vol");
    let vol_unmounted = base_dir.join("vol_unmounted");
    std::fs::create_dir_all(&vol_dir).unwrap();
    let vol_proj = vol_dir.join("scene.is2d");

    let mut vol_app = Is2dApp::default();
    vol_app.recovery.dir = Some(rec_dir.clone());
    assert!(apply(
        &mut vol_app,
        Action::PaintStroke {
            points: vec![Point::new(80.0, 80.0), Point::new(140.0, 80.0)],
            radius: 8.0,
            color: red,
        },
    ));
    vol_app.tabs[0].file = Some(vol_proj);
    vol_app.recovery.write_unsaved_tabs(&vol_app.tabs);

    std::fs::rename(&vol_dir, &vol_unmounted).unwrap();
    let mut check_app = Is2dApp::default();
    check_app.recovery.dir = Some(rec_dir.clone());
    check_app.recovery.find_leftovers();
    assert_eq!(check_app.recovery.found.len(), 0);
    let entry_count = std::fs::read_dir(&rec_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("entry"))
        .count();
    assert_eq!(
        entry_count, 1,
        "フォルダ自体が見えないときは一覧ファイルを消さずに残すこと"
    );

    std::fs::rename(&vol_unmounted, &vol_dir).unwrap();
    check_app.recovery.find_leftovers();
    assert_eq!(
        check_app.recovery.found.len(),
        1,
        "フォルダが戻った後の起動で復元対象として見つかること"
    );

    let _ = std::fs::remove_dir_all(&base_dir);
}

#[test]
fn test_merge_touching_sliced_pieces_on_additive_select() {
    use crate::doc_actions::{apply, Action};

    let make_app_with_bar = || {
        let mut app = Is2dApp::default();
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 200.0), Point::new(300.0, 200.0)],
                radius: 20.0,
                color: Color32::RED,
            },
        ));
        app
    };

    // 1. 左半分を矩形で選び、Shift+クリックで右半分を足す → 片が1つ、選択が1つ、残りが0
    // 2. 1 のあと、左の所を Shift+クリック → 選択が0、Floating が無い、図形は元の図形（番号と外周の Contour が同じ）
    {
        let mut app = make_app_with_bar();
        let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes[0].clone();

        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(70.0, 150.0),
                p1: Point::new(200.0, 250.0),
                additive: false,
            },
        ));
        let right_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(260.0, 200.0)))
            .unwrap();
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt {
                scene_idx: right_idx
            }
        ));
        let fl = app.session.floating().expect("Floating が存在すること");
        assert_eq!(fl.pieces.len(), 1);
        assert_eq!(fl.remainder.len(), 0);
        assert_eq!(app.session.selection.shapes.len(), 1);

        let left_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(130.0, 200.0)))
            .unwrap();
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt {
                scene_idx: left_idx
            }
        ));
        assert_eq!(app.session.selection.shapes.len(), 0);
        assert!(app.session.floating().is_none());
        let restored = &app.doc.root.layers[0].keyframes[0].cel.shapes[0];
        assert_eq!(restored.shape_id, orig_shape.shape_id);
        assert!(std::sync::Arc::ptr_eq(
            &restored.exterior.segments,
            &orig_shape.exterior.segments
        ));
    }

    // 3. 1 のあとグループ化して選択を外す → セルの図形は1つで、グループの中にある
    {
        let mut app = make_app_with_bar();
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(70.0, 150.0),
                p1: Point::new(200.0, 250.0),
                additive: false,
            },
        ));
        let right_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(260.0, 200.0)))
            .unwrap();
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt {
                scene_idx: right_idx
            }
        ));
        assert!(apply(&mut app, Action::Group));
        assert!(apply(&mut app, Action::Deselect));
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].group_path.len(), 1);
    }

    // 4. 左1/3 を選び、Shift+矩形で真ん中 1/3 を足す → 片が1つ、残りが1つ
    {
        let mut app = make_app_with_bar();
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(70.0, 150.0),
                p1: Point::new(160.0, 250.0),
                additive: false,
            },
        ));
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(160.0, 150.0),
                p1: Point::new(240.0, 250.0),
                additive: true,
            },
        ));
        let fl = app.session.floating().expect("Floating が存在すること");
        assert_eq!(fl.pieces.len(), 1);
        assert_eq!(fl.remainder.len(), 1);
    }

    // 5. 左端を選び、Shift+矩形で右端を足す（つながっていない）→ 片が2つ
    {
        let mut app = make_app_with_bar();
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(70.0, 150.0),
                p1: Point::new(140.0, 250.0),
                additive: false,
            },
        ));
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(260.0, 150.0),
                p1: Point::new(330.0, 250.0),
                additive: true,
            },
        ));
        let fl = app.session.floating().expect("Floating が存在すること");
        assert_eq!(fl.pieces.len(), 2);
    }

    // 6. 1 のあと動かさずに選択を外す → 元の図形のまま（番号も同じ）で、Undo の履歴が増えない
    {
        let mut app = make_app_with_bar();
        let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes[0].clone();
        let undo_before = app.history.undo_stack.len();

        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(70.0, 150.0),
                p1: Point::new(200.0, 250.0),
                additive: false,
            },
        ));
        let right_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(260.0, 200.0)))
            .unwrap();
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt {
                scene_idx: right_idx
            }
        ));
        assert!(apply(&mut app, Action::Deselect));
        assert_eq!(app.history.undo_stack.len(), undo_before);
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].shape_id, orig_shape.shape_id);
        assert!(std::sync::Arc::ptr_eq(
            &shapes[0].exterior.segments,
            &orig_shape.exterior.segments
        ));
    }
}

#[test]
fn test_deselected_slice_piece_merges_into_touching_remainder() {
    use crate::doc_actions::{apply, Action};

    let make_app_with_long_bar = || {
        let mut app = Is2dApp::default();
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 200.0), Point::new(500.0, 200.0)],
                radius: 20.0,
                color: Color32::RED,
            },
        ));
        app
    };

    // 1. A（x 80〜150）を矩形で選び、Shift+矩形で B（x 280〜340）を足し、Shift+クリックで B を外す
    //    → 片が1つ、残りが1つ。B のあたり（x 310）にある図形の外接矩形が x 150 より左から x 500 より右まで続いている
    // 2. 1 のあと、B のあたりを Shift+クリック → 片が1つ・残りが0・選択が1つ（図形全体）
    {
        let mut app = make_app_with_long_bar();
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(80.0, 150.0),
                p1: Point::new(150.0, 250.0),
                additive: false,
            },
        ));
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(280.0, 150.0),
                p1: Point::new(340.0, 250.0),
                additive: true,
            },
        ));
        let fl_before = app.session.floating().unwrap();
        assert_eq!(fl_before.pieces.len(), 2);
        assert_eq!(fl_before.remainder.len(), 2);

        let b_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(310.0, 200.0)))
            .expect("B の片にヒットすること");
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt { scene_idx: b_idx }
        ));

        let fl_after = app.session.floating().unwrap();
        assert_eq!(fl_after.pieces.len(), 1);
        assert_eq!(fl_after.remainder.len(), 1);
        let rem_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(310.0, 200.0)))
            .expect("B のあたりの残りにヒットすること");
        let rem_bbox = app.scene().get(rem_idx).unwrap().bounding_box();
        assert!(
            rem_bbox.x0 <= 150.5 && rem_bbox.x1 >= 500.0,
            "B のあたりの図形が A 以外の残り全体になっていること: {rem_bbox:?}"
        );

        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt { scene_idx: rem_idx }
        ));
        let fl_all = app.session.floating().unwrap();
        assert_eq!(fl_all.pieces.len(), 1);
        assert_eq!(fl_all.remainder.len(), 0);
        assert_eq!(app.session.selection.shapes.len(), 1);
    }

    // 3. 1 のあと、動かさずに選択を外す → 図形は1つで、元の番号のまま。Undo の履歴は増えない
    {
        let mut app = make_app_with_long_bar();
        let orig_id = app.doc.root.layers[0].keyframes[0].cel.shapes[0].shape_id;
        let undo_before = app.history.undo_stack.len();

        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(80.0, 150.0),
                p1: Point::new(150.0, 250.0),
                additive: false,
            },
        ));
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(280.0, 150.0),
                p1: Point::new(340.0, 250.0),
                additive: true,
            },
        ));
        let b_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(310.0, 200.0)))
            .unwrap();
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt { scene_idx: b_idx }
        ));
        assert!(apply(&mut app, Action::Deselect));
        assert_eq!(app.history.undo_stack.len(), undo_before);
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].shape_id, orig_id);
    }

    // 4. つながっていない片を外すとき（赤の左半分を選び→Shift+クリックで赤の右半分も足して赤の残りを0にし→Shift+矩形で青の左半分を足し→赤をShift+クリックで外す）は、今までどおり残りに入り、Scene に同じ番号の図形が無い
    {
        let mut app = Is2dApp::default();
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(300.0, 100.0)],
                radius: 15.0,
                color: Color32::RED,
            },
        ));
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 250.0), Point::new(300.0, 250.0)],
                radius: 15.0,
                color: Color32::BLUE,
            },
        ));
        // 赤の左半分をスライス → Shift+クリックで赤の右半分も足す（赤の残りは 0 になる）
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(70.0, 60.0),
                p1: Point::new(200.0, 140.0),
                additive: false,
            },
        ));
        let red_right_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(260.0, 100.0)))
            .unwrap();
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt {
                scene_idx: red_right_idx
            }
        ));
        // Shift+矩形で青の左半分をスライス追加
        assert!(apply(
            &mut app,
            Action::SelectMarquee {
                p0: Point::new(70.0, 210.0),
                p1: Point::new(200.0, 290.0),
                additive: true,
            },
        ));
        assert_eq!(app.session.floating().unwrap().pieces.len(), 2);
        assert_eq!(app.session.floating().unwrap().remainder.len(), 1);

        // 上の赤（残りが無い片）を Shift+クリックで外す
        let red_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(150.0, 100.0)))
            .unwrap();
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt { scene_idx: red_idx }
        ));
        let fl = app.session.floating().unwrap();
        assert_eq!(fl.pieces.len(), 1);
        assert_eq!(fl.remainder.len(), 2);

        let scene_ids: Vec<_> = app.scene().iter().map(|s| s.shape_id).collect();
        let unique_ids: std::collections::HashSet<_> = scene_ids.iter().copied().collect();
        assert_eq!(scene_ids.len(), unique_ids.len());
    }
}

#[test]
fn test_boolean_ops_with_far_holes_detached() {
    use crate::doc_actions::{apply, Action};
    use crate::vector_boolean::{boolean_operation, BoolOp};
    use crate::vector_engine::{rects_overlap, union_bounding_boxes, FillShape};
    use crate::vector_stroke::{
        build_smoothed_stroke_shapes_at_zoom, generate_brush_outline_at_zoom,
    };

    let shape_net_area = |s: &FillShape| -> f64 {
        s.exterior.signed_area().abs() - s.holes.iter().map(|h| h.signed_area().abs()).sum::<f64>()
    };
    let make_stroke_shapes = |pts: &[Point], radius: f64, color: Color32| -> Vec<FillShape> {
        let raw = generate_brush_outline_at_zoom(pts, radius, 1.0);
        build_smoothed_stroke_shapes_at_zoom(&raw, color, 1.0)
    };

    let mut app = Is2dApp::default();
    for i in 0..7 {
        let y = 100.0 + i as f64 * 40.0;
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(80.0, y), Point::new(480.0, y)],
                radius: 5.0,
                color: Color32::RED,
            },
        ));
    }
    for j in 0..7 {
        let x = 100.0 + j as f64 * 60.0;
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(x, 80.0), Point::new(x, 360.0)],
                radius: 5.0,
                color: Color32::RED,
            },
        ));
    }
    let orig_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
    assert_eq!(orig_shapes.len(), 1);
    let orig_shape = orig_shapes[0].clone();
    assert!(
        orig_shape.holes.len() >= 30,
        "格子図形が 30 個以上の穴を持つこと: {}",
        orig_shape.holes.len()
    );

    let corner_pts = vec![Point::new(85.0, 90.0), Point::new(115.0, 115.0)];

    // 1. 結合（Union）：片隅に同じ色の短い線を足す
    {
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: corner_pts.clone(),
                radius: 5.0,
                color: Color32::RED,
            },
        ));
        let res = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        let incoming = make_stroke_shapes(&corner_pts, 5.0, Color32::RED);
        let direct = boolean_operation(
            std::slice::from_ref(&orig_shape),
            &incoming,
            BoolOp::Union,
            Color32::RED,
        );
        assert_eq!(res.len(), 1);
        assert_eq!(direct.len(), 1);
        assert_eq!(res[0].holes.len(), direct[0].holes.len());
        let a_res = shape_net_area(&res[0]);
        let a_dir = shape_net_area(&direct[0]);
        assert!(
            ((a_res - a_dir).abs() / a_dir) <= 1e-6,
            "Union 面積が一致すること: {a_res} vs {a_dir}"
        );
        let near = union_bounding_boxes(&incoming).inflate(1.5, 1.5);
        let far_holes: Vec<_> = orig_shape
            .holes
            .iter()
            .filter(|h| !rects_overlap(h.bounding_box(), near))
            .collect();
        assert!(!far_holes.is_empty());
        for fh in far_holes {
            assert!(
                res[0]
                    .holes
                    .iter()
                    .any(|h| std::sync::Arc::ptr_eq(&h.segments, &fh.segments)),
                "遠い穴の Contour が Arc::ptr_eq で保たれていること"
            );
        }
        assert!(apply(&mut app, Action::Undo));
    }

    // 2. 切り抜き（消しゴム）：片隅を消しゴムで短く消す
    {
        assert!(apply(
            &mut app,
            Action::EraseStroke {
                points: corner_pts.clone(),
                radius: 6.0,
            },
        ));
        let res = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        let eraser = make_stroke_shapes(&corner_pts, 6.0, Color32::WHITE);
        let direct = boolean_operation(
            std::slice::from_ref(&orig_shape),
            &eraser,
            BoolOp::Difference,
            Color32::RED,
        );
        assert_eq!(res.len(), direct.len());
        assert_eq!(
            res.iter().map(|s| s.holes.len()).sum::<usize>(),
            direct.iter().map(|s| s.holes.len()).sum::<usize>()
        );
        let a_res: f64 = res.iter().map(&shape_net_area).sum();
        let a_dir: f64 = direct.iter().map(&shape_net_area).sum();
        assert!(
            ((a_res - a_dir).abs() / a_dir) <= 1e-6,
            "Difference 面積が一致すること: {a_res} vs {a_dir}"
        );
        let near = union_bounding_boxes(&eraser).inflate(1.5, 1.5);
        for fh in orig_shape
            .holes
            .iter()
            .filter(|h| !rects_overlap(h.bounding_box(), near))
        {
            assert!(
                res.iter()
                    .flat_map(|s| s.holes.iter())
                    .any(|h| std::sync::Arc::ptr_eq(&h.segments, &fh.segments)),
                "消しゴム後も遠い穴の Contour が Arc::ptr_eq で保たれていること"
            );
        }
        assert!(apply(&mut app, Action::Undo));
    }

    // 3. 別の色：片隅に別の色の短い線を描く
    {
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: corner_pts.clone(),
                radius: 6.0,
                color: Color32::BLUE,
            },
        ));
        let red_res: Vec<_> = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes
            .iter()
            .filter(|s| s.color == Color32::RED)
            .collect();
        let diff_stroke = make_stroke_shapes(&corner_pts, 6.0, Color32::BLUE);
        let direct = boolean_operation(
            std::slice::from_ref(&orig_shape),
            &diff_stroke,
            BoolOp::Difference,
            Color32::RED,
        );
        assert_eq!(red_res.len(), direct.len());
        assert_eq!(
            red_res.iter().map(|s| s.holes.len()).sum::<usize>(),
            direct.iter().map(|s| s.holes.len()).sum::<usize>()
        );
        let a_res: f64 = red_res.iter().map(|s| shape_net_area(s)).sum();
        let a_dir: f64 = direct.iter().map(&shape_net_area).sum();
        assert!(
            ((a_res - a_dir).abs() / a_dir) <= 1e-6,
            "異色ストローク後の元色面積が Difference と一致すること: {a_res} vs {a_dir}"
        );
        let near = union_bounding_boxes(&diff_stroke).inflate(1.5, 1.5);
        for fh in orig_shape
            .holes
            .iter()
            .filter(|h| !rects_overlap(h.bounding_box(), near))
        {
            assert!(
                red_res
                    .iter()
                    .flat_map(|s| s.holes.iter())
                    .any(|h| std::sync::Arc::ptr_eq(&h.segments, &fh.segments)),
                "異色ストローク後も遠い穴の Contour が Arc::ptr_eq で保たれていること"
            );
        }
        assert!(apply(&mut app, Action::Undo));
    }

    // 4. 分かれる切り抜き：消しゴムで真ん中から2つに切る（左右どちらにも遠い穴がある）
    {
        let split_pts = vec![Point::new(280.0, 50.0), Point::new(280.0, 400.0)];
        assert!(apply(
            &mut app,
            Action::EraseStroke {
                points: split_pts.clone(),
                radius: 10.0,
            },
        ));
        let res = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        let eraser = make_stroke_shapes(&split_pts, 10.0, Color32::WHITE);
        let direct = boolean_operation(
            std::slice::from_ref(&orig_shape),
            &eraser,
            BoolOp::Difference,
            Color32::RED,
        );
        assert_eq!(res.len(), 2);
        assert_eq!(direct.len(), 2);
        assert_eq!(
            res.iter().map(|s| s.holes.len()).sum::<usize>(),
            direct.iter().map(|s| s.holes.len()).sum::<usize>()
        );
        let a_res: f64 = res.iter().map(&shape_net_area).sum();
        let a_dir: f64 = direct.iter().map(&shape_net_area).sum();
        assert!(
            ((a_res - a_dir).abs() / a_dir) <= 1e-6,
            "分割切り抜き後の合計面積が一致すること: {a_res} vs {a_dir}"
        );
        let near = union_bounding_boxes(&eraser).inflate(1.5, 1.5);
        let far_holes: Vec<_> = orig_shape
            .holes
            .iter()
            .filter(|h| !rects_overlap(h.bounding_box(), near))
            .collect();
        assert!(far_holes.iter().any(|h| h.bounding_box().x1 < 260.0));
        assert!(far_holes.iter().any(|h| h.bounding_box().x0 > 300.0));
        for fh in far_holes {
            let pt = fh.segments[0].p0;
            let owner_piece = res
                .iter()
                .find(|piece| {
                    piece
                        .holes
                        .iter()
                        .any(|h| std::sync::Arc::ptr_eq(&h.segments, &fh.segments))
                })
                .expect("どの遠い穴も分かれた図形のどちらかに付いていること");
            assert!(
                owner_piece.bounding_box().inflate(1e-6, 1e-6).contains(pt),
                "遠い穴がそれを含む側の図形に付いていること"
            );
        }
        assert!(apply(&mut app, Action::Undo));
    }

    // 5. 変わらないとき：塗られた所の中に同じ色の短い線を描く → shape_id が変わらない
    {
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(180.0, 100.0), Point::new(200.0, 100.0)],
                radius: 2.0,
                color: Color32::RED,
            },
        ));
        let res = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].shape_id, orig_shape.shape_id);
        assert_eq!(res[0].holes.len(), orig_shape.holes.len());
    }
}

#[test]
fn test_resolve_deformed_shape_and_move_vertex_preserve_untouched_hole_contours() {
    let mut app = Is2dApp::default();
    for i in 0..7 {
        let y = 100.0 + i as f64 * 40.0;
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(80.0, y), Point::new(480.0, y)],
                radius: 5.0,
                color: Color32::RED,
            },
        ));
    }
    for j in 0..7 {
        let x = 100.0 + j as f64 * 60.0;
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(x, 80.0), Point::new(x, 360.0)],
                radius: 5.0,
                color: Color32::RED,
            },
        ));
    }
    let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes[0].clone();
    assert!(orig_shape.holes.len() >= 30);

    // 1. 1つの穴を少し変形して resolve_deformed_shape を呼ぶと、変形していない穴は入力の Contour と同じもの（ptr_eq）が残ること
    let mut deformed = orig_shape.clone();
    deformed.edit_contour(1, |contour| {
        let segs = contour.segments_mut();
        let last_idx = segs.len() - 1;
        let delta = kurbo::Vec2::new(1.0, 1.0);
        segs[0].p0 += delta;
        segs[0].p1 += delta * 0.5;
        segs[last_idx].p2 += delta;
        segs[last_idx].p1 += delta * 0.5;
        true
    });
    let resolved = resolve_deformed_shape(&deformed);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].holes.len(), orig_shape.holes.len());
    for untouched_hole in &orig_shape.holes[1..] {
        assert!(
            resolved[0]
                .holes
                .iter()
                .any(|h| std::sync::Arc::ptr_eq(&h.segments, &untouched_hole.segments)),
            "resolve_deformed_shape 後も変形していない穴が入力の Contour と ptr_eq で共有されていること"
        );
    }

    // 2. Action::MoveVertexFromTo を穴の多い図形に行ったとき、触っていない穴が操作前の Contour と同じもの（ptr_eq）として残ること
    let v_from = orig_shape.holes[0].segments[0].p0;
    let v_to = v_from + kurbo::Vec2::new(1.0, 1.0);
    assert!(apply(
        &mut app,
        Action::MoveVertexFromTo {
            from: v_from,
            to: v_to,
        },
    ));
    let after_shape = &app.doc.root.layers[0].keyframes[0].cel.shapes[0];
    assert_eq!(after_shape.holes.len(), orig_shape.holes.len());
    for untouched_hole in &orig_shape.holes[1..] {
        assert!(
            after_shape
                .holes
                .iter()
                .any(|h| std::sync::Arc::ptr_eq(&h.segments, &untouched_hole.segments)),
            "MoveVertexFromTo 後も触っていない穴が操作前の Contour と ptr_eq で共有されていること"
        );
    }
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
fn test_format_v2_contour_rounding_idempotence_and_unrounded_symbol_xform() {
    use crate::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let mut rng = fastrand::Rng::with_seed(12345);
    for i in 0..8 {
        let color = Color32::from_rgb((40 + i * 25) as u8, (80 + i * 15) as u8, 190);
        let mut pts = Vec::with_capacity(14);
        let mut x = 100.0 + i as f64 * 45.1234567;
        let mut y = 120.0 + (i % 3) as f64 * 55.9876543;
        for step in 0..14 {
            x += 11.1234567 + rng.f64() * 9.8765432;
            y += ((step as f64 * 0.63).sin() * 21.3456789) + (rng.f64() - 0.5) * 7.654321;
            pts.push(Point::new(x, y));
        }
        assert!(apply(
            &mut app,
            Action::PaintStroke {
                points: pts,
                radius: 5.5,
                color,
            },
        ));
    }

    let orig_shapes = app.doc.root.layers[0].keyframes[0].cel.shapes.clone();
    assert!(!orig_shapes.is_empty());

    let path1 = std::env::temp_dir().join(format!("is2d_fmt_v2_1_{}.is2d", std::process::id()));
    let path2 = std::env::temp_dir().join(format!("is2d_fmt_v2_2_{}.is2d", std::process::id()));
    let path3 = std::env::temp_dir().join(format!("is2d_fmt_v2_3_{}.is2d", std::process::id()));

    project_io::save_project_to_path(&mut app, &path1).unwrap();
    let json1 = std::fs::read_to_string(&path1).unwrap();

    // 3. 形式：保存したファイルの輪郭が、数の平らな配列で、数の個数が区間の数の4倍であること
    let parsed_val: serde_json::Value = serde_json::from_str(&json1).unwrap();
    assert_eq!(parsed_val["version"], 2);
    let saved_shapes_json = parsed_val["timeline"]["layers"][0]["keyframes"][0]["shapes"]
        .as_array()
        .unwrap();
    assert_eq!(saved_shapes_json.len(), orig_shapes.len());
    for (s_json, orig_s) in saved_shapes_json.iter().zip(orig_shapes.iter()) {
        let ext_arr = s_json["exterior"]["segments"].as_array().unwrap();
        assert_eq!(ext_arr.len(), orig_s.exterior.segments.len() * 4);
        assert!(ext_arr.iter().all(|v| v.is_number()));
        let holes_arr = s_json["holes"].as_array().unwrap();
        assert_eq!(holes_arr.len(), orig_s.holes.len());
        for (h_json, orig_h) in holes_arr.iter().zip(orig_s.holes.iter()) {
            let h_seg_arr = h_json["segments"].as_array().unwrap();
            assert_eq!(h_seg_arr.len(), orig_h.segments.len() * 4);
            assert!(h_seg_arr.iter().all(|v| v.is_number()));
        }
    }

    // 1. 丸めの幅：保存して開くと区間の数は同じで、どの点（p0・p1・p2）も元から 0.0005 px 以内。
    //    開いた図形のどの輪郭も、区間の終わりの点が次の区間の始まりの点と同じであること
    let mut app2 = Is2dApp::default();
    project_io::open_project_in_tab(&mut app2, &path1, None).unwrap();
    let loaded_shapes = &app2.doc.root.layers[0].keyframes[0].cel.shapes;
    assert_eq!(loaded_shapes.len(), orig_shapes.len());

    let check_contour = |orig: &vector_engine::Contour, loaded: &vector_engine::Contour| {
        assert_eq!(loaded.segments.len(), orig.segments.len());
        let n = loaded.segments.len();
        for i in 0..n {
            let os = orig.segments[i];
            let ls = loaded.segments[i];
            assert!((ls.p0.x - os.p0.x).abs() <= 0.0005 + 1e-9);
            assert!((ls.p0.y - os.p0.y).abs() <= 0.0005 + 1e-9);
            assert!((ls.p1.x - os.p1.x).abs() <= 0.0005 + 1e-9);
            assert!((ls.p1.y - os.p1.y).abs() <= 0.0005 + 1e-9);
            assert!((ls.p2.x - os.p2.x).abs() <= 0.0005 + 1e-9);
            assert!((ls.p2.y - os.p2.y).abs() <= 0.0005 + 1e-9);
            assert_eq!(
                ls.p2,
                loaded.segments[(i + 1) % n].p0,
                "区間の終わりの点 p2 が次の区間の始まりの点 p0 と完全に同じであること"
            );
        }
    };
    for (orig_s, load_s) in orig_shapes.iter().zip(loaded_shapes.iter()) {
        check_contour(&orig_s.exterior, &load_s.exterior);
        assert_eq!(load_s.holes.len(), orig_s.holes.len());
        for (oh, lh) in orig_s.holes.iter().zip(load_s.holes.iter()) {
            check_contour(oh, lh);
        }
    }

    // 2. 保存し直しても変わらない：開いて保存（2回目）し、もう一度開いて保存（3回目）すると、
    //    2回目と3回目のファイルの中身が1バイトも違わないこと
    project_io::save_project_to_path(&mut app2, &path2).unwrap();
    let mut app3 = Is2dApp::default();
    project_io::open_project_in_tab(&mut app3, &path2, None).unwrap();
    project_io::save_project_to_path(&mut app3, &path3).unwrap();
    let bytes2 = std::fs::read(&path2).unwrap();
    let bytes3 = std::fs::read(&path3).unwrap();
    let _ = std::fs::remove_file(&path1);
    let _ = std::fs::remove_file(&path2);
    let _ = std::fs::remove_file(&path3);
    assert_eq!(
        bytes2, bytes3,
        "2回目の保存と3回目の保存でファイルの中身が1バイトも違わないこと"
    );

    // 4. 版 1 のファイル：版 1 の形の輪郭を持つ JSON を開くとエラーが返り、タブは増えず、落ちないこと
    let v1_json = r#"{
        "version": 1,
        "stage_size": [800.0, 450.0],
        "next_symbol_id": 1,
        "next_group_id": 1,
        "timeline": {
            "fps": 24,
            "total_frames": 1,
            "active_layer": 0,
            "current_frame": 0,
            "next_layer_id": 2,
            "layers": [{
                "id": 1,
                "name": "Layer 1",
                "visible": true,
                "locked": false,
                "frame_count": 1,
                "keyframes": [{
                    "start_frame": 0,
                    "tween": false,
                    "tween_rotate": "Auto",
                    "shapes": [{
                        "color": [255, 0, 0, 255],
                        "exterior": {
                            "segments": [
                                [[100.0, 100.0], [150.0, 100.0], [200.0, 100.0]],
                                [[200.0, 100.0], [200.0, 150.0], [200.0, 200.0]],
                                [[200.0, 200.0], [150.0, 150.0], [100.0, 100.0]]
                            ]
                        },
                        "holes": [],
                        "layer_id": 1,
                        "group_path": []
                    }],
                    "instances": []
                }]
            }]
        },
        "library": []
    }"#;
    let v1_path = std::env::temp_dir().join(format!("is2d_fmt_v1_err_{}.is2d", std::process::id()));
    std::fs::write(&v1_path, v1_json).unwrap();
    let tabs_before = app3.tabs.len();
    let v1_res = project_io::open_project_in_tab(&mut app3, &v1_path, None);
    let _ = std::fs::remove_file(&v1_path);
    assert!(
        v1_res.is_err(),
        "版 1 の輪郭形式を持つファイルを開くとエラーが返ること"
    );
    assert_eq!(
        app3.tabs.len(),
        tabs_before,
        "版 1 のファイルの読み込み失敗でタブが増えないこと"
    );

    // 5. ほかの数は丸めない：拡大・回転したシンボルのインスタンスを持つ作品を保存して開くと、
    //    インスタンスの変換の行列が元と同じ（丸めていない）であること
    let mut sym_app = Is2dApp::default();
    assert!(apply(
        &mut sym_app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(180.0, 140.0)],
            radius: 8.0,
            color: Color32::RED,
        },
    ));
    sym_app.session.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut sym_app).unwrap();
    assert!(apply(
        &mut sym_app,
        Action::ScaleSelection {
            sx: 1.3456789012345,
            sy: 0.8765432109876,
        },
    ));
    assert!(apply(
        &mut sym_app,
        Action::RotateSelection {
            angle_rad: 0.4321098765432,
        },
    ));
    let orig_coeffs = sym_app.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .xform
        .as_coeffs();
    let sym_path =
        std::env::temp_dir().join(format!("is2d_fmt_v2_sym_{}.is2d", std::process::id()));
    project_io::save_project_to_path(&mut sym_app, &sym_path).unwrap();
    let mut sym_loaded = Is2dApp::default();
    project_io::open_project_in_tab(&mut sym_loaded, &sym_path, None).unwrap();
    let _ = std::fs::remove_file(&sym_path);
    let loaded_coeffs = sym_loaded.doc.root.layers[0].keyframes[0].cel.instances[0]
        .binding
        .xform
        .as_coeffs();
    for (a, b) in orig_coeffs.iter().zip(loaded_coeffs.iter()) {
        assert!(
            (a - b).abs() < 1e-13,
            "シンボルインスタンスの変換行列は小数第3位に丸められず元の精度が保たれること: {a} vs {b}"
        );
    }
}

#[test]
fn test_video_export_parallel_render_frames_and_without_mesh() {
    use crate::canvas_tool::View;
    use crate::doc_actions::{apply, Action};
    use crate::symbol::SymbolEvalCtx;
    use crate::vector_engine::Contour;
    use crate::video_export::{
        export_dimensions, render_frame, render_frames_in_order, VideoExportOutcome,
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    // コマごとに絵が違う作品を作る（クラシックトゥイーンで動くシンボル、10 コマ）
    let mut app = Is2dApp::default();
    let center = View::DEFAULT_STAGE_CENTER;
    let p00 = Point::new(center.x - 320.0, center.y - 30.0);
    let p10 = Point::new(center.x - 260.0, center.y - 30.0);
    let p11 = Point::new(center.x - 260.0, center.y + 30.0);
    let p01 = Point::new(center.x - 320.0, center.y + 30.0);
    let rect_shape = FillShape::new(
        Color32::from_rgb(230, 50, 50),
        Contour::new(vec![
            QuadBez::new(p00, p00.midpoint(p10), p10),
            QuadBez::new(p10, p10.midpoint(p11), p11),
            QuadBez::new(p11, p11.midpoint(p01), p01),
            QuadBez::new(p01, p01.midpoint(p00), p00),
        ]),
        Vec::new(),
    );
    put_cel_shapes(&mut app, 0, 0, vec![rect_shape]);
    app.tab_mut().session.set_selected_indices(vec![0]);
    assert!(apply(&mut app, Action::ConvertToSymbol));
    assert!(apply(&mut app, Action::ToggleTween));
    for _ in 0..9 {
        assert!(apply(&mut app, Action::ExtendFrame));
    }
    assert!(apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 9,
        },
    ));
    assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: true },));
    app.tab_mut().session.set_selected_indices(vec![0]);
    assert!(apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(540.0, 0.0),
        },
    ));
    assert_eq!(app.tab().doc.root.total_frames, 10);

    let doc = app.tab().committed_document_copy();
    let (w, h) = export_dimensions(doc.stage_size);
    let expected_frames: Vec<Vec<u8>> = (0..10).map(|f| render_frame(&doc, f, w, h)).collect();
    for f in 1..10 {
        assert_ne!(
            expected_frames[f - 1],
            expected_frames[f],
            "コマごとに絵が異なること"
        );
    }

    // 1. スレッドの数を 1・3・4・16 にして render_frames_in_order を呼ぶ
    for threads in [1usize, 3, 4, 16] {
        let cancel_flag = AtomicBool::new(false);
        let mut received_indices = Vec::new();
        let mut received_frames = Vec::new();
        let outcome = render_frames_in_order(&doc, (w, h), threads, &cancel_flag, |frame, rgba| {
            received_indices.push(frame);
            received_frames.push(rgba.to_vec());
            Ok(())
        });
        assert_eq!(outcome, VideoExportOutcome::Completed);
        assert_eq!(received_indices, (0..10).collect::<Vec<usize>>());
        assert_eq!(received_frames, expected_frames);
    }

    // 2. 3本で、4 コマ目（番号 3）を渡したところで中止の合図を立てる
    {
        let cancel_flag = AtomicBool::new(false);
        let mut pushed_count = 0usize;
        let outcome = render_frames_in_order(&doc, (w, h), 3, &cancel_flag, |frame, _rgba| {
            pushed_count += 1;
            if frame == 3 {
                cancel_flag.store(true, Ordering::Release);
            }
            Ok(())
        });
        assert_eq!(outcome, VideoExportOutcome::Cancelled);
        assert!(
            pushed_count <= 6,
            "4コマ目（番号3）で中止したとき渡ったコマは6コマ以下であること: {pushed_count}"
        );
    }

    // 3. 3本で、5 コマ目（番号 4）で push がエラーを返す
    {
        let cancel_flag = AtomicBool::new(false);
        let mut pushed_count = 0usize;
        let outcome = render_frames_in_order(&doc, (w, h), 3, &cancel_flag, |frame, _rgba| {
            pushed_count += 1;
            if frame == 4 {
                return Err("mock push error".to_owned());
            }
            Ok(())
        });
        assert_eq!(
            outcome,
            VideoExportOutcome::Failed("mock push error".to_owned())
        );
        assert_eq!(pushed_count, 5);
    }

    // 4. 文書の図形はメッシュを持たず、書き出し用の展開はメッシュを持たず、画面用の展開はメッシュを持つ
    for sym in &doc.library {
        for layer in &sym.timeline.layers {
            for kf in &layer.keyframes {
                for s in kf.cel.shapes.iter() {
                    assert!(!s.has_cached_fill_mesh());
                }
            }
        }
    }
    let layer = &doc.root.layers[0];
    let without_mesh_shapes = layer.evaluate_shapes_at(
        (2, 2),
        layer.id,
        SymbolEvalCtx::new(&doc.library).without_mesh(),
    );
    assert!(!without_mesh_shapes.is_empty());
    assert!(
        without_mesh_shapes
            .iter()
            .all(|s| !s.has_cached_fill_mesh()),
        "書き出し用（without_mesh）の展開の図形はどれもメッシュを持たないこと"
    );

    let with_mesh_shapes =
        layer.evaluate_shapes_at((2, 2), layer.id, SymbolEvalCtx::new(&doc.library));
    assert!(!with_mesh_shapes.is_empty());
    assert!(
        with_mesh_shapes.iter().all(|s| s.has_cached_fill_mesh()),
        "画面用（SymbolEvalCtx::new）の展開の図形はどれもメッシュを持つこと"
    );
}

#[test]
fn test_reshape_edge_with_floating_is_noop_and_empty_v1_file_rejected() {
    use crate::doc_actions::{apply, Action};
    use kurbo::ParamCurve;

    // 1. Floating があるときの輪郭の変形は何もしない
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 200.0), Point::new(300.0, 200.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    ));
    let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes[0].clone();
    let orig_id = orig_shape.shape_id;
    let orig_bbox = orig_shape.bounding_box();
    let orig_seg_count = orig_shape.exterior.segments.len();
    let orig_seg = orig_shape.exterior.segments[0];
    let grab_t = 0.5;
    let grab_pos = orig_seg.eval(grab_t);

    assert!(apply(
        &mut app,
        Action::SelectMarquee {
            p0: Point::new(200.0, 150.0),
            p1: Point::new(350.0, 250.0),
            additive: false,
        },
    ));
    assert!(app.session.floating().is_some());
    let undo_before = app.history.undo_stack.len();

    assert!(!apply(
        &mut app,
        Action::ReshapeEdge {
            shape_id: orig_id,
            contour_idx: 0,
            seg_idx: 0,
            grab_t,
            grab_pos,
            target_pos: grab_pos + kurbo::Vec2::new(0.0, 50.0),
            orig_seg,
        },
    ));
    assert_eq!(app.history.undo_stack.len(), undo_before);
    assert!(app.session.floating().is_some());
    let after_shape = &app.doc.root.layers[0].keyframes[0].cel.shapes[0];
    assert_eq!(after_shape.bounding_box(), orig_bbox);
    assert_eq!(after_shape.exterior.segments.len(), orig_seg_count);

    // 2. 図形のない版 1 のファイルを開くと「未対応のプロジェクトバージョン」エラーが返り、タブが増えない
    let empty_v1_doc = project_io::ProjectDocument {
        version: 1,
        stage_size: [800.0, 450.0],
        timeline: (&timeline::Timeline::default()).into(),
        library: Vec::new(),
        next_symbol_id: 1,
        next_group_id: 1,
        recovery_source: None,
    };
    let v1_path = std::env::temp_dir().join(format!("is2d_empty_v1_{}.is2d", std::process::id()));
    std::fs::write(&v1_path, serde_json::to_string(&empty_v1_doc).unwrap()).unwrap();
    let tabs_before = app.tabs.len();
    let err = project_io::open_project_in_tab(&mut app, &v1_path, None)
        .expect_err("図形のない版 1 のファイルも開けないこと");
    let _ = std::fs::remove_file(&v1_path);
    assert!(
        err.contains("未対応のプロジェクトバージョン"),
        "想定外のエラー: {err}"
    );
    assert_eq!(app.tabs.len(), tabs_before);
}

#[test]
fn test_wavy_stroke_fully_covering_small_dot_erases_cuts_or_merges_it() {
    use crate::doc_actions::{apply, Action};
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
                .shapes
                .iter()
                .map(|s| s.color)
                .collect();
            assert_eq!(colors, expected, "{label} (x={x})");
        }
    }
}

#[test]
fn test_cross_tab_repaste_after_source_symbol_edit_imports_current_content() {
    use crate::doc_actions::{apply, Action};

    let ctx = egui::Context::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 90, 230);
    let green = Color32::from_rgb(40, 180, 90);
    let yellow = Color32::from_rgb(230, 200, 40);
    let lib_names =
        |app: &Is2dApp| -> Vec<String> { app.doc.library.iter().map(|s| s.name.clone()).collect() };

    // 1. タブ A で赤の線をシンボル S にしてコピーし、新しいタブ B に貼る。B のライブラリは ["Symbol 1"]
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(50.0, 50.0), Point::new(120.0, 50.0)],
            radius: 8.0,
            color: red,
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let sym_s_id = app.doc.library[0].id;
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(lib_names(&app), vec!["Symbol 1"]);

    // 2. A で S に入って青の線を足し、出て、全部選んでコピーし、B に貼る。
    //    B のライブラリは ["Symbol 1", "Symbol 1 (2)"] になり、B の画面に青がある
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(
        &mut app,
        Action::EnterSymbolFromLibrary { sym_id: sym_s_id },
    ));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(-30.0, 0.0), Point::new(30.0, 0.0)],
            radius: 8.0,
            color: blue,
        },
    ));
    assert!(apply(&mut app, Action::ExitSymbolTimeline));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(lib_names(&app), vec!["Symbol 1", "Symbol 1 (2)"]);
    assert!(
        app.scene().iter().any(|s| s.color == blue),
        "B の画面に編集後の青い線が表示されていること"
    );

    // 3. B でもう一度貼っても、ライブラリは2つのまま（中身が同じなら増えない）
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(lib_names(&app).len(), 2);

    // 4. A で緑の線を子シンボル C にし、それをさらに親シンボル P にしてコピーし、B に貼る（B のライブラリは4つ）。
    //    A で C に入って黄の線を足して出て、P を選んでコピーし、B に貼ると、ライブラリは6つになり、B の画面に黄がある
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(&mut app, Action::ClearActiveFrame));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(150.0, 150.0), Point::new(220.0, 150.0)],
            radius: 8.0,
            color: green,
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let sym_c_id = app.doc.library.last().unwrap().id;
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let sym_p_id = app.doc.library.last().unwrap().id;
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 4);

    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(
        &mut app,
        Action::EnterSymbolFromLibrary { sym_id: sym_c_id },
    ));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(-20.0, 10.0), Point::new(20.0, 10.0)],
            radius: 8.0,
            color: yellow,
        },
    ));
    assert!(apply(&mut app, Action::ExitSymbolTimeline));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 6);
    assert!(
        app.scene().iter().any(|s| s.color == yellow),
        "子シンボル C に足した黄色い線が B の画面に表示されていること"
    );

    // 5. A で C にもう一度青の線を足して出る。A のルートに C を置いてコピーし、B に貼る（7つ）。
    //    その後で A の P を選んでコピーし、B に貼ると、ライブラリは8つになり（P だけが新しく入る）、
    //    新しい P のインスタンスはどれも、さっきの新しい C を指している
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(
        &mut app,
        Action::EnterSymbolFromLibrary { sym_id: sym_c_id },
    ));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(-25.0, -10.0), Point::new(25.0, -10.0)],
            radius: 6.0,
            color: blue,
        },
    ));
    assert!(apply(&mut app, Action::ExitSymbolTimeline));
    assert!(apply(&mut app, Action::PlaceSymbol { sym_id: sym_c_id }));
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 7);
    let new_c_in_b = app.doc.library.last().unwrap().id;

    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    let p_gid = app.doc.root.layers[0].keyframes[0]
        .cel
        .instances
        .iter()
        .find(|inst| inst.binding.symbol_id == sym_p_id)
        .unwrap()
        .binding
        .instance_gid;
    app.session.select_symbol_instance_by_gid(p_gid);
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 8);
    let new_p_in_b = app.doc.library.last().unwrap();
    assert!(
        new_p_in_b
            .timeline
            .instances()
            .all(|inst| inst.binding.symbol_id == new_c_in_b),
        "新しい P のインスタンスはすべて先に移植された新しい C を指していること"
    );

    // 6. B で関係のないシンボル（最初の Symbol 1）を削除する（7つ）。もう一度貼っても7つのまま
    let first_sym_id_in_b = app.doc.library[0].id;
    assert_eq!(app.doc.library[0].name, "Symbol 1");
    assert!(apply(
        &mut app,
        Action::DeleteSymbol {
            sym_id: first_sym_id_in_b,
        },
    ));
    assert_eq!(app.doc.library.len(), 7);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library.len(), 7);
}

#[test]
fn test_adding_base_shape_to_floating_selection_keeps_floating_piece_on_top() {
    use crate::doc_actions::{apply, Action};
    use crate::vector_engine::Contour;

    let make_rect = |x0: f64, y0: f64, x1: f64, y1: f64, color: Color32| -> FillShape {
        let p00 = Point::new(x0, y0);
        let p10 = Point::new(x1, y0);
        let p11 = Point::new(x1, y1);
        let p01 = Point::new(x0, y1);
        FillShape::new(
            color,
            Contour::new(vec![
                QuadBez::new(p00, p00.midpoint(p10), p10),
                QuadBez::new(p10, p10.midpoint(p11), p11),
                QuadBez::new(p11, p11.midpoint(p01), p01),
                QuadBez::new(p01, p01.midpoint(p00), p00),
            ]),
            Vec::new(),
        )
    };
    let shape_area = |s: &FillShape| -> f64 {
        let holes_area: f64 = s.holes.iter().map(|h| h.signed_area().abs()).sum();
        s.exterior.signed_area().abs() - holes_area
    };

    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 80, 220);

    let cases = [
        (
            "Shift+クリックで選び足して移動",
            vec![
                Action::ToggleSelectionAt { scene_idx: 0 },
                Action::FinishMoveSelectionDrag {
                    total_delta: kurbo::Vec2::new(10.0, 10.0),
                    alt_duplicate: false,
                },
            ],
            Point::new(260.0, 210.0),
        ),
        (
            "Shift+矩形で一部を選び足す",
            vec![Action::SelectMarquee {
                p0: Point::new(150.0, 120.0),
                p1: Point::new(350.0, 280.0),
                additive: true,
            }],
            Point::new(250.0, 200.0),
        ),
    ];

    for (label, add_actions, overlap_pt) in cases {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                make_rect(100.0, 100.0, 300.0, 300.0, red),
                make_rect(400.0, 100.0, 500.0, 200.0, blue),
            ],
        );

        assert!(apply(
            &mut app,
            Action::SelectSingleAt {
                pt: Point::new(450.0, 150.0),
            },
        ));
        assert!(apply(
            &mut app,
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(-200.0, 50.0),
            },
        ));

        run(&mut app, &add_actions);

        let top_color = app
            .scene()
            .iter()
            .rev()
            .find(|s| s.contains_point(overlap_pt))
            .map(|s| s.color);
        assert_eq!(
            top_color,
            Some(blue),
            "{label}: 選び足した後の重なり部分の一番上が青であること"
        );

        assert!(apply(&mut app, Action::Deselect));
        let cel_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes;
        let blue_area: f64 = cel_shapes
            .iter()
            .filter(|s| s.color == blue)
            .map(shape_area)
            .sum();
        let red_area: f64 = cel_shapes
            .iter()
            .filter(|s| s.color == red)
            .map(shape_area)
            .sum();
        assert!(
            (blue_area - 10_000.0).abs() < 10.0,
            "{label}: 確定後の青の面積が 10,000 であること (actual={blue_area})"
        );
        assert!(
            (red_area - 30_000.0).abs() < 10.0,
            "{label}: 確定後の赤の面積が 30,000 であること (actual={red_area})"
        );
    }
}

#[test]
fn test_merging_floating_pieces_layer_by_layer_matches_merging_all_at_once() {
    use crate::canvas_tool::{merge_floating_into_base_shapes, Floating};
    use crate::vector_engine::next_shape_id;

    let mut rng = fastrand::Rng::with_seed(7);
    let palette = [
        Color32::BLACK,
        Color32::from_rgb(220, 50, 50),
        Color32::from_rgb(50, 80, 220),
    ];

    let mut base_shapes = Vec::new();
    for lid in 1u64..=3 {
        for _ in 0..25 {
            let color = palette[rng.usize(..palette.len())];
            let radius = 1.5 + rng.f64() * 4.5;
            let p0 = Point::new(80.0 + rng.f64() * 340.0, 60.0 + rng.f64() * 240.0);
            let p1 = Point::new(80.0 + rng.f64() * 340.0, 60.0 + rng.f64() * 240.0);
            let pts: Vec<Point> = (0..=16)
                .map(|i| {
                    let t = i as f64 / 16.0;
                    p0.lerp(p1, t) + KVec2::new(0.0, (t * std::f64::consts::TAU).sin() * 8.0)
                })
                .collect();
            let raw = generate_brush_outline(&pts, radius);
            let stroke_shapes: Vec<FillShape> = build_smoothed_stroke_shapes(&raw, color)
                .into_iter()
                .map(|s| s.with_scope(lid, &[]))
                .collect();
            merge_shapes_into_canvas(&mut base_shapes, stroke_shapes);
        }
    }
    let orig_count = base_shapes.len();
    assert!(orig_count > 10);

    let pieces: Vec<FillShape> = base_shapes
        .iter()
        .map(|s| {
            let mut copy = s.clone();
            copy.shape_id = next_shape_id();
            copy.translate(KVec2::new(23.0, 17.0));
            copy
        })
        .collect();

    let mut all_at_once = base_shapes.clone();
    merge_shapes_into_canvas(&mut all_at_once, pieces.clone());
    all_at_once.sort_by_key(|s| s.layer_id);

    let mut layer_by_layer = base_shapes;
    merge_floating_into_base_shapes(
        &mut layer_by_layer,
        Floating {
            pieces,
            remainder: Vec::new(),
            hidden: std::collections::HashSet::new(),
            modified: true,
        },
        &[1, 2, 3],
    );

    assert!(layer_by_layer.len() > orig_count);

    let summarize = |shapes: &[FillShape]| -> Vec<(u64, Color32, i64, i64, i64, usize)> {
        shapes
            .iter()
            .map(|s| {
                let holes_area: f64 = s.holes.iter().map(|h| h.signed_area().abs()).sum();
                let area = s.exterior.signed_area().abs() - holes_area;
                let bbox = s.bounding_box();
                (
                    s.layer_id,
                    s.color,
                    (area * 100.0).round() as i64,
                    (bbox.x0 * 100.0).round() as i64,
                    (bbox.y0 * 100.0).round() as i64,
                    s.holes.len(),
                )
            })
            .collect()
    };

    assert_eq!(summarize(&layer_by_layer), summarize(&all_at_once));
}
