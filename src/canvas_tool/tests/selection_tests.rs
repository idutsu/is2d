//! 選択と浮いた片（選ぶ・持ち上げる・動かす・変形・グループ・重ね順・輪郭の変形・コピーと貼り付け）のテスト

use super::*;

// ---- 色の違う図形の境目をドラッグしても剥がれない（境目を一緒に動かす） ----

/// 範囲 `inside` の中を 1px おきに調べ、（どの図形にも塗られていない点の数, 2つ以上の図形に塗られている点の数）を返す。
/// 点は境目の上に乗らないよう、少しずらして置く
fn shared_boundary_coverage(
    app: &Is2dApp,
    inside: impl Fn(Point) -> bool,
    area: (f64, f64, f64, f64),
) -> (usize, usize) {
    let shapes = scene_summary(app).shapes.clone();
    let (mut gaps, mut overlaps) = (0, 0);
    let mut y = area.1;
    while y <= area.3 {
        let mut x = area.0;
        while x <= area.2 {
            let p = Point::new(x + 0.137, y + 0.173);
            if inside(p) {
                match shapes.iter().filter(|s| s.contains_point(p)).count() {
                    0 => gaps += 1,
                    1 => {}
                    _ => overlaps += 1,
                }
            }
            x += 1.0;
        }
        y += 1.0;
    }
    (gaps, overlaps)
}

fn red_area(app: &Is2dApp) -> f64 {
    app.scene()
        .iter()
        .filter(|s| s.color() == Color32::RED)
        .map(|s| s.contours().map(|c| c.signed_area()).sum::<f64>())
        .sum()
}

#[test]
fn test_m5_1_deform_shape_without_editable_target_preserves_undo_and_clean_state() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use kurbo::QuadBez;

    let mut app = Is2dApp::default();
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let dummy_seg = QuadBez::new(c, Point::new(c.x + 20.0, c.y), Point::new(c.x + 40.0, c.y));
    let empty_layer = app.tab().doc.root.layers[0].id;

    // 図形が存在しない状態で ReshapeEdge / MoveVertex を呼んでも Undo や未保存マークが増えないこと
    assert!(!apply(
        &mut app,
        Action::ReshapeEdge {
            layer_id: empty_layer,
            grab_t: 0.5,
            target_pos: Point::new(c.x, c.y + 30.0),
            orig_seg: dummy_seg,
            follow: true,
        },
    ));
    assert!(!apply(
        &mut app,
        Action::MoveVertex {
            layer_id: empty_layer,
            orig_vertex: c,
            target_pos: Point::new(c.x, c.y + 30.0),
            follow: false,
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
    let layer_id = app.tab().doc.root.active_cel_shapes()[0].layer_id;
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
            layer_id,
            grab_t: 0.5,
            target_pos: Point::new(c.x, c.y + 30.0),
            orig_seg: dummy_seg,
            follow: true,
        },
    ));
    assert_eq!(app.tab().history.undo_stack.len(), undo_len);
    assert!(!app.tab().history.is_dirty());
}

/// レビュー d91cd4c の N-3003・41766f9 の N-3003・N-5001：浮いた片の確定を断られた（置けない）ら、輪郭の頂点・辺・塗りを
/// つかんでも（Alt の複製・自由変形ツールでも）、ドラッグを始めず、再生も始めない
/// （浮いた片はみな選ばれたまま。矩形選択などにも落ちない）。前は、断られたのを見ずに始め、選択を空にして浮いた片の表示を
/// 消し、離しても何も起きなかった
#[test]
fn grabbing_an_outline_after_a_refused_placement_starts_nothing() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(40, 180, 90);
    put_cel_shapes(&mut app, 0, 0, make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red));
    doc_actions::apply(&mut app, doc_actions::Action::AddLayer);
    put_cel_shapes(&mut app, 1, 0, make_stroke(Point::new(100.0, 300.0), Point::new(300.0, 300.0), 25.0, blue));
    // 浮いた片に入らない、つかむための四角（レイヤー 0。浮いた片を置けなくしても、こちらはつかめる）
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    let mut both = make_stroke(Point::new(100.0, 200.0), Point::new(300.0, 200.0), 25.0, red);
    both.push(map_overlay::rect_shape(500.0, 500.0, 600.0, 600.0, green));
    put_cel_shapes(&mut app, 0, 0, both);
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Select });
    select_ops::apply_marquee_selection(&mut app, Point::new(50.0, 150.0), Point::new(350.0, 350.0), false);
    doc_actions::finish_move_selection_drag(&mut app, KVec2::new(0.0, 100.0), false);
    assert!(app.session.floating().is_some());
    // 置く先のレイヤーの1つを編集できなくする（置けない場面の代わり）
    app.tab_mut().with_active_timeline_mut(|tl, _| tl.layers[1].locked = true);
    let before = app.session.selection().clone();
    // 輪郭の頂点・辺、塗り（レビュー 41766f9 の N-3003）、Alt で塗り（複製）を選択ツールでつかむ
    let grabs = [
        (Point::new(500.0, 500.0), false),
        (Point::new(550.0, 500.0), false),
        (Point::new(550.0, 550.0), false),
        (Point::new(550.0, 550.0), true),
    ];
    for (grab, alt) in grabs {
        app.notices = Default::default();
        tool_select::start_reshape_drag(&mut app, grab, false, alt);
        assert!(app.notices.iter().next().is_some(), "{grab:?}：置けないことを知らせない");
        assert!(matches!(app.session.drag, canvas_tool::ActiveCanvasDrag::None), "{grab:?}（Alt {alt}）：断られたのにドラッグを始めた");
        assert!(app.session.floating().is_some());
        assert_eq!(app.session.selection(), &before, "{grab:?}（Alt {alt}）：浮いた片の選択が変わった");
    }
    // 自由変形ツールで、選んでいない図形をつかむ（N-3003）
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Transform });
    app.notices = Default::default();
    tool_transform::start_transform_drag(&mut app, Point::new(550.0, 550.0), false, false);
    assert!(app.notices.iter().next().is_some(), "自由変形：置けないことを知らせない");
    assert!(matches!(app.session.drag, canvas_tool::ActiveCanvasDrag::None), "自由変形：断られたのにドラッグを始めた");
    assert_eq!(app.session.selection(), &before);
    // 再生を押す（レビュー 41766f9 の N-5001：前は断られても再生を始め、止めたあとの確定で、止まったコマの絵が消えた）
    let frame = app.tab().active_timeline().current_frame;
    app.notices = Default::default();
    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing, "断られたのに再生を始めた");
    assert!(app.notices.iter().next().is_some(), "再生：置けないことを知らせない");
    assert_eq!(app.tab().active_timeline().current_frame, frame);
    assert!(app.session.floating().is_some());
    assert_eq!(app.session.selection(), &before);
}

/// S-3：`Alt` 複製は、移動先が格子の外なら何もしない（元の所に複製を残さず、元も動かさず、Undo も積まない）。置けるなら
/// 複製だけが移動先にあり、元はそのまま（グループで確かめる）
#[test]
fn alt_duplicate_places_once_or_changes_nothing() {
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(300.0, 200.0),
            25.0,
            red,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 150.0),
        Point::new(350.0, 250.0),
        false,
    );
    let count =
        |app: &Is2dApp, p: Point| app.scene().iter().filter(|s| s.contains_point(p)).count();
    let undo_len = app.history.undo_stack.len();
    doc_actions::finish_move_selection_drag(&mut app, KVec2::new(70_000.0, 0.0), true);
    assert_eq!(app.history.undo_stack.len(), undo_len);
    assert_eq!(app.scene().len(), 1);
    assert_eq!(count(&app, Point::new(200.0, 200.0)), 1);

    doc_actions::finish_move_selection_drag(&mut app, KVec2::new(0.0, 100.0), true);
    assert_eq!(app.history.undo_stack.len(), undo_len + 1);
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(count(&app, Point::new(200.0, 200.0)), 1);
    assert_eq!(count(&app, Point::new(200.0, 300.0)), 1);
}

/// 潰れた変換（逆にできない）で置いた地図は描かない（面積 0 の絵は見えない）ので、カーソル合わせは何にも
/// 合わない（地図全体の写しを作る予備の道を置かない）
#[test]
fn collapsed_placement_hits_nothing() {
    use crate::planar_map::{LayerBase, MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(235, 75, 75);
    let m = PlanarMap::default()
        .overlay(
            MapInput::Shapes(&[map_overlay::rect_shape(0.0, 0.0, 100.0, 100.0, red)]),
            MapRule::Paint(Some(red)),
        )
        .unwrap();
    let base = LayerBase::new(&m.checked().unwrap().uniquely_labeled().into_map());
    let p = Point::new(0.0, 0.0);
    for xf in [Affine::scale(0.0), Affine::scale_non_uniform(1.0, 0.0)] {
        assert_eq!(base.hit_node(p, 1000.0, xf), None);
        assert_eq!(base.hit_edge(p, 1000.0, xf), None);
    }
    // ふつうの変換なら合う
    assert_eq!(base.hit_node(p, 1.0, Affine::IDENTITY), Some(p));
}

/// Shift を押してダブルクリックすると、つながった線を今の選択に足す（前にダブルクリックで選んだ線は選んだまま。利用者の指摘：
/// 前は、ダブルクリックがいつも今の選択を外してから選んだので、前の線が選択から外れた）。Shift を押さなければ入れ替える
#[test]
fn shift_double_click_adds_connected_lines() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let ink = EdgeStroke { color: Color32::BLACK, width: 8 };
    let wave = |y: f64| -> Vec<Point> {
        (0..=200).map(|i| { let x = 100.0 + 3.0 * i as f64; Point::new(x, y + 40.0 * (x / 50.0).sin()) }).collect()
    };
    assert!(apply(&mut app, Action::PencilStroke { points: wave(200.0), stroke: ink }));
    assert!(apply(&mut app, Action::PencilStroke { points: wave(500.0), stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let lines_of = |app: &Is2dApp, upper: bool| -> Vec<u64> {
        app.scene().iter().filter(|s| s.line.is_some() && (s.bounding_box().y1 < 350.0) == upper).map(|s| s.shape_id).collect()
    };
    let (a, b) = (lines_of(&app, true), lines_of(&app, false));
    assert!(a.len() >= 2 && b.len() >= 2, "つなぎ目のある線にならなかった");
    let on = |y: f64| Point::new(400.0, y + 40.0 * (400.0f64 / 50.0).sin());
    let selected = |app: &Is2dApp, ids: &[u64]| ids.iter().filter(|id| app.session.selection().shapes.contains(id)).count();
    select_ops::handle_canvas_double_click(&mut app, on(200.0), false);
    assert_eq!(selected(&app, &a), a.len(), "1本目がまとめて選ばれていない");
    select_ops::handle_canvas_double_click(&mut app, on(500.0), true);
    assert_eq!(selected(&app, &a), a.len(), "Shift のダブルクリックで、前の線が選択から外れた");
    assert_eq!(selected(&app, &b), b.len(), "Shift のダブルクリックで、2本目がまとめて選ばれていない");
    select_ops::handle_canvas_double_click(&mut app, on(500.0), false);
    assert_eq!(selected(&app, &a), 0, "Shift を押さないダブルクリックは選び直す");
    assert_eq!(selected(&app, &b), b.len());
}

/// 上に重ねる線の取り出し（`LayerBase::lines_along`）：中身の線の辺の一部だけが覚えた形に沿うとき、形は沿う所だけ（覚えた
/// 形の曲線）で、番号はその中身の辺の番号。沿わない線からは何も取り出さない（確定・見た目・持ち上げは、この同じ答えを使う：
/// 設計審査 01e4283 の D-1）
#[test]
fn lines_along_takes_only_the_part_on_the_remembered_shape() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PencilStroke {
            points: vec![Point::new(c.x - 80.0, c.y), Point::new(c.x, c.y + 10.0), Point::new(c.x + 80.0, c.y)],
            stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 },
        },
    ));
    let base = app.doc.root.layers[0].keyframes[0].cel.scope_base(&[]);
    let m = base.map().clone();
    let e = (0..m.edges.len()).find(|&e| m.edges[e].stroke.is_some()).expect("線");
    let q = m.quad(e);
    let id = m.edges[e].line_id.expect("線の番号");
    let line_map = |q: kurbo::QuadBez| crate::planar_map::PlanarMap {
        nodes: vec![q.p0, q.p2],
        edges: vec![crate::planar_map::MapEdge {
            a: 0,
            b: 1,
            ctrl: q.p1,
            left: None,
            right: None,
            left_src: None,
            right_src: None,
            stroke: m.edges[e].stroke,
            line_id: None,
        }],
    };
    // 前半だけを覚えた形にする
    let half = q.subsegment(0.0..0.5);
    let got = base.lines_along(&line_map(half));
    assert_eq!(got.shape_ids, vec![id], "沿う中身の線の番号でない");
    let s = got.shape.quad(0);
    let ends = |a: Point, b: Point| (a - b).hypot() < 1e-6;
    assert!(
        got.shape.edges.len() == 1 && ((ends(s.p0, half.p0) && ends(s.p2, half.p2)) || (ends(s.p0, half.p2) && ends(s.p2, half.p0))),
        "取り出した形が、覚えた形の沿う所でない：{:?}",
        got.shape.edges.iter().enumerate().map(|(k, _)| got.shape.quad(k)).collect::<Vec<_>>()
    );
    // 離れた所の線は沿わない
    let away = base.lines_along(&line_map(kurbo::QuadBez::new(Point::new(c.x - 80.0, c.y + 200.0), Point::new(c.x, c.y + 210.0), Point::new(c.x + 80.0, c.y + 200.0))));
    assert!(away.shape.edges.is_empty() && away.shape_ids.is_empty(), "沿わない線から取り出した");
}

/// 動かしていない片を残りに戻しても（Shift で外す）、その塗りの内側を通っていた残りの線は消えない（同じレイヤーの絵では
/// 線はいつも塗りの上なので、動かしていない塗りは元どおり線の下に戻る：`MapRule::Restore`。設計審査 01e4283 の D-2。前は
/// 片を上に重ねる決まりで戻したので、線が覆われて消え、そのあと残りを動かして確定すると、線の右半分が消えた）
#[test]
fn returning_an_unmoved_face_keeps_the_line_over_it() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(c.x - 100.0, c.y - 60.0, c.x + 100.0, c.y + 60.0, Color32::RED)]);
    assert!(apply(&mut app, Action::PencilStroke {
        points: vec![Point::new(c.x - 80.0, c.y), Point::new(c.x, c.y + 10.0), Point::new(c.x + 80.0, c.y)],
        stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 },
    }));
    let line_len = |app: &Is2dApp| -> f64 {
        use kurbo::ParamCurveArclen;
        let doc = app.tab().committed_document_copy().unwrap();
        let m = doc.root.layers[0].keyframes[0].cel.scope_base(&[]).map().clone();
        (0..m.edges.len()).filter(|&e| m.edges[e].stroke.is_some()).map(|e| m.quad(e).arclen(1e-6)).sum()
    };
    let want = line_len(&app);
    let world = app.session.edit_path().world_xform();
    // 左半分を矩形で選ぶ（塗りと線を切って持ち上げる）
    assert!(apply(&mut app, Action::SelectMarquee { p0: world * Point::new(c.x - 120.0, c.y - 80.0), p1: world * Point::new(c.x - 20.0, c.y + 80.0), additive: false }));
    // 残りの塗り（右）を Shift で選び足し（線は選ばない）、Shift で外す（元の所に戻す）
    let red_rest = app.scene().iter().position(|s| s.color() == Color32::RED && s.line.is_none() && !app.session.selection().contains_shape(s)).expect("残りの塗り");
    assert!(apply(&mut app, Action::ToggleSelectionAt { scene_idx: red_rest }));
    let red_piece = app
        .scene()
        .iter()
        .enumerate()
        .filter(|(_, s)| s.color() == Color32::RED && s.line.is_none() && app.session.selection().contains_shape(s))
        .map(|(i, s)| (i, s.bounding_box().x1))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("右の片")
        .0;
    assert!(apply(&mut app, Action::ToggleSelectionAt { scene_idx: red_piece }));
    // 残りの片を動かして確定する
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(0.0, 30.0), alt_duplicate: false }));
    assert!(apply(&mut app, Action::Deselect));
    assert!((line_len(&app) - want).abs() < 0.05, "戻した塗りの上の線が消えた：{} のはずが {}", want, line_len(&app));
}

#[test]
fn test_pull_edge_across_circle_splits_into_two_shapes() {
    let center = Point::new(200.0, 200.0);
    let circle_app = || {
        let mut app = Is2dApp::default();
        run(
            &mut app,
            &[Action::PaintStroke {
                points: vec![center, center + KVec2::new(0.01, 0.0)],
                radius: 50.0,
                color: Color32::BLACK,
            }],
        );
        assert_eq!(scene_summary(&app).shapes.len(), 1);
        app
    };
    // 上端 (200, 150) にいちばん近い区間の真ん中（頂点から離れた所）をつかむ
    let grab_point = |app: &Is2dApp| {
        let top = Point::new(200.0, 150.0);
        scene_summary(app).shapes[0]
            .exterior()
            .segments
            .iter()
            .map(|q| q.eval(0.5))
            .min_by(|a, b| (*a - top).hypot().total_cmp(&(*b - top).hypot()))
            .unwrap()
    };

    // 円の上端エッジを円内部を通って下端 (200, 250) の向こう側 (200, 275) まで突っ切るようにドラッグ
    let mut app = circle_app();
    let from = grab_point(&app);
    assert!(pull_edge(&mut app, from, Point::new(200.0, 275.0)));
    let split_shapes = scene_summary(&app).shapes.clone();
    assert_eq!(
        split_shapes.len(),
        2,
        "円を向こう側の円周へ突っ切るようにドラッグすると2つのシェイプに分割されること (actual = {})",
        split_shapes.len()
    );

    // 左右の半円領域は塗られており、突っ切った中央 (200, 200) と突き抜けた先 (200, 265) は空であること。左右の点は、
    // 引っぱりでついてくる範囲（つかんだ所から向きが `PULL_REACH_TURN` 回る所まで）から遠い、円の下寄りの縁の近く
    let any_contains = |pt: Point| split_shapes.iter().any(|s| s.contains_point(pt));
    assert!(
        any_contains(Point::new(160.0, 225.0)),
        "左側の分割片が存在すること"
    );
    assert!(
        any_contains(Point::new(240.0, 225.0)),
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

    // 向こう側の円周の手前 (200, 249) で離したときは、押し出さず見えているとおりに確定し、分かれず1つのまま
    let mut app = circle_app();
    let from = grab_point(&app);
    assert!(pull_edge(&mut app, from, Point::new(200.0, 249.0)));
    assert_eq!(
        scene_summary(&app).shapes.len(),
        1,
        "向こう側の円周の手前で離したときは、勝手に押し出して分けない"
    );
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
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(
        app.scene().len(),
        1,
        "未編集で選択解除すると元の1つのシェイプに復元されること"
    );
    assert!(app.session.selection().is_empty());

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

    // 5. 1回の Undo で、削除する直前（左右両端のスライスが選ばれたまま）に戻り、選択を外すと、スライス前の元の1つの
    //    シェイプに完全復帰すること（取り消しは、絵と一緒にその時の選択も戻す）
    app.undo();
    assert_eq!(app.session.selected_indices().len(), 2, "削除する直前の選択に戻る");
    for p in [Point::new(110.0, 200.0), Point::new(200.0, 200.0), Point::new(290.0, 200.0)] {
        assert!(scene_summary(&app).shapes.iter().any(|s| s.contains_point(p)));
    }
    assert!(canvas_tool::clear_selection(&mut app));
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
            .any(|s| s.color() == blue && s.contains_point(Point::new(130.0, 200.0))),
        "選択された左半分が青になっていること"
    );
    assert!(
        app.scene()
            .iter()
            .any(|s| s.color() == red && s.contains_point(Point::new(260.0, 200.0))),
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
    app.set_selected_indices(vec![0]);
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
    assert!(scale_selection(&mut app, 0.5, 0.5));
    let s0 = app.scene().get(0).unwrap();
    assert!(!s0.contains_point(Point::new(200.0, 110.0)));
    assert!(s0.contains_point(Point::new(200.0, 155.0)));

    // 3. カスタムピボット点 (200, 200) に対して、左側の点 (150, 200) にあるシェイプを左右反転 -> (250, 200) に移り、ワインディングも正常に保たれること
    assert!(canvas_tool::clear_selection(&mut app));
    let left_piece = make_stroke(
        Point::new(130.0, 200.0),
        Point::new(170.0, 200.0),
        18.0,
        red,
    );
    put_cel_shapes(&mut app, 0, 0, left_piece);
    app.set_selected_indices(vec![0]);
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
        s0.exterior().signed_area() > 0.0,
        "反転時にベジェ輪郭の向き（ワインディング）が自動補正され正の面積を保つこと"
    );
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

    // 青と緑は重なっているので、セル画に入れると地図の面に置き換わる（並びは地図の面の順）。色で選ぶ
    let eyes: Vec<usize> = scene_summary(&app)
        .shapes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.color() != red)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(eyes.len(), 2);
    app.set_selected_indices(eyes);
    let eye_gid = doc_actions::group_selected_shapes(&mut app).expect("グループ化が成功すること");
    let in_group: Vec<Color32> = scene_summary(&app)
        .shapes
        .iter()
        .filter(|s| s.group_path == vec![eye_gid])
        .map(|s| s.color())
        .collect();
    assert_eq!(in_group.len(), 2);
    assert!(!in_group.contains(&red));

    // 1つクリックするだけでグループ全体の2シェイプが同時選択されること
    assert!(canvas_tool::clear_selection(&mut app));
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::SelectSingleAt {
            pt: Point::new(100.0, 100.0),
        }
    ));
    assert_eq!(app.session.selected_indices().len(), 2);

    // グループ選択状態でバケツツールを使っても無効であること
    app.tool.brush_color = red;
    assert!(!doc_actions::bucket_fill_at(
        &mut app,
        Point::new(100.0, 100.0)
    ));
    assert_eq!(scene_summary(&app).shapes[1].color(), blue);
    assert_eq!(scene_summary(&app).shapes[2].color(), green);

    // グループを赤い顔シェイプの上へ移動して選択解除しても下の生シェイプを切り抜かないこと
    doc_actions::apply(
        &mut app,
        doc_actions::Action::NudgeSelection {
            delta: KVec2::new(0.0, 100.0),
        },
    );
    assert!(canvas_tool::clear_selection(&mut app));
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 3);
    assert!(s.shapes[0].contains_point(Point::new(100.0, 200.0)));
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
    assert!(canvas_tool::clear_selection(&mut app));
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 1);
    assert!(s.shapes[0].contains_point(Point::new(120.0, 200.0)));
    assert!(s.shapes[0].contains_point(Point::new(260.0, 200.0)));
}

#[test]
fn test_adobe_animate_click_selects_edge_and_selected_shape_disables_deformation() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(235, 75, 75);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );

    // 細い 2px ブラシストロークを描画（ほぼ全域が境界線のカーソル合わせの範囲内になる細い線）
    let thin_stroke = make_stroke(Point::new(100.0, 200.0), Point::new(220.0, 200.0), 2.0, red);
    put_cel_shapes(&mut app, 0, 0, thin_stroke);
    let s = scene_summary(&app);
    assert_eq!(s.shapes.len(), 1);

    let vertex_pt = s.shapes[0].exterior().segments[0].p0;
    let edge_pt = s.shapes[0].exterior().segments[0].eval(0.5);

    let s0_id = s.shapes[0].shape_id;

    // 1. 未選択状態では、頂点や境界線にホバーすると Vertex / Edge（変形対象）になること
    assert!(matches!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            vertex_pt,
            app.view.zoom(),
            app.tool.select_sub_mode
        ),
        HoverTarget::Vertex { shape_id, .. } if shape_id == s0_id
    ));
    assert!(matches!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            edge_pt,
            app.view.zoom(),
            app.tool.select_sub_mode
        ),
        HoverTarget::Edge { shape_id, .. } | HoverTarget::Vertex { shape_id, .. } if shape_id == s0_id
    ));

    // 2. 細い線の境界線や頂点の上でクリック（SelectSingleAt）した際にも、確実に選択状態になること
    assert!(
        doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectSingleAt { pt: vertex_pt },
        ),
        "頂点や境界線上をクリックしてもシェイプが選択状態になること"
    );
    assert_eq!(app.session.selected_indices(), vec![0]);

    // 3. 選択状態（網点表示）になった後は、頂点上・境界線上・少し外側のマージンのどこを掴んでも
    //    変形（Vertex / Edge）は無効化され、常に移動（HoverTarget::Fill）になること
    assert_eq!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            vertex_pt,
            app.view.zoom(),
            app.tool.select_sub_mode
        ),
        HoverTarget::Fill { shape_id: s0_id },
        "選択状態のシェイプの頂点上では変形ではなく移動（Fill）になること"
    );
    assert_eq!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            edge_pt,
            app.view.zoom(),
            app.tool.select_sub_mode
        ),
        HoverTarget::Fill { shape_id: s0_id },
        "選択状態のシェイプの境界線上では変形ではなく移動（Fill）になること"
    );
    assert_eq!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            Point::new(160.0, 204.5),
            app.view.zoom(),
            app.tool.select_sub_mode
        ),
        HoverTarget::Fill { shape_id: s0_id },
        "選択状態の細い線の外周マージン内でも移動（Fill）として掴めること"
    );
}

/// レビュー 96fdc26 の場当たりの直し（`tool_transform.rs` の倍率の下限 0.02）：拡大のドラッグに倍率の下限は無く、潰れる
/// 変換（0 倍）は、ドラッグの間「置けない」と見せ（確定と同じ判定 `TransformFit::fits`）、離すと断る（文書は変わらない）
#[test]
fn collapsing_scale_drag_shows_unplaceable_and_commit_refuses() {
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(200.0, 200.0),
            10.0,
            Color32::RED,
        ),
    );
    app.set_selected_indices(vec![0]);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool {
            tool: Tool::Transform,
        },
    );
    let bbox = app
        .scene()
        .selection_bounding_box(app.session.selection(), app.view.zoom())
        .unwrap();
    let before = app.doc.root.layers[0].keyframes[0].cel.shapes();
    // 右の辺のつまみを、左の辺（拡大の基準）まで引く：横の倍率が 0
    let handle = Point::new(bbox.x1, bbox.center().y);
    tool_transform::start_transform_drag(&mut app, handle, false, false);
    assert!(matches!(
        app.session.drag,
        canvas_tool::ActiveCanvasDrag::Transform(canvas_tool::TransformDragState::Scale { .. })
    ));
    let collapsed = Point::new(bbox.x0, bbox.center().y);
    tool_transform::update_transform_drag(&mut app, collapsed, false, false);
    assert!(app.session.drag.unplaceable(), "0 倍は置けない");
    tool_transform::update_transform_drag(&mut app,
        Point::new(bbox.x1 + 20.0, bbox.center().y),
        false,
        false,
    );
    assert!(!app.session.drag.unplaceable(), "ふつうの拡大は置ける");
    tool_transform::update_transform_drag(&mut app, collapsed, false, false);
    app.notices = Default::default();
    tool_transform::finish_transform_drag(&mut app, Some(collapsed));
    let after = app.doc.root.layers[0].keyframes[0].cel.shapes();
    assert_eq!(after.len(), before.len());
    assert!(after
        .iter()
        .zip(&before)
        .all(|(a, b)| a.exterior().segments == b.exterior().segments));
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
        Some(canvas_tool::Refusal::OffGrid.message().text.as_str())
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
    assert!(app.session.selection().is_empty());
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
    assert_eq!(scene_summary(&app).shapes[0].color(), blue);
}

/// 線の行き止まりの端のアンカーを消すと、その区間が無くなり、線が1区間短くなる（残りの区間はそのまま）。1区間だけの線は
/// 線ごと消える。線の先が塗りの輪郭につながる点は、今までどおり消せない（利用者が決めた）
#[test]
fn deleting_a_line_end_anchor_shortens_the_line() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let ink = EdgeStroke { color: Color32::BLACK, width: 4 };
    let centers = |app: &Is2dApp| -> Vec<QuadBez> { app.scene().iter().filter_map(|s| s.center_curve()).collect() };
    // 行き止まりの端：ほかの線の端と重ならない端
    let dead_ends = |lines: &[QuadBez]| -> Vec<Point> {
        let ends: Vec<Point> = lines.iter().flat_map(|q| [q.p0, q.p2]).collect();
        ends.iter().copied().filter(|p| ends.iter().filter(|q| *q == p).count() == 1).collect()
    };
    // 1. つなぎ目のある線
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let wave: Vec<Point> = (0..=200).map(|i| { let x = 100.0 + 3.0 * i as f64; Point::new(x, 300.0 + 60.0 * (x / 50.0).sin()) }).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: wave, stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let before = centers(&app);
    assert!(before.len() >= 2, "つなぎ目のある線にならなかった");
    let end = dead_ends(&before)[0];
    let gone = *before.iter().find(|q| q.p0 == end || q.p2 == end).unwrap();
    assert!(apply(&mut app, Action::DeleteAnchor { pt: end }), "線の端のアンカーを消せない");
    let after = centers(&app);
    assert_eq!(after.len(), before.len() - 1, "線が1区間短くならない");
    for q in &before {
        let same = |a: &QuadBez| (a.p0 == q.p0 && a.p1 == q.p1 && a.p2 == q.p2) || (a.p0 == q.p2 && a.p1 == q.p1 && a.p2 == q.p0);
        assert_eq!(after.iter().any(same), *q != gone, "消した区間のほかの区間が変わった（または消した区間が残った）");
    }
    // 2. 1区間だけの線：線ごと消える
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let straight: Vec<Point> = (0..=50).map(|i| Point::new(100.0 + 4.0 * i as f64, 200.0)).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: straight, stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert_eq!(centers(&app).len(), 1);
    assert!(apply(&mut app, Action::DeleteAnchor { pt: Point::new(100.0, 200.0) }));
    assert!(centers(&app).is_empty(), "1区間だけの線が残った");
    // 3. 線の先が塗りの輪郭につながる点は消せない
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(200.0, 200.0, 400.0, 400.0, Color32::from_rgb(120, 175, 160))]);
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let out: Vec<Point> = (0..=50).map(|i| Point::new(300.0, 400.0 + 4.0 * i as f64)).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: out, stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let n = centers(&app).len();
    assert!(!apply(&mut app, Action::DeleteAnchor { pt: Point::new(300.0, 400.0) }), "輪郭につながる点を消せた");
    assert_eq!(centers(&app).len(), n);
}

/// 選択ツールのアンカー編集は、A で表示、D で追加、F で削除を切り替える（もう1回押すと戻る）。`+`・`-` はアンカー編集の
/// キーではない（`Cmd` と合わせたズームだけ。利用者が決めた）
#[test]
fn anchor_edit_modes_toggle_with_a_d_f() {
    use eframe::egui::{self, Key};
    let mut app = Is2dApp::default();
    crate::canvas_tool::doc_actions::apply(&mut app, crate::canvas_tool::doc_actions::Action::SetTool { tool: Tool::Select });
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
    let press = |app: &mut Is2dApp, key: Key| {
        let ev = |pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [vec![ev(true)], vec![ev(false)]] {
            let _ = ctx.run(egui::RawInput { screen_rect, events, ..Default::default() }, |ctx| app.update_ui(ctx));
        }
    };
    let before = app.tab().view.show_wireframe;
    press(&mut app, Key::A);
    assert_eq!(app.tab().view.show_wireframe, !before, "A で表示が切り替わらない");
    press(&mut app, Key::A);
    assert_eq!(app.tab().view.show_wireframe, before);
    press(&mut app, Key::D);
    assert_eq!(app.tool.select_sub_mode, SelectSubMode::AddAnchor, "D で追加にならない");
    press(&mut app, Key::F);
    assert_eq!(app.tool.select_sub_mode, SelectSubMode::DeleteAnchor, "F で削除にならない");
    press(&mut app, Key::F);
    assert_eq!(app.tool.select_sub_mode, SelectSubMode::Reshape, "もう1回 F で戻らない");
    press(&mut app, Key::Minus);
    press(&mut app, Key::Equals);
    assert_eq!(app.tool.select_sub_mode, SelectSubMode::Reshape, "+・- でアンカー編集が切り替わった");
    // ほかのツールから押すと、選択ツールにして ON にする（もう1回押すと、選択ツールのまま切り替える）
    for (key, check) in [
        (Key::A, (|app: &Is2dApp| app.tab().view.show_wireframe) as fn(&Is2dApp) -> bool),
        (Key::D, |app: &Is2dApp| app.tool.select_sub_mode == SelectSubMode::AddAnchor),
        (Key::F, |app: &Is2dApp| app.tool.select_sub_mode == SelectSubMode::DeleteAnchor),
    ] {
        crate::canvas_tool::doc_actions::apply(&mut app, crate::canvas_tool::doc_actions::Action::SetTool { tool: Tool::Pencil });
        press(&mut app, key);
        assert_eq!(app.tool.tool, Tool::Select, "{key:?}：選択ツールにならない");
        assert!(check(&app), "{key:?}：ほかのツールから押して ON にならない");
        press(&mut app, key);
        assert_eq!(app.tool.tool, Tool::Select);
        assert!(!check(&app), "{key:?}：選択ツールでもう1回押して切り替わらない");
    }
    // 表示が ON のまま、ほかのツールから A を押しても OFF にしない
    press(&mut app, Key::A);
    assert!(app.tab().view.show_wireframe);
    crate::canvas_tool::doc_actions::apply(&mut app, crate::canvas_tool::doc_actions::Action::SetTool { tool: Tool::Brush });
    press(&mut app, Key::A);
    assert_eq!(app.tool.tool, Tool::Select);
    assert!(app.tab().view.show_wireframe, "ほかのツールから A を押して表示が OFF になった");
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

    // 初回フレームで app.canvas_rect を確定させる
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
    let center_world = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
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
    let expected_dx = 50.0 / (app.view.zoom() as f64);
    assert!(
        (moved_center.x - (initial_bbox_center.x + expected_dx)).abs() < 2.0,
        "handle_transform_tool_input 経由の左ドラッグでシェイプが平行移動されること: moved_center={:?}, expected_x={}",
        moved_center,
        initial_bbox_center.x + expected_dx
    );
}

