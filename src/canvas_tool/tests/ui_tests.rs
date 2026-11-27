//! 画面（画面の土台・拡大・先読み）・メニューとキー・知らせ・タブとダイアログ・操作の入口と取り消し・砦・層の向きと呼び名のテスト

use super::*;

/// 先読みのテスト用：レイヤー 0 のコマ 0・3・6・9 にキーフレームを打ち、それぞれに違う四角を置いたアプリ（12 コマ）
fn app_with_cels_every_three_frames() -> Is2dApp {
    let mut app = Is2dApp::default();
    for f in [0usize, 3, 6, 9] {
        doc_actions::switch_to_layer_and_frame(&mut app, 0, f);
        if f > 0 {
            assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
        }
        put_cel_shapes(&mut app, 0, f, vec![map_overlay::rect_shape(10.0 + f as f64, 10.0, 90.0, 90.0, Color32::RED)]);
    }
    app.tab_mut().with_active_timeline_mut(|tl, _| {
        tl.layers[0].frame_count = 12;
        tl.sync_total_frames();
    });
    app
}

/// 1コマ動かす（`modifiers` を押したまま、合図 `events` を送る）。`before` はそのコマの初めに呼ぶ
fn menu_test_frame(
    ctx: &egui::Context,
    app: &mut Is2dApp,
    modifiers: egui::Modifiers,
    events: Vec<egui::Event>,
    before: impl FnOnce(&egui::Context, &mut Is2dApp),
) {
    let mut before = Some(before);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1280.0, 820.0),
            )),
            modifiers,
            events,
            ..Default::default()
        },
        |ctx| {
            if let Some(before) = before.take() {
                before(ctx, app);
            }
            app.update_ui(ctx);
        },
    );
}

/// キーを押して離す合図（離さないと、egui は次に押したときを押しっぱなしの繰り返しとする）
fn menu_key_tap(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    [true, false]
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        })
        .to_vec()
}

#[test]
fn test_viewport_fit_stage_to_window() {
    let mut app = Is2dApp {
        canvas_rect: eframe::egui::Rect::from_min_max(
            Pos2::new(0.0, 0.0),
            Pos2::new(1600.0, 900.0),
        ),
        ..Default::default()
    };
    app.doc.stage_size = Vec2::new(800.0, 450.0);
    app.fit_stage_to_window();
    assert!(
        app.view.zoom() > 1.5 && app.view.zoom() <= 2.0,
        "1600x900 キャンバスに対して 800x450 ステージがマージン付きでフィット拡大されること: zoom={}",
        app.view.zoom()
    );
    let center = app.canvas_rect.center();
    let world_before = app.view.screen_to_world(center);
    app.view.zoom_around_screen_pos(center, 1.5);
    let world_after = app.view.screen_to_world(center);
    assert!(
        (world_before - world_after).hypot() < 1e-3,
        "指定スクリーン座標を中心としたズーム後もその座標下のワールド座標がずれないこと"
    );
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
            .topmost_shape_at(Point::new(140.0, 100.0), app.session.editing_group_path()),
        Some(0),
        "Scene::topmost_shape_at で現在フレームの図形がヒットすること"
    );
    let s0_id = app.scene().get(0).unwrap().shape_id;
    assert_eq!(
        select_ops::compute_hover_target(
            app.scene(),
            &app.session,
            Point::new(140.0, 100.0),
            app.view.zoom(),
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
    let (mask, any_sel) = app.scene().selected_shape_mask(app.session.selection());
    assert!(any_sel);
    assert_eq!(mask, vec![true]);
    let bbox = app
        .scene()
        .selection_bounding_box(app.session.selection(), app.view.zoom())
        .unwrap();
    let tl_handle = canvas_tool::ScaleHandle::TopLeft.position(bbox);
    assert_eq!(
        tool_transform::compute_transform_hover(
            app.scene(),
            &app.session,
            tl_handle,
            app.view.zoom()
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
            .topmost_shape_at(Point::new(140.0, 160.0), app.session.editing_group_path()),
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
fn test_stage7c_scene_base_cache_and_write_cels() {
    use crate::canvas_tool::doc_actions::Action;

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
    let rev_bottom_before = app.doc.root.layers[0].keyframes[0].cel.revision();
    let rev_top_before = app.doc.root.layers[1].keyframes[0].cel.revision();

    // 1. 下のレイヤーのメッシュキャッシュを取得しておき、上のレイヤーだけをブラシ描画で更新しても
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
        app.doc.root.layers[0].keyframes[0].cel.revision(), rev_bottom_before,
        "編集対象外のレイヤーの Cel::revision は変化しないこと"
    );
    assert_ne!(
        app.doc.root.layers[1].keyframes[0].cel.revision(), rev_top_before,
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

    // 2. セル画を直接書いた場合も指定したレイヤーのみ書き換わり、SceneBaseCache が即座に同期されること
    let changed = canvas_tool::write_cels(
        &mut app.tabs[app.active],
        &[layer_bottom_id],
        |_, cel, held| {
            Ok(cel.remove(
                &[],
                &std::collections::HashSet::from([bottom_shape_id]),
                &std::collections::HashSet::new(),
                held,
            )?)
        },
    );
    assert!(changed);
    assert_ne!(
        app.doc.root.layers[0].keyframes[0].cel.revision(),
        rev_bottom_before
    );
    assert!(app.scene().iter().all(|s| s.shape_id != bottom_shape_id));
    let _ = layer_top_id;
}

#[test]
fn test_stage7_cel_writes_preserve_mesh_cache_without_rebuilding_from_doc() {
    use crate::canvas_tool::doc_actions::Action;

    let mut app = Is2dApp::default();
    let mut initial_shapes = Vec::with_capacity(2000);
    for i in 0..2000 {
        let col = i % 50;
        let row = i / 50;
        let x0 = (col as f64) * 40.0;
        let y0 = (row as f64) * 30.0;
        let c = Color32::from_rgb(
            ((i * 37) % 200 + 40) as u8,
            ((i * 73) % 200 + 40) as u8,
            150,
        );
        // 隣の線と重ならない間隔にする（同じレイヤーの生の図形が重なると、地図として正しくないので、
        // レイヤーを塗り重ねて作り直し、図形が全部書き換わる）
        initial_shapes.extend(make_stroke(
            Point::new(x0, y0 + 10.0),
            Point::new(x0 + 10.0, y0 + 10.0),
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
        "セル画を書いた後に未変更図形の描画キャッシュ (Arc<Mesh>) が複製や再構築なしでそのまま引き継がれること"
    );

    // 2. 回転したシンボル内部でも同様に描画キャッシュがそのまま引き継がれること
    let all_shapes: Vec<_> = app.scene().iter().cloned().collect();
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    for s in &all_shapes {
        { let mut sel = app.session.selection().clone(); sel.insert_shape(s); app.session.replace_selection(sel); }
    }
    run(&mut app, &[Action::ConvertToSymbol]);
    let sym_shape = app.scene().get(0).unwrap().clone();
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&sym_shape); app.session.replace_selection(sel); }
    run(&mut app, &[Action::RotateSelection { angle_rad: 0.35 }]);
    let binding = app.scene().get(0).unwrap().symbol_instance.unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app.tabs[app.active],
        binding.instance_gid
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
        "回転したシンボル内でセル画を書いた後も未変更図形の Arc<Mesh> がそのまま引き継がれること"
    );
}

#[test]
fn test_stage7_scene_unchanged_after_rebuild_overlay_and_base_cache_on_every_action() {
    use crate::canvas_tool::doc_actions::Action;

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
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&first_shape); app.session.replace_selection(sel); }
    run_and_verify_scene_cache_consistency(&mut app, Action::Group, "Group");
    run_and_verify_scene_cache_consistency(&mut app, Action::Ungroup, "Ungroup group");

    let sym_target = app.scene().get(0).unwrap().clone();
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&sym_target); app.session.replace_selection(sel); }
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
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&sym_inst_shape); app.session.replace_selection(sel); }
    run_and_verify_scene_cache_consistency(
        &mut app,
        Op::Scale(1.35, 0.72),
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
            instance_gid: transformed_binding.instance_gid,
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
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&inner_first); app.session.replace_selection(sel); }
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
        Action::ExitGroupSteps { steps: 1 },
        "ExitSymbolTimeline from rotated+scaled symbol",
    );
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
            binding: symbol_binding::SymbolInstanceBinding::new(
                (k + 1) as u64,
                sym_id,
                kurbo::Affine::IDENTITY,
            ),
            layer_id: 1,
            parent_scope: Vec::new(),
            order: 0,
        })
        .collect();
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_shapes_and_instances(Vec::new(), instances_120);
    app.refresh_scene_base();
    assert_eq!(
        app.session.base_cache().shapes().len(),
        12_000,
        "100図形のシンボルを120個置いたレイヤーで、土台キャッシュに12,000個の図形がすべて載ること"
    );
    assert_eq!(app.scene().len(), 12_000);
}

#[test]
fn test_other_2_cmd_q_triggers_close_confirm_and_does_not_switch_tool() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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

/// 全部の操作の種類で、操作のあとの画面の図形が、画面の写し（土台・上乗せ）を作り直した結果（お手本）と同じ
/// （`run_and_verify_scene_cache_consistency`）。操作は、でたらめな操作の試しと同じ `Action` の全部の種類から作る（種類を
/// 足すと、作れるようにするまで止まる：設計 claude/約束を入口で守る設計_5616fcc.md の 7.。前は手で並べた 21 種だけだった：
/// 約束の棚卸し 5616fcc の A の P-16 (f)）
#[test]
fn scene_matches_a_full_rebuild_after_every_action_kind() {
    let names = action_kind_names();
    let drawing = ["PaintStroke", "PencilStroke", "BucketFill", "SelectMarquee"];
    let mut used = std::collections::BTreeSet::new();
    for seed in [0x5eed_0001u64, 0x5eed_0002] {
        let mut rng = OpRng(seed);
        let mut app = Is2dApp::default();
        for step in 0..1500 {
            let kind = if rng.range(10) < 3 { drawing[rng.range(drawing.len())].to_owned() } else { names[rng.range(names.len())].clone() };
            let action = random_action(&kind, &mut rng, &app);
            let label = format!("seed={seed:#x} step={step} op={}", action_kind_name(&action));
            used.insert(action_kind_name(&action));
            run_and_verify_scene_cache_consistency(&mut app, action, &label);
        }
    }
    let unused: Vec<&String> = names.iter().filter(|n| !used.contains(*n)).collect();
    assert!(unused.is_empty(), "試していない種類：{unused:?}");
}

/// カラーピッカーの四角の十字は、縁や角でも切らずに全部描く：黒（明るさ 0・鮮やかさ 0 で左下の角）でも十字が見える（利用者の
/// 指摘：インクボトルの線の色は最初が黒で、四角の中に切り取ると、縁の上に乗った腕が切れて十字がまったく見えなかった）
#[test]
fn the_color_picker_cross_is_whole_even_at_the_corner() {
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)));
    let mut color = Color32::BLACK;
    let out = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            crate::color_picker::color_picker(ui, &mut color);
        });
    });
    // 十字の線（黒の上なので白）が、どちらも切られずに描かれる（線の両端が、その形を切り取る範囲の中にある）
    let whole: Vec<_> = out
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::LineSegment { points, stroke } if stroke.color == Color32::WHITE => {
                Some(points.iter().all(|p| c.clip_rect.contains(*p)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(whole.len(), 2, "十字の線が2本描かれていない");
    assert!(whole.iter().all(|&w| w), "十字の線が四角の縁で切られる");
}

/// 拡大したときの曲線のなめらかさ（設計：claude/拡大したときの曲線のなめらかさの設計_41766f9.md）：画面に描くメッシュの段は、
/// 画面でのずれ（許容 × 倍率）が画素の 1/4 を超えないいちばん小さい段。6.25 倍までは図形が持つメッシュのまま
#[test]
fn the_mesh_level_keeps_the_screen_error_under_a_quarter_pixel() {
    use crate::vector_engine::{mesh_level, MAX_VIEW_ZOOM, MESH_TOL, MIN_VIEW_ZOOM};
    let quarter = crate::vector_stroke::FIT_TOL_PX;
    let mut zoom = MIN_VIEW_ZOOM;
    while zoom <= MAX_VIEW_ZOOM {
        let k = mesh_level(zoom);
        let tol = MESH_TOL * 0.5f64.powi(k as i32);
        assert!(tol * f64::from(zoom) <= quarter * (1.0 + 1e-12), "倍率 {zoom}：段 {k} では粗い");
        if k > 0 {
            assert!(tol * 2.0 * f64::from(zoom) > quarter, "倍率 {zoom}：段 {k} は細かすぎる");
        }
        zoom *= 1.01;
    }
    assert_eq!(mesh_level(1.0), 0);
    assert_eq!(mesh_level(6.25), 0);
    assert_eq!(mesh_level(8.0), 1);
    assert_eq!(mesh_level(16.0), 2);
}

/// 画面を描くと、先読みの係が、次に見るセル画の写しを裏で作って持つ（前は持たず、見る瞬間に作っていた）。描き変えたら、
/// 描く図形は描き変えたあとの中身（古い写しを使わない）
#[test]
fn the_app_prefetches_the_next_cel_and_never_shows_a_stale_one() {
    let mut app = app_with_cels_every_three_frames();
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 4);
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
    let _ = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
    let base_at = |app: &Is2dApp, f: usize| {
        let layer = &app.tab().active_timeline().layers[0];
        layer.keyframes[layer.keyframe_index_at(f).unwrap()].cel.maps()[0].base()
    };
    // 係は裏で作るので、持つまで待つ（待つ長さはテストの守り）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::sync::Arc::strong_count(&base_at(&app, 6)) < 2 {
        assert!(std::time::Instant::now() < deadline, "先読みの係が、次に見るセル画（コマ 6）の写しを持たない");
        std::thread::yield_now();
    }
    // 描き変えると、描く図形は新しい中身
    put_cel_shapes(&mut app, 0, 6, vec![map_overlay::rect_shape(200.0, 200.0, 260.0, 260.0, Color32::BLUE)]);
    let layer = &app.tab().active_timeline().layers[0];
    let shapes = layer.keyframes[layer.keyframe_index_at(6).unwrap()].cel.shapes();
    assert!(shapes.iter().any(|s| s.color() == Color32::BLUE), "描き変えたあとの中身が描かれない");
}

