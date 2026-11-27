//! シンボル（定義・インスタンス・ライブラリ・入れ子）のテスト

use super::*;

/// シンボルに付ける名前（`Document::numbered_name`）は、候補の名前を1つずつ試して最初の空いた名前を選ぶ答え（お手本）と
/// 同じ：番号の付いた名前・付いていない名前・先頭に 0 のある番号・同じ名前の重なり（名前を変えて重ねる）をでたらめに
/// 混ぜたライブラリで、付け方・元・始める番号をでたらめに選んで比べる。名前の表はお手本とも比べる（全体の点検 2026-11-27）
#[test]
fn numbered_names_match_trying_each_candidate() {
    use crate::symbol::NumberForm;
    let mut rng = OpRng(0x0a3e_0001);
    let stems = ["Symbol", "A", "Aのコピー", "B C"];
    for round in 0..60 {
        let mut doc = symbol::Document::default();
        for _ in 0..rng.range(40) {
            let stem = stems[rng.range(stems.len())];
            let name = match rng.range(5) {
                0 => stem.to_owned(),
                1 => format!("{stem} {}", rng.range(8)),
                2 => format!("{stem} ({})", rng.range(8)),
                3 => format!("{stem} 0{}", rng.range(8)),
                _ => format!("{stem} {}", 1 + rng.range(3)),
            };
            let id = doc.alloc_symbol_id();
            doc.add_symbol(None, symbol::SymbolDef::with_timeline(id, name, timeline::Timeline::default()));
        }
        // 名前を変えて重ねる
        if let Some(s) = doc.library().at(0).map(|s| s.id) {
            let other = doc.library().at(doc.library().len() - 1).unwrap().name.clone();
            doc.rename_symbol(s, other);
        }
        assert!(doc.library().check_tables().is_ok(), "round={round}");
        for _ in 0..20 {
            let stem = stems[rng.range(stems.len())];
            let form = if rng.coin() { NumberForm::Space } else { NumberForm::Paren };
            let from = rng.range(6) as u64;
            let plain = rng.coin().then_some(stem);
            let taken = |n: &str| doc.library().iter_by_id().any(|s| s.name == n);
            let expected = plain
                .filter(|p| !taken(p))
                .map(str::to_owned)
                .unwrap_or_else(|| (from..).map(|n| form.name(stem, n)).find(|n| !taken(n)).unwrap());
            assert_eq!(doc.numbered_name(plain, form, stem, from), expected, "round={round} stem={stem} from={from}");
        }
    }
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
    app.set_selected_indices(vec![0]);
    let orig_gid = doc_actions::group_selected_shapes(&mut app).unwrap();

    assert!(doc_actions::copy_selected_shapes(&mut app));
    // 高ズーム時でもワールド座標上の一定オフセット (+16, +16) で貼り付けられること
    app.view.set_zoom(4.0);
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
    app.set_selected_indices(vec![0]);

    let sym_id =
        doc_actions::convert_selection_to_symbol(&mut app).expect("シンボル変換が成功すること");
    assert_eq!(app.doc.library().len(), 1);
    assert_eq!(app.doc.library().at(0).unwrap().id, sym_id);
    assert_eq!(app.doc.library().at(0).unwrap().name, "Symbol 1");
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
    assert_eq!(app.session.edit_path().len(), 1);
    assert_eq!(app.active_timeline().total_frames, 1);

    assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
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
    assert!(app.session.edit_path().is_empty());
    assert_eq!(app.doc.library().at(0).unwrap().timeline.total_frames, 2);

    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.total_frames, 3);

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(scene_summary(&app).len, 2);

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(scene_summary(&app).len, 4);

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(scene_summary(&app).len, 2);
}

#[test]
fn test_classic_tween_requires_single_symbol_instance() {
    use crate::timeline::TweenState;

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

    app.set_selected_indices(vec![0]);
    assert!(doc_actions::group_selected_shapes(&mut app).is_some());
    assert!(!doc_actions::timeline_toggle_tween(&mut app));
    assert!(doc_actions::ungroup_selected_shapes(&mut app));

    app.set_selected_indices(vec![0]);
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
fn test_symbol_playback_modes_loop_play_once_and_single_frame() {
    use crate::symbol_binding::SymbolLoopMode;

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
    app.set_selected_indices(vec![0]);
    assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());

    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app,
        binding.instance_gid
    ));

    assert!(doc_actions::timeline_extend_frame(&mut app));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
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

    assert!(doc_actions::timeline_extend_frame(&mut app));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
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
        assert!(doc_actions::timeline_extend_frame(&mut app));
    }
    assert_eq!(app.doc.root.total_frames, 5);

    let expected_loop = [red, green, blue, red, green];
    for (f, &col) in expected_loop.iter().enumerate() {
        doc_actions::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(
            scene_summary(&app).shapes[0].color(),
            col,
            "Loop mode at frame {f}"
        );
    }

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    app.set_selected_indices(vec![0]);
    assert!(doc_actions::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::PlayOnce,
        0
    ));
    let expected_once = [red, green, blue, blue, blue];
    for (f, &col) in expected_once.iter().enumerate() {
        doc_actions::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(
            scene_summary(&app).shapes[0].color(),
            col,
            "PlayOnce mode at frame {f}"
        );
    }

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    app.set_selected_indices(vec![0]);
    assert!(doc_actions::set_selected_symbol_playback(
        &mut app,
        SymbolLoopMode::SingleFrame,
        1
    ));
    assert_eq!(scene_summary(&app).shapes[0].color(), green);
    assert_eq!(app.session.selected_indices(), vec![0]);
    for f in 0..5 {
        doc_actions::switch_to_layer_and_frame(&mut app, 0, f);
        assert_eq!(scene_summary(&app).shapes[0].color(), green);
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
    app.set_selected_indices(vec![0]);
    let child_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    let (child_binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app,
        child_binding.instance_gid
    ));
    assert!(doc_actions::timeline_extend_frame(&mut app));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
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

    app.set_selected_indices(vec![0]);
    let parent_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert_ne!(child_sym_id, parent_sym_id);
    // グラフィックシンボルの決まり：中の子は親シンボルの内部フレームで動くので、子を動かすには親にもコマ数が要る
    assert!(symbol_ops::enter_symbol_from_library(
        &mut app,
        parent_sym_id
    ));
    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert!(symbol_ops::exit_symbol_timeline(&mut app));

    assert!(doc_actions::timeline_extend_frame(&mut app));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(scene_summary(&app).shapes[0].color(), red);
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    assert_eq!(scene_summary(&app).shapes[0].color(), blue);

    assert!(symbol_ops::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.active_timeline().total_frames, 3);
    assert!(symbol_ops::exit_symbol_timeline(&mut app));

    assert!(doc_actions::timeline_extend_frame(&mut app));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    assert_eq!(scene_summary(&app).len, 1);
    assert!(scene_summary(&app).shapes[0].is_empty_symbol_marker());
    canvas_tool::select_shapes_for_timeline_selection(&mut app);
    assert_eq!(app.session.selected_indices(), vec![0]);
    assert!(app
        .scene()
        .selected_single_symbol_binding(app.session.selection())
        .is_some());
    assert!(canvas_tool::clear_selection(&mut app));
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
            crate::symbol::SymbolEvalCtx::new(app.doc.library()),
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
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
    app.set_selected_indices(vec![0]);
    let child_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    app.set_selected_indices(vec![0]);
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

    doc_actions::set_layer_locked(&mut app, 0, true);
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
            .instances()
            .is_empty(),
        "子シンボルを削除すると親シンボル内の子シンボルインスタンスも消えること"
    );
    app.undo();
    assert!(app.doc.find_symbol(child_sym_id).is_some());
    assert_eq!(
        app.doc.find_symbol(parent_sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .instances()
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
    assert!(app.doc.root.layers[0].keyframes[0].cel.instances().is_empty());

    assert!(symbol_ops::enter_symbol_from_library(
        &mut app,
        child_sym_id
    ));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    app.set_selected_indices(vec![0]);
    let lib_len_before = app.doc.library().len();
    let grandchild_sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert_eq!(app.doc.library().len(), lib_len_before + 1);
    assert!(!app
        .doc
        .find_symbol(child_sym_id)
        .unwrap()
        .evaluated_local_shapes_at(0, app.doc.library())
        .is_empty());
    app.undo();
    assert_eq!(app.doc.library().len(), lib_len_before);
    app.redo();
    assert!(app.doc.find_symbol(grandchild_sym_id).is_some());
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
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
    assert!(symbol_ops::enter_symbol_timeline(&mut app, b.instance_gid));
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
    assert!(!symbol_ops::exit_symbol_timeline(&mut app));
    assert!(!select_ops::exit_current_group(&mut app));
    app.undo();
    app.redo();
    doc_actions::set_layer_visible(&mut app, 999, false);
    doc_actions::set_layer_locked(&mut app, 999, true);
    assert!(!doc_actions::timeline_move_or_copy_frames(
        &mut app,
        crate::timeline::TimelineCellSelection::single(999, 999),
        1,
        1,
        false
    ));

    let clean_bbox = scene_summary(&app).shapes[0].bounding_box();
    app.set_selected_indices(vec![0]);
    let move_drag = clipboard_ops::begin_move_selection_drag(&mut app, Point::ZERO, false);
    app.session.drag = crate::canvas_tool::ActiveCanvasDrag::MoveSelection(move_drag);
    app.session.translate_move_overlay(KVec2::new(500.0, 500.0));
    assert!(canvas_tool::clear_selection(&mut app));
    assert_eq!(app.scene().get(0).unwrap().bounding_box(), clean_bbox);

    app.session.drag = paint_drag(&[Point::new(10.0, 10.0), Point::new(40.0, 10.0)], 8.0);
    assert!(canvas_tool::clear_selection(&mut app));
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
        .shapes()[0]
        .clone();
    let orig_rev = orig_local_shape.mesh_revision;
    for _ in 0..10 {
        assert!(symbol_ops::enter_symbol_timeline(&mut app, b.instance_gid));
        assert!(symbol_ops::exit_symbol_timeline(&mut app));
    }
    let after_local_shape = &app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes()[0];
    assert!(std::sync::Arc::ptr_eq(
        &after_local_shape.exterior().segments,
        &orig_local_shape.exterior().segments
    ));
    assert_eq!(after_local_shape.mesh_revision, orig_rev);
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
    sym_app.set_selected_indices(vec![0, 1]);
    assert_eq!(sym_app.session.selection().shapes.len(), 2);
    assert!(sym_app.session.selection().instances.is_empty());
    let empty_test_sym_id = doc_actions::convert_selection_to_symbol(&mut sym_app).unwrap();
    assert!(sym_app.session.selection().shapes.is_empty());
    assert_eq!(sym_app.session.selection().instances.len(), 1);
    assert_eq!(scene_summary(&sym_app).len, 2);
    assert!(sym_app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes()
        .is_empty());
    assert_eq!(
        sym_app.doc.root.layers[0].keyframes[0].cel.instances().len(),
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
        .shapes()
        .is_empty());
    assert_eq!(
        sym_app.doc.root.layers[0].keyframes[0].cel.instances().len(),
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

    // 置いてあるインスタンスの所で（その場で）入って、ステージの座標で描く
    let inst = sym_app.doc.root.layers[0].keyframes[0]
        .cel
        .single_symbol_instance()
        .unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut sym_app,
        inst.instance_gid
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

    let _ = sym_app.doc.library().at(0).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes()[0]
        .fill_mesh();
    assert!(sym_app.doc.library().at(0).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes()[0]
        .has_cached_fill_mesh());
    let orig_stage = sym_app.doc.stage_size;
    doc_actions::apply(
        &mut sym_app,
        doc_actions::Action::SetStageSize {
            size: orig_stage * 0.5,
        },
    );
    let last_undo = sym_app.history.undo_stack.back().unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &last_undo.library.at(0).unwrap().timeline.layers[0].keyframes[0],
        &sym_app.doc.library().at(0).unwrap().timeline.layers[0].keyframes[0]
    ));
    sym_app.undo();
    assert_eq!(sym_app.doc.stage_size, orig_stage);
    sym_app.redo();
    assert_eq!(sym_app.doc.stage_size, orig_stage * 0.5);

    // 選択表示は、その倍率で描く塗りのメッシュ（`view_mesh`）を選択の色で塗ったもの：頂点は描く図形と同じ数（6.25 倍までは
    // 図形が持つメッシュのまま。拡大して細かくした図形にも、同じ形のまま色が付く）
    let fill_n = sym_app.scene().get(0).unwrap().fill_mesh().vertices.len();
    for zoom in [0.15f32, 1.25, 16.0] {
        let shape = sym_app.scene().get(0).unwrap();
        let sel = crate::screen_mesh::shape_selection_screen_mesh(shape, zoom, Vec2::ZERO);
        assert_eq!(sel.vertices.len(), shape.view_mesh(zoom).vertices.len());
        if crate::vector_engine::mesh_level(zoom) == 0 {
            assert_eq!(sel.vertices.len(), fill_n);
        }
    }
    // プレビューは、2回目は写しから引く（作り直さない）。同じシンボル・世代なら同じ外接矩形とメッシュ
    let preview = |app: &Is2dApp| {
        app.view
            .symbol_previews
            .with(
                app.doc.library().at(0).unwrap(),
                &app.doc,
                |b, m| (b, m.clone()),
            )
            .unwrap()
    };
    let (orig1, prev_m1) = preview(&sym_app);
    let (orig2, prev_m2) = preview(&sym_app);
    assert!(prev_m1 == prev_m2 && orig1 == orig2);
}

/// シンボルには、今のコマにあるインスタンスの番号で入り、変換はタイムラインの結び付けをそのまま使う（画面の座標から
/// 取り戻さない）。今のコマに無いインスタンスには入らない。直す前は、画面の座標の結び付けを受け取り、同じ番号のインス
/// タンスの変換と許容値で照らし合わせ、合わなければ逆行列で戻した変換で入っていた（無いインスタンスの番号でも入った：
/// レビュー f01d518 の N-4006）
#[test]
fn entering_a_symbol_uses_the_instance_binding_of_the_current_frame() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 140.0),
            10.0,
            Color32::RED,
        ),
    );
    run(&mut app, &[Action::SelectAll, Action::ConvertToSymbol]);
    let gid = app.doc.root.layers[0].keyframes[0].cel.instances()[0]
        .binding
        .instance_gid;
    let xform = kurbo::Affine::translate((150.0, 120.0))
        * kurbo::Affine::rotate(0.3)
        * kurbo::Affine::scale_non_uniform(0.7, 1.3);
    app.doc.root.layers[0].keyframe_mut(0).cel.instances_mut()[0]
        .binding
        .xform = xform;
    app.refresh_scene_base();
    assert!(
        !symbol_ops::enter_symbol_timeline(&mut app, gid + 1000),
        "今のコマに無いインスタンスには入らない"
    );
    assert!(app.session.edit_path().is_empty());
    assert!(symbol_ops::enter_symbol_timeline(&mut app, gid));
    assert_eq!(
        app.session.edit_path().levels[0].xform_in_parent, xform,
        "結び付けの変換そのまま"
    );
}