#[test]
fn test_pull_edge_near_endpoint_and_off_center_does_not_jump_on_drag_start() {
    use kurbo::ParamCurve;

    let mut app = Is2dApp::default();
    // 地図の図形は重ね合わせで作るので、頂点はいつも格子（1/4096px）の上にある。この試しの図形も頂点を格子の上に置く
    // （輪郭の変形の確定は重ね合わせで行い、入れた辺の頂点を格子に置くので）
    let g = |p: Point| {
        Point::new(
            (p.x * 4096.0).round() / 4096.0,
            (p.y * 4096.0).round() / 4096.0,
        )
    };
    let on_grid = |c: &crate::vector_engine::Contour| {
        crate::vector_engine::Contour::new(
            c.segments
                .iter()
                .map(|q| QuadBez::new(g(q.p0), q.p1, g(q.p2)))
                .collect(),
        )
    };
    let shapes = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        20.0,
        Color32::from_rgb(220, 60, 60),
    )
    .into_iter()
    .map(|s| {
        FillShape::new(
            s.color(),
            on_grid(s.exterior()),
            s.holes().iter().map(on_grid).collect(),
        )
    })
    .collect();
    put_cel_shapes(&mut app, 0, 0, shapes);

    // 最も長いセグメントを選び、端点付近 (t = 0.05 < 0.18) かつ曲線から少し外れた座標でドラッグ開始 (#5)
    let seg_idx = scene_summary(&app).shapes[0]
        .exterior()
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
    let orig_seg = scene_summary(&app).shapes[0].exterior().segments[seg_idx];
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
    let after_zero_move = app.scene().get(0).unwrap().exterior().segments[seg_idx];
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
        app.doc.root.layers[0].keyframes[0].cel.shapes()[0]
            .exterior()
            .segments[seg_idx]
            .p1,
        orig_seg.p1,
        "ドラッグ中はセル内の図形が書き換えられないこと"
    );
    // つかんだ点はカーソルの動きどおりに動く：輪郭が、つかんだ点をカーソルの動いた分だけずらした所を通る（引っぱった辺の
    // 両端も、なめらかにつながる丸い端と一緒についてくるので、辺は両端では探さない）
    let moved_to = grab_before + kurbo::Vec2::new(0.0, 2.0);
    let gap = app.scene().get(0).unwrap().exterior().segments.iter()
        .map(|q| crate::vector_engine::nearest_on_quad(q, moved_to).1)
        .fold(f64::MAX, f64::min);
    // 許容は地図の格子の1目（ついてきて動いた頂点は、確定で格子の点に丸まる）
    assert!(
        gap < 1.0 / 4096.0,
        "微小ドラッグ時の曲線変化量がカーソル移動量と正確に一致すること (#5)：ずれ {gap}"
    );
}

/// アンカーを動かしても、近くのほかのアンカーに吸い付かない（吸い付きは無くした。利用者が決めた）。丸 A の右端のアンカーを、
/// 丸 B の左端のアンカーから 5px の所（前の吸い付く距離 12px の内）へ動かすと、カーソルの所に残る
#[test]
fn dragging_an_anchor_near_another_anchor_does_not_snap() {
    for show_anchors in [false, true] {
        let mut app = Is2dApp::default();
        for c in [Point::new(200.0, 300.0), Point::new(320.0, 300.0)] {
            run(&mut app, &[Action::PaintStroke { points: vec![c, c + KVec2::new(0.01, 0.0)], radius: 30.0, color: Color32::BLACK }]);
        }
        app.tab_mut().view.show_wireframe = show_anchors;
        let nodes = |app: &Is2dApp| -> Vec<Point> {
            scene_summary(app).shapes.iter().flat_map(|s| s.exterior().segments.iter().map(|q| q.p0)).collect()
        };
        let before = nodes(&app);
        let near = |p: Point| *before.iter().min_by(|a, b| (**a - p).hypot().total_cmp(&(**b - p).hypot())).expect("頂点がある");
        let (v, u) = (near(Point::new(230.0, 300.0)), near(Point::new(290.0, 300.0)));
        let to = u + KVec2::new(-5.0, 0.0);
        tool_select::start_reshape_drag(&mut app, v, false, false);
        tool_select::update_reshape_drag(&mut app, to);
        tool_select::finish_reshape_drag(&mut app, Some(to));
        let after = nodes(&app);
        assert!(after.iter().any(|p| (*p - to).hypot() < 1e-3), "アンカーの表示 {show_anchors}：カーソルの所に無い（吸い付いた）");
        assert!(after.contains(&u), "吸い付かれた側のアンカーが変わった");
    }
}

/// 線の上をつかんで引っぱると、アンカーを表示していないときは、まわりのアンカーもついてくる（ざっくりした変形）。表示して
/// いるときは、つかんだ1区間だけが曲がり、アンカーはどれも動かない（細かい変形。Animate の部分選択ツールの代わり。利用者が
/// 決めた）
#[test]
fn pulling_an_edge_moves_only_that_edge_while_anchors_are_shown() {
    let center = Point::new(400.0, 700.0);
    let arc: Vec<Point> = (0..=60)
        .map(|i| {
            let a = (-120.0 + i as f64).to_radians();
            center + KVec2::new(a.cos(), a.sin()) * 450.0
        })
        .collect();
    for show_anchors in [false, true] {
        let mut app = Is2dApp::default();
        run(&mut app, &[Action::PaintStroke { points: arc.clone(), radius: 10.0, color: Color32::BLACK }]);
        app.tab_mut().view.show_wireframe = show_anchors;
        let segs: Vec<QuadBez> = scene_summary(&app).shapes[0].exterior().segments.to_vec();
        let anchors: Vec<Point> = segs.iter().map(|q| q.p0).collect();
        // 外側の縁の真ん中あたりの区間の真ん中をつかむ
        let outer_mid = center + KVec2::new(0.0, -460.0);
        let q = *segs
            .iter()
            .min_by(|a, b| (a.eval(0.5) - outer_mid).hypot().total_cmp(&(b.eval(0.5) - outer_mid).hypot()))
            .expect("区間がある");
        let from = q.eval(0.5);
        tool_select::start_reshape_drag(&mut app, from, false, false);
        let to = from + KVec2::new(0.0, -15.0);
        tool_select::update_reshape_drag(&mut app, to);
        tool_select::finish_reshape_drag(&mut app, Some(to));
        let after: Vec<Point> = scene_summary(&app).shapes[0].exterior().segments.iter().map(|q| q.p0).collect();
        let kept = anchors.iter().filter(|p| after.contains(p)).count();
        if show_anchors {
            assert_eq!(kept, anchors.len(), "アンカー表示中に線を引っぱって、アンカーが動いた");
            let pulled = scene_summary(&app).shapes[0].exterior().segments.iter().any(|s| {
                ((s.p0, s.p2) == (q.p0, q.p2) || (s.p2, s.p0) == (q.p0, q.p2)) && s.p1 != q.p1
            });
            assert!(pulled, "つかんだ区間が曲がっていない");
        } else {
            assert!(kept < anchors.len(), "アンカー非表示で線を引っぱって、まわりがついてこない");
        }
    }
}

/// 選択ツールでアンカーをつかんで動かすと、アンカーを表示していないときは、まわりがついてくる引っぱりになり、表示している
/// ときは、そのアンカーだけが動く（Animate の部分選択ツールの代わり。利用者が決めた）
#[test]
fn dragging_an_anchor_pulls_its_neighbours_only_while_anchors_are_hidden() {
    // ゆるい弧に沿ったブラシの線（外側の縁に、向きが少しずつ変わるアンカーが並ぶ）。外側の縁の真ん中のアンカーをつかむ
    let center = Point::new(400.0, 700.0);
    let arc: Vec<Point> = (0..=60)
        .map(|i| {
            let a = (-120.0 + i as f64).to_radians();
            center + KVec2::new(a.cos(), a.sin()) * 450.0
        })
        .collect();
    let outer_mid = center + KVec2::new(0.0, -460.0);
    for show_anchors in [false, true] {
        let mut app = Is2dApp::default();
        run(
            &mut app,
            &[Action::PaintStroke {
                points: arc.clone(),
                radius: 10.0,
                color: Color32::BLACK,
            }],
        );
        app.tab_mut().view.show_wireframe = show_anchors;
        let before: Vec<Point> = scene_summary(&app).shapes[0].exterior().segments.iter().map(|q| q.p0).collect();
        let v = *before
            .iter()
            .min_by(|a, b| (**a - outer_mid).hypot().total_cmp(&(**b - outer_mid).hypot()))
            .expect("頂点がある");
        tool_select::start_reshape_drag(&mut app, v, false, false);
        assert!(
            matches!(
                app.session.drag,
                canvas_tool::ActiveCanvasDrag::Select(canvas_tool::SelectDragState::MoveVertex { follow, .. })
                    if follow != show_anchors
            ),
            "アンカーの表示 {show_anchors}：引っぱりかアンカーだけかが違う"
        );
        let to = v + KVec2::new(0.0, -20.0);
        tool_select::update_reshape_drag(&mut app, to);
        tool_select::finish_reshape_drag(&mut app, Some(to));
        let after: Vec<Point> = scene_summary(&app).shapes[0].exterior().segments.iter().map(|q| q.p0).collect();
        assert!(after.iter().any(|p| (*p - to).hypot() < 1e-3), "つかんだアンカーがカーソルの所に無い");
        let kept = before.iter().filter(|p| **p != v && after.contains(p)).count();
        if show_anchors {
            assert_eq!(kept, before.len() - 1, "アンカーだけのはずが、まわりが動いた");
        } else {
            assert!(kept < before.len() - 1, "まわりがついてこない");
        }
    }
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

    let area_blue_top_before = scene_summary(&app).shapes[0].exterior().signed_area();
    let area_blue_bottom_before = scene_summary(&app).shapes[1].exterior().signed_area();

    // 赤い図形2つ (index 2, 3) を選択して左へ -100px 移動し、2つの青い図形の上にそれぞれ重ねる
    app.set_selected_indices(vec![2, 3]);
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
    assert_eq!(app.scene().get(remaining_idx).unwrap().color(), red);
    assert!(doc_actions::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(100.0, 0.0)
    ));
    assert!(canvas_tool::clear_selection(&mut app));

    // 結果：先に選択解除された上側の赤い図形は上側の青い図形を切り抜いているが、
    // 選択に残して後から退かした下側の赤い図形は下側の青い図形を切り抜いていないこと！
    let summary_after = scene_summary(&app);
    let blue_shapes: Vec<&FillShape> = summary_after
        .shapes
        .iter()
        .filter(|s| s.color() == blue)
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
        top_blue.holes().len(),
        1,
        "選択解除された上側の赤い図形は背景の青い図形を即座に切り抜いて穴を開けていること (#7)"
    );
    assert!(
        bottom_blue.holes().is_empty()
            && (bottom_blue.exterior().signed_area() - area_blue_bottom_before).abs() < 1.0,
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
        .selection_bounding_box(app.session.selection(), app.view.zoom())
        .unwrap();
    let current_pivot = app.scene().active_transform_pivot(
        app.session.selection(),
        app.session.transform_pivot,
        bbox,
    );
    tool_transform::start_transform_drag(&mut app, current_pivot, false, false);
    tool_transform::update_transform_drag(&mut app, Point::new(130.0, 110.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(Point::new(130.0, 110.0)));

    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes().len(),
        1,
        "未変形のマーキースライス状態でピボットだけ動かしてもスライスの切れ目がタイムラインに書き込まれないこと (#8)"
    );

    // 2. シンボルインスタンスをマーキー選択（生シェイプも同時にスライス）した状態でピボットを変更した場合、
    // 生シェイプのスライス控えを復元しつつシンボルの新しいピボットがタイムラインへ永続化され失われないこと (#8)
    assert!(canvas_tool::clear_selection(&mut app));
    let sym_shapes = make_stroke(
        Point::new(100.0, 220.0),
        Point::new(180.0, 220.0),
        16.0,
        Color32::from_rgb(200, 80, 40),
    );
    let mut shapes = scene_summary(&app).shapes;
    shapes.extend(sym_shapes);
    put_cel_shapes(&mut app, 0, 0, shapes);
    app.set_selected_indices(vec![1]);
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
    app.set_selected_indices(sym_indices);
    let new_world_pivot = Point::new(155.0, 235.0);
    assert!(set_pivot(
        &mut app,
        Some(new_world_pivot),
    ));
    assert!(app.session.floating().is_none());
    assert!(canvas_tool::clear_selection(&mut app));

    let saved_inst = &app.doc.root.layers[0].keyframes[0].cel.instances()[0];
    assert!(
        (saved_inst.binding.world_pivot() - new_world_pivot).hypot() < 1e-4,
        "ピボット変更後に選択解除しても stale な未スライススナップショットでピボットが上書き消失しないこと (#8)"
    );
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
    app.set_selected_indices(vec![0]);

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
    app.set_selected_indices(vec![0]);
    let bbox = app
        .scene()
        .selection_bounding_box(app.session.selection(), app.view.zoom())
        .unwrap();
    tool_transform::start_transform_drag(&mut app,
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
    app.set_selected_indices(vec![0]);
    let stage = app.stage();
    let orig_shapes = app.session.begin_transform_overlay(&stage);
    let pivot = scene_summary(&app).shapes[0].bounding_box().center();
    let scale_xform = kurbo::Affine::translate(pivot.to_vec2())
        * kurbo::Affine::scale(4.0)
        * kurbo::Affine::translate(-pivot.to_vec2());
    app.session.update_transform_overlay(&orig_shapes, scale_xform);
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
    // 拡大率の積が使い回せる上限（許すずれ ÷ 作る許容 = 0.25 ÷ 0.04 = 6.25 倍：`keeps_mesh`）の中の間は頂点を写して
    // 使い回し（1.2 の 10 乗 ≒ 6.19）、超えたら作り直す（11 乗 ≒ 7.43）
    for _ in 0..10 {
        s.transform(step_scale);
    }
    assert_eq!(
        s.fill_mesh().vertices.len(),
        v0,
        "拡大率の積が上限の中の間は、メッシュを使い回すこと"
    );
    s.transform(step_scale);
    let v_after = s.fill_mesh().vertices.len();
    assert!(
        v_after > v0,
        "1.25倍以内の拡大 transform でもメッシュキャッシュが再構築されること (#5): v0={v0}, v_after={v_after}"
    );
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
    merge_shapes_into_canvas(&mut canvas, non_intersecting_blue);
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
    app.set_selected_indices(vec![0]);
    let bbox = app
        .scene()
        .selection_bounding_box(app.session.selection(), 1.0)
        .unwrap();
    let top_left = canvas_tool::ScaleHandle::TopLeft.position(bbox);
    let hover = tool_transform::compute_transform_hover(app.scene(), &app.session, top_left, 1.0);
    assert_eq!(
        hover,
        canvas_tool::TransformHoverTarget::Scale(canvas_tool::ScaleHandle::TopLeft),
        "小さな図形の四隅ハンドル座標では Pivot に遮られず Scale(TopLeft) がヒットすること (#14)"
    );

    // 3. #11: レイヤー数上限 (MAX_LAYERS_PER_TIMELINE = 1000) を超えて timeline_add_layer されないこと
    let mut tl = timeline::Timeline::default();
    for i in 1..crate::timeline::MAX_LAYERS_PER_TIMELINE {
        tl.layers
            .push(timeline::Layer::new_with_id((i + 1) as u64, "L"));
    }
    assert_eq!(tl.layers.len(), crate::timeline::MAX_LAYERS_PER_TIMELINE);
    tl.add_layer_above_active(10_000);
    assert_eq!(
        tl.layers.len(),
        crate::timeline::MAX_LAYERS_PER_TIMELINE,
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
        let target_content = std::fs::read(&target_path).unwrap();
        assert!(
            target_content.starts_with(b"IS2D")
                && target_content[4..8] == save_format::FORMAT_VERSION.to_le_bytes(),
            "シンボリックリンクの実体ファイル側が最新の内容に更新されること (#13)"
        );
        let _ = std::fs::remove_file(&link_path);
        let _ = std::fs::remove_file(&target_path);
    }
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
            Action::NudgeSelection {
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
            Action::NudgeSelection {
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
            Action::NudgeSelection {
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
    pull_edge(&mut app, Point::new(200.0, 180.0), Point::new(200.0, 120.0));
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
    move_vertex(&mut app, Point::new(200.0, 220.0), Point::new(200.0, 270.0));
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
    scale_selection(&mut app, 2.0, 1.5);
    run(&mut app, &[Action::RotateSelection {
                angle_rad: std::f64::consts::FRAC_PI_2,
            }, Action::FlipSelection {
                flip_x: true,
                flip_y: false,
            }]);
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
            Action::NudgeSelection {
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
    assert_eq!(app.session.selection().shapes.len(), 1);
    let moved_shape_id = *app.session.selection().shapes.iter().next().unwrap();

    let move_drag = clipboard_ops::begin_move_selection_drag(&mut app, target_pt, false);
    app.session.drag = canvas_tool::ActiveCanvasDrag::MoveSelection(move_drag);
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

    // さらにマウス移動してドラッグ確定（Action::NudgeSelection 相当のタイムライン同期含む）
    tool_select::update_reshape_drag(&mut app, Point::new(32.0, 25.0));
    run(
        &mut app,
        &[Action::NudgeSelection {
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
    use crate::canvas_tool::doc_actions::Action;

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
    assert!(!fl.modified());
    assert_eq!(app.session.floating_view().pieces().len(), 1);
    assert_eq!(app.session.floating_view().remainder().count(), 1);
    assert_eq!(app.session.floating_view().hidden().len(), 1);

    run(&mut app, &[Action::Deselect]);
    assert!(app.session.floating().is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "未変更のマーキースライスは選択解除時に破棄され元の1図形に戻ること"
    );

    // 2. マーキースライス後に移動すると Floating.modified が true になり、Action::Deselect で1回だけ結合確定されること
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
        &[Action::NudgeSelection {
            delta: KVec2::new(10.0, 0.0),
        }],
    );
    let fl_moved = app
        .session
        .floating()
        .expect("スライス断片の移動後も Floating が保持されること");
    assert!(fl_moved.modified());

    // 重なっている同色断片が Action::Deselect で1図形に Union 結合され、Floating が空になること
    run(&mut app, &[Action::Deselect]);
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
    assert!(fl_paste.modified());
    assert_eq!(app.scene().len(), 2);

    run(&mut app, &[Action::Deselect]);
    assert!(app.session.floating().is_none());
    assert_eq!(
        app.scene().len(),
        1,
        "インプレースペーストした同色図形が CommitFloating で1図形に結合されること"
    );
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

    let orig_red_bbox = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].bounding_box();
    let orig_blue_bbox = app.doc.root.layers[0].keyframes[0].cel.shapes()[1].bounding_box();

    // 1. 選択移動ドラッグ中は文書のセルが変わらず、Scene の最前面（TopOfLayer）にプレビューが出る
    let red_id = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].shape_id;
    {
        let s0 = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].clone();
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&s0); app.session.replace_selection(sel); }
    }
    tool_select::start_reshape_drag(&mut app, Point::new(120.0, 100.0), false, false);
    tool_select::update_reshape_drag(&mut app, Point::new(120.0, 140.0));
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes()[0].bounding_box(),
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
        let s0 = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].clone();
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&s0); app.session.replace_selection(sel); }
    }
    tool_select::start_reshape_drag(&mut app, Point::new(120.0, 100.0), false, true);
    tool_select::update_reshape_drag(&mut app, Point::new(120.0, 150.0));
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes().len(),
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
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 2);

    // 3. 自由変形（拡大・回転）ドラッグ中は文書のセルが変わらず、Scene に変形プレビューが出る
    {
        let s0 = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].clone();
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&s0); app.session.replace_selection(sel); }
    }
    let bbox = app
        .scene()
        .selection_bounding_box(app.session.selection(), 1.0)
        .unwrap();
    let tr_corner = Point::new(bbox.x1, bbox.y0);
    tool_transform::start_transform_drag(&mut app, tr_corner, false, false);
    tool_transform::update_transform_drag(&mut app,
        Point::new(bbox.x1 + 30.0, bbox.y0 - 10.0),
        false,
        false,
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes()[0].bounding_box(),
        orig_red_bbox,
        "自由変形スケール中は文書のセルが書き換えられないこと"
    );
    let preview_bbox = app
        .scene()
        .selection_bounding_box(app.session.selection(), 1.0)
        .unwrap();
    assert!(preview_bbox.width() > orig_red_bbox.width() + 20.0);
    tool_transform::finish_transform_drag(&mut app,
        Some(Point::new(bbox.x1 + 30.0, bbox.y0 - 10.0)),
    );
    let committed_red = app.scene().iter().find(|s| s.shape_id == red_id).unwrap();
    assert!(committed_red.bounding_box().width() > orig_red_bbox.width() + 20.0);
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.scene().get(0).unwrap().bounding_box(), orig_red_bbox);
}

#[test]
fn test_stage7b_floating_holds_pieces_without_mutating_cels_until_commit() {
    use crate::canvas_tool::doc_actions::Action;

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
    let rev_initial = app.doc.root.layers[0].keyframes[0].cel.revision();
    let cel_before = cel_summary(&app.doc, lid, 0);
    let orig_red = app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes()
        .iter()
        .find(|s| s.color() == red)
        .unwrap()
        .exterior()
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
        app.doc.root.layers[0].keyframes[0].cel.revision(), rev_initial,
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
        app.doc.root.layers[0].keyframes[0].cel.revision(),
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
        app.doc.root.layers[0].keyframes[0].cel.revision(), rev_initial,
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
        app.doc.root.layers[0].keyframes[0].cel.revision(),
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
        app.doc.root.layers[0].keyframes[0].cel.revision(), rev_initial,
        "保存後も Document の Cel は書き換えられないこと"
    );
    let mut loaded_app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded_app, &tmp_save, None).unwrap();
    let _ = std::fs::remove_file(&tmp_save);
    let loaded_red: f64 = loaded_app
        .scene()
        .iter()
        .filter(|s| s.color() == red)
        .map(|s| s.exterior().signed_area())
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
        app.doc.root.layers[0].keyframes[0].cel.revision(), rev_initial,
        "確定時に1回だけ文書の Cel が更新されること"
    );
}

#[test]
fn test_g9_1_transform_click_on_rotate_or_scale_handle_preserves_selection() {
    use crate::canvas_tool::{Tool, TransformHoverTarget};
    use crate::canvas_tool::doc_actions::{apply, Action};
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    // 画面の座標を文書の座標のまま使う（表示は 100%・ずらしなし。開いたときの窓への合わせはしない）
    app.view.fit_pending = false;
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
    assert!(!app.session.selection().is_empty());

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
        .selection_bounding_box(app.session.selection(), app.view.zoom())
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
            app.view.zoom()
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
        !app.session.selection().is_empty(),
        "自由変形ツールで回転ゾーンをクリックしたとき選択が外れないこと"
    );

    // 2. 拡縮ハンドルの上（図形外・境界枠の左上角の少し外側）をクリックしても選択が外れないこと
    let scale_pt = Point::new(bbox.x0 - 3.0, bbox.y0 - 3.0);
    assert!(
        !app.scene().get(0).unwrap().contains_point(scale_pt),
        "拡縮ハンドル上の座標は図形の外側であること"
    );
    assert!(matches!(
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            scale_pt,
            app.view.zoom()
        ),
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
        !app.session.selection().is_empty(),
        "自由変形ツールで拡縮ハンドルの上をクリックしたとき選択が外れないこと"
    );

    // 3. 何もない場所をクリックしたときは選択が外れること
    let empty_pt = Point::new(bbox.x0 - 80.0, bbox.y0 - 80.0);
    assert_eq!(
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            empty_pt,
            app.view.zoom()
        ),
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
        app.session.selection().is_empty(),
        "何もない場所をクリックしたときは選択が外れること"
    );
}

#[test]
fn test_g1_5_floating_mutations_auto_rebuild_overlay() {
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
    let raw = brush_line_at_zoom(
        &[Point::new(100.0, 100.0), Point::new(180.0, 100.0)],
        6.0,
        1.0,
    );
    let mut shape = brush_line_shapes_at_zoom(&raw, Color32::RED, 1.0)
        .into_iter()
        .next()
        .unwrap();
    shape.layer_id = app.doc.root.layers[0].id;
    assert_eq!(app.scene().len(), 0);

    let lid = shape.layer_id;
    let map = crate::planar_map::LayerBase::of(&[&shape])
        .unwrap()
        .map()
        .clone();
    let stage = app.tabs[0].stage();
    app.tabs[0].session.add_floating_maps(&stage, vec![(
        lid,
        map.checked().unwrap(),
        kurbo::Affine::IDENTITY,
    )]);
    assert_eq!(app.scene().len(), 1);
    assert_eq!(app.scene().get(0).unwrap().color(), Color32::RED);

    let layers: HashSet<u64> = [app.doc.root.layers[0].id].into();
    let ids = app.tabs[0].session.floating_view().piece_ids();
    let stage = app.tabs[0].stage();
    app.tabs[0].session.update_floating(&stage, |fl_opt| {
        if let Some(fl) = fl_opt {
            assert!(fl.recolor(&ids, Color32::BLUE, &layers).unwrap());
        }
    });
    assert_eq!(app.scene().get(0).unwrap().color(), Color32::BLUE);

    let taken = app.tabs[0].session.take_floating();
    assert!(taken.is_some());
    assert_eq!(app.scene().len(), 0);
}

#[test]
fn test_m2_7_disjoint_floating_commit_matches_full_merge_on_lift_slice_alt_dup() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
        dense_app.doc.root.layers[l_idx].keyframe_mut(0).cel =
            timeline::Cel::from_shapes_unwrap(shapes);
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
fn test_m4_1_alt_duplicate_touching_sliced_pieces_merges_into_single_shape_on_deselect() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    assert_eq!(app.doc.root.layers[0].cel_at(0).unwrap().shapes().len(), 1);

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
            .is_some_and(|_| app.session.floating_view().pieces().len() == 1),
        "つながったスライス片は選び足した時点で1片に結合され、Alt複製後も1片が floating に入っていること"
    );

    // 5. 選択を解除すると、元の1図形と、1つに結合された複製の計2図形になること
    let _ = apply(&mut app, Action::Deselect);
    assert_eq!(
        app.doc.root.layers[0].cel_at(0).unwrap().shapes().len(),
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
        reopened.doc.root.layers[0]
            .cel_at(0)
            .unwrap()
            .shapes()
            .len(),
        2,
        "保存して読み込んでも2図形であること"
    );
}

/// 地図は格子の内（±65,536px）の形しか持てないので、選んだ図形を格子の外へ動かす操作はしない（図形は元のまま残る）
#[test]
fn test_move_selection_off_grid_is_refused() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    ));
    let before = app.doc.root.layers[0].keyframes[0].cel.shapes().clone();
    assert!(apply(&mut app, Action::SelectAll));
    assert!(!apply(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(70_000.0, 0.0),
        },
    ));
    assert!(canvas_tool::clear_selection(&mut app));
    let after = &app.doc.root.layers[0].keyframes[0].cel.shapes();
    assert_eq!(after.len(), before.len());
    assert!(after
        .iter()
        .zip(before.iter())
        .all(|(a, b)| a.bounding_box() == b.bounding_box()));
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
    app.set_selected_indices(vec![0]);

    let (_, init_pivot) = app
        .session
        .selection_bbox_and_pivot(app.view.zoom())
        .expect("selection bbox and pivot should exist");
    tool_transform::start_transform_drag(&mut app, init_pivot, false, false);
    tool_transform::update_transform_drag(&mut app, Point::new(20.0, 60.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(Point::new(20.0, 60.0)));
    assert_eq!(app.session.transform_pivot, Some(Point::new(20.0, 60.0)));

    tool_transform::start_transform_drag(&mut app, Point::new(40.0, 50.0), false, false);
    tool_transform::update_transform_drag(&mut app, Point::new(90.0, 50.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(Point::new(90.0, 50.0)));
    assert_eq!(app.session.transform_pivot, Some(Point::new(70.0, 60.0)));
}

#[test]
fn test_deselected_slice_piece_does_not_collide_with_kept_piece_id() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 200.0), Point::new(300.0, 200.0)],
            radius: 20.0,
            color: Color32::RED,
        },
    ));
    let orig_id = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].shape_id;

    // 1. 左半分を矩形で選ぶ（スライス）
    crate::canvas_tool::select_ops::apply_marquee_selection(
        &mut app,
        Point::new(80.0, 150.0),
        Point::new(200.0, 250.0),
        false,
    );
    assert_eq!(app.session.selection().shapes.len(), 1);

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
    assert_eq!(app.session.selection().shapes.len(), 1);

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
    assert!(app.session.selection().shapes.is_empty());
    assert!(app.session.floating().is_none());
    assert_eq!(app.scene().len(), 1);
    assert_eq!(app.scene().get(0).unwrap().shape_id, orig_id);
}

#[test]
fn test_alt_duplicate_and_paste_raw_shape_with_group_keeps_both_selected() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    crate::canvas_tool::select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(350.0, 160.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 2);

    let mut drag =
        crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, Point::new(130.0, 100.0), true);
    drag.total_delta = kurbo::Vec2::new(0.0, 100.0);
    crate::canvas_tool::doc_actions::finish_move_selection_drag(&mut app, drag.total_delta, drag.alt_duplicate);

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
    let red_count = app.scene().iter().filter(|s| s.color() == red).count();
    let blue_count = app.scene().iter().filter(|s| s.color() == blue).count();
    assert_eq!(red_count, 2, "赤の元の図形と複製の両方が残っていること");
    assert_eq!(blue_count, 2);

    // 2. 同じ2つ（y=100 の赤と青）をコピーしてペーストする → 両方選択され、そのまま動かすと両方が動く
    crate::canvas_tool::select_ops::apply_marquee_selection(
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
        Action::NudgeSelection {
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

/// 選んだ生の図形の選択の色は、同じ段（同じレイヤー・同じ階層の線どうし）の図形を描き終えた所で塗る：交わる線に隠されず、
/// 同じ段の中ではいちばん上に見える（利用者の指摘。前は選んだ図形を描いた直後に重ねたので、あとから描く線の下に回っていた）
#[test]
fn the_selection_tint_is_drawn_above_every_shape() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let green = EdgeStroke { color: Color32::from_rgb(40, 180, 90), width: 8 };
    let blue = EdgeStroke { color: Color32::from_rgb(55, 135, 245), width: 8 };
    let across: Vec<Point> = (0..=60).map(|i| Point::new(100.0 + 5.0 * i as f64, 200.0)).collect();
    let down: Vec<Point> = (0..=60).map(|i| Point::new(250.0, 50.0 + 5.0 * i as f64)).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: across, stroke: green }));
    assert!(apply(&mut app, Action::PencilStroke { points: down, stroke: blue }));
    // 画面の図形の並び（描く順）でいちばん下の線を、交わる所から離れた所で選ぶ
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let low = app.scene().iter().find(|s| s.line.is_some()).unwrap().clone();
    let q = low.center_curve().unwrap();
    let pick = if (q.eval(0.0) - Point::new(250.0, 200.0)).hypot() > (q.eval(1.0) - Point::new(250.0, 200.0)).hypot() {
        q.eval(0.1)
    } else {
        q.eval(0.9)
    };
    assert!(apply(&mut app, Action::SelectSingleAt { pt: pick }));
    assert!(app.session.selection().contains_shape(app.scene().iter().find(|s| s.shape_id == low.shape_id).unwrap()));
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
    let out = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
    let tint = crate::screen_mesh::selection_color(low.color());
    let first_color = |s: &egui::epaint::ClippedShape| match &s.shape {
        egui::Shape::Mesh(m) => m.vertices.first().map(|v| v.color),
        _ => None,
    };
    let colors: Vec<Option<Color32>> = out.shapes.iter().map(first_color).collect();
    let tint_at = colors.iter().position(|c| *c == Some(tint)).expect("選択の色を描いていない");
    let last_line = colors
        .iter()
        .rposition(|c| *c == Some(green.color) || *c == Some(blue.color))
        .expect("線を描いていない");
    assert!(tint_at > last_line, "選択の色（{tint_at} 番目）が、あとから描く線（{last_line} 番目）の下になる");
}

/// 線で囲んだ塗りを Alt で複製して動かし、そのままグループにすると、選択は新しいグループの全部（塗りも線も）になり、続けて
/// ドラッグすると線も一緒に動く（利用者の指摘。前は、グループにする前の番号で選び直したので、動かした先に置いて番号の変わった
/// 線だけが選択から外れ、ドラッグ中に線が置いていかれた）
#[test]
fn grouping_moved_duplicates_selects_the_whole_group() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    let green = Color32::from_rgb(120, 175, 160);
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(200.0, 200.0, 300.0, 300.0, green)]);
    let ink = EdgeStroke { color: Color32::BLACK, width: 10 };
    assert!(apply(&mut app, Action::InkBottle { pt: Point::new(250.0, 250.0), stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(300.0, 0.0), alt_duplicate: true }));
    assert!(apply(&mut app, Action::Group));
    let scope = app.session.editing_group_path().to_vec();
    let group: Vec<(bool, bool)> = app
        .scene()
        .iter()
        .filter(|s| s.active_group_id_at_scope(&scope).is_some())
        .map(|s| (s.line.is_some(), app.session.selection().contains_shape(s)))
        .collect();
    assert_eq!(group.iter().filter(|g| g.0).count(), 4, "新しいグループに線が4本ある");
    assert!(group.iter().all(|g| g.1), "新しいグループの中に、選ばれていない図形がある：{group:?}");
    // 続けてドラッグすると、線も一緒に動く
    let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, Point::new(550.0, 250.0), false);
    app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
    app.tab_mut().session.translate_move_overlay(KVec2::new(0.0, 100.0));
    for s in app.scene().iter().filter(|s| s.bounding_box().x0 > 400.0) {
        assert!(s.bounding_box().y0 > 280.0, "ドラッグ中に置いていかれた図形がある（線 {}）", s.line.is_some());
    }
}