/// 中身を変えない操作は、取り消しの控えを積まない：キーフレームの上で F6 を押し、次のコマにもうキーフレームがあるとき
/// （次のコマへ進むだけ）と、中身の無いコマを動かしたとき（選択とコマの位置が動くだけ）。前は「変わった」と答えて控えを
/// 積んだので、取り消しても絵は何も変わらず、コマの位置が戻るだけだった（設計：claude/取り消しの約束を入口で守る設計_43c2564.md
/// の 2.1）。コマは進む・動く（後始末も走る）
#[test]
fn operations_that_change_nothing_leave_no_undo_step() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::timeline::TimelineCellSelection;
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED)]);
    // コマ 0 と 1 をキーフレームにする（コマ 0 で F6：次のコマへ進んで入れる）
    assert!(apply(&mut app, Action::SwitchLayerAndFrame { layer_idx: 0, frame: 0 }));
    assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: true }));
    assert_eq!(app.doc.root.current_frame, 1);
    assert!(app.doc.root.layers[0].keyframe_index_starting_at(1).is_some());
    // コマ 0 に戻って F6：次のコマ 1 はもうキーフレームなので、進むだけ
    apply(&mut app, Action::SwitchLayerAndFrame { layer_idx: 0, frame: 0 });
    let undo = app.history.undo_stack.len();
    let keyframes = app.doc.root.layers[0].keyframes.clone();
    apply(&mut app, Action::InsertKeyframe { copy_prev: true });
    assert_eq!(app.doc.root.current_frame, 1, "次のコマへ進む");
    assert!(app.doc.root.layers[0].keyframes.ptr_eq(&keyframes), "中身は変わらない");
    assert_eq!(app.history.undo_stack.len(), undo, "中身を変えない F6 で取り消しの控えを積んだ");
    // 中身の無いコマ（レイヤーのコマの外）を動かす：選択とコマの位置だけが動く
    let frames = app.doc.root.layers[0].frame_count;
    let undo = app.history.undo_stack.len();
    apply(&mut app, Action::MoveOrCopyFrames {
        sel: TimelineCellSelection::single(0, frames + 3),
        layer_delta: 0,
        frame_delta: -1,
        copy_mode: false,
    });
    assert!(app.doc.root.layers[0].keyframes.ptr_eq(&keyframes), "中身は変わらない");
    assert_eq!(app.history.undo_stack.len(), undo, "中身の無いコマを動かして取り消しの控えを積んだ");
}

/// キャンバスのダブルクリックは、2回のクリックが近い所のときだけ：A をクリックしてすぐ B をクリックしても、B のグループに
/// 入らず B を選ぶ（egui の判定は時間だけで決めるので、直す前は B に入った）。同じ所の2回はグループに入る。A・B のあとすぐ
/// B をもう1回クリックすると、B の2回でダブルクリックになる。選択ツールと自由変形ツールの両方
#[test]
fn double_click_needs_both_clicks_at_the_same_place() {
    use doc_actions::Action;
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 100, 220);
    for tool in [Tool::Select, Tool::Transform] {
        // クリックの列（ワールド座標の x, 前のクリックからの秒）ごとに、終わったあとに入っているグループの数と、選んでいるか。
        // 図形の真ん中（自由変形の基準点。ダブルクリックで基準点を戻す）は避ける
        for (clicks, want_depth, want_selected) in [
            (vec![(330.0, 0.0), (520.0, 0.2)], 0, true),
            (vec![(330.0, 0.0), (330.0, 0.2)], 1, false),
            (vec![(330.0, 0.0), (520.0, 0.2), (520.0, 0.1)], 1, false),
        ] {
            let mut app = Is2dApp::default();
            for (k, color) in [red, blue].into_iter().enumerate() {
                let x = 300.0 + 150.0 * k as f64;
                run(
                    &mut app,
                    &[
                        Action::PaintStroke {
                            points: vec![Point::new(x, 300.0), Point::new(x + 100.0, 300.0)],
                            radius: 20.0,
                            color,
                        },
                        Action::SelectSingleAt {
                            pt: Point::new(x + 50.0, 300.0),
                        },
                        Action::Group,
                        Action::Deselect,
                    ],
                );
            }
            run(&mut app, &[Action::SetTool { tool }]);
            let ctx = egui::Context::default();
            let screen_rect = Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1280.0, 820.0),
            ));
            let frame = |app: &mut Is2dApp, events: Vec<egui::Event>, t: f64| {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect,
                        time: Some(t),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.update_ui(ctx),
                );
            };
            let button = |p: Pos2, pressed: bool| egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let mut t = 1.0;
            frame(&mut app, vec![], t);
            for &(x, dt) in &clicks {
                t += dt;
                let p = app.view.world_to_screen(Point::new(x, 300.0));
                frame(&mut app, vec![egui::Event::PointerMoved(p)], t);
                frame(&mut app, vec![button(p, true)], t + 0.01);
                frame(&mut app, vec![button(p, false)], t + 0.05);
            }
            assert_eq!(
                (
                    app.session.editing_group_path().len(),
                    !app.session.selection().is_empty()
                ),
                (want_depth, want_selected),
                "{tool:?} {clicks:?}"
            );
        }
    }
}

/// 先読み（設計：claude/先読みの設計_31d78a6.md の 4.）：レイヤーごとに、今描いているコマの範囲のすぐ外の、次に見るセル画
/// （進む向き。止まっているときは戻る向きも）。くり返すときは、端の先は反対の端。隠したレイヤーは先読みしない
#[test]
fn prefetch_picks_the_next_cels_just_outside_what_is_drawn() {
    let mut app = app_with_cels_every_three_frames();
    let tl = app.tab().active_timeline();
    let layer = &tl.layers[0];
    let map_at = |f: usize| layer.keyframes[layer.keyframe_index_at(f).unwrap()].cel.maps()[0].clone();
    let picked = |first: usize, last: usize, both: bool, wrap: bool| -> Vec<usize> {
        let maps = crate::prefetch::wanted_maps(tl, first, last, both, wrap);
        maps.iter()
            .map(|m| [0usize, 3, 6, 9].into_iter().find(|&f| map_at(f).same(m)).expect("キーフレームの地図"))
            .collect()
    };
    // コマ 4（コマ 3 のセル画）：進む向きはコマ 6、戻る向きはコマ 0
    assert_eq!(picked(4, 4, false, false), vec![6]);
    assert_eq!(picked(4, 4, true, false), vec![6, 0]);
    // オニオンスキンの範囲 4..=7：その外のコマ 9 とコマ 0。範囲 2..=7 なら、コマ 0 のセル画はもう描いている（コマ 2 は
    // コマ 0 のセル画）ので、戻る向きは無い
    assert_eq!(picked(4, 7, true, false), vec![9, 0]);
    assert_eq!(picked(2, 7, true, false), vec![9]);
    // 端：くり返さなければ先は無い。くり返すなら反対の端
    assert_eq!(picked(10, 10, true, false), vec![6]);
    assert_eq!(picked(10, 10, false, true), vec![0]);
    assert_eq!(picked(1, 1, true, true), vec![3, 9]);
    app.tab_mut().with_active_timeline_mut(|tl, _| tl.layers[0].visible = false);
    assert!(crate::prefetch::wanted_maps(app.tab().active_timeline(), 4, 4, true, true).is_empty());
}

/// 先読みした写しは、見るときにそのまま使われ（描く図形を作り直さない）、作り直したもの（お手本）と同じ図形になる
#[test]
fn prefetched_cels_are_drawn_as_built_and_match_a_fresh_build() {
    let app = app_with_cels_every_three_frames();
    let tl = app.tab().active_timeline();
    let layer = &tl.layers[0];
    let map = layer.keyframes[layer.keyframe_index_at(6).unwrap()].cel.maps()[0].clone();
    let mesh_of = |shapes: &[std::sync::Arc<FillShape>]| -> Vec<Vec<(u32, u32)>> {
        shapes
            .iter()
            .map(|s| s.fill_mesh().vertices.iter().map(|v| (v.pos.x.to_bits(), v.pos.y.to_bits())).collect())
            .collect()
    };
    let held = crate::prefetch::prefetched(&map);
    let prefetched_shapes = held.drawn_shapes(&map.group_path);
    let seen = map.drawn_shapes();
    assert_eq!(seen.len(), prefetched_shapes.len());
    for (a, b) in seen.iter().zip(&prefetched_shapes) {
        assert!(std::sync::Arc::ptr_eq(a, b), "見るときに、先読みした図形を作り直した");
    }
    let prefetched_mesh = mesh_of(&prefetched_shapes);
    drop((held, prefetched_shapes, seen));
    // お手本：誰も写しを持たない所から作り直したもの
    assert_eq!(mesh_of(&map.drawn_shapes()), prefetched_mesh);
}

/// 1か所描き足しても、画面の土台は、変わらない図形の画面の写しを前と同じ物で使い回し、新しい図形だけを作る（地図の
/// 書き戻しは、変わらない図形を前の表と分け合う。前は、そのレイヤーの今のコマの図形を全部写して置き直したので、図形の
/// 数に比例して重くなった：レビュー de92ceb の N-6001。設計：claude/画面の図形の一覧を触った所だけで作る設計_f8b299c.md）
#[test]
fn drawing_reuses_the_screen_copies_of_unchanged_shapes() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 2 };
    let mut app = Is2dApp::default();
    let mut shapes = Vec::new();
    for i in 0..20 {
        for j in 0..20 {
            let (x, y) = (100.0 + i as f64 * 8.0, 100.0 + j as f64 * 8.0);
            shapes.push(map_overlay::rect_shape(x, y, x + 5.0, y + 5.0, Color32::RED));
        }
    }
    put_cel_shapes(&mut app, 0, 0, shapes);
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let before: Vec<std::sync::Arc<FillShape>> = app.session.base_cache().shapes().to_vec();
    // 離れた所に短い線を1本
    let pts: Vec<Point> = (0..=10).map(|t| Point::new(600.0 + t as f64 * 5.0, 600.0)).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: pts, stroke: ink }));
    let after = app.session.base_cache().shapes();
    let reused = after.iter().filter(|a| before.iter().any(|b| std::sync::Arc::ptr_eq(a, b))).count();
    assert_eq!(before.len(), 400);
    assert_eq!(reused, 400, "変わらない四角の画面の写しを作り直した");
    assert!(after.len() > before.len(), "描き足した線が画面に出ていない");
}