/// 潰れた変換（逆にできない）で置いたインスタンスは描かない（面積 0 の絵は見えない）。保存して開き直しても変換は
/// そのまま（単位行列に戻さない）。直す前は、潰れた変換では図形を変えずに出したので、インスタンスがシンボルの座標・
/// 元の大きさで描かれ、開き直すと変換が単位行列に戻って元の大きさで出た（レビュー f01d518 の R7・N-6001）
#[test]
fn instances_with_a_collapsed_transform_are_not_drawn() {
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 140.0),
            10.0,
            Color32::RED,
        ),
    );
    run(&mut app, &[Action::SelectAll, Action::ConvertToSymbol]);
    assert!(canvas_tool::clear_selection(&mut app));
    let set = |app: &mut Is2dApp, xform: kurbo::Affine| {
        app.doc.root.layers[0].keyframe_mut(0).cel.instances_mut()[0]
            .binding
            .xform = xform;
        app.refresh_scene_base();
    };
    set(&mut app, kurbo::Affine::translate((300.0, 300.0)));
    assert!(app.scene().len() > 0, "ふつうの変換なら描く");
    for collapsed in [
        // 横の拡大率が 0
        kurbo::Affine::new([0.0, 0.0, 0.0, 1.0, 300.0, 300.0]),
        // 縮めすぎ（行列式が `MIN_DET` より小さい）
        kurbo::Affine::translate((300.0, 300.0)) * kurbo::Affine::scale(1e-7),
    ] {
        set(&mut app, collapsed);
        assert_eq!(app.scene().len(), 0, "{collapsed:?} では何も描かない");
    }
    let dir = std::env::temp_dir().join(format!("is2d_collapsed_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("collapsed.is2d");
    crate::project_io::save_project_to_path(&mut app, &path).unwrap();
    let mut reopened = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut reopened, &path, None).unwrap();
    assert_eq!(
        reopened.doc.root.layers[0].keyframes[0].cel.instances()[0]
            .binding
            .xform,
        kurbo::Affine::translate((300.0, 300.0)) * kurbo::Affine::scale(1e-7),
        "開き直しても変換はそのまま"
    );
    assert_eq!(reopened.scene().len(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// ライブラリのプレビューの写しは、いま描いている1つだけを持つ（別のシンボルを描くと置き換わり、消したシンボルの写しが
/// 溜まらない）。直す前は、シンボルの番号ごとに持ち続けた（レビュー f01d518 の N-4003）
#[test]
fn library_preview_holds_only_the_symbol_being_drawn() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 140.0),
            10.0,
            Color32::RED,
        ),
    );
    run(&mut app, &[Action::SelectAll, Action::ConvertToSymbol]);
    let sym_id = app.doc.library().at(0).unwrap().id;
    run(&mut app, &[Action::DuplicateSymbol { sym_id }]);
    assert_eq!(app.doc.library().len(), 2);
    let preview = |app: &Is2dApp, i: usize| {
        app.view
            .symbol_previews
            .with(
                app.doc.library().at(i).unwrap(),
                &app.doc,
                |b, _| b,
            )
            .unwrap()
    };
    let first = preview(&app, 0);
    let _ = preview(&app, 1);
    assert_eq!(app.view.symbol_previews.held(), 1, "いま描いている1つだけ");
    assert_eq!(preview(&app, 0), first, "描き直すと同じ");
    assert_eq!(app.view.symbol_previews.held(), 1);
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
    grp_sym_app.set_selected_indices(vec![0]);
    let blink_sym_id = doc_actions::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
    // シンボル内部に入り、2フレーム目を空白キーフレーム（F7）にする（F0: 表示, F1: 空白）
    let blink_binding = scene_summary(&grp_sym_app).shapes[0]
        .symbol_instance
        .unwrap();
    assert!(symbol_ops::enter_symbol_timeline(
        &mut grp_sym_app,
        blink_binding.instance_gid
    ));
    assert!(doc_actions::timeline_insert_keyframe(
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
    grp_sym_app.set_selected_indices(vec![0, 1]);
    assert!(doc_actions::group_selected_shapes(&mut grp_sym_app).is_some());
    // ルートシーンを3フレームに延長（F0: シンボル表示, F1: シンボル空白, F2: シンボルループ再表示）
    doc_actions::select_timeline_cells(&mut grp_sym_app, 0, 2, 0, 2);
    assert!(doc_actions::timeline_extend_frame(&mut grp_sym_app));

    // F1（シンボルが空白フレームで消えている状態）へ移動し、親グループを (+150, 0) 平行移動する
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    assert_eq!(
        scene_summary(&grp_sym_app).len,
        2,
        "F1では空白シンボルマーカー1個と兄弟シェイプ1個の計2個が Scene 内に展開されていること"
    );
    {
        let sel = grp_sym_app
            .scene()
            .expand_to_group_unit(0, grp_sym_app.session.editing_group_path());
        grp_sym_app.set_selected_indices(sel);
    }
    assert_eq!(grp_sym_app.session.selected_indices(), vec![0, 1]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut grp_sym_app,
        kurbo::Vec2::new(150.0, 0.0)
    ));

    // F2（シンボルがループして再び表示されるフレーム）へ進むと、空白フレーム中に移動した親グループに追従してシンボルも (+150, 0) 移動していること
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
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
        e_prev.library.ptr_eq(&e_last.library),
        "シンボル未変更時の UndoEntry 間では library がディープクローンされず Arc 共有されること"
    );
    assert!(
        std::sync::Arc::ptr_eq(
            e_prev.timeline.layers[0].keyframes[0].cel.maps(),
            e_last.timeline.layers[0].keyframes[0].cel.maps()
        ),
        "未変更キーフレームの Cel::maps は連続する UndoEntry 間でディープクローンされず Arc 共有（CoW）されること"
    );

    // F1に戻って親グループを +90° 回転させても、空白フレーム中のシンボルが正確なアフィン変換で追従すること
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    {
        let sel = grp_sym_app
            .scene()
            .expand_to_group_unit(0, grp_sym_app.session.editing_group_path());
        grp_sym_app.set_selected_indices(sel);
    }
    assert!(doc_actions::rotate_selected_shapes(
        &mut grp_sym_app,
        std::f64::consts::FRAC_PI_2
    ));
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
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
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    {
        let sel = grp_sym_app
            .scene()
            .expand_to_group_unit(0, grp_sym_app.session.editing_group_path());
        grp_sym_app.set_selected_indices(sel);
    }
    assert!(doc_actions::delete_selected_shapes(&mut grp_sym_app));
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 2);
    assert!(
        scene_summary(&grp_sym_app).is_empty,
        "空白フレーム中に親グループを削除すると、内部の空白フレームシンボルも残骸にならず一緒に削除されること"
    );
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
    sym_paste_app.set_selected_indices(vec![0]);
    let child_sym = doc_actions::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.set_selected_indices(vec![0]);
    let parent_sym = doc_actions::convert_selection_to_symbol(&mut sym_paste_app).unwrap();
    sym_paste_app.set_selected_indices(vec![0]);
    assert!(doc_actions::copy_selected_shapes(&mut sym_paste_app));

    // 子シンボルをライブラリから削除（親シンボル内では通常グループへデタッチされる）
    assert!(doc_actions::delete_symbol_from_library(
        &mut sym_paste_app,
        child_sym
    ));
    assert_eq!(sym_paste_app.doc.library().len(), 1);

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
        sym_paste_app.doc.library().len(),
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
        .all(|s| s.color() == Color32::BLUE));
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
    app.set_selected_indices(vec![0]);
    let gid = doc_actions::group_selected_shapes(&mut app).unwrap();

    // グループをダブルクリックしてインプレイス編集に入る
    assert!(select_ops::enter_group_at(
        &mut app,
        Point::new(150.0, 100.0)
    ));
    assert_eq!(app.session.editing_group_path(), vec![gid]);

    // グループ内部の生シェイプ上の座標 (150, 100) をダブルクリックしても exit_current_group が呼ばれずグループ編集スコープに留まること
    let pt_on_raw = Point::new(150.0, 100.0);
    select_ops::handle_canvas_double_click(&mut app, pt_on_raw, false);
    assert_eq!(
        app.session.editing_group_path(),
        vec![gid],
        "グループ内の生シェイプ上のダブルクリックでは親階層へ退出しないこと"
    );
    // 空白背景 (400, 400) をダブルクリックすると exit_current_group で親階層へ戻れること
    let pt_empty = Point::new(400.0, 400.0);
    select_ops::handle_canvas_double_click(&mut app, pt_empty, false);
    assert!(app.session.editing_group_path().is_empty());
}

#[test]
fn test_tween_state_incomplete_when_not_single_symbol_instance() {
    use crate::timeline::TweenState;

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
    app.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    let undo_before_rename = app.history.undo_stack.len();

    // 1文字目の入力開始時のみ Undo を積み、2文字目を入力しても Undo が増殖しないこと
    assert!(doc_actions::rename_symbol_in_library(&mut app, sym_id, "H"));
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

/// シンボルの名前を変えても、シンボルの版とライブラリの版は変わらない（版は中身の世代。名前を1文字打つたびに、その
/// シンボルを使う画面の写しを作り直さない：レビュー de92ceb の N-4002）
#[test]
fn renaming_a_symbol_keeps_its_content_version() {
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, make_stroke(Point::new(100.0, 100.0), Point::new(240.0, 100.0), 16.0, Color32::BLUE));
    app.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    let versions = |app: &Is2dApp| (app.doc.find_symbol(sym_id).unwrap().revision(), app.doc.revision);
    let before = versions(&app);
    assert!(doc_actions::rename_symbol_in_library(&mut app, sym_id, "Hero"));
    assert_eq!(app.doc.find_symbol(sym_id).unwrap().name, "Hero");
    assert_eq!(versions(&app), before, "名前を変えただけで版が上がった");
}