/// 線と塗りの絵のグループを Alt で複製しながら動かし、続けてそのままドラッグすると、複製したグループの全部（塗りも線も）が
/// 動く（利用者の指摘：塗りだけが動いた）。ドラッグ中の見た目と、離した後の両方で
#[test]
fn dragging_an_alt_duplicated_group_moves_its_lines_too() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    let green = Color32::from_rgb(120, 175, 160);
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(200.0, 200.0, 300.0, 300.0, green)]);
    let ink = EdgeStroke { color: Color32::BLACK, width: 10 };
    assert!(apply(&mut app, Action::InkBottle { pt: Point::new(250.0, 250.0), stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Group));
    // Alt で複製しながら右へ（画面の操作と同じ道：つかんで、動かして、離す）
    let grab = Point::new(250.0, 250.0);
    crate::canvas_tool::select_ops::grab_and_move(&mut app, grab, true, true);
    crate::canvas_tool::select_ops::update_selection_drag(&mut app, grab + KVec2::new(300.0, 0.0));
    crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
    let scope = app.session.editing_group_path().to_vec();
    let dup: Vec<(bool, bool)> = app
        .scene()
        .iter()
        .filter(|s| s.bounding_box().x0 > 400.0)
        .map(|s| (s.line.is_some(), app.session.selection().contains_shape(s)))
        .collect();
    assert!(dup.iter().any(|d| d.0) && dup.iter().any(|d| !d.0), "複製に塗りと線がある：{dup:?}");
    assert!(dup.iter().all(|d| d.1), "複製の中に選ばれていない図形がある：{dup:?}");
    assert!(app.scene().iter().filter(|s| s.bounding_box().x0 > 400.0).all(|s| s.active_group_id_at_scope(&scope).is_some()));
    // 続けてそのままドラッグ（つかむのは複製の上）
    let grab = Point::new(550.0, 250.0);
    crate::canvas_tool::select_ops::grab_and_move(&mut app, grab, true, false);
    crate::canvas_tool::select_ops::update_selection_drag(&mut app, grab + KVec2::new(0.0, 150.0));
    for s in app.scene().iter().filter(|s| s.bounding_box().x0 > 400.0) {
        assert!(s.bounding_box().y0 > 330.0, "ドラッグ中に置いていかれた図形がある（線 {}）", s.line.is_some());
    }
    crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
    for s in app.scene().iter().filter(|s| s.bounding_box().x0 > 400.0) {
        assert!(s.bounding_box().y0 > 330.0, "離した後に置いていかれた図形がある（線 {}）", s.line.is_some());
    }
    // 貼り付けも同じ道（`put_placements`）：グループを貼り付けた直後は、貼り付けた全部（塗りも線も）が選ばれている
    let before: std::collections::HashSet<u64> = app.scene().iter().map(|s| s.shape_id).collect();
    assert!(apply(&mut app, Action::Copy));
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    let pasted: Vec<(bool, bool)> = app
        .scene()
        .iter()
        .filter(|s| !before.contains(&s.shape_id))
        .map(|s| (s.line.is_some(), app.session.selection().contains_shape(s)))
        .collect();
    assert!(pasted.iter().any(|d| d.0) && pasted.iter().any(|d| !d.0), "貼り付けに塗りと線がある：{pasted:?}");
    assert!(pasted.iter().all(|d| d.1), "貼り付けた中に選ばれていない図形がある：{pasted:?}");
}

/// 線で縁どった塗りの、塗りだけをその場に貼り付けて確定しても、縁の線は消えない（Animate と同じ。利用者が確かめた）。上に
/// 重ねる塗りの境目に線が無ければ、下の線はそのまま残る（内側にすっぽり入った下の線だけが、上の塗りに覆われて消える）。
/// 前は、上の塗りの線の無い境目が、ぴったり重なる下の線を置き換えて消した
#[test]
fn pasting_a_fill_in_place_keeps_the_ink_under_its_edge() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    let green = Color32::from_rgb(120, 175, 160);
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(200.0, 200.0, 300.0, 300.0, green)]);
    let ink = EdgeStroke { color: Color32::BLACK, width: 10 };
    assert!(apply(&mut app, Action::InkBottle { pt: Point::new(250.0, 250.0), stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let lines = |app: &Is2dApp| app.scene().iter().filter(|s| s.line.is_some()).count();
    assert_eq!(lines(&app), 4);
    // 塗りだけを選んで、コピーして、その場に貼り付けて、確定する
    assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(250.0, 250.0) }));
    assert!(app.scene().iter().filter(|s| app.session.selection().contains_shape(s)).all(|s| s.line.is_none()));
    assert!(apply(&mut app, Action::Copy));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert!(apply(&mut app, Action::Deselect));
    assert_eq!(lines(&app), 4, "縁の線が消えた");
    // 縁の線の外側の半分（塗りの外）も、線の色のまま
    let top_outside = Point::new(250.0, 197.0);
    let hit = app.scene().iter().rev().find(|s| s.contains_point(top_outside)).map(|s| s.color());
    assert_eq!(hit, Some(Color32::BLACK));
}

/// 線で縁どった塗りのグループを2つ選んで少し動かし、取り消して（やり直して）すぐドラッグしても、線も一緒に動く（利用者の
/// 指摘：塗りだけが動いた）。前は線の番号を線の位置と形から作ったので、動かすと変わり、取り消しのとき、取り消す直前の
/// 選択（動かした後の番号）を持ち越して無くなった番号を外したので、線が選択から落ちた。取り消しの控えに、その時の選択もしまう
#[test]
fn undo_and_redo_keep_the_lines_of_selected_groups_selected() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    let ink = EdgeStroke { color: Color32::BLACK, width: 8 };
    for (x, c) in [(150.0, Color32::from_rgb(120, 175, 160)), (400.0, Color32::from_rgb(230, 180, 60))] {
        put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(x, 200.0, x + 100.0, 300.0, c)]);
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(x + 50.0, 250.0), stroke: ink }));
        apply(&mut app, Action::SetTool { tool: Tool::Select });
        assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(x - 20.0, 180.0), p1: Point::new(x + 120.0, 320.0), additive: false }));
        assert!(apply(&mut app, Action::Group));
        assert!(apply(&mut app, Action::Deselect));
    }
    let scope = app.session.editing_group_path().to_vec();
    let all_selected = |app: &Is2dApp| -> bool {
        app.scene().iter().filter(|s| s.active_group_id_at_scope(&scope).is_some()).all(|s| app.session.selection().contains_shape(s))
    };
    assert!(apply(&mut app, Action::SelectAll));
    assert!(all_selected(&app));
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(30.0, 20.0), alt_duplicate: false }));
    assert!(all_selected(&app), "動かした直後に、グループの一部が選ばれていない");
    assert!(apply(&mut app, Action::Undo));
    assert!(all_selected(&app), "取り消した後に、グループの一部（線）が選ばれていない");
    // すぐドラッグすると、線も一緒に動く
    let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, Point::new(200.0, 250.0), false);
    app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
    crate::canvas_tool::select_ops::update_selection_drag(&mut app, Point::new(200.0, 400.0));
    assert!(app.scene().iter().all(|s| s.bounding_box().y0 > 320.0), "ドラッグ中に置いていかれた図形がある");
    crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
    assert!(app.scene().iter().all(|s| s.bounding_box().y0 > 320.0), "離した後に置いていかれた図形がある");
    // 取り消して、やり直しても
    assert!(apply(&mut app, Action::Undo));
    assert!(apply(&mut app, Action::Redo));
    assert!(all_selected(&app), "やり直した後に、グループの一部（線）が選ばれていない");
}

/// 生の絵とグループを一緒に選んでも、グループは生の絵の上に見える（生の絵はいつもグループより下：Animate と同じ。前は、
/// 選んだ生の絵（浮いた片）をレイヤーのいちばん上に描いたので、選んだ顔の塗りが眉のグループを隠した。利用者の指摘）。
/// 選んだまま動かしている間も同じ（離した後と同じ重なり）
#[test]
fn selected_raw_art_stays_under_groups() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    let skin = Color32::from_rgb(240, 210, 185);
    let brow = Color32::from_rgb(20, 20, 20);
    // 眉（グループ）を先に作り、そのあと顔の塗り（生の絵）を置く
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(220.0, 220.0, 300.0, 240.0, brow)]);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Group));
    assert!(apply(&mut app, Action::Deselect));
    assert!(apply(&mut app, Action::SetTool { tool: Tool::Brush }));
    run(&mut app, &[Action::PaintStroke { points: vec![Point::new(260.0, 300.0), Point::new(260.01, 300.0)], radius: 150.0, color: skin }]);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let top_at = |app: &Is2dApp, p: Point| app.scene().iter().rev().find(|s| s.contains_point(p)).map(|s| s.color());
    let on_brow = Point::new(260.0, 230.0);
    assert_eq!(top_at(&app, on_brow), Some(brow), "選ぶ前から眉が見えていない");
    // 矩形で両方を選ぶ（顔の塗りは浮いた片になる）
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(500.0, 500.0), additive: false }));
    assert!(app.scene().iter().any(|s| s.color() == skin && app.session.selection().contains_shape(s)));
    assert_eq!(top_at(&app, on_brow), Some(brow), "選ぶと、眉が顔の塗りに隠れた");
    // 選んだまま動かしている間も、離した後も
    let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, Point::new(260.0, 300.0), false);
    app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
    crate::canvas_tool::select_ops::update_selection_drag(&mut app, Point::new(270.0, 310.0));
    assert_eq!(top_at(&app, on_brow + KVec2::new(10.0, 10.0)), Some(brow), "動かしている間に、眉が顔の塗りに隠れた");
    crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
    assert_eq!(top_at(&app, on_brow + KVec2::new(10.0, 10.0)), Some(brow), "離した後に、眉が顔の塗りに隠れた");
}

/// 浮いた片は、持ち上げた絵（セル画の地図）を覚えていて、確定するときに今のセル画の地図がそれと違えば、何も書かずに断る
/// （設計審査 de92ceb の D-3）。確定を通らずにセル画が変わった（確定の呼び忘れ）ときに、持ち上げたときの「残り」で今の絵を
/// 黙って置き換えない。前は、確定は今の画面のセル画に残りを書いたので、変わった絵が消えた
#[test]
fn committing_floating_pieces_onto_a_changed_cel_is_refused() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(200.0, 350.0), additive: false }));
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(0.0, 200.0) }));
    assert!(app.session.floating().is_some());
    // 確定を通らずに、同じセル画が別の絵に変わる（確定の呼び忘れの代わり）
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(500.0, 100.0, 600.0, 200.0, Color32::BLUE)]);
    let maps_before = app.doc.root.layers[0].keyframes[0].cel.maps().clone();
    app.notices = Default::default();
    assert!(!apply(&mut app, Action::Deselect), "変わったセル画に浮いた片を確定した");
    let last = app.notices.iter().last().map(|t| crate::canvas_tool::NoticeText { text: t.message.clone(), undo_exit: t.undo_exit });
    assert_eq!(last.as_ref(), Some(&crate::canvas_tool::Refusal::Unplaceable.message()));
    assert!(std::sync::Arc::ptr_eq(app.doc.root.layers[0].keyframes[0].cel.maps(), &maps_before), "変わった絵が書き換わった");
    // 断った知らせは、出口（取り消し）を案内する。取り消しは確定を断られたときも効き、動かす前に戻る（新しい操作は作らない：
    // 持ち主が決めた 2026-11-27。審査 5616fcc の D-2）
    assert!(crate::shown_notice_text(last.as_ref().unwrap()).contains("取り消し（"), "知らせが出口を案内しない");
    assert!(apply(&mut app, Action::Undo), "確定を断られたあとの取り消しが効かない");
    assert!(app.session.floating().is_none_or(|f| !f.modified()), "取り消しても動かしたまま");
    assert!(saved_from_app(app.tab()).is_ok(), "取り消したあとも保存できない");
    assert!(apply(&mut app, Action::Deselect), "取り消したあとも選択を外せない");
}

/// 浮いた片の片は、いつもみな選ばれている：矩形で選んだあと、同じ色の塗りを分ける線を Shift クリックで外しても、線が
/// 外れて1つになった塗り（新しい番号の片）は選ばれたまま。前は、片の番号が付け直される道のうち1つ（動かしていない片を
/// 戻す道）だけが片を選び直さず、選択の表示に無い塗りが、ドラッグで動き、Delete で消えた（設計審査 de92ceb の D-1 の
/// (a)）。片を選ぶのは、浮いた片の写しを作り直す1か所（`Session::sync_floating`）
#[test]
fn floating_pieces_stay_selected_after_deselecting_a_dividing_line() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 4 };
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let cut: Vec<Point> = (0..=20).map(|i| Point::new(200.0, 80.0 + 12.0 * i as f64)).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: cut, stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(350.0, 350.0), additive: false }));
    assert!(app.session.floating().is_some(), "矩形で選ぶと浮いた片になる");
    // 塗りを分ける線を Shift クリックで外す
    crate::canvas_tool::select_ops::apply_click_selection(&mut app, Point::new(200.0, 200.0), true, true);
    app.session.floating().expect("塗りは浮いたまま");
    let unselected: Vec<u64> = app.session.floating_view().pieces().iter().filter(|p| !app.session.selection().contains_shape(p)).map(|p| p.shape_id).collect();
    assert!(unselected.is_empty(), "浮いているのに選ばれていない片がある: {unselected:?}");
}

/// 線の番号は辺にしまってあり、線を動かしても変わらない（設計 claude/線の番号を辺にしまう設計_81b715e.md）：動かして確定・
/// グループにして動かす・取り消し・やり直し・線を引っぱる、のどれのあとも同じ番号。前は番号を線の形から作ったので、動かす
/// たびに変わり、選び直す所で線が選択から漏れた
#[test]
fn line_ids_stay_the_same_when_lines_move() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ink = crate::planar_map::EdgeStroke { color: Color32::from_rgb(20, 40, 90), width: 6 };
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let wave: Vec<Point> = (0..=60).map(|i| Point::new(100.0 + 5.0 * i as f64, 200.0 + 20.0 * (i as f64 * 0.2).sin())).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: wave, stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let line_ids = |app: &Is2dApp| -> Vec<u64> {
        let mut v: Vec<u64> = app.scene().iter().filter(|s| s.line.is_some()).map(|s| s.shape_id).collect();
        v.sort_unstable();
        v
    };
    let first = line_ids(&app);
    assert!(first.len() > 1, "線が何本かの辺になっている");
    // 動かして確定する
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(20.0, 60.0), alt_duplicate: false }));
    assert_eq!(line_ids(&app), first, "動かしている間に番号が変わった");
    assert!(apply(&mut app, Action::Deselect));
    assert_eq!(line_ids(&app), first, "動かして確定したら番号が変わった");
    // グループにして動かす
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Group));
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(-30.0, 10.0) }));
    assert_eq!(line_ids(&app), first, "グループにして動かしたら番号が変わった");
    // 取り消し・やり直し
    assert!(apply(&mut app, Action::Undo));
    assert_eq!(line_ids(&app), first, "取り消したら番号が変わった");
    assert!(apply(&mut app, Action::Redo));
    assert_eq!(line_ids(&app), first, "やり直したら番号が変わった");
    // グループを解いて、線の1区間を引っぱる（アンカーを表示して、その区間だけ）
    assert!(apply(&mut app, Action::Ungroup));
    assert!(apply(&mut app, Action::Deselect));
    app.tab_mut().view.show_wireframe = true;
    let seg = app.scene().iter().find(|s| s.line.is_some()).and_then(|s| s.center_curve()).expect("線");
    let from = seg.eval(0.5);
    tool_select::start_reshape_drag(&mut app, from, false, false);
    tool_select::update_reshape_drag(&mut app, from + KVec2::new(0.0, -12.0));
    tool_select::finish_reshape_drag(&mut app, None);
    let after = line_ids(&app);
    assert_eq!(after.len(), first.len());
    assert_eq!(after, line_ids(&app));
    let moved_ids: Vec<u64> = after.iter().filter(|id| !first.contains(id)).copied().collect();
    assert!(moved_ids.is_empty(), "引っぱった線の番号が変わった：{} 本", moved_ids.len());
}

/// 線の漏れの洗い出し：線と塗りとグループの混ざった絵で、選び方と、新しい中身ができる操作（グループ化・解除・重ね順・
/// シンボル化・移動・Alt の複製・貼り付け・拡大・回転・反転・取り消し・やり直し）をでたらめな順に重ね、そのたびに確かめる：
/// - グループのまとまり：グループの図形は、塗りも線も、みな選ばれているか、みな選ばれていないか
/// - 選んでいる範囲：形を変えない操作（グループ化・解除・重ね順・シンボル化・その場に貼り付け）のあとは、選んだ絵が覆う所が
///   前と同じ。動かす操作（移動・Alt の複製）のあとは、動かした分だけずれた所（線の番号は形から決まるので、形や持ち主が
///   変わると番号が変わる。選び直しで線が漏れると、線の所が欠ける）
/// - 取り消し・やり直しのあとは、その時の選択の範囲に戻っている（取り消しは選択も戻す。前は取り消す直前の選択を持ち越し、
///   動かして番号の変わった線が落ちた）
/// - 最後に続けてドラッグしたとき、ドラッグ中の見た目と離した後が同じ
#[test]
fn selections_keep_their_lines_through_operations() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::canvas_tool::select_ops::ArrangeMode;
    let ink = crate::planar_map::EdgeStroke { color: Color32::from_rgb(20, 40, 90), width: 8 };
    let line = |a: Point, b: Point| -> Vec<Point> { (0..=40).map(|i| a.lerp(b, i as f64 / 40.0)).collect() };
    // 選んでいる絵
    let selected = |app: &Is2dApp| -> Vec<FillShape> {
        app.scene().iter().filter(|s| app.session.selection().contains_shape(s)).cloned().collect()
    };
    // 格子の点が、絵 `sel` を `shift` だけずらしたものに覆われているか
    let covered = |sel: &[FillShape], shift: KVec2| -> Vec<bool> {
        let mut out = Vec::new();
        let mut y = 120.0;
        while y < 760.0 {
            let mut x = 60.0;
            while x < 900.0 {
                let p = Point::new(x + 0.137, y + 0.173) - shift;
                out.push(sel.iter().any(|s| s.contains_point(p)));
                x += 4.0;
            }
            y += 4.0;
        }
        out
    };
    let colors = |app: &Is2dApp| -> Vec<Option<Color32>> {
        let shapes: Vec<FillShape> = app.scene().iter().cloned().collect();
        let mut out = Vec::new();
        let mut y = 120.0;
        while y < 760.0 {
            let mut x = 60.0;
            while x < 900.0 {
                let p = Point::new(x + 0.137, y + 0.173);
                out.push(shapes.iter().rev().find(|s| s.contains_point(p)).map(|s| s.color()));
                x += 4.0;
            }
            y += 4.0;
        }
        out
    };
    let mut rng = fastrand::Rng::with_seed(0x11e5_1eaf);
    let mut bad: Vec<String> = Vec::new();
    for case in 0..600 {
        let mut app = Is2dApp::default();
        apply(&mut app, Action::SetTool { tool: Tool::Pencil });
        for (a, b) in [
            (Point::new(100.0, 200.0), Point::new(500.0, 200.0)),
            (Point::new(220.0, 200.0), Point::new(220.0, 450.0)),
            (Point::new(380.0, 200.0), Point::new(380.0, 450.0)),
        ] {
            assert!(apply(&mut app, Action::PencilStroke { points: line(a, b), stroke: ink }));
        }
        put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(420.0, 300.0, 520.0, 420.0, Color32::from_rgb(230, 180, 60))]);
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(470.0, 360.0), stroke: ink }));
        apply(&mut app, Action::SetTool { tool: Tool::Select });
        let on_art = |rng: &mut fastrand::Rng| match rng.usize(0..5) {
            0 => Point::new(100.0 + rng.f64() * 400.0, 200.0),
            1 => Point::new(220.0, 200.0 + rng.f64() * 250.0),
            2 => Point::new(380.0, 200.0 + rng.f64() * 250.0),
            3 => Point::new(430.0 + rng.f64() * 80.0, 310.0 + rng.f64() * 100.0),
            _ => Point::new(420.0 + rng.f64() * 100.0, 300.0),
        };
        let mut ops: Vec<String> = Vec::new();
        // 取り消しの控えごとの、その時の選択（控えが増えた操作の前の選択。取り消したら戻るはずの選択）と、やり直しの分
        let (mut undo_sels, mut redo_sels): (Vec<Vec<FillShape>>, Vec<Vec<FillShape>>) = (Vec::new(), Vec::new());
        for _ in 0..(3 + rng.usize(0..8)) {
            let before = selected(&app);
            let (n_undo, n_redo) = (app.history.undo_stack.len(), app.history.redo_stack.len());
            let pick = rng.usize(0..16);
            // （名前, 選んだ範囲の確かめ：None は確かめない、Some(d) は d だけずれる）
            let (name, expect): (&str, Option<KVec2>) = match pick {
                0 => {
                    let (x, y) = (60.0 + rng.f64() * 500.0, 150.0 + rng.f64() * 350.0);
                    let add = rng.bool();
                    apply(&mut app, Action::SelectMarquee { p0: Point::new(x, y), p1: Point::new(x + 40.0 + rng.f64() * 300.0, y + 40.0 + rng.f64() * 300.0), additive: add });
                    ("矩形", None)
                }
                1 => {
                    let pt = on_art(&mut rng);
                    crate::canvas_tool::select_ops::apply_click_selection(&mut app, pt, rng.bool(), true);
                    ("クリック", None)
                }
                2 => {
                    let pt = on_art(&mut rng);
                    select_ops::handle_canvas_double_click(&mut app, pt, rng.bool());
                    ("ダブル", None)
                }
                3 => ("全部", { apply(&mut app, Action::SelectAll); None }),
                4 => ("グループ化", apply(&mut app, Action::Group).then_some(KVec2::ZERO)),
                5 => ("解除", apply(&mut app, Action::Ungroup).then_some(KVec2::ZERO)),
                6 => {
                    let mode = [ArrangeMode::ToFront, ArrangeMode::Forward, ArrangeMode::Backward, ArrangeMode::ToBack][rng.usize(0..4)];
                    ("重ね順", apply(&mut app, Action::Arrange { mode }).then_some(KVec2::ZERO))
                }
                7 => ("シンボル化", apply(&mut app, Action::ConvertToSymbol).then_some(KVec2::ZERO)),
                8 => {
                    let d = KVec2::new(rng.f64() * 60.0 - 30.0, rng.f64() * 60.0 - 30.0).round();
                    ("移動", apply(&mut app, Action::NudgeSelection { delta: d }).then_some(d))
                }
                9 => {
                    let d = KVec2::new(rng.f64() * 300.0 - 100.0, rng.f64() * 200.0).round();
                    let alt = rng.bool();
                    let ok = apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: d, alt_duplicate: alt });
                    (if alt { "Alt複製" } else { "ドラッグ" }, ok.then_some(d))
                }
                10 => {
                    let ok = apply(&mut app, Action::Copy) && apply(&mut app, Action::Paste { in_place: true });
                    ("その場に貼り付け", ok.then_some(KVec2::ZERO))
                }
                11 => ("拡大", { scale_selection(&mut app, 1.25, 0.8); None }),
                12 => ("回転", { apply(&mut app, Action::RotateSelection { angle_rad: 0.4 }); None }),
                13 => ("反転", { apply(&mut app, Action::FlipSelection { flip_x: true, flip_y: false }); None }),
                14 => ("取り消し", { apply(&mut app, Action::Undo); None }),
                _ => ("やり直し", { apply(&mut app, Action::Redo); None }),
            };
            // 取り消し・やり直しのあとは、絵と一緒に、その時の選択に戻っている（選んだ範囲が欠けない）
            let (u, r) = (app.history.undo_stack.len(), app.history.redo_stack.len());
            let restored = if pick == 14 && u + 1 == n_undo {
                redo_sels.push(before.clone());
                undo_sels.pop()
            } else if pick == 15 && r + 1 == n_redo {
                undo_sels.push(before.clone());
                redo_sels.pop()
            } else {
                for _ in n_undo..u {
                    undo_sels.push(before.clone());
                }
                if r < n_redo {
                    redo_sels.clear();
                }
                None
            };
            if let Some(want_sel) = restored {
                let (got, want) = (covered(&selected(&app), KVec2::ZERO), covered(&want_sel, KVec2::ZERO));
                let missing = got.iter().zip(&want).filter(|(g, w)| !**g && **w).count();
                if missing > 0 {
                    ops.push(name.to_string());
                    bad.push(format!("{case}（{}）：戻した選択の範囲が {missing} 点欠けた", ops.join("・")));
                    break;
                }
            }
            ops.push(name.to_string());
            // 浮いた片の片は、いつもみな選ばれている（`Session::sync_floating`）
            if app.session.floating().is_some() {
                let n = app.session.floating_view().pieces().iter().filter(|p| !app.session.selection().contains_shape(p)).count();
                if n > 0 {
                    bad.push(format!("{case}（{}）：浮いているのに選ばれていない片が {n} 個", ops.join("・")));
                    break;
                }
            }
            // グループのまとまり（今の階層の子グループ・インスタンスごとに、みな選ばれているか、みな選ばれていないか）
            let scope_now = app.session.editing_group_path().to_vec();
            let mut units: std::collections::HashMap<u64, (usize, usize, usize)> = std::collections::HashMap::new();
            for sh in app.scene().iter() {
                if let Some(g) = sh.active_group_id_at_scope(&scope_now) {
                    let e = units.entry(g).or_default();
                    e.0 += 1;
                    if app.session.selection().contains_shape(sh) {
                        e.1 += 1;
                    }
                    if sh.line.is_some() {
                        e.2 += 1;
                    }
                }
            }
            if let Some((g, (n, k, l))) = units.iter().find(|(_, (n, k, _))| *k != 0 && k != n) {
                bad.push(format!("{case}（{}）：グループ {} の {n} 個のうち {k} 個だけ選ばれている（線 {l}）", ops.join("・"), g % 1000));
                break;
            }
            if let Some(d) = expect {
                let after = selected(&app);
                let (got, want) = (covered(&after, KVec2::ZERO), covered(&before, d));
                // 欠けた所だけを見る（線が漏れると、その線の所が欠ける。増えるのは、選んだ線が隣の線から離れて端が丸く
                // なる分で、決まりどおり）
                let missing = got.iter().zip(&want).filter(|(g, w)| !**g && **w).count();
                if missing > 0 {
                    let lines = |v: &[FillShape]| v.iter().filter(|s| s.line.is_some()).count();
                    bad.push(format!(
                        "{case}（{}）：選んだ範囲が {missing} 点欠けた（選んだ線 {} → {}）",
                        ops.join("・"),
                        lines(&before),
                        lines(&after)
                    ));
                    break;
                }
            }
        }
        // 続けてドラッグしたとき、ドラッグ中の見た目と離した後が同じ
        if !app.session.selection().is_empty() && !bad.iter().any(|b| b.starts_with(&format!("{case}（"))) {
            let from = Point::new(300.0, 300.0);
            let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, from, false);
            app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
            crate::canvas_tool::select_ops::update_selection_drag(&mut app, from + KVec2::new(20.0, 130.0));
            let during = colors(&app);
            crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
            let diff = during.iter().zip(&colors(&app)).filter(|(a, b)| a != b).count();
            if diff > 0 {
                bad.push(format!("{case}（{}・続けてドラッグ）：ドラッグ中と離した後が {diff} 点違う", ops.join("・")));
            }
        }
    }
    assert!(bad.is_empty(), "線の漏れ（{} 件）：{bad:?}", bad.len());
}

/// 線で囲んだ塗りの、塗りと線1本を一緒に選んでも、選んでいない線は細く見えない：選んだ塗りの選択の色は、同じ段（同じ
/// レイヤー・同じ階層の塗り）の最後に塗り、線はそのあとに描く（線は塗りより上）。前は選択の色を全部の図形の上に塗ったので、
/// 塗りの上に乗っている線の内側の半分を、選んだ塗りの色が覆った（利用者の指摘）
#[test]
fn a_selected_fill_does_not_cover_the_lines_on_it() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    let green = Color32::from_rgb(120, 175, 160);
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(200.0, 200.0, 500.0, 500.0, green)]);
    let ink = EdgeStroke { color: Color32::BLACK, width: 10 };
    assert!(apply(&mut app, Action::InkBottle { pt: Point::new(350.0, 350.0), stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(350.0, 350.0) }));
    let left = app.scene().iter().position(|s| s.line.is_some() && s.contains_point(Point::new(200.0, 350.0))).unwrap();
    assert!(apply(&mut app, Action::ToggleSelectionAt { scene_idx: left }));
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
    let out = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
    let first_color = |s: &egui::epaint::ClippedShape| match &s.shape {
        egui::Shape::Mesh(m) => m.vertices.first().map(|v| v.color),
        _ => None,
    };
    let colors: Vec<Option<Color32>> = out.shapes.iter().map(first_color).collect();
    let fill_tint = colors
        .iter()
        .position(|c| *c == Some(crate::screen_mesh::selection_color(green)))
        .expect("塗りの選択の色を描いていない");
    let line_tint = colors
        .iter()
        .position(|c| *c == Some(crate::screen_mesh::selection_color(ink.color)))
        .expect("線の選択の色を描いていない");
    let black: Vec<usize> = (0..colors.len()).filter(|&i| colors[i] == Some(Color32::BLACK)).collect();
    assert!(!black.is_empty(), "選んでいない線を描いていない");
    assert!(black.iter().all(|&i| i > fill_tint), "選んだ塗りの色が、選んでいない線を覆う");
    assert!(black.iter().all(|&i| i < line_tint), "選んだ線の色が、同じ段の線の下になる");
}

/// 線の図形の選択の色は、両端を丸くした帯：同じ線が続くつなぎ目でも、選んだ区間の端が丸く見える（利用者が決めた）。
/// 描く線そのものは、つなぎ目で切れたまま（隣の線とつながって見える）。両端がもともと丸い線は、描くメッシュと同じ
#[test]
fn a_selected_line_shows_round_ends_even_at_joints() {
    use crate::vector_stroke::LineEnd;
    let inside = |mesh: &egui::Mesh, p: Point| {
        mesh.indices.chunks_exact(3).any(|t| {
            let v = |i: u32| {
                let q = mesh.vertices[i as usize].pos;
                Point::new(f64::from(q.x), f64::from(q.y))
            };
            let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
            let side = |p1: Point, p2: Point| (p2 - p1).cross(p - p1);
            let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
            !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
        })
    };
    let stroke = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 10 };
    let center = QuadBez::new((0.0, 0.0), (50.0, 0.0), (100.0, 0.0));
    let make = |ends: [LineEnd; 2]| {
        let (ext, holes, mesh) = crate::planar_map::line_band(center, stroke, ends).unwrap();
        FillShape::new(stroke.color, ext, holes).with_line(stroke, center, ends).with_mesh(mesh)
    };
    // 終わりの端がつなぎ目（隣の線へ扇を足さない側）
    let joined = make([LineEnd::Cap, LineEnd::Open]);
    let beyond = Point::new(103.0, 0.0);
    for zoom in [1.0f32, 16.0] {
        // 選択の色のメッシュは画面の座標（倍率をかけた所）
        let on_screen = Point::new(beyond.x * f64::from(zoom), beyond.y * f64::from(zoom));
        assert!(inside(&crate::screen_mesh::shape_selection_screen_mesh(&joined, zoom, Vec2::ZERO), on_screen), "倍率 {zoom}：選択の色の端が丸くない");
        assert!(!inside(&joined.view_mesh(zoom), beyond), "倍率 {zoom}：描く線まで丸くなった");
    }
    let capped = make([LineEnd::Cap, LineEnd::Cap]);
    assert_eq!(
        crate::screen_mesh::shape_selection_screen_mesh(&capped, 1.0, Vec2::ZERO).vertices.len(),
        capped.fill_mesh().vertices.len(),
        "両端が丸い線は、描くメッシュをそのまま使う"
    );
}