/// 画面の上乗せの重なり順は、地図の描く順と同じ1つの決まり（`planar_map::draw_rank`：同じ地図から来た図形は塗りのあとに
/// 線、組と組は下から順）で決まる。足した順によらない（レビュー de92ceb の N-3001：前は、生の図形の置き場 `RawOf` だけが
/// 線を特別に扱い、そのすぐ上の置き場 `AboveRaw` は足した順に並べたので、足す順しだいで線が塗りに隠れた）
#[test]
fn overlay_order_follows_the_map_drawing_order() {
    use crate::canvas_tool::{build_scene_order_by_layer_ids, Overlay, Placement, SceneEntry};
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 4 };
    let shape = |id: u64, line: bool| {
        let mut s = map_overlay::rect_shape(0.0, 0.0, 10.0, 10.0, Color32::RED);
        s.shape_id = id;
        s.layer_id = 1;
        s.group_path = Vec::new();
        if line {
            s.line = Some(ink);
        }
        s
    };
    // 土台：その階層の生の図形の地図（塗り 1・2、線 3）
    let base: Vec<std::sync::Arc<FillShape>> =
        [shape(1, false), shape(2, false), shape(3, true)].into_iter().map(std::sync::Arc::new).collect();
    let scope: Vec<u64> = Vec::new();
    let mut overlay = Overlay::default();
    // わざと線を先に足す：地図の中のもの（線 11・塗り 12）、片の組 1（線 21・塗り 22）、片の組 0（線 31・塗り 32）
    for (at, id, line) in [
        (Placement::RawOf(1, scope.clone()), 11, true),
        (Placement::RawOf(1, scope.clone()), 12, false),
        (Placement::AboveRaw(1, scope.clone(), 1), 21, true),
        (Placement::AboveRaw(1, scope.clone(), 1), 22, false),
        (Placement::AboveRaw(1, scope.clone(), 0), 31, true),
        (Placement::AboveRaw(1, scope.clone(), 0), 32, false),
    ] {
        overlay.add.push((at, std::sync::Arc::new(shape(id, line))));
    }
    let ids: Vec<u64> = build_scene_order_by_layer_ids(&base, &overlay, &[1])
        .into_iter()
        .map(|e| match e {
            SceneEntry::Base(i) => base[i].shape_id,
            SceneEntry::Overlay(i) => overlay.add[i].1.shape_id,
        })
        .collect();
    // 地図の塗り（1・2・12）→ 地図の線（3・11）→ 組 0 の塗り・線（32・31）→ 組 1 の塗り・線（22・21）
    assert_eq!(ids, vec![1, 2, 12, 3, 11, 32, 31, 22, 21]);
}

/// 輪郭の表示（ワイヤーフレーム）は、同じ辺・同じ頂点を1回だけ描く：線で囲んだ四角の塗り（塗りの輪郭の4辺と、それに
/// 乗った線4本は同じ辺）で、輪郭4本・アンカー4個（レビュー 41766f9 の N-3001：前は図形ごとに描いたので、輪郭8本・
/// アンカー12個を描いた）。ドラッグで動かしている最中も、輪郭とアンカーは画面の図形（上乗せ）についていく
#[test]
fn the_wireframe_draws_each_edge_and_anchor_once() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::EdgeStroke;
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(200.0, 200.0, 400.0, 400.0, Color32::from_rgb(120, 175, 160))]);
    assert!(apply(&mut app, Action::InkBottle { pt: Point::new(300.0, 300.0), stroke: EdgeStroke { color: Color32::BLACK, width: 6 } }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    app.tab_mut().view.show_wireframe = true;
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
    let wire = Color32::from_rgba_unmultiplied(30, 80, 180, 140);
    let drawn = |app: &mut Is2dApp| {
        let out = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
        let mut paths: Vec<Vec<egui::Pos2>> = Vec::new();
        let mut dots: Vec<egui::Pos2> = Vec::new();
        for c in &out.shapes {
            match &c.shape {
                egui::Shape::Path(p) if p.stroke.color == egui::epaint::ColorMode::Solid(wire) => paths.push(p.points.clone()),
                egui::Shape::LineSegment { points, stroke } if stroke.color == wire => paths.push(points.to_vec()),
                egui::Shape::Circle(cs) if cs.fill == Color32::WHITE && cs.radius == 3.0 => dots.push(cs.center),
                _ => {}
            }
        }
        (paths, dots)
    };
    let (paths, dots) = drawn(&mut app);
    assert_eq!(paths.len(), 4, "輪郭の本数");
    assert_eq!(dots.len(), 4, "アンカーの数");
    // 全部を選んで動かし始める：輪郭とアンカーも、動かした所に描かれる
    assert!(apply(&mut app, Action::SelectAll));
    let drag = crate::canvas_tool::clipboard_ops::begin_move_selection_drag(&mut app, Point::new(300.0, 300.0), false);
    app.tab_mut().session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(drag);
    app.tab_mut().session.translate_move_overlay(KVec2::new(100.0, 0.0));
    let (moved_paths, moved_dots) = drawn(&mut app);
    let shift = 100.0 * app.view.zoom();
    let min_x = |v: &[egui::Pos2]| v.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    assert_eq!(moved_dots.len(), 4);
    assert!((min_x(&moved_dots) - (min_x(&dots) + shift)).abs() < 0.01, "アンカーがドラッグについてこない");
    let all = |v: &[Vec<egui::Pos2>]| v.iter().flatten().copied().collect::<Vec<_>>();
    assert!((min_x(&all(&moved_paths)) - (min_x(&all(&paths)) + shift)).abs() < 0.01, "輪郭がドラッグについてこない");
}

/// 選択削除のキーは Backspace・Delete・G（利用者が決めた）。入力欄に文字を入れている最中の G は、入力欄の文字（絵を消さない）
#[test]
fn g_deletes_the_selection_unless_typing() {
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let none = egui::Modifiers::NONE;
    put_cel_shapes(&mut app, 0, 0, make_stroke(Point::new(100.0, 100.0), Point::new(180.0, 100.0), 12.0, Color32::RED));
    app.set_selected_indices(vec![0]);
    let focus = |ctx: &egui::Context, _: &mut Is2dApp| ctx.memory_mut(|m| m.request_focus(egui::Id::new("入力欄")));
    menu_test_frame(&ctx, &mut app, none, menu_key_tap(egui::Key::G, none), focus);
    assert_eq!(app.scene().len(), 1, "入力中の G で絵が消えた");
    let unfocus = |ctx: &egui::Context, _: &mut Is2dApp| ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("入力欄")));
    menu_test_frame(&ctx, &mut app, none, Vec::new(), unfocus);
    menu_test_frame(&ctx, &mut app, none, menu_key_tap(egui::Key::G, none), |_, _| {});
    assert_eq!(app.scene().len(), 0, "G で選択削除にならない");
}

#[test]
fn test_g3_2_end_coalesce_via_apply_preserves_playback_and_coalesce_boundaries() {
    use crate::canvas_tool::CoalesceKey;
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    let sym_id = app.doc.library().at(0).unwrap().id;
    let _ = apply(&mut app, Action::EnterSymbolFromLibrary { sym_id });
    let _ = apply(&mut app, Action::ExtendFrame);
    let _ = apply(&mut app, Action::ExtendFrame);
    let _ = apply(&mut app, Action::ExitGroupSteps { steps: 1 });
    let gid = app.doc.root.layers[0].keyframes[0].cel.instances()[0]
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

/// 画面の仕事が止まっても、コマの終わりの仕事（控えを復旧の係へ渡す）は走る（画面の1コマ `Is2dApp::frame` は2つの仕事を
/// それぞれ砦の中で走らせる。前は1つの仕事だったので、描く所で止まり続けると控えが渡らなかった：設計審査 6fb4faa の D-3）
#[test]
fn the_end_of_frame_work_runs_even_when_the_screen_work_stops() {
    use crate::recovery::RecoveryWriter;
    let rec_dir = std::env::temp_dir().join(format!("is2d_frame_end_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&rec_dir);
    let mut app = Is2dApp { recovery_writer: Some(RecoveryWriter::start(rec_dir.clone())), ..Default::default() };
    // 作品を変えてから、画面の仕事が止まる
    assert!(crate::canvas_tool::doc_actions::apply(
        &mut app,
        crate::canvas_tool::doc_actions::Action::PaintStroke { points: vec![Point::new(100.0, 100.0), Point::new(200.0, 120.0)], radius: 8.0, color: Color32::RED },
    ));
    app.run_frame(|_| panic!("計算の前提が破れた"));
    app.run_frame(Is2dApp::finish_frame);
    // 控えが渡り、復旧ファイルができる（裏の書き手が書き終えるのを待つ）
    let start = std::time::Instant::now();
    let journals = || {
        std::fs::read_dir(&rec_dir).map_or(0, |d| d.flatten().filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("is2dj")).count())
    };
    while journals() == 0 {
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "画面の仕事が止まったあと、控えが復旧の係へ渡らなかった");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.recovery_writer.take().unwrap().finish(true);
    let _ = std::fs::remove_dir_all(&rec_dir);
}

/// ★1（レビュー「軽量化・高速化」）：ロック中のレイヤーを描き直すとき、セル画が変わらなければ、しまう地図から計算に
/// 使う写しを作り直さない（セル画の結合メッシュの写しを持つ：`CelMeshCache`）。再生中も同じ
#[test]
fn redrawing_a_locked_layer_does_not_rebuild_its_map() {
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(300.0, 160.0),
            10.0,
            Color32::from_rgb(220, 60, 60),
        ),
    );
    app.tab_mut().doc.root.layers[0].locked = true;
    app.tab_mut().refresh_scene_base();
    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
    let before = crate::planar_map::dense_builds();
    for _ in 0..3 {
        let _ = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
    }
    assert_eq!(crate::planar_map::dense_builds(), before);
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
    app.set_selected_indices(vec![0]);
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
    doc_actions::switch_to_layer_and_frame(&mut loaded_app, 0, 0);
    let summary = scene_summary(&loaded_app);
    let sym_indices: Vec<usize> = (0..summary.len)
        .filter(|&i| summary.shapes[i].symbol_instance.is_some())
        .collect();
    loaded_app.set_selected_indices(sym_indices);
    loaded_app.session.transform_pivot = Some(Point::new(111.0, 222.0));
    assert!(!loaded_app.session.selection().is_empty());
    assert!(doc_actions::copy_selected_shapes(&mut loaded_app));

    // Cmd+S（save_project_to_path）を実行しても現在の選択状態とカスタムピボットが強制解除されないこと
    crate::project_io::save_project_to_path(&mut loaded_app, &tmp_path)
        .expect("選択維持のまま上書き保存に成功すること");
    assert!(!loaded_app.session.selection().is_empty());
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
    assert!(loaded_app.doc.library().is_empty());
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
        loaded_app.doc.library().len(),
        1,
        "別タブからペーストしたシンボル定義がターゲットタブのライブラリに自動移植されること"
    );
    // Undo すれば移植されたシンボルごと元の未変更状態（!is_dirty）に戻ること
    loaded_app.undo();
    assert!(loaded_app.doc.library().is_empty());
    assert!(!loaded_app.history.is_dirty());

    // Undo 後に再度 Tab 1 でペーストした際、消去されたシンボル定義が再移植され図形が消失しないこと（指摘1-A）
    assert!(doc_actions::paste_clipboard(&mut loaded_app, true));
    assert_eq!(loaded_app.doc.library().len(), 1);
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
        loaded_app.doc.library().len(),
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

    // 再度閉じて「保存せずに閉じる」を選ぶとタブ0が閉じられること
    project_io::close_tab(&mut app, 0, Some(&ctx));
    project_io::confirm_pending_close_discard(&mut app, &ctx);
    assert!(app.pending_close_confirm.is_none());
    assert_eq!(app.tabs.len(), 1);
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
    let canvas_center = app.canvas_rect.center();
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
    // 線の左端の頂点を、ほかの辺から離れた外側（左上）へ動かす
    let v0_world = scene_summary(&app).shapes[0]
        .exterior()
        .segments
        .iter()
        .map(|q| q.p0)
        .min_by(|a, b| a.x.total_cmp(&b.x))
        .unwrap();
    let v0_screen = app.view.world_to_screen(v0_world);
    let v0_drag_to = v0_screen + Vec2::new(-25.0, -30.0);
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
    let expected_moved_v0 = Point::new(v0_world.x - 25.0, v0_world.y - 30.0);
    assert!(
        scene_summary(&app).shapes[0]
            .exterior()
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
    app.set_selected_indices(vec![0]);
    let bbox_before = app
        .scene()
        .selection_bounding_box(app.session.selection(), app.view.zoom())
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
        .selection_bounding_box(app.session.selection(), app.view.zoom())
        .unwrap();
    assert!(
        bbox_scaled.width() > bbox_before.width() + 40.0,
        "自由変形ツールの右下スケールハンドルドラッグで拡大されること: before={bbox_before:?}, after={bbox_scaled:?}"
    );

    // 四隅外側の回転ゾーンからのドラッグで回転されること
    let rot_start_world = Point::new(bbox_scaled.x1 + 10.0, bbox_scaled.y0 - 10.0);
    let rot_start_scr = app.view.world_to_screen(rot_start_world);
    let rot_drag_scr = rot_start_scr + Vec2::new(0.0, 80.0);
    let p0_before_rot = scene_summary(&app).shapes[0].exterior().segments[0].p0;
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
    let p0_after_rot = app.scene().get(0).unwrap().exterior().segments[0].p0;
    assert!(
        (p0_after_rot - p0_before_rot).hypot() > 5.0,
        "自由変形ツールの四隅外側ドラッグで図形が回転されること"
    );

    // マウスホイールによるカーソル中心ズームの RawInput 検証
    let zoom_before = app.view.zoom();
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
        app.view.zoom() > zoom_before,
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

/// 知らせは並びで持ち、あとの知らせが前の知らせを消さない（同じ瞬間の「保存すると元のファイルからも消えます」と「音を
/// 読めなかった」が両方出る）。ファイルから読んだものの知らせは、出ている間に別のコマで見つかった分も「ほか N か所」に
/// まとまる（設計審査 b7208d9 の 4.。前は箱が1枠で上書きし、まとめも1コマの中だけだった）
#[test]
fn notices_stack_and_load_notices_add_up_while_shown() {
    use crate::canvas_tool::doc_actions::ActionHost;
    let mut app = Is2dApp::default();
    app.notify_warning("一つ目".to_owned());
    app.notify_warning("二つ目".to_owned());
    app.notify_warning("一つ目".to_owned());
    let shown: Vec<&str> = app.notices.iter().map(|t| t.message.as_str()).collect();
    assert_eq!(shown, vec!["二つ目", "一つ目"], "あとの知らせが前の知らせを消した・同じ文が2つ出た");
    // 裏の確かめが、別のコマで1つずつ見つける
    let ctx = egui::Context::default();
    let frame = |app: &mut Is2dApp| {
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
        let _ = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.frame(ctx));
    };
    app.tab().doc().load_notices.push(crate::timeline::LoadNotice::Blank("コマ 3".to_owned()));
    frame(&mut app);
    app.tab().doc().load_notices.push(crate::timeline::LoadNotice::Blank("コマ 9".to_owned()));
    frame(&mut app);
    let load: Vec<&str> = app.notices.iter().map(|t| t.message.as_str()).filter(|m| m.contains("何も無いもの")).collect();
    assert_eq!(load.len(), 1, "読み込みの知らせが2つに分かれた：{load:?}");
    assert!(load[0].starts_with("コマ 3 ほか 1 か所"), "別のコマで見つかった分がまとまらない：{load:?}");
    assert!(app.notices.iter().any(|t| t.message == "二つ目"), "読み込みの知らせが前の知らせを消した");
}