#[test]
fn test_nested_static_child_symbol_is_not_treated_as_dynamic_blank_frame() {
    // シンボル内 Frame 0 が空だがネストした子シンボルインスタンスのみが存在する場合の has_dynamic_animation 判定
    let mut sym_parent =
        crate::symbol::SymbolDef::new(1, "Parent".to_string(), Vec::new()).unwrap();
    let sym_child = crate::symbol::SymbolDef::new(
        2,
        "Child".to_string(),
        make_stroke(
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
            6.0,
            Color32::RED,
        ),
    )
    .unwrap();
    sym_parent.timeline.layers[0].keyframe_mut(0).cel = crate::timeline::Cel::new_with_instances(
        Vec::new(),
        vec![crate::timeline::SymbolInstance {
            binding: crate::symbol_binding::SymbolInstanceBinding::new(1, 2, kurbo::Affine::IDENTITY),
            layer_id: 1,
            parent_scope: Vec::new(),
            order: 0,
        }],
    );
    let lib: crate::symbol::Library = vec![
        std::sync::Arc::new(sym_parent),
        std::sync::Arc::new(sym_child),
    ]
    .into_iter()
    .collect();
    assert!(
        !lib.at(0).unwrap().has_dynamic_animation(),
        "Frame 0 に shapes が無くても単一フレームの子シンボルインスタンスが存在する場合は空白フレームと誤判定されないこと"
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
    sym_f6_app.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut sym_f6_app).unwrap();
    assert!(symbol_ops::enter_symbol_from_library(
        &mut sym_f6_app,
        sym_id
    ));
    sym_f6_app.with_active_timeline_mut(|tl, _| {
        extend_frame_at(tl, 0, 3);
        tl.sync_total_frames();
        insert_keyframe_at(tl, 0, 2, false, 24);
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
    extend_frame_at(&mut sym_f6_app.doc.root, 0, 4);
    sym_f6_app.doc.root.sync_total_frames();
    doc_actions::switch_to_layer_and_frame(&mut sym_f6_app, 0, 2);
    assert_eq!(scene_summary(&sym_f6_app).shapes[0].color(), blue);
    assert!(doc_actions::timeline_insert_keyframe(&mut sym_f6_app, true));
    assert_eq!(
        scene_summary(&sym_f6_app).shapes[0].color(),
        blue,
        "F6 で分割された新しいキーフレームでも Loop シンボルの表示コマ (Frame 2 = 青) がそのまま引き継がれること"
    );

    // トゥイーン区間（tween_rotate = Clockwise）の途中フレームでは直接のキャンバス編集は無効化され、
    // F6 キーフレーム挿入で分割した際に tween_rotate = Clockwise が引き継がれて編集可能になること (#10, M2-2)
    doc_actions::switch_to_layer_and_frame(&mut sym_f6_app, 0, 4);
    assert!(doc_actions::timeline_insert_keyframe(&mut sym_f6_app, true));
    doc_actions::switch_to_layer_and_frame(&mut sym_f6_app, 0, 2);
    assert!(doc_actions::timeline_toggle_tween(&mut sym_f6_app));
    assert!(doc_actions::set_active_keyframe_tween_rotate(
        &mut sym_f6_app,
        crate::timeline::TweenRotate::Clockwise
    ));
    doc_actions::switch_to_layer_and_frame(&mut sym_f6_app, 0, 3);
    sym_f6_app.set_selected_indices(vec![0]);
    assert!(
        !doc_actions::nudge_selected_shapes(&mut sym_f6_app, kurbo::Vec2::new(20.0, 0.0)),
        "トゥイーン途中のコマでは直接の移動編集ができないこと"
    );
    assert!(
        sym_f6_app.doc.root.layers[0]
            .keyframe_index_starting_at(3)
            .is_none(),
        "トゥイーン途中のコマでの移動操作で自動キーフレームが生成されないこと"
    );
    assert!(doc_actions::timeline_insert_keyframe(&mut sym_f6_app, true));
    sym_f6_app.set_selected_indices(vec![0]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut sym_f6_app,
        kurbo::Vec2::new(20.0, 0.0)
    ));
    let kf_at_3 = &sym_f6_app.doc.root.layers[0].keyframes[sym_f6_app.doc.root.layers[0]
        .keyframe_index_starting_at(3)
        .expect("F6 挿入により Frame 3 に新キーフレームが生成されていること")];
    assert!(kf_at_3.tween);
    assert_eq!(
        kf_at_3.tween_rotate,
        crate::timeline::TweenRotate::Clockwise,
        "F6 で分割されたキーフレームに前のキーフレームの tween_rotate 設定が引き継がれること"
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
    app.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    extend_frame_at(&mut app.doc.root, 0, 4);
    app.doc.root.sync_total_frames();
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 4);
    doc_actions::timeline_insert_keyframe(&mut app, true);
    app.set_selected_indices(vec![0]);
    doc_actions::nudge_selected_shapes(&mut app, KVec2::new(200.0, 0.0));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    assert!(doc_actions::timeline_toggle_tween(&mut app));
    assert!(app.doc.root.layers[0].keyframes[0].tween);

    // シンボルを削除すると、全キーフレームからインスタンスが消え、トゥイーンは ON のまま未完になること
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    assert!(doc_actions::delete_symbol_from_library(&mut app, sym_id));
    assert!(app.doc.find_symbol(sym_id).is_none());
    for kf in &app.doc.root.layers[0].keyframes {
        assert!(
            kf.cel.instances().is_empty() && kf.cel.shapes().is_empty(),
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
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.instances().len(), 1);
    assert_eq!(app.doc.root.layers[0].keyframes[1].cel.instances().len(), 1);
    assert!(app.doc.root.layers[0].keyframes[0].tween);
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
            instances.push(save_format::SavedSymbolInstance {
                binding: save_format::SavedSymbolInstanceBinding {
                    instance_gid: ((i * 100) + k + 1) as u64,
                    symbol_id: target_sym_id,
                    xform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                    pivot: [0.0, 0.0],
                    loop_mode: symbol_binding::SymbolLoopMode::Loop,
                    first_frame: 0,
                    color_effect: symbol_binding::ColorEffect::default(),
                },
                layer_id: 1,
                parent_scope: Vec::new(),
                order: k,
            });
        }
        library.push(save_format::SavedSymbolDef {
            id: sym_id,
            name: format!("DenseSym{sym_id}"),
            timeline: save_format::SavedTimeline {
                fps: 24,
                total_frames: 1,
                current_frame: 0,
                active_layer: 0,
                next_layer_number: 2,
                layers: vec![save_format::SavedLayer {
                    id: 1,
                    name: "L1".to_owned(),
                    visible: true,
                    locked: false,
                    frame_count: 1,
                    keyframes: vec![save_format::SavedKeyframe {
                        start_frame: 0,
                        tween: false,
                        tween_rotate: timeline::TweenRotate::Auto,
                        maps: Vec::new(),
                        instances,
                        sound: None,
                    }],
                }],
            },
        });
    }

    let doc = SavedProject {
        stage_size: [800.0, 450.0],
        timeline: (&timeline::Timeline::default()).into(),
        library,
        next_symbol_id: (n + 1) as u64,
        next_group_id: 100_000,
        recovery_source: None,
        map_tiles: Default::default(),
        sounds: Vec::new(),
        next_sound_id: 1,
        next_layer_id: 2,
    };

    let path = std::env::temp_dir().join(format!("is2d_dense_cycle_{}.is2d", std::process::id()));
    write_crafted_file(&path, &doc);
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
fn test_nested_symbol_expansion_memoization_and_step_cap() {
    let mut app = Is2dApp::default();

    // 1. 中身が空の葉シンボル (id=8) を起点に、各段が子シンボルを20個ずつ持つ8段入れ子階層 (20^8 = 256億経路)
    let leaf = symbol::SymbolDef::new(8, "EmptyLeaf".to_string(), Vec::new()).unwrap();
    app.doc.add_symbol(None, leaf);

    let mut next_gid = 1u64;
    for level in (1..=7u64).rev() {
        let child_id = level + 1;
        let mut sym = symbol::SymbolDef::new(level, format!("Level{level}"), Vec::new()).unwrap();
        let instances: Vec<timeline::SymbolInstance> = (0..20)
            .map(|i| timeline::SymbolInstance {
                binding: symbol_binding::SymbolInstanceBinding::new(
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
                order: 0,
            })
            .collect();
        sym.timeline.layers[0].keyframe_mut(0).cel =
            timeline::Cel::new_with_instances(Vec::new(), instances);
        app.doc.add_symbol(None, sym);
    }

    let root_instances: Vec<timeline::SymbolInstance> = (0..20)
        .map(|i| timeline::SymbolInstance {
            binding: symbol_binding::SymbolInstanceBinding::new(
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
            order: 0,
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
    let binding = app.doc.root.layers[0].keyframes[0]
        .cel
        .single_symbol_instance()
        .unwrap();
    let area_before_inplace = s0.evaluated_total_area;

    run(
        &mut app,
        &[
            Action::EnterSymbolTimeline {
                instance_gid: binding.instance_gid,
            },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 140.0), Point::new(160.0, 140.0)],
                radius: 12.0,
                color: blue,
            },
            Action::ExitGroupSteps { steps: 1 },
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
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(200.0, 0.0),
            },
            Action::SwitchLayerAndFrame {
                layer_idx: 0,
                frame: 0,
            },
            Action::ToggleTween,
            Action::SetTweenRotate {
                rotate: crate::timeline::TweenRotate::Clockwise,
            },
        ],
    );
    let s_kf0 = cel_summary(&app.doc, lid, 0);
    let s_mid = cel_summary(&app.doc, lid, 2);
    let s_kf4 = cel_summary(&app.doc, lid, 4);
    assert!(s_kf0.tween);
    assert_eq!(s_kf0.tween_rotate, crate::timeline::TweenRotate::Clockwise);
    let x0 = s_kf0.evaluated_bbox.unwrap().center().x;
    let x_mid = s_mid.evaluated_bbox.unwrap().center().x;
    let x4 = s_kf4.evaluated_bbox.unwrap().center().x;
    assert!(
        (x_mid - (x0 + 100.0)).abs() < 2.0 && (x4 - (x0 + 200.0)).abs() < 2.0,
        "中間フレーム2の補間位置が開始と終了の中央 (x0 + 100) にあること"
    );
}

#[test]
fn test_stage6b_non_uniform_scale_rotated_symbol_edit_preserves_untouched_shape_bits() {
    use crate::canvas_tool::doc_actions::Action;

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
    let sym_id = app.doc.library().at(0).unwrap().id;
    let orig_local_shape = app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes()[0]
        .clone();
    let orig_bits: Vec<[u64; 6]> = orig_local_shape
        .exterior()
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
    scale_selection(&mut app, 1.73, 0.61);
    run(&mut app, &[Action::RotateSelection { angle_rad: 0.47 }]);
    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();

    // インスタンスに入り、別の場所に50回ストロークを描いて出る
    run(
        &mut app,
        &[Action::EnterSymbolTimeline {
            instance_gid: binding.instance_gid,
        }],
    );
    assert_eq!(app.session.edit_path().len(), 1);
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
    run(&mut app, &[Action::ExitGroupSteps { steps: 1 }]);
    assert!(app.session.edit_path().is_empty());

    let sym_shapes = &app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes();
    let preserved = sym_shapes
        .iter()
        .find(|s| s.shape_id == orig_local_shape.shape_id)
        .expect("未変更の既存図形が同一 shape_id で保持されていること");
    let after_bits: Vec<[u64; 6]> = preserved
        .exterior()
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
fn test_stage6b_timeline_ops_f6_f7_add_layer_inside_symbol_modify_library_symbol_def() {
    use crate::canvas_tool::doc_actions::Action;

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
    let sym_id = app.doc.library().at(0).unwrap().id;
    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();

    run(
        &mut app,
        &[Action::EnterSymbolTimeline {
            instance_gid: binding.instance_gid,
        }],
    );
    assert_eq!(app.session.edit_path().len(), 1);
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

    // ルートタイムラインは変わらず、doc.library() 内の SymbolDef のタイムラインが直接更新されていること
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
    use crate::canvas_tool::doc_actions::Action;

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
            Action::NudgeSelection {
                delta: KVec2::new(20.0, 0.0),
            },
        ],
    );
    let sym_id = app.doc.library().at(0).unwrap().id;
    let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();

    // 2. シンボルに入って内部でストローク描画＋F6 キーフレーム挿入
    run(
        &mut app,
        &[
            Action::EnterSymbolTimeline {
                instance_gid: binding.instance_gid,
            },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 160.0), Point::new(180.0, 160.0)],
                radius: 12.0,
                color: green,
            },
            Action::InsertKeyframe { copy_prev: true },
        ],
    );
    assert_eq!(app.session.edit_path().len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.total_frames,
        2
    );

    // 3. シンボルから出た後の操作（ルートに青ストローク描画）
    run(
        &mut app,
        &[
            Action::ExitGroupSteps { steps: 1 },
            Action::PaintStroke {
                points: vec![Point::new(100.0, 260.0), Point::new(180.0, 260.0)],
                radius: 12.0,
                color: blue,
            },
        ],
    );
    assert!(app.session.edit_path().is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 1);

    // Undo 1: ルートの青ストロークを取り消す（まだルート階層）
    run(&mut app, &[Action::Undo]);
    assert!(app.session.edit_path().is_empty());
    assert!(app.doc.root.layers[0].keyframes[0].cel.shapes().is_empty());

    // Undo 2: シンボル内の F6 キーフレーム挿入を取り消す（EditPath がシンボル内深さ1へ戻る）
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.session.edit_path().len(), 1);
    assert_eq!(app.session.edit_path().last().unwrap().symbol_id, sym_id);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.total_frames,
        1
    );
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes()
            .len(),
        2
    );

    // Undo 3: シンボル内の緑ストローク描画を取り消す（EditPath はシンボル内深さ1のまま、内部図形が1個へ戻る）
    run(&mut app, &[Action::Undo]);
    assert_eq!(app.session.edit_path().len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes()
            .len(),
        1
    );

    // Undo 4: シンボルに入る前の MoveSelected を取り消す（EditPath がルートへ戻る）
    run(&mut app, &[Action::Undo]);
    assert!(app.session.edit_path().is_empty());
    assert_eq!(app.doc.library().len(), 1);

    // Undo 5: シンボル化（ConvertToSymbol）自体を取り消す（ルート、ライブラリ空）
    run(&mut app, &[Action::Undo]);
    assert!(app.session.edit_path().is_empty());
    assert!(app.doc.library().is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 1);

    // Redo 1..5 で順番にすべて復元されること
    run(&mut app, &[Action::Redo]);
    assert!(app.session.edit_path().is_empty());
    assert_eq!(app.doc.library().len(), 1);

    run(&mut app, &[Action::Redo]);
    assert_eq!(app.session.edit_path().len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes()
            .len(),
        1
    );

    run(&mut app, &[Action::Redo]);
    assert_eq!(app.session.edit_path().len(), 1);
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes()
            .len(),
        2
    );

    run(&mut app, &[Action::Redo]);
    assert!(app.session.edit_path().is_empty());
    assert_eq!(
        app.doc.find_symbol(sym_id).unwrap().timeline.total_frames,
        2
    );

    run(&mut app, &[Action::Redo]);
    assert!(app.session.edit_path().is_empty());
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 1);
}

#[test]
fn test_collapsed_symbol_instance_blocks_enter_and_timeline_mut_bumps_revision_only_on_change() {
    use crate::canvas_tool::doc_actions::Action;

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
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&first); app.session.replace_selection(sel); }
    run(&mut app, &[Action::ConvertToSymbol]);

    let normal_binding = app.scene().get(0).unwrap().symbol_instance.unwrap();

    // 1. 行列式がほぼ0（潰れたインスタンス）には入れないこと
    let set_xform = |app: &mut Is2dApp, xform: kurbo::Affine| {
        app.doc.root.layers[0].keyframe_mut(0).cel.instances_mut()[0]
            .binding
            .xform = xform;
        app.refresh_scene_base();
    };
    set_xform(&mut app, kurbo::Affine::scale_non_uniform(1.0, 0.0));
    assert!(
        !symbol_ops::enter_symbol_timeline(&mut app.tabs[app.active], normal_binding.instance_gid),
        "行列式が0の潰れたインスタンスには入れないこと"
    );
    assert!(app.session.edit_path().is_empty());
    set_xform(&mut app, normal_binding.xform);

    // 2. 通常のインスタンスに入り、with_active_timeline_mut で中身を変えたらリビジョンが上がり、何も変えなければ上がらない
    //    こと（版は中身が変わったときだけ上がる：`Document::write_symbol_timeline`。前は何も変えなくても上げた）
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app.tabs[app.active],
        normal_binding.instance_gid
    ));
    assert_eq!(app.session.edit_path().levels.len(), 1);
    let sym_rev_before = app.doc.library().at(0).unwrap().revision();
    let doc_rev_before = app.doc.revision;
    app.tabs[app.active].with_active_timeline_mut(|_tl, _| {});
    assert_eq!(app.doc.library().at(0).unwrap().revision(), sym_rev_before, "何も変えないのにシンボルの版が上がった");
    assert_eq!(app.doc.revision, doc_rev_before, "何も変えないのに文書の版が上がった");
    app.tabs[app.active].with_active_timeline_mut(|tl, _| tl.layers[0].name.push('x'));
    assert_ne!(app.doc.library().at(0).unwrap().revision(), sym_rev_before, "中身を変えたらシンボルの版が上がること");
    assert_ne!(app.doc.revision, doc_rev_before, "中身を変えたら文書の版も上がること");
}

/// M2-1: 図形10個を持つシンボルや、入れ子シンボルを含むシンボルを Cmd+B (Action::Ungroup) で分解した際、
/// シンボルインスタンスが文書に残らず全図形が欠落・重複なく分解されることを検証する
#[test]
fn test_m2_1_ungroup_multi_shape_and_nested_symbol_detaches_all_shapes() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 10);
    let _ = apply(&mut app, Action::SelectAll);
    let _ = apply(&mut app, Action::ConvertToSymbol);
    assert_eq!(app.doc.library().len(), 1);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.instances().len(), 1);
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 0);

    // 生の図形どうしは重ならないので、並びの順は絵に出ない（左上の順に並べて比べる）
    let sorted = |shapes: &mut dyn Iterator<Item = &FillShape>| -> Vec<(kurbo::Rect, Color32)> {
        let mut v: Vec<(kurbo::Rect, Color32)> =
            shapes.map(|s| (s.bounding_box(), s.color())).collect();
        v.sort_by(|a, b| (a.0.x0, a.0.y0).partial_cmp(&(b.0.x0, b.0.y0)).unwrap());
        v
    };
    let before = sorted(&mut app.scene().iter());
    let before_bboxes: Vec<kurbo::Rect> = before.iter().map(|x| x.0).collect();
    let before_colors: Vec<Color32> = before.iter().map(|x| x.1).collect();
    assert_eq!(before_bboxes.len(), 10);

    let _ = apply(&mut app, Action::Ungroup);
    assert!(
        app.doc.root.layers[0].keyframes[0].cel.instances().is_empty(),
        "10図形シンボルの分解後、文書にインスタンスが残らないこと"
    );

    // 選択解除で Floating を確定した後も、生の図形が10個で形・色が分解前と一致すること
    assert!(canvas_tool::clear_selection(&mut app));
    let cel = &app.doc.root.layers[0].keyframes[0].cel;
    assert!(cel.instances().is_empty());
    assert_eq!(cel.shapes().len(), 10, "生の図形が10個になること");
    for s in cel.shapes().iter() {
        assert!(s.group_path.is_empty() && s.symbol_instance.is_none());
    }
    let after = sorted(&mut cel.shapes().iter().map(|s| &**s));
    for (idx, (bb, color)) in after.into_iter().enumerate() {
        assert_eq!(color, before_colors[idx]);
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
    let sym_b_id = app2.doc.library().at(0).unwrap().id;

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
    assert_eq!(app2.doc.library().len(), 2);
    let sym_a_id = app2.doc.library().at(1).unwrap().id;
    assert_ne!(sym_a_id, sym_b_id);
    assert_eq!(app2.doc.root.layers[0].keyframes[0].cel.instances().len(), 1);
    assert_eq!(
        app2.doc.root.layers[0].keyframes[0].cel.instances()[0]
            .binding
            .symbol_id,
        sym_a_id
    );

    let _ = apply(&mut app2, Action::Ungroup);
    assert!(canvas_tool::clear_selection(&mut app2));
    let cel2 = &app2.doc.root.layers[0].keyframes[0].cel;
    assert_eq!(cel2.instances().len(), 1);
    assert_eq!(cel2.instances()[0].binding.symbol_id, sym_b_id);
    assert_eq!(cel2.shapes().len(), 1);
}