/// つながった同じ線をまとめて選んだとき、つなぎ目で丸い端どうしが重なっても、選択の色は濃くならない（選択の色は図形の
/// 色に青を重ねた不透明な色なので、重なった所も同じ色：`selection_color`。前は半透明の青を重ねたので、2回塗られて濃い丸に
/// 見えた）
#[test]
fn joined_selected_lines_do_not_double_the_tint_at_joints() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let ink = EdgeStroke { color: Color32::BLACK, width: 16 };
    let wave: Vec<Point> = (0..=200)
        .map(|i| {
            let x = 100.0 + 3.0 * i as f64;
            Point::new(x, 300.0 + 80.0 * (x / 60.0).sin())
        })
        .collect();
    assert!(apply(&mut app, Action::PencilStroke { points: wave, stroke: ink }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectAll));
    let lines: Vec<QuadBez> = app.scene().iter().filter_map(|s| s.center_curve()).collect();
    assert!(lines.len() >= 3, "つなぎ目のある線にならなかった（{} 本）", lines.len());
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
    let out = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
    let tint = crate::screen_mesh::selection_color(ink.color);
    assert_eq!(tint.a(), 255, "不透明な線を選んだ色は不透明");
    let tints: Vec<std::sync::Arc<egui::Mesh>> = out
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Mesh(m) if m.vertices.first().is_some_and(|v| v.color == tint) => Some(m.clone()),
            _ => None,
        })
        .collect();
    assert!(!tints.is_empty(), "選択の色を描いていない");
    let (zoom, pan) = (app.view.zoom(), app.view.pan);
    let covers = |m: &egui::Mesh, p: Point| {
        let p = Point::new(p.x * f64::from(zoom) + f64::from(pan.x), p.y * f64::from(zoom) + f64::from(pan.y));
        m.indices.chunks_exact(3).any(|t| {
            let v = |i: u32| {
                let q = m.vertices[i as usize].pos;
                Point::new(f64::from(q.x), f64::from(q.y))
            };
            let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
            let side = |p1: Point, p2: Point| (p2 - p1).cross(p - p1);
            let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
            !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
        })
    };
    let mut joints = 0;
    for a in &lines {
        for b in &lines {
            if a.p2 != b.p0 {
                continue;
            }
            joints += 1;
            // つなぎ目から、続く線の上を太さの 1/4 だけ進んだ点
            let t = (1..1000).map(|k| k as f64 / 1000.0).find(|&t| (b.eval(t) - b.p0).hypot() >= 4.0).unwrap();
            let p = b.eval(t);
            // その点を覆う選択の色は、どれも同じ不透明な色（何回重なっても、見える色は1回のときと同じ）
            let covering: Vec<_> = tints.iter().filter(|m| covers(m, p)).collect();
            assert!(!covering.is_empty(), "つなぎ目 {:?} の先が選択の色で塗られていない", b.p0);
            assert!(covering.iter().all(|m| m.vertices.iter().all(|v| v.color == tint)), "つなぎ目 {:?}", b.p0);
        }
    }
    assert!(joints > 0, "つなぎ目が無い");
}

#[test]
fn test_merge_touching_sliced_pieces_on_additive_select() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
        let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].clone();

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
        app.session.floating().expect("Floating が存在すること");
        assert_eq!(app.session.floating_view().pieces().len(), 1);
        assert_eq!(app.session.floating_view().remainder().count(), 0);
        assert_eq!(app.session.selection().shapes.len(), 1);

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
        assert_eq!(app.session.selection().shapes.len(), 0);
        assert!(app.session.floating().is_none());
        let restored = &app.doc.root.layers[0].keyframes[0].cel.shapes()[0];
        assert_eq!(restored.shape_id, orig_shape.shape_id);
        assert!(std::sync::Arc::ptr_eq(
            &restored.exterior().segments,
            &orig_shape.exterior().segments
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
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
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
        app.session.floating().expect("Floating が存在すること");
        assert_eq!(app.session.floating_view().pieces().len(), 1);
        assert_eq!(app.session.floating_view().remainder().count(), 1);
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
        app.session.floating().expect("Floating が存在すること");
        assert_eq!(app.session.floating_view().pieces().len(), 2);
    }

    // 6. 1 のあと動かさずに選択を外す → 元の図形のまま（番号も同じ）で、Undo の履歴が増えない
    {
        let mut app = make_app_with_bar();
        let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].clone();
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
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].shape_id, orig_shape.shape_id);
        assert!(std::sync::Arc::ptr_eq(
            &shapes[0].exterior().segments,
            &orig_shape.exterior().segments
        ));
    }
}

#[test]
fn test_deselected_slice_piece_merges_into_touching_remainder() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
        app.session.floating().unwrap();
        assert_eq!(app.session.floating_view().pieces().len(), 2);
        assert_eq!(app.session.floating_view().remainder().count(), 2);

        let b_idx = app
            .scene()
            .iter()
            .position(|s| s.contains_point(Point::new(310.0, 200.0)))
            .expect("B の片にヒットすること");
        assert!(apply(
            &mut app,
            Action::ToggleSelectionAt { scene_idx: b_idx }
        ));

        app.session.floating().unwrap();
        assert_eq!(app.session.floating_view().pieces().len(), 1);
        assert_eq!(app.session.floating_view().remainder().count(), 1);
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
        app.session.floating().unwrap();
        assert_eq!(app.session.floating_view().pieces().len(), 1);
        assert_eq!(app.session.floating_view().remainder().count(), 0);
        assert_eq!(app.session.selection().shapes.len(), 1);
    }

    // 3. 1 のあと、動かさずに選択を外す → 図形は1つで、元の番号のまま。Undo の履歴は増えない
    {
        let mut app = make_app_with_long_bar();
        let orig_id = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].shape_id;
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
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
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
        assert_eq!(app.session.floating_view().pieces().len(), 2);
        assert_eq!(app.session.floating_view().remainder().count(), 1);

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
        app.session.floating().unwrap();
        assert_eq!(app.session.floating_view().pieces().len(), 1);
        assert_eq!(app.session.floating_view().remainder().count(), 2);

        let scene_ids: Vec<_> = app.scene().iter().map(|s| s.shape_id).collect();
        let unique_ids: std::collections::HashSet<_> = scene_ids.iter().copied().collect();
        assert_eq!(scene_ids.len(), unique_ids.len());
    }
}

#[test]
fn test_move_vertex_preserves_untouched_hole_contours() {
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
    let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].clone();
    assert!(orig_shape.holes().len() >= 30);

    // 頂点を動かす（本番の Action::MoveVertex）のを穴の多い図形に行ったとき、触っていない穴が操作前の Contour と同じもの（ptr_eq）として残ること
    let v_from = orig_shape.holes()[0].segments[0].p0;
    let v_to = v_from + kurbo::Vec2::new(1.0, 1.0);
    assert!(move_vertex(&mut app, v_from, v_to));
    let after_shape = &app.doc.root.layers[0].keyframes[0].cel.shapes()[0];
    assert_eq!(after_shape.holes().len(), orig_shape.holes().len());
    for untouched_hole in &orig_shape.holes()[1..] {
        assert!(
            after_shape
                .holes()
                .iter()
                .any(|h| std::sync::Arc::ptr_eq(&h.segments, &untouched_hole.segments)),
            "MoveVertexFromTo 後も触っていない穴が操作前の Contour と ptr_eq で共有されていること"
        );
    }
}

#[test]
fn test_adding_base_shape_to_floating_selection_keeps_floating_piece_on_top() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
        let holes_area: f64 = s.holes().iter().map(|h| h.signed_area().abs()).sum();
        s.exterior().signed_area().abs() - holes_area
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
            .map(|s| s.color());
        assert_eq!(
            top_color,
            Some(blue),
            "{label}: 選び足した後の重なり部分の一番上が青であること"
        );

        assert!(apply(&mut app, Action::Deselect));
        let cel_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
        let blue_area: f64 = cel_shapes
            .iter()
            .filter(|s| s.color() == blue)
            .map(|s| shape_area(s))
            .sum();
        let red_area: f64 = cel_shapes
            .iter()
            .filter(|s| s.color() == red)
            .map(|s| shape_area(s))
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

/// 頂点を動かして図形が丸ごと裏返ると、図形は消える（Undo で戻る）。一部だけが裏返ったときは、裏返った側だけが消える（利用者と決めた動き。2026-11-06）
#[test]
fn test_vertex_drag_flip_removes_flipped_part() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::vector_engine::FillShape;
    use kurbo::QuadBez;

    let make_poly = |pts: &[Point], lid: u64| -> FillShape {
        let segs: Vec<QuadBez> = (0..pts.len())
            .map(|i| {
                let p0 = pts[i];
                let p2 = pts[(i + 1) % pts.len()];
                QuadBez::new(p0, p0.lerp(p2, 0.5), p2)
            })
            .collect();
        let ext = positive_contour(segs);
        FillShape::new(Color32::BLACK, ext, Vec::new()).with_layer_id(lid)
    };

    let square_pts = [
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        Point::new(200.0, 200.0),
        Point::new(100.0, 200.0),
    ];
    let tri_pts = [
        Point::new(200.0, 100.0),
        Point::new(250.0, 200.0),
        Point::new(150.0, 200.0),
    ];

    for (pts, from_pt, to_pt) in [
        (
            &square_pts[..],
            Point::new(100.0, 100.0),
            Point::new(230.0, 230.0),
        ),
        (
            &square_pts[..],
            Point::new(100.0, 100.0),
            Point::new(320.0, 320.0),
        ),
        (
            &tri_pts[..],
            Point::new(200.0, 100.0),
            Point::new(200.0, 320.0),
        ),
    ] {
        let mut app = Is2dApp::default();
        let lid = app.doc.root.layers[0].id;
        put_cel_shapes(&mut app, 0, 0, vec![make_poly(pts, lid)]);
        assert!(move_vertex(&mut app, from_pt, to_pt));
        assert!(app.doc.root.layers[0].keyframes[0].cel.shapes().is_empty());
        assert!(apply(&mut app, Action::Undo));
        assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 1);
    }

    // 一部だけが裏返る場合：裏返っていない側（面積 1,250）だけが残ること
    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    put_cel_shapes(&mut app, 0, 0, vec![make_poly(&square_pts, lid)]);
    assert!(move_vertex(&mut app, Point::new(100.0, 100.0), Point::new(300.0, 150.0)));
    let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
    assert_eq!(shapes.len(), 1);
    let area = shapes[0].exterior().signed_area();
    assert!(
        (area - 1250.0).abs() < 20.0,
        "裏返っていない側の三角形（約 1,250）が残ること: {area}"
    );
}

/// 青の図形の外周の区間の数
fn blue_segment_count(app: &Is2dApp) -> usize {
    scene_summary(app)
        .shapes
        .iter()
        .filter(|x| x.color() == Color32::BLUE)
        .map(|x| x.exterior().segments.len())
        .sum()
}

/// 赤の横線（y = 200、太さ 40）の上に、青の縦線（x = 300、太さ 30）を重ねて描く
fn crossed_brush_strokes() -> Is2dApp {
    let mut app = Is2dApp::default();
    let line = |a: Point, b: Point| -> Vec<Point> {
        (0..=200).map(|i| a.lerp(b, i as f64 / 200.0)).collect()
    };
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: line(Point::new(100.0, 200.0), Point::new(500.0, 200.0)),
                radius: 20.0,
                color: Color32::RED,
            },
            Action::PaintStroke {
                points: line(Point::new(300.0, 100.0), Point::new(300.0, 300.0)),
                radius: 15.0,
                color: Color32::BLUE,
            },
        ],
    );
    app
}

/// バケツで塗った境目の辺・頂点をドラッグすると、円の線の内側のふちも一緒に動き、すき間も重なりもできない。Undo 1回で両方戻る
#[test]
fn test_drag_bucket_fill_boundary_moves_both_sides() {
    let in_disk = |p: Point| (p - Point::new(300.0, 300.0)).hypot() < 106.0;
    let area = (190.0, 190.0, 410.0, 410.0);
    // 内側のふち（半径 92）の、描き始めと描き終わりが重なる所（右端）から離れた所（下端）をつかむ
    for (from, to) in [
        (Point::new(300.0, 392.0), Point::new(300.0, 370.0)),
        (Point::new(300.0, 392.0), Point::new(300.0, 400.0)),
    ] {
        let mut app = ring_with_bucket_fill();
        assert!(pull_edge(&mut app, from, to));
        assert_eq!(
            shared_boundary_coverage(&app, in_disk, area),
            (0, 0),
            "辺を {from:?} から {to:?} へ"
        );
    }
    let mut app = ring_with_bucket_fill();
    let before: Vec<(Color32, f64)> = scene_summary(&app)
        .shapes
        .iter()
        .map(|s| (s.color(), s.exterior().signed_area()))
        .collect();
    let v = scene_summary(&app)
        .shapes
        .iter()
        .find(|s| s.color() == Color32::RED)
        .unwrap()
        .exterior()
        .segments[2]
        .p0;
    let to = v + (Point::new(300.0, 300.0) - v) * (15.0 / (Point::new(300.0, 300.0) - v).hypot());
    assert!(move_vertex(&mut app, v, to));
    assert_eq!(
        shared_boundary_coverage(&app, in_disk, area),
        (0, 0),
        "頂点を動かす"
    );
    run(&mut app, &[Action::Undo]);
    let after: Vec<(Color32, f64)> = scene_summary(&app)
        .shapes
        .iter()
        .map(|s| (s.color(), s.exterior().signed_area()))
        .collect();
    assert_eq!(before, after, "Undo 1回で両方戻る");
}

/// ブラシを重ねた境目をドラッグすると、下の図形の切り口も一緒に動く。下の図形の短い切り口や角をつかんでも、
/// 上の図形の長い辺が境目の端で切り分けられて一緒に動く
#[test]
fn test_drag_overpainted_brush_boundary_moves_both_sides() {
    let in_strip = |p: Point| p.y > 181.0 && p.y < 219.0;
    let area = (110.0, 181.0, 490.0, 219.0);
    // 青の右の辺を、赤の帯の所で青の中へ
    let mut app = crossed_brush_strokes();
    assert!(pull_edge(&mut app, Point::new(315.0, 200.0), Point::new(305.0, 200.0)));
    assert_eq!(shared_boundary_coverage(&app, in_strip, area), (0, 0));
    // 赤（左）の切り口を直接つかんで右へ、赤（左）の角の頂点を直接動かす
    for move_vertex in [false, true] {
        let mut app = crossed_brush_strokes();
        let blue_before = blue_segment_count(&app);
        let s = scene_summary(&app);
        let red = s
            .shapes
            .iter()
            .find(|x| x.color() == Color32::RED && x.bounding_box().x1 < 300.0)
            .unwrap();
        let si = red
            .exterior()
            .segments
            .iter()
            .position(|q| (q.p0.x - 285.0).abs() < 1.0 && (q.p2.x - 285.0).abs() < 1.0)
            .unwrap();
        let seg = red.exterior().segments[si];
        // 境目の線は1本なので、赤の側からつかんでも同じ辺・同じ頂点になる
        let action = if move_vertex {
            Op::MoveVertex(seg.p0, seg.p0 + KVec2::new(-9.0, -8.0))
        } else {
            Op::PullEdge(seg.eval(0.5), seg.eval(0.5) + KVec2::new(12.0, 0.0))
        };
        assert!(op(&mut app, action));
        assert_eq!(
            shared_boundary_coverage(&app, in_strip, area),
            (0, 0),
            "頂点 {move_vertex}"
        );
        assert_eq!(
            blue_segment_count(&app),
            blue_before,
            "青の輪郭は、左右の境目にある赤の角 4 か所で切り分けられたまま（区間の数が変わらない）"
        );
    }
}

/// 3つの図形が接する点（分かれ目）をまたぐ辺を、分かれ目より外でつかむと、分かれ目と分かれ目の間だけが曲がり、
/// 分かれ目の向こうの図形は変わらない（Animate と同じ）。ドラッグ中の表示も、離した後と同じ
#[test]
fn test_drag_edge_bends_only_between_junctions() {
    let reds = |app: &Is2dApp| -> Vec<Vec<QuadBez>> {
        let mut v: Vec<Vec<QuadBez>> = scene_summary(app)
            .shapes
            .iter()
            .filter(|s| s.color() == Color32::RED)
            .map(|s| s.exterior().segments.to_vec())
            .collect();
        v.sort_by(|a, b| a[0].p0.x.total_cmp(&b[0].p0.x));
        v
    };
    let mut app = crossed_brush_strokes();
    let reds_before = reds(&app);
    let blue_before = blue_segment_count(&app);
    tool_select::start_reshape_drag(&mut app, Point::new(285.0, 140.0), false, false);
    tool_select::update_reshape_drag(&mut app, Point::new(270.0, 140.0));
    let during: Vec<QuadBez> = app
        .scene()
        .iter()
        .filter(|s| s.color() == Color32::BLUE)
        .flat_map(|s| s.exterior().segments.to_vec())
        .collect();
    let reds_during: Vec<Vec<QuadBez>> = {
        let mut v: Vec<Vec<QuadBez>> = app
            .scene()
            .iter()
            .filter(|s| s.color() == Color32::RED)
            .map(|s| s.exterior().segments.to_vec())
            .collect();
        v.sort_by(|a, b| a[0].p0.x.total_cmp(&b[0].p0.x));
        v
    };
    tool_select::finish_reshape_drag(&mut app, None);
    let after: Vec<QuadBez> = scene_summary(&app)
        .shapes
        .iter()
        .filter(|s| s.color() == Color32::BLUE)
        .flat_map(|s| s.exterior().segments.to_vec())
        .collect();
    assert_eq!(reds_during, reds_before, "ドラッグ中も赤は変わらない");
    assert_eq!(reds(&app), reds_before, "離した後も赤は変わらない");
    assert_eq!(
        after.len(),
        blue_before,
        "青の輪郭は、左右の境目にある赤の角 4 か所で切り分けられたまま（区間の数が変わらない）"
    );
    assert_eq!(during, after, "ドラッグ中の表示と、離した後の形が同じ");
}

/// 3色が接する点の頂点をつかんで動かせ、すき間も重なりもできない
#[test]
fn test_drag_three_color_junction_vertex() {
    let c = Point::new(300.0, 300.0);
    let app0 = three_color_junction_scene();
    let s = scene_summary(&app0);
    let mut junctions: Vec<Point> = s
        .shapes
        .iter()
        .filter(|x| x.color() == Color32::BLUE || x.color() == Color32::GREEN)
        .flat_map(|x| x.exterior().segments.iter().map(|q| q.p0))
        .filter(|p| {
            ((*p - c).hypot() - 92.0).abs() < 1.5
                && ((p.y - 294.0).abs() < 1.0 || (p.y - 306.0).abs() < 1.0)
        })
        .collect();
    junctions.dedup_by(|a, b| (*a - *b).hypot() < 0.1);
    assert_eq!(junctions.len(), 4);
    for j in junctions {
        let mut app = three_color_junction_scene();
        let to = j
            + (c - j) * (6.0 / (c - j).hypot())
            + KVec2::new(0.0, if j.y < 300.0 { -2.0 } else { 2.0 });
        assert!(
            move_vertex(&mut app, j, to),
            "{j:?} の頂点をつかめる"
        );
        assert_eq!(
            shared_boundary_coverage(
                &app,
                |p| (p - c).hypot() < 106.0,
                (190.0, 190.0, 410.0, 410.0)
            ),
            (0, 0),
            "{j:?}"
        );
    }
}

/// 輪郭の変形のカーソル合わせに前後は無い：下の図形（赤）の角が上の図形（青）の辺の上にあってもつかめ、
/// 境目の辺は分かれ目（赤の角）と分かれ目の間だけが強調される
#[test]
fn test_reshape_hover_has_no_front_and_back() {
    let app = crossed_brush_strokes();
    let s = scene_summary(&app);
    let red = s
        .shapes
        .iter()
        .find(|x| x.color() == Color32::RED && x.bounding_box().x1 < 300.0)
        .unwrap();
    let corner = red
        .exterior()
        .segments
        .iter()
        .map(|q| q.p0)
        .filter(|p| (p.x - 285.0).abs() < 1.0)
        .min_by(|a, b| a.y.total_cmp(&b.y))
        .unwrap();
    let hover = |pt: Point| {
        crate::canvas_tool::select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            pt,
            app.view.zoom(),
            crate::canvas_tool::SelectSubMode::Reshape,
        )
    };
    match hover(corner + KVec2::new(0.5, 0.5)) {
        crate::canvas_tool::HoverTarget::Vertex {
            pos,
            map_node: Some(_),
            ..
        } => {
            assert!((pos - corner).hypot() < 1e-9, "赤の角そのものをつかむ");
        }
        other => panic!("赤の角は頂点としてつかめる：{other:?}"),
    }
    match hover(Point::new(285.5, 200.0)) {
        crate::canvas_tool::HoverTarget::Edge {
            seg,
            map_edge: Some(_),
            ..
        } => {
            assert!(
                seg.p0.y > 175.0 && seg.p0.y < 225.0 && seg.p2.y > 175.0 && seg.p2.y < 225.0,
                "境目の辺は赤の角と角の間だけ：{seg:?}"
            );
        }
        other => panic!("境目は辺としてつかめる：{other:?}"),
    }
}

/// 境目をドラッグしても、境目に接していない図形はそのまま（作り直さない）。触れた図形だけが差し替わる
#[test]
fn test_reshape_rewrites_only_touched_shapes() {
    let mut app = three_color_junction_scene();
    let before = scene_summary(&app).shapes.clone();
    let red_ids: Vec<u64> = before
        .iter()
        .filter(|s| s.color() == Color32::RED)
        .map(|s| s.shape_id)
        .collect();
    // 青と黒の境目（円の内側のふち、上のほう）を少しだけ外へ
    let from = Point::new(300.0, 300.0 - 92.0);
    assert!(pull_edge(&mut app, from, from + KVec2::new(0.0, -2.0)));
    let after = scene_summary(&app).shapes.clone();
    for id in &red_ids {
        let (b, a) = (
            before.iter().find(|s| s.shape_id == *id).unwrap(),
            after
                .iter()
                .find(|s| s.shape_id == *id)
                .expect("赤はそのまま残る"),
        );
        assert!(std::sync::Arc::ptr_eq(
            &b.exterior().segments,
            &a.exterior().segments
        ));
    }
    // 青は新しい形に差し替わる（形の変わった図形は、番号はそのままで新しい版になる）
    assert!(
        !after.iter().any(|s| s.color() == Color32::BLUE
            && before
                .iter()
                .any(|b| (b.shape_id, b.mesh_revision) == (s.shape_id, s.mesh_revision))),
        "青は差し替わる"
    );
    assert!(
        after.iter().any(|s| s.color() == Color32::BLUE
            && before
                .iter()
                .any(|b| b.color() == Color32::BLUE && b.shape_id == s.shape_id)),
        "形の変わった青は、元の番号を引き継ぐ"
    );
}

/// 輪郭の変形のドラッグ中も、離したときと同じ計算の結果を見せる：線を交わらせて引っぱっても、
/// ドラッグ中の見た目（どの点が何色か）と離した後の形が同じ（重ね順で潜ったり乗ったりしない）
#[test]
fn test_reshape_preview_matches_commit_when_crossing() {
    let colors_on_grid = |shapes: &[FillShape]| -> Vec<Option<Color32>> {
        let mut out = Vec::new();
        let mut y = 182.0;
        while y < 420.0 {
            let mut x = 182.0;
            while x < 420.0 {
                let p = Point::new(x + 0.137, y + 0.173);
                let hits: Vec<Color32> = shapes
                    .iter()
                    .filter(|s| s.contains_point(p))
                    .map(|s| s.color())
                    .collect();
                assert!(hits.len() <= 1, "重なりがない {p:?}");
                out.push(hits.first().copied());
                x += 2.0;
            }
            y += 2.0;
        }
        out
    };
    // 3色の場面で、青と赤の境目（赤の帯の上のふち）を、赤の帯と緑を突き抜けるまで下へ
    for (from, to) in [
        (Point::new(250.0, 294.0), Point::new(250.0, 340.0)),
        (Point::new(330.0, 294.0), Point::new(345.0, 330.0)),
    ] {
        let mut app = three_color_junction_scene();
        tool_select::start_reshape_drag(&mut app, from, false, false);
        assert!(matches!(
            app.session.drag,
            crate::canvas_tool::ActiveCanvasDrag::Select(_)
        ));
        tool_select::update_reshape_drag(&mut app, to);
        let during: Vec<FillShape> = app.scene().iter().cloned().collect();
        let during = colors_on_grid(&during);
        tool_select::finish_reshape_drag(&mut app, None);
        let after = colors_on_grid(&scene_summary(&app).shapes);
        assert_eq!(
            during, after,
            "ドラッグ中の見た目と離した後が同じ（{from:?} → {to:?}）"
        );
    }
}

/// 矩形選択・クリック・ダブルクリック（Shift で足すものも）を混ぜて選んだあとのドラッグ（Alt の複製も）でも、ドラッグ中の
/// 見た目（どの点が何色か）は離した後の形と同じ（利用者の指摘：きのこの線の絵で、ドラッグ中だけ一部の線が置いていかれ、
/// 離すと正しく動いた）。直す前は、線だけの絵の 200 通り中 17 通りで違った。原因は3つ：
/// - 片を切り出すとき、選ぶ所の線を、中を消す側（残り）にも置いた（`MapRule::stroke`：選んだ線が片と残りの両方に入った）
/// - 片の番号の一覧が面の番号だけで、線の番号を含まなかった（`PieceGroup::ids`：クリックで一部の片だけ残すとき、線の片を
///   残りに戻せず、片と残りの両方に残った）
/// - ドラッグ中の見た目が、選んでいる残り（矩形の後に Shift のダブルクリックで足した線）を動かさなかった
///   （`Session::begin_selection_overlay`。確定は持ち上げて動かす）
///
/// 線で縁どった塗りの絵での Alt の複製も：動かしていない矩形の切り口は、確定で捨てて元に戻してから写しを置くので、ドラッグ
/// 中の止まった絵もセル画のまま（前は切った片と残りを出し、レイヤーのいちばん上に描く片の塗りが、切り口のすぐ近くを通る
/// 残りの線の半分を覆って見えた）
#[test]
fn dragging_after_mixed_selections_matches_the_drop() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ink = crate::planar_map::EdgeStroke { color: Color32::from_rgb(20, 40, 90), width: 8 };
    let line = |a: Point, b: Point| -> Vec<Point> { (0..=40).map(|i| a.lerp(b, i as f64 / 40.0)).collect() };
    let colors_on_grid = |shapes: &[FillShape]| -> Vec<Option<Color32>> {
        let mut out = Vec::new();
        let mut y = 150.0;
        while y < 640.0 {
            let mut x = 80.0;
            while x < 680.0 {
                let p = Point::new(x + 0.137, y + 0.173);
                out.push(shapes.iter().rev().find(|s| s.contains_point(p)).map(|s| s.color()));
                x += 3.0;
            }
            y += 3.0;
        }
        out
    };
    let mut rng = fastrand::Rng::with_seed(0x6b17_0c0de);
    let mut bad = Vec::new();
    for case in 0..400 {
        let mut app = Is2dApp::default();
        apply(&mut app, Action::SetTool { tool: Tool::Pencil });
        for (a, b) in [
            (Point::new(100.0, 200.0), Point::new(500.0, 200.0)),
            (Point::new(220.0, 200.0), Point::new(220.0, 450.0)),
            (Point::new(380.0, 200.0), Point::new(380.0, 450.0)),
        ] {
            assert!(apply(&mut app, Action::PencilStroke { points: line(a, b), stroke: ink }));
        }
        // 半分の回は、線で縁どった塗りも置く（塗りと線は別々に選ぶ。選んだ塗りの縁の、選んでいない線が残りに残るか）
        let with_fill = case % 2 == 1;
        if with_fill {
            put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(420.0, 300.0, 520.0, 420.0, Color32::from_rgb(230, 180, 60))]);
            assert!(apply(&mut app, Action::InkBottle { pt: Point::new(470.0, 360.0), stroke: ink }));
        }
        apply(&mut app, Action::SetTool { tool: Tool::Select });
        let rect = |rng: &mut fastrand::Rng| {
            let (x, y) = (90.0 + rng.f64() * 400.0, 150.0 + rng.f64() * 300.0);
            (Point::new(x, y), Point::new(x + 30.0 + rng.f64() * 250.0, y + 30.0 + rng.f64() * 250.0))
        };
        // 選び方：矩形・Shift の矩形・クリック・Shift のクリック・ダブルクリック・Shift のダブルクリックを混ぜる（線の上の点）
        let on_line = |rng: &mut fastrand::Rng| match rng.usize(0..5) {
            0 => Point::new(100.0 + rng.f64() * 400.0, 200.0),
            1 => Point::new(220.0, 200.0 + rng.f64() * 250.0),
            2 => Point::new(380.0, 200.0 + rng.f64() * 250.0),
            3 => Point::new(430.0 + rng.f64() * 80.0, 310.0 + rng.f64() * 100.0),
            _ => Point::new(420.0 + rng.f64() * 100.0, 300.0),
        };
        let n = 1 + rng.usize(0..5);
        let mut ops = Vec::new();
        for k in 0..n {
            let shift = k > 0 && rng.bool();
            match rng.usize(0..3) {
                0 => {
                    let (p0, p1) = rect(&mut rng);
                    ops.push(format!("矩形{}", if shift { "+" } else { "" }));
                    apply(&mut app, Action::SelectMarquee { p0, p1, additive: shift });
                }
                1 => {
                    let pt = on_line(&mut rng);
                    ops.push(format!("クリック{}", if shift { "+" } else { "" }));
                    crate::canvas_tool::select_ops::apply_click_selection(&mut app, pt, shift, true);
                }
                _ => {
                    let pt = on_line(&mut rng);
                    ops.push(format!("ダブル{}", if shift { "+" } else { "" }));
                    select_ops::handle_canvas_double_click(&mut app, pt, shift);
                }
            }
        }
        if app.session.selection().is_empty() {
            continue;
        }
        let from = Point::new(300.0, 300.0);
        let alt = rng.bool();
        let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, from, alt);
        app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
        crate::canvas_tool::select_ops::update_selection_drag(&mut app, from + KVec2::new(15.0, 140.0));
        let during: Vec<FillShape> = app.scene().iter().cloned().collect();
        let during = colors_on_grid(&during);
        crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
        let after = colors_on_grid(&app.scene().iter().cloned().collect::<Vec<_>>());
        let diff = during.iter().zip(&after).filter(|(a, b)| a != b).count();
        if diff > 0 {
            bad.push(format!("{case}（{}{}）：{diff} 点", ops.join("・"), if alt { "・Alt" } else { "" }));
        }
    }
    assert!(bad.is_empty(), "ドラッグ中の見た目と離した後が違う：{bad:?}");
}

/// 動かした浮いた片があるときの複製のドラッグで、ドラッグ中に見せた確定した絵の画面の写しを、離したあとの画面の土台が
/// 同じ物で使い回す（確定の答えの地図の写しも、離したときの書き戻しが同じものを使う：`LayerBase::of_local`。設計：
/// claude/画面の図形の一覧を触った所だけで作る設計_f8b299c.md の 2.3）
#[test]
fn alt_duplicate_drop_reuses_the_committed_picture_from_the_drag() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(200.0, 350.0), additive: false }));
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(30.0, 40.0) }));
    let from = Point::new(150.0, 200.0);
    let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, from, true);
    // 画面の図形の番地で見る：ドラッグの前の土台 `b0`、ドラッグ中の画面 `during`、離したあとの土台 `b1`
    let addrs = |it: &mut dyn Iterator<Item = *const FillShape>| it.collect::<std::collections::HashSet<_>>();
    let b0 = addrs(&mut app.session.base_cache().shapes().iter().map(std::sync::Arc::as_ptr));
    let during = addrs(&mut app.scene().iter().map(|s| s as *const FillShape));
    app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
    crate::canvas_tool::select_ops::update_selection_drag(&mut app, from + KVec2::new(200.0, 0.0));
    crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
    let b1 = addrs(&mut app.session.base_cache().shapes().iter().map(std::sync::Arc::as_ptr));
    // 離したあとの土台で新しくなった図形（確定した絵）は、みなドラッグ中に画面に出した物と同じ物
    let fresh: Vec<_> = b1.difference(&b0).collect();
    assert!(!fresh.is_empty(), "確定した絵が土台に入っていない");
    assert!(fresh.iter().all(|p| during.contains(p)), "離したあとの画面の土台が、ドラッグ中の確定した絵を作り直した");
}