#[test]
fn test_zoom_around_cursor_preserves_world_point_and_clamps_bounds() {
    let mut app = Is2dApp::default();
    let cursor = Pos2::new(320.0, 240.0);

    let world_before = app.view.screen_to_world(cursor);
    app.view.zoom_around_screen_pos(cursor, 4.0);
    app.view.zoom_around_screen_pos(cursor, 2.0);
    assert!((app.view.zoom() - 8.0).abs() < 1e-5);
    let world_after = app.view.screen_to_world(cursor);
    assert!(
        (world_before - world_after).hypot() < 1e-4,
        "カーソル中心ズームの前後でカーソル直下のワールド座標が一致すること"
    );
    let screen_roundtrip = app.view.world_to_screen(world_after);
    assert!((screen_roundtrip - cursor).length() < 1e-3);

    // 上限 (MAX_VIEW_ZOOM = 16.0) と下限 (MIN_VIEW_ZOOM = 0.15) でクランプされ、かつカーソル下の座標が保たれること
    app.view.zoom_around_screen_pos(cursor, 100.0);
    assert_eq!(app.view.zoom(), crate::vector_engine::MAX_VIEW_ZOOM);
    assert!((app.view.screen_to_world(cursor) - world_before).hypot() < 1e-3);

    app.view.zoom_around_screen_pos(cursor, 0.001);
    assert_eq!(app.view.zoom(), crate::vector_engine::MIN_VIEW_ZOOM);
    assert!((app.view.screen_to_world(cursor) - world_before).hypot() < 1e-3);

    app.view.reset_view();
    assert_eq!(app.view.zoom(), 1.0);
    assert_eq!(app.view.pan, Vec2::ZERO);
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

/// 同時に計算する `parallel_map` は、`items` の順に結果を返し、CPU が2つ以上なら2つ以上のスレッドで計算する
#[test]
fn test_parallel_map_keeps_order_and_uses_threads() {
    let items: Vec<usize> = (0..64).collect();
    let out = crate::planar_map::parallel_map(&items, |&i| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        (i * 2, std::thread::current().id())
    });
    assert_eq!(
        out.iter().map(|o| o.0).collect::<Vec<_>>(),
        (0..128).step_by(2).collect::<Vec<_>>()
    );
    let threads: std::collections::HashSet<_> = out.iter().map(|o| o.1).collect();
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert_eq!(
        threads.len() > 1,
        cpus > 1,
        "スレッド {} 本・CPU {cpus} 個",
        threads.len()
    );
    assert!(threads.len() <= cpus);
}

#[test]
fn test_egui_shortcuts_clipboard_sync_and_cmd0_preserves_ui_zoom_factor() {
    use eframe::egui;

    let ctx = egui::Context::default();
    crate::configure_sharp_light_theme(&ctx);
    let mut app = Is2dApp::default();
    // 画面の座標を文書の座標のまま使う（表示は 100%・ずらしなし。開いたときの窓への合わせはしない）
    app.view.fit_pending = false;
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
    app.set_selected_indices(vec![0]);

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
        !out_copy
            .platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::CopyText(_))),
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

    // その Cmd+V の V を離しても、もう一度は貼らないこと（押下は Event::Paste として来ている）
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
    assert_eq!(app.scene().len(), 2, "V を離しても二重に貼らない");

    // OS クリップボードが空で egui-winit が押下時の Event::Paste / Key(V, pressed:true) を捨てた場合でも、
    // 押下の合図が来ないまま V を離したら（捨てられた Cmd+V）ペーストされること (#15)
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

    // V より先に Cmd を離した場合でも、押下の合図が来ないまま V を離したらペーストされること (#15)
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
    assert_eq!(app.doc.library().len(), 1);

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
        app.doc.library().len(),
        1,
        "F8 のキーリピート (repeat: true) ではシンボル化が重複暴発しないこと"
    );
    assert_eq!(app.tool.tool, Tool::Transform);

    // 3. Cmd+Plus（US 配列の `=`／`+` のキー）でキャンバスのみがズームインし egui 全体の UI zoom_factor は 1.0 のまま維持され、Cmd+0 で viewport.zoom が 1.0 にリセットされること (#1)
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
                physical_key: Some(egui::Key::Equals),
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
        (app.view.zoom() - 1.25).abs() < 1e-6,
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
        (app.view.zoom() - 1.0).abs() < 1e-6 && app.view.pan == Vec2::ZERO,
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
    assert!(app.session.selection().is_empty());
    assert_eq!(
        app.tool.tool,
        Tool::Transform,
        "Esc で選択解除しても現在のツール (Tool::Transform) が勝手に Tool::Select へ切り替わらないこと"
    );
}

/// `+`／`-` のキーは、キーの位置だけで決まる（レビュー 96fdc26 の H4）。配列や日本語入力で論理キーが
/// 何になっても（来なくても）、位置が同じなら同じ答え。US・JIS の `+` の刻印の位置はどちらも `+`
#[test]
fn plus_minus_keys_are_decided_by_key_position() {
    use crate::panels::{UiAction, ZoomStep};
    use eframe::egui::Key;
    // `Cmd` と論理キー `key`・位置 `at` のキーを押したとき、表（`menu::COMMANDS`）で決まるズーム
    let zoom = |key: Key, at: Option<Key>, shift: bool| {
        let pressed = menu::Pressed { key, position: at, command: true, shift };
        menu::command_for_key(pressed, false).and_then(|i| match menu::COMMANDS[i].action {
            UiAction::Zoom(z) => Some(z),
            _ => None,
        })
    };
    let (plus, minus) = (Some(ZoomStep::In), Some(ZoomStep::Out));
    // US の `=`／`+` のキー（Shift の有無で論理キーが変わっても同じ）
    assert_eq!(zoom(Key::Equals, Some(Key::Equals), false), plus);
    assert_eq!(zoom(Key::Plus, Some(Key::Equals), true), plus);
    // JIS の `;`／`+` のキー（論理キーは `;`・`+`）
    assert_eq!(zoom(Key::Semicolon, Some(Key::Semicolon), false), plus);
    assert_eq!(zoom(Key::Plus, Some(Key::Semicolon), true), plus);
    // テンキーの `+`（テンキーの `-` は `-` のキーと同じ位置の名前で来る）
    assert_eq!(zoom(Key::Plus, Some(Key::Plus), false), plus);
    // `-` のキー（日本語入力で論理キーが来ないときは、egui-winit が位置の名前を論理キーにする）
    assert_eq!(zoom(Key::Minus, Some(Key::Minus), false), minus);
    // 論理キーが `+`・`-` でも、位置が違えば `+`／`-` ではない（配列で刻印が動いても位置で決める）
    assert_eq!(zoom(Key::Plus, Some(Key::Num1), false), None);
    assert_eq!(zoom(Key::Minus, Some(Key::Quote), false), None);
    assert_eq!(zoom(Key::Minus, None, false), None);
    // 押しっぱなしの繰り返しも数える（`Cmd` と合わせたズームを続ける）
    let out = menu::command_for_key(menu::Pressed { key: Key::Minus, position: Some(Key::Minus), command: true, shift: false }, false).unwrap();
    assert!(menu::COMMANDS[out].repeats);
}

/// ボタンの吹き出しのキーは、キーの決まりから出す（キーを変えたとき、吹き出しだけ古いキーのまま残っていた：鉛筆 (Y)・
/// インクボトル (S)・選択削除に G が無い。説明書の確かめで見つかった）
#[test]
fn tooltips_show_the_current_keys() {
    assert!(crate::panels::tool_tooltip(Tool::Pencil).contains("(S)"));
    assert!(crate::panels::tool_tooltip(Tool::InkBottle).contains("(O)"));
    let delete = crate::menu::keys_text(|a| matches!(a, crate::panels::UiAction::Doc(crate::canvas_tool::doc_actions::Action::DeleteSelection)));
    assert_eq!(delete, "Backspace / Delete / G");
}

/// `Cmd` のショートカット（コピーなど）のすぐ後に、ふつうに `V`（選択ツール）を押して離しても貼り付けない。直す前は、
/// `Cmd` を離してから4コマの猶予のうちに `V` を離すと貼り付けた（レビュー f01d518 の N-5003）
#[test]
fn pressing_v_right_after_a_command_shortcut_does_not_paste() {
    use eframe::egui;
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
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
    app.set_selected_indices(vec![0]);
    let cmd = egui::Modifiers {
        command: true,
        ctrl: true,
        ..Default::default()
    };
    let key = |key: egui::Key, pressed: bool, modifiers: egui::Modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    };
    let frame = |app: &mut Is2dApp, modifiers: egui::Modifiers, events: Vec<egui::Event>| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(1280.0, 820.0),
                )),
                modifiers,
                events,
                ..Default::default()
            },
            |ctx| app.update_ui(ctx),
        );
    };
    // Cmd+C（コピー）→ C を離す → Cmd を離す → V を押す → V を離す
    frame(&mut app, cmd, vec![egui::Event::Copy]);
    assert!(app.clipboard.content.is_some());
    let undo_len = app.history.undo_stack.len();
    frame(&mut app, cmd, vec![key(egui::Key::C, false, cmd)]);
    let none = egui::Modifiers::NONE;
    frame(&mut app, none, vec![]);
    frame(&mut app, none, vec![key(egui::Key::V, true, none)]);
    assert_eq!(app.tool.tool, Tool::Select, "V で選択ツール");
    frame(&mut app, none, vec![key(egui::Key::V, false, none)]);
    assert_eq!(app.clipboard.paste_count, 0, "貼り付けない");
    assert_eq!(app.history.undo_stack.len(), undo_len);
    assert_eq!(app.scene().len(), 1);
}