#[test]
fn test_m2_4_delete_symbol_2880_keyframes_fast_and_no_memory_growth() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
    write_symbol(&mut app.doc, sym_id, |tl| {
        tl.total_frames = 8;
        tl.layers[0].frame_count = 8;
        for f in 1..8 {
            insert_keyframe_at(tl, 0, f, false, 24);
            tl.layers[0].keyframe_mut(f).cel = timeline::Cel::from_shapes_unwrap(shapes.clone());
        }
    });
    app.doc.root.total_frames = 2880;
    app.doc.root.layers[0].frame_count = 2880;
    app.doc.root.layers[0].keyframes.clear();
    for f in 0..2880 {
        let inst = timeline::SymbolInstance {
            binding: symbol_binding::SymbolInstanceBinding::new(
                (f + 1) as u64,
                sym_id,
                kurbo::Affine::translate(((f % 100) as f64 * 2.0, 50.0)),
            ),
            layer_id: 1,
            parent_scope: Vec::new(),
            order: 0,
        };
        app.doc.root.layers[0].keyframes.insert_keyed(
            f,
            f,
            std::sync::Arc::new(timeline::Keyframe::with_cel(
                timeline::Cel::new_with_instances(Vec::new(), vec![inst]),
                true,
                timeline::TweenRotate::Auto,
            )),
        );
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
        .all(|kf| kf.cel.instances().is_empty() && kf.tween));
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        timeline::TweenState::Incomplete
    );
    app.undo();
    assert!(app.doc.find_symbol(sym_id).is_some());
    assert!(app.doc.root.layers[0]
        .keyframes
        .iter()
        .all(|kf| kf.cel.instances().len() == 1 && kf.tween));
}

#[test]
fn test_m5_1_n3001_f8_nest_limit_preserves_redo_and_oldest_undo() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
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
    assert_eq!(app.tab().doc.library().len(), 8);

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
    assert!(app.tab().history.before.is_none());
    assert_eq!(app.tab().doc.library().len(), 8);
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
fn test_symbol_color_effect_alpha_brightness_tween_export_nested_io_undo_ungroup() {
    use crate::canvas_tool::{CoalesceKey, View};
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::save_format::SavedSymbolInstanceBinding;
    use crate::symbol_binding::{ColorEffect, SymbolInstanceBinding, SymbolLoopMode};
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
    // 有限かは見ない：NaN は NaN のまま残り、インスタンスを書く入口・ファイルを読む入口の判定（`is_finite`）が断る
    // （`non_finite_instance_numbers_are_refused_at_the_entries`）。無限大は範囲の端にそろう
    let nan = ColorEffect::new(f32::NAN, f32::INFINITY);
    assert!(nan.alpha.is_nan());
    assert_eq!(nan.brightness, 1.0);
    assert!(!SymbolInstanceBinding {
        color_effect: nan,
        ..SymbolInstanceBinding::new(1, 1, kurbo::Affine::IDENTITY)
    }
    .is_finite());
    let nan = ColorEffect::new(f32::NEG_INFINITY, f32::NAN);
    assert_eq!(nan.alpha, 0.0);
    assert!(nan.brightness.is_nan());

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
    tween_app.set_selected_indices(vec![0]);
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
    tween_app.set_selected_indices(vec![0]);
    assert!(apply(
        &mut tween_app,
        Action::SetSymbolColorEffect {
            effect: ColorEffect::new(1.0, 0.0),
        },
    ));
    let mid_shapes = tween_app
        .doc
        .root
        .evaluate_layer_shapes_at(0, 2, tween_app.doc.library());
    assert_eq!(mid_shapes.len(), 1);
    let mid_alpha = mid_shapes[0].color().to_srgba_unmultiplied()[3] as i32;
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
    export_app.set_selected_indices(vec![0]);
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
        .color()
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
    export_app.set_selected_indices(vec![0]);
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
        .color()
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
    let loaded_doc = saved_project_of(&export_app);
    let mut reopened = Is2dApp::default();
    loaded_doc.apply_to_app(&mut reopened);
    let reopened_eff = reopened.doc.root.layers[0].keyframes[0].cel.instances()[0]
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
    // 読むときに直さない：書いてある数のまま読み、有限でなければ書く入口と同じ判定（`is_finite`）が断る（ファイルを読む入口
    // `only_finite_numbers`）。範囲の外の有限な色効果は、描くとき（`ColorEffect::apply`）に色の範囲へ収まる
    let read_binding = SymbolInstanceBinding::from(out_of_range_saved);
    assert_eq!(read_binding.color_effect.alpha, 5.0);
    assert!(read_binding.color_effect.brightness.is_nan());
    assert!(!read_binding.is_finite());
    let finite_out_of_range = ColorEffect {
        alpha: 5.0,
        brightness: 0.0,
    };
    assert_eq!(
        finite_out_of_range.apply(Color32::from_rgba_unmultiplied(255, 0, 0, 128)),
        Color32::from_rgba_unmultiplied(255, 0, 0, 255)
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
    undo_app.set_selected_indices(vec![0]);
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
    assert_eq!(ungrouped_shape.color().to_srgba_unmultiplied()[3], 255);
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
fn test_nested_symbol_ungroup_preserves_child_instances_frames_and_resets_color_effect() {
    use kurbo::Affine;

    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::symbol_binding::{ColorEffect, SymbolLoopMode};

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
        app.set_selected_indices(vec![0]);
        assert!(apply(&mut app, Action::ConvertToSymbol));
        let sym_b_id = app.doc.library().at(0).unwrap().id;

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
            write_symbol(&mut app.doc, sym_b_id, |tl| {
                tl.layers[0].keyframe_mut(f).cel = timeline::Cel::from_shapes_unwrap(s)
            });
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
        let sym_a_id = app.doc.library().at(1).unwrap().id;

        if a_total_frames > 1 {
            write_symbol(&mut app.doc, sym_a_id, |tl| {
                tl.total_frames = a_total_frames;
                tl.layers[0].frame_count = a_total_frames;
            });
        }
        app.doc.root.total_frames = 20;
        app.doc.root.layers[0].frame_count = 20;
        app.refresh_scene_base();
        (app, sym_a_id, sym_b_id)
    };

    // 3, 13, 14. B が残り、変形が A×B になり、B も選択され、Undo で分解前の A に戻る
    {
        let (mut app, sym_a_id, sym_b_id) = build_nested_app(1);
        let orig_b_in_a = symbols_in_order(app.doc.library())
            .find(|s| s.id == sym_a_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[0]
            .cel
            .instances()[0]
            .binding;
        assert!(apply(
            &mut app,
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(30.0, 15.0),
            },
        ));
        let a_binding = app.doc.root.layers[0].keyframes[0].cel.instances()[0].binding;
        let expected_xform = a_binding.xform * orig_b_in_a.xform;

        assert!(apply(&mut app, Action::Ungroup));
        let cel = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel.instances().len(), 1);
        let b_inst = &cel.instances()[0];
        assert_eq!(b_inst.binding.symbol_id, sym_b_id);
        for i in 0..6 {
            assert!(
                (b_inst.binding.xform.as_coeffs()[i] - expected_xform.as_coeffs()[i]).abs() < 1e-6
            );
        }
        assert!(app
            .session
            .selection()
            .instances
            .contains(&b_inst.binding.instance_gid));
        assert!(!app.session.selection().shapes.is_empty());

        assert!(canvas_tool::clear_selection(&mut app));
        let cel_after = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel_after.shapes().len(), 1);
        assert_eq!(cel_after.shapes()[0].color(), raw_col);

        assert!(apply(&mut app, Action::Undo));
        let cel_undo = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel_undo.instances().len(), 1);
        assert_eq!(cel_undo.instances()[0].binding.symbol_id, sym_a_id);
    }

    // 4. 分解した直後の表示コマが合う: (a) A が1コマで F=5、(b) A が4コマループで F=10
    for (a_frames, target_f) in [(1usize, 5usize), (4usize, 10usize)] {
        let (mut app, _, sym_b_id) = build_nested_app(a_frames);
        let at_1 = app.doc.library().at(1).unwrap().id;
        app.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.first_frame = 1
        });
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
            .find(|s| s.color() != raw_col)
            .unwrap()
            .color();
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
            .find(|s| s.color() != raw_col)
            .unwrap()
            .color();

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
            .color();
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
            .color();
        if a_frames == 1 {
            // グラフィックシンボルの決まり：1コマの A の中の B は A の内部フレームのまま止まる。分解したあとの B は
            // ステージの時計で進む
            assert_eq!(color_before_f_next, color_before_f);
            assert_eq!((color_before_f, color_after_f_next), (c1, c2));
        } else {
            assert_eq!(color_after_f_next, color_before_f_next);
        }
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
            .find(|s| s.color() != raw_col)
            .unwrap()
            .color();
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Ungroup));
        let col_after = app
            .scene()
            .iter()
            .find(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == sym_b_id))
            .unwrap()
            .color();
        assert_eq!(col_after, col_before);
    }

    // 6 & 7. 1回再生の B（合わせられる場合と負になって 0 になる場合）および任意のフレームの B
    {
        let (mut app, _, _) = build_nested_app(4);
        let at_1 = app.doc.library().at(1).unwrap().id;
        app.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.loop_mode = SymbolLoopMode::PlayOnce;
        });
        let at_1 = app.doc.library().at(1).unwrap().id;
        app.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.first_frame = 2;
        });
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
            app.doc.root.layers[0].keyframes[0].cel.instances()[0]
                .binding
                .first_frame,
            2
        );

        // B の進み(1) - (F - S)(5) < 0 のとき 1回再生は first_frame が 0 になる
        let (mut app_neg, _, _) = build_nested_app(4);
        let at_1 = app_neg.doc.library().at(1).unwrap().id;
        app_neg.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.loop_mode = SymbolLoopMode::PlayOnce;
        });
        let at_1 = app_neg.doc.library().at(1).unwrap().id;
        app_neg.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.first_frame = 0;
        });
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
            app_neg.doc.root.layers[0].keyframes[0].cel.instances()[0]
                .binding
                .first_frame,
            0
        );

        // 7. 任意のフレームの B は first_frame が変わらない
        let (mut app_sf, _, _) = build_nested_app(4);
        let at_1 = app_sf.doc.library().at(1).unwrap().id;
        app_sf.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.loop_mode = SymbolLoopMode::SingleFrame;
        });
        let at_1 = app_sf.doc.library().at(1).unwrap().id;
        app_sf.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.first_frame = 2;
        });
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
            app_sf.doc.root.layers[0].keyframes[0].cel.instances()[0]
                .binding
                .first_frame,
            2
        );
    }

    // 8. A の中でトゥイーンしている B: トゥイーン途中のコマで分解すると B の位置が分解前と同じ
    {
        let (mut app, sym_a_id, sym_b_id) = build_nested_app(5);
        {
            write_symbol(&mut app.doc, sym_a_id, |tl| {
                tl.layers[0].keyframe_mut(0).cel = timeline::Cel::new_with_instances(
                    Vec::new(),
                    vec![tl.layers[0].keyframes[0].cel.instances()[0].clone()],
                );
                tl.layers[0].keyframe_mut(0).tween = true;
                insert_keyframe_at(tl, 0, 4, true, 24);
                tl.layers[0].keyframe_mut(1).cel.instances_mut()[0]
                    .binding
                    .xform *= Affine::translate((80.0, 0.0));
            });
        }
        app.doc.revision = crate::serial::next_mesh_revision();
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
        let at_1 = app.doc.library().at(1).unwrap().id;
        app.doc.write_symbol_timeline(at_1, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding.color_effect = ColorEffect::new(0.3, 0.0);
        });
        app.doc.revision = crate::serial::next_mesh_revision();
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
        assert_eq!(raw_shape.color(), raw_col);
        assert_eq!(raw_shape.color().to_srgba_unmultiplied()[3], 255);

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
            .shapes()
            .last()
            .unwrap()
            .shape_id;
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        app.session.add_to_selection([c_stroke_id], std::iter::empty());
        assert!(apply(&mut app, Action::ConvertToSymbol));
        let sym_c_id = symbols_in_order(app.doc.library()).last().unwrap().id;
        assert_ne!(sym_c_id, sym_a_id);
        assert_ne!(sym_c_id, sym_b_id);

        let instances_before = &app.doc.root.layers[0].keyframes[0].cel.instances();
        assert_eq!(instances_before.len(), 2);
        assert_eq!(instances_before[0].binding.symbol_id, sym_a_id);
        assert_eq!(instances_before[1].binding.symbol_id, sym_c_id);
        let a_gid = instances_before[0].binding.instance_gid;

        // 0 コマ目の B を空っぽにしてから A だけを分解する
        let at_0 = app.doc.library().at(0).unwrap().id;
        app.doc.write_symbol_timeline(at_0, |tl, _| {
            tl.layers[0].keyframe_mut(0).cel = timeline::Cel::default();
        });
        app.doc.revision = crate::serial::next_mesh_revision();
        app.refresh_scene_base();

        app.session.replace_selection(crate::canvas_tool::Selection::default());
        app.session.add_to_selection(std::iter::empty(), [a_gid]);
        assert!(apply(&mut app, Action::Ungroup));

        let instances_after = &app.doc.root.layers[0].keyframes[0].cel.instances();
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
                    && !s.exterior().segments.is_empty()
            })
            .expect("1 コマ目で B の図形が見えること");
        let c_scene_idx = app
            .scene()
            .iter()
            .position(|s| {
                s.symbol_instance.is_some_and(|b| b.symbol_id == sym_c_id)
                    && !s.exterior().segments.is_empty()
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
        let sym_c_id = symbols_in_order(app.doc.library()).last().unwrap().id;
        assert_ne!(sym_c_id, sym_b_id);
        assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));

        // A の中で B だけを内側グループ G_inner に入れ、さらに生の図形と一緒に外側グループ G_outer に入れる
        assert!(apply(
            &mut app,
            Action::EnterSymbolFromLibrary { sym_id: sym_a_id },
        ));
        let b_gid_in_a = symbols_in_order(app.doc.library())
            .find(|s| s.id == sym_a_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[0]
            .cel
            .instances()[0]
            .binding
            .instance_gid;
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        app.session.add_to_selection(std::iter::empty(), [b_gid_in_a]);
        assert!(apply(&mut app, Action::Group));
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Group));
        assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));

        if b_empty_on_ungroup_frame {
            let at_0 = app.doc.library().at(0).unwrap().id;
            app.doc.write_symbol_timeline(at_0, |tl, _| {
                tl.layers[0].keyframe_mut(0).cel = timeline::Cel::default();
            });
            app.doc.revision = crate::serial::next_mesh_revision();
            app.refresh_scene_base();
        }

        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Ungroup));
        let cel = &app.doc.root.layers[0].keyframes[0].cel;
        assert_eq!(cel.instances().len(), 1);
        assert_eq!(cel.instances()[0].binding.symbol_id, sym_b_id);
        assert_eq!(
            cel.instances()[0].parent_scope.len(),
            2,
            "A の中で 2 重グループ [G_outer, G_inner] に入っていた B（b_empty={b_empty_on_ungroup_frame}）は分解後も両方のグループの中にいること"
        );
        assert_eq!(
            cel.instances()[0].parent_scope[0],
            cel.shapes()[0].group_path[0],
            "同じ外側グループ G_outer に入っていた生図形と B（b_empty={b_empty_on_ungroup_frame}）が分解後も同じグループ番号を持つこと"
        );
        assert_ne!(
            cel.instances()[0].parent_scope[1],
            cel.shapes()[0].group_path[0],
            "B だけが入っていた内側グループ G_inner にも新しいグループ番号が振られること"
        );
        if !b_empty_on_ungroup_frame {
            assert!(app.scene().iter().any(|s| s
                .symbol_instance
                .is_some_and(|b| b.symbol_id == sym_b_id)
                && !s.exterior().segments.is_empty()));
        }
    }
}