/// ドラッグ中の最後の見た目と、離したあとの見た目が同じ（お手本の比べ：設計 claude/浮いた片と選択の決まりを1つにする設計_ddcd41e.md
/// の 2.2）。ブラシ・鉛筆・インクボトル・グループ・2枚目のレイヤーを混ぜた絵で、矩形・クリック・ダブルクリック（Shift も）
/// で選び、半分の回は先に選んだものを動かしてから（動かした浮いた片がある所で）選び足し、移動・Alt の複製・拡大・回転の
/// ドラッグをする（設計審査 de92ceb の D-1 の (c)・レビュー de92ceb の N-3001）
#[test]
fn drag_previews_match_the_drop() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ink = crate::planar_map::EdgeStroke { color: Color32::from_rgb(20, 40, 90), width: 6 };
    let line = |a: Point, b: Point| -> Vec<Point> { (0..=30).map(|i| a.lerp(b, i as f64 / 30.0)).collect() };
    let colors_on_grid = |app: &Is2dApp| -> Vec<Option<Color32>> {
        let shapes: Vec<FillShape> = app.scene().iter().cloned().collect();
        let mut out = Vec::new();
        let mut y = 60.0;
        while y < 760.0 {
            let mut x = 40.0;
            while x < 900.0 {
                let p = Point::new(x + 0.137, y + 0.173);
                out.push(shapes.iter().rev().find(|s| s.contains_point(p)).map(|s| s.color()));
                x += 4.0;
            }
            y += 4.0;
        }
        out
    };
    let mut rng = fastrand::Rng::with_seed(0xd1_0e_51);
    let mut bad: Vec<String> = Vec::new();
    let cases: usize = std::env::var("IS2D_DRAG_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(2000);
    for case in 0..cases {
        let mut app = Is2dApp::default();
        let colors = [Color32::from_rgb(230, 80, 60), Color32::from_rgb(70, 160, 90), Color32::from_rgb(240, 200, 60)];
        for _ in 0..(2 + rng.usize(0..3)) {
            let a = Point::new(150.0 + rng.f64() * 450.0, 150.0 + rng.f64() * 350.0);
            let b = a + KVec2::new(rng.f64() * 200.0 - 100.0, rng.f64() * 200.0 - 100.0);
            apply(&mut app, Action::PaintStroke { points: line(a, b), radius: 20.0 + rng.f32() * 30.0, color: colors[rng.usize(0..3)] });
        }
        apply(&mut app, Action::SetTool { tool: Tool::Pencil });
        for _ in 0..rng.usize(0..3) {
            let a = Point::new(120.0 + rng.f64() * 500.0, 120.0 + rng.f64() * 400.0);
            let b = a + KVec2::new(rng.f64() * 300.0 - 150.0, rng.f64() * 300.0 - 150.0);
            apply(&mut app, Action::PencilStroke { points: line(a, b), stroke: ink });
        }
        if rng.bool() {
            apply(&mut app, Action::InkBottle { pt: Point::new(150.0 + rng.f64() * 450.0, 150.0 + rng.f64() * 350.0), stroke: ink });
        }
        apply(&mut app, Action::SetTool { tool: Tool::Select });
        if rng.bool() {
            let p0 = Point::new(100.0 + rng.f64() * 300.0, 100.0 + rng.f64() * 300.0);
            apply(&mut app, Action::SelectMarquee { p0, p1: p0 + KVec2::new(150.0, 150.0), additive: false });
            apply(&mut app, Action::Group);
            apply(&mut app, Action::Deselect);
        }
        if rng.bool() {
            apply(&mut app, Action::AddLayer);
            apply(&mut app, Action::PaintStroke { points: line(Point::new(300.0, 300.0), Point::new(520.0, 380.0)), radius: 25.0, color: colors[rng.usize(0..3)] });
        }
        let on_art = |rng: &mut fastrand::Rng| Point::new(120.0 + rng.f64() * 520.0, 120.0 + rng.f64() * 420.0);
        let mut ops: Vec<String> = Vec::new();
        let select = |app: &mut Is2dApp, rng: &mut fastrand::Rng, ops: &mut Vec<String>, first: bool| {
            for k in 0..(1 + rng.usize(0..4)) {
                let shift = (k > 0 || !first) && rng.bool();
                match rng.usize(0..3) {
                    0 => {
                        let p0 = on_art(rng);
                        let p1 = p0 + KVec2::new(30.0 + rng.f64() * 250.0, 30.0 + rng.f64() * 250.0);
                        ops.push(format!("矩形{}", if shift { "+" } else { "" }));
                        apply(app, Action::SelectMarquee { p0, p1, additive: shift });
                    }
                    1 => {
                        ops.push(format!("クリック{}", if shift { "+" } else { "" }));
                        crate::canvas_tool::select_ops::apply_click_selection(app, on_art(rng), shift, true);
                    }
                    _ => {
                        ops.push(format!("ダブル{}", if shift { "+" } else { "" }));
                        select_ops::handle_canvas_double_click(app, on_art(rng), shift);
                    }
                }
            }
        };
        select(&mut app, &mut rng, &mut ops, true);
        if rng.bool() && !app.session.selection().is_empty() {
            ops.push("移動".into());
            apply(&mut app, Action::NudgeSelection { delta: KVec2::new(rng.f64() * 120.0 - 60.0, rng.f64() * 120.0 - 60.0) });
            select(&mut app, &mut rng, &mut ops, false);
        }
        if app.session.selection().is_empty() {
            continue;
        }
        let kind = rng.usize(0..4);
        let during;
        match kind {
            0 | 1 => {
                let from = Point::new(300.0, 300.0);
                let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, from, kind == 1);
                app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
                crate::canvas_tool::select_ops::update_selection_drag(&mut app, from + KVec2::new(23.0, 117.0));
                during = colors_on_grid(&app);
                crate::canvas_tool::select_ops::finish_selection_drag(&mut app, None);
            }
            _ => {
                let zoom = app.tab().view.zoom();
                let Some((bbox, pivot)) = app.session.selection_bbox_and_pivot(zoom) else {
                    continue;
                };
                let (start, end) = if kind == 2 {
                    let start = Point::new(bbox.x1, bbox.center().y);
                    (start, start + KVec2::new(60.0, 0.0))
                } else {
                    let start = Point::new(bbox.x1 + 15.0 / zoom as f64, bbox.y0 - 15.0 / zoom as f64);
                    (start, kurbo::Affine::rotate_about(0.5, pivot) * start)
                };
                tool_transform::start_transform_drag(&mut app, start, false, false);
                tool_transform::update_transform_drag(&mut app, end, false, false);
                during = colors_on_grid(&app);
                tool_transform::finish_transform_drag(&mut app, Some(end));
            }
        }
        let after = colors_on_grid(&app);
        let diff = during.iter().zip(&after).filter(|(a, b)| a != b).count();
        if diff > 0 {
            let name = ["移動", "Alt", "拡大", "回転"][kind];
            bad.push(format!("{case}（{}・{name}）：{diff} 点", ops.join("・")));
        }
    }
    assert!(bad.is_empty(), "ドラッグ中の見た目と離した後が違う（{} 回）：{bad:?}", bad.len());
}

/// 円の上のフチを下のフチの向こうまで引っぱると、離す前から2つに分かれて見える
#[test]
fn test_reshape_preview_shows_split_during_drag() {
    let mut app = Is2dApp::default();
    let center = Point::new(200.0, 200.0);
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![center, center + KVec2::new(0.01, 0.0)],
            radius: 50.0,
            color: Color32::BLACK,
        }],
    );
    let top = Point::new(200.0, 150.0);
    let from = scene_summary(&app).shapes[0]
        .exterior()
        .segments
        .iter()
        .map(|q| q.eval(0.5))
        .min_by(|a, b| (*a - top).hypot().total_cmp(&(*b - top).hypot()))
        .unwrap();
    tool_select::start_reshape_drag(&mut app, from, false, false);
    tool_select::update_reshape_drag(&mut app, Point::new(200.0, 275.0));
    let during = app
        .scene()
        .iter()
        .filter(|s| !s.exterior().segments.is_empty())
        .count();
    assert_eq!(during, 2, "ドラッグ中から2つに分かれて見える");
    tool_select::finish_reshape_drag(&mut app, None);
    assert_eq!(scene_summary(&app).shapes.len(), 2);
}

/// 向こう側のフチの近くまで引っぱっても、ドラッグ中に形が勝手に先へ飛ばない（押し出さない）。離した後も同じ
#[test]
fn test_reshape_drag_near_opposite_edge_does_not_jump() {
    let mut app = Is2dApp::default();
    let center = Point::new(200.0, 200.0);
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![center, center + KVec2::new(0.01, 0.0)],
            radius: 50.0,
            color: Color32::BLACK,
        }],
    );
    let top = Point::new(200.0, 150.0);
    let from = scene_summary(&app).shapes[0]
        .exterior()
        .segments
        .iter()
        .map(|q| q.eval(0.5))
        .min_by(|a, b| (*a - top).hypot().total_cmp(&(*b - top).hypot()))
        .unwrap();
    tool_select::start_reshape_drag(&mut app, from, false, false);
    tool_select::update_reshape_drag(&mut app, Point::new(200.0, 248.0));
    let covered = |shapes: &[FillShape], p: Point| shapes.iter().any(|s| s.contains_point(p));
    let during: Vec<FillShape> = app.scene().iter().cloned().collect();
    assert_eq!(
        during
            .iter()
            .filter(|s| !s.exterior().segments.is_empty())
            .count(),
        1,
        "向こう側のフチの手前では、分かれて見えない"
    );
    assert!(
        covered(&during, Point::new(200.0, 249.3)),
        "引っぱった辺と向こう側のフチの間の細い所が残って見える"
    );
    tool_select::finish_reshape_drag(&mut app, None);
    let after = scene_summary(&app).shapes.clone();
    assert_eq!(after.len(), 1);
    assert!(covered(&after, Point::new(200.0, 249.3)));
}

/// 矩形で一部を選んで動かさずに、残りや片の境目の角を消しても、隣の図形との間に穴があかない（N-2001）。
/// 動かしていない片と残りは地図から切り出したもので、境目を隣と共有するので、選択を外して地図で両側を一緒に変える
#[test]
fn test_anchor_delete_on_unmoved_selection_keeps_shared_border() {
    for right_side in [false, true] {
        let mut app = Is2dApp::default();
        app.tool.brush_color = Color32::RED;
        let red: Vec<Point> = (0..=10)
            .map(|i| Point::new(-150.0 + 30.0 * i as f64, 0.0))
            .collect();
        commit_brush_line(&mut app, &brush_line(&red, 40.0));
        app.tool.brush_color = Color32::BLACK;
        // 曲がった線（輪郭の途中に頂点ができる）を、赤の帯に沿わせる
        let black: Vec<Point> = (0..=24)
            .map(|i| {
                let x = -120.0 + 10.0 * i as f64;
                Point::new(x, 6.0 * (x / 25.0).sin())
            })
            .collect();
        commit_brush_line(&mut app, &brush_line(&black, 16.0));
        let covered = |app: &Is2dApp, p: Point| app.scene().iter().any(|s| s.contains_point(p));
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectMarquee {
                p0: Point::new(0.0, -200.0),
                p1: Point::new(300.0, 200.0),
                additive: false,
            },
        ));
        // 赤と黒の境目にある頂点（赤の図形と黒の図形の両方の輪郭にある点）を、残り（左）か片（右）から選ぶ
        app.session.floating().expect("選択");
        let side: Vec<FillShape> = if right_side {
            app.session.floating_view().pieces().to_vec()
        } else {
            app.session.floating_view().remainder().cloned().collect()
        };
        let verts = |c: Color32| -> Vec<Point> {
            side.iter()
                .filter(|s| s.color() == c)
                .flat_map(|s| {
                    s.contours()
                        .flat_map(|k| k.segments.iter().map(|q| q.p0).collect::<Vec<_>>())
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let blacks = verts(Color32::BLACK);
        let v = verts(Color32::RED)
            .into_iter()
            .filter(|p| p.y.abs() < 15.0 && (p.x > 5.0) == right_side && p.x.abs() > 5.0)
            .find(|p| blacks.iter().any(|b| b == p))
            .expect("赤と黒の境目の頂点");
        let probe: Vec<Point> = (0..80)
            .flat_map(|i| {
                (0..80).map(move |j| {
                    Point::new(
                        v.x - 20.0 + 0.5 * i as f64 + 0.13,
                        v.y - 20.0 + 0.5 * j as f64 + 0.17,
                    )
                })
            })
            .collect();
        let before: Vec<bool> = probe.iter().map(|p| covered(&app, *p)).collect();
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::DeleteAnchor { pt: v }
        ));
        doc_actions::apply(&mut app, doc_actions::Action::Deselect);
        let holes = probe
            .iter()
            .zip(&before)
            .filter(|(p, was)| **was && !covered(&app, **p))
            .count();
        assert_eq!(
            holes,
            0,
            "角を消したら穴があいた（{}）",
            if right_side { "片" } else { "残り" }
        );
        assert!(layer_map_is_valid(&app), "地図が壊れた");
    }
}

/// 動かして浮いている片の、色の違う片どうしの境目の角を消すと、両側の片が一緒に変わる（片側だけ変えて、隙間や重なりを
/// 作らない。レビュー 4a9ea60 の N-2002）
#[test]
fn test_floating_anchor_delete_on_shared_border_changes_both_pieces() {
    let mut app = Is2dApp::default();
    app.tool.brush_color = Color32::RED;
    let red: Vec<Point> = (0..=10)
        .map(|i| Point::new(-150.0 + 30.0 * i as f64, 0.0))
        .collect();
    commit_brush_line(&mut app, &brush_line(&red, 40.0));
    app.tool.brush_color = Color32::BLACK;
    let black: Vec<Point> = (0..=24)
        .map(|i| {
            let x = -120.0 + 10.0 * i as f64;
            Point::new(x, 6.0 * (x / 25.0).sin())
        })
        .collect();
    commit_brush_line(&mut app, &brush_line(&black, 16.0));
    for action in [
        doc_actions::Action::SelectMarquee {
            p0: Point::new(0.0, -200.0),
            p1: Point::new(300.0, 200.0),
            additive: false,
        },
        doc_actions::Action::NudgeSelection {
            delta: KVec2::new(0.0, 40.0),
        },
    ] {
        assert!(doc_actions::apply(&mut app, action));
    }
    let pieces = |app: &Is2dApp| app.session.floating_view().pieces().to_vec();
    assert!(app.session.has_modified_floating());
    // 赤と黒の境目にある角（赤の片と黒の片の両方の輪郭にある点）
    let verts = |c: Color32| -> Vec<Point> {
        pieces(&app)
            .iter()
            .filter(|s| s.color() == c)
            .flat_map(|s| {
                s.contours()
                    .flat_map(|k| k.segments.iter().map(|q| q.p0).collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let blacks = verts(Color32::BLACK);
    let v = verts(Color32::RED)
        .into_iter()
        .filter(|p| (p.y - 40.0).abs() < 15.0 && p.x > 20.0)
        .find(|p| blacks.contains(p))
        .expect("赤と黒の境目の角");
    let probe: Vec<Point> = (0..80)
        .flat_map(|i| {
            (0..80).map(move |j| {
                Point::new(
                    v.x - 20.0 + 0.5 * i as f64 + 0.13,
                    v.y - 20.0 + 0.5 * j as f64 + 0.17,
                )
            })
        })
        .collect();
    let covered = |ps: &[FillShape], p: Point| ps.iter().filter(|s| s.contains_point(p)).count();
    let before_pieces = pieces(&app);
    let before: Vec<usize> = probe.iter().map(|p| covered(&before_pieces, *p)).collect();
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::DeleteAnchor { pt: v }
    ));
    let after_pieces = pieces(&app);
    assert!(
        crate::planar_map::PlanarMap::from_faces(after_pieces.iter()).is_valid(),
        "片どうしの境目がそろわない"
    );
    let gaps = probe
        .iter()
        .zip(&before)
        .filter(|(p, was)| **was > 0 && covered(&after_pieces, **p) == 0)
        .count();
    assert_eq!(gaps, 0, "角を消したら片の間に隙間があいた");
    assert!(
        !after_pieces.iter().any(|s| s.color() == Color32::RED
            && s.contours().any(|k| k.segments.iter().any(|q| q.p0 == v)))
            && !after_pieces.iter().any(|s| s.color() == Color32::BLACK
                && s.contours().any(|k| k.segments.iter().any(|q| q.p0 == v))),
        "角が両側の片から消えていない"
    );
}

/// 同じ地図から切った残り（穴のあいた外側）に、残りの穴にはまる片（穴のあいた中央）を戻しても（重ねる）、何も消えない
/// （同じ色でつながる所は1つの面になる。レビュー 8b33f96 の N-1002）
#[test]
fn test_merge_touching_faces_keeps_piece_in_remainder_hole() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    use crate::vector_engine::make_marquee_rect_shape;
    use kurbo::Rect as KRect;
    let area = |s: &FillShape| {
        s.exterior().signed_area().abs() - s.holes().iter().map(|h| h.signed_area().abs()).sum::<f64>()
    };
    let mut square = make_marquee_rect_shape(KRect::new(0.0, 0.0, 200.0, 200.0));
    square.recolor(|_| Color32::RED);
    let lid = square.layer_id;
    let hole = make_marquee_rect_shape(KRect::new(80.0, 80.0, 120.0, 120.0));
    let map = PlanarMap::from_faces(std::iter::once(&square))
        .overlay(
            MapInput::Shapes(std::slice::from_ref(&hole)),
            MapRule::Paint(None),
        )
        .expect("穴をあける");
    let ring = map.faces().0;
    assert_eq!(ring.len(), 1);
    assert!((area(&map.face_shape(&ring[0], lid, &[])) - 38_400.0).abs() < 1e-6);
    let center = make_marquee_rect_shape(KRect::new(50.0, 50.0, 150.0, 150.0));
    let region = MapInput::Shapes(std::slice::from_ref(&center));
    let rest = map.overlay(region, MapRule::Paint(None)).expect("残り");
    let inside = map.overlay(region, MapRule::KeepInside).expect("片");
    assert_eq!(rest.faces().0.len() + inside.faces().0.len(), 2, "残りと片");
    let merged = rest
        .overlay(MapInput::Map(&inside), MapRule::Overlay)
        .expect("戻す");
    let faces = merged.faces().0;
    assert_eq!(faces.len(), 1);
    let total = area(&merged.face_shape(&faces[0], lid, &[]));
    assert!(
        (total - 38_400.0).abs() < 1e-6,
        "戻したら面積が {total} になった"
    );
}

/// アンカーは辺の端の近くでも、クリックした所にそのまま足す。足せないのは、新しい頂点が端と同じ格子の点に丸まる所だけ
/// （レビュー 6cca852 の N-1002）
#[test]
fn test_anchor_added_near_edge_end_at_clicked_position() {
    use crate::planar_map::PlanarMap;
    use crate::vector_engine::make_marquee_rect_shape;
    use kurbo::Rect as KRect;
    let mut square = make_marquee_rect_shape(KRect::new(0.0, 0.0, 200.0, 200.0));
    square.recolor(|_| Color32::RED);
    let map = PlanarMap::from_faces(std::iter::once(&square));
    assert!(map.is_valid());
    let e = 0;
    let q = map.quad(e);
    // 200px の辺の端から 6px（辺の長さの 3%）
    let added = map.with_anchor_added(e, 0.03).expect("端の近くにも足せる");
    assert!(added.is_valid());
    assert!(
        added
            .nodes
            .iter()
            .any(|p| (*p - q.eval(0.03)).hypot() < 1e-12),
        "クリックした所に角が入る"
    );
    // 端そのもの・格子の1目より近い所には足さない
    assert!(map.with_anchor_added(e, 0.0).is_none());
    assert!(map.with_anchor_added(e, 1e-7).is_none());
}

/// 両側に色がある地図の辺の真ん中（アンカーの追加でつかむ点）
fn shared_edge_midpoint(app: &Is2dApp, color_a: Color32, color_b: Color32) -> Point {
    let maps = crate::map_hover::scope_maps(app.scene(), &app.session);
    let m = &maps.maps[0].base.map();
    let e = (0..m.edges.len())
        .find(|&e| {
            let ed = &m.edges[e];
            (ed.left, ed.right) == (Some(color_a), Some(color_b))
                || (ed.left, ed.right) == (Some(color_b), Some(color_a))
        })
        .expect("境目の辺");
    m.quad(e).eval(0.5)
}

fn has_vertex_at(shape: &FillShape, p: Point) -> bool {
    shape
        .contours()
        .any(|c| c.segments.iter().any(|q| (q.p0 - p).hypot() < 1e-9))
}

/// アンカーの追加・削除も境目の線は1本：境目に頂点を足すと両側の図形に同じ頂点が入り、消すと両側から消える
#[test]
fn test_anchor_add_and_delete_on_shared_boundary_change_both_sides() {
    let mut app = ring_with_bucket_fill();
    let red = Color32::RED;
    let segs = |app: &Is2dApp, color: Color32| -> usize {
        scene_summary(app)
            .shapes
            .iter()
            .filter(|s| s.color() == color)
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum()
    };
    let (black0, red0) = (segs(&app, Color32::BLACK), segs(&app, red));
    let pt = shared_edge_midpoint(&app, Color32::BLACK, red);
    let target = crate::canvas_tool::select_ops::compute_hover_target(
        app.scene(),
        &app.session,
        pt,
        app.view.zoom(),
        crate::canvas_tool::SelectSubMode::AddAnchor,
    );
    let crate::canvas_tool::HoverTarget::Edge {
        closest_pos,
        map_edge: Some(_),
        ..
    } = target
    else {
        panic!("境目の辺をつかむ：{target:?}");
    };
    assert!(apply(&mut app, Action::AddAnchor { pt }));
    let s = scene_summary(&app).shapes.clone();
    let black = s.iter().find(|x| x.color() == Color32::BLACK).unwrap();
    let red_shape = s.iter().find(|x| x.color() == red).unwrap();
    assert!(has_vertex_at(black, closest_pos), "黒にも頂点が入る");
    assert!(has_vertex_at(red_shape, closest_pos), "赤にも頂点が入る");
    assert_eq!(segs(&app, Color32::BLACK), black0 + 1);
    assert_eq!(segs(&app, red), red0 + 1);
    assert_eq!(
        shared_boundary_coverage(
            &app,
            |p| (p - Point::new(300.0, 300.0)).hypot() < 106.0,
            (180.0, 180.0, 420.0, 420.0)
        ),
        (0, 0)
    );

    // 足した頂点を消すと、両側から消えて元の区間の数に戻る
    let undo_before = app.history.undo_stack.len();
    assert!(apply(&mut app, Action::DeleteAnchor { pt: closest_pos }));
    assert_eq!(app.history.undo_stack.len(), undo_before + 1);
    let s = scene_summary(&app).shapes.clone();
    assert!(
        s.iter().all(|x| !has_vertex_at(x, closest_pos)),
        "両側から消える"
    );
    assert_eq!(segs(&app, Color32::BLACK), black0);
    assert_eq!(segs(&app, red), red0);
    assert_eq!(
        shared_boundary_coverage(
            &app,
            |p| (p - Point::new(300.0, 300.0)).hypot() < 106.0,
            (180.0, 180.0, 420.0, 420.0)
        ),
        (0, 0)
    );
}

/// 3色以上が接する角は消せない（カーソルを合わせても削除の対象にならず、押しても何も変わらず Undo も積まない）
#[test]
fn test_anchor_delete_refuses_junction_vertex() {
    let mut app = crossed_brush_strokes();
    let s = scene_summary(&app);
    let red = s
        .shapes
        .iter()
        .find(|x| x.color() == Color32::RED && x.bounding_box().x1 < 300.0)
        .unwrap();
    let corner = red
        .exterior()
        .segments
        .iter()
        .map(|q| q.p0)
        .filter(|p| (p.x - 285.0).abs() < 1.0)
        .min_by(|a, b| a.y.total_cmp(&b.y))
        .unwrap();
    let target = crate::canvas_tool::select_ops::compute_hover_target(
        app.scene(),
        &app.session,
        corner,
        app.view.zoom(),
        crate::canvas_tool::SelectSubMode::DeleteAnchor,
    );
    assert!(
        !matches!(target, crate::canvas_tool::HoverTarget::Vertex { pos, .. } if (pos - corner).hypot() < 1e-9),
        "分かれ目の角は削除の対象にならない：{target:?}"
    );
    let undo_before = app.history.undo_stack.len();
    let shapes_before: Vec<u64> = scene_summary(&app)
        .shapes
        .iter()
        .map(|s| s.shape_id)
        .collect();
    assert!(!apply(&mut app, Action::DeleteAnchor { pt: corner }));
    let shapes_after: Vec<u64> = scene_summary(&app)
        .shapes
        .iter()
        .map(|s| s.shape_id)
        .collect();
    assert_eq!(shapes_before, shapes_after, "何も変わらない");
    assert_eq!(app.history.undo_stack.len(), undo_before, "Undo も積まない");
}

/// 変換が使えるかの判定は `invertible` 1つ：読み込みで受け入れる変換（行列式の大きさが 1e-12 より大きい）は、中心点の
/// 写しやインスタンスの置き直しでも使える。有限でない数を含む変換は使えない（洗い出しの 7）
#[test]
fn one_rule_decides_whether_a_transform_is_usable() {
    use crate::vector_engine::invertible;
    assert!(
        invertible(Affine::scale(1e-5)),
        "行列式 1e-10 は使える（前は中心点の写しだけ 1e-9 で断っていた）"
    );
    assert!(!invertible(Affine::scale(1e-7)));
    assert!(!invertible(Affine::new([
        f64::INFINITY,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0
    ])));
    assert!(!invertible(Affine::new([
        1.0,
        0.0,
        0.0,
        1.0,
        f64::NAN,
        0.0
    ])));
    let mut loaded = vector_engine::make_marquee_rect_shape(kurbo::Rect::new(0.0, 0.0, 10.0, 10.0));
    loaded.symbol_instance = Some(symbol_binding::SymbolInstanceBinding::new(
        1,
        1,
        Affine::scale(1e-5),
    ));
    let before = loaded.clone();
    loaded.transform(Affine::new([1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0]));
    assert_eq!(
        loaded.symbol_instance, before.symbol_instance,
        "使えない変換はかけない"
    );
}

/// N-1001：浮いた片の確定は、片の面が元の面とまったく同じ形でも書かれる（まるごと選んで色だけ変えた片、元と同じ所に
/// 貼った片）。元の図形と番号・色まで同じ面だけが「変わらない面」
#[test]
fn committing_recolored_or_coinciding_pieces_writes_them() {
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let green = Color32::from_rgb(40, 180, 90);
    let at = |app: &Is2dApp, p: Point| -> Vec<Color32> {
        app.scene()
            .iter()
            .filter(|s| s.contains_point(p))
            .map(|s| s.color())
            .collect()
    };

    // 1. まるごと選んで、バケツで色だけ変える
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(300.0, 200.0),
            25.0,
            red,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 150.0),
        Point::new(350.0, 250.0),
        false,
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Bucket },
    );
    app.tool.brush_color = green;
    assert!(doc_actions::bucket_fill_at(
        &mut app,
        Point::new(200.0, 200.0)
    ));
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(at(&app, Point::new(200.0, 200.0)), vec![green]);

    // 2. 全部をコピーし、青い線を消してから、同じ所に貼る（赤い線は元とまったく同じ形で重なる）
    let mut app = Is2dApp::default();
    let mut shapes = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        25.0,
        red,
    );
    shapes.extend(make_stroke(
        Point::new(100.0, 300.0),
        Point::new(300.0, 300.0),
        25.0,
        blue,
    ));
    put_cel_shapes(&mut app, 0, 0, shapes);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 150.0),
        Point::new(350.0, 350.0),
        false,
    );
    assert!(doc_actions::copy_selected_shapes(&mut app));
    assert!(canvas_tool::clear_selection(&mut app));
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 260.0),
        Point::new(350.0, 350.0),
        false,
    );
    assert!(doc_actions::delete_selected_shapes(&mut app));
    assert!(at(&app, Point::new(200.0, 300.0)).is_empty());
    assert!(doc_actions::paste_clipboard(&mut app, true));
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(at(&app, Point::new(200.0, 300.0)), vec![blue]);
    assert_eq!(at(&app, Point::new(200.0, 200.0)), vec![red]);
}

/// N-2002：浮いた片の確定は全部か無しか。書けないレイヤーがあれば何も書かず、浮いた片と選択を残して知らせる（片は
/// いつも文書か浮いた片のどちらかにある）。置けない片があるあいだは保存もしない
#[test]
fn refused_commit_keeps_the_floating_pieces() {
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(300.0, 200.0),
            25.0,
            red,
        ),
    );
    doc_actions::apply(&mut app, doc_actions::Action::AddLayer);
    put_cel_shapes(
        &mut app,
        1,
        0,
        make_stroke(
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            25.0,
            blue,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 150.0),
        Point::new(350.0, 350.0),
        false,
    );
    doc_actions::finish_move_selection_drag(&mut app, KVec2::new(0.0, 100.0), false);
    assert!(app.session.floating().is_some());
    // 置く先のレイヤーの1つを編集できなくする（書けない場面の代わり）
    let locked_id = app.tab().active_timeline().layers[1].id;
    app.tab_mut().with_active_timeline_mut(|tl, _| {
        tl.layers[1].locked = true;
        true
    });
    let doc_before = app.tab().doc.root.clone();
    assert!(!canvas_tool::clear_selection(&mut app));
    assert_eq!(
        app.session.refusal.take(),
        Some(canvas_tool::Refusal::Unplaceable)
    );
    assert!(app.session.floating().is_some());
    assert!(!app.session.selection().is_empty());
    for (a, b) in doc_before.layers.iter().zip(&app.tab().doc.root.layers) {
        assert!(std::sync::Arc::ptr_eq(&a.keyframes[0], &b.keyframes[0]));
    }
    assert!(saved_from_app(app.tab()).is_err());

    app.tab_mut().with_active_timeline_mut(|tl, _| {
        tl.layers
            .iter_mut()
            .find(|l| l.id == locked_id)
            .unwrap()
            .locked = false;
        true
    });
    assert!(canvas_tool::clear_selection(&mut app));
    let at = |p: Point| {
        app.scene()
            .iter()
            .filter(|s| s.contains_point(p))
            .map(|s| s.color())
            .collect::<Vec<_>>()
    };
    assert_eq!(at(Point::new(200.0, 300.0)), vec![red]);
    assert_eq!(at(Point::new(200.0, 400.0)), vec![blue]);
}

/// 動かした片 A を絵 B に重ね、B を選び足してから A だけを外しても、見えている重なり（A が上）のまま確定する。直す前は、
/// 外した瞬間に B が A の上に出て、確定すると重なった所の A が消えた（レビュー f01d518 の N-3002）
#[test]
fn deselecting_a_moved_piece_keeps_it_above_a_piece_selected_later() {
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let mut app = Is2dApp::default();
    let mut shapes = make_stroke(
        Point::new(100.0, 150.0),
        Point::new(200.0, 150.0),
        40.0,
        red,
    );
    shapes.extend(make_stroke(
        Point::new(260.0, 150.0),
        Point::new(360.0, 150.0),
        40.0,
        blue,
    ));
    put_cel_shapes(&mut app, 0, 0, shapes);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    let top = |app: &Is2dApp| {
        let scene = app.scene();
        let i = scene
            .topmost_shape_at(Point::new(290.0, 150.0), &[])
            .unwrap();
        scene.get(i).unwrap().color()
    };
    // 赤を選んで右へ動かし（青に重なる）、青を選び足す
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(60.0, 100.0),
        Point::new(225.0, 200.0),
        false,
    );
    doc_actions::finish_move_selection_drag(&mut app, KVec2::new(110.0, 0.0), false);
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(230.0, 100.0),
        Point::new(400.0, 200.0),
        true,
    );
    assert_eq!(top(&app), red, "動かした赤が上");
    // 赤だけを外す（青を残す）
    let keep: Vec<usize> = {
        let scene = app.scene();
        app.session
            .selected_indices()
            .into_iter()
            .filter(|&i| scene.get(i).is_some_and(|s| s.color() == blue))
            .collect()
    };
    assert!(!keep.is_empty());
    assert!(canvas_tool::commit_floating_except(app.tab_mut(), &keep));
    assert_eq!(top(&app), red, "外しても赤が上のまま");
    app.session.floating().unwrap();
    assert!(!app.session.floating_view().pieces().is_empty());
    assert!(
        app.session.floating_view().pieces().iter().all(|p| p.color() == blue),
        "青は浮いたまま"
    );
    assert!(
        app.session.selected_indices().len() >= app.session.floating_view().pieces().len(),
        "浮いた片はみな選ばれている"
    );
    // 残した青を下へ動かしても、赤はそのまま（青は確定していないので、その下の中身を削っていない）
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::NudgeSelection {
            delta: KVec2::new(0.0, 100.0),
        },
    ));
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(top(&app), red, "確定しても重なった所は赤");
    let colors_at = |p: Point| -> Vec<Color32> {
        app.scene()
            .iter()
            .filter(|s| s.contains_point(p))
            .map(|s| s.color())
            .collect()
    };
    assert_eq!(
        colors_at(Point::new(350.0, 250.0)),
        vec![blue],
        "動かした青"
    );
    assert!(
        colors_at(Point::new(350.0, 150.0)).is_empty(),
        "青の元の所は空く"
    );
}

/// A-1：グループの中で、中身をみな選んでグループ化・シンボル化しても、そのグループの重なりの位置は変わらない
#[test]
fn grouping_everything_inside_a_group_keeps_its_place() {
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    for symbolize in [false, true] {
        let mut app = Is2dApp::default();
        let mut shapes = Vec::new();
        for (i, &c) in colors.iter().enumerate() {
            let y = 200.0 + 100.0 * i as f64;
            shapes.extend(make_stroke(
                Point::new(100.0, y),
                Point::new(300.0, y),
                25.0,
                c,
            ));
        }
        put_cel_shapes(&mut app, 0, 0, shapes);
        doc_actions::apply(
            &mut app,
            doc_actions::Action::SetTool { tool: Tool::Select },
        );
        // 下から G1・G_outer・G2 の3つのグループにする
        for i in 0..3 {
            let y = 200.0 + 100.0 * i as f64;
            select_ops::apply_marquee_selection(
                &mut app,
                Point::new(50.0, y - 40.0),
                Point::new(350.0, y + 40.0),
                false,
            );
            doc_actions::group_selected_shapes(&mut app).unwrap();
            assert!(canvas_tool::clear_selection(&mut app));
        }
        let order =
            |app: &Is2dApp| -> Vec<Color32> { app.scene().iter().map(|s| s.color()).collect() };
        assert_eq!(order(&app), colors.to_vec());
        // 真ん中のグループに入り、中身をみな選んでグループ化（シンボル化）して出る
        assert!(select_ops::enter_group_at(
            app.tab_mut(),
            Point::new(200.0, 300.0)
        ));
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(50.0, 250.0),
            Point::new(350.0, 350.0),
            false,
        );
        if symbolize {
            doc_actions::convert_selection_to_symbol(&mut app).unwrap();
        } else {
            doc_actions::group_selected_shapes(&mut app).unwrap();
        }
        assert!(canvas_tool::clear_selection(&mut app));
        assert!(select_ops::exit_current_group(app.tab_mut()));
        assert_eq!(order(&app), colors.to_vec(), "シンボル化 {symbolize}");
    }
}

/// S-4：潰れるほど縮める変換（行列式の大きさが限りより小さい・あふれる）は、地図を置けるかの判定（`fits_grid`）で断る。
/// 浮いた片を縮め続けても、元の大きさに戻って確定されない
#[test]
fn collapsing_transforms_are_refused() {
    let red = Color32::from_rgb(235, 75, 75);
    let tiny = kurbo::Affine::scale(1e-7);
    assert!(!crate::vector_engine::invertible(tiny));
    assert!(!crate::vector_engine::invertible(kurbo::Affine::scale(
        1e200
    )));
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(300.0, 200.0),
            25.0,
            red,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 150.0),
        Point::new(350.0, 250.0),
        false,
    );
    let area = |app: &Is2dApp| -> f64 { app.scene().iter().map(|s| s.bounding_box().area()).sum() };
    let before = area(&app);
    // 0.01 倍を4回：合わせて 1e-8 倍（行列式 1e-16）は潰れるので、どこかで断る
    let mut done = 0;
    for _ in 0..4 {
        if scale_selection(&mut app, 0.01, 0.01) {
            done += 1;
        }
    }
    assert!(done < 4, "潰れる変換は断る");
    assert!(area(&app) < before * 1e-3, "縮めた片が元の大きさに戻らない");
    assert!(canvas_tool::clear_selection(&mut app));
    assert!(area(&app) < before * 1e-3);
}