#[test]
fn test_key_repeat_suppressed_for_enter_anchors_tween_and_cmd3() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    // 1. A キー初回押下フレーム (repeat: false) でアンカーの表示が ON になり、別フレームでの長押しリピート (repeat: true) では OFF に戻らないこと (#16。
    //    前はオニオンスキンの O で確かめていたが、オニオンスキンのキーは無くした)
    assert!(!app.view.show_wireframe);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::A,
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
    assert!(app.view.show_wireframe);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect,
            events: vec![egui::Event::Key {
                key: egui::Key::A,
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
        app.view.show_wireframe,
        "別フレームの A キー長押しリピートでアンカーの表示が切り替わり続けないこと (#16)"
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
    app.set_selected_indices(vec![0]);
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
    extend_frame_at(&mut app.doc.root, 0, 3);
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

/// 作品をタブに入れる道（最後のタブを閉じる・まっさらなタブに開く・新規）のどれでも、タブの番号（「どの作品か」の見分け）
/// は新しくなり、前の作品でコピーしたシンボルを、入れ直した作品の同じ番号の別のシンボルと取り違えない（番号を替えるのは、
/// 作品を入れる道 `install_document` の1か所：設計審査 392e9af の D-4）
#[test]
fn installing_a_work_always_gives_the_tab_a_new_identity() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let ctx = egui::Context::default();
    let symbol_of = |app: &mut Is2dApp, color: Color32| -> u64 {
        assert!(apply(app, Action::PaintStroke { points: vec![Point::new(100.0, 100.0), Point::new(200.0, 120.0)], radius: 8.0, color }));
        assert!(apply(app, Action::SelectAll));
        assert!(apply(app, Action::ConvertToSymbol));
        symbols_in_order(app.tab().doc().library()).last().unwrap().id
    };
    // 赤い絵のシンボルをコピーする
    let mut app = Is2dApp::default();
    let red = symbol_of(&mut app, Color32::RED);
    assert!(apply(&mut app, Action::Copy));
    // 最後のタブを閉じる（同じタブに白紙の作品が入る）：番号が新しくなる
    let before = app.tab().id;
    crate::project_io::close_tab(&mut app, 0, None);
    assert_ne!(app.tab().id, before, "最後のタブを閉じて作品を入れても、番号が同じ");
    // 入れ直した作品で、同じ番号の青いシンボルを作ってから貼る：赤いシンボルを移して貼る（青を使い回さない）
    let blue = symbol_of(&mut app, Color32::BLUE);
    assert_eq!(blue, red, "この確かめには、同じ番号の別のシンボルが要る");
    assert!(canvas_tool::clear_selection(&mut app));
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    let pasted = app.tab().session.selection().instances.iter().next().copied().expect("貼ったインスタンス");
    let cel = app.tab().active_timeline().layers[0].cel_at(0).unwrap();
    let sym = cel.instances().iter().find(|i| i.binding.instance_gid == pasted).unwrap().binding.symbol_id;
    assert_ne!(sym, blue, "前の作品でコピーしたシンボルを、入れ直した作品の同じ番号の別のシンボルと取り違えた");
    // まっさらなタブに開く・新規：番号が新しくなる
    let mut app = Is2dApp::default();
    let before = app.tab().id;
    crate::project_io::open_document_in_new_tab(&mut app, symbol::Document::default(), None).unwrap();
    assert_ne!(app.tab().id, before, "まっさらなタブに開いても、番号が同じ");
    let before = app.tab().id;
    crate::project_io::new_project(&mut app, &ctx);
    assert_ne!(app.tab().id, before, "新規のタブの番号が前のタブと同じ");
}

/// 最後の砦の受け止め（`guarded`）：途中で止まる計算は `None`、止まらない計算はその答え。スレッドを始める1つの関数
/// （`spawn_guarded`）も、止まったスレッドは `None` で終わる（設計 claude/最後の砦を仕事の入口に置く設計.md）
#[test]
fn guarded_returns_none_only_when_the_work_stops() {
    assert_eq!(crate::guard::guarded(|| 1 + 1), Some(2));
    let stopped: Option<u32> = crate::guard::guarded(|| panic!("計算の前提が破れた"));
    assert_eq!(stopped, None);
    let ok = crate::guard::spawn_guarded("is2d-test", || 3).expect("スレッドを作れる");
    assert_eq!(ok.join().ok(), Some(Some(3)));
    let mut app = Is2dApp::default();
    let bad = crate::guard::spawn_guarded("is2d-test-stop", || -> u32 { panic!("計算の前提が破れた") }).expect("スレッドを作れる");
    assert_eq!(bad.join().ok(), Some(None), "止まったスレッドが砦の外へ漏れた");
    assert!(app.stopped_threads.take_new().contains(&"is2d-test-stop".to_owned()), "止まったスレッドを知らせない");
}

/// 画面の1コマの途中で止まっても、アプリは落ちず、そのコマを無かったことにする：キャンバスとタイムラインのドラッグは取り
/// 消され、作品と取り消しの控えはそのコマの前のまま、知らせが出て、次のコマから続けて使える（画面の仕事の入口は
/// `Is2dApp::run_frame` の1つ：設計 claude/最後の砦を仕事の入口に置く設計.md。前は止まると、復旧ファイルを書いてから
/// アプリごと落ちた）
#[test]
fn a_panic_inside_a_frame_cancels_the_drags_and_keeps_the_work() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    // 辺をつまんで動かしている途中（ドラッグ中の仮表示がある）、タイムラインでもレイヤーをつかんでいる
    crate::tool_select::start_reshape_drag(&mut app, Point::new(200.0, 100.0), false, false);
    crate::tool_select::update_reshape_drag(&mut app, Point::new(200.0, 60.0));
    assert!(!matches!(app.session.drag, crate::canvas_tool::ActiveCanvasDrag::None), "ドラッグが始まっていない");
    app.timeline_ui.dragging_layer = Some(0);
    let doc_before = app.doc.clone();
    let undo_before = app.history.undo_stack.len();
    app.notices = Default::default();
    // 1コマの仕事の途中で止まる
    app.run_frame(|app| {
        crate::tool_select::update_reshape_drag(app, Point::new(210.0, 50.0));
        panic!("計算の前提が破れた");
    });
    assert!(matches!(app.session.drag, crate::canvas_tool::ActiveCanvasDrag::None), "止まったコマのドラッグが残った");
    assert_eq!(app.timeline_ui.dragging_layer, None, "止まったコマのタイムラインのドラッグが残った");
    assert_eq!(app.notices.iter().last().map(|t| t.message.as_str()), Some("内部の不具合で画面の処理を続けられませんでした（それまでの作業は残っています）"));
    assert!(
        app.doc.root.layers.iter().zip(doc_before.root.layers.iter()).all(|(a, b)| a.keyframes.iter().zip(b.keyframes.iter()).all(|(x, y)| std::sync::Arc::ptr_eq(x.cel.maps(), y.cel.maps()))),
        "作品がコマの前と違う"
    );
    assert_eq!(app.history.undo_stack.len(), undo_before, "取り消しの控えが変わった");
    assert!(app.scene_base_is_fresh(), "画面の土台を作り直していない");
    // 次のコマから続けて使える：もう一度つまんで動かして離すと、絵が変わり、取り消すと戻る
    app.run_frame(|app| {
        crate::tool_select::start_reshape_drag(app, Point::new(200.0, 100.0), false, false);
        crate::tool_select::update_reshape_drag(app, Point::new(200.0, 60.0));
        crate::tool_select::finish_reshape_drag(app, Some(Point::new(200.0, 60.0)));
    });
    assert_eq!(app.history.undo_stack.len(), undo_before + 1, "続けて使えない");
    assert!(apply(&mut app, Action::Undo));
}

/// 操作の途中（操作の前の写し `UndoHistory::before` がある）で画面の1コマが止まったら、作品を操作の前に戻す（戻し方は
/// `recover_from_panic` の1つ：取り消しと同じ戻し方。設計 claude/最後の砦を仕事の入口に置く設計.md）。操作の途中の状態は、
/// 入口 `apply` が作るのと同じ形（操作の前の写しを持ち、作品を書きかけた）にする
#[test]
fn a_panic_in_the_middle_of_an_operation_puts_the_work_back() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    let doc_before = app.doc.clone();
    // 操作の前の写し：ブラシ1本の取り消しの控え（ブラシを引く前の作品）
    assert!(apply(&mut app, Action::PaintStroke { points: vec![Point::new(400.0, 400.0), Point::new(450.0, 420.0)], radius: 10.0, color: Color32::BLUE }));
    let before = app.history.undo_stack.back().cloned().expect("ブラシの控えがある");
    let undo_len = app.history.undo_stack.len();
    app.notices = Default::default();
    app.run_frame(|app| {
        // 操作の途中：操作の前の写しを持ち、作品を書きかけたところで止まる
        app.history.before = Some(before);
        put_cel_shapes(app, 0, 0, vec![map_overlay::rect_shape(500.0, 500.0, 600.0, 600.0, Color32::GREEN)]);
        panic!("計算の前提が破れた");
    });
    assert!(app.history.before.is_none(), "操作の前の写しが残った");
    assert!(
        app.doc.root.layers.iter().zip(doc_before.root.layers.iter()).all(|(a, b)| a.keyframes.len() == b.keyframes.len() && a.keyframes.iter().zip(b.keyframes.iter()).all(|(x, y)| std::sync::Arc::ptr_eq(x.cel.maps(), y.cel.maps()))),
        "作品が操作の前に戻っていない"
    );
    assert_eq!(app.history.undo_stack.len(), undo_len, "取り消しの控えが変わった");
    assert_eq!(app.notices.iter().last().map(|t| t.message.as_str()), Some("内部の不具合でこの操作を続けられなかったので、操作の前に戻しました"));
    assert!(app.scene_base_is_fresh(), "画面の土台を作り直していない");
}