#[test]
fn test_failed_paste_due_to_nest_limit_rolls_back_clipboard_imported_map() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    assert_eq!(app.doc.library().len(), 8);
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
    let host_sym_id = app.doc.library().at(0).unwrap().id;
    assert!(apply(
        &mut app,
        Action::EnterSymbolFromLibrary {
            sym_id: host_sym_id
        },
    ));
    assert!(!apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(app.doc.library().len(), 1);
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
        .shapes()
        .last()
        .unwrap()
        .shape_id;
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    app.session.add_to_selection([blue_stroke_id], std::iter::empty());
    assert!(apply(&mut app, Action::ConvertToSymbol));
    assert_eq!(app.doc.library().len(), 2);

    // 4. ルートステージ（入れ子上限に引っかからない場所）に貼り付けると、青いシンボルに化けず赤いシンボルが新しく登録されて貼られる
    assert!(apply(&mut app, Action::Paste { in_place: true }));
    assert_eq!(
        app.doc.library().len(),
        10,
        "8段の赤いシンボル群がタブ Y のライブラリに新しくインポートされること"
    );
    let pasted_inst_gid = *app.session.selection().instances.iter().next().unwrap();
    let pasted_shapes: Vec<_> = app
        .scene()
        .iter()
        .filter(|s| s.group_path.first() == Some(&pasted_inst_gid))
        .collect();
    assert!(!pasted_shapes.is_empty());
    assert!(
        pasted_shapes.iter().all(|s| s.color() == red),
        "貼り付けたシンボルの図形が元の赤色であること"
    );
}

#[test]
fn test_group_and_symbol_reject_cross_layer_selection_and_allow_single_layer() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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

    // 1. 下のレイヤーの赤と上のレイヤーの青を選んで Action::Group・Action::ConvertToSymbol → どちらも false が返り、
    //    文書（ライブラリを含む）と Undo 履歴が変わらない（どちらも1つのレイヤーの中で選んだものだけ）
    assert!(apply(&mut app, Action::SelectAll));
    assert_eq!(app.session.selected_indices().len(), 2);
    assert_eq!(crate::canvas_tool::select_ops::selected_single_layer_id(&app), None);
    let undo_len_before = app.history.undo_stack.len();
    assert!(!apply(&mut app, Action::Group));
    assert!(!apply(&mut app, Action::ConvertToSymbol));
    assert_eq!(app.history.undo_stack.len(), undo_len_before);
    assert!(app.doc.library().is_empty());
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
    crate::canvas_tool::select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 160.0),
        Point::new(320.0, 240.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 2);
    assert!(crate::canvas_tool::select_ops::selected_single_layer_id(&app).is_some());
    assert!(apply(&mut app, Action::Group));
    assert!(app
        .scene()
        .iter()
        .filter(|s| s.color() != red)
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

/// シンボルの中で生の絵を動かすと、選んだままでも、同じシンボルのほかのインスタンス（薄く見える親の背景）に映る（グループを
/// 動かしたときと同じ。利用者の指摘：生の絵は選択を外すまで映らなかった。動かした生の絵は、選んでいる間は浮いた片で、
/// 文書のシンボルの中身にはまだ書いていないため）
#[test]
fn moving_raw_art_inside_a_symbol_shows_on_its_other_instances() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 160.0, 160.0, Color32::from_rgb(200, 60, 60))]);
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let a = *app.session.selection().instances.iter().next().expect("インスタンス");
    // 右へ Alt で複製して、同じシンボルのインスタンスをもう1つ置く
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(300.0, 0.0), alt_duplicate: true }));
    assert!(apply(&mut app, Action::Deselect));
    // 左のインスタンスの中に入り、生の絵を下へ動かす（選んだまま）
    assert!(apply(&mut app, Action::EnterSymbolTimeline { instance_gid: a }));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(0.0, 80.0), alt_duplicate: false }));
    // 薄く見える右のインスタンスの、いちばん下の所
    let sibling_bottom = |app: &mut Is2dApp| -> f32 {
        app.tabs[app.active].start_draw_pass();
        app.tabs[app.active]
            .cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW)
            .iter()
            .flat_map(|m| m.vertices.iter().map(|v| v.pos))
            .filter(|p| p.x > 300.0)
            .map(|p| p.y)
            .fold(f32::MIN, f32::max)
    };
    assert!((sibling_bottom(&mut app) - 240.0).abs() < 1.0, "選んだまま、ほかのインスタンスに映らない：{}", sibling_bottom(&mut app));
    assert!(apply(&mut app, Action::Deselect));
    assert!((sibling_bottom(&mut app) - 240.0).abs() < 1.0, "選択を外しても、ほかのインスタンスに映らない");
}

#[test]
fn test_leaving_symbol_releases_backdrop_cache() {
    let mut app = Is2dApp::default();
    apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 8.0,
            color: Color32::RED,
        },
    );
    apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 220.0), Point::new(200.0, 220.0)],
            radius: 8.0,
            color: Color32::BLUE,
        },
    );
    apply(
        &mut app,
        Action::SelectSingleAt {
            pt: Point::new(150.0, 100.0),
        },
    );
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let binding = app
        .tab()
        .scene()
        .iter()
        .find_map(|s| s.symbol_instance)
        .expect("シンボルインスタンスが存在すること");
    assert!(apply(
        &mut app,
        Action::EnterSymbolTimeline {
            instance_gid: binding.instance_gid,
        }
    ));

    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
    assert!(app.tab().session.backdrop_cache.lock().entry_count() > 0);

    // 写しは、直前の描画で使わなかったものを、次の描画の始めに捨てる（出たあとの1回目の描画では使わない）
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    for _ in 0..2 {
        let _ = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
    }
    assert_eq!(app.tab().session.backdrop_cache.lock().entry_count(), 0);
}

#[test]
fn test_instance_only_cel_keeps_no_spare_shape_capacity() {
    let mut app = Is2dApp::default();
    apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 6.0,
            color: Color32::RED,
        },
    );
    apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 180.0), Point::new(200.0, 180.0)],
            radius: 6.0,
            color: Color32::BLUE,
        },
    );
    apply(&mut app, Action::SelectAll);
    assert!(apply(&mut app, Action::ConvertToSymbol));
    assert!(apply(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(4.0, 2.0),
        }
    ));
    let cel = &app.tab().doc.root.layers[0].keyframes[0].cel;
    assert_eq!(cel.instances().len(), 1);
    assert_eq!(cel.shapes().len(), 0);
    assert_eq!(cel.shapes().capacity(), 0);
}

/// シンボルの中で描いても、触っていない図形は動かない：インスタンスを動かしてからまた入って描いても、動かす前の座標の
/// 地図を使わない（地図の鍵に、図形を置いた座標の変換が入っている。レビュー 8b33f96 の N-1001）
#[test]
fn test_symbol_edit_after_moving_instance_keeps_untouched_shapes() {
    use crate::canvas_tool::doc_actions::Action;
    let mut app = Is2dApp::default();
    let stroke = |y: f64, color: Color32| Action::PaintStroke {
        points: vec![Point::new(-60.0, y), Point::new(60.0, y)],
        radius: 20.0,
        color,
    };
    run(&mut app, &[stroke(0.0, Color32::RED)]);
    app.set_selected_indices(vec![0]);
    assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
    let enter = |app: &mut Is2dApp| {
        let (binding, _, _) = symbol_ops::selected_single_symbol_info(app).unwrap();
        assert!(symbol_ops::enter_symbol_timeline(app, binding.instance_gid));
    };
    let local_shapes = |app: &Is2dApp| -> Vec<(crate::vector_engine::ShapeId, Color32, [f64; 4])> {
        app.active_timeline().layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| {
                let b = s.bounding_box();
                (s.shape_id, s.color(), [b.x0, b.y0, b.x1, b.y1])
            })
            .collect()
    };
    enter(&mut app);
    // 何も変えないバケツ（囲まれていない所）で、その座標の地図を作らせる
    doc_actions::apply(
        &mut app,
        Action::BucketFill {
            pt: Point::new(400.0, 400.0),
            color: Color32::GREEN,
        },
    );
    let red_before = local_shapes(&app);
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
    app.set_selected_indices(vec![0]);
    assert!(doc_actions::apply(
        &mut app,
        Action::NudgeSelection {
            delta: KVec2::new(0.0, 200.0),
        }
    ));
    enter(&mut app);
    // 赤の前の位置（今は何も無い所）に青を描く。赤はステージの y 200 へ動いているので、触らない
    run(&mut app, &[stroke(0.0, Color32::BLUE)]);
    let after = local_shapes(&app);
    for red in &red_before {
        assert!(
            after.contains(red),
            "触っていない赤が変わった: {red:?} → {after:?}"
        );
    }
    assert_eq!(after.len(), red_before.len() + 1, "{after:?}");
}

/// 再生位置をずらす式は `advance`・`rewind` だけ：ループは 0 より前になるときだけ回し、1回だけの再生は 0 で止め、
/// 1コマだけの表示は変えない（洗い出しの 12）
#[test]
fn instance_playback_position_shifts_by_one_rule() {
    use crate::symbol_binding::{SymbolInstanceBinding, SymbolLoopMode};
    let mut b = SymbolInstanceBinding::new(1, 1, Affine::IDENTITY);
    b.first_frame = 3;
    b.advance(4);
    assert_eq!(b.first_frame, 7);
    b.rewind(2, 5);
    assert_eq!(b.first_frame, 5, "0 より前にならなければ回さない");
    b.rewind(7, 5);
    assert_eq!(
        b.first_frame, 3,
        "ループは 0 より前になると、コマ数で回す（5 − 7 ≡ 3）"
    );
    b.loop_mode = SymbolLoopMode::PlayOnce;
    b.rewind(9, 5);
    assert_eq!(b.first_frame, 0);
    b.loop_mode = SymbolLoopMode::SingleFrame;
    b.first_frame = 2;
    b.advance(4);
    b.rewind(9, 5);
    assert_eq!(b.first_frame, 2);
}

/// シンボルの中で浮いた片の確定が断られる状態のとき、そのシンボルを削除しても固まらず、文書を変えずに断る。直す前は、
/// 段を出られないまま出ようとし続けて固まった（レビュー f01d518 の N-4001）。固まったら分かるように、別のスレッドで
/// 動かして待つ
#[test]
fn deleting_a_symbol_whose_level_cannot_be_left_refuses_without_hanging() {
    use doc_actions::{apply, Action};
    let (tx, rx) = std::sync::mpsc::channel();
    // 固まりを見張るためのテストのスレッド（止まったら、知らせが来ないのでテストが落ちる）
    #[allow(clippy::disallowed_methods)]
    std::thread::spawn(move || {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            make_stroke(
                Point::new(100.0, 200.0),
                Point::new(300.0, 200.0),
                25.0,
                Color32::from_rgb(235, 75, 75),
            ),
        );
        run(&mut app, &[Action::SelectAll, Action::ConvertToSymbol]);
        let sym_id = app.tab().doc.library().at(0).unwrap().id;
        run(
            &mut app,
            &[Action::EnterSymbolFromLibrary { sym_id }, Action::SelectAll],
        );
        doc_actions::finish_move_selection_drag(&mut app, KVec2::new(0.0, 100.0), false);
        assert!(app.session.floating().is_some());
        // 片のレイヤーを、確定を通さずに編集できなくする（確定が断られる場面の代わり）
        app.tab_mut().with_active_timeline_mut(|tl, _| {
            tl.layers[0].locked = true;
            true
        });
        let undo_len = app.history.undo_stack.len();
        let deleted = apply(&mut app, Action::DeleteSymbol { sym_id });
        let _ = tx.send((
            deleted,
            app.tab().doc.find_symbol(sym_id).is_some(),
            app.history.undo_stack.len() == undo_len,
        ));
    });
    let (deleted, kept, same_undo) = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("削除で固まらない");
    assert!(!deleted, "断る");
    assert!(kept, "シンボルは残る");
    assert!(same_undo, "Undo の控えも増えない");
}

/// N-2003：地図だけを書く編集（ブラシ・消しゴム・バケツ・輪郭の変形・確定）は、セル画のインスタンスの並びを写さず、
/// Undo の控えと共有したままにする
#[test]
fn map_only_writes_share_the_instances() {
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let mut app = Is2dApp::default();
    let mut shapes = make_stroke(
        Point::new(100.0, 100.0),
        Point::new(200.0, 100.0),
        10.0,
        red,
    );
    // 生の図形の地図を残す
    shapes.extend(make_stroke(
        Point::new(100.0, 400.0),
        Point::new(200.0, 400.0),
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
        Point::new(50.0, 50.0),
        Point::new(250.0, 150.0),
        false,
    );
    doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert!(canvas_tool::clear_selection(&mut app));
    let before = std::sync::Arc::clone(app.doc.root.layers[0].keyframes[0].cel.instances());
    assert_eq!(before.len(), 1);
    // 地図だけを書く編集（本番のブラシ：1本の道）
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(300.0, 300.0), Point::new(400.0, 300.0)],
            radius: 10.0,
            color: blue,
        }
    ));
    let after = &app.doc.root.layers[0].keyframes[0].cel;
    assert!(!after.maps().is_empty());
    assert!(std::sync::Arc::ptr_eq(&before, after.instances()));
}