/// N-2001：切っただけで動かしていない片がある所で、グループを解除して選択を外しても、生の図形に切り口が残らない
#[test]
fn ungrouping_beside_an_unmoved_slice_leaves_no_cut() {
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let mut app = Is2dApp::default();
    let mut shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(500.0, 100.0),
        25.0,
        red,
    );
    shapes.extend(make_stroke(
        Point::new(400.0, 200.0),
        Point::new(500.0, 200.0),
        15.0,
        blue,
    ));
    put_cel_shapes(&mut app, 0, 0, shapes);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    // 青い線をグループにする
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(380.0, 170.0),
        Point::new(520.0, 230.0),
        false,
    );
    doc_actions::group_selected_shapes(&mut app).unwrap();
    assert!(canvas_tool::clear_selection(&mut app));
    let segs = |app: &Is2dApp| -> usize {
        app.scene()
            .iter()
            .filter(|s| s.color() == red)
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum()
    };
    let before = segs(&app);
    // 赤い線の右半分とグループを矩形で選び（動かさない）、解除して選択を外す
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(250.0, 60.0),
        Point::new(600.0, 260.0),
        false,
    );
    assert!(doc_actions::ungroup_selected_shapes(&mut app));
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(segs(&app), before);
}

/// S-6：動かしていない片の一部を選択から外すとき、外した片を残りに戻せなければ、浮いた片と選択を前のままにして断る
/// （浮いた片はいつもみな選ばれている）
#[test]
fn failing_to_return_unselected_pieces_keeps_the_selection() {
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    let mut shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(300.0, 100.0),
        10.0,
        red,
    );
    shapes.extend(make_stroke(
        Point::new(100.0, 300.0),
        Point::new(300.0, 300.0),
        10.0,
        red,
    ));
    put_cel_shapes(&mut app, 0, 0, shapes);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(200.0, 50.0),
        Point::new(400.0, 350.0),
        false,
    );
    let picked = app.session.selected_indices();
    assert_eq!(picked.len(), 2);
    let floating_before = app.session.floating_view().pieces().len();
    // 戻す先のセル画が引けない（ロックしたレイヤーは画面の土台に地図を持たない）
    app.doc.root.layers[0].locked = true;
    app.refresh_scene_base();
    assert!(!canvas_tool::commit_floating_except(&mut app, &picked[..1]));
    app.session.floating().unwrap();
    assert_eq!(app.session.floating_view().pieces().len(), floating_before);
    assert!(app.session.floating_view()
        .pieces()
        .iter()
        .all(|p| app.session.selection().contains_shape(p)));
}

/// A-7：選択ツールの「アンカーの追加・削除」の副モードで貼り付けると、貼った図形を動かせる形の変形に戻る
#[test]
fn pasting_returns_the_select_tool_to_reshape() {
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            10.0,
            red,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(250.0, 150.0),
        false,
    );
    assert!(doc_actions::copy_selected_shapes(&mut app));
    doc_actions::apply(
        &mut app,
        doc_actions::Action::ToggleSelectSubMode {
            mode: SelectSubMode::AddAnchor,
        },
    );
    assert_eq!(app.tool.select_sub_mode, SelectSubMode::AddAnchor);
    assert!(doc_actions::paste_clipboard(&mut app, false));
    assert!(!app.session.selection().is_empty());
    assert_eq!(app.tool.select_sub_mode, SelectSubMode::Reshape);
}

/// A-7：`Alt` で始めた拡大も、途中で `Alt` を離せば反対側のハンドルを基準にする
#[test]
fn releasing_alt_while_scaling_returns_to_the_opposite_handle() {
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            10.0,
            red,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool {
            tool: Tool::Transform,
        },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(250.0, 150.0),
        false,
    );
    let (bbox, _) = app
        .session
        .selection_bbox_and_pivot(app.view.zoom())
        .unwrap();
    let right = Point::new(bbox.x1, bbox.center().y);
    tool_transform::start_transform_drag(&mut app, right, false, true);
    tool_transform::update_transform_drag(&mut app, right + KVec2::new(50.0, 0.0), false, false);
    tool_transform::finish_transform_drag(&mut app, Some(right + KVec2::new(50.0, 0.0)));
    let (after, _) = app
        .session
        .selection_bbox_and_pivot(app.view.zoom())
        .unwrap();
    assert!(
        (after.x0 - bbox.x0).abs() < 0.5,
        "{} → {}",
        bbox.x0,
        after.x0
    );
    assert!((after.x1 - (bbox.x1 + 50.0)).abs() < 0.5);
}

/// 貼り付けを断ったら、貼るたびのずらしも進まない（入口が、断った操作のクリップボードを操作の前に戻す。直す前は、置けるかの
/// 判定より先にずらしを進めたので、断っても次の貼り付けが1段余計にずれた：約束の棚卸し 5616fcc の A の P-16）
#[test]
fn a_refused_paste_does_not_advance_the_paste_offset() {
    // 格子の端（±65,536px）ぎりぎりの形を写す。ずらして貼ると格子の外へ出るので断る
    let mut app = Is2dApp::default();
    let e = 65_536.0 - 8.0;
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(e - 100.0, 0.0, e, 30.0, Color32::RED)]);
    let red = app.scene().iter().position(|s| s.color() == Color32::RED).unwrap();
    app.set_selected_indices(vec![red]);
    assert!(apply(&mut app, Action::Copy));
    let before = (app.clipboard.paste_count, app.clipboard.paste_offset);
    assert!(!apply(&mut app, Action::Paste { in_place: false }));
    assert_eq!((app.clipboard.paste_count, app.clipboard.paste_offset), before, "断った貼り付けでずらしが進んだ");
}

/// 選択を足すと、自由変形の中心点は新しい選択の真ん中に戻る（Shift クリックでも Shift ダブルクリックでも同じ：持ち主が
/// 決めた 2026-11-27。直す前は Shift ダブルクリックで選び足すと、前の選択に合わせて置いた中心点が残った：約束の棚卸し
/// 5616fcc の B の G1）
#[test]
fn adding_to_the_selection_resets_the_transform_pivot() {
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![
        map_overlay::rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED),
        map_overlay::rect_shape(300.0, 100.0, 400.0, 200.0, Color32::BLUE),
    ]);
    assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(150.0, 150.0) }));
    app.session.transform_pivot = Some(Point::new(120.0, 120.0));
    assert!(apply(&mut app, Action::SelectConnectedAt { pt: Point::new(350.0, 150.0), add: true }));
    assert_eq!(app.session.transform_pivot, None, "選び足しても前の中心点が残った");
}

/// 矢印で動かす → 選び足す → また矢印で動かすと、取り消しは2手（選択が変わったら続けた編集のまとめを区切る。直す前は
/// Shift ダブルクリックで選び足す道が区切らず、2回の移動が1手にまとまった：約束の棚卸し 5616fcc の A の P-15）
#[test]
fn changing_the_selection_ends_a_run_of_nudges() {
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![
        map_overlay::rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED),
        map_overlay::rect_shape(300.0, 100.0, 400.0, 200.0, Color32::BLUE),
    ]);
    assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(150.0, 150.0) }));
    let before = app.history.undo_stack.len();
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(1.0, 0.0) }));
    assert!(apply(&mut app, Action::SelectConnectedAt { pt: Point::new(350.0, 150.0), add: true }));
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(1.0, 0.0) }));
    assert_eq!(app.history.undo_stack.len(), before + 2, "選び足しのあとの移動が、前の移動と1手にまとまった");
}

/// 絵の一部を選んで動かし、選んだまま（持ち上げた絵を持ったまま）インクボトルで線を付けても、そのあと選択を外せ、保存
/// できる（入口が先に貼り戻す：操作ごとの表 `Action::policy`。直す前は、インクボトルが貼り戻さずにセル画を書き、持ち上げた
/// 絵の出どころと食い違って、選択を外す・コマを移る・保存がみな断られた：設計審査 5616fcc の D-1）
#[test]
fn ink_bottle_while_holding_moved_pieces_keeps_the_work_savable() {
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(200.0, 350.0), additive: false }));
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(20.0, 0.0) }));
    assert!(app.session.floating().is_some_and(|f| f.modified()));
    // 残りの右の縁に線を付ける
    let ink = EdgeStroke { color: Color32::BLACK, width: 4 };
    apply(&mut app, Action::InkBottle { pt: Point::new(300.0, 200.0), stroke: ink });
    assert!(saved_from_app(app.tab()).is_ok(), "インクボトルのあと保存できない");
    assert!(apply(&mut app, Action::Deselect), "インクボトルのあと選択を外せない");
}

/// `Alt` で、中心点をハンドルにほとんど重ねて拡大しても、倍率が飛ばない：中心とハンドルが画面で重なっている向きは
/// 拡大しない（`View::moves_on_screen`。直す前は文書の座標で 0.001 より離れていれば割り、0.002 離れていると 5px 動かす
/// だけで 2500 倍になった：レビュー de92ceb の N-3003）
#[test]
fn alt_scaling_with_the_pivot_on_the_handle_does_not_jump() {
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, make_stroke(Point::new(100.0, 100.0), Point::new(200.0, 100.0), 10.0, red));
    doc_actions::apply(&mut app, doc_actions::Action::SetTool { tool: Tool::Transform });
    select_ops::apply_marquee_selection(&mut app, Point::new(50.0, 50.0), Point::new(250.0, 150.0), false);
    let (bbox, _) = app.session.selection_bbox_and_pivot(app.view.zoom()).unwrap();
    let right = Point::new(bbox.x1, bbox.center().y);
    app.session.transform_pivot = Some(right - KVec2::new(0.002, 0.0));
    tool_transform::start_transform_drag(&mut app, right, false, true);
    tool_transform::update_transform_drag(&mut app, right + KVec2::new(5.0, 0.0), false, true);
    let canvas_tool::ActiveCanvasDrag::Transform(canvas_tool::TransformDragState::Scale { last_affine, .. }) = &app.session.drag else {
        panic!("拡大のドラッグになっていない");
    };
    let sx = last_affine.as_coeffs()[0];
    assert!((sx - 1.0).abs() < 1e-9, "横の倍率が {sx} に飛んだ");
}

/// A-3：矩形で切っただけで動かしていない片の上でも、アンカーの追加のカーソル合わせが出る（クリックすると、片を戻して
/// から同じ辺に足す）
#[test]
fn anchor_hover_works_on_unmoved_slices() {
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(300.0, 100.0),
            10.0,
            red,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    let before: usize = app
        .scene()
        .iter()
        .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
        .sum();
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(200.0, 50.0),
        Point::new(400.0, 150.0),
        false,
    );
    assert!(app.session.floating().is_some_and(|fl| !fl.modified()));
    app.tool.select_sub_mode = SelectSubMode::AddAnchor;
    // 切り出した片の中の辺（線の下の縁）
    let pt = Point::new(260.0, 110.0);
    let hover = select_ops::compute_hover_target(
        app.scene(),
        &app.session,
        pt,
        app.view.zoom(),
        SelectSubMode::AddAnchor,
    );
    assert!(
        matches!(
            hover,
            HoverTarget::Edge {
                map_edge: Some(_),
                ..
            }
        ),
        "{hover:?}"
    );
    let segs = |app: &Is2dApp| -> usize {
        app.scene()
            .iter()
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum()
    };
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::AddAnchor { pt },
    ));
    assert!(app.session.floating().is_none());
    assert_eq!(segs(&app), before + 1);
}

/// R1（レビュー f01d518）：重ね合わせで、端が動かない前の地図の辺の判定の形は、計算に使う写しが持つもの（`LayerBase` の
/// `polys`）を使い、作り直さない（入れる辺を決めるときの入れない辺の形、入れなかった辺の確かめ、丸ごと残った辺を曲線に
/// 戻すとき）。同じ地図を同じ位置に重ねると、作るのはほとんど無い（前は、丸ごと残った辺ごとに作り直していた）
#[test]
fn pasting_reuses_the_polylines_of_unmoved_edges() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut rng = map_overlay::Rng(3);
    let mut m = PlanarMap::default();
    for k in 0..20 {
        let c = colors[k % 3];
        let r = 2.0 + rng.f() * 6.0;
        let s = wiggly_stroke(&mut rng, (0.0, 0.0, 300.0, 200.0), r, c);
        m = m
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
            .unwrap();
    }
    let base = LayerBase::new(&m.checked().unwrap().uniquely_labeled().into_map());
    let piece = base.map().clone();
    let edges = piece.edges.len();
    let before = crate::planar_map::polys_built();
    // 片を置く（1本の道：`edit_local`）
    let pasted = match base.edit_local(&base.map_change(&piece, crate::planar_map::MapRule::Overlay), &[]) {
        LocalEdit::Changed(w) => w.stored.to_dense(),
        LocalEdit::Same => base.map().clone(),
        LocalEdit::Refused(r) => panic!("重ねられない {r:?}"),
    };
    assert!(pasted.is_valid());
    // 前は辺の数の3倍（辺 557 本で 1,671）作っていた。今は約2倍（1,126。重ねる地図から来た鎖を曲線に戻すときと、2本の
    // まとめの索引の分）
    let built = crate::planar_map::polys_built() - before;
    assert!(built < 3 * edges, "作った判定の形 {built}（辺 {edges} 本）");
}

/// N-1001（レビュー f01d518）：同じ線は切らない。前からある辺と形がビットまで同じ重ねる地図の辺（同じ線の組）は、
/// どちらの地図のどの辺とも交わらない（2つの地図はそれぞれ正しい地図）ので、i_overlay に入れずに1本で残し、両側の色だけを
/// 決まりで決める（`PlanarMap::same_edges`）。同じ地図を同じ位置に重ねると、判定の形を1本も作らない（前は、重ねる地図の
/// 辺をみな折れ線にして切っていた）。違う色の同じ形を重ねると、色は重ねる地図の色になり、両側が同じ色になった辺は消える
#[test]
fn pasting_onto_the_same_lines_cuts_nothing() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut rng = map_overlay::Rng(3);
    let mut m = PlanarMap::default();
    for k in 0..20 {
        let c = colors[k % 3];
        let r = 2.0 + rng.f() * 6.0;
        let s = wiggly_stroke(&mut rng, (0.0, 0.0, 300.0, 200.0), r, c);
        m = m
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
            .unwrap();
    }
    let m = m.checked().unwrap().uniquely_labeled().into_map();
    let base = LayerBase::new(&m);
    // 片を置く（1本の道：`edit_local`）
    let paste = |p: &PlanarMap| match base.edit_local(&base.map_change(p, crate::planar_map::MapRule::Overlay), &[]) {
        LocalEdit::Changed(w) => w.stored.to_dense(),
        LocalEdit::Same => base.map().clone(),
        LocalEdit::Refused(r) => panic!("重ねられない {r:?}"),
    };
    // 計算に使う写しの判定の形を先に作っておく（升目の索引を作ると、どの辺の判定の形もできる）
    let _ = base.index();
    let before = crate::planar_map::polys_built();
    let pasted = paste(base.map());
    assert_eq!(
        crate::planar_map::polys_built() - before,
        0,
        "同じ地図を同じ位置に重ねて作った判定の形"
    );
    assert!(pasted.is_valid());
    assert_eq!(pasted.edges.len(), m.edges.len());
    // 同じ形を青1色にして重ねる：色のある所はみな青になり、青どうしの境目は消える
    let blue = Color32::from_rgb(20, 40, 200);
    let mut recolored = m.clone();
    for e in &mut recolored.edges {
        e.left = e.left.map(|_| blue);
        e.right = e.right.map(|_| blue);
    }
    let painted = paste(&recolored);
    assert!(painted.is_valid());
    assert!(painted.edges.iter().all(|e| e.left != e.right
        && [e.left, e.right]
            .iter()
            .all(|c| c.is_none_or(|c| c == blue))));
    for p in [
        Point::new(50.0, 50.0),
        Point::new(150.0, 100.0),
        Point::new(250.0, 150.0),
    ] {
        assert_eq!(
            painted.color_at(p),
            m.color_at(p).map(|_| blue),
            "{p:?} の色"
        );
    }
}

/// レビュー d91cd4c の N-2003：縦横で違う拡大・斜めの変形をした絵（速さの偏った曲線）を、矩形で切ってその場に戻すのを
/// くり返しても、切れ目の頂点が残らない（辺の数が変わらない）。前は、切った所の t を折れ線の区間の中で比例で決めたので、
/// 速さの偏った曲線では切った2本が1本にまとまらず、くり返すたびに辺が増えていた
#[test]
fn cut_and_return_of_transformed_art_leaves_no_cut_vertices() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let xforms = [
        ("縦 1/4", Affine::new([1.0, 0.0, 0.0, 0.25, 0.0, 150.0])),
        ("斜め", Affine::new([1.0, 0.0, 0.8, 1.0, -120.0, 0.0])),
    ];
    for seed in 1..=8u64 {
        let mut rng = map_overlay::Rng(seed);
        let mut m = PlanarMap::default();
        for k in 0..30 {
            let c = colors[k % colors.len()];
            let r = 2.0 + rng.f() * 8.0;
            let s = wiggly_stroke(&mut rng, (100.0, 100.0, 300.0, 200.0), r, c);
            m = m.overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c))).unwrap();
        }
        for (name, xform) in xforms {
            let mut m = crate::planar_map::realized(&m, xform).expect("置ける").into_map();
            let start = m.edges.len();
            let bb = m.bounding_box().expect("絵がある");
            let mut rng = map_overlay::Rng(seed * 7 + 3);
            for _ in 0..40 {
                let (x, y) = (bb.x0 + rng.f() * bb.width(), bb.y0 + rng.f() * bb.height());
                let rect = vec![map_overlay::rect_shape(x, y, x + 20.0 + rng.f() * 80.0, y + 10.0 + rng.f() * 40.0, colors[0])];
                let inside = m.overlay(MapInput::Shapes(&rect), MapRule::KeepInside).unwrap();
                let rest = m.overlay(MapInput::Shapes(&rect), MapRule::Paint(None)).unwrap();
                m = rest.overlay(MapInput::Map(&inside), MapRule::Overlay).unwrap();
                assert!(m.is_valid());
            }
            assert_eq!(m.edges.len(), start, "種 {seed}・{name}：切って戻すたびに辺が増えた");
        }
    }
}

/// N-1001（地図のレビュー 4cb53aa）：線の重なった地図を、矩形で切ってその場に戻すのを繰り返しても、切れ目の頂点が
/// 残らない（辺の数が変わらない）。切れ目の頂点が折れ線どうしの交点（曲線から `FLAT_TOL` ほど離れる）のままだと、
/// 切った2本が1本にまとまらず、5回に1回ほど残って溜まっていた
#[test]
fn cut_and_return_leaves_no_cut_vertices() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut rng = map_overlay::Rng(1);
    let mut m = PlanarMap::default();
    for k in 0..30 {
        let c = colors[k % colors.len()];
        let r = 2.0 + rng.f() * 8.0;
        let s = wiggly_stroke(&mut rng, (100.0, 100.0, 300.0, 200.0), r, c);
        m = m
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
            .unwrap();
    }
    let start = m.edges.len();
    for _ in 0..40 {
        let (x, y) = (100.0 + rng.f() * 250.0, 100.0 + rng.f() * 150.0);
        let rect = vec![map_overlay::rect_shape(
            x,
            y,
            x + 30.0 + rng.f() * 100.0,
            y + 30.0 + rng.f() * 80.0,
            colors[0],
        )];
        let inside = m
            .overlay(MapInput::Shapes(&rect), MapRule::KeepInside)
            .unwrap();
        let rest = m
            .overlay(MapInput::Shapes(&rect), MapRule::Paint(None))
            .unwrap();
        m = rest
            .overlay(MapInput::Map(&inside), MapRule::Overlay)
            .unwrap();
        assert!(m.is_valid());
        assert_eq!(m.edges.len(), start);
    }
}

/// 同じ絵を少し傾けた矩形で切って戻すのをくり返しても、線の形がずれず、辺がほとんど溜まらない（地図の精度 `FLAT_TOL`
/// の根拠）。ずれ＝くり返したあとの辺の上の点から、元の地図のいちばん近い辺までの距離の最大。今の精度 0.002px では
/// 300 回で 0.0004px 以下・辺 +1 以下。0.02px にすると、10 回で 5.8px ずれて辺が 3 本溜まり、300 回で 3〜7.6px・
/// 辺 +10〜15 になった（レビュー f01d518 の N-1001 の①を採らない理由）。
///
/// 切れ目の頂点は、まれに残る（地図が切った所の媒介変数を、折れ線の区間の中で比例で決めるため。戻した片が曲線から
/// 少し外れて、まとめる許容を超えることがある）。そこで、種 16 通りで、残る辺の合計が、R6 の前のブラシのときの合計
/// （12 本。どの種でも残らないわけではなかった）を超えないことと、どの種でもずれないことを確かめる
#[test]
fn repeated_cut_and_return_keeps_the_curves() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut extra_total = 0usize;
    for seed in 1u64..=16 {
        let mut rng = map_overlay::Rng(seed);
        let mut m = PlanarMap::default();
        for k in 0..30 {
            let c = colors[k % colors.len()];
            let r = 2.0 + rng.f() * 8.0;
            let s = wiggly_stroke(&mut rng, (100.0, 100.0, 300.0, 200.0), r, c);
            m = m
                .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
                .unwrap();
        }
        let orig = m.clone();
        for _ in 0..20 {
            let (x, y) = (100.0 + rng.f() * 250.0, 100.0 + rng.f() * 150.0);
            let mut rect = map_overlay::rect_shape(
                x,
                y,
                x + 30.0 + rng.f() * 100.0,
                y + 30.0 + rng.f() * 80.0,
                colors[0],
            );
            let c = rect.bounding_box().center();
            rect.transform(kurbo::Affine::rotate_about((rng.f() - 0.5) * 0.02, c));
            let rect = vec![rect];
            let inside = m
                .overlay(MapInput::Shapes(&rect), MapRule::KeepInside)
                .unwrap();
            let rest = m
                .overlay(MapInput::Shapes(&rect), MapRule::Paint(None))
                .unwrap();
            m = rest
                .overlay(MapInput::Map(&inside), MapRule::Overlay)
                .unwrap();
            assert!(m.is_valid());
        }
        let mut drift = 0.0f64;
        for e in 0..m.edges.len() {
            let q = m.quad(e);
            for i in 0..=8 {
                let p = kurbo::ParamCurve::eval(&q, i as f64 / 8.0);
                let d = (0..orig.edges.len())
                    .map(|f| crate::vector_engine::nearest_on_quad(&orig.quad(f), p).1)
                    .fold(f64::INFINITY, f64::min);
                drift = drift.max(d);
            }
        }
        assert!(drift < 0.01, "seed {seed}: ずれ {drift}px");
        assert!(
            m.edges.len() >= orig.edges.len(),
            "seed {seed}: 辺が減らない"
        );
        extra_total += m.edges.len() - orig.edges.len();
    }
    assert!(extra_total <= 12, "残った辺の合計 {extra_total}");
}

/// N-2001：大きな面の一部を矩形で選んで動かして確定しても、大きな面は元の番号を引き継ぐ（面の側の印のうち、いちばん
/// 多いもの：重ね合わせで面に伝える番号と同じ決まり）。番号が変わると、面の輪郭全体の辺の印が変わり、しまう地図の
/// 変わらない所の塊まで新しくなって、Undo の履歴が触った面の輪郭の長さで膨らむ
#[test]
fn moving_part_of_a_large_face_keeps_its_number() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        vec![map_overlay::rect_shape(0.0, 0.0, 400.0, 400.0, red)],
    );
    let cel_ids = |app: &Is2dApp| -> Vec<(u64, bool)> {
        app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| (s.shape_id, s.contains_point(Point::new(350.0, 350.0))))
            .collect()
    };
    let [(big, true)] = cel_ids(&app)[..] else {
        panic!("{:?}", cel_ids(&app));
    };
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(100.0, 100.0),
        Point::new(140.0, 140.0),
        false,
    );
    assert!(apply(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(5.0, 5.0)
        }
    ));
    assert!(canvas_tool::clear_selection(&mut app));
    // 動かした片は残りとつながって1つの面になる（L 字の穴が空く）。その面は元の大きな面の番号のまま
    assert_eq!(cel_ids(&app), vec![(big, true)]);
}

/// N-2004：貼り付けた片を動かさずに確定するとき、片の地図を全部確かめ直さない（片の地図は、貼り付けの準備で確かめた
/// 地図をそのまま持つ。確かめるのは片を作った所の1回だけ）
#[test]
fn committing_an_unmoved_pasted_piece_does_not_recheck_it() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 140.0),
            10.0,
            red,
        ),
    );
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));
    assert!(canvas_tool::clear_selection(&mut app));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    let before = crate::planar_map::full_checks();
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(crate::planar_map::full_checks(), before);
}

/// 矩形で一部を選ぶとき、中（片）と外（残り）を1回の重ね合わせで作る：入れる辺を決める・切る・面の色を伝えるのは1回
/// （`overlay_rules`。設計：claude/N-2002の残り・R5・★4・★5の設計_80fe785.md。直す前は中と外で別々に重ね合わせた）
#[test]
fn marquee_cut_overlays_once_for_inside_and_rest() {
    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let mut stroke = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(300.0, 100.0),
        20.0,
        Color32::RED,
    );
    stroke[0].layer_id = lid;
    put_cel_shapes(&mut app, 0, 0, stroke);
    let before = crate::planar_map::fed_runs();
    run(
        &mut app,
        &[Action::SelectMarquee {
            p0: Point::new(80.0, 70.0),
            p1: Point::new(200.0, 130.0),
            additive: false,
        }],
    );
    assert_eq!(crate::planar_map::fed_runs() - before, 1);
    app.session.floating().expect("片が浮く");
    assert_eq!(app.session.floating_view().pieces().len(), 1);
    assert_eq!(app.session.floating_view().remainder().count(), 1);
}

/// 長い辺の端の近くを少し引いても、辺全体が大きく膨らまない（R7：曲線がいつもカーソルの下を通る決まり（8e03e61）では、
/// 900px の辺の端から 5% を 20px 引くと、辺がいちばん高い所で約 90px 膨らんだ。今の決まりでは約 34px）
#[test]
fn pulling_near_the_end_of_a_long_edge_keeps_the_bulge_small() {
    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let mut s = map_overlay::rect_shape(100.0, 200.0, 1000.0, 300.0, Color32::RED);
    s.layer_id = lid;
    put_cel_shapes(&mut app, 0, 0, vec![s]);
    let from = Point::new(145.0, 200.0);
    tool_select::start_reshape_drag(&mut app, from, false, false);
    for k in 1..=20 {
        tool_select::update_reshape_drag(&mut app, from + kurbo::Vec2::new(0.0, -k as f64));
    }
    tool_select::finish_reshape_drag(&mut app, None);
    // 上の辺（両端が y = 200）の、元の直線からいちばん離れる所は、制御点の離れ方の半分
    let shapes = &scene_summary(&app).shapes;
    let top = shapes[0]
        .exterior()
        .segments
        .iter()
        .find(|q| q.p0.y == 200.0 && q.p2.y == 200.0)
        .copied()
        .expect("上の辺");
    let bulge = (top.p1.y - 200.0).abs() / 2.0;
    assert!(bulge > 30.0 && bulge < 40.0, "いちばん高い所 {bulge}px");
}

/// グループの中を編集するとき、グループの外の図形は、上に重なっていても中身より先（下）に薄く描く。薄い色は不透明なので、
/// 重なり順のまま描くと、上のグループが中身を隠していた
#[test]
fn group_edit_draws_shapes_outside_the_group_below_its_contents() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 100, 220);
    let green = Color32::from_rgb(40, 170, 90);
    // 3つの図形をそれぞれグループにして、少しずつずらして重ねる（赤がいちばん下）
    for (k, color) in [red, blue, green].into_iter().enumerate() {
        let y = 300.0 + 20.0 * k as f64;
        let x = 300.0 + 40.0 * k as f64;
        run(
            &mut app,
            &[
                Action::PaintStroke {
                    points: vec![Point::new(x, y), Point::new(x + 200.0, y)],
                    radius: 20.0,
                    color,
                },
                Action::SelectSingleAt {
                    pt: Point::new(x + 190.0, y),
                },
                Action::Group,
                Action::Deselect,
            ],
        );
    }
    let groups = cel_summary(&app.doc, app.doc.root.layers[0].id, 0).group_paths;
    assert_eq!(groups.len(), 3, "3つのグループ");
    // いちばん下（赤）のグループに入る
    run(
        &mut app,
        &[Action::EnterGroupAt {
            pt: Point::new(305.0, 300.0),
        }],
    );
    assert!(!app.session.editing_group_path().is_empty(), "グループの中");
    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(raw_input, |ctx| app.update_ui(ctx));
    // 描いたメッシュの色の並び：中身（赤のまま）と、グループの外（青・緑を薄くした色）
    let dim = [blue, green].map(crate::screen_mesh::dim_backdrop_color);
    let order: Vec<(usize, bool)> = out
        .shapes
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match &c.shape {
            egui::Shape::Mesh(m) => m.vertices.first().and_then(|v| {
                if v.color == red {
                    Some((i, true))
                } else if dim.contains(&v.color) {
                    Some((i, false))
                } else {
                    None
                }
            }),
            _ => None,
        })
        .collect();
    let first_inside = order.iter().find(|o| o.1).map(|o| o.0).expect("中身を描く");
    let last_outside = order
        .iter()
        .rev()
        .find(|o| !o.1)
        .map(|o| o.0)
        .expect("外を描く");
    assert!(
        last_outside < first_inside,
        "グループの外を、中身より先に描く：{order:?}"
    );
}

/// 動かした絵があるときに、動かしていない絵を選び足して、その一部を Shift クリックで選択から外しても、外した絵は消えない
/// （その場でも、続けて消す・動かすをして確定したあとも）。直す前は、一部を確定したあとの残りの作り直しが、確定する側の
/// 動かしていない組を扱えず、外した絵がその場で画面から消え、続けて消すと戻らなかった：設計審査 4f8ef46 の D-1
#[test]
fn deselecting_an_unmoved_part_after_moving_keeps_it() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    for then in ["Delete", "Move"] {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                map_overlay::rect_shape(100.0, 100.0, 150.0, 150.0, Color32::RED),
                map_overlay::rect_shape(300.0, 100.0, 350.0, 150.0, Color32::GREEN),
                map_overlay::rect_shape(500.0, 100.0, 550.0, 150.0, Color32::BLUE),
            ],
        );
        assert!(apply(&mut app, Action::SetTool { tool: Tool::Select }));
        assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(80.0, 80.0), p1: Point::new(170.0, 170.0), additive: false }));
        assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(0.0, 200.0) }));
        assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(280.0, 80.0), p1: Point::new(570.0, 170.0), additive: true }));
        let green = app.scene().iter().position(|s| s.color() == Color32::GREEN).expect("緑");
        assert!(apply(&mut app, Action::ToggleSelectionAt { scene_idx: green }));
        assert!(app.scene().iter().any(|s| s.color() == Color32::GREEN), "外した緑がその場で消えた");
        match then {
            "Delete" => assert!(apply(&mut app, Action::DeleteSelection)),
            _ => assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(30.0, 0.0) })),
        }
        assert!(apply(&mut app, Action::Deselect));
        let doc = app.tab().doc();
        let colors: Vec<Color32> = doc.root.layers[0].keyframes[0]
            .cel
            .maps()
            .iter()
            .flat_map(|m| m.base().drawn_shapes(&m.group_path).into_iter().map(|f| f.color()).collect::<Vec<_>>())
            .collect();
        assert!(colors.contains(&Color32::GREEN), "{then} のあと確定すると、外した緑が消えた：{colors:?}");
    }
}

/// 浮いた片の確定は、画面の土台が古くても（作り直していなくても）文書のとおりに書く（土台は文書から作る：`Tab::stage`。
/// 設計審査 b7208d9 の 8.。前は画面の土台からセル画とレイヤーの順を読んだので、入口の外から確定を呼ぶ道は、土台を先に
/// 作り直しておく約束を覚えている必要があり、忘れると確定を断った）
#[test]
fn committing_a_lift_does_not_need_a_fresh_screen_base() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let lifted = || {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(20.0, 0.0), alt_duplicate: false }));
        assert!(app.session.floating().is_some_and(|fl| fl.modified()), "動かした絵が浮いていない");
        app
    };
    // 画面の土台を作り直してから確定したときの答え（お手本）
    let mut whole = lifted();
    whole.refresh_scene_base();
    assert!(canvas_tool::commit_floating(&mut whole));
    // 画面の土台を捨てて（作り直さずに）確定する
    let mut app = lifted();
    app.session.drop_scene_base();
    assert!(canvas_tool::commit_floating(&mut app), "画面の土台が古いと確定を断った");
    let painted = |tab: &Tab| {
        tab.doc.root.layers[0].keyframes[0].cel.scope_base(&[]).drawn_shapes(&[]).iter().map(|s| (s.color(), s.geometry_bounding_box().map(|b| (b.x0.round() as i64, b.x1.round() as i64)))).collect::<Vec<_>>()
    };
    assert_eq!(painted(&app), painted(&whole), "画面の土台が古いと別の絵になった");
    assert!(painted(&app).iter().any(|(_, b)| *b == Some((120, 320))), "動かした所に絵が無い");
}