/// 本番のソースのファイル（`src/` の下の `.rs`。テストのファイルの置き場 `canvas_tool/tests/` を除く）
fn production_sources() -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("ソースを読める") {
            let path = entry.expect("ソースを読める").path();
            if path.is_dir() {
                // テストのファイルの置き場（`canvas_tool/tests/`）は読まない
                if !path.ends_with("canvas_tool/tests") {
                    dirs.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// OS・部品から呼ばれる受け口の名簿（受け口はどれも、中身を砦の中で走らせる：設計 claude/最後の砦を仕事の入口に置く設計.md）。
/// 新しい受け口を作ると、ここが落ちるので、砦を置いて名簿に足す
#[test]
fn every_entry_called_from_outside_is_on_the_list() {
    const PATTERNS: [&str; 7] = [
        "extern \"system\" fn",
        "extern \"C\" fn",
        "#[unsafe(method(",
        "RcBlock::new(",
        "StackBlock::new(",
        "build_output_stream(",
        "build_input_stream(",
    ];
    // （ファイル, 受け口の数）：Windows の窓の受け口・Mac のメニューの受け口・書き出しを書き終えた知らせ・音の装置の呼び出し
    let expected = [
        ("session_end_windows.rs", 1),
        ("app_menu_macos.rs", 1),
        ("video_export_macos.rs", 1),
        ("audio_out.rs", 1),
    ];
    let mut found = Vec::new();
    for path in production_sources() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        let text = std::fs::read_to_string(&path).expect("ソースを読める");
        let n = PATTERNS.iter().map(|p| text.matches(p).count()).sum::<usize>();
        if n > 0 {
            found.push((name, n));
        }
    }
    found.sort();
    let mut expected: Vec<(String, usize)> = expected.iter().map(|(f, n)| ((*f).to_owned(), *n)).collect();
    expected.sort();
    assert_eq!(found, expected, "OS・部品から呼ばれる受け口が名簿と違う");
}

/// メッシュ `mesh` の外周の辺（三角形の1つだけが使う辺）
fn mesh_boundary(mesh: &egui::Mesh) -> Vec<(Point, Point)> {
    let mut count: std::collections::HashMap<(u32, u32), usize> = std::collections::HashMap::new();
    for t in mesh.indices.chunks_exact(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let pt = |i: u32| {
        let p = mesh.vertices[i as usize].pos;
        Point::new(f64::from(p.x), f64::from(p.y))
    };
    count.into_iter().filter(|&(_, n)| n == 1).map(|((a, b), _)| (pt(a), pt(b))).collect()
}

/// 点 `p` から、外周の辺 `edges` のいちばん近いものまでの距離
fn distance_to_edges(p: Point, edges: &[(Point, Point)]) -> f64 {
    use kurbo::ParamCurveNearest;
    edges
        .iter()
        .map(|&(a, b)| kurbo::Line::new(a, b).nearest(p, 0.0).distance_sq.sqrt())
        .fold(f64::MAX, f64::min)
}

/// 倍率が高いときに画面に描くメッシュは、その段の許容で曲線に沿う（塗りは輪郭の曲線、線は中心の曲線から太さの半分の
/// 所。シンボルの中のように、縦横で違う拡大をかけた線も、太さごと写した形に沿う）。倍率を戻すと、図形が持つメッシュを使う
#[test]
fn zoomed_in_meshes_follow_the_curves_within_the_level_tolerance() {
    use crate::vector_engine::{mesh_level, MESH_TOL};
    let zoom = 16.0f32;
    let tol = MESH_TOL * 0.5f64.powi(mesh_level(zoom) as i32);
    // 塗り：大きく曲がった輪郭の図形
    let q = QuadBez::new((0.0, 0.0), (200.0, 300.0), (400.0, 0.0));
    let fill = FillShape::new(
        Color32::RED,
        crate::vector_engine::Contour::new(vec![q, QuadBez::new((400.0, 0.0), (200.0, 0.0), (0.0, 0.0))]),
        Vec::new(),
    );
    let edges = mesh_boundary(&fill.view_mesh(zoom));
    let coarse = mesh_boundary(fill.fill_mesh());
    let worst = |edges: &[(Point, Point)]| (0..=400).map(|i| distance_to_edges(q.eval(i as f64 / 400.0), edges)).fold(0.0, f64::max);
    assert!(worst(&edges) <= tol * 1.01, "塗りのずれ {} > {tol}", worst(&edges));
    assert!(worst(&coarse) > tol * 1.01, "図形が持つメッシュの方が粗いはず（試しの形が粗さを見分けない）");
    assert!(std::sync::Arc::ptr_eq(&fill.view_mesh(1.0), fill.fill_mesh()), "倍率を戻したら図形が持つメッシュ");
    // 線：中心の曲線から太さの半分の所。縦横で違う拡大をかけた線（シンボルの中）も同じ
    let stroke = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 10 };
    let center = QuadBez::new((0.0, 0.0), (60.0, 90.0), (120.0, 0.0));
    let ends = [crate::vector_stroke::LineEnd::Cap; 2];
    let (ext, holes, mesh) = crate::planar_map::line_band(center, stroke, ends).unwrap();
    let line = FillShape::new(stroke.color, ext, holes).with_line(stroke, center, ends).with_mesh(mesh);
    for xform in [Affine::IDENTITY, Affine::new([2.0, 0.0, 0.3, 0.5, 40.0, 10.0])] {
        let mut s = line.clone();
        s.place(xform);
        let edges = mesh_boundary(&s.view_mesh(zoom));
        let mut worst = 0.0f64;
        for i in 1..400 {
            let t = i as f64 / 400.0;
            let d = crate::vector_engine::quad_deriv(&center, t);
            let n = KVec2::new(-d.y, d.x) / d.hypot() * 5.0;
            for side in [n, -n] {
                worst = worst.max(distance_to_edges(xform * (center.eval(t) + side), &edges));
            }
        }
        assert!(worst <= tol * 1.05, "線（変換 {xform:?}）のずれ {worst} > {tol}");
        assert_eq!(s.center_curve().unwrap().p1, xform * center.p1, "線の中心の曲線は今の座標");
    }
}

/// ロック中のレイヤー（セル画の写しから描く絵）も、倍率が高いときは、画面に見えている図形だけを、編集している図形と同じ
/// 細かさのメッシュで描く
#[test]
fn locked_layers_are_drawn_finely_when_zoomed_in() {
    let mut app = Is2dApp::default();
    let circle = |cx: f64| {
        let r = 40.0;
        let k = r * 0.4142;
        let pts = [(r, 0.0), (r, k), (k, r), (0.0, r), (-k, r), (-r, k), (-r, 0.0), (-r, -k), (-k, -r), (0.0, -r), (k, -r), (r, -k)];
        let segs: Vec<QuadBez> = (0..4)
            .map(|i| {
                let p = |j: usize| Point::new(cx + pts[j % 12].0, 100.0 + pts[j % 12].1);
                QuadBez::new(p(3 * i), p(3 * i + 1), p(3 * i + 3))
            })
            .collect();
        FillShape::new(Color32::RED, crate::vector_engine::Contour::new(segs), Vec::new())
    };
    put_cel_shapes(&mut app, 0, 0, vec![circle(100.0), circle(400.0)]);
    app.tab_mut().with_active_timeline_mut(|tl, _| tl.layers[0].locked = true);
    let tab = app.tab();
    let plain = crate::screen_mesh::CelTint::Plain;
    let n = |zoom: f32, visible: kurbo::Rect| {
        tab.cached_cel_screen_mesh(0, 0, zoom, Vec2::ZERO, visible, plain).map_or(0, |m| m.vertices.len())
    };
    let left = kurbo::Rect::new(0.0, 0.0, 200.0, 200.0);
    let at_1 = n(1.0, WHOLE_VIEW);
    let at_16_left = n(16.0, left);
    let at_16_all = n(16.0, WHOLE_VIEW);
    assert!(at_16_all > at_1, "拡大しても細かくならない：{at_16_all} ≤ {at_1}");
    assert!(at_16_left * 2 <= at_16_all + 2 && at_16_left > 0, "見えていない図形まで描いた：{at_16_left} / {at_16_all}");
}

/// 最後のタブを閉じて白紙に戻すと、貼り付けのずらしも初めに戻る（新しい文書をタブに入れる道は1つ：`install_document`。直す
/// 前は、この道とまっさらなタブに開く道でずらしが前の作品の続きになった：約束の棚卸し 5616fcc の C の B2）
#[test]
fn closing_the_last_tab_resets_the_paste_offset() {
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(150.0, 150.0) }));
    assert!(apply(&mut app, Action::Copy));
    assert!(apply(&mut app, Action::Paste { in_place: false }));
    assert!(app.clipboard.paste_count > 0);
    project_io::close_tab(&mut app, 0, None);
    assert_eq!(app.clipboard.paste_count, 0, "最後のタブを閉じても貼り付けのずらしが残った");
    assert_eq!(app.clipboard.paste_offset, KVec2::ZERO);
}

/// 「タブを閉じる」の未保存の確認の最中に終了（Cmd+Q・窓を閉じる）を押すと、アプリ全体を終える確認に切り替わる（終了は
/// どのダイアログでも効く：持ち主が決めた 2026-11-27。直す前は、タブを閉じる確認の最中の終了は取り消されて何も起きな
/// かった：約束の棚卸し 5616fcc の C の D6）
#[test]
fn quitting_during_a_tab_close_confirm_switches_to_the_quit_confirm() {
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    assert!(apply(&mut app, Action::PaintStroke { points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)], radius: 8.0, color: Color32::RED }));
    app.apply_action(UiAction::CloseActiveTab, &ctx);
    assert!(matches!(app.pending_close_confirm, Some(PendingCloseConfirm::SingleTab { .. })));
    let mut close_raw = egui::RawInput::default();
    close_raw.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
    let out = ctx.run(close_raw, |ctx| app.update_ui(ctx));
    assert!(
        matches!(app.pending_close_confirm, Some(PendingCloseConfirm::ViewportClose { .. })),
        "タブを閉じる確認の最中の終了が、アプリを終える確認にならない"
    );
    assert!(out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
}

/// ダイアログ（ここでは作業の復元）が出ている間は、どこから来た操作も入口が断る（タイムラインのパネルのように `apply` を
/// 直接呼ぶ所も同じ関所を通る）。まとめの区切りだけは受け付ける（未保存の確認の「保存」が、保存の前に区切るため）。直す前は
/// タイムラインのパネルが関所を通らず、ダイアログの間にレイヤー名などを変えられた：約束の棚卸し 5616fcc の C の C1
#[test]
fn operations_are_refused_while_a_dialog_is_shown() {
    let mut app = Is2dApp::default();
    let layers = app.doc.root.layers.len();
    app.recovery.found.push(recovery::Found {
        entry_path: "a".into(),
        recovery_path: "b".into(),
        modified: std::time::SystemTime::UNIX_EPOCH,
    });
    assert!(!apply(&mut app, Action::AddLayer), "ダイアログの間にレイヤーを足せた");
    assert_eq!(app.doc.root.layers.len(), layers);
    let lid = app.doc.root.layers[0].id;
    assert!(!apply(&mut app, Action::RenameLayer { layer_id: lid, new_name: "x".into() }));
    assert!(apply(&mut app, Action::EndCoalesce { key: None }), "まとめの区切りは受け付ける");
}

/// S-5：格子の外（±65,536px）にかかるブラシ・輪郭の変形は、断って画面の下に知らせる
#[test]
fn off_grid_strokes_and_reshapes_are_refused_with_a_notice() {
    let red = Color32::from_rgb(235, 75, 75);
    let off_grid = canvas_tool::Refusal::OffGrid.message();
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
    // ブラシ
    app.notices = Default::default();
    assert!(!doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(65_000.0, 100.0), Point::new(66_000.0, 100.0)],
            radius: 5.0,
            color: red,
        },
    ));
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
        Some(off_grid.text.as_str())
    );
    // 輪郭の変形（辺を格子の外へ引っぱる）
    app.notices = Default::default();
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SetTool { tool: Tool::Select },
    );
    assert!(!pull_edge(&mut app, Point::new(150.0, 110.0), Point::new(150.0, 200_000.0)));
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
        Some(off_grid.text.as_str())
    );
    // 格子の中のブラシは知らせない
    app.notices = Default::default();
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(300.0, 300.0), Point::new(400.0, 300.0)],
            radius: 5.0,
            color: red,
        },
    ));
    assert!(app.notices.iter().next().is_none());
}

/// 「まだ何も編集していない空の無題タブ」は、編集の入口が進める世代 ID で決める（表示の切り替えは編集ではない）。直す
/// 前は、文書とタブの既定値を写した 18 の条件で推していたので、ステージの外を隠す表示を切り替えただけのタブを、編集した
/// タブとみなした（レビュー f01d518 の N-5008）
#[test]
fn a_pristine_tab_is_one_that_was_never_edited() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::project_io::is_active_tab_pristine_empty;
    let mut app = Is2dApp::default();
    assert!(is_active_tab_pristine_empty(app.tab()));
    app.tab_mut().view.clip_outside_stage = true;
    assert!(
        is_active_tab_pristine_empty(app.tab()),
        "表示の切り替えは編集ではない"
    );
    // 編集すれば、取り消しても編集したタブ
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(40.0, 40.0), Point::new(120.0, 40.0)],
            radius: 6.0,
            color: Color32::from_rgb(220, 50, 50),
        },
    ));
    assert!(!is_active_tab_pristine_empty(app.tab()));
    assert!(apply(&mut app, Action::Undo));
    assert!(!is_active_tab_pristine_empty(app.tab()));
    // 未保存の印が付いたタブ（別のタブの上書きでファイルから外したタブなど）は、空のタブとみなさない
    let mut detached = Is2dApp::default();
    detached.tab_mut().history.mark_unsaved();
    assert!(!is_active_tab_pristine_empty(detached.tab()));
}

/// ステージの大きさは、書く入口（`apply` の `SetStageSize`）とファイルを読む入口が同じ1つの判定（`stage_size_of`）で決める：
/// 有限でない大きさは書く入口で断り（文書も Undo も変わらない）、ファイルなら壊れたファイルとして断る。範囲の外は範囲に
/// そろえる（画面の欄と同じ範囲）
#[test]
fn stage_size_is_decided_by_one_rule_at_the_entries() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::symbol::{STAGE_MAX, STAGE_MIN};
    let mut app = Is2dApp::default();
    let before = app.doc.stage_size;
    let undo_before = app.history.undo_stack.len();
    for bad in [Vec2::new(f32::NAN, 450.0), Vec2::new(800.0, f32::INFINITY)] {
        assert!(!apply(&mut app, Action::SetStageSize { size: bad }));
        assert_eq!(app.doc.stage_size, before, "有限でない大きさは書かない");
    }
    assert_eq!(app.history.undo_stack.len(), undo_before, "Undo も積まない");
    assert!(apply(
        &mut app,
        Action::SetStageSize {
            size: Vec2::new(1.0, 100_000.0),
        },
    ));
    assert_eq!(
        app.doc.stage_size,
        Vec2::new(STAGE_MIN, STAGE_MAX.y),
        "範囲にそろえる"
    );
    // ファイルを読む入口も同じ判定：有限でない大きさのファイルは断り、範囲の外はそろえて読む
    let pd = saved_from_app(app.tab()).unwrap();
    let mut nan_stage = pd.clone();
    nan_stage.stage_size = [f32::NAN, 450.0];
    assert_eq!(
        opened_document(&crafted_bytes(&nan_stage).unwrap()).expect_err("断る"),
        "ファイルが壊れています（有限でない数があります）"
    );
    let mut wide = pd;
    wide.stage_size = [10_000.0, 10.0];
    let doc = project_from_bytes(&crafted_bytes(&wide).unwrap())
        .expect("読める")
        .into_document()
        .expect("読める");
    assert_eq!(doc.stage_size, Vec2::new(STAGE_MAX.x, STAGE_MIN));
}

/// 表の名前の番号（テストで項目を名前で指す）
fn menu_index(label: &str) -> usize {
    menu::COMMANDS
        .iter()
        .position(|c| c.label == label)
        .expect("表にある項目")
}