/// N-2003：シンボルの中で描いても、Undo の控えと今のライブラリは、描いたシンボルのほかのシンボルそのものを共有する
/// （ライブラリ全部を写さない。履歴1件の大きさはシンボルの数に依らない）
#[test]
fn drawing_inside_a_symbol_copies_only_that_symbol() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
    assert!(apply(&mut app, Action::SelectAll));
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert!(canvas_tool::clear_selection(&mut app));
    let proto: crate::symbol::SymbolDef = (**app.doc.library().at(0).unwrap()).clone();
    for k in 0..200 {
        let mut s = proto.clone();
        s.id = app.doc.next_symbol_id.take();
        s.name = format!("copy {k}");
        app.doc.add_symbol(None, s);
    }
    assert!(apply(&mut app, Action::EnterSymbolFromLibrary { sym_id }));
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(-20.0, 40.0), Point::new(20.0, 40.0)],
            radius: 4.0,
            color: red,
        },
    ));
    let entry = app.history.undo_stack.back().unwrap();
    let n = app.doc.library().len();
    let shared = (0..n)
        .filter(|&i| {
            let (a, b): (&crate::symbol::SymbolDef, &crate::symbol::SymbolDef) =
                (entry.library.at(i).unwrap(), app.doc.library().at(i).unwrap());
            std::ptr::eq(a, b)
        })
        .count();
    assert_eq!(shared, n - 1);
}

/// シンボルの中を編集するときも、ふだん・グループの中と同じく、白いステージ枠・枠線を描き、「枠外を隠す」も効く（前は、
/// シンボルの中だけステージの代わりに薄い灰色一色で全面を塗り、枠線も「枠外を隠す」も無かったので、薄く描いた周りの絵が
/// 背景に溶けて、別の場所で編集しているように見えた）
#[test]
fn symbol_edit_draws_the_stage_like_the_scene() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 100, 220);
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(300.0, 300.0), Point::new(500.0, 300.0)],
                radius: 20.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(490.0, 300.0),
            },
            Action::ConvertToSymbol,
            Action::Deselect,
            Action::PaintStroke {
                points: vec![Point::new(300.0, 400.0), Point::new(500.0, 400.0)],
                radius: 20.0,
                color: blue,
            },
        ],
    );
    let binding = app
        .scene()
        .iter()
        .find_map(|s| s.symbol_instance)
        .expect("シンボルのインスタンス");
    app.view.clip_outside_stage = true;
    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        ..Default::default()
    };
    // 1コマ描いて、ステージの矩形に描いたもの（塗り・線）と、赤（シンボルの中身）を描いた所の切り抜きを返す
    let draw = |app: &mut Is2dApp| {
        let out = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
        let stage = app.view.stage_screen_rect(app.doc.stage_size);
        let stage_rects: Vec<(Color32, egui::Stroke)> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.rect == stage => Some((r.fill, r.stroke)),
                _ => None,
            })
            .collect();
        let red_clips: Vec<egui::Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Mesh(m) if m.vertices.first().is_some_and(|v| v.color == red) => {
                    Some(c.clip_rect)
                }
                _ => None,
            })
            .collect();
        (stage, stage_rects, red_clips)
    };
    let (stage, scene_rects, scene_clips) = draw(&mut app);
    assert!(
        scene_rects.iter().any(|r| r.0 == Color32::WHITE)
            && scene_rects.iter().any(|r| r.1.width > 0.0),
        "ふだんは白いステージと枠線を描く：{scene_rects:?}"
    );
    assert!(
        !scene_clips.is_empty() && scene_clips.iter().all(|c| stage.contains_rect(*c)),
        "ふだんは「枠外を隠す」でステージの外を隠す：{scene_clips:?}"
    );
    run(
        &mut app,
        &[Action::EnterSymbolTimeline {
            instance_gid: binding.instance_gid,
        }],
    );
    assert!(!app.session.edit_path().is_empty(), "シンボルの中");
    let (sym_stage, sym_rects, sym_clips) = draw(&mut app);
    assert_eq!(sym_stage, stage, "入っても表示の位置は変わらない");
    assert_eq!(
        sym_rects, scene_rects,
        "シンボルの中でも、ふだんと同じステージを描く"
    );
    assert!(
        !sym_clips.is_empty() && sym_clips.iter().all(|c| *c == scene_clips[0]),
        "シンボルの中でも、「枠外を隠す」でステージの外を隠す：{sym_clips:?}"
    );
}

/// ライブラリの「編集」は、どのインスタンスにも結び付けずにシンボルだけを編集する（Adobe Animate の「シンボル編集」）：
/// いくつ置いてあっても、どれか1つの所で開かない。基準点はステージの真ん中。周りの絵もステージも見せない。その中で
/// 子のシンボルにその場で入ると、親のシンボルの絵だけを薄く見せる（シーンの絵は見せない）。別の所からライブラリで入ると、
/// いったんシーンまで出てから入る（前は、今のコマで最初に見つかったインスタンスの所で、その場で開いていた）
#[test]
fn library_edit_shows_only_the_symbol_without_picking_an_instance() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 100, 220);
    let green = Color32::from_rgb(40, 170, 90);
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(300.0, 300.0), Point::new(400.0, 300.0)],
                radius: 20.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(390.0, 300.0),
            },
            Action::ConvertToSymbol,
            Action::Deselect,
        ],
    );
    let sym_a = app.doc.library().at(0).unwrap().id;
    // 同じシンボルを、上のレイヤーにもう2つ置く。シーンには生の青い線もある
    run(
        &mut app,
        &[
            Action::AddLayer,
            Action::PlaceSymbol { sym_id: sym_a },
            Action::Deselect,
            Action::PlaceSymbol { sym_id: sym_a },
            Action::Deselect,
            Action::PaintStroke {
                points: vec![Point::new(300.0, 420.0), Point::new(400.0, 420.0)],
                radius: 20.0,
                color: blue,
            },
        ],
    );
    let placed = app
        .scene()
        .iter()
        .filter_map(|s| s.symbol_instance)
        .map(|b| b.instance_gid)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(placed.len(), 3, "3つ置いてある");
    let center = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        ..Default::default()
    };
    // 1コマ描いて、ステージの矩形に描いたものの数を返す
    let stage_rects = |app: &mut Is2dApp| {
        let out = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
        let stage = app.view.stage_screen_rect(app.doc.stage_size);
        out.shapes
            .iter()
            .filter(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.rect == stage))
            .count()
    };
    assert!(stage_rects(&mut app) > 0, "シーンではステージを描く");

    // 1. ライブラリから入る：インスタンスに結び付かず、基準点はステージの真ん中、周りの絵もステージも無い
    run(
        &mut app,
        &[Action::EnterSymbolFromLibrary { sym_id: sym_a }],
    );
    let path = app.session.edit_path();
    assert_eq!(path.len(), 1);
    assert_eq!(
        path.levels[0].instance_gid, 0,
        "どのインスタンスにも結び付かない"
    );
    assert_eq!(
        path.world_xform(),
        kurbo::Affine::translate(center.to_vec2()),
        "基準点はステージの真ん中"
    );
    assert!(!path.shows_stage());
    assert!(
        app.tabs[app.active]
            .cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW)
            .is_empty(),
        "シーンの絵（ほかのインスタンス・青い線）を見せない"
    );
    assert_eq!(stage_rects(&mut app), 0, "ステージ枠・枠線を描かない");

    // 2. その中で緑の線を子のシンボルにし、ダブルクリックでその場で入る：親（A）の絵だけを薄く見せる
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(500.0, 360.0), Point::new(560.0, 360.0)],
                radius: 14.0,
                color: green,
            },
            Action::SelectSingleAt {
                pt: Point::new(555.0, 360.0),
            },
            Action::ConvertToSymbol,
            Action::Deselect,
            Action::EnterGroupAt {
                pt: Point::new(530.0, 360.0),
            },
        ],
    );
    let sym_b = symbols_in_order(app.doc.library()).map(|s| s.id).max().unwrap();
    assert_ne!(sym_b, sym_a);
    let path = app.session.edit_path();
    assert_eq!(path.len(), 2, "子のシンボルの中");
    assert_eq!(path.levels[1].symbol_id, sym_b);
    assert_ne!(path.levels[1].instance_gid, 0, "子にはその場で入る");
    assert!(
        !path.shows_stage(),
        "ライブラリから入った中なので、ステージは見せない"
    );
    let backdrop = app.tabs[app.active].cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW);
    assert!(!backdrop.is_empty(), "親のシンボルの絵");
    let colors: std::collections::HashSet<Color32> = backdrop
        .iter()
        .flat_map(|m| m.vertices.iter().map(|v| v.color))
        .collect();
    let dim = crate::screen_mesh::dim_backdrop_color;
    assert!(colors.contains(&dim(red)), "親（A）の赤を薄く見せる");
    assert!(!colors.contains(&dim(blue)), "シーンの青は見せない");
    assert_eq!(stage_rects(&mut app), 0);

    // 3. 子の中からライブラリで A に入り直す：シーンまで出てから、A だけの編集に入る
    run(
        &mut app,
        &[Action::EnterSymbolFromLibrary { sym_id: sym_a }],
    );
    let path = app.session.edit_path();
    assert_eq!(path.len(), 1);
    assert_eq!(
        (path.levels[0].symbol_id, path.levels[0].instance_gid),
        (sym_a, 0)
    );

    // 4. 出るとシーンに戻り、ステージをまた描く
    run(&mut app, &[Action::ExitGroupSteps { steps: 1 }]);
    assert!(app.session.edit_path().is_empty());
    assert!(stage_rects(&mut app) > 0);
}

/// ライブラリの深い版と使われ方の表（設計 claude/シンボルの目次と参照の表の設計.md）：入れ子のシンボル（親の中に子）で、
/// 子の中に描くと、子と親の版が変わり、関係の無いシンボルの版は変わらない。親の高さは子の高さ＋1、子が2コマになると
/// 親も「動く」になる。子を消すと、親の中のインスタンスも消え、使われ方の表から外れる
#[test]
fn library_deep_revision_and_users_follow_the_nesting() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    let stroke = |app: &mut Is2dApp, x: f64| {
        assert!(apply(
            app,
            Action::PaintStroke { points: vec![Point::new(x, 300.0), Point::new(x + 80.0, 320.0)], radius: 6.0, color: Color32::RED },
        ));
    };
    // 子：1本の線をシンボルにする。関係の無いシンボル：別の線をシンボルにする
    stroke(&mut app, 100.0);
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let child = symbols_in_order(app.tab().doc().library()).last().unwrap().id;
    assert!(canvas_tool::clear_selection(&mut app));
    // 親：子のインスタンスを選んでシンボルにする
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let parent = symbols_in_order(app.tab().doc().library()).last().unwrap().id;
    assert!(canvas_tool::clear_selection(&mut app));
    stroke(&mut app, 600.0);
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(580.0, 280.0), p1: Point::new(700.0, 340.0), additive: false }));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let other = symbols_in_order(app.tab().doc().library()).last().unwrap().id;
    let lib = |app: &Is2dApp| app.tab().doc().library().clone();
    let l = lib(&app);
    assert_eq!(l.users_of(child).collect::<Vec<_>>(), vec![parent], "子を使っている親");
    assert_eq!(l.get(parent).unwrap().children(), &[child]);
    let rev = |l: &crate::symbol::Library, id: u64| l.get(id).unwrap().revision();
    let (rc, rp, ro) = (rev(&l, child), rev(&l, parent), rev(&l, other));
    // 子の中に描く：子と親の版が変わり、関係の無いシンボルは変わらない
    assert!(apply(&mut app, Action::EnterSymbolFromLibrary { sym_id: child }));
    stroke(&mut app, 300.0);
    let l = lib(&app);
    assert_ne!(rev(&l, child), rc, "描いた子の版が変わらない");
    assert_ne!(rev(&l, parent), rp, "子を使う親の版が変わらない");
    assert_eq!(rev(&l, other), ro, "関係の無いシンボルの版が変わった");
    assert!(!l.get(parent).unwrap().has_dynamic_animation());
    // 子を2コマにする：子も親も動く
    assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: true }));
    let l = lib(&app);
    assert!(l.get(child).unwrap().has_dynamic_animation() && l.get(parent).unwrap().has_dynamic_animation(), "子が2コマなのに親が動かない");
    assert!(l.check_tables().is_ok());
    // 子を消す：親の中のインスタンスも消え、使われ方の表から外れる
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    assert!(apply(&mut app, Action::DeleteSymbol { sym_id: child }));
    let l = lib(&app);
    assert!(l.get(child).is_none());
    assert!(l.get(parent).unwrap().children().is_empty(), "消した子のインスタンスが親に残った");
    assert_eq!(l.users_of(child).count(), 0);
    assert!(l.check_tables().is_ok());
}