/// 持ち上げた絵の見張りは、地図だけでなく場所（レイヤー・キーフレーム・階層）も比べる：F6 で写したキーフレームは地図を
/// 分け合うが、別の場所なので確定を断る（設計審査 b7208d9 の 3.。前は地図の番地だけを比べたので、確定を通らずにコマが
/// F6 の写しへ替わる道ができると、持ち上げた絵が別のコマに黙って書き込まれた）
#[test]
fn the_lift_watch_tells_an_f6_copy_from_the_original() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: true }));
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 0 }));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(20.0, 0.0), alt_duplicate: false }));
    let fl = app.session.floating().expect("持ち上げた絵").clone();
    let scope = app.session.editing_group_path().to_vec();
    // 元のコマ（0）では確定できる
    let here = crate::floating::Stage::of(app.active_timeline(), Vec::new(), &scope);
    assert!(fl.commit_edits(&here).iter().all(Option::is_some), "元のコマで確定できない");
    // F6 で写したコマ（1）は地図を分け合うが、別の場所なので断る
    let mut tl = app.active_timeline().clone();
    tl.current_frame = 1;
    let copy = crate::floating::Stage::of(&tl, Vec::new(), &scope);
    assert!(fl.commit_edits(&copy).iter().all(Option::is_none), "F6 で写した別のコマに確定した");
}

/// 動かした絵（塗りとその上の線）から、Shift で線だけを選択から外し、あとで塗りを確定しても、線は残る（一度に全部を確定
/// したときと同じ絵。外した線は、持ったままの塗りの「上に重ねる線」：`PieceGroup::lines_over`。設計審査 b7208d9 の 9. の
/// 「線が消える件」。前は一部の確定で線が塗りの下に入り、塗りを確定すると覆われて消えた）
#[test]
fn deselecting_a_line_from_a_moved_fill_keeps_the_line() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let setup = || {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(c.x - 100.0, c.y - 60.0, c.x + 100.0, c.y + 60.0, Color32::RED)]);
        assert!(apply(
            &mut app,
            Action::PencilStroke {
                points: vec![Point::new(c.x - 80.0, c.y), Point::new(c.x, c.y + 10.0), Point::new(c.x + 80.0, c.y)],
                stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 },
            },
        ));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(30.0, 20.0), alt_duplicate: false }));
        app
    };
    // 一度に全部を確定する
    let mut all = setup();
    assert!(apply(&mut all, Action::Deselect));
    let want = cel_picture(&all);
    assert!(want.lines > 0, "線が無い");
    // 線だけを選択から外す
    let mut part = setup();
    let line_idx = part.scene().iter().position(|s| s.line.is_some()).expect("線の片");
    assert!(apply(&mut part, Action::ToggleSelectionAt { scene_idx: line_idx }));
    // 持っている間も、線は塗りの上に見える（線の上の点で、いちばん上の図形は線）
    let on_line = part.session.edit_path().world_xform() * Point::new(c.x + 30.0, c.y + 20.0 + 10.0);
    let scene = part.scene();
    let top = scene.topmost_shape_at(on_line, part.session.editing_group_path()).and_then(|i| scene.get(i)).expect("図形がある");
    assert!(top.line.is_some(), "持っている間、外した線が塗りの下に隠れた");
    // あとで塗りを確定しても、一度に全部を確定したときと同じ絵（線の数・辺と点の数も同じ：余計な点を増やさない）
    assert!(apply(&mut part, Action::Deselect));
    assert_eq!(cel_picture(&part), want, "線だけを外してから塗りを確定すると、一度に確定したときと違う絵になった");
}

/// 確定したセル画（レイヤー 0・キーフレーム 0・ルートの階層）の絵の要約：塗りの色と外枠（1/10px に丸める）・線の数・
/// 辺の数・頂点の数
#[derive(Debug, PartialEq)]
struct CelPicture {
    fills: Vec<(Color32, [i64; 4])>,
    lines: usize,
    edges: usize,
    nodes: usize,
}

fn cel_picture(app: &Is2dApp) -> CelPicture {
    let base = app.doc.root.layers[0].keyframes[0].cel.scope_base(&[]);
    let r = |v: f64| (v * 10.0).round() as i64;
    let mut fills: Vec<(Color32, [i64; 4])> = base
        .drawn_shapes(&[])
        .iter()
        .filter(|f| f.line.is_none())
        .filter_map(|f| f.geometry_bounding_box().map(|b| (f.color(), [r(b.x0), r(b.y0), r(b.x1), r(b.y1)])))
        .collect();
    fills.sort_by_key(|(c, b)| (c.to_array(), *b));
    let lines = base.drawn_shapes(&[]).iter().filter(|f| f.line.is_some()).count();
    CelPicture { fills, lines, edges: base.map().edges.len(), nodes: base.map().nodes.len() }
}

/// 外した線を、持ったままの塗りの上に重ねる線として覚えたあと、その線を持ち上げて動かす・消す・一部を矩形で切り取って
/// 動かしてから確定しても、元の場所に幽霊の線が出ない（覚えるのは線の番号で、形はその時点の中身から取る：
/// `PieceGroup::lines_over`）
#[test]
fn a_remembered_line_leaves_no_ghost_after_it_moves() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let setup = || {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(c.x - 100.0, c.y - 60.0, c.x + 100.0, c.y + 60.0, Color32::RED)]);
        assert!(apply(
            &mut app,
            Action::PencilStroke {
                points: vec![Point::new(c.x - 80.0, c.y), Point::new(c.x, c.y + 10.0), Point::new(c.x + 80.0, c.y)],
                stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 },
            },
        ));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(30.0, 20.0), alt_duplicate: false }));
        let line_idx = app.scene().iter().position(|s| s.line.is_some()).expect("線の片");
        assert!(apply(&mut app, Action::ToggleSelectionAt { scene_idx: line_idx }));
        app
    };
    // 外した線（選ばれていない線）を選び直す
    let reselect_line = |app: &mut Is2dApp| {
        let sel = app.session.selection().shapes.clone();
        let line_idx = app.scene().iter().position(|s| s.line.is_some() && !sel.contains(&s.shape_id)).expect("外した線");
        assert!(apply(app, Action::ToggleSelectionAt { scene_idx: line_idx }));
    };
    // 動かす：線を選び直して、塗りと一緒にもう一度動かす。お手本は、線を外さずに2回動かしたもの
    let mut want = Is2dApp::default();
    put_cel_shapes(&mut want, 0, 0, vec![map_overlay::rect_shape(c.x - 100.0, c.y - 60.0, c.x + 100.0, c.y + 60.0, Color32::RED)]);
    assert!(apply(
        &mut want,
        Action::PencilStroke {
            points: vec![Point::new(c.x - 80.0, c.y), Point::new(c.x, c.y + 10.0), Point::new(c.x + 80.0, c.y)],
            stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 },
        },
    ));
    assert!(apply(&mut want, Action::SelectAll));
    assert!(apply(&mut want, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(30.0, 20.0), alt_duplicate: false }));
    assert!(apply(&mut want, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(0.0, 200.0), alt_duplicate: false }));
    assert!(apply(&mut want, Action::Deselect));
    let mut moved = setup();
    reselect_line(&mut moved);
    assert!(apply(&mut moved, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(0.0, 200.0), alt_duplicate: false }));
    assert!(apply(&mut moved, Action::Deselect));
    assert_eq!(cel_picture(&moved), cel_picture(&want), "線を動かしたあと、幽霊の線が出たか、線が消えた");
    // 消す：線を選び直して、塗りと一緒に消すと、何も残らない
    let mut deleted = setup();
    reselect_line(&mut deleted);
    assert!(apply(&mut deleted, Action::DeleteSelection));
    assert!(apply(&mut deleted, Action::Deselect));
    assert_eq!(cel_picture(&deleted).lines, 0, "線を消したあと、幽霊の線が出た");
    // 一部を切り取る：線の左半分を矩形で選び足して動かすと、元の場所（矩形の中）に線は残らない
    let mut cut = setup();
    let world = cut.session.edit_path().world_xform();
    let (p0, p1) = (world * Point::new(c.x - 70.0, c.y - 30.0), world * Point::new(c.x + 20.0, c.y + 70.0));
    assert!(apply(&mut cut, Action::SelectMarquee { p0, p1, additive: true }));
    assert!(apply(&mut cut, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(0.0, 300.0), alt_duplicate: false }));
    assert!(apply(&mut cut, Action::Deselect));
    let base = cut.doc.root.layers[0].keyframes[0].cel.scope_base(&[]);
    let old = kurbo::Rect::new(c.x - 70.0, c.y - 30.0, c.x + 20.0, c.y + 70.0).inset(-0.0);
    let ghosts = base
        .drawn_shapes(&[])
        .iter()
        .filter(|f| f.line.is_some())
        .filter_map(|f| f.geometry_bounding_box())
        .filter(|b| b.center().x > old.x0 + 5.0 && b.center().x < old.x1 - 5.0 && b.center().y > old.y0 && b.center().y < old.y1)
        .count();
    assert_eq!(ghosts, 0, "線の一部を切り取って動かしたあと、元の場所に幽霊の線が出た");
    assert!(cel_picture(&cut).lines > 0, "線が無くなった");
}

/// 一部を確定しても（選んだ片の一部を残して確定する：`RetainSelection`）、見えている重なりを変えない：一部の確定の前と
/// あとで、今みな確定したときの絵（`committed_picture`）が同じ（設計審査 b7208d9 の 9. の「線が消える件」。前は、外した
/// 線が持ったままの塗りの下に入り、あとで塗りを確定すると覆われて消えた）。お手本と同じでたらめな操作の流れで確かめる
#[test]
fn a_partial_commit_keeps_the_committed_picture() {
    let (mut checked, mut wrong) = (0usize, Vec::new());
    floating_random_run(|seed, step, kind, app, action| {
        if kind != "RetainSelection" {
            return crate::canvas_tool::doc_actions::apply(app, action);
        }
        let before = committed_picture(app);
        let ok = crate::canvas_tool::doc_actions::apply(app, action);
        if ok {
            checked += 1;
            let after = committed_picture(app);
            let same = before.len() == after.len() && before.iter().zip(&after).all(|(a, b)| picture_line_near(a, b));
            if !same {
                wrong.push(format!("{seed:#x} {step}\n  前：{before:?}\n  後：{after:?}"));
            }
        }
        ok
    });
    assert!(checked >= 20, "一部の確定を試せていない：{checked}");
    assert!(wrong.is_empty(), "一部の確定で、みな確定したときの絵が変わった（{} / {checked} 回）：\n{}", wrong.len(), wrong.join("\n"));
}

/// 動かした塗りと線を別の塗り（青）の上へ重ね、線を1本ずつ選択から外してから確定しても、一度に確定したときと同じ絵（外した
/// 線は、青のふちで切られて番号が変わる。上に重ねる線は形で覚えるので迷子にならない：`PieceGroup::lines_over`。設計審査
/// 01e4283 の D-1。前は番号で覚えたので、切られた線が持っている間は塗りの下に隠れ、確定で消えた）
#[test]
fn a_remembered_line_cut_by_the_picture_below_keeps_its_place() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let setup = || {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                map_overlay::rect_shape(c.x - 100.0, c.y - 60.0, c.x + 100.0, c.y + 60.0, Color32::RED),
                map_overlay::rect_shape(c.x + 150.0, c.y - 80.0, c.x + 350.0, c.y + 80.0, Color32::BLUE),
            ],
        );
        assert!(apply(
            &mut app,
            Action::PencilStroke {
                points: vec![Point::new(c.x - 80.0, c.y), Point::new(c.x, c.y + 10.0), Point::new(c.x + 80.0, c.y)],
                stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 },
            },
        ));
        assert!(apply(&mut app, Action::SelectAll));
        // 青を選択から外す（赤と線だけを動かす）
        let blue = app.scene().iter().position(|s| s.color() == Color32::BLUE && s.line.is_none()).expect("青");
        assert!(apply(&mut app, Action::ToggleSelectionAt { scene_idx: blue }));
        assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(100.0, 0.0), alt_duplicate: false }));
        app
    };
    let mut all = setup();
    assert!(apply(&mut all, Action::Deselect));
    let want = cel_picture(&all);
    assert!(want.lines > 0, "線が無い");
    // 線の辺を1本ずつ選択から外す
    let mut part = setup();
    loop {
        let sel = part.session.selection().shapes.clone();
        let Some(i) = part.scene().iter().position(|s| s.line.is_some() && sel.contains(&s.shape_id)) else {
            break;
        };
        assert!(apply(&mut part, Action::ToggleSelectionAt { scene_idx: i }));
    }
    // 持っている間、線に沿った列で、いちばん上は線（塗りの下に隠れない）
    let world = part.session.edit_path().world_xform();
    let scope = part.session.editing_group_path().to_vec();
    let scene = part.scene();
    let hidden = (0..=32)
        .map(|k| c.x + 30.0 + k as f64 * 4.0)
        .filter(|&x| {
            // 動かした線の上の点（線はおよそ c.y〜c.y+10 の間）
            let y = c.y + 10.0 * (1.0 - ((x - c.x - 100.0) / 80.0).powi(2)).max(0.0);
            scene
                .topmost_shape_at(world * Point::new(x, y), &scope)
                .and_then(|i| scene.get(i))
                .is_some_and(|s| s.line.is_none())
        })
        .count();
    assert_eq!(hidden, 0, "持っている間、外した線が塗りの下に隠れた");
    assert!(apply(&mut part, Action::Deselect));
    assert_eq!(cel_picture(&part), want, "線を外してから確定すると、一度に確定したときと違う絵になった");
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
    assert_eq!(app.session.selection().shapes.len(), 1);
    assert!(app.session.selection().instances.is_empty());
    let shape_id = *app.session.selection().shapes.iter().next().unwrap();
    assert_eq!(app.session.selected_indices(), vec![0]);

    // 移動してから Undo / Redo しても同じ ShapeId の選択が維持されること
    run(
        &mut app,
        &[Action::NudgeSelection {
            delta: KVec2::new(30.0, 15.0),
        }],
    );
    assert!(app.session.selection().shapes.contains(&shape_id));

    run(&mut app, &[Action::Undo]);
    assert!(
        app.session.selection().shapes.contains(&shape_id),
        "Undo 後も存在する ShapeId の選択が保たれること"
    );
    assert_eq!(app.session.selected_indices(), vec![0]);

    run(&mut app, &[Action::Redo]);
    assert!(
        app.session.selection().shapes.contains(&shape_id),
        "Redo 後も存在する ShapeId の選択が保たれること"
    );
    assert_eq!(app.session.selected_indices(), vec![0]);

    // タイムラインからの再評価（refresh_scene_base）や同じキーフレーム内のホールドフレーム移動でも選択が保たれること
    app.session.drop_scene_base();
    app.refresh_scene_base();
    assert!(app.session.selection().shapes.contains(&shape_id));

    run(
        &mut app,
        &[Action::ExtendFrame, Action::ScrubRulerToFrame { frame: 1 }],
    );
    assert_eq!(app.doc.root.current_frame, 1);
    assert!(
        app.session.selection().shapes.contains(&shape_id),
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
    assert!(app.session.selection().shapes.contains(&shape_id));
    run(
        &mut app,
        &[Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 1,
        }],
    );
    assert!(
        app.session.selection().shapes.contains(&shape_id),
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
        app.session.selection().is_empty(),
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
    assert!(app.session.selection().shapes.is_empty());
    assert_eq!(app.session.selection().instances.len(), 1);
    let inst_gid = *app.session.selection().instances.iter().next().unwrap();
    let eval_shape_id_before = scene_summary(&app).shapes[0].shape_id;

    app.session.drop_scene_base();
    app.refresh_scene_base();
    let eval_shape_id_after = scene_summary(&app).shapes[0].shape_id;
    assert_ne!(
        eval_shape_id_before, eval_shape_id_after,
        "シンボル再展開のたびに展開後 FillShape の shape_id は新しく割り当てられること"
    );
    assert!(
        app.session.selection().instances.contains(&inst_gid),
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
        app.session.selection().instances.contains(&inst_gid),
        "シンボル移動の Undo 後も InstanceGid 選択が保たれること"
    );
    run(&mut app, &[Action::Redo]);
    assert!(
        app.session.selection().instances.contains(&inst_gid),
        "シンボル移動の Redo 後も InstanceGid 選択が保たれること"
    );

    // 3. シンボル移動とシンボル変換自体を Undo すると、消滅した InstanceGid は選択から除去され、
    //    古い選択が残らないこと
    run(&mut app, &[Action::Undo, Action::Undo]);
    assert!(
        !app.session.selection().instances.contains(&inst_gid),
        "Undo で消滅した InstanceGid が古い選択として残らないこと"
    );

    // 新規図形を描画して選択した状態から描画自体を Undo すると、消滅した ShapeId が選択から除去され、描画する前の選択に
    // 戻ること（取り消しは、絵と一緒にその時の選択も戻す）
    let before_stroke = app.session.selection().clone();
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
    let transient_shape_id = *app.session.selection().shapes.iter().next().unwrap();
    run(&mut app, &[Action::Undo]);
    assert!(
        !app.session.selection().shapes.contains(&transient_shape_id),
        "Undo で消滅した ShapeId が古い選択として残らないこと"
    );
    assert_eq!(app.session.selection(), &before_stroke, "描画する前の選択に戻る");
}

#[test]
fn test_stage5_anchor_edit_preserved_after_nudge_deselect_and_frame_scrub() {
    use crate::canvas_tool::doc_actions::Action;

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
    let initial_red_segs = app.scene().get(0).unwrap().exterior().segments.len();
    let initial_blue_segs = app.scene().get(1).unwrap().exterior().segments.len();

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
        app.scene().get(1).unwrap().exterior().segments.len(),
        initial_red_segs + 1,
        "選択中図形への AddAnchor(+2) と DeleteAnchor(-1) が反映されていること"
    );

    run(&mut app, &[Action::Deselect]);
    let red_after_deselect = app
        .scene()
        .iter()
        .find(|s| s.color() == red)
        .unwrap()
        .exterior()
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
    // 浮いている図形のほかのアンカー編集は地図で行うので、先に浮いている図形（動かした赤）を確定する
    assert!(app.session.floating().is_none());
    let blue_before_commit = app
        .scene()
        .iter()
        .find(|s| s.color() == blue)
        .unwrap()
        .exterior()
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

/// いくつかのレイヤーから写した絵を貼り付けると、作業中のレイヤー1つにまとめて入る（元のレイヤーへは分けない）。
/// 重なる所は上のレイヤーの絵が残る
#[test]
fn pasting_drawing_copied_from_several_layers_goes_into_the_working_layer() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(230, 50, 50);
    let green = Color32::from_rgb(40, 180, 60);
    // 下のレイヤーに赤の横線、その上のレイヤーに緑の縦線（交わる）、いちばん上に空のレイヤー
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 10.0,
            color: red,
        },
    ));
    assert!(apply(&mut app, Action::AddLayer));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(150.0, 50.0), Point::new(150.0, 150.0)],
            radius: 10.0,
            color: green,
        },
    ));
    assert!(apply(&mut app, Action::AddLayer));
    // 色からレイヤーを決める（いちばん新しいレイヤーが作業中で、空）
    let layer_of = |app: &Is2dApp, c: Color32| {
        app.scene()
            .iter()
            .find(|s| s.color() == c)
            .map(|s| s.layer_id)
            .expect("その色の絵がある")
    };
    let (red_layer, green_layer) = (layer_of(&app, red), layer_of(&app, green));
    let target = app.doc.root.active_layer_id();
    assert!(target != red_layer && target != green_layer);
    let target_idx = app
        .doc
        .root
        .layers
        .iter()
        .position(|l| l.id == target)
        .unwrap();
    // 重なりの上のレイヤーの色（ステージのレイヤーの順は下から）
    let order = app.session.base_cache().layer_order_ids().to_vec();
    let pos = |lid: u64| order.iter().position(|&l| l == lid).unwrap();
    let upper = if pos(green_layer) > pos(red_layer) {
        green
    } else {
        red
    };
    let count = |app: &Is2dApp, lid: u64| app.scene().iter().filter(|s| s.layer_id == lid).count();
    assert_eq!((count(&app, red_layer), count(&app, green_layer)), (1, 1));

    // 赤と緑を写し、空のレイヤーだけを選んで、同じ位置に貼り付けて確定する
    assert!(apply(&mut app, Action::SelectAll));
    assert_eq!(app.session.selected_indices().len(), 2);
    assert!(apply(&mut app, Action::Copy));
    assert!(apply(&mut app, Action::Deselect));
    assert!(apply(
        &mut app,
        Action::SelectTimelineCells {
            l0: target_idx,
            f0: 0,
            l1: target_idx,
            f1: 0
        }
    ));
    assert_eq!(app.doc.root.active_layer_id(), target);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert!(crate::canvas_tool::clear_selection(&mut app));

    // 写した元のレイヤーは変わらず、貼り付けた絵は全部、選んだレイヤーに入る
    assert_eq!((count(&app, red_layer), count(&app, green_layer)), (1, 1));
    let color_in_target = |p: Point| {
        app.scene()
            .iter()
            .find(|s| s.layer_id == target && s.contains_point(p))
            .map(|s| s.color())
    };
    assert_eq!(color_in_target(Point::new(110.0, 100.0)), Some(red));
    assert_eq!(color_in_target(Point::new(150.0, 60.0)), Some(green));
    // 交わる所は、上のレイヤーの絵
    assert_eq!(color_in_target(Point::new(150.0, 100.0)), Some(upper));
}

/// 復旧の書き手は、1回の操作で、触ったタブの変わった所だけを書く：ほかのタブを触っても、変わっていない未保存のタブの
/// 復旧ファイルには何も書き足さない（浮いた片を置いたままのタブも。前は、どれか1つのタブが変わると未保存のタブをみな
/// 書き足し、浮いた片のあるタブは置いた文書を作り直して塊ごと書いた：全体の点検 2026-11-27）
/// 選択は「どのキーフレームの絵を選んだか」を持ち、見えるキーフレームが変わったら、どの道でも外れる（設計審査 b7208d9 の
/// 2.。前は選択を番号だけで持ち、F6 の写しは元と同じ番号なので、F6 のあと絵を選んで `Shift+F6` を押すと、選択がコマ 1 の
/// 絵に残り、`Delete` でコマ 1 の絵が消えた。ルーラーで移ると外れた）。同じキーフレームの中でコマを送っても残る
#[test]
fn the_selection_leaves_when_the_shown_keyframe_changes_on_every_path() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let drawn = |app: &Is2dApp| -> usize { app.doc.root.layers[0].keyframes[0].cel.maps().len() };
    let setup = || {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
        assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: true }));
        assert_eq!(app.active_timeline().current_frame, 1);
        assert!(apply(&mut app, Action::SelectAll));
        assert!(!app.session.selection().is_empty());
        app
    };
    // Shift+F6（タイムラインを書き換える道）：コマ 1 の絵が見えるようになり、選択は外れ、Delete してもコマ 1 の絵は消えない
    let mut app = setup();
    let before = drawn(&app);
    assert!(apply(&mut app, Action::DeleteKeyframe));
    assert!(app.session.selection().is_empty(), "タイムラインを書き換えたあと、選択が別のキーフレームの絵に残った");
    apply(&mut app, Action::DeleteSelection);
    assert_eq!(drawn(&app), before, "選んでいないコマ 1 の絵が消えた");
    // ルーラー（コマを移る道）：外れる
    let mut app = setup();
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 0 }));
    assert!(app.session.selection().is_empty(), "コマを移ったあと、選択が別のキーフレームの絵に残った");
    // 取り消し（戻す道）：選んだときのキーフレームが見えていれば、取り消しのあとも残る
    let mut app = setup();
    assert!(apply(&mut app, Action::ExtendFrame));
    assert!(apply(&mut app, Action::Undo));
    // 同じキーフレームの中でコマを送る：残る
    let mut app = setup();
    assert!(apply(&mut app, Action::ExtendFrame));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 2 }));
    assert!(!app.session.selection().is_empty(), "同じキーフレームの中のコマ送りで選択が外れた");
}

/// 片のあるレイヤーが複数あるとき、レイヤーごとに同時に置いても、1レイヤーずつ順に置いたときと同じになる
#[test]
fn test_merging_floating_pieces_layer_by_layer_matches_merging_each_layer_in_turn() {
    use crate::serial::next_shape_id;

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
            let raw = brush_line(&pts, radius);
            let stroke_shapes: Vec<FillShape> = brush_line_shapes(&raw, color)
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

    let mut in_turn = base_shapes.clone();
    for lid in 1u64..=3 {
        let layer_pieces = pieces
            .iter()
            .filter(|p| p.layer_id == lid)
            .cloned()
            .collect();
        merge_shapes_into_canvas(&mut in_turn, layer_pieces);
    }

    let mut layer_by_layer = base_shapes;
    merge_shapes_into_canvas(&mut layer_by_layer, pieces);

    assert!(layer_by_layer.len() > orig_count);

    let summarize = |shapes: &[FillShape]| -> Vec<(u64, Color32, i64, i64, i64, usize)> {
        shapes
            .iter()
            .map(|s| {
                let holes_area: f64 = s.holes().iter().map(|h| h.signed_area().abs()).sum();
                let area = s.exterior().signed_area().abs() - holes_area;
                let bbox = s.bounding_box();
                (
                    s.layer_id,
                    s.color(),
                    (area * 100.0).round() as i64,
                    (bbox.x0 * 100.0).round() as i64,
                    (bbox.y0 * 100.0).round() as i64,
                    s.holes().len(),
                )
            })
            .collect()
    };

    assert_eq!(summarize(&layer_by_layer), summarize(&in_turn));
}

/// 地図に壊れた面の輪郭（色がまざる）ができる場面（N-4001）。
/// 赤の右のフチは少し右へふくらみ、緑の左のフチの上の端は 0.06 右にずれている（0.05 ではまとまらない）ので、
/// 下の角で2色の辺の並びが入れ違い、赤の面の輪郭が緑の辺をまたぐ
fn broken_face_contour_app() -> Is2dApp {
    let mut app = Is2dApp::default();
    let lid = app.doc.root.layers[0].id;
    let p = Point::new;
    let line = |a: Point, b: Point| QuadBez::new(a, a.midpoint(b), b);
    let red = vector_engine::Contour::new(vec![
        line(p(100.0, 100.0), p(200.0, 100.0)),
        QuadBez::new(p(200.0, 100.0), p(200.1, 150.0), p(200.0, 200.0)),
        line(p(200.0, 200.0), p(100.0, 200.0)),
        line(p(100.0, 200.0), p(100.0, 100.0)),
    ]);
    let green = vector_engine::Contour::new(vec![
        line(p(200.0, 100.0), p(300.0, 100.0)),
        line(p(300.0, 100.0), p(300.0, 200.0)),
        line(p(300.0, 200.0), p(200.06, 200.0)),
        line(p(200.06, 200.0), p(200.0, 100.0)),
    ]);
    let shapes = vec![
        FillShape::new(Color32::RED, red, Vec::new()).with_scope(lid, &[]),
        FillShape::new(Color32::GREEN, green, Vec::new()).with_scope(lid, &[]),
    ];
    let map = crate::planar_map::PlanarMap::from_faces(shapes.iter());
    assert!(
        map.faces().1 > 0,
        "前提：図形から作った地図に壊れた面の輪郭がある"
    );
    // セル画に入れると、図形を並びの順に塗り重ねて作り直した地図の面になる
    app.doc.root.layers[0].keyframe_mut(0).cel = timeline::Cel::from_shapes_unwrap(shapes);
    app.refresh_scene_base();
    assert!(layer_map_is_valid(&app), "セル画の図形は正しい地図になる");
    app
}

fn green_area(app: &Is2dApp) -> f64 {
    app.scene()
        .iter()
        .filter(|s| s.color() == Color32::GREEN)
        .map(|s| s.contours().map(|c| c.signed_area()).sum::<f64>())
        .sum()
}

/// 図形から作った地図に壊れた面の輪郭がある（前の版の結合で描いたような）レイヤーでも、輪郭の頂点・辺を動かせる（N-4001）。
/// そのレイヤーの図形を並びの順に塗り重ねて地図を作り直してから動かすので、赤は消えず、上にある緑はそのまま残る
#[test]
fn test_reshape_on_broken_face_contour_repaints_layer() {
    let mut app = broken_face_contour_app();
    let p = Point::new;
    let (red0, green0) = (red_area(&app), green_area(&app));
    let undo0 = app.history.undo_stack.len();
    // 赤の左上の角を左へ 2px：左のフチが傾き、赤は三角形 1つ分（2 × 100 / 2 = 100）ふえる
    // （塗り重ねで、赤の右のフチのふくらみのうち緑の下になる所は、緑が見えるので赤から外れる）
    assert!(move_vertex(&mut app, p(100.0, 100.0), p(98.0, 100.0)));
    let red1 = red_area(&app);
    assert!((red1 - (red0 + 100.0)).abs() < 5.0, "赤 {red0} → {red1}");
    // 上のフチを上へ 2px 引っぱる：赤がふえる
    assert!(pull_edge(&mut app, p(150.0, 100.0), p(150.0, 98.0)));
    assert!(red_area(&app) > red1 + 50.0, "引っぱった分だけ赤がふえる");
    assert!((green_area(&app) - green0).abs() < 0.01, "緑はそのまま");
    assert_eq!(app.history.undo_stack.len(), undo0 + 2, "Undo を積む");
    assert!(layer_map_is_valid(&app), "書き戻した図形は正しい地図になる");
}

/// 壊れた面の輪郭がある層でも、辺にアンカーを足し、角を消せる（塗り重ねて作り直した地図で編集する）
#[test]
fn test_anchor_edit_on_broken_face_contour_repaints_layer() {
    let mut app = broken_face_contour_app();
    let green0 = green_area(&app);
    let undo0 = app.history.undo_stack.len();
    assert!(apply(
        &mut app,
        Action::AddAnchor {
            pt: Point::new(150.0, 100.0),
        }
    ));
    assert!(apply(
        &mut app,
        Action::DeleteAnchor {
            pt: Point::new(100.0, 100.0),
        }
    ));
    assert!(red_area(&app) > 1000.0, "赤が残る");
    assert!((green_area(&app) - green0).abs() < 0.01, "緑はそのまま");
    assert_eq!(app.history.undo_stack.len(), undo0 + 2, "Undo を積む");
    assert!(layer_map_is_valid(&app), "書き戻した図形は正しい地図になる");
}

/// カーソル合わせ用の地図は、編集したレイヤーの分だけ作り直し、触っていないレイヤーの地図は使い回す（N-2001）
#[test]
fn test_scope_maps_rebuild_only_edited_layer() {
    use crate::canvas_tool::doc_actions::Action;
    let mut app = Is2dApp::default();
    let stroke = |y: f64, color: Color32| Action::PaintStroke {
        points: vec![Point::new(100.0, y), Point::new(300.0, y + 10.0)],
        radius: 8.0,
        color,
    };
    run(
        &mut app,
        &[
            stroke(100.0, Color32::RED),
            Action::AddLayer,
            stroke(200.0, Color32::BLUE),
        ],
    );
    let before = crate::map_hover::scope_maps(app.scene(), &app.session);
    assert_eq!(before.maps.len(), 2);
    let edited = app.active_timeline().active_layer_id();
    run(&mut app, &[stroke(220.0, Color32::GREEN)]);
    let after = crate::map_hover::scope_maps(app.scene(), &app.session);
    assert_eq!(after.maps.len(), 2);
    for m in &after.maps {
        let old = before
            .maps
            .iter()
            .find(|o| o.layer_id == m.layer_id)
            .unwrap();
        assert_eq!(
            std::sync::Arc::ptr_eq(&old.base, &m.base),
            m.layer_id != edited,
            "編集したレイヤーだけ作り直す"
        );
    }
}