/// 表の外にあったキー（矢印・F5・F6・Shift の付く道具のキー・Cmd+G など）も、表（`menu::COMMANDS`）から前と同じに動く
/// （約束の棚卸し 5616fcc の C の E3）：
/// - 矢印：選んでいれば動かす（Shift なら 10px）。押しっぱなしの繰り返しは1手にまとまり、押し直すと別の1手。選んでいなければ
///   左右でコマを送る
/// - Shift と押しても道具が変わる。F5 で延長、Shift+F5 で削除。Cmd+G でグループ化
/// - 入力欄に文字を入れている最中は、メニューに出さないキーは受け付けない（Cmd+G でもグループにしない）
#[test]
fn keys_outside_the_menu_work_from_the_table() {
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let none = egui::Modifiers::NONE;
    // キーを押して離す（`repeat` なら、押しっぱなしの繰り返しの合図を足してから離す）
    let key = |key: egui::Key, modifiers: egui::Modifiers, repeat: bool| {
        let ev = |pressed, repeat| egui::Event::Key { key, physical_key: Some(key), pressed, repeat, modifiers };
        let mut events = vec![ev(true, false)];
        if repeat {
            events.extend([ev(true, true), ev(true, true)]);
        }
        events.push(ev(false, false));
        events
    };
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED)]);
    app.set_selected_indices(vec![0]);
    let undo0 = app.history.undo_stack.len();
    let left = |app: &Is2dApp| app.tab().committed_document_copy().unwrap().root.layers[0].keyframes[0].cel.map_bounds().unwrap().x0;
    // 押して、押しっぱなしの繰り返しを2回：3px 動いて1手
    menu_test_frame(&ctx, &mut app, none, key(egui::Key::ArrowRight, none, true), |_, _| {});
    assert!((left(&app) - 103.0).abs() < 1e-6, "矢印で動かない：{}", left(&app));
    assert_eq!(app.history.undo_stack.len(), undo0 + 1, "押しっぱなしが1手にまとまらない");
    // 押し直すと別の1手。Shift なら 10px
    let shift = egui::Modifiers::SHIFT;
    menu_test_frame(&ctx, &mut app, shift, key(egui::Key::ArrowRight, shift, false), |_, _| {});
    assert!((left(&app) - 113.0).abs() < 1e-6, "Shift+矢印で 10px 動かない：{}", left(&app));
    assert_eq!(app.history.undo_stack.len(), undo0 + 2, "押し直しが別の1手にならない");
    // Shift と押しても道具が変わる
    menu_test_frame(&ctx, &mut app, shift, key(egui::Key::B, shift, false), |_, _| {});
    assert_eq!(app.tool.tool, Tool::Brush);
    // 入力中の Cmd+G はグループにしない。入力をやめれば、グループにする
    let cmd = egui::Modifiers::COMMAND;
    menu_test_frame(&ctx, &mut app, none, key(egui::Key::V, none, false), |_, _| {});
    app.set_selected_indices(vec![0]);
    let focus = |ctx: &egui::Context, _: &mut Is2dApp| ctx.memory_mut(|m| m.request_focus(egui::Id::new("入力欄")));
    menu_test_frame(&ctx, &mut app, cmd, key(egui::Key::G, cmd, false), focus);
    assert!(app.scene().iter().all(|s| s.group_path.is_empty()), "入力中の Cmd+G でグループにした");
    let unfocus = |ctx: &egui::Context, _: &mut Is2dApp| ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("入力欄")));
    menu_test_frame(&ctx, &mut app, none, Vec::new(), unfocus);
    menu_test_frame(&ctx, &mut app, cmd, key(egui::Key::G, cmd, false), |_, _| {});
    assert!(app.scene().iter().any(|s| !s.group_path.is_empty()), "Cmd+G でグループにならない");
    // 選んでいなければ、左右の矢印でコマを送る。F5 で延長、Shift+F5 で削除
    menu_test_frame(&ctx, &mut app, none, key(egui::Key::Escape, none, false), |_, _| {});
    assert!(app.session.selection().is_empty());
    let frames = app.tab().active_timeline().total_frames;
    menu_test_frame(&ctx, &mut app, none, key(egui::Key::F5, none, false), |_, _| {});
    assert_eq!(app.tab().active_timeline().total_frames, frames + 1, "F5 で延長しない");
    menu_test_frame(&ctx, &mut app, none, key(egui::Key::ArrowRight, none, false), |_, _| {});
    assert_eq!(app.tab().active_timeline().current_frame, 1, "選んでいないときの矢印でコマを送らない");
    menu_test_frame(&ctx, &mut app, shift, key(egui::Key::F5, shift, false), |_, _| {});
    assert_eq!(app.tab().active_timeline().total_frames, frames, "Shift+F5 で削除しない");
}

/// 表の決まり：Mac の OS のメニューに付ける `Cmd` のショートカットは1文字のキー（`keyEquivalent` にする）。
/// egui のキー処理が受け取るキーは、項目どうしで重ならない（OS のメニューがあってもなくても）
#[test]
fn menu_table_keys_are_unique_and_command_keys_are_single_characters() {
    use menu::ShiftKey;
    for command in &menu::COMMANDS {
        if let Some(s) = command.shortcut.filter(|s| s.command && command.menu.is_some()) {
            assert_eq!(s.key.name().chars().count(), 1, "{}", command.label);
            // OS のメニューに付けるキーは、Shift を決めた、キーの意味で見るキー
            assert!(s.shift != ShiftKey::Either && !s.by_position, "{}", command.label);
        }
    }
    // 1回の押し方で2つの項目に当たるキーが無い（Shift がどちらでもよいキーは、Shift を押す・押さないの両方と重なる。キーの
    // 意味と位置は、配列で重なりうるので区別しない。前は表の外の決まりと `else if` の並びで、黙って片方が勝った：約束の
    // 棚卸し 5616fcc の C の E3）
    let keys: Vec<(&str, menu::Shortcut)> = menu::COMMANDS
        .iter()
        .flat_map(|c| c.shortcut.into_iter().chain(c.aliases.iter().copied()).map(move |k| (c.label, k)))
        .collect();
    let overlaps = |a: menu::Shortcut, b: menu::Shortcut| {
        a.key == b.key
            && a.command == b.command
            && (a.shift == b.shift || a.shift == ShiftKey::Either || b.shift == ShiftKey::Either)
    };
    for (i, &(la, a)) in keys.iter().enumerate() {
        for &(lb, b) in &keys[i + 1..] {
            assert!(!overlaps(a, b), "{a:?}（{la}）と {b:?}（{lb}）が重なる");
        }
        // どのキーも、押せばその項目が動く（OS のメニューがあるときは、メニューに出す `Cmd` のキーは OS が受け取る）
        for shift in [false, true] {
            if a.shift == ShiftKey::On && !shift || a.shift == ShiftKey::Off && shift {
                continue;
            }
            let pressed = menu::Pressed { key: a.key, position: Some(a.key), command: a.command, shift };
            for os_menu in [false, true] {
                let found = menu::command_for_key(pressed, os_menu).map(|i| menu::COMMANDS[i].label);
                let os_takes = os_menu && a.command && menu::COMMANDS.iter().any(|c| c.label == la && c.menu.is_some());
                assert!(found == Some(la) || (os_takes && found.is_none()), "{a:?}（{la}）：{found:?}");
            }
        }
    }
    // OS のメニューがあるときは、メニューに出す `Cmd` のキーだけを egui が受け取らない
    let press = |key, command, shift| menu::Pressed { key, position: Some(key), command, shift };
    let cmd_c = press(egui::Key::C, true, false);
    assert_eq!(menu::command_for_key(cmd_c, false), Some(menu_index("コピー")));
    assert_eq!(menu::command_for_key(cmd_c, true), None);
    assert_eq!(menu::command_for_key(press(egui::Key::Backspace, false, false), true), Some(menu_index("選択削除")));
    assert_eq!(menu::command_for_key(press(egui::Key::Z, true, true), true), Some(menu_index("やり直し")));
    // メニューに出さない `Cmd` のキーは、OS のメニューがあっても egui が受け取る
    assert_eq!(menu::command_for_key(press(egui::Key::G, true, false), true), Some(menu_index("グループ化")));
}

/// メニューの見せ方：表の決まり（今と同じ）で使えるかが決まり、表示の切り替えにはチェックが付く。入力欄に文字を
/// 入れている最中は、文字の操作のある項目はいつも使える。復旧のダイアログが出ている間は、アプリへ渡す項目は終了の
/// ほかは使えない（終了はどのダイアログでも受け付ける：持ち主が決めた 2026-11-26、レビュー de92ceb の N-5002）
#[test]
fn menu_states_follow_table_rules_typing_and_dialogs() {
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let copy = menu_index("コピー");
    let save = menu_index("保存");
    let onion = menu_index("オニオンスキン");
    assert!(
        !app.menu_item_state(copy, false).enabled,
        "選んでいないとコピーできない"
    );
    assert!(
        app.menu_item_state(copy, true).enabled,
        "入力欄へはいつも渡せる"
    );
    assert!(app.menu_item_state(save, false).enabled);
    assert_eq!(app.menu_item_state(onion, false).checked, Some(false));
    assert_eq!(app.menu_item_state(save, false).checked, None);
    app.view.onion_skin_enabled = true;
    assert_eq!(app.menu_item_state(onion, false).checked, Some(true));

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
    app.set_selected_indices(vec![0]);
    assert!(app.menu_item_state(copy, false).enabled);

    app.recovery.found.push(recovery::Found {
        entry_path: "a".into(),
        recovery_path: "b".into(),
        modified: std::time::SystemTime::UNIX_EPOCH,
    });
    menu_test_frame(&ctx, &mut app, egui::Modifiers::NONE, vec![], |_, _| {});
    let states = app.menu_states(&ctx);
    let quit = menu::COMMANDS
        .iter()
        .position(|c| matches!(c.action, panels::UiAction::Quit))
        .expect("終了の項目");
    assert!(states[quit].enabled, "復旧のダイアログの間も終了できる");
    assert!(
        states.iter().enumerate().all(|(i, s)| i == quit || !s.enabled),
        "復旧のダイアログの間は、終了のほかは使えない"
    );
}

/// キーの道：表のキーで表の `UiAction` が1回だけ動く。入力欄に文字を入れている最中は、文字の操作のある項目のキーと
/// `Cmd` の付かないキーは入力欄のもの（絵は変わらない）。ファイルのキーは入力中も動く
#[test]
fn menu_keys_dispatch_table_actions_and_leave_text_keys_to_fields() {
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let cmd = egui::Modifiers::COMMAND;
    let none = egui::Modifiers::NONE;
    menu_test_frame(
        &ctx,
        &mut app,
        cmd,
        menu_key_tap(egui::Key::Num3, cmd),
        |_, _| {},
    );
    assert!(
        app.view.clip_outside_stage,
        "Cmd+3 で枠外を隠す（1回だけ切り替わる）"
    );
    menu_test_frame(
        &ctx,
        &mut app,
        none,
        menu_key_tap(egui::Key::O, none),
        |_, _| {},
    );
    assert!(!app.view.onion_skin_enabled, "O でオニオンスキンが切り替わった（オニオンスキンのキーは無い）");
    assert_eq!(app.tool.tool, Tool::InkBottle, "O でインクボトルにならない");
    apply(&mut app, Action::SetTool { tool: Tool::Select });

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
    app.set_selected_indices(vec![0]);
    let focus = |ctx: &egui::Context, _: &mut Is2dApp| {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("入力欄")));
    };
    // 入力中：コピー（Cmd+C は egui-winit がコピーの合図にする）・O（ツールのキー）・Backspace は入力欄のもの
    menu_test_frame(&ctx, &mut app, cmd, vec![egui::Event::Copy], focus);
    assert!(
        app.clipboard.content.is_none(),
        "入力中のコピーは絵をコピーしない"
    );
    menu_test_frame(
        &ctx,
        &mut app,
        none,
        menu_key_tap(egui::Key::O, none),
        focus,
    );
    assert_eq!(app.tool.tool, Tool::Select, "入力中の O は入力欄の文字（ツールを変えない）");
    menu_test_frame(
        &ctx,
        &mut app,
        none,
        menu_key_tap(egui::Key::Backspace, none),
        focus,
    );
    assert_eq!(app.scene().len(), 1, "入力中の Backspace は絵を消さない");
    // 入力中でない：同じキーで絵が変わる（egui は、頼んだ次のコマまで focus を残すので、ここで外す）
    let unfocus = |ctx: &egui::Context, _: &mut Is2dApp| {
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("入力欄")));
    };
    menu_test_frame(&ctx, &mut app, cmd, vec![egui::Event::Copy], unfocus);
    assert!(app.clipboard.content.is_some());
    menu_test_frame(
        &ctx,
        &mut app,
        none,
        menu_key_tap(egui::Key::Backspace, none),
        |_, _| {},
    );
    assert_eq!(app.scene().len(), 0, "Backspace で選択削除");
}