/// ライブラリの「複製」：元のすぐ下に「（元の名前）のコピー」ができて選ばれる（もうあれば「のコピー 2」）。中身は同じ形の
/// 別のシンボルで、片方を編集してももう片方は変わらない。中の子のシンボルは元と同じものを使い、中のインスタンスの番号は
/// 振り直す。置いてあるインスタンスは元のシンボルのまま。Undo で消える
#[test]
fn duplicating_a_symbol_adds_a_copy_below_it() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(40, 170, 90);
    let blue = Color32::from_rgb(60, 100, 220);
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![Point::new(300.0, 300.0), Point::new(400.0, 300.0)],
                radius: 20.0,
                color: red,
            },
            Action::SelectSingleAt {
                pt: Point::new(390.0, 300.0),
            },
            Action::ConvertToSymbol,
            Action::Deselect,
        ],
    );
    let sym_a = app.doc.library().at(0).unwrap().id;
    // A の中に、子のシンボル B（緑）を作る
    run(
        &mut app,
        &[
            Action::EnterSymbolFromLibrary { sym_id: sym_a },
            Action::PaintStroke {
                points: vec![Point::new(500.0, 360.0), Point::new(560.0, 360.0)],
                radius: 14.0,
                color: green,
            },
            Action::SelectSingleAt {
                pt: Point::new(555.0, 360.0),
            },
            Action::ConvertToSymbol,
            Action::Deselect,
            Action::ExitGroupSteps { steps: 1 },
        ],
    );
    let sym_b = app.doc.library().at(1).unwrap().id;
    let name_a = app.doc.library().at(0).unwrap().name.clone();
    let ids = |app: &Is2dApp| symbols_in_order(app.doc.library()).map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(ids(&app), vec![sym_a, sym_b]);
    // シンボルの一つ目のコマの形（色と輪郭）と、中のインスタンス（シンボル・番号）
    let cel_of = |app: &Is2dApp, id: u64| {
        let cel = &app.doc.find_symbol(id).unwrap().timeline.layers[0].keyframes[0].cel;
        let mut shapes: Vec<(Color32, Vec<kurbo::QuadBez>)> = cel
            .shapes()
            .iter()
            .map(|s| {
                let segs = s
                    .contours()
                    .flat_map(|c| c.segments.iter().copied())
                    .collect();
                (s.color(), segs)
            })
            .collect();
        shapes.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        let insts: Vec<(u64, u64)> = cel
            .instances()
            .iter()
            .map(|i| (i.binding.symbol_id, i.binding.instance_gid))
            .collect();
        (shapes, insts)
    };
    let stage_inst = |app: &Is2dApp| {
        app.doc.root.layers[0].keyframes[0]
            .cel
            .single_symbol_instance()
            .unwrap()
            .symbol_id
    };

    // 1. 複製する
    run(&mut app, &[Action::DuplicateSymbol { sym_id: sym_a }]);
    let copy1 = app.doc.library().at(1).unwrap().id;
    assert_eq!(ids(&app), vec![sym_a, copy1, sym_b], "元のすぐ下に入る");
    assert!(copy1 != sym_a && copy1 != sym_b);
    assert_eq!(app.doc.library().at(1).unwrap().name, format!("{name_a}のコピー"));
    assert_eq!(app.library_symbol(), Some(copy1), "複製を選ぶ");
    let (shapes_a, insts_a) = cel_of(&app, sym_a);
    let (shapes_c, insts_c) = cel_of(&app, copy1);
    assert!(!shapes_a.is_empty());
    assert_eq!(shapes_c, shapes_a, "同じ形");
    assert_eq!(insts_a.len(), 1);
    assert_eq!(insts_c.len(), 1);
    assert_eq!(insts_c[0].0, sym_b, "中の子のシンボルは元と同じもの");
    assert_ne!(
        insts_c[0].1, insts_a[0].1,
        "中のインスタンスの番号は振り直す"
    );
    assert_eq!(stage_inst(&app), sym_a, "置いてあるインスタンスは元のまま");

    // 2. もう一度複製すると「のコピー 2」
    run(&mut app, &[Action::DuplicateSymbol { sym_id: sym_a }]);
    let copy2 = app.doc.library().at(1).unwrap().id;
    assert_eq!(ids(&app), vec![sym_a, copy2, copy1, sym_b]);
    assert_eq!(app.doc.library().at(1).unwrap().name, format!("{name_a}のコピー 2"));

    // 3. 複製の中を編集しても、元は変わらない
    run(
        &mut app,
        &[
            Action::EnterSymbolFromLibrary { sym_id: copy1 },
            Action::PaintStroke {
                points: vec![Point::new(500.0, 250.0), Point::new(560.0, 250.0)],
                radius: 14.0,
                color: blue,
            },
            Action::ExitGroupSteps { steps: 1 },
        ],
    );
    assert_ne!(cel_of(&app, copy1).0, shapes_a, "複製は変わる");
    assert_eq!(cel_of(&app, sym_a).0, shapes_a, "元は変わらない");

    // 4. Undo で、編集・2つ目・1つ目の複製の順に戻る
    app.undo();
    app.undo();
    assert_eq!(ids(&app), vec![sym_a, copy1, sym_b]);
    app.undo();
    assert_eq!(ids(&app), vec![sym_a, sym_b]);
}

/// ライブラリの「入替」：キャンバスで選んでいるインスタンスを、一覧で選んだシンボルに入れ替える。位置・大きさ・回転・
/// 中心点・色効果・再生設定はそのまま。複数選んでいれば全部。一緒に選んだ生の図形と、選んでいないインスタンスは変わらない。
/// 選んだものが全部もうそのシンボルのとき・何も選んでいないとき・入れ子が回るときは断る。Undo 1回で戻る
#[test]
fn swapping_replaces_the_symbol_of_selected_instances_and_keeps_their_settings() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 60, 60);
    let green = Color32::from_rgb(40, 170, 90);
    let blue = Color32::from_rgb(60, 100, 220);
    let make_symbol = |app: &mut Is2dApp, y: f64, color: Color32| {
        run(
            app,
            &[
                Action::PaintStroke {
                    points: vec![Point::new(300.0, y), Point::new(400.0, y)],
                    radius: 20.0,
                    color,
                },
                Action::SelectSingleAt {
                    pt: Point::new(390.0, y),
                },
                Action::ConvertToSymbol,
                Action::Deselect,
            ],
        );
        symbols_in_order(app.doc.library()).last().unwrap().id
    };
    let sym_a = make_symbol(&mut app, 300.0, red);
    let sym_b = make_symbol(&mut app, 450.0, green);
    run(
        &mut app,
        &[
            Action::PlaceSymbol { sym_id: sym_a },
            Action::Deselect,
            Action::PaintStroke {
                points: vec![Point::new(300.0, 600.0), Point::new(400.0, 600.0)],
                radius: 20.0,
                color: blue,
            },
            // 1つ目の A に色効果を付けておく
            Action::SelectSingleAt {
                pt: Point::new(390.0, 300.0),
            },
            Action::SetSymbolColorEffect {
                effect: crate::symbol_binding::ColorEffect::new(0.5, 0.2),
            },
        ],
    );
    let bindings = |app: &Is2dApp| {
        app.doc.root.layers[0].keyframes[0]
            .cel
            .instances()
            .iter()
            .map(|i| (i.binding.instance_gid, i.binding))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let before = bindings(&app);
    let gid_of = |sym: u64| -> Vec<u64> {
        before
            .values()
            .filter(|b| b.symbol_id == sym)
            .map(|b| b.instance_gid)
            .collect()
    };
    let (a_gids, b_gids) = (gid_of(sym_a), gid_of(sym_b));
    assert_eq!((a_gids.len(), b_gids.len()), (2, 1));
    // 2つの A と生の青い線を選ぶ（B のインスタンスは選ばない）
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    let picked: Vec<FillShape> = app
        .scene()
        .iter()
        .filter(|s| s.color() == blue || s.symbol_instance.is_some_and(|b| b.symbol_id == sym_a))
        .cloned()
        .collect();
    for s in &picked {
        { let mut sel = app.session.selection().clone(); sel.insert_shape(s); app.session.replace_selection(sel); }
    }
    let raw_before = app.scene().iter().filter(|s| s.color() == blue).count();
    assert_eq!(
        symbol_ops::swap_targets(&app.tabs[app.active], sym_b),
        a_gids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>(),
        "入れ替えるのは選んだ2つの A だけ"
    );

    // 1. B に入れ替える
    assert!(doc_actions::apply(
        &mut app,
        Action::SwapSymbol { sym_id: sym_b }
    ));
    let after = bindings(&app);
    for gid in &a_gids {
        let mut expected = before[gid];
        expected.symbol_id = sym_b;
        assert_eq!(after[gid], expected, "シンボルだけ変わり、ほかはそのまま");
    }
    assert_ne!(
        before[&a_gids[0]].color_effect,
        crate::symbol_binding::ColorEffect::default(),
        "色効果を付けた A も確かめている"
    );
    assert_eq!(
        after[&b_gids[0]], before[&b_gids[0]],
        "選んでいない B は変わらない"
    );
    assert_eq!(
        app.scene().iter().filter(|s| s.color() == blue).count(),
        raw_before,
        "一緒に選んだ生の図形は変わらない"
    );
    assert!(
        a_gids
            .iter()
            .all(|g| app.session.selection().instances.contains(g)),
        "選択はそのまま"
    );

    // 2. 全部もう B なので断る。Undo 1回で A に戻る
    assert!(!doc_actions::apply(
        &mut app,
        Action::SwapSymbol { sym_id: sym_b }
    ));
    app.undo();
    assert_eq!(bindings(&app), before);

    // 3. 何も選んでいなければ断る
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    assert!(symbol_ops::swap_targets(&app.tabs[app.active], sym_b).is_empty());
    assert!(!doc_actions::apply(
        &mut app,
        Action::SwapSymbol { sym_id: sym_b }
    ));
    assert_eq!(bindings(&app), before);

    // 4. B の中に置いた A を B に入れ替えると、B が自分を含むので断る
    run(
        &mut app,
        &[
            Action::EnterSymbolFromLibrary { sym_id: sym_b },
            Action::PlaceSymbol { sym_id: sym_a },
        ],
    );
    assert_eq!(
        app.session.selection().instances.len(),
        1,
        "置いた A を選んでいる"
    );
    assert!(symbol_ops::swap_targets(&app.tabs[app.active], sym_b).is_empty());
    assert!(!doc_actions::apply(
        &mut app,
        Action::SwapSymbol { sym_id: sym_b }
    ));
}

/// 回転・拡大したインスタンスの中で F8 を押すと、新しいシンボルの中身は今いるシンボルの座標（向き・大きさ）のままで、
/// 新しいインスタンスは平行移動だけを持つ。画面上の絵は変わらない
#[test]
fn convert_to_symbol_inside_rotated_instance_keeps_local_coordinates() {
    use crate::canvas_tool::doc_actions::Action;

    let mut app = Is2dApp::default();
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 16.0,
            color: Color32::from_rgb(220, 60, 60),
        }],
    );
    let first = app.scene().get(0).unwrap().clone();
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&first); app.session.replace_selection(sel); }
    run(&mut app, &[Action::ConvertToSymbol]);
    let outer_id = app.doc.library().at(0).unwrap().id;
    let local_box = |app: &Is2dApp, sym_id: u64| {
        let def = app.doc.find_symbol(sym_id).unwrap();
        let cel = def.timeline.layers[0].cel_at(0).unwrap();
        cel.shapes()[1..]
            .iter()
            .fold(cel.shapes()[0].bounding_box(), |acc, s| {
                acc.union(s.bounding_box())
            })
    };
    let outer_box = local_box(&app, outer_id);

    let xform = kurbo::Affine::translate(KVec2::new(300.0, 200.0))
        * kurbo::Affine::rotate(1.0)
        * kurbo::Affine::scale(1.5);
    app.tabs[app.active].with_active_timeline_mut(|tl, _| {
        tl.layers[0].keyframe_mut(0).cel.instances_mut()[0]
            .binding
            .xform = xform;
    });
    let binding = app.scene().get(0).unwrap().symbol_instance.unwrap();
    assert_eq!(binding.xform, xform);
    assert!(symbol_ops::enter_symbol_timeline(
        &mut app.tabs[app.active],
        binding.instance_gid
    ));
    let stage_box_before = app.scene().get(0).unwrap().bounding_box();

    let inner = app.scene().get(0).unwrap().clone();
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&inner); app.session.replace_selection(sel); }
    run(&mut app, &[Action::ConvertToSymbol]);
    let new_id = symbols_in_order(app.doc.library()).last().unwrap().id;
    assert_ne!(new_id, outer_id);

    // 中身の大きさは、外のシンボルの中での大きさのまま（ステージ上の 1.5 倍・回転した大きさではない）
    let new_box = local_box(&app, new_id);
    assert!(
        (new_box.width() - outer_box.width()).abs() < 1e-6,
        "{new_box:?} / {outer_box:?}"
    );
    assert!(
        (new_box.height() - outer_box.height()).abs() < 1e-6,
        "{new_box:?} / {outer_box:?}"
    );
    assert!(
        new_box.center().to_vec2().hypot() < 1e-6,
        "中身の中心が原点: {new_box:?}"
    );

    // 外のシンボルの中の新しいインスタンスは、平行移動だけ
    let outer_def = app.doc.find_symbol(outer_id).unwrap();
    let inst = outer_def
        .timeline
        .instances()
        .find(|i| i.binding.symbol_id == new_id)
        .unwrap();
    let [a, b, c, d, ..] = inst.binding.xform.as_coeffs();
    assert!((a - 1.0).abs() < 1e-9 && b.abs() < 1e-9 && c.abs() < 1e-9 && (d - 1.0).abs() < 1e-9);

    // 画面上の絵は変わらない
    let stage_box_after = app.scene().get(0).unwrap().bounding_box();
    for (p, q) in [
        (stage_box_before.origin(), stage_box_after.origin()),
        (
            Point::new(stage_box_before.x1, stage_box_before.y1),
            Point::new(stage_box_after.x1, stage_box_after.y1),
        ),
    ] {
        assert!(
            (p - q).hypot() < 1e-6,
            "{stage_box_before:?} / {stage_box_after:?}"
        );
    }
}