/// レイヤーが多くても（しまう地図の数より多い 20 枚）、カーソル合わせの地図は編集したレイヤーの分だけ作り直す
/// （レビュー 8b33f96 の N-2002）
#[test]
fn test_scope_maps_rebuild_only_edited_layer_with_many_layers() {
    use crate::canvas_tool::doc_actions::Action;
    let mut app = Is2dApp::default();
    let stroke = |y: f64| Action::PaintStroke {
        points: vec![Point::new(100.0, y), Point::new(300.0, y + 10.0)],
        radius: 8.0,
        color: Color32::RED,
    };
    run(&mut app, &[stroke(0.0)]);
    for i in 1..20 {
        run(&mut app, &[Action::AddLayer, stroke(20.0 * i as f64)]);
    }
    let before = crate::map_hover::scope_maps(app.scene(), &app.session);
    assert_eq!(before.maps.len(), 20);
    let edited = app.active_timeline().active_layer_id();
    run(&mut app, &[stroke(500.0)]);
    let after = crate::map_hover::scope_maps(app.scene(), &app.session);
    let rebuilt: Vec<crate::vector_engine::LayerId> = after
        .maps
        .iter()
        .filter(|m| {
            let old = before
                .maps
                .iter()
                .find(|o| o.layer_id == m.layer_id)
                .unwrap();
            !std::sync::Arc::ptr_eq(&old.base, &m.base)
        })
        .map(|m| m.layer_id)
        .collect();
    assert_eq!(rebuilt, vec![edited], "編集したレイヤーだけ作り直す");
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

    app.set_selected_indices(vec![1, 2]);
    let eye_gid = doc_actions::group_selected_shapes(&mut app).unwrap();
    app.set_selected_indices(vec![0, 1, 2]);
    let head_gid = doc_actions::group_selected_shapes(&mut app).unwrap();
    let s = scene_summary(&app);
    assert_eq!(s.shapes[0].group_path, vec![head_gid]);
    assert_eq!(s.shapes[1].group_path, vec![head_gid, eye_gid]);
    assert_eq!(s.shapes[2].group_path, vec![head_gid, eye_gid]);

    assert!(select_ops::enter_group_at(
        &mut app,
        Point::new(200.0, 200.0)
    ));
    assert_eq!(app.session.editing_group_path(), vec![head_gid]);
    let s = scene_summary(&app);
    assert!(s.shapes[0].is_raw_at_scope(app.session.editing_group_path()));
    assert_eq!(
        s.shapes[1].active_group_id_at_scope(app.session.editing_group_path()),
        Some(eye_gid)
    );

    assert!(select_ops::enter_group_at(
        &mut app,
        Point::new(100.0, 200.0)
    ));
    assert_eq!(app.session.editing_group_path(), vec![head_gid, eye_gid]);
    assert!(scene_summary(&app).shapes[1].is_raw_at_scope(app.session.editing_group_path()));

    assert!(select_ops::exit_current_group(&mut app));
    assert_eq!(app.session.editing_group_path(), vec![head_gid]);
    assert!(select_ops::exit_current_group(&mut app));
    assert!(app.session.editing_group_path().is_empty());
    assert_eq!(app.session.selected_indices().len(), 3);

    assert!(doc_actions::ungroup_selected_shapes(&mut app));
    // 生に戻った赤は、選んだまま浮いた片になる（確定のときにルートの地図に重ねる）。目のグループは文書に残る
    let cel_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
    assert_eq!(cel_shapes.len(), 2);
    assert!(cel_shapes.iter().all(|s| s.group_path == vec![eye_gid]));
    app.session.floating().expect("生に戻った図形は浮いた片");
    assert!(app.session.floating_view()
        .pieces()
        .iter()
        .any(|p| p.color() == red && p.group_path.is_empty()));
    let s = scene_summary(&app);
    assert!(s
        .shapes
        .iter()
        .find(|sh| sh.color() == red)
        .unwrap()
        .group_path
        .is_empty());
    assert_eq!(
        s.shapes
            .iter()
            .find(|sh| sh.color() == blue)
            .unwrap()
            .group_path,
        vec![eye_gid]
    );
    assert_eq!(
        s.shapes
            .iter()
            .find(|sh| sh.color() == green)
            .unwrap()
            .group_path,
        vec![eye_gid]
    );
    // 選択を外すと、赤はルートの地図に入る
    assert!(canvas_tool::clear_selection(&mut app));
    let cel_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
    assert!(cel_shapes
        .iter()
        .any(|s| s.color() == red && s.group_path.is_empty()));
}

/// レビュー 41766f9 の N-6001：変換が潰れて画面に図形を出さないシンボル（反転のトゥイーンの真ん中で分けたコマなど）が
/// あっても、重ね順を変えると、そのシンボルは並びの中の位置のまま動かない（重ね順の並びを文書のセル画の子から作る。前は
/// 画面の図形から作ったので、見えないシンボルが並びから漏れ、黙っていちばん下へ動いた）
#[test]
fn arranging_keeps_an_invisible_instance_in_place() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::canvas_tool::select_ops::ArrangeMode;
    let mut app = Is2dApp::default();
    let color = Color32::from_rgb(120, 175, 160);
    let rects: Vec<FillShape> = (0..4)
        .map(|k| map_overlay::rect_shape(100.0 + 150.0 * k as f64, 100.0, 200.0 + 150.0 * k as f64, 200.0, color))
        .collect();
    put_cel_shapes(&mut app, 0, 0, rects);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    // 下から：グループ・シンボル・グループ・グループ
    let center = |k: usize| Point::new(150.0 + 150.0 * k as f64, 150.0);
    for k in 0..4 {
        assert!(apply(&mut app, Action::SelectSingleAt { pt: center(k) }));
        assert!(apply(&mut app, if k == 1 { Action::ConvertToSymbol } else { Action::Group }));
        apply(&mut app, Action::Deselect);
    }
    let order = |app: &Is2dApp| -> Vec<u64> {
        let tl = app.tab().active_timeline();
        tl.layers[0].cel_at(tl.current_frame).unwrap().children(&[])
    };
    let before = order(&app);
    assert_eq!(before.len(), 4);
    let inst = app.tab().active_timeline().instances().next().unwrap().binding.instance_gid;
    assert_eq!(before[1], inst, "シンボルが下から2番目にある");
    // シンボルの変換を潰す（横幅 0：画面に図形を出さない）
    let set_xform = |app: &mut Is2dApp, f: &dyn Fn(Affine) -> Affine| {
        app.tab_mut().with_active_timeline_mut(|tl, _| {
            let kf = tl.layers[0].keyframe_index_at(0).unwrap();
            let cel = &mut tl.layers[0].keyframe_mut(kf).cel;
            let i = cel.instances_mut();
            i[0].binding.xform = f(i[0].binding.xform);
        });
    };
    let orig = app.tab().active_timeline().instances().next().unwrap().binding.xform;
    set_xform(&mut app, &|x| x * Affine::scale_non_uniform(0.0, 1.0));
    assert!(
        app.scene().iter().all(|s| s.symbol_instance.is_none_or(|b| b.instance_gid != inst)),
        "変換の潰れたシンボルが画面に図形を出している"
    );
    // いちばん上のグループを最背面へ
    assert!(apply(&mut app, Action::SelectSingleAt { pt: center(3) }));
    assert!(apply(&mut app, Action::Arrange { mode: ArrangeMode::ToBack }));
    set_xform(&mut app, &|_| orig);
    let after = order(&app);
    assert_eq!(after, vec![before[3], before[0], before[1], before[2]], "見えないシンボルが並びの中で動いた");
}

#[test]
fn test_stage7_z_order_group_symbol_ungroup_paste_alt_duplicate_and_move() {
    use crate::canvas_tool::doc_actions::Action;

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
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&red_shape); app.session.replace_selection(sel); }
        run(&mut app, &[Action::Group]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color()).collect();
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
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&red_shape); app.session.replace_selection(sel); }
        run(&mut app, &[Action::ConvertToSymbol]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color()).collect();
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
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&sym_shape); app.session.replace_selection(sel); }
        run(&mut app, &[Action::Ungroup]);
        let colors_after_break: Vec<_> = app.scene().iter().map(|s| s.color()).collect();
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
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&red_shape); app.session.replace_selection(sel); }
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
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&grouped_shape); app.session.replace_selection(sel); }
        run(&mut app, &[Action::Ungroup]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color()).collect();
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
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&red_shape); app.session.replace_selection(sel); }
        run(&mut app, &[Action::Copy, Action::Paste { in_place: false }]);
        let colors: Vec<_> = app.scene().iter().map(|s| s.color()).collect();
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
        app_alt.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app_alt.session.selection().clone(); sel.insert_shape(&first_red); app_alt.session.replace_selection(sel); }
        run(
            &mut app_alt,
            &[Action::FinishMoveSelectionDrag {
                total_delta: KVec2::new(0.0, 80.0),
                alt_duplicate: true,
            }],
        );
        let colors_after_alt: Vec<_> = app_alt.scene().iter().map(|s| s.color()).collect();
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
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&red_shape); app.session.replace_selection(sel); }
        run(&mut app, &[Action::ConvertToSymbol]);
        // 変換で BottomSym が最前面に来たので、残りの blue もシンボル化して TopSym を最前面にする
        let blue_shape = app.scene().get(0).unwrap().clone();
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&blue_shape); app.session.replace_selection(sel); }
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
        assert_eq!(app.scene().get(0).unwrap().color(), red);
        assert_eq!(app.scene().get(1).unwrap().color(), blue);

        // 下側のシンボル (BottomSym) を TopSym とさらに重なる位置へ移動
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&bottom_sym); app.session.replace_selection(sel); }
        let stage = app.stage();
        app.session.begin_move_overlay(&stage, false, None, Vec::new());
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
        let orig_red_area = app.scene().get(0).unwrap().exterior().signed_area().abs();
        let green_shape = app.scene().get(1).unwrap().clone();
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&green_shape); app.session.replace_selection(sel); }
        run(&mut app, &[Action::Group]);
        let grouped_green = app.scene().get(1).unwrap().clone();

        // グループを生の図形 (red) の上に重ねて移動し、選択解除する
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&grouped_green); app.session.replace_selection(sel); }
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
        assert_eq!(app.scene().get(0).unwrap().color(), red);
        assert_eq!(app.scene().get(1).unwrap().color(), green);
        let red_area = app.scene().get(0).unwrap().exterior().signed_area().abs();
        assert!(
            (red_area - orig_red_area).abs() < 1.0,
            "グループを生の図形の上に動かしても生の図形は削られないこと (area={red_area}, orig={orig_red_area})"
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
        |app: &Is2dApp| -> Vec<Color32> { app.tab().scene().iter().map(|s| s.color()).collect() };
    let select_by_color = |app: &mut Is2dApp, target_colors: &[Color32]| {
        apply(app, Action::Deselect);
        let mut sel = crate::canvas_tool::Selection::default();
        let depth = app.tab().session.editing_group_path().len();
        for s in app.tab().scene().iter() {
            if target_colors.contains(&s.color()) {
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
        app.tab_mut().session.replace_selection(sel);
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
            .filter(|s| [c_raw, c_a, c_b, c_c].contains(&s.color()))
            .all(|s| s.layer_id == l1_id));
        assert!(app
            .tab()
            .scene()
            .iter()
            .filter(|s| [c_l2_1, c_l2_2].contains(&s.color()))
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
        assert_eq!(app.tab().session.editing_group_path().len(), 1);
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
        let sym_id = app.tab().doc.library().at(0).unwrap().id;
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

/// ゆらゆらした長い横線の、波の頂点を引っぱっても、線全体はついてこない（つかんだ波のあたりだけ。利用者が見つけた）。
/// 回った角度を符号付きで足していたときは、右へ・左へ曲がる波で打ち消し合って `PULL_REACH_TURN` に届かず、線の端まで
/// ついてきた
#[test]
fn pulling_a_wave_crest_does_not_drag_the_whole_wavy_line() {
    let mut app = Is2dApp::default();
    // 横 800px に 8つの波（高さ ±8px、いちばん急な所の傾き 27°）
    let wave: Vec<Point> = (0..=400)
        .map(|i| {
            let x = 100.0 + 2.0 * i as f64;
            Point::new(x, 300.0 + 8.0 * (std::f64::consts::TAU * (x - 100.0) / 100.0).sin())
        })
        .collect();
    run(&mut app, &[Action::PaintStroke { points: wave, radius: 4.0, color: Color32::BLACK }]);
    let before: Vec<Point> = scene_summary(&app).shapes[0].exterior().segments.iter().map(|q| q.p0).collect();
    // 真ん中あたりの、上の縁の波の頂点（x = 525 の山。y が小さい方が上）
    let crest = Point::new(525.0, 300.0 - 8.0 - 4.0);
    let v = *before.iter().min_by(|a, b| (**a - crest).hypot().total_cmp(&(**b - crest).hypot())).expect("頂点がある");
    tool_select::start_reshape_drag(&mut app, v, false, false);
    let to = v + KVec2::new(0.0, -20.0);
    tool_select::update_reshape_drag(&mut app, to);
    tool_select::finish_reshape_drag(&mut app, Some(to));
    let after: Vec<Point> = scene_summary(&app).shapes.iter().flat_map(|s| s.exterior().segments.iter().map(|q| q.p0)).collect();
    let far_moved: Vec<&Point> = before.iter().filter(|p| (p.x - v.x).abs() > 150.0 && !after.contains(p)).collect();
    assert!(far_moved.is_empty(), "つかんだ所から 150px より遠い頂点が動いた：{} 個", far_moved.len());
    assert!(before.iter().any(|p| *p != v && (p.x - v.x).abs() < 50.0 && !after.contains(p)), "まわりがついてこない");
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
    cross_gid_app.set_selected_indices(vec![0]);
    let tab0_inner_gid = doc_actions::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab0_inner_gid, 1);
    cross_gid_app.set_selected_indices(vec![0]);
    let _tab0_sym_id = doc_actions::convert_selection_to_symbol(&mut cross_gid_app).unwrap();
    cross_gid_app.set_selected_indices(vec![0]);
    assert!(doc_actions::copy_selected_shapes(&mut cross_gid_app));

    // 2つ目のタブを開き、Tab 1 側でもグループ（gid=1）を作成してから Tab 0 のシンボルをペースト＆グループ解除（デタッチ）
    crate::project_io::open_project_in_tab(&mut cross_gid_app, &tmp_cross_tab, None).unwrap();
    assert!(
        cross_gid_app.clipboard.content.is_some(),
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
    cross_gid_app.set_selected_indices(vec![0]);
    let tab1_existing_gid = doc_actions::group_selected_shapes(&mut cross_gid_app).unwrap();
    assert_eq!(tab1_existing_gid, 1);

    assert!(doc_actions::paste_clipboard(&mut cross_gid_app, true));
    assert!(doc_actions::ungroup_selected_shapes(&mut cross_gid_app));
    let summary_cross = scene_summary(&cross_gid_app);
    let blue_gid = summary_cross
        .shapes
        .iter()
        .find(|s| s.color() == Color32::BLUE)
        .unwrap()
        .group_path[0];
    let red_gid = summary_cross
        .shapes
        .iter()
        .find(|s| s.color() == Color32::RED)
        .unwrap()
        .group_path[0];
    assert_ne!(
        blue_gid, red_gid,
        "別タブから移植したシンボル内部のグループIDが再採番され、貼り付け先タブの既存グループとID衝突しないこと"
    );
    let _ = std::fs::remove_file(&tmp_cross_tab);
}

/// 拡大表示で大きい絵を少しだけ拡大・回転しても、画面で動いた分は確定する（「変わったか」を、外接矩形の4隅が画面で
/// 動く量で測る）。直す前は、倍率の差 0.005・角度 0.002rad の決め打ちで測っていたので、ズーム 400% で画面 16px の
/// 拡大のドラッグが捨てられた（レビュー f01d518 の N-3003）
#[test]
fn small_scale_and_rotate_drags_at_high_zoom_are_committed() {
    use doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(0.0, 200.0),
            Point::new(1000.0, 200.0),
            20.0,
            Color32::RED,
        ),
    );
    apply(
        &mut app,
        Action::SetTool {
            tool: Tool::Transform,
        },
    );
    apply(&mut app, Action::SelectAll);
    let zoom = 4.0;
    app.tab_mut().view.set_zoom(zoom);
    let bounds = |app: &Is2dApp| {
        app.scene()
            .iter()
            .map(|s| s.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap()
    };
    let before = bounds(&app);
    // 右の辺のつまみを、文書で 4px（画面で 16px）右へ
    let (bbox, _) = app.session.selection_bbox_and_pivot(zoom).unwrap();
    let undo_len = app.history.undo_stack.len();
    let start = Point::new(bbox.x1, bbox.center().y);
    let end = start + KVec2::new(4.0, 0.0);
    tool_transform::start_transform_drag(&mut app, start, false, false);
    tool_transform::update_transform_drag(&mut app, end, false, false);
    tool_transform::finish_transform_drag(&mut app, Some(end));
    assert_eq!(app.history.undo_stack.len(), undo_len + 1, "拡大を確定する");
    assert!((bounds(&app).width() - before.width() - 4.0).abs() < 0.5);
    // 角の外側の回転のつまみを、基準点のまわりに 0.001rad（角は画面で約 2px 動く）
    let undo_len = app.history.undo_stack.len();
    let (bbox, pivot) = app.session.selection_bbox_and_pivot(zoom).unwrap();
    let start = Point::new(bbox.x1 + 15.0 / zoom as f64, bbox.y0 - 15.0 / zoom as f64);
    let end = kurbo::Affine::rotate_about(0.001, pivot) * start;
    tool_transform::start_transform_drag(&mut app, start, false, false);
    tool_transform::update_transform_drag(&mut app, end, false, false);
    tool_transform::finish_transform_drag(&mut app, Some(end));
    assert_eq!(app.history.undo_stack.len(), undo_len + 1, "回転を確定する");
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
    app.set_selected_indices(vec![0]);
    let bbox = app
        .scene()
        .selection_bounding_box(app.session.selection(), app.view.zoom())
        .unwrap();

    // 1. 四隅の少し外側 (+10px, -10px) は Rotate になるが、上辺中央の少し外側 (四隅から100px以上離れた位置) は Rotate にならないこと (#17)
    let corner_outside = Point::new(bbox.x1 + 10.0, bbox.y0 - 10.0);
    assert_eq!(
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            corner_outside,
            app.view.zoom()
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
            app.view.zoom()
        ),
        canvas_tool::TransformHoverTarget::None,
        "四隅から離れた辺中央の外側は Rotate ゾーンにならないこと (#17)"
    );

    // 2. 連続ペーストの途中にズーム倍率を変更しても、毎回前回位置から +16px (画面上) ずつ単調増加の階段状に配置されること (#18)
    app.view.set_zoom(1.0);
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
    app.view.set_zoom(4.0);
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
fn test_cross_tab_symbol_paste_preserves_imported_map_across_intermediate_copies() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    assert_eq!(app.doc.library().len(), 1);
    assert_eq!(app.doc.library().at(0).unwrap().id, 1);
    assert_eq!(app.doc.library().at(0).unwrap().name, "Symbol 1");
    assert!(apply(&mut app, Action::Copy));

    // タブ B（idx 1）を作って 1 回目の貼り付け
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 1);
    assert_eq!(app.doc.library().at(0).unwrap().name, "Symbol 1");

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
        .shapes()
        .last()
        .unwrap()
        .shape_id;
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    app.session.add_to_selection([raw_id], std::iter::empty());
    assert!(apply(&mut app, Action::Copy));

    // タブ A（idx 0）に戻ってシンボル S をもう一度コピーし、タブ B（idx 1）に戻って貼り付ける
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));
    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(
        app.doc.library().len(),
        1,
        "2回目の貼り付けの後もタブ B のライブラリが「Symbol 1」の1つだけであること"
    );
    assert_eq!(app.doc.library().at(0).unwrap().name, "Symbol 1");
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.instances().len(), 2);

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
    assert_eq!(app.doc.library().at(0).unwrap().id, 1);
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(
        app.doc.library().len(),
        2,
        "タブ A の 1 番とタブ C の 1 番が取り違えられず、別々のシンボルとして 2 つ取り込まれること"
    );
    assert_eq!(app.doc.library().at(0).unwrap().name, "Symbol 1");
    assert_eq!(app.doc.library().at(1).unwrap().name, "Symbol 1 (2)");
    assert!(app.scene().iter().any(|s| s.color() == red));
    assert!(app.scene().iter().any(|s| s.color() == green));
}

/// クリップボードの対応表（別のタブへ移したシンボル）は共有の箱で、入口 `apply` が毎回クリップボードを写しても、表は写さず
/// 分け合う。いなくなったタブ（閉じた・作品を入れ替えた）を貼り先とする記録は捨てる。閉じたタブから写した中身は、閉じた
/// あとに何度貼っても同じシンボルを使い回し、中身を写し直したら、その閉じたタブの記録も捨てる（前は表を毎回丸ごと写し、
/// 捨てる所が無く、アプリを閉じるまで増えた：設計審査 6fb4faa の D-4）
#[test]
fn the_clipboard_import_table_is_shared_and_forgets_gone_tabs() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ctx = egui::Context::default();
    let symbol_tab = |app: &mut Is2dApp| {
        assert!(apply(
            app,
            Action::PaintStroke { points: vec![Point::new(50.0, 50.0), Point::new(120.0, 50.0)], radius: 8.0, color: Color32::RED },
        ));
        assert!(apply(app, Action::SelectAll));
        assert!(apply(app, Action::ConvertToSymbol));
        assert!(apply(app, Action::Copy));
    };
    // タブ A（0）のシンボルを、タブ B（1）とタブ C（2）に貼る
    let mut app = Is2dApp::default();
    symbol_tab(&mut app);
    let a = app.tab().id();
    crate::project_io::new_project(&mut app, &ctx);
    let b = app.tab().id();
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    crate::project_io::new_project(&mut app, &ctx);
    let c = app.tab().id();
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    let mentions = |app: &Is2dApp, id: u64| app.clipboard.imported_map.keys().any(|&(s, _, t)| s == id || t == id);
    assert!(mentions(&app, b) && mentions(&app, c));
    // 写しは表を分け合う
    let copy = app.clipboard.clone();
    assert!(std::sync::Arc::ptr_eq(&copy.imported_map, &app.clipboard.imported_map), "クリップボードの写しが表を写した");
    // タブ C を閉じると、C の記録が消える。B の記録は残る
    crate::project_io::close_tab(&mut app, 2, None);
    assert!(!mentions(&app, c), "閉じたタブの記録が残った");
    assert!(mentions(&app, b));
    // コピー元のタブ A を閉じても、A から写した中身をタブ B に貼るたびに、同じシンボルを使い回す
    crate::project_io::close_tab(&mut app, 0, None);
    assert_eq!(app.tab().id(), b);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 1, "閉じたタブから写した中身を貼って、シンボルが増えた");
    assert!(mentions(&app, a));
    // 最後のタブ B を閉じる（タブに新しい作品を入れる道 `install_document` を通る）と、B の前の番号の記録が消える
    crate::project_io::close_tab(&mut app, 0, None);
    assert_ne!(app.tab().id(), b);
    assert!(!mentions(&app, b), "作品を入れ替えたタブの前の番号の記録が残った");
    // 中身を写し直すと、閉じたタブ A の記録も消える
    symbol_tab(&mut app);
    assert!(!mentions(&app, a), "閉じたコピー元のタブの記録が、写し直したあとも残った");
}

#[test]
fn test_cross_tab_symbol_roundtrip_paste_reuses_original_symbol() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    let orig_sym_id = app.doc.library().at(0).unwrap().id;
    assert!(apply(&mut app, Action::Copy));

    // タブ2（index 1）へペースト → タブ2でそのインスタンスをコピー
    crate::project_io::new_project(&mut app, &ctx);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 1);
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));

    // タブ1へ戻してペースト → タブ1のライブラリはシンボル1つのままで、貼ったインスタンスは元のシンボルを指す
    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    assert_eq!(app.doc.library().len(), 1);
    let instances = &app.doc.root.layers[0].keyframes[0].cel.instances();
    assert_eq!(instances.len(), 2);
    assert!(instances
        .iter()
        .all(|inst| inst.binding.symbol_id == orig_sym_id));

    // 続けてタブ1でコピー → タブ2へペーストしても、タブ2のライブラリはシンボル1つのまま
    assert!(apply(&mut app, Action::Copy));
    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    assert_eq!(app.doc.library().len(), 1);
}

/// グループの変形は、地図を書く入口（`write_local`）の1つを通る：選んだグループが2つのレイヤーにあっても、どちらも動く。
/// 書く所のどれかが断ったら、そこで止めて知らせる（前はグループの変形だけが入口の中身を写した道で書き、断った理由を捨て、
/// 断られたグループだけが黙って動かずに残った：レビュー de92ceb の N-4001。止めたあとの戻しは `apply` の
/// `restore_state_before_action`）
#[test]
fn group_transforms_go_through_the_one_map_entry() {
    use crate::canvas_tool::doc_actions::{apply, write_local, Action};
    use crate::planar_map::{LocalEdit, LocalRefusal};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED)]);
    assert!(apply(&mut app, Action::AddLayer));
    let other = app.doc.root.active_layer;
    put_cel_shapes(&mut app, other, 0, vec![map_overlay::rect_shape(300.0, 100.0, 400.0, 200.0, Color32::BLUE)]);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    // 2つのレイヤーの絵を、それぞれグループにして、両方を選ぶ
    for (i, pt) in [Point::new(150.0, 150.0), Point::new(350.0, 150.0)].into_iter().enumerate() {
        crate::canvas_tool::select_ops::apply_click_selection(&mut app, pt, false, true);
        assert!(apply(&mut app, Action::Group), "グループ {i}");
    }
    assert!(apply(&mut app, Action::SelectAll));
    let scopes = app.session.selected_group_scopes();
    assert_eq!(scopes.len(), 2, "2つのレイヤーのグループの階層");
    let at = |app: &Is2dApp, x: f64| app.scene().iter().any(|s| s.contains_point(Point::new(x, 150.0)));
    // 動かすと、両方のレイヤーのグループが動く
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(0.0, 300.0) }));
    assert!(!at(&app, 150.0) && !at(&app, 350.0), "どちらかのグループが動かずに残った");
    assert!(apply(&mut app, Action::Undo));
    assert!(at(&app, 150.0) && at(&app, 350.0));
    // 書く所の1つめが断ったら、そこで止めて知らせる（2つめは書かない）
    let maps_before: Vec<_> = app.doc.root.layers.iter().map(|l| l.keyframes[0].cel.maps().clone()).collect();
    let first = scopes[0].1.clone();
    let tab = &mut app.tabs[app.active];
    let wrote = write_local(tab, &scopes, |base, scope| {
        if scope == first.as_slice() {
            LocalEdit::Refused(LocalRefusal::Unwritable)
        } else {
            base.transformed_local(kurbo::Affine::translate((0.0, 300.0)), scope)
        }
    });
    assert!(!wrote);
    assert_eq!(app.session.refusal, Some(crate::canvas_tool::Refusal::Unwritable), "断ったことを知らせない");
    assert!(app.doc.root.layers.iter().zip(&maps_before).all(|(l, m)| std::sync::Arc::ptr_eq(l.keyframes[0].cel.maps(), m)), "断ったあとも書き続けた");
}

#[test]
fn test_cross_tab_repaste_after_source_symbol_edit_imports_current_content() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let ctx = egui::Context::default();
    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 90, 230);
    let green = Color32::from_rgb(40, 180, 90);
    let yellow = Color32::from_rgb(230, 200, 40);
    let lib_names =
        |app: &Is2dApp| -> Vec<String> { symbols_in_order(app.doc.library()).map(|s| s.name.clone()).collect() };

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
    let sym_s_id = app.doc.library().at(0).unwrap().id;
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
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(lib_names(&app), vec!["Symbol 1", "Symbol 1 (2)"]);
    assert!(
        app.scene().iter().any(|s| s.color() == blue),
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
    let sym_c_id = symbols_in_order(app.doc.library()).last().unwrap().id;
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let sym_p_id = symbols_in_order(app.doc.library()).last().unwrap().id;
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 4);

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
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 6);
    assert!(
        app.scene().iter().any(|s| s.color() == yellow),
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
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    assert!(apply(&mut app, Action::PlaceSymbol { sym_id: sym_c_id }));
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 7);
    let new_c_in_b = symbols_in_order(app.doc.library()).last().unwrap().id;

    crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
    let p_gid = app.doc.root.layers[0].keyframes[0]
        .cel
        .instances()
        .iter()
        .find(|inst| inst.binding.symbol_id == sym_p_id)
        .unwrap()
        .binding
        .instance_gid;
    app.session.select_symbol_instance_by_gid(p_gid);
    assert!(apply(&mut app, Action::Copy));

    crate::project_io::switch_to_tab(&mut app, 1, Some(&ctx));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 8);
    let new_p_in_b = symbols_in_order(app.doc.library()).last().unwrap();
    assert!(
        new_p_in_b
            .timeline
            .instances()
            .all(|inst| inst.binding.symbol_id == new_c_in_b),
        "新しい P のインスタンスはすべて先に移植された新しい C を指していること"
    );

    // 6. B で関係のないシンボル（最初の Symbol 1）を削除する（7つ）。もう一度貼っても7つのまま
    let first_sym_id_in_b = app.doc.library().at(0).unwrap().id;
    assert_eq!(app.doc.library().at(0).unwrap().name, "Symbol 1");
    assert!(apply(
        &mut app,
        Action::DeleteSymbol {
            sym_id: first_sym_id_in_b,
        },
    ));
    assert_eq!(app.doc.library().len(), 7);
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 7);
}

/// 絵を書く入口（`write_cel`）は、持ち上げた絵の出どころ（そのレイヤー・編集中の階層の、持ち上げた元の地図）を、確定の
/// ほかで書き換えない：書かずに断りを記す（操作ごとの表を書き間違えても、絵は壊れずに断られる：設計
/// claude/約束を入口で守る設計_5616fcc.md の 2.）。ほかの階層や、持ち上げていないレイヤーには書ける
#[test]
fn writing_under_lifted_pieces_is_refused_at_the_write_entry() {
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(200.0, 350.0), additive: false }));
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(20.0, 0.0) }));
    let lid = app.doc.root.layers[0].id;
    let before = app.doc.root.layers[0].keyframes[0].cel.maps().clone();
    let tab = app.tab_mut();
    let wrote = crate::canvas_tool::write_cel(tab, lid, |cel| {
        cel.clear(&[])
    });
    assert!(!wrote, "持ち上げた絵の出どころを書き換えた");
    assert_eq!(tab.session.refusal, Some(crate::canvas_tool::Refusal::UnderLifted));
    assert!(std::sync::Arc::ptr_eq(app.doc.root.layers[0].keyframes[0].cel.maps(), &before));
}

/// 自由変形の中心点は、選択か見ている場所（コマ・レイヤー・シンボルの中・グループの階層）が変わったら真ん中に戻す（戻すのは
/// 入口 `apply` の1か所：持ち主が決めた 2026-11-27）。選んだ図形が同じままのグループ化では残る（持ち主が決めた）。前は
/// グループ化・タイムラインの後片付け・取り消しなど 8 か所で手で戻していた（設計 入口の部屋の設計_27c4085 の 2.3）
#[test]
fn transform_pivot_resets_only_when_selection_or_view_changes() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    assert!(apply(&mut app, Action::ExtendFrame));
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectAll));
    let pivot = Point::new(120.0, 130.0);
    let set_pivot = |app: &mut Is2dApp| {
        assert!(apply(app, Action::CommitPivotDrag { new_pivot: pivot, orig_pivot: Point::new(200.0, 200.0), had_custom_pivot: false, changed: true }));
        assert_eq!(app.session.transform_pivot(), Some(pivot));
    };
    set_pivot(&mut app);
    // 選んだものを動かしても（選択も見ている場所も同じ）真ん中には戻らず、一緒に動く
    assert!(apply(&mut app, Action::NudgeSelection { delta: KVec2::new(5.0, 0.0) }));
    let moved = Some(pivot + KVec2::new(5.0, 0.0));
    assert_eq!(app.session.transform_pivot(), moved, "動かしただけで中心点が戻った");
    // グループ化（選んだ図形は同じ）では残る
    assert!(apply(&mut app, Action::Group));
    assert_eq!(app.session.transform_pivot(), moved, "グループ化で中心点が戻った");
    // コマを変えると戻る
    assert!(apply(&mut app, Action::SwitchLayerAndFrame { layer_idx: 0, frame: 1 }));
    assert_eq!(app.session.transform_pivot(), None, "コマを変えても中心点が残った");
    // 選択を変えると戻る
    assert!(apply(&mut app, Action::SwitchLayerAndFrame { layer_idx: 0, frame: 0 }));
    assert!(apply(&mut app, Action::SelectAll));
    set_pivot(&mut app);
    assert!(apply(&mut app, Action::Deselect));
    assert_eq!(app.session.transform_pivot(), None, "選択を外しても中心点が残った");
}

/// A-7：自由変形の回転の不感帯は画面で測る（拡大していても、基準点から画面で 2px より離れれば回る）
#[test]
fn rotation_dead_zone_is_measured_on_screen() {
    let red = Color32::from_rgb(235, 75, 75);
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            10.0,
            red,
        ),
    );
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool {
            tool: Tool::Transform,
        },
    );
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(250.0, 150.0),
        false,
    );
    app.view.set_zoom(16.0);
    let (bbox, pivot) = app
        .session
        .selection_bbox_and_pivot(app.view.zoom())
        .unwrap();
    let corner = Point::new(bbox.x1, bbox.y1);
    tool_transform::start_transform_drag(&mut app, corner + KVec2::new(0.5, 0.5), false, false);
    // 基準点から 1px（画面で 16px）の真上
    tool_transform::update_transform_drag(&mut app, pivot + KVec2::new(0.0, 1.0), false, false);
    let canvas_tool::ActiveCanvasDrag::Transform(canvas_tool::TransformDragState::Rotate {
        current_angle,
        ..
    }) = &app.session.drag
    else {
        panic!("回転のドラッグになっていない");
    };
    assert!(current_angle.abs() > 0.1);
}

/// 境目の辺や頂点をランダムに 40 回ドラッグしても、図形どうしの間にすき間も重なりもできず、区間が細切れにならない
#[test]
fn test_random_shared_boundary_drags_keep_shapes_together() {
    let c = Point::new(300.0, 300.0);
    let mut app = three_color_junction_scene();
    let segs_before: usize = scene_summary(&app)
        .shapes
        .iter()
        .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
        .sum();
    let mut state = 3u64;
    let mut rnd = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 11) as f64) / ((1u64 << 53) as f64)
    };
    for k in 0..40 {
        let s = scene_summary(&app);
        let fills: Vec<&FillShape> = s
            .shapes
            .iter()
            .filter(|x| x.color() == Color32::BLUE || x.color() == Color32::GREEN)
            .collect();
        let f = fills[(rnd() * fills.len() as f64) as usize % fills.len()];
        let q = f.exterior().segments
            [(rnd() * f.exterior().segments.len() as f64) as usize % f.exterior().segments.len()];
        let ang = rnd() * std::f64::consts::TAU;
        let d = KVec2::new(ang.cos(), ang.sin()) * (2.0 + 4.0 * rnd());
        if rnd() < 0.5 {
            let p = q.eval(0.3 + 0.4 * rnd());
            let _ = pull_edge(&mut app, p, p + d);
        } else {
            let _ = move_vertex(&mut app, q.p0, q.p0 + d);
        }
        assert_eq!(
            shared_boundary_coverage(
                &app,
                |p| (p - c).hypot() < 106.0,
                (190.0, 190.0, 410.0, 410.0)
            ),
            (0, 0),
            "{k} 回目"
        );
    }
    let segs_after: usize = scene_summary(&app)
        .shapes
        .iter()
        .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
        .sum();
    assert!(
        segs_after < segs_before * 2,
        "区間 {segs_before} → {segs_after}"
    );
}