/// OS のメニューの受け箱から `apply_action` までの道：項目の番号で、キーと同じ `UiAction` が動く。入力欄に文字を
/// 入れている最中の文字の操作は、絵の操作でなく egui の入力の合図にして入力欄へ渡す
#[cfg(target_os = "macos")]
#[test]
fn os_menu_choices_dispatch_like_keys_and_forward_text_ops_to_fields() {
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let none = egui::Modifiers::NONE;
    let choose = |labels: &'static [&'static str]| {
        move |ctx: &egui::Context, app: &mut Is2dApp| {
            app.run_os_menu_choices(labels.iter().map(|l| menu_index(l)).collect(), ctx);
        }
    };
    menu_test_frame(
        &ctx,
        &mut app,
        none,
        vec![],
        choose(&["枠外を隠す", "オニオンスキン"]),
    );
    assert!(app.view.clip_outside_stage && app.view.onion_skin_enabled);
    app.tab_mut().view.set_zoom(2.0);
    menu_test_frame(&ctx, &mut app, none, vec![], choose(&["100%表示"]));
    assert_eq!(app.view.zoom(), 1.0);

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
    app.set_selected_indices(vec![0]);
    let undo_len = app.history.undo_stack.len();

    // 入力中：コピー・元に戻す・選択削除は、入力欄への合図になり、絵は変わらない
    let forwarded = std::cell::RefCell::new(Vec::new());
    menu_test_frame(&ctx, &mut app, none, vec![], |ctx, app| {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("入力欄")));
        app.run_os_menu_choices(
            ["コピー", "元に戻す", "選択削除"]
                .iter()
                .map(|l| menu_index(l))
                .collect(),
            ctx,
        );
        forwarded.replace(ctx.input(|i| i.events.clone()));
    });
    let forwarded = forwarded.into_inner();
    assert!(matches!(forwarded[0], egui::Event::Copy));
    assert!(matches!(
        forwarded[1],
        egui::Event::Key { key: egui::Key::Z, pressed: true, modifiers, .. }
            if modifiers.matches_logically(egui::Modifiers::COMMAND)
    ));
    assert!(matches!(
        forwarded[2],
        egui::Event::Key {
            key: egui::Key::Backspace,
            pressed: true,
            ..
        }
    ));
    assert!(app.clipboard.content.is_none(), "絵はコピーしない");
    assert_eq!(app.scene().len(), 1, "絵は消さない");
    assert_eq!(app.history.undo_stack.len(), undo_len, "文書は元に戻さない");

    // 入力中でない：同じ番号で絵の操作になる（egui は、頼んだ次のコマまで focus を残すので、ここで外す）
    menu_test_frame(&ctx, &mut app, none, vec![], |ctx, app| {
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("入力欄")));
        choose(&["コピー", "選択削除"])(ctx, app);
    });
    assert!(app.clipboard.content.is_some());
    assert_eq!(app.scene().len(), 0);
}

/// 作品の番号を使い切ったら、番号を配らずに計算を止め、入口の砦が操作の前に戻す（番号は決してぶつからない。数え札の決まりは
/// `symbol::Counter` の1つ：設計審査 01e4283 の D-4。前はグループとシンボルの番号は上限で 1 に戻り、今ある番号とぶつかりえた）
#[test]
fn a_used_up_counter_never_gives_a_number_twice() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectAll));
    app.doc.next_group_id = crate::symbol::Counter::starting_at(u64::MAX);
    let before = app.doc.root.layers[0].keyframes[0].cel.clone();
    app.notices = Default::default();
    assert!(!apply(&mut app, Action::Group), "番号を使い切ったのにグループにした");
    assert!(std::sync::Arc::ptr_eq(app.doc.root.layers[0].keyframes[0].cel.maps(), before.maps()), "断ったのに作品が変わった");
    assert_eq!(app.doc.next_group_id.next(), u64::MAX, "断ったのに数え札が進んだ");
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
        Some("内部の不具合でこの操作を続けられなかったので、操作の前に戻しました")
    );
}

/// 部屋と層の表（層の決まりはここの1か所：ARCHITECTURE.md の 7 章と同じ）。1：形の計算と何も参照しない部品、2：作品の
/// データ、3：作品を変える操作、4：画面の部品、5：アプリ全体
const MODULE_LAYERS: &[(&str, u8)] = &[
    ("chunked_list", 1),
    ("guard", 1),
    ("symbol_binding", 1),
    ("sync_cache", 1),
    ("planar_map", 1),
    ("serial", 1),
    ("stored_map", 1),
    ("vector_engine", 1),
    ("vector_stroke", 1),
    ("audio_latency_macos", 2),
    ("audio_out", 2),
    ("prefetch", 2),
    ("save_format", 2),
    ("sound", 2),
    ("symbol", 2),
    ("symbol_library", 2),
    ("timeline", 2),
    ("canvas_tool", 3),
    ("cel_writes", 3),
    ("clipboard_ops", 3),
    ("doc_actions", 3),
    ("floating", 3),
    ("map_hover", 3),
    ("project_io", 3),
    ("recovery", 3),
    ("recovery_journal", 3),
    ("scene_base_cache", 3),
    ("screen_mesh", 3),
    ("select_ops", 3),
    ("session_end_windows", 3),
    ("symbol_ops", 3),
    ("tool_paint", 3),
    ("tool_select", 3),
    ("tool_transform", 3),
    ("video_export", 3),
    ("video_export_macos", 3),
    ("video_export_windows", 3),
    ("color_picker", 4),
    ("menu", 4),
    ("panels", 4),
    ("timeline_ui", 4),
    ("app_menu_macos", 5),
    ("main", 5),
];

/// ソースの文 `src` から、行の終わりまでの注釈（`//`）を除いたもの（説明の中の名前を参照と数えない）
fn without_line_comments(src: &str) -> String {
    src.lines().map(|l| l.find("//").map_or(l, |i| &l[..i])).collect::<Vec<_>>().join("\n")
}

/// ソースの文 `src` の中で、`crate::` のすぐあとに書かれた名前と、`use crate::{…}` の括弧の中の名前
fn crate_paths(src: &str) -> Vec<String> {
    let ident = |s: &str| s.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>();
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("crate::") {
        rest = &rest[i + "crate::".len()..];
        if let Some(group) = rest.strip_prefix('{') {
            // 括弧の終わりまで（入れ子も数える）の名前をみな
            let mut depth = 1;
            let end = group
                .char_indices()
                .find(|&(_, c)| {
                    depth += match c {
                        '{' => 1,
                        '}' => -1,
                        _ => 0,
                    };
                    depth == 0
                })
                .map_or(group.len(), |(j, _)| j);
            out.extend(group[..end].split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|s| !s.is_empty()).map(str::to_owned));
        } else {
            out.push(ident(rest));
        }
    }
    out
}

/// アプリの頭（`main.rs`）が取り込んだ名前と、その部屋（`use 部屋::{…}`・`pub(crate) use 部屋::…`。部品の取り込み
/// （`eframe`・`kurbo` など）も含む）。文ごと（`;` まで。括弧が何行にまたがっても1つ）に読む（層の向きと呼び名の見張りが使う）
fn main_imports() -> Vec<(String, String)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let main = without_line_comments(&std::fs::read_to_string(path).unwrap());
    let mut out = Vec::new();
    for stmt in main.split(';') {
        let stmt = stmt.trim_start();
        let stmt = stmt.strip_prefix("#[cfg(test)]").map_or(stmt, str::trim_start);
        let line = stmt.trim_start_matches("pub(crate) ");
        let Some(path) = line.strip_prefix("use ") else {
            continue;
        };
        let module: String = path.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let names = &path[module.len()..];
        out.extend(
            names
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .filter(|s| !s.is_empty() && *s != "self")
                .map(|s| (s.to_owned(), module.clone())),
        );
    }
    out
}

/// 1つのものに呼び名は1つ（設計：claude/呼び名を1つにする設計.md）：(1) 本番のファイルは、よその部屋のものを出し直さない
/// （`pub(crate) use` などで出してよいのは、同じファイルが外から見えない小部屋（`mod 名前;`）として持つもの。アプリの頭
/// （`main.rs`）の部屋はどこからでも見えるので、頭は何も出し直さない）、(2) どのファイル（テストを含む）も `use crate::*`
/// を使わない（アプリの頭が自分のために取り込んだ名前まで拾う）、(3) アプリの頭が取り込んだ名前を、ほかのファイルが
/// `crate::名前` で呼ばない
#[test]
fn every_item_has_one_name() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let imported: Vec<String> = main_imports().into_iter().map(|(name, _)| name).collect();
    let tests_dir = src.join("canvas_tool/tests");
    let mut files = production_sources();
    files.extend(std::fs::read_dir(&tests_dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "rs")));
    let mut wrong = Vec::new();
    for p in files {
        let rel = p.strip_prefix(&src).unwrap().display().to_string();
        let text = without_line_comments(&std::fs::read_to_string(&p).unwrap());
        let is_main = rel == "main.rs";
        let is_test = p.starts_with(&tests_dir);
        // 外から見えない小部屋（`mod 名前;`。`pub(crate) mod` でないもの）
        let private_mods: Vec<String> = text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("mod ").and_then(|r| r.strip_suffix(';')).map(|m| m.trim().to_owned()))
            .collect();
        for stmt in text.split(';') {
            let stmt = stmt.trim();
            let stmt = stmt.rsplit('\n').next().unwrap_or(stmt).trim();
            if stmt == "use crate::*" {
                wrong.push(format!("{rel}：use crate::*"));
            }
            if is_test {
                continue;
            }
            let Some(path) = ["pub(crate) use ", "pub use ", "pub(super) use "].iter().find_map(|k| stmt.strip_prefix(k)) else {
                continue;
            };
            let path = path.strip_prefix("self::").unwrap_or(path);
            let first: String = path.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if is_main || !private_mods.contains(&first) {
                wrong.push(format!("{rel}：よその部屋のものの出し直し `{stmt}`"));
            }
        }
        if !is_main {
            for n in crate_paths(&text) {
                if imported.contains(&n) {
                    wrong.push(format!("{rel}：アプリの頭が取り込んだ名前 crate::{n}"));
                }
            }
        }
    }
    wrong.sort();
    wrong.dedup();
    assert!(wrong.is_empty(), "1つのものに呼び名が2つある：\n{}", wrong.join("\n"));
}

/// 下の層の部屋は上の層の部屋を使わない：`src/` の下のどのファイル（テストのファイルを除く）も、`crate::部屋`・`use
/// crate::{…}`・アプリの頭で付け直した名前（`main.rs` の `use 部屋::{…}` と `pub(crate) use 部屋::…`）で、自分より上の層の
/// 部屋を使っていない。表 `MODULE_LAYERS` に無い部屋のファイルがあっても落ちる（新しい部屋は層を決めて表に書く）
#[test]
fn every_module_uses_only_lower_layers() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let layer_of = |name: &str| MODULE_LAYERS.iter().find(|(m, _)| *m == name).map(|&(_, l)| l);
    // アプリの頭で取り込んだ名前 → その部屋の層
    let renamed: Vec<(String, u8)> =
        main_imports().into_iter().filter_map(|(name, module)| layer_of(&module).map(|l| (name, l))).collect();
    let mut wrong = Vec::new();
    for p in production_sources() {
        let rel = p.strip_prefix(&src).unwrap();
        // サブフォルダの中のファイルは、フォルダの部屋（planar_map の中）か、自分の名前の部屋（canvas_tool の中）
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        let parent = rel.parent().and_then(|d| d.to_str()).filter(|d| !d.is_empty());
        let module = match parent {
            Some(d) if layer_of(&stem).is_none() => d.to_owned(),
            _ => stem,
        };
        let Some(own) = layer_of(&module) else {
            wrong.push(format!("{}：部屋 {module} が層の表 MODULE_LAYERS に無い", rel.display()));
            continue;
        };
        let text = without_line_comments(&std::fs::read_to_string(&p).unwrap());
        let mut used: Vec<(String, u8)> = crate_paths(&text)
            .into_iter()
            .filter_map(|n| layer_of(&n).map(|l| (n.clone(), l)).or_else(|| renamed.iter().find(|(r, _)| *r == n).cloned()))
            .filter(|&(_, l)| l > own)
            .collect();
        used.sort();
        used.dedup();
        for (n, l) in used {
            wrong.push(format!("{}（{own} 層め）が {n}（{l} 層め）を使っている", rel.display()));
        }
    }
    assert!(wrong.is_empty(), "下の層が上の層を使っている：\n{}", wrong.join("\n"));
}

/// アプリ全体の通し番号は、使い切っても同じ番号を2回配らない：使い切ったら計算を止める（砦が受け止める）。前は数の上限で
/// 黙って 0 に戻り、前に配った番号をまた配った（設計：claude/層の分け方の直しの設計.md の 2.）
#[test]
fn a_used_up_serial_never_gives_a_number_twice() {
    let s = crate::serial::Serial::starting_at(u64::MAX - 1);
    assert_eq!(s.take(), u64::MAX - 1);
    assert_eq!(crate::guard::guarded(|| s.take()), None, "番号を使い切ったのに配った");
    assert_eq!(crate::guard::guarded(|| s.take()), None, "番号を使い切ったのに配った");
    let many = crate::serial::Serial::starting_at(u64::MAX - 3);
    assert_eq!(crate::guard::guarded(|| many.take_many(10)), None, "番号が足りないのにまとめて配った");
}