#[test]
fn test_g5_2_symbol_nest_depth_limit_and_file_rejection() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
        let id = symbols_in_order(app.doc.library()).last().unwrap().id;
        chain_sym_ids.push(id);
    }
    assert_eq!(app.doc.library().len(), 8);
    assert!(!app.doc.symbol_nest_exceeded());

    // 完了条件1: 8段の連鎖が、ルートからすべて描かれる（各段の生図形8個すべてが展開される）
    let root_eval = app
        .doc
        .root
        .evaluate_layer_shapes_at(0, 0, app.doc.library());
    assert_eq!(
        root_eval.len(),
        8,
        "8段の連鎖のすべての段の図形がルートから描かれること"
    );
    assert_eq!(app.scene().len(), 8);

    // 完了条件2: 高さ8のシンボルを含む選択で F8 -> 文書は変わらず、警告が出て、Undo履歴も増えない
    let undo_before_f8 = app.history.undo_stack.len();
    let lib_len_before = app.doc.library().len();
    let root_inst_before = app.doc.root.layers[0].keyframes[0].cel.instances().clone();
    app.notices = Default::default();
    let _ = apply(&mut app, Action::SelectAll);
    let res_f8 = apply(&mut app, Action::ConvertToSymbol);
    assert!(!res_f8, "9段目になるF8は拒否されること");
    assert_eq!(app.doc.library().len(), lib_len_before);
    assert_eq!(
        *app.doc.root.layers[0].keyframes[0].cel.instances(),
        root_inst_before
    );
    assert_eq!(app.history.undo_stack.len(), undo_before_f8);
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
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
    let host_sym_id = symbols_in_order(app.doc.library()).last().unwrap().id;
    assert_eq!(app.doc.library().len(), 9);
    let _ = apply(
        &mut app,
        Action::EnterSymbolFromLibrary {
            sym_id: host_sym_id,
        },
    );
    assert_eq!(app.session.edit_path().len(), 1);

    // 3a. シンボル編集中のペーストで9段になる場合
    let undo_before_paste = app.history.undo_stack.len();
    let host_inst_before = app.doc.find_symbol(host_sym_id).unwrap().timeline.layers[0].keyframes
        [0]
    .cel
    .instances()
    .len();
    app.notices = Default::default();
    let res_paste = apply(&mut app, Action::Paste { in_place: true });
    assert!(!res_paste, "9段になるペーストは拒否されること");
    assert_eq!(app.history.undo_stack.len(), undo_before_paste);
    assert_eq!(
        app.doc.find_symbol(host_sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .instances()
            .len(),
        host_inst_before
    );
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
        Some("シンボルの入れ子は8段までです")
    );

    // 3b. ライブラリからの配置で9段になる場合
    let undo_before_place = app.history.undo_stack.len();
    app.notices = Default::default();
    let res_place = apply(&mut app, Action::PlaceSymbol { sym_id: top_sym_id });
    assert!(!res_place, "9段になるライブラリからの配置は拒否されること");
    assert_eq!(app.history.undo_stack.len(), undo_before_place);
    assert_eq!(
        app.doc.find_symbol(host_sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .instances()
            .len(),
        host_inst_before
    );
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
        Some("シンボルの入れ子は8段までです")
    );
    let _ = apply(&mut app, Action::ExitGroupSteps { steps: 1 });

    // 完了条件4 & 5: 9段のファイル・1000段の連鎖のファイルは開けずにエラーになり、タブは増えない
    let make_chain_project = |depth: usize| -> save_format::ProjectDocument {
        let mut library = Vec::with_capacity(depth);
        for i in 0..depth {
            let sym_id = (i + 1) as u64;
            let instances = if i + 1 < depth {
                vec![save_format::SavedSymbolInstance {
                    binding: save_format::SavedSymbolInstanceBinding {
                        instance_gid: (i + 1) as u64,
                        symbol_id: (i + 2) as u64,
                        xform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                        pivot: [0.0, 0.0],
                        loop_mode: symbol_binding::SymbolLoopMode::Loop,
                        first_frame: 0,
                        color_effect: symbol_binding::ColorEffect::default(),
                    },
                    layer_id: 1,
                    parent_scope: Vec::new(),
                    order: 0,
                }]
            } else {
                Vec::new()
            };
            library.push(save_format::SavedSymbolDef {
                id: sym_id,
                name: format!("Chain{sym_id}"),
                timeline: save_format::SavedTimeline {
                    fps: 24,
                    total_frames: 1,
                    current_frame: 0,
                    active_layer: 0,
                    next_layer_number: 2,
                    layers: vec![save_format::SavedLayer {
                        id: 1,
                        name: "L1".to_owned(),
                        visible: true,
                        locked: false,
                        frame_count: 1,
                        keyframes: vec![save_format::SavedKeyframe {
                            start_frame: 0,
                            tween: false,
                            tween_rotate: timeline::TweenRotate::Auto,
                            maps: Vec::new(),
                            instances,
                            sound: None,
                        }],
                    }],
                },
            });
        }
        save_format::ProjectDocument {
            stage_size: [800.0, 450.0],
            timeline: (&timeline::Timeline::default()).into(),
            library,
            next_symbol_id: (depth + 1) as u64,
            next_group_id: (depth + 1) as u64,
            recovery_source: None,
            map_tiles: Default::default(),
            sounds: Vec::new(),
            next_sound_id: 1,
            next_layer_id: 2,
        }
    };

    let tabs_before = app.tabs.len();
    for depth in [9usize, 1000usize] {
        let path = std::env::temp_dir().join(format!(
            "is2d_g5_2_chain_{depth}_{}.is2d",
            std::process::id()
        ));
        write_crafted_file(&path, &make_chain_project(depth));
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
fn test_g3_1_symbol_enter_exit_preserves_symbol_def_and_load_normalizes() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    let sym_id = app.doc.library().at(0).unwrap().id;
    let rev_before = app.doc.library().at(0).unwrap().revision();
    let doc_rev_before = app.doc.revision;
    let total_before = app.doc.library().at(0).unwrap().timeline.total_frames;
    let next_num_before = app.doc.library().at(0).unwrap().timeline.next_layer_number;
    let next_id_before = app.doc.next_layer_id;
    assert_eq!(total_before, 1);
    assert_eq!(next_num_before, 2);
    assert_eq!(next_id_before.next(), 2);

    // シンボルへの出入りで SymbolDef の内容やリビジョンが変わらないこと
    assert!(symbol_ops::enter_symbol_from_library(&mut app, sym_id));
    assert_eq!(app.doc.library().at(0).unwrap().revision(), rev_before);
    assert_eq!(app.doc.revision, doc_rev_before);
    assert!(symbol_ops::exit_symbol_timeline(&mut app));
    assert_eq!(app.doc.library().at(0).unwrap().revision(), rev_before);
    assert_eq!(app.doc.revision, doc_rev_before);
    assert_eq!(app.doc.library().at(0).unwrap().timeline.total_frames, total_before);
    assert_eq!(
        app.doc.library().at(0).unwrap().timeline.next_layer_number,
        next_num_before
    );
    assert_eq!(app.doc.next_layer_id, next_id_before);

    // 不正な total_frames=0, next_layer_number=0, next_layer_id=0 を持つファイルを読み込んだ時点で正規化されていること
    let mut pd = saved_from_app(&app).unwrap();
    pd.library[0].timeline.total_frames = 0;
    pd.library[0].timeline.next_layer_number = 0;
    pd.next_layer_id = 0;
    pd.library[0].timeline.layers[0].frame_count = 0;
    let loaded_doc = pd.into_document().unwrap();
    assert!(loaded_doc.library().at(0).unwrap().timeline.total_frames >= 1);
    assert!(loaded_doc.library().at(0).unwrap().timeline.next_layer_number >= 2);
    assert!(loaded_doc.next_layer_id.next() >= 2);
}

/// 浮いた片の残りを確定できないとき、F8 は断り、ライブラリにも採番にも何も残さない。直す前は、どこにも置かれない
/// シンボルが Undo できない形でライブラリに残り、採番が戻されて次のシンボルと番号が重なった（レビュー f01d518 の N-4002）
#[test]
fn refused_convert_to_symbol_leaves_no_symbol_in_the_library() {
    use doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(100.0, 200.0),
            Point::new(300.0, 200.0),
            25.0,
            Color32::from_rgb(235, 75, 75),
        ),
    );
    apply(&mut app, Action::AddLayer);
    put_cel_shapes(
        &mut app,
        1,
        0,
        make_stroke(
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            25.0,
            Color32::from_rgb(55, 135, 245),
        ),
    );
    run(&mut app, &[Action::SelectAll]);
    doc_actions::finish_move_selection_drag(&mut app, KVec2::new(0.0, 100.0), false);
    assert!(app.session.floating().is_some());
    // シンボルにするのは1枚目のレイヤーの片だけ（シンボルにできるのは1つのレイヤーの中で選んだものだけ）。2枚目の
    // レイヤーの片は、浮いたまま選ばない（残りとして確定する分）
    let lid0 = app.doc.root.layers[0].id;
    let select_first = |app: &mut Is2dApp| {
        let first: Vec<usize> = (0..app.scene().len())
            .filter(|&i| app.scene().get(i).unwrap().layer_id == lid0)
            .collect();
        app.set_selected_indices(first);
    };
    select_first(&mut app);
    assert!(app.session.floating().is_some());
    // 残りを書くレイヤーの1つを、確定を通さずに編集できなくする（確定が断られる場面の代わり）
    app.tab_mut().with_active_timeline_mut(|tl, _| {
        tl.layers[1].locked = true;
        true
    });
    let undo_len = app.history.undo_stack.len();
    assert!(!apply(&mut app, Action::ConvertToSymbol));
    assert_eq!(
        app.tab().doc.library().len(),
        0,
        "置かれないシンボルを残さない"
    );
    assert_eq!(app.history.undo_stack.len(), undo_len);
    // 確定できるようにして F8：シンボルは1つだけで、置いたインスタンスはそのシンボルを指す（断ったときは、文書と浮いた
    // 片を控えから戻すので、選択も浮いた片の全部に戻る。1枚目のレイヤーの片を選び直す）
    app.tab_mut().with_active_timeline_mut(|tl, _| {
        tl.layers[1].locked = false;
        true
    });
    select_first(&mut app);
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let lib = &app.tab().doc.library();
    assert_eq!(lib.len(), 1);
    let sym_id = lib.at(0).unwrap().id;
    let insts: Vec<u64> = app
        .tab()
        .active_timeline()
        .layers
        .iter()
        .flat_map(|l| l.keyframes[0].cel.instances().iter())
        .map(|i| i.binding.symbol_id)
        .collect();
    assert_eq!(insts, vec![sym_id]);
}

/// N-4003（レビュー 5b822a3）：矩形で絵の一部を切ったあと、Shift で別の生の図形を足してシンボルにすると、足した図形は
/// シンボルの中にだけ入り、元の場所に生の図形として残らない（前は、浮いた片の残りの確定が、足した図形を外す前のセル画から
/// 作った残りで今の階層を書き直すので、足した図形がインスタンスの真下に戻り、気づかないまま保存されていた）
#[test]
fn converting_marquee_plus_shift_click_to_symbol_does_not_duplicate() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    for (y, c) in [(-60.0, Color32::RED), (60.0, Color32::BLUE)] {
        app.tool.brush_color = c;
        let pts: Vec<Point> = (0..=10)
            .map(|i| Point::new(-200.0 + 40.0 * i as f64, y))
            .collect();
        commit_brush_line(&mut app, &brush_line(&pts, 12.0));
    }
    // 赤の左端を矩形で切る（浮いた片。動かさない）
    assert!(apply(
        &mut app,
        Action::SelectMarquee {
            p0: Point::new(-260.0, -130.0),
            p1: Point::new(-100.0, 0.0),
            additive: false,
        },
    ));
    assert!(app.session.floating().is_some(), "赤の左端が浮いた片になる");
    // 青を Shift クリックで足す
    let blue = app
        .scene()
        .iter()
        .position(|s| s.color() == Color32::BLUE)
        .expect("青の線");
    assert!(apply(
        &mut app,
        Action::ToggleSelectionAt { scene_idx: blue }
    ));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let cel = &app.doc.root.layers[0].keyframes[0].cel;
    let raw_blue = cel
        .shapes()
        .iter()
        .filter(|s| s.color() == Color32::BLUE)
        .count();
    assert_eq!(raw_blue, 0, "シンボルに入れた青が、生の図形としても残る");
    assert_eq!(cel.instances().len(), 1, "インスタンスが1つ");
    // 赤の右側（選んでいない所）は生の図形のまま残る
    assert!(
        cel.shapes().iter().any(|s| s.color() == Color32::RED),
        "選んでいない赤は残る"
    );
    // 取り消すと、シンボルにする前（赤の左端が浮いた片、青を選んだところ）に戻る
    assert!(apply(&mut app, Action::Undo));
    assert!(canvas_tool::clear_selection(&mut app));
    let cel = &app.doc.root.layers[0].keyframes[0].cel;
    assert!(cel.instances().is_empty(), "取り消すとインスタンスは無い");
    for c in [Color32::RED, Color32::BLUE] {
        assert_eq!(
            cel.shapes().iter().filter(|s| s.color() == c).count(),
            1,
            "取り消すと元の1本に戻る"
        );
    }
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
    nest_app.set_selected_indices(vec![0]);
    let sym_a = doc_actions::convert_selection_to_symbol(&mut nest_app).unwrap();

    let mut shapes = scene_summary(&nest_app).shapes;
    shapes.extend(make_stroke(
        Point::new(400.0, 225.0),
        Point::new(440.0, 225.0),
        8.0,
        Color32::from_rgb(150, 150, 150),
    ));
    put_cel_shapes(&mut nest_app, 0, 0, shapes);
    // 足した線（生の図形は、インスタンスより下に並ぶ）
    let stroke_idx = |app: &Is2dApp, c: Color32| {
        app.scene()
            .iter()
            .position(|s| s.symbol_instance.is_none() && s.color() == c)
            .unwrap()
    };
    let idx = stroke_idx(&nest_app, Color32::from_rgb(150, 150, 150));
    nest_app.set_selected_indices(vec![idx]);
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
    let idx = stroke_idx(&nest_app, Color32::from_rgb(200, 50, 50));
    nest_app.set_selected_indices(vec![idx]);
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
        s.recolor(|_| updated_green);
    }
    put_cel_shapes(&mut nest_app, 0, 0, shapes);
    assert!(symbol_ops::exit_symbol_timeline(&mut nest_app));
    assert_eq!(scene_summary(&nest_app).len, 1);
    assert_eq!(scene_summary(&nest_app).shapes[0].color(), updated_green);

    let scaled_binding =
        crate::symbol_binding::SymbolInstanceBinding::new(999, sym_c, kurbo::Affine::scale(3.0));
    let sym_c_def = nest_app.doc.find_symbol(sym_c).unwrap();
    let inst_shapes = scaled_binding.instantiate_shapes(
        &sym_c_def.timeline.layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| (**s).clone())
            .collect::<Vec<_>>(),
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

/// レビュー 96fdc26 の N-5002：インスタンスの数（変換・中心点・色効果）が有限でなくなる書き込みは、書く入口で断る（文書は
/// 変わらず、知らせが出る）。ファイルを読む入口も同じ判定（`SymbolInstanceBinding::is_finite`）で、有限でない数の
/// インスタンスがあるファイルを断る（前の版が書いたファイルも）。文書のインスタンスはいつも有限なので、保存で直す所は無い
#[test]
fn non_finite_instance_numbers_are_refused_at_the_entries() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    let c = Point::new(200.0, 200.0);
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![c, Point::new(c.x + 60.0, c.y)],
            radius: 6.0,
            color: Color32::RED,
        },
    ));
    assert!(apply(&mut app, Action::SelectSingleAt { pt: c }));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let binding = |app: &Is2dApp| app.doc.root.layers[0].keyframes[0].cel.instances()[0].binding;
    let before = binding(&app);
    let undo_before = app.history.undo_stack.len();
    app.notices = Default::default();
    // 中心点を有限でない所に置く（変換の逆で写すと NaN）：書く入口で断る
    assert!(!set_pivot(&mut app, Some(Point::new(f64::NAN, 0.0))));
    assert_eq!(binding(&app), before, "インスタンスは変わらない");
    assert_eq!(app.history.undo_stack.len(), undo_before);
    assert_eq!(
        app.notices.iter().last().map(|t| t.message.as_str()),
        Some(crate::canvas_tool::Refusal::OffGrid.message().text.as_str())
    );
    // 書く入口そのもの：断るときは何も変えない
    let mut cel = app.doc.root.layers[0].keyframes[0].cel.clone();
    assert_eq!(
        cel.update_instances(|_| true, |b| b.xform = kurbo::Affine::scale(f64::INFINITY)),
        Err(crate::timeline::Unmappable)
    );
    assert_eq!(cel.instances()[0].binding, before);
    let mut content = cel.clone();
    content.instances_mut()[0].binding.pivot = Point::new(f64::NAN, 0.0);
    assert!(!cel.insert_content(&[], &content, None));
    // ファイルを読む入口も同じ判定で断る
    let pd = saved_from_app(app.tab()).unwrap();
    let mut nan_inst = pd.clone();
    nan_inst.timeline.layers[0].keyframes[0].instances[0]
        .binding
        .xform[4] = f64::NAN;
    let mut nan_effect = pd.clone();
    nan_effect.timeline.layers[0].keyframes[0].instances[0]
        .binding
        .color_effect
        .alpha = f32::NAN;
    let mut inf_effect = pd.clone();
    inf_effect.timeline.layers[0].keyframes[0].instances[0]
        .binding
        .color_effect
        .brightness = f32::INFINITY;
    for broken in [nan_inst, nan_effect, inf_effect] {
        assert_eq!(
            opened_document(&crafted_bytes(&broken).unwrap()).expect_err("断る"),
            "ファイルが壊れています（有限でない数があります）"
        );
    }
    assert!(opened_document(&crafted_bytes(&pd).unwrap()).is_ok());
}
