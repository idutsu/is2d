//! タイムライン（コマ・キーフレーム・レイヤー・トゥイーン・オニオンスキン・再生）のテスト

use super::*;

/// Undo の控え（文書の複製）と、そのあと1つのキーフレームを書き換えた列は、触った塊のほかを共有する（N-2004）
/// 鍵の順の列のまとめての差し替え（`ChunkedList::replace_sorted`）と鍵で引く所（`find_sorted`）が、ランダムな書き換えを
/// 重ねても、ふつうの並び（`BTreeMap`）と同じ中身・同じ引き方になり、かからない塊は共有し、塊の大きさがそろう
#[test]
fn chunked_list_sorted_replace_matches_a_plain_sorted_list() {
    use crate::chunked_list::ChunkedList;
    use std::sync::Arc;
    let mut seed = 0x2026_1120_u64;
    let mut rnd = |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m
    };
    let key = |x: &(u32, u64)| x.0;
    for start in [0usize, 1, 30, 64, 65, 500, 3000] {
        let mut model: std::collections::BTreeMap<u32, u64> =
            (0..start as u32).map(|k| (k * 3, k as u64)).collect();
        let mut list: ChunkedList<(u32, u64)> =
            model.iter().map(|(&k, &v)| Arc::new((k, v))).collect();
        for step in 0..400u64 {
            let span = (model.len() as u64 * 3 + 30).max(30);
            let mut gone: Vec<u32> = (0..rnd(6)).map(|_| rnd(span) as u32).collect();
            if rnd(10) == 0 {
                let a = rnd(span) as u32;
                gone.extend(a..a + rnd(200) as u32);
            }
            gone.sort_unstable();
            gone.dedup();
            for g in &gone {
                model.remove(g);
            }
            let mut added: Vec<u32> = (0..rnd(6))
                .map(|_| rnd(span) as u32)
                .filter(|k| !model.contains_key(k))
                .collect();
            if rnd(10) == 0 {
                let a = rnd(span) as u32;
                added.extend((a..a + rnd(200) as u32).filter(|k| !model.contains_key(k)));
            }
            added.sort_unstable();
            added.dedup();
            for &a in &added {
                model.insert(a, step);
            }
            let before = list.clone();
            let touches = gone.len() + added.len();
            list.replace_sorted(
                &gone,
                added.iter().map(|&k| Arc::new((k, step))).collect(),
                key,
            );
            let got: Vec<(u32, u64)> = list.iter().map(|x| **x).collect();
            let want: Vec<(u32, u64)> = model.iter().map(|(&k, &v)| (k, v)).collect();
            assert_eq!(got, want, "start {start} step {step}");
            assert_eq!(list.len(), want.len());
            for _ in 0..5 {
                let k = rnd(span) as u32;
                assert_eq!(
                    list.find_sorted(k, key).map(|x| x.1),
                    model.get(&k).copied()
                );
            }
            let (shared, total) = list.shared_chunks_with(&before);
            let (_, before_total) = before.shared_chunks_with(&before);
            if before_total > touches + 2 {
                assert!(
                    shared > 0,
                    "start {start} step {step}: 塊を共有していない（{shared}/{total}）"
                );
            }
            // 塊の大きさ：差し替えで作った塊は 64 個以下で、塊が細かく割れていかない（列を作るときと、1つずつ入れて
            // 分けるときの決まりの塊は、そのまま共有する）
            let sizes = list.chunk_sizes();
            assert!(
                total <= list.len() / 16 + 2,
                "start {start} step {step}: 塊が細かく割れた {sizes:?}"
            );
            assert!(
                sizes.iter().all(|&n| n > 0 && n <= 128),
                "start {start} step {step}: 塊の大きさ {sizes:?}"
            );
        }
    }
}

/// 塊の並びの比べ（`same_items`）は、要素を1つずつ比べた答え（お手本）と同じ：途中に足す・消す・消して同じ所に戻す
/// （中身は同じで塊の切れ目だけがずれる）をでたらめにくり返した写しと比べる（全体の点検 2026-11-27）
#[test]
fn chunked_list_comparison_matches_item_by_item() {
    use crate::chunked_list::ChunkedList;
    let mut rng = OpRng(0x0c4a_0001);
    for round in 0..200 {
        let n = 1 + rng.range(400);
        let base: ChunkedList<u32> = (0..n as u32).map(std::sync::Arc::new).collect();
        let mut copy = base.clone();
        for _ in 0..rng.range(6) {
            let i = rng.range(copy.len().max(1));
            match rng.range(3) {
                0 => copy.insert(i.min(copy.len()), std::sync::Arc::new(9999)),
                1 if !copy.is_empty() => {
                    copy.remove(i);
                }
                _ if !copy.is_empty() => {
                    let x = copy.remove(i);
                    copy.insert(i, std::sync::Arc::new(*x));
                }
                _ => {}
            }
        }
        let naive = base.len() == copy.len() && base.iter().zip(copy.iter()).all(|(a, b)| a == b);
        assert_eq!(base.same_items(&copy, |a, b| a == b), naive, "round={round}");
    }
}

/// 塊の並びの範囲の消去（`remove_range`）は、ふつうの並びから同じ範囲を消した答え（お手本）と同じで、範囲にかからない
/// 塊は写さずに共有したまま（全体の点検 2026-11-27）
#[test]
fn chunked_list_remove_range_matches_a_plain_vec() {
    use crate::chunked_list::ChunkedList;
    let mut rng = OpRng(0x0c4a_0002);
    for round in 0..300 {
        let n = 1 + rng.range(500);
        let base: ChunkedList<u32> = (0..n as u32).map(std::sync::Arc::new).collect();
        let mut list = base.clone();
        let mut plain: Vec<u32> = (0..n as u32).collect();
        for _ in 0..1 + rng.range(4) {
            if plain.is_empty() {
                break;
            }
            let a = rng.range(plain.len());
            let b = a + rng.range(plain.len() - a + 1);
            list.remove_range(a..b);
            plain.drain(a..b);
        }
        assert_eq!(list.iter().map(|x| **x).collect::<Vec<_>>(), plain, "round={round}");
        assert_eq!(list.len(), plain.len());
    }
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

    insert_keyframe_at(&mut app.doc.root, 0, 4, false, 24);
    assert_eq!(app.doc.root.layers[0].frame_count, 5);
    assert_eq!(app.doc.root.layers[0].keyframe_index_at(3), Some(0));
    assert_eq!(app.doc.root.layers[0].keyframe_index_at(4), Some(1));
    assert_eq!(app.doc.root.layers[0].cel_at(2).unwrap().shapes().len(), 1);
    assert!(app.doc.root.layers[0].cel_at(4).unwrap().is_empty());

    insert_keyframe_at(&mut app.doc.root, 0, 3, true, 24);
    assert_eq!(app.doc.root.layers[0].cel_at(3).unwrap().shapes().len(), 1);
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

    assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
    assert_eq!(app.doc.root.current_frame, 1);
    assert_eq!(scene_summary(&app).shapes.len(), 1);

    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 2);
    assert!(scene_summary(&app).shapes.is_empty());

    let prev_total = app.doc.root.total_frames;
    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.total_frames, prev_total + 1);
    assert!(doc_actions::timeline_remove_frame(&mut app));
    assert_eq!(app.doc.root.total_frames, prev_total);

    doc_actions::select_timeline_cells(&mut app, 0, 9, 0, 9);
    assert_eq!(app.doc.root.current_frame, 9);
    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.layers[0].frame_count, 10);
    assert_eq!(app.doc.root.total_frames, 10);
    app.undo();
    assert_eq!(app.doc.root.total_frames, prev_total);
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);

    assert!(doc_actions::timeline_delete_keyframe(&mut app));
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

    doc_actions::timeline_add_layer(&mut app);
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
    paint_rect(
        &mut app,
        &[
            Point::new(120.0, 70.0),
            Point::new(120.0, 130.0),
            Point::new(135.0, 130.0),
            Point::new(135.0, 70.0),
        ],
    );
    assert_eq!(app.doc.root.layers[0].cel_at(0).unwrap().shapes().len(), 1);
    assert_eq!(app.doc.root.layers[1].cel_at(0).unwrap().shapes().len(), 1);
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
        doc_actions::Action::NudgeSelection {
            delta: KVec2::new(30.0, 0.0),
        },
    );
    assert!(canvas_tool::clear_selection(&mut app));
    assert!(app.doc.root.layers[0].cel_at(0).unwrap().shapes()[0]
        .contains_point(Point::new(155.0, 100.0)));
    assert!(app.doc.root.layers[1].cel_at(0).unwrap().shapes()[0]
        .contains_point(Point::new(155.0, 100.0)));

    doc_actions::set_layer_locked(&mut app, 0, true);
    select_ops::apply_marquee_selection(
        &mut app,
        Point::new(50.0, 50.0),
        Point::new(250.0, 200.0),
        false,
    );
    assert_eq!(app.session.selected_indices().len(), 1);
    assert!(canvas_tool::clear_selection(&mut app));
    doc_actions::set_layer_locked(&mut app, 0, false);
    assert_eq!(scene_summary(&app).shapes.len(), 2);
}

#[test]
fn test_timeline_layer_reorder_move_and_delete() {
    let mut app = Is2dApp::default();
    doc_actions::timeline_add_layer(&mut app);
    doc_actions::timeline_add_layer(&mut app);
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

    assert!(doc_actions::timeline_reorder_layer(&mut app, 2, 0));
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

    assert!(doc_actions::timeline_reorder_layer(&mut app, 0, 1));
    assert_eq!(app.doc.root.active_layer, 1);
    assert!(doc_actions::timeline_delete_active_layer(&mut app));
    assert_eq!(app.doc.root.layers.len(), 2);
    assert!(doc_actions::timeline_delete_active_layer(&mut app));
    assert_eq!(app.doc.root.layers.len(), 1);
}

#[test]
fn test_timeline_cell_selection_sync_with_canvas_selection() {
    let mut app = Is2dApp::default();
    let blue = Color32::from_rgb(55, 135, 245);
    paint_rect(
        &mut app,
        &[
            Point::new(40.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(80.0, 80.0),
            Point::new(40.0, 80.0),
        ],
    );
    doc_actions::timeline_add_layer(&mut app);
    app.tool.brush_color = blue;
    paint_rect(
        &mut app,
        &[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ],
    );
    let base_len = app.doc.root.layers[0].frame_count;

    doc_actions::select_timeline_cells(&mut app, 1, 0, 1, 0);
    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.layers[0].frame_count, base_len);
    assert_eq!(app.doc.root.layers[1].frame_count, base_len + 1);

    doc_actions::scrub_ruler_to_frame(&mut app, 0);
    assert!(app.doc.root.cell_selection.is_none());
    assert!(doc_actions::timeline_extend_frame(&mut app));
    assert_eq!(app.doc.root.layers[0].frame_count, base_len + 1);
    assert_eq!(app.doc.root.layers[1].frame_count, base_len + 2);

    doc_actions::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert_eq!(app.session.selected_indices().len(), 1);
    assert_eq!(
        scene_summary(&app).shapes[app.session.selected_indices()[0]].layer_id,
        app.doc.root.layers[0].id
    );

    assert!(canvas_tool::clear_selection(&mut app));
    doc_actions::scrub_ruler_to_frame(&mut app, 0);
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::SelectSingleAt {
            pt: Point::new(160.0, 60.0),
        }
    ));
    assert_eq!(app.doc.root.active_layer, 1);
    assert_eq!(
        app.doc
            .root
            .cell_selection
            .map(|s| (s.min_layer, s.max_layer)),
        Some((1, 1))
    );

    doc_actions::select_timeline_cells(&mut app, 0, 0, 1, 0);
    assert_eq!(app.session.selected_indices().len(), 2);
}

#[test]
fn test_timeline_move_and_copy_frames_and_shift_f5_keyframe_removal() {
    let mut app = Is2dApp::default();
    let blue = Color32::from_rgb(55, 135, 245);
    paint_rect(
        &mut app,
        &[
            Point::new(40.0, 40.0),
            Point::new(80.0, 40.0),
            Point::new(80.0, 80.0),
            Point::new(40.0, 80.0),
        ],
    );
    doc_actions::timeline_add_layer(&mut app);
    app.tool.brush_color = blue;
    paint_rect(
        &mut app,
        &[
            Point::new(140.0, 40.0),
            Point::new(180.0, 40.0),
            Point::new(180.0, 80.0),
            Point::new(140.0, 80.0),
        ],
    );
    doc_actions::scrub_ruler_to_frame(&mut app, 0);
    doc_actions::timeline_extend_frame(&mut app);

    let sel_l1_f0 = timeline::TimelineCellSelection::single(1, 0);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut app, sel_l1_f0, 0, 3, true
    ));
    assert!(!app.doc.root.layers[1].cel_at(0).unwrap().is_empty());
    assert_eq!(app.doc.root.layers[1].keyframe_index_at(3), Some(1));
    assert!(!app.doc.root.layers[1].cel_at(3).unwrap().is_empty());

    // コマ 4 のキーフレームをコマ 6 へ動かす：空いたコマ 4 は前のキーフレーム（コマ 1）の絵が続く（Adobe Animate と同じ。
    // 空白キーフレームや写しのキーフレームはできない）
    let sel_l1_f3 = timeline::TimelineCellSelection::single(1, 3);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut app, sel_l1_f3, 0, 2, false
    ));
    let starts = |app: &Is2dApp, l: usize| -> Vec<usize> {
        app.doc.root.layers[l]
            .keyframes_with_start()
            .map(|(s, _)| s)
            .collect()
    };
    assert_eq!(starts(&app, 1), vec![0, 5]);
    assert_eq!(app.doc.root.layers[1].keyframe_index_at(3), Some(0));
    assert!(!app.doc.root.layers[1].cel_at(3).unwrap().is_empty());
    assert!(!app.doc.root.layers[1].cel_at(5).unwrap().is_empty());

    // コマ 1 のキーフレームをコマ 3 へ動かす：コマ 1 だけは空白キーフレームになり、コマ 2 も空（その続き）
    let sel_l0_f0 = timeline::TimelineCellSelection::single(0, 0);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut app, sel_l0_f0, 0, 2, false
    ));
    assert_eq!(starts(&app, 0), vec![0, 2]);
    assert!(app.doc.root.layers[0].cel_at(0).unwrap().is_empty());
    assert!(app.doc.root.layers[0].cel_at(1).unwrap().is_empty());
    assert!(!app.doc.root.layers[0].cel_at(2).unwrap().is_empty());

    insert_keyframe_at(&mut app.doc.root, 1, 3, false, 24);
    insert_keyframe_at(&mut app.doc.root, 1, 4, false, 24);
    let kf_count_before = app.doc.root.layers[1].keyframes.len();
    doc_actions::select_timeline_cells(&mut app, 1, 0, 1, 0);
    assert!(doc_actions::timeline_remove_frame(&mut app));
    assert_eq!(app.doc.root.layers[1].keyframes.len(), kf_count_before);

    doc_actions::select_timeline_cells(&mut app, 1, 2, 1, 2);
    assert!(doc_actions::timeline_remove_frame(&mut app));
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
            crate::symbol::SymbolEvalCtx::new(app.doc.library()),
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
            crate::symbol::SymbolEvalCtx::new(app.doc.library()),
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
fn test_classic_tween_translation_and_custom_pivot_ccw_rotation() {
    use crate::timeline::TweenState;

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
    doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut app));
    for _ in 0..4 {
        assert!(doc_actions::timeline_extend_frame(&mut app));
    }
    assert_eq!(app.doc.root.total_frames, 5);
    assert_eq!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Incomplete
    );

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 4);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
    assert!(matches!(
        app.doc.root.layers[0].tween_state_at_keyframe(0),
        TweenState::Complete {
            start_frame: 0,
            end_frame: 4,
            ..
        }
    ));

    app.set_selected_indices(vec![0]);
    doc_actions::nudge_selected_shapes(&mut app, KVec2::new(200.0, 80.0));

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_bbox = scene_summary(&app).shapes[0].bounding_box();
    let mid_cx = (mid_bbox.x0 + mid_bbox.x1) * 0.5;
    let mid_cy = (mid_bbox.y0 + mid_bbox.y1) * 0.5;
    assert!((mid_cx - 250.0).abs() < 1.0);
    assert!((mid_cy - 190.0).abs() < 1.0);

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    let start_bbox = scene_summary(&app).shapes[0].bounding_box();
    let start_cx = (start_bbox.x0 + start_bbox.x1) * 0.5;
    let start_cy = (start_bbox.y0 + start_bbox.y1) * 0.5;
    assert!((start_cx - 150.0).abs() < 1.0);
    assert!((start_cy - 150.0).abs() < 1.0);

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 4);
    app.set_selected_indices(vec![0]);
    doc_actions::nudge_selected_shapes(&mut app, KVec2::new(-200.0, -80.0));

    let joint_world = Point::new(100.0, 150.0);
    assert!(set_pivot(
        &mut app,
        Some(joint_world),
    ));
    assert!(canvas_tool::clear_selection(&mut app));
    app.set_selected_indices(vec![0]);
    let bbox_f4 = app
        .scene()
        .selection_bounding_box(app.session.selection(), app.view.zoom())
        .unwrap();
    assert!(
        (app.scene().active_transform_pivot(
            app.session.selection(),
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

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    let mid_binding = scene_summary(&app).shapes[0].symbol_instance.unwrap();
    assert!((mid_binding.world_pivot() - joint_world).hypot() < 1e-4);
    let expected_mid_center = Point::new(
        100.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        150.0 + 50.0 * std::f64::consts::FRAC_1_SQRT_2,
    );
    assert!(scene_summary(&app).shapes[0].contains_point(expected_mid_center));

    doc_actions::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert!(app.view.inspect_timeline_frame);
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SelectSingleAt {
            pt: Point::new(150.0, 150.0),
        },
    );
    assert!(!app.view.inspect_timeline_frame);

    doc_actions::select_timeline_cells(&mut app, 0, 0, 0, 0);
    assert!(doc_actions::set_active_keyframe_tween_rotate(
        &mut app,
        crate::timeline::TweenRotate::CounterClockwise
    ));
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    let expected_ccw_mid_center = Point::new(
        100.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
        150.0 - 50.0 * std::f64::consts::FRAC_1_SQRT_2,
    );
    assert!(scene_summary(&app).shapes[0].contains_point(expected_ccw_mid_center));
}

#[test]
fn test_timeline_resize_total_frames_and_fill_shape_lazy_retessellation() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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

    let small_raw = brush_line(&[Point::new(100.0, 100.0)], 4.0);
    let mut small_circle = brush_line_shapes(&small_raw, Color32::WHITE)
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
    grp_sym_app.set_selected_indices(vec![0]);
    let blink_sym_id = doc_actions::convert_selection_to_symbol(&mut grp_sym_app).unwrap();
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
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(doc_actions::clear_active_frame_shapes(&mut grp_sym_app));
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
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

    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(doc_actions::clear_active_frame_shapes(&mut grp_sym_app));
    doc_actions::select_timeline_cells(&mut grp_sym_app, 0, 2, 0, 2);
    assert!(doc_actions::timeline_extend_frame(&mut grp_sym_app));
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    assert!(
        doc_actions::place_symbol_instance_from_library(&mut grp_sym_app, blink_sym_id),
        "Frame 0 が空白のマルチフレームシンボルでもライブラリからステージへ配置できること"
    );
    assert_eq!(
        grp_sym_app.doc.root.layers[0].keyframes[0]
            .cel
            .instances()
            .len(),
        1,
        "Frame 0 時点で Cel::instances にシンボルインスタンスが登録されていること"
    );
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 1);
    assert_eq!(
        scene_summary(&grp_sym_app).len,
        1,
        "Frame 1 に進むと配置したシンボルの2コマ目の図形がステージ中央に出現すること"
    );
    let stage_center = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
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
            crate::symbol::SymbolEvalCtx::new(grp_sym_app.doc.library()),
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
            crate::symbol::SymbolEvalCtx::new(grp_sym_app.doc.library()),
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
            crate::symbol::SymbolEvalCtx::new(grp_sym_app.doc.library()),
        )
        .unwrap();
    assert!(
        (m_scr_panned.vertices[0].pos.x - (m_scr_2.vertices[0].pos.x + 50.0)).abs() < 1e-3,
        "pan の移動量が頂点スクリーン座標に正確に反映されていること"
    );

    let m_onion_1 = grp_sym_app
        .cached_onion_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, WHOLE_VIEW, ([35, 175, 105], 90))
        .unwrap();
    let m_onion_2 = grp_sym_app
        .cached_onion_cel_screen_mesh(0, 1, 1.0, Vec2::ZERO, WHOLE_VIEW, ([35, 175, 105], 90))
        .unwrap();
    // ズーム・パン・色の変え方が同じなら、画面に写したものも写しから返す（写し直さない）
    assert!(std::sync::Arc::ptr_eq(&m_onion_1, &m_onion_2));

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
        grp_sym_app
            .doc
            .find_symbol(blink_sym_id)
            .unwrap()
            .timeline
            .layers[0]
            .keyframes[1]
            .cel
            .maps(),
    );
    let inst_b = grp_sym_app.doc.root.layers[0].keyframes[0].cel.instances()[0].binding;
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
    let mut shapes = scene_summary(&grp_sym_app).shapes;
    shapes.extend(make_stroke(
        Point::new(20.0, 20.0),
        Point::new(50.0, 20.0),
        8.0,
        Color32::GRAY,
    ));
    put_cel_shapes(&mut grp_sym_app, 0, 0, shapes);
    assert!(symbol_ops::enter_symbol_timeline(
        &mut grp_sym_app,
        inst_b.instance_gid
    ));
    let bd_m1 = grp_sym_app.cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW);
    assert!(
        !bd_m1.is_empty(),
        "背景シェイプが存在するため backdrop メッシュが生成されること"
    );
    let bd_m2 = grp_sym_app.cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW);
    assert!(
        bd_m1.len() == bd_m2.len()
            && bd_m1
                .iter()
                .zip(&bd_m2)
                .all(|(a, b)| std::sync::Arc::ptr_eq(a, b)),
        "キャッシュ済みの backdrop メッシュが返ること"
    );
    doc_actions::switch_to_layer_and_frame(&mut grp_sym_app, 0, 0);
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
        .maps();
    assert!(
        std::sync::Arc::ptr_eq(&f1_local_arc_before, f1_local_arc_after),
        "シンボルインプレイス編集で Undo 保存や別キーフレーム編集を挟んでも、未変更キーフレームのローカル Cel::shapes は Arc 共有（CoW）が維持されること"
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
    lru_tween_app.set_selected_indices(vec![0]);
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
    doc_actions::select_timeline_cells(&mut lru_tween_app, 0, 11, 0, 11);
    assert!(doc_actions::timeline_insert_keyframe(
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
                    crate::symbol::SymbolEvalCtx::new(lru_tween_app.doc.library()),
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
                crate::symbol::SymbolEvalCtx::new(lru_tween_app.doc.library()),
            )
            .unwrap();
        assert!(
            !std::sync::Arc::ptr_eq(first_m, &second_m) && first_m.vertices == second_m.vertices,
            "トゥイーンフレーム（frame={f}）はその場で同一頂点の Mesh を構築すること"
        );
    }
}

/// キーフレームの列（塊で共有する列）は、足す・消す・始まりのコマを書き換える・後ろをずらす・ふるいにかける、を続けても、
/// ふつうの配列と同じ中身になる（始まりのコマは、塊の基準と要素の差で持つ：レビュー f01d518 の R5）
#[test]
fn test_keyframe_list_matches_plain_vec() {
    use crate::timeline::{Keyframe, KeyframeList};
    let mut r = map_overlay::Rng(3);
    let mut list = KeyframeList::default();
    let mut plain: Vec<usize> = Vec::new();
    let mut next = 0usize;
    for step in 0..4000 {
        let op = (r.f() * 11.0) as usize;
        if op < 6 || plain.is_empty() {
            let i = (r.f() * (plain.len() + 1) as f64) as usize % (plain.len() + 1);
            list.insert_keyed(i, next, std::sync::Arc::new(Keyframe::new_blank()));
            plain.insert(i, next);
            next += 1;
        } else if op < 8 {
            let i = (r.f() * plain.len() as f64) as usize % plain.len();
            assert_eq!(list.key(i), plain[i]);
            list.remove(i);
            plain.remove(i);
        } else if op < 9 {
            let i = (r.f() * plain.len() as f64) as usize % plain.len();
            list.set_key(i, plain[i] + 1_000_000);
            plain[i] += 1_000_000;
        } else if op < 10 {
            // 後ろをずらす（減らすときは、0 を下回らない量だけ）
            let i = (r.f() * plain.len() as f64) as usize % plain.len();
            let min_after = plain[i..].iter().copied().min().unwrap_or(0) as i64;
            let delta = (r.f() * 200.0) as i64 - 100;
            let delta = delta.max(-min_after);
            list.shift_keys_from(i, delta);
            for f in &mut plain[i..] {
                *f = (*f as i64 + delta) as usize;
            }
        } else {
            // 番号の範囲を消す（`remove_range`）
            let a = (r.f() * plain.len() as f64) as usize % plain.len();
            let b = a + (r.f() * (plain.len() - a + 1) as f64) as usize % (plain.len() - a + 1);
            list.remove_range(a..b);
            plain.drain(a..b);
        }
        assert_eq!(list.len(), plain.len(), "step {step}");
        if step % 97 == 0 {
            let got: Vec<usize> = list.iter_keyed().map(|(f, _)| f).collect();
            assert_eq!(got, plain, "step {step}");
            for (i, f) in plain.iter().enumerate() {
                assert_eq!(list.key(i), *f);
            }
        }
    }
    let got: Vec<usize> = list.iter_keyed().map(|(f, _)| f).collect();
    assert_eq!(got, plain);
}

#[test]
fn test_keyframe_list_copy_shares_untouched_chunks() {
    use crate::timeline::Layer;
    let mut tl = timeline::Timeline {
        layers: vec![Layer::new_with_id_and_frames(1, "L", 3000)],
        ..Default::default()
    };
    for f in 1..3000 {
        insert_keyframe_at(&mut tl, 0, f, false, 24);
    }
    let layer = &mut tl.layers[0];
    let snapshot = layer.clone();
    layer.keyframe_mut(1500).tween = true;
    let (shared, total) = layer.keyframes.shared_chunks_with(&snapshot.keyframes);
    assert!(total > 20, "塊の数 {total}");
    assert_eq!(shared, total - 1, "触った塊だけが写される");
    assert!(!snapshot.keyframes[1500].tween, "控えは変わらない");
    assert!(layer.keyframes[1500].tween);
}

/// F5（コマを足す）と Shift+F5（コマを消す。範囲もまとめて）は、後ろのキーフレームをみな写さない。Undo の控えと共有
/// しない塊は、列の長さによらず数個だけ（後ろの塊は基準だけを変える：レビュー f01d518 の R5）
#[test]
fn test_frame_insert_and_remove_keep_later_keyframe_chunks_shared() {
    use crate::timeline::{Layer, Timeline, TimelineCellSelection};
    let mut tl = Timeline {
        layers: vec![Layer::new_with_id_and_frames(1, "L", 6000)],
        ..Default::default()
    };
    for f in (2..6000).step_by(2) {
        insert_keyframe_at(&mut tl, 0, f, false, 24);
    }
    let keyframes = tl.layers[0].keyframes.len();
    // キーフレームの始まりのコマ（保持の終わりから求める）
    let starts = |l: &Layer| -> Vec<usize> {
        (0..l.keyframes.len())
            .map(|i| {
                if i == 0 {
                    0
                } else {
                    l.span_end_for_keyframe(i - 1) + 1
                }
            })
            .collect()
    };

    // F5：コマ 11 を伸ばす。後ろのキーフレームは 1 コマ後ろへ
    let snapshot = tl.layers[0].clone();
    extend_frame_at(&mut tl, 0, 10);
    let layer = &tl.layers[0];
    let (shared, total) = layer.keyframes.shared_chunks_with(&snapshot.keyframes);
    assert!(total > 40, "塊の数 {total}");
    assert!(
        shared + 1 >= total,
        "F5 で写した塊 {} / {total}",
        total - shared
    );
    let s = starts(layer);
    assert_eq!((s[5], s[6], s[keyframes - 1]), (10, 13, 5999));
    assert_eq!(layer.frame_count, 6001);

    // Shift+F5：コマ 11 を消す。元に戻る
    let snapshot = tl.layers[0].clone();
    assert!(remove_frames_at(&mut tl, 0, 10, 1, 24));
    let layer = &tl.layers[0];
    let (shared, total) = layer.keyframes.shared_chunks_with(&snapshot.keyframes);
    assert!(
        shared + 1 >= total,
        "Shift+F5 で写した塊 {} / {total}",
        total - shared
    );
    assert_eq!(starts(layer), (0..6000).step_by(2).collect::<Vec<_>>());
    assert_eq!(layer.frame_count, 6000);

    // 範囲の Shift+F5：コマ 11〜1010 を消す。消すコマに始まるキーフレームは外れ、コマ 1011 のキーフレームがコマ 11 に来る
    tl.cell_selection = Some(TimelineCellSelection::from_corners(0, 10, 0, 1009));
    let snapshot = tl.layers[0].clone();
    assert!(tl.remove_frame_on_targets(&[0], 24));
    let layer = &tl.layers[0];
    let (shared, total) = layer.keyframes.shared_chunks_with(&snapshot.keyframes);
    assert!(
        shared + 3 >= total,
        "範囲の Shift+F5 で写した塊 {} / {total}",
        total - shared
    );
    let expected: Vec<usize> = (0..10)
        .step_by(2)
        .chain((1010..6000).step_by(2).map(|f| f - 1000))
        .collect();
    assert_eq!(starts(layer), expected);
    assert_eq!(layer.frame_count, 5000);
}

/// コマ `first` から `count` コマを一度に消す（`remove_frames`）と、キーフレームの並び（始まりのコマと、どのキーフレーム
/// か）は、1コマずつ消す元の決まり（Adobe Animate の Shift+F5）を `count` 回くり返したのと同じになる（レビュー f01d518 の R5）
#[test]
fn test_remove_frames_matches_repeated_single_frame_removal() {
    use crate::timeline::{Cel, Keyframe, KeyframeList, Layer, SymbolInstance, TweenRotate};
    // 元の1コマの削除（直す前の `remove_frame_at`）。キーフレームは（始まりのコマ、印。空の絵は 0）
    fn reference_remove(kfs: &mut Vec<(usize, u64)>, n: &mut usize, f: usize) -> bool {
        if f >= *n {
            return false;
        }
        let idx = kfs.partition_point(|&(s, _)| s <= f) - 1;
        let s = kfs[idx].0;
        let e = kfs.get(idx + 1).map_or(*n - 1, |k| (k.0 - 1).min(*n - 1));
        if e - s + 1 >= 2 {
            kfs[idx + 1..].iter_mut().for_each(|k| k.0 -= 1);
            *n -= 1;
            true
        } else if *n <= 1 {
            let changed = kfs[0].1 != 0;
            kfs[0].1 = 0;
            changed
        } else {
            kfs.remove(idx);
            kfs[idx..].iter_mut().for_each(|k| k.0 -= 1);
            *n -= 1;
            if kfs.first().is_none_or(|k| k.0 > 0) {
                kfs.insert(0, (0, 0));
            }
            true
        }
    }
    let marked = |tag: u64| {
        let inst = SymbolInstance {
            binding: crate::symbol_binding::SymbolInstanceBinding::new(tag, 1, kurbo::Affine::IDENTITY),
            layer_id: 1,
            parent_scope: Vec::new(),
            order: 0,
        };
        Keyframe::with_cel(
            Cel::new_with_instances(Vec::new(), vec![inst]),
            false,
            TweenRotate::Auto,
        )
    };
    let read = |l: &Layer| -> Vec<(usize, u64)> {
        l.keyframes_with_start()
            .map(|(s, kf)| {
                (
                    s,
                    kf.cel
                        .instances()
                        .first()
                        .map_or(0, |i| i.binding.instance_gid),
                )
            })
            .collect()
    };
    let mut r = map_overlay::Rng(11);
    for case in 0..3000 {
        let n = 1 + (r.f() * 300.0) as usize;
        let mut kfs = vec![(0usize, 1u64)];
        for f in 1..n {
            if r.f() < 0.3 {
                kfs.push((f, f as u64 + 1));
            }
        }
        let first = (r.f() * (n + 2) as f64) as usize;
        let count = 1 + (r.f() * (n + 3) as f64) as usize;
        let mut tl = crate::timeline::Timeline {
            layers: vec![Layer::new_with_id_and_frames(1, "L", n)],
            ..Default::default()
        };
        tl.layers[0].keyframes = KeyframeList::from_keyed(
            kfs.iter()
                .map(|&(s, tag)| (s, std::sync::Arc::new(marked(tag)))),
        );
        tl.sync_total_frames();
        let (mut want, mut want_n, mut want_changed) = (kfs.clone(), n, false);
        for _ in 0..count {
            want_changed |= reference_remove(&mut want, &mut want_n, first);
        }
        let changed = remove_frames_at(&mut tl, 0, first, count, 24);
        let layer = &tl.layers[0];
        assert_eq!(
            (read(layer), layer.frame_count, changed),
            (want, want_n, want_changed),
            "case {case}: n {n}, first {first}, count {count}"
        );
    }
}

#[test]
fn test_multilayer_f7_advances_frame_when_any_selected_layer_has_content() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);

    // Layer 1 (index 0) の Frame 0 に図形を描画し、Layer 2 (index 1) は Frame 0 が空白のまま
    app.tool.brush_color = red;
    let raw = brush_line(&[Point::new(100.0, 100.0), Point::new(160.0, 100.0)], 10.0);
    commit_brush_line(&mut app, &raw);
    doc_actions::timeline_add_layer(&mut app);
    assert!(app.doc.root.layers[1].keyframes[0].cel.is_empty());
    assert!(!app.doc.root.layers[0].keyframes[0].cel.is_empty());

    // 両レイヤーの Frame 0 を範囲選択して F7 (空白キーフレーム挿入) を実行
    // → Layer 1 に絵があるため Frame 0 の絵を消去せず Frame 1 に進んで空白キーフレームを挿入すること
    doc_actions::select_timeline_cells(&mut app, 0, 0, 1, 0);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
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
        doc_actions::timeline_extend_frame(&mut move_app);
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
    doc_actions::switch_to_layer_and_frame(&mut move_app, 0, 3);
    doc_actions::timeline_insert_keyframe(&mut move_app, false);
    doc_actions::switch_to_layer_and_frame(&mut move_app, 0, 4);
    doc_actions::timeline_insert_keyframe(&mut move_app, false);
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
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut move_app,
        sel,
        0,
        3,
        false
    ));
    let kfs_in_dst: Vec<_> = move_app.doc.root.layers[0]
        .keyframes_with_start()
        .filter(|(s, _)| (3..=5).contains(s))
        .collect();
    assert_eq!(
        kfs_in_dst.len(),
        1,
        "移動先範囲 [3..=5] 内の古いキーフレーム (frame 4) が残存せず、移動したキーフレーム (frame 3) のスパンが [3..=5] を保持すること"
    );
    assert_eq!(kfs_in_dst[0].0, 3);
    assert_eq!(
        move_app.doc.root.layers[0].cel_at(4).unwrap().shapes()[0].color(),
        red
    );

    // 別レイヤー（layer_offset = +1）へのフレーム区間移動で移動先レイヤーの layer_id に更新されること
    doc_actions::timeline_add_layer(&mut move_app);
    let dst_layer_id = move_app.doc.root.layers[1].id;
    let sel_cross = crate::timeline::TimelineCellSelection::from_corners(0, 3, 0, 5);
    assert!(doc_actions::timeline_move_or_copy_frames(
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
    assert_eq!(moved_cel.shapes().len(), 1);
    assert_eq!(moved_cel.shapes()[0].color(), red);
    assert_eq!(
        move_app
            .doc
            .root
            .evaluate_layer_shapes_at(1, 1, move_app.doc.library())[0]
            .layer_id,
        dst_layer_id,
        "別レイヤーへ移動したキーフレーム内のシェイプは、移動先レイヤーの図形として出ること"
    );
}

#[test]
fn test_step_frame_loop_mode_and_total_frames_over_600() {
    let mut app = Is2dApp::default();

    // 600フレーム超（1200フレーム）のタイムライン延長と step_frame のループ ON / OFF 挙動
    extend_frame_at(&mut app.doc.root, 0, 1199);
    app.doc.root.sync_total_frames();
    assert_eq!(app.doc.root.total_frames, 1200);
    assert_eq!(app.doc.root.layers[0].frame_count, 1200);

    // loop_playback == true のとき、先頭フレーム (0) から -1 コマ送りで最終フレーム (1199) へ折り返すこと
    app.timeline_ui.loop_playback = true;
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    timeline_ui::step_frame(&mut app, -1);
    assert_eq!(app.doc.root.current_frame, 1199);
    timeline_ui::step_frame(&mut app, 1);
    assert_eq!(app.doc.root.current_frame, 0);

    // loop_playback == false のとき、先頭フレーム (0) から -1 でも 0 で停止し、最終フレーム (1199) から +1 でも 1199 で停止すること
    app.timeline_ui.loop_playback = false;
    timeline_ui::step_frame(&mut app, -1);
    assert_eq!(app.doc.root.current_frame, 0);
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1199);
    timeline_ui::step_frame(&mut app, 1);
    assert_eq!(app.doc.root.current_frame, 1199);
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
    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, false));
    assert_eq!(app.doc.root.current_frame, 1);
    assert!(
        app.doc.root.layers[0].cel_at(1).unwrap().is_empty(),
        "F7 で自動進行した Frame 1 に空白キーフレームが挿入されること"
    );
    assert_eq!(
        app.doc.root.layers[0].cel_at(2).unwrap().shapes()[0].color(),
        blue,
        "Frame 1 にあった既存の青いキーフレームが消去されず Frame 2 に保持されていること"
    );
}

/// レビュー 41766f9 の N-2002：回転したシンボルを左右に反転するトゥイーンを、横幅が 0 になる真ん中のコマで分けても
/// （F6 など）、後ろのコマの動きが分ける前と同じ。真ん中のコマの行列は横の列が潰れているので、残った縦の列で向きを
/// 決める（前は回転を 0 と決め打ちし、本当の回転が斜めと縦の拡大に入って、後ろの動きが斜めに歪んだ）
#[test]
fn splitting_a_flip_tween_at_its_collapsed_frame_keeps_the_motion() {
    use crate::timeline::{interpolate_affine_with_pivot, TweenRotate};
    let pivot = Point::new(40.0, -30.0);
    for deg in [0.0f64, 30.0, 60.0, 90.0, 135.0, -70.0] {
        let rot = kurbo::Affine::rotate(deg.to_radians());
        let start = kurbo::Affine::translate((200.0, 150.0)) * rot;
        let end = kurbo::Affine::translate((320.0, 180.0)) * rot * kurbo::Affine::scale_non_uniform(-1.0, 1.0);
        let at = |a, b, t| interpolate_affine_with_pivot(a, pivot, b, pivot, TweenRotate::Auto, t).0;
        // 10 コマのトゥイーンを、コマ 5（横幅 0）で分ける。分けたあとのコマ 7 は、後ろのトゥイーン（コマ 5〜10）の t = 0.4
        let mid = at(start, end, 0.5);
        for (frame, t_whole) in [(6.0, 0.6), (7.0, 0.7), (9.0, 0.9)] {
            let whole = at(start, end, t_whole);
            let split = at(mid, end, (frame - 5.0) / 5.0);
            let diff = whole
                .as_coeffs()
                .iter()
                .zip(split.as_coeffs())
                .map(|(x, y)| (x - y).abs())
                .fold(0.0, f64::max);
            assert!(diff < 1e-9, "回転 {deg}°・コマ {frame}：分けたあとの動きが変わった（係数の差 {diff}）");
        }
    }
}

#[test]
fn test_flip_tween_midpoint_and_horizontal_flip_without_spurious_rotation() {
    use crate::timeline::{interpolate_affine_with_pivot, TweenRotate};

    // 1. 垂直反転 (scale_non_uniform(1.0, -1.0)) を含むトゥイーンの中間フレーム (t = 0.5) では、拡大率が 0 を通る（変換は
    //    潰れ、インスタンスは描かない：`instantiate_shapes`）。ピボットの位置は補間した所
    let a0 = kurbo::Affine::translate((100.0, 100.0));
    let a1 = kurbo::Affine::translate((300.0, 100.0)) * kurbo::Affine::scale_non_uniform(1.0, -1.0);
    let (mid, _) =
        interpolate_affine_with_pivot(a0, Point::ZERO, a1, Point::ZERO, TweenRotate::Auto, 0.5);
    assert!(
        !crate::vector_engine::invertible(mid),
        "反転トゥイーンの中間 (t=0.5) は潰れた変換（下限で太さを残さない）"
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

/// オニオンスキンの写しの無いコマを先に同時に作っても（`prepare_onion_cels`）、1つずつ作るのと同じ絵になる。シンボルの
/// トゥイーン（シンボルの中身のメモをスレッドの間で共有する）と、ふつうの絵のレイヤーで確かめる（レビュー f01d518 の N-5004）
#[test]
fn test_onion_prepare_in_parallel_matches_one_by_one() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let make = || {
        let mut app = Is2dApp::default();
        // レイヤー 1：シンボルのトゥイーン（コマ 1〜8、9 から止まる）
        let stroke = |y: f64, color| Action::PaintStroke {
            points: vec![Point::new(100.0, y), Point::new(260.0, y + 30.0)],
            radius: 10.0,
            color,
        };
        assert!(apply(&mut app, stroke(120.0, Color32::RED)));
        app.set_selected_indices(vec![0]);
        assert!(crate::canvas_tool::doc_actions::convert_selection_to_symbol(&mut app).is_some());
        assert!(crate::canvas_tool::doc_actions::timeline_toggle_tween(&mut app));
        while app.doc.root.total_frames < 12 {
            assert!(crate::canvas_tool::doc_actions::timeline_extend_frame(&mut app));
        }
        crate::canvas_tool::doc_actions::switch_to_layer_and_frame(&mut app, 0, 8);
        assert!(crate::canvas_tool::doc_actions::timeline_insert_keyframe(&mut app, true));
        app.set_selected_indices(vec![0]);
        crate::canvas_tool::doc_actions::nudge_selected_shapes(&mut app, KVec2::new(80.0, 40.0));
        assert!(canvas_tool::clear_selection(&mut app));
        // レイヤー 2：ふつうの絵（コマごとに少しずらしたキーフレーム）
        assert!(apply(&mut app, Action::AddLayer));
        let layer = app.doc.root.active_layer;
        crate::canvas_tool::doc_actions::switch_to_layer_and_frame(&mut app, layer, 0);
        assert!(apply(&mut app, stroke(220.0, Color32::BLUE)));
        for f in 1..12 {
            assert!(insert_keyframe_at(&mut app.doc.root, layer, f, true, 24));
            crate::canvas_tool::doc_actions::switch_to_layer_and_frame(&mut app, layer, f);
            let lid = app.doc.root.layers[layer].id;
            let idx: Vec<usize> = (0..app.scene().len())
                .filter(|&i| app.scene().get(i).unwrap().layer_id == lid)
                .collect();
            app.set_selected_indices(idx);
            crate::canvas_tool::doc_actions::nudge_selected_shapes(&mut app, KVec2::new(3.0, 0.0));
            assert!(canvas_tool::clear_selection(&mut app));
        }
        app
    };
    let cels: Vec<(usize, usize)> = (0..12).flat_map(|f| [(0, f), (1, f)]).collect();
    let tint = ([45, 115, 245], 84);
    let (one_by_one, prepared) = (make(), make());
    one_by_one.start_draw_pass();
    prepared.start_draw_pass();
    prepared.prepare_onion_cels(&cels);
    let entries = prepared.session.onion_cache.lock().entry_count();
    assert!(entries >= 20, "先に作った写し {entries} 個");
    for &(l, f) in &cels {
        let a = one_by_one.cached_onion_cel_screen_mesh(l, f, 1.0, Vec2::ZERO, WHOLE_VIEW, tint);
        let b = prepared.cached_onion_cel_screen_mesh(l, f, 1.0, Vec2::ZERO, WHOLE_VIEW, tint);
        let key = |m: Option<std::sync::Arc<eframe::egui::Mesh>>| {
            m.map(|m| {
                (
                    m.indices.clone(),
                    m.vertices
                        .iter()
                        .map(|v| (v.pos.x.to_bits(), v.pos.y.to_bits(), v.color))
                        .collect::<Vec<_>>(),
                )
            })
        };
        assert_eq!(key(a), key(b), "コマ {f}");
    }
    assert_eq!(
        prepared.session.onion_cache.lock().entry_count(),
        entries,
        "描くときは、先に作った写しを使う"
    );
}

/// オニオンスキンは、写し（`CelMeshCache`）の結合メッシュを、描くたびに複製せずに egui に渡す（egui 0.31 の
/// `Shape::Mesh` は `Arc<Mesh>` を持つ：レビュー f01d518 の N-5004）
#[test]
fn test_onion_skin_passes_cached_mesh_to_egui_without_copying() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use eframe::egui;
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(300.0, 160.0)],
            radius: 12.0,
            color: Color32::RED,
        }
    ));
    assert!(insert_keyframe_at(&mut app.doc.root, 0, 1, true, 24));
    app.doc.root.sync_total_frames();
    crate::canvas_tool::doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    app.view.onion_skin_enabled = true;
    app.view.onion_prev_frames = 1;
    app.view.onion_next_frames = 0;
    let ctx = egui::Context::default();
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1280.0, 820.0),
            )),
            ..Default::default()
        },
        |ctx| app.update_ui(ctx),
    );
    // コマ 1 のオニオンスキン（1つ前・色 [45, 115, 245]・濃さ 84）の写し
    let cached = app
        .cached_onion_cel_screen_mesh(0, 0, app.view.zoom(), app.view.pan, WHOLE_VIEW, ([45, 115, 245], 84))
        .expect("コマ 1 に絵がある");
    // 写し・描いた形・ここで受け取ったもの、の3つが同じメッシュを指す（描いた形が複製なら 2）
    assert_eq!(std::sync::Arc::strong_count(&cached), 3);
    drop(out);
    assert_eq!(std::sync::Arc::strong_count(&cached), 2);
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
    doc_actions::timeline_add_layer(&mut app);
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
    doc_actions::set_layer_visible(&mut app, 0, false);
    assert_eq!(
        scene_summary(&app).len,
        1,
        "非表示レイヤー (visible=false) の図形は Scene から除外されること"
    );
    assert_eq!(scene_summary(&app).shapes[0].color(), blue);

    // その状態で Layer 2 を編集しても、非表示の Layer 1 の図形が消去されないこと
    app.set_selected_indices(vec![0]);
    assert!(doc_actions::nudge_selected_shapes(
        &mut app,
        kurbo::Vec2::new(20.0, 0.0)
    ));
    assert_eq!(
        app.doc.root.layers[0].keyframes[0].cel.shapes().len(),
        1,
        "非表示レイヤーが除外された状態で他レイヤーを編集しても非表示レイヤーのセル内容は保護されること"
    );

    // 再び Layer 1 を表示に戻すと赤い図形が復活すること
    doc_actions::set_layer_visible(&mut app, 0, true);
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
    extend_frame_at(&mut app.doc.root, 0, 2);
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    doc_actions::timeline_insert_keyframe(&mut app, false);
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    doc_actions::timeline_insert_keyframe(&mut app, false);
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
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

    // t = 1.35s (最終フレーム Frame 2 を鳴らし終えて（0.3 秒）自動停止)
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
        "loop_playback == false のとき最終フレームを鳴らし終えると is_playing が false になり自動停止すること"
    );
}

#[test]
fn test_copy_frames_within_layer_shares_cels_and_keeps_ids() {
    use std::sync::Arc;

    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);

    extend_frame_at(&mut app.doc.root, 0, 5);
    app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(Point::new(60.0, 60.0), Point::new(100.0, 60.0), 10.0, red),
    );
    let sel = crate::timeline::TimelineCellSelection {
        min_layer: 0,
        max_layer: 0,
        min_frame: 0,
        max_frame: 1,
    };
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut app, sel, 0, 3, true
    ));

    let cel0 = app.doc.root.layers[0].cel_at(0).unwrap();
    let cel3 = app.doc.root.layers[0].cel_at(3).unwrap();
    assert!(Arc::ptr_eq(cel0.maps(), cel3.maps()));
    assert_eq!(cel0.shapes()[0].shape_id, cel3.shapes()[0].shape_id);

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 3);
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::PaintStroke {
            points: vec![Point::new(60.0, 160.0), Point::new(100.0, 160.0)],
            radius: 6.0,
            color: blue,
        },
    ));
    assert_eq!(app.doc.root.layers[0].cel_at(0).unwrap().shapes().len(), 1);
    assert_eq!(app.doc.root.layers[0].cel_at(3).unwrap().shapes().len(), 2);
}

#[test]
fn test_copy_frames_onto_a_keyframe_replaces_it_keeps_tween_pose_and_remaps_ids() {
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 50, 50);
    let blue = Color32::from_rgb(50, 120, 220);

    // 1. 6フレームのレイヤー (Frame 0..=1: 赤, Frame 2..=5: 青) で Frame 0..=1 を Frame 2..=3 へ複製すると、
    // Frame 2 の青のキーフレームは赤に置き換わり、その後ろの Frame 4..=5 にも赤が続く（Adobe Animate と同じく、
    // 移動先の後ろに写しのキーフレームは作らない）
    extend_frame_at(&mut app.doc.root, 0, 5);
    app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(Point::new(60.0, 60.0), Point::new(100.0, 60.0), 10.0, red),
    );
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    doc_actions::timeline_insert_keyframe(&mut app, false);
    put_cel_shapes(
        &mut app,
        0,
        2,
        make_stroke(Point::new(160.0, 60.0), Point::new(200.0, 60.0), 10.0, blue),
    );

    let sel_0_1 = timeline::TimelineCellSelection::from_corners(0, 0, 0, 1);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut app, sel_0_1, 0, 2, true
    ));
    assert_eq!(
        app.doc.root.layers[0].cel_at(2).unwrap().shapes()[0].color(),
        red
    );
    assert_eq!(
        app.doc.root.layers[0].cel_at(3).unwrap().shapes()[0].color(),
        red
    );
    assert_eq!(
        app.doc.root.layers[0]
            .keyframes_with_start()
            .map(|(s, _)| s)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    for f in 4..=5 {
        assert_eq!(
            app.doc.root.layers[0].cel_at(f).unwrap().shapes()[0].color(),
            red,
            "移動先の後ろの Frame {f} にも、置いたキーフレームの絵が続くこと"
        );
    }

    // 2. トゥイーン中間フレームから開始する範囲を移動/複製した際、開始フレームの中割り補間姿勢を引き継ぐこと (#9-2)
    let mut tween_app = Is2dApp::default();
    extend_frame_at(&mut tween_app.doc.root, 0, 4);
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
    tween_app.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut tween_app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut tween_app));
    doc_actions::switch_to_layer_and_frame(&mut tween_app, 0, 4);
    doc_actions::timeline_insert_keyframe(&mut tween_app, true);
    assert!(doc_actions::nudge_selected_shapes(
        &mut tween_app,
        kurbo::Vec2::new(200.0, 0.0)
    ));

    // 中間フレーム Frame 2 (t = 0.5 -> dx = +100.0) のみを選択して別レイヤーへ複製
    let mid_shapes = tween_app
        .doc
        .root
        .evaluate_layer_shapes_at(0, 2, tween_app.doc.library());
    let mid_center_x = mid_shapes[0].bounding_box().center().x;
    doc_actions::timeline_add_layer(&mut tween_app);
    let sel_mid = timeline::TimelineCellSelection::single(0, 2);
    assert!(doc_actions::timeline_move_or_copy_frames(
        &mut tween_app,
        sel_mid,
        1,
        0,
        true
    ));
    let copied_shapes = tween_app
        .doc
        .root
        .evaluate_layer_shapes_at(1, 2, tween_app.doc.library());
    assert!(
        (copied_shapes[0].bounding_box().center().x - mid_center_x).abs() < 1e-3,
        "トゥイーン途中フレームから複製したセルが中割り補間姿勢を引き継いでいること (#9-2)"
    );

    // 3. 別レイヤーへの複製で instance_gid および group_path ID が重複せず一意に再採番されること (#9-3)
    let orig_gid = tween_app.doc.root.layers[0].keyframes[0].cel.instances()[0]
        .binding
        .instance_gid;
    let copied_gid = tween_app.doc.root.layers[1].cel_at(2).unwrap().instances()[0]
        .binding
        .instance_gid;
    assert_ne!(
        orig_gid, copied_gid,
        "別レイヤーへ複製されたシンボルインスタンスの instance_gid が再採番され重複しないこと (#9-3)"
    );
    assert!(
        tween_app.doc.next_group_id.next() > copied_gid,
        "再採番後に SymbolManager::next_group_id が更新され以降の採番と衝突しないこと (#9-3)"
    );
}

/// キーフレームをドラッグで動かすと、キーフレームそのものが動く（Adobe Animate と同じ）。空いたコマには前のキーフレーム
/// の絵が続き（コマ 1 が空いたときだけ空白キーフレーム）、動かしたキーフレームは次のキーフレームの手前まで続く。空白や
/// 写しのキーフレームはできない
#[test]
fn moving_a_keyframe_moves_it_without_blank_or_copied_keyframes() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let red = Color32::from_rgb(220, 50, 50);
    let green = Color32::from_rgb(40, 180, 60);
    let paint = |app: &mut Is2dApp, c: Color32| {
        assert!(apply(
            app,
            Action::PaintStroke {
                points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
                radius: 10.0,
                color: c,
            },
        ));
    };
    let go = |app: &mut Is2dApp, f: usize| {
        assert!(canvas_tool::clear_selection(app));
        doc_actions::select_timeline_cells(app, 0, f, 0, f);
    };
    let starts = |app: &Is2dApp| -> Vec<usize> {
        app.doc.root.layers[0]
            .keyframes_with_start()
            .map(|(s, _)| s)
            .collect()
    };
    let color_at = |app: &Is2dApp, f: usize| {
        let cel = app.doc.root.layers[0].cel_at(f).unwrap();
        cel.shapes().first().map(|s| s.color())
    };
    // コマ 1 に赤、コマ 5 に緑のキーフレーム（10 コマ）
    let two_keys = || {
        let mut app = Is2dApp::default();
        paint(&mut app, red);
        go(&mut app, 4);
        assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: false }));
        paint(&mut app, green);
        go(&mut app, 9);
        assert!(apply(&mut app, Action::ExtendFrame));
        app
    };
    let move_frame = |app: &mut Is2dApp, from: usize, to: usize| {
        go(app, from);
        let sel = app.doc.root.cell_selection.unwrap();
        assert!(apply(
            app,
            Action::MoveOrCopyFrames {
                sel,
                layer_delta: 0,
                frame_delta: to as isize - from as isize,
                copy_mode: false,
            },
        ));
    };

    // 緑のキーフレームを後ろ（コマ 5 → 8）へ：赤がコマ 7 まで続き、緑はコマ 8〜10
    let mut app = two_keys();
    move_frame(&mut app, 4, 7);
    assert_eq!(starts(&app), vec![0, 7]);
    assert!((0..=6).all(|f| color_at(&app, f) == Some(red)));
    assert!((7..=9).all(|f| color_at(&app, f) == Some(green)));

    // 緑のキーフレームを前（コマ 5 → 3）へ：赤はコマ 2 まで、緑がコマ 3〜10
    let mut app = two_keys();
    move_frame(&mut app, 4, 2);
    assert_eq!(starts(&app), vec![0, 2]);
    assert!((0..=1).all(|f| color_at(&app, f) == Some(red)));
    assert!((2..=9).all(|f| color_at(&app, f) == Some(green)));

    // コマ 1 だけのキーフレーム（1〜10）をコマ 5 へ：コマ 1〜4 は空白キーフレームの続き、赤はコマ 5〜10
    let mut app = Is2dApp::default();
    paint(&mut app, red);
    go(&mut app, 9);
    assert!(apply(&mut app, Action::ExtendFrame));
    move_frame(&mut app, 0, 4);
    assert_eq!(starts(&app), vec![0, 4]);
    assert!((0..=3).all(|f| color_at(&app, f).is_none()));
    assert!((4..=9).all(|f| color_at(&app, f) == Some(red)));
    assert_eq!(app.doc.root.layers[0].frame_count, 10);
}

#[test]
fn test_partial_span_frame_move_moves_keyframes_and_leaves_hold_frames() {
    let red = Color32::from_rgb(220, 50, 50);
    let starts = |app: &Is2dApp| -> Vec<usize> {
        app.doc.root.layers[0]
            .keyframes_with_start()
            .map(|(s, _)| s)
            .collect()
    };

    // 1. 0〜5フレーム (6コマ) のキーフレームスパンのうち先頭 0〜2 を Frame 8〜10 へ通常移動 (Altなし)
    // → キーフレームが動くので、移動元はコマ 1 の空白キーフレームだけになり（3〜5 も空。Adobe Animate と同じく
    // 写しのキーフレームは作らない）、移動先の 8〜10 は赤
    let mut app = Is2dApp::default();
    extend_frame_at(&mut app.doc.root, 0, 5);
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
    assert_eq!(starts(&app), vec![0, 8]);
    for f in 0..=7 {
        assert!(
            app.doc.root.layers[0].cel_at(f).unwrap().is_empty(),
            "移動元の Frame {f} は空（コマ 1 の空白キーフレームの続き）"
        );
    }
    for f in 8..=10 {
        let cel = app.doc.root.layers[0].cel_at(f).unwrap();
        assert_eq!(cel.shapes().len(), 1);
        assert_eq!(cel.shapes()[0].color(), red);
    }

    // 2. 0〜5フレームのキーフレームスパンのうち保持フレームのみ (2〜3) を選んで Frame 8〜9 へ通常移動
    // → 動くキーフレームが無いので移動元はそのまま赤が続き、移動先のコマ 9 にその絵のキーフレームができる
    let mut hold_app = Is2dApp::default();
    extend_frame_at(&mut hold_app.doc.root, 0, 5);
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
    assert_eq!(starts(&hold_app), vec![0, 8]);
    for f in 0..=9 {
        assert_eq!(
            hold_app.doc.root.layers[0]
                .cel_at(f)
                .unwrap()
                .shapes()
                .len(),
            1,
            "Frame {f} は赤"
        );
    }
}

/// 四角 `pts`（4つの角。ステージの座標）の内側に収まる太い線を、今の塗りの色のブラシで1本引く（本番の
/// `Action::PaintStroke`：四角の短い辺の半分を太さにし、長い向きに引く。下準備に塗りを1つ置くため）
fn paint_rect(host: &mut impl doc_actions::ActionHost, pts: &[Point]) -> bool {
    let r = kurbo::Rect::from_points(pts[0], pts[2]);
    let radius = r.width().min(r.height()) / 2.0;
    let c = r.center();
    let (a, b) = if r.width() >= r.height() {
        (Point::new(r.x0 + radius, c.y), Point::new(r.x1 - radius, c.y))
    } else {
        (Point::new(c.x, r.y0 + radius), Point::new(c.x, r.y1 - radius))
    };
    let color = host.tool().brush_color;
    doc_actions::apply(host, doc_actions::Action::PaintStroke { points: vec![a, b], radius: radius as f32, color })
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
fn test_stage7_set_active_frame_and_per_layer_backdrop_cache() {
    use crate::canvas_tool::doc_actions::Action;

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
    let red_shape = app.scene().iter().next_back().unwrap().clone();
    app.session.replace_selection(crate::canvas_tool::Selection::default());
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&red_shape); app.session.replace_selection(sel); }
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
        sym_binding.instance_gid
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
    let sym_rev_before = app.doc.library().at(0).unwrap().revision();
    assert!(app.tabs[app.active].set_active_frame(0));
    assert!(app.tabs[app.active].set_active_frame(1));
    assert!(app.tabs[app.active].set_active_frame(2));
    app.tabs[app.active].set_active_layer(0);
    app.tabs[app.active].set_active_cell_selection(None);
    assert_eq!(
        app.doc.revision, doc_rev_before,
        "シンボル内で set_active_frame 等を呼んでも doc.revision が変わらないこと"
    );
    assert_eq!(
        app.doc.library().at(0).unwrap().revision(), sym_rev_before,
        "シンボル内で set_active_frame 等を呼んでもシンボルの revision が変わらないこと"
    );

    // 2. 親シーン背景キャッシュを初回構築したあと、シンボル内で再生フレーム送りしても背景メッシュが再構築されずそのまま返ること
    let same = |a: &[std::sync::Arc<eframe::egui::Mesh>],
                b: &[std::sync::Arc<eframe::egui::Mesh>]| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| std::sync::Arc::ptr_eq(x, y))
    };
    let vertices = |m: &[std::sync::Arc<eframe::egui::Mesh>]| -> usize {
        m.iter().map(|m| m.vertices.len()).sum()
    };
    let bd_initial = {
        // アプリと同じく、描く前に写しとシンボルのメモの古いものを捨ててから（`draw_canvas_contents`）
        app.tabs[app.active].start_draw_pass();
        app.tabs[app.active].cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW)
    };
    assert_eq!(
        bd_initial.len(),
        2,
        "親シーン背景（2000図形のレイヤーと、兄弟インスタンスのレイヤー）が存在すること"
    );
    let t_play = std::time::Instant::now();
    assert!(app.tabs[app.active].set_active_frame(1));
    app.tabs[app.active].refresh_scene_base();
    let bd_after_step = {
        // アプリと同じく、描く前に写しとシンボルのメモの古いものを捨ててから（`draw_canvas_contents`）
        app.tabs[app.active].start_draw_pass();
        app.tabs[app.active].cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW)
    };
    for s in app.scene().iter() {
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.0, Vec2::ZERO, false);
    }
    let play_step_elapsed = t_play.elapsed();
    assert!(
        same(&bd_initial, &bd_after_step),
        "シンボル内での再生フレーム送りでは親シーン背景メッシュが再計算されないこと"
    );

    // 3. シンボルの中で（親シーンが参照している先頭フレーム 0 に戻って）1本描いても、
    //    編集中のシンボルのインスタンスを含まない親レイヤー（2000図形）は作り直されず、
    //    兄弟インスタンスを含むレイヤーのみ更新されて背景メッシュに反映されること
    assert!(app.tabs[app.active].set_active_frame(0));
    app.tabs[app.active].refresh_scene_base();
    let initial_vert_count = vertices(&bd_initial);
    let t_draw = std::time::Instant::now();
    run(
        &mut app,
        &[Action::PaintStroke {
            points: vec![Point::new(100.0, 140.0), Point::new(180.0, 140.0)],
            radius: 12.0,
            color: Color32::YELLOW,
        }],
    );
    let bd_after_draw = {
        // アプリと同じく、描く前に写しとシンボルのメモの古いものを捨ててから（`draw_canvas_contents`）
        app.tabs[app.active].start_draw_pass();
        app.tabs[app.active].cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW)
    };
    for s in app.scene().iter() {
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.0, Vec2::ZERO, false);
    }
    let draw_and_render_elapsed = t_draw.elapsed();
    assert!(
        vertices(&bd_after_draw) > initial_vert_count,
        "兄弟インスタンスを含む親レイヤーの背景のみ更新され頂点数が増えること"
    );
    assert!(
        std::sync::Arc::ptr_eq(&bd_initial[0], &bd_after_draw[0]),
        "2000図形の親レイヤーは作り直さないこと"
    );
    // 2回目以降の同フレーム描画では、写しをそのまま再利用すること
    let bd_after_draw_2 = {
        // アプリと同じく、描く前に写しとシンボルのメモの古いものを捨ててから（`draw_canvas_contents`）
        app.tabs[app.active].start_draw_pass();
        app.tabs[app.active].cached_backdrop_screen_meshes(1.0, Vec2::ZERO, WHOLE_VIEW)
    };
    assert!(same(&bd_after_draw, &bd_after_draw_2));
    println!(
        "[symbol backdrop] play_step = {:.4} ms, draw_and_render = {:.4} ms",
        play_step_elapsed.as_secs_f64() * 1000.0,
        draw_and_render_elapsed.as_secs_f64() * 1000.0,
    );
}

#[test]
fn test_g4_1_shared_keyframes_fast_undo_and_rss_bound() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
                kfs.push((
                    f,
                    Arc::new(timeline::Keyframe::with_cel(
                        timeline::Cel::from_shapes_unwrap(shapes),
                        false,
                        timeline::TweenRotate::Auto,
                    )),
                ));
            }
            app.doc.root.layers.push(timeline::Layer {
                id: lid,
                name: format!("Layer {}", l + 1),
                visible: true,
                locked: false,
                frame_count: 3_000,
                keyframes: timeline::KeyframeList::from_keyed(kfs),
            });
        }
        app.doc.next_layer_id = crate::symbol::Counter::starting_at(11);
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
                kfs.push((
                    f,
                    Arc::new(timeline::Keyframe::with_cel(
                        timeline::Cel::from_shapes_unwrap(shapes),
                        false,
                        timeline::TweenRotate::Auto,
                    )),
                ));
            }
            app.doc.root.layers.push(timeline::Layer {
                id: lid,
                name: format!("Layer {}", l + 1),
                visible: true,
                locked: false,
                frame_count: 600,
                keyframes: timeline::KeyframeList::from_keyed(kfs),
            });
        }
        app.doc.next_layer_id = crate::symbol::Counter::starting_at(21);
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
fn test_g1_4_playback_advance_skips_base_cache_rebuild_and_stop_refreshes() {
    use eframe::egui;

    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let screen_rect = Some(egui::Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(1280.0, 820.0),
    ));

    app.doc.fps = 10; // 1コマ = 0.1秒
    extend_frame_at(&mut app.doc.root, 0, 2);
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 1);
    doc_actions::timeline_insert_keyframe(&mut app, false);
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    doc_actions::timeline_insert_keyframe(&mut app, false);
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
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    assert_eq!(app.session.base_cache().shapes()[0].color(), Color32::RED);
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
        app.session.base_cache().shapes()[0].color(),
        Color32::RED,
        "再生中のフレーム進行では SceneBaseCache の再構築が走らないこと"
    );

    // 手動停止すると Frame 1 の土台キャッシュが再構築されること
    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing);
    assert_eq!(
        app.session.base_cache().shapes()[0].color(),
        Color32::GREEN,
        "再生停止時点で現在フレーム (Frame 1) の土台キャッシュが揃うこと"
    );

    // 再び再生を開始し、最終フレーム (Frame 2) を鳴らし終えて自動停止した際にも stop_playback 経由で土台キャッシュが
    // 揃うこと（Frame 1 から始めて 0.2 秒で Frame 2 の終わり）
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
            time: Some(2.25),
            ..Default::default()
        },
        |ctx| {
            app.update_ui(ctx);
        },
    );
    assert!(!app.timeline_ui.is_playing);
    assert_eq!(app.doc.root.current_frame, 2);
    assert_eq!(
        app.session.base_cache().shapes()[0].color(),
        Color32::BLUE,
        "最終フレームでの自動停止でも stop_playback により Frame 2 の土台キャッシュが揃うこと"
    );
}

#[test]
fn test_insert_keyframe_on_held_frame_shares_shapes() {
    use std::sync::Arc;

    let mut app = Is2dApp::default();
    extend_frame_at(&mut app.doc.root, 0, 5);
    app.doc.root.sync_total_frames();
    put_cel_shapes(
        &mut app,
        0,
        0,
        make_stroke(
            Point::new(60.0, 60.0),
            Point::new(120.0, 60.0),
            8.0,
            Color32::RED,
        ),
    );
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 3);
    assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
    let kfs = &app.doc.root.layers[0].keyframes;
    assert_eq!(kfs.len(), 2);
    assert_eq!(app.doc.root.layers[0].start_of(1), 3);
    assert!(Arc::ptr_eq(kfs[0].cel.maps(), kfs[1].cel.maps()));
}

#[test]
fn test_other_1_insert_keyframe_via_snapshot_keyframe_at() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::timeline::TweenRotate;
    use crate::symbol_binding::SymbolLoopMode;

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
        write_symbol(&mut app.doc, sym_id, |tl| {
            extend_frame_at(tl, 0, 2);
            tl.sync_total_frames();
        });
    }

    // ルートにシンボルを配置し、Frame 0..4 のクラシックトゥイーンを作成
    extend_frame_at(&mut app.doc.root, 0, 7);
    app.doc.root.sync_total_frames();
    let inst0 = timeline::SymbolInstance {
        binding: symbol_binding::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((100.0, 100.0)),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
            color_effect: symbol_binding::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        order: 0,
    };
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_shapes_and_instances(Vec::new(), vec![inst0]);
    let _ = apply(&mut app, Action::ToggleTween);
    let _ = apply(
        &mut app,
        Action::SetTweenRotate {
            rotate: TweenRotate::Clockwise,
        },
    );

    doc_actions::switch_to_layer_and_frame(&mut app, 0, 4);
    doc_actions::timeline_insert_keyframe(&mut app, true);
    let inst4 = timeline::SymbolInstance {
        binding: symbol_binding::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((300.0, 100.0)),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 1,
            color_effect: symbol_binding::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        order: 0,
    };
    app.doc.root.layers[0]
        .keyframe_mut(1)
        .cel
        .set_shapes_and_instances(Vec::new(), vec![inst4]);

    // 1. トゥイーン途中 (Frame 2) で F6 キーフレーム挿入：中割り座標 (200, 100)・first_frame=2・tween=true・Clockwise が引き継がれること
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
    doc_actions::timeline_insert_keyframe(&mut app, true);
    let kf_mid = &app.doc.root.layers[0].keyframes[1];
    assert_eq!(app.doc.root.layers[0].start_of(1), 2);
    assert!(kf_mid.tween);
    assert_eq!(kf_mid.tween_rotate, TweenRotate::Clockwise);
    let b_mid = kf_mid.cel.instances()[0].binding;
    assert!((b_mid.xform.as_coeffs()[4] - 200.0).abs() < 1e-6);
    assert_eq!(b_mid.first_frame, 2);

    // 2. 保持フレーム上 (Frame 6、Frame 4 キーフレームからの経過2コマ) で F6 キーフレーム挿入：first_frame が +2 進んだ値になること
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 6);
    doc_actions::timeline_insert_keyframe(&mut app, true);
    let kf_hold = &app.doc.root.layers[0].keyframes[3];
    assert_eq!(app.doc.root.layers[0].start_of(3), 6);
    assert_eq!(kf_hold.cel.instances()[0].binding.first_frame, 3);

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
        .keyframe_index_starting_at(5)
        .is_some());
}

#[test]
fn test_g3_3_clear_timeline_cells_and_no_direct_ui_mutation_calls() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
fn test_g6_1_resolved_cel_cache_byte_limit_and_2min_playback_memory() {
    use crate::symbol_binding::SymbolLoopMode;

    // 1. 通常の純粋図形セルは Tier 2/3 キャッシュを再利用し、トゥイーン・シンボルセルはキャッシュを溜めずその場で構築すること
    let mut app = Is2dApp::default();
    let raw_pts: Vec<Point> = (0..8)
        .map(|p| Point::new((p as f64) * 15.0 + 20.0, 40.0))
        .collect();
    let raw_outline = brush_line_at_zoom(&raw_pts, 4.0, 1.0);
    let mut raw_shapes =
        brush_line_shapes_at_zoom(&raw_outline, Color32::from_rgb(40, 160, 90), 1.0);
    for s in &mut raw_shapes {
        s.layer_id = 1;
    }
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_shapes_and_instances(raw_shapes, Vec::new());
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
            crate::symbol::SymbolEvalCtx::new(app.doc.library()),
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
            crate::symbol::SymbolEvalCtx::new(app.doc.library()),
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
        let raw = brush_line_at_zoom(&pts, 5.0, 1.0);
        let mut shapes =
            brush_line_shapes_at_zoom(&raw, Color32::from_rgb((s_idx * 4) as u8, 120, 200), 1.0);
        for s in &mut shapes {
            s.layer_id = 1;
            let _ = s.fill_mesh();
        }
        sym_shapes.extend(shapes);
    }
    let sym_id = app.doc.register_new_symbol(sym_shapes);

    extend_frame_at(&mut app.doc.root, 0, 2_879);
    app.doc.root.sync_total_frames();
    let inst_start = timeline::SymbolInstance {
        binding: symbol_binding::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((50.0, 50.0)),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
            color_effect: symbol_binding::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        order: 0,
    };
    let inst_end = timeline::SymbolInstance {
        binding: symbol_binding::SymbolInstanceBinding {
            instance_gid: 1,
            symbol_id: sym_id,
            xform: kurbo::Affine::translate((650.0, 350.0)) * kurbo::Affine::rotate(1.2),
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
            color_effect: symbol_binding::ColorEffect::default(),
        },
        layer_id: 1,
        parent_scope: Vec::new(),
        order: 0,
    };
    app.doc.root.layers[0]
        .keyframe_mut(0)
        .cel
        .set_shapes_and_instances(Vec::new(), vec![inst_start]);
    app.doc.root.layers[0].keyframe_mut(0).tween = true;
    insert_keyframe_at(&mut app.doc.root, 0, 2_879, true, 24);
    let last_kf_idx = app.doc.root.layers[0].keyframes.len() - 1;
    app.doc.root.layers[0]
        .keyframe_mut(last_kf_idx)
        .cel
        .set_shapes_and_instances(Vec::new(), vec![inst_end]);

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
            crate::symbol::SymbolEvalCtx::new(app.doc.library()),
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

/// M2-2: トゥイーンの途中のコマでは直接編集できず警告が表示され、F6 挿入後に編集・開始フレーム指定が正しく動くこと、
/// およびキーフレームで選択・移動した状態で途中コマへ移動すると移る前のコマで確定して選択が解除されることを検証する
#[test]
fn test_m2_2_tween_midframe_blocks_edit_shows_warning_and_f6_enables_edit() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::symbol_binding::SymbolLoopMode;

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
    let sym_id = app.doc.library().at(0).unwrap().id;

    // シンボル内を 8 コマのループアニメにする
    let _ = apply(&mut app, Action::EnterSymbolFromLibrary { sym_id });
    for _ in 0..7 {
        let _ = apply(&mut app, Action::ExtendFrame);
    }
    let _ = apply(&mut app, Action::ExitGroupSteps { steps: 1 });
    assert_eq!(app.doc.library().at(0).unwrap().timeline.total_frames, 8);

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
    let init_kf0_ty = app.doc.root.layers[0].keyframes[0].cel.instances()[0]
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
    assert!(!app.session.selection().is_empty());
    let _ = apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 2,
        },
    );
    assert!(
        app.session.selection().is_empty(),
        "トゥイーン途中のコマへ移るとそのレイヤーの選択が外れること"
    );
    assert!(
        (app.doc.root.layers[0].keyframes[0].cel.instances()[0]
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
            "トゥイーンの途中のコマは編集できません（F6 でキーフレームを挿入してください）".to_owned(),
        )),
        "トゥイーン途中のコマでキャンバスの警告メッセージが返ること"
    );

    // 2. トゥイーンの途中のコマでブラシ・消しゴム・バケツ・選択と移動・自由変形・再生設定・ペースト・配置をしても文書が変わらないこと
    let undo_len_before = app.history.undo_stack.len();
    let kf_count_before = app.doc.root.layers[0].keyframes.len();
    let kf0_rev_before = app.doc.root.layers[0].keyframes[0].cel.revision();
    let kf1_rev_before = app.doc.root.layers[0].keyframes[1].cel.revision();

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
    assert!(app.session.selection().is_empty());
    let _ = apply(&mut app, Action::SelectAll);
    assert!(app.session.selection().is_empty());
    app.set_selected_indices(vec![0]);
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
        app.doc.root.layers[0].keyframes[0].cel.revision(),
        kf0_rev_before
    );
    assert_eq!(
        app.doc.root.layers[0].keyframes[1].cel.revision(),
        kf1_rev_before
    );

    // 3. トゥイーン途中のコマ (Frame 2) で F6 を押すと、補間された姿勢・続きの開始フレーム (1 + 2 = 3) のキーフレームができ、
    //    その後開始フレームに 3 と入力すると経過コマ数が二重加算されず 3 になること
    let kf0_tx = app.doc.root.layers[0].keyframes[0].cel.instances()[0]
        .binding
        .xform
        .as_coeffs()[4];
    let _ = apply(&mut app, Action::InsertKeyframe { copy_prev: true });
    assert_eq!(app.doc.root.layers[0].keyframes.len(), 3);
    let kf_mid = &app.doc.root.layers[0].keyframes[1];
    assert_eq!(app.doc.root.layers[0].start_of(1), 2);
    assert_eq!(
        kf_mid.cel.instances()[0].binding.first_frame, 3,
        "F6 挿入時は元の開始フレーム (1) + 経過コマ数 (2) = 3 になること"
    );
    assert!(
        (kf_mid.cel.instances()[0].binding.xform.as_coeffs()[4] - (kf0_tx + 100.0)).abs() < 1e-6,
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
        app.doc.root.layers[0].keyframes[1].cel.instances()[0]
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
        app.doc.root.layers[0].keyframes[1].cel.instances()[0]
            .binding
            .first_frame,
        3,
        "F6 後のキーフレームで開始フレームに 3 と入力するとそのまま 3 になること"
    );
}

#[test]
fn test_m2_6_onion_and_dim_backdrop_color_cache_matches_per_vertex() {
    use crate::screen_mesh::{dim_backdrop_color, onion_tint_color, transform_mesh_to_screen};
    use crate::vector_engine::append_mesh;
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
fn test_m2_8_document_shapes_never_store_fill_mesh_after_playback_onion_edit_and_undo() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
        .ensure_timeline_context(&mut app.tabs[app.active]);
    app.view.onion_skin_enabled = true;
    app.view.onion_prev_frames = 2;
    app.view.onion_next_frames = 2;
    // 再生のコマは鳴らした数（装置の無いテストでは画面の時刻）で進む：時刻 0 で始め、コマ f の半ばの時刻に描く
    timeline_ui::toggle_playback(&mut app);
    let fps = f64::from(app.doc.fps);
    for (f, time) in std::iter::once((0, 0.0)).chain((0..8).map(|f| (f, (f as f64 + 0.5) / fps))) {
        let full_output = ctx.run(
            egui::RawInput {
                time: Some(time),
                ..raw_input.clone()
            },
            |ctx| {
                app.update_ui(ctx);
            },
        );
        let _ = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        assert!(app.timeline_ui.is_playing);
        assert_eq!(app.doc.root.current_frame, f);
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
                    .any(|kf| kf.cel.shapes().iter().any(|s| s.has_cached_hit_bez_path()))
            })
        };
        tl_has_hit(&doc.root) || symbols_in_order(doc.library()).any(|sym| tl_has_hit(&sym.timeline))
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
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.0, Vec2::ZERO, false);
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
fn test_m3_4_delete_keyframe_only_on_keyframe_start_frame() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
fn test_m4_2_mid_tween_edit_restoring_classic_tween_rebuilds_base_cache_at_interpolated_position() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    app.tabs[app.active].session.drop_scene_base();
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
fn test_m5_1_n3002_f7_on_empty_frame_preserves_redo_and_clean_state() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
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
    assert!(app.tab().history.before.is_none());
    assert_eq!(app.tab().history.undo_stack.len(), 0);
    assert_eq!(app.tab().history.redo_stack.len(), 1);
    assert!(!app.tab().history.is_dirty());

    // Redo で元の線が戻ること
    assert!(app.tab_mut().redo());
    assert_eq!(app.tab().doc.root.active_cel_shapes().len(), 1);
}

#[test]
fn test_m5_1_f7_twice_on_drawn_frame_second_is_noop() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
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
    assert!(app.tab().history.before.is_none());
    assert_eq!(app.tab().history.undo_stack.len(), undo_after_first);
    assert!(!app.tab().history.is_dirty());
}

#[test]
fn test_toggle_playback_cancels_in_progress_brush_and_eraser_stroke() {
    use crate::canvas_tool::doc_actions::{apply, Action};

    let mut app = Is2dApp::default();

    // 1. ブラシで描いている途中（session.drag = PaintStroke）に toggle_playback で再生を始めると、drag が None になり図形が増えないこと
    app.tool.tool = Tool::Brush;
    app.session.drag = paint_drag(&[Point::new(50.0, 50.0), Point::new(150.0, 50.0)], 8.0);
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    assert!(matches!(app.session.drag, crate::canvas_tool::ActiveCanvasDrag::None));
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 0);
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
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 1);
    app.tool.tool = Tool::Eraser;
    app.session.drag = paint_drag(&[Point::new(40.0, 40.0), Point::new(160.0, 40.0)], 8.0);
    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    assert!(matches!(app.session.drag, crate::canvas_tool::ActiveCanvasDrag::None));
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 1);
}

#[test]
fn test_timeline_move_or_copy_frames_clips_range_per_source_layer_frame_count() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
            let a_has = layer_a.cel_at(f).is_some_and(|c| !c.shapes().is_empty());
            let b_has = layer_b.cel_at(f).is_some_and(|c| !c.shapes().is_empty());
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
        assert_eq!(layer_b.start_of(0), 0);
        assert_eq!(layer_b.start_of(1), 10);
        assert_eq!(layer_b.span_end_for_keyframe(1), 14);
        for f in 10..=14 {
            assert!(layer_b.cel_at(f).is_some_and(|c| !c.shapes().is_empty()));
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
            let b_has = layer_b.cel_at(f).is_some_and(|c| !c.shapes().is_empty());
            assert_eq!(b_has, (0..=4).contains(&f));
        }
    }
}

/// コマを別のレイヤーへ写しても、線の番号は写した元と重ならない（番号を付け替える `StoredMap::relabeled` が、線の番号も新しく
/// する）。前は線の番号を付け替えずに写したので、片方のレイヤーで線を選んで消すと、もう片方のレイヤーの同じ番号の線も
/// 消えた（レビュー de92ceb の N-1001。作品が消える）
#[test]
fn copying_frames_to_another_layer_gives_lines_new_ids() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::timeline::TimelineCellSelection;
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 4 };
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let wave: Vec<Point> = (0..=40).map(|i| Point::new(100.0 + 6.0 * i as f64, 200.0 + 15.0 * (i as f64 * 0.3).sin())).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: wave, stroke: ink }));
    assert!(apply(&mut app, Action::AddLayer));
    let has_lines = |app: &Is2dApp, i: usize| app.doc.root.layers[i].keyframes.first().is_some_and(|k| !k.cel.maps().is_empty());
    let src = (0..app.doc.root.layers.len()).find(|&i| has_lines(&app, i)).expect("線のあるレイヤー");
    let dst = (0..app.doc.root.layers.len()).find(|&i| i != src).expect("もう1つのレイヤー");
    assert!(apply(&mut app, Action::MoveOrCopyFrames {
        sel: TimelineCellSelection::single(src, 0),
        layer_delta: dst as isize - src as isize,
        frame_delta: 0,
        copy_mode: true,
    }));
    apply(&mut app, Action::SetTool { tool: Tool::Select });
    let lines: Vec<(u64, u64)> = app.scene().iter().filter(|s| s.line.is_some()).map(|s| (s.layer_id, s.shape_id)).collect();
    let n = lines.len();
    assert!(n >= 2, "2つのレイヤーに線がある");
    let mut ids: Vec<u64> = lines.iter().map(|l| l.1).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "写したレイヤーの線が、元の線と同じ番号");
    // 片方のレイヤーの線を1本選んで消しても、もう片方のレイヤーの線は消えない
    // 写したコマの中身は選ばれた状態なので、選び直してから1本だけ選ぶ
    let (layer, id) = lines[0];
    let other_before = lines.iter().filter(|l| l.0 != layer).count();
    assert!(apply(&mut app, Action::Deselect));
    app.session.add_to_selection([id], std::iter::empty());
    assert!(apply(&mut app, Action::DeleteSelection));
    let other_after = app.scene().iter().filter(|s| s.line.is_some() && s.layer_id != layer).count();
    assert_eq!(other_after, other_before, "もう片方のレイヤーの線まで消えた");
}

/// レビュー d91cd4c の N-5002：オニオンスキンで描いたコマの、地図の計算に使う写し（面と線の図形の表を持つ）は、オニオン
/// スキンの写しが持っているので、そのコマを今のコマとして描くときに作り直さない（前は、オニオンスキンの写しを作ると計算に
/// 使う写しを捨てたので、同じコマの面と線の図形を2回ずつ作り、線の多い作品のスクラブが重かった）
#[test]
fn onion_skin_keeps_the_cel_copies_it_drew() {
    let mut app = Is2dApp::default();
    for f in [0usize, 2] {
        doc_actions::switch_to_layer_and_frame(&mut app, 0, f);
        assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
        put_cel_shapes(&mut app, 0, f, vec![map_overlay::rect_shape(10.0 + f as f64, 10.0, 90.0, 90.0, Color32::RED)]);
    }
    doc_actions::switch_to_layer_and_frame(&mut app, 0, 0);
    app.tab_mut().view.onion_skin_enabled = true;
    app.tab_mut().view.onion_next_frames = 2;
    let ctx = egui::Context::default();
    let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
    let _ = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
    // コマ 2 のセル画はオニオンスキンに描いた：その計算に使う写しを、オニオンスキンの写しも持っている
    let tl = app.tab().active_timeline();
    let kf = tl.layers[0].keyframe_index_at(2).unwrap();
    let base = tl.layers[0].keyframes[kf].cel.maps()[0].base();
    assert!(std::sync::Arc::strong_count(&base) >= 2, "オニオンスキンの写しが、計算に使う写しを持っていない");
}

#[test]
fn test_onion_skin_is_not_drawn_during_playback() {
    let mut app = Is2dApp::default();
    for (f, y) in [(0usize, 100.0), (1usize, 180.0), (2usize, 260.0)] {
        if f > 0 {
            apply(
                &mut app,
                Action::SwitchLayerAndFrame {
                    layer_idx: 0,
                    frame: f,
                },
            );
            apply(&mut app, Action::InsertKeyframe { copy_prev: false });
        }
        apply(
            &mut app,
            Action::PaintStroke {
                points: vec![Point::new(100.0, y), Point::new(260.0, y)],
                radius: 6.0,
                color: Color32::from_rgb(220, 60, 60),
            },
        );
    }
    apply(
        &mut app,
        Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 1,
        },
    );
    app.tab_mut().view.onion_skin_enabled = true;

    let ctx = egui::Context::default();
    let raw_input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        ..Default::default()
    };

    let _ = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
    assert!(app.tab().session.onion_cache.lock().entry_count() > 0);

    timeline_ui::toggle_playback(&mut app);
    assert!(app.timeline_ui.is_playing);
    let _ = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
    assert_eq!(app.tab().session.onion_cache.lock().entry_count(), 0);

    timeline_ui::toggle_playback(&mut app);
    assert!(!app.timeline_ui.is_playing);
    let _ = ctx.run(raw_input, |ctx| app.update_ui(ctx));
    assert!(app.tab().session.onion_cache.lock().entry_count() > 0);
}

/// タイムラインの操作で変えてよいレイヤーは、選んでいるときも選んでいないときも同じ決まり（ロックしていない）。
/// 非表示のレイヤーも変える（同じ目的の判定を1つにする：洗い出しの 22）
#[test]
fn frame_ops_take_the_same_layers_with_or_without_a_selection() {
    use crate::timeline::{Layer, Timeline, TimelineCellSelection};
    let mut tl = Timeline::default();
    tl.layers.push(Layer::new_with_id(2, "hidden"));
    tl.layers.push(Layer::new_with_id(3, "locked"));
    tl.layers[1].visible = false;
    tl.layers[2].locked = true;
    tl.cell_selection = Some(TimelineCellSelection::from_corners(0, 0, 2, 0));
    assert_eq!(tl.target_layers_for_frame_op(), vec![0, 1]);
    tl.cell_selection = None;
    assert_eq!(tl.target_layers_for_frame_op(), vec![0, 1]);
}

/// F6 で写した（地図を共有する）キーフレームを別のレイヤーへ複製・移動しても、運んだ先の写しどうしは地図と塊を共有した
/// まま：メモリの増え方は地図1枚分、ファイルの増え方は地図1枚分と写しごとの塊の番号の並びで、写しの数の地図にならない。
/// Undo で元に戻る（レビュー e90f27c の N-2002）
#[test]
fn moving_shared_keyframes_to_another_layer_keeps_their_maps_shared() {
    const N: usize = 20;
    let file = |app: &Is2dApp| {
        saved_file_bytes(&app.doc).len()
    };
    let mut app = Is2dApp::default();
    let empty = file(&app);
    for k in 0..6 {
        let y = -180.0 + k as f64 * 70.0;
        let pts: Vec<Point> = (0..=40)
            .map(|j| Point::new(-380.0 + j as f64 * 19.0, y + 20.0 * (j as f64 * 0.5).sin()))
            .collect();
        commit_brush_line(&mut app, &brush_line(&pts, 6.0));
    }
    let one_map = file(&app) - empty;
    for f in 1..N {
        doc_actions::scrub_ruler_to_frame(&mut app, f);
        assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
    }
    doc_actions::timeline_add_layer(&mut app);
    // レイヤー `l` の地図の大きさ（塊の並びと辺のバイト。共有しているものは1回だけ数える）
    let map_bytes = |app: &Is2dApp, l: usize| {
        let mut seen = std::collections::HashSet::new();
        app.doc.root.layers[l]
            .keyframes
            .iter()
            .flat_map(|kf| kf.cel.maps().iter())
            .map(|m| {
                let (list, edges) = m.stored().bytes_unseen(&mut seen);
                list + edges
            })
            .sum::<usize>()
    };
    let shared = map_bytes(&app, 0);
    let before = file(&app);
    for copy_mode in [true, false] {
        let sel = timeline::TimelineCellSelection::from_corners(0, 0, 0, N - 1);
        assert!(doc_actions::timeline_move_or_copy_frames(
            &mut app, sel, 1, 0, copy_mode
        ));
        assert_eq!(app.doc.root.layers[1].keyframes.len(), N);
        assert_eq!(map_bytes(&app, 1), shared, "運んだ先の写しが地図を共有する");
        assert!(
            file(&app) - before < 2 * one_map,
            "ファイルの増え方が地図1枚分（写しの数 {N} 枚分でない）"
        );
        app.undo();
        assert_eq!(file(&app), before);
        assert_eq!(map_bytes(&app, 0), shared);
        assert!(app.doc.root.layers[1]
            .keyframes
            .iter()
            .all(|kf| kf.cel.is_empty()));
    }
}

/// レビュー f478c69 の N-5003：タイムラインのドラッグの途中で、ほかの所（キー・Undo）で文書が変わったら、ドラッグを捨てる
/// （ドラッグが持つレイヤー・コマの番号は始めたときの文書のもので、変わった文書に当てると、別のレイヤー・キーフレームを書き
/// 換える）。文書が変わらない間は、ドラッグはそのまま
#[test]
fn timeline_drags_are_dropped_when_the_document_changes_meanwhile() {
    use crate::timeline::TimelineCellSelection;
    use crate::timeline_ui::TimelineGridDrag;
    let mut app = Is2dApp::default();
    doc_actions::timeline_add_layer(&mut app);
    let moving = TimelineGridDrag::MoveFrames {
        orig_sel: TimelineCellSelection::from_corners(0, 0, 0, 0),
        grab_layer: 0,
        grab_frame: 0,
        curr_layer: 1,
        curr_frame: 3,
        copy_mode: false,
        has_moved: true,
    };
    let generation = |app: &Is2dApp| app.tab().history.current_commit_id;
    // コマ・レイヤーのドラッグ中に F5、そのあと Undo
    for change in 0..2 {
        for layer_drag in [false, true] {
            if layer_drag {
                app.timeline_ui.dragging_layer = Some(0);
            } else {
                app.timeline_ui.grid_drag = moving;
            }
            let g = generation(&app);
            app.timeline_ui.remember_drag_generation(g);
            // 文書が変わらない画面では、ドラッグはそのまま
            app.timeline_ui.drop_drag_if_document_changed(g);
            assert!(app.timeline_ui.dragging_layer.is_some() || !matches!(app.timeline_ui.grid_drag, TimelineGridDrag::None));
            if change == 0 {
                assert!(doc_actions::timeline_extend_frame(&mut app));
            } else {
                assert!(app.tab_mut().undo());
            }
            let g = generation(&app);
            app.timeline_ui.drop_drag_if_document_changed(g);
            assert!(app.timeline_ui.dragging_layer.is_none(), "文書が変わったのにレイヤーのドラッグが残った");
            assert!(matches!(app.timeline_ui.grid_drag, TimelineGridDrag::None), "文書が変わったのにコマのドラッグが残った");
        }
    }
}

/// タイムラインを描くとき、見えている範囲のキーフレームだけを調べる（作品の長さによらない）。直す前は、描くたびに
/// 全部のキーフレームを調べた（レビュー f01d518 の N-5005）
#[test]
fn drawing_the_timeline_examines_only_visible_keyframes() {
    let mut app = Is2dApp::default();
    app.tab_mut().with_active_timeline_mut(|tl, _| {
        for k in 1..2000 {
            assert!(insert_keyframe_at(tl, 0, 2 * k, false, 24));
        }
        tl.sync_total_frames();
    });
    let before = crate::timeline_ui::keyframes_examined();
    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1280.0, 820.0),
            )),
            ..Default::default()
        },
        |ctx| app.update_ui(ctx),
    );
    let examined = crate::timeline_ui::keyframes_examined() - before;
    assert!(examined > 0);
    assert!(examined < 200, "見えている範囲だけ（{examined} 個）");
}

/// 開いた直後に、まだ見ていないキーフレームを別のレイヤーへ写しても、地図を確かめない（面の番号は確かめるときに新しく
/// 振るので、写した先と元で番号が重ならない）。直す前は、番号を振り直すために写すキーフレームの地図を全部確かめた
/// （レビュー f01d518 の N-2001：レイヤー1枚分で M1 約 6.5 秒）
#[test]
fn copying_unviewed_frames_to_another_layer_does_not_check_their_maps() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let path = save_many_keyframes("lazy_copy", 6);
    let mut app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut app, &path, None).unwrap();
    assert!(apply(&mut app, Action::AddLayer));
    let before = crate::planar_map::full_checks();
    let sel = timeline::TimelineCellSelection {
        min_layer: 0,
        max_layer: 0,
        min_frame: 0,
        max_frame: 10,
    };
    assert!(apply(
        &mut app,
        Action::MoveOrCopyFrames {
            sel,
            layer_delta: 1,
            frame_delta: 0,
            copy_mode: true,
        },
    ));
    assert!(app.scene().len() > 0);
    assert_eq!(
        crate::planar_map::full_checks() - before,
        0,
        "写しても確かめない"
    );
    let layers = &app.doc.root.layers;
    let unchecked = |l: usize| {
        layers[l]
            .keyframes
            .iter()
            .filter(|kf| kf.cel.maps().iter().all(|m| m.is_unchecked()))
            .count()
    };
    assert_eq!(
        unchecked(0),
        5,
        "元の見ていないキーフレームは確かめないまま"
    );
    assert_eq!(unchecked(1), 5, "写した先も確かめないまま");
    // 使うと確かめる。写した先と元は同じ絵で、面の番号は重ならない
    for (src, copy) in layers[0].keyframes.iter().zip(layers[1].keyframes.iter()) {
        let (a, b) = (src.cel.shapes(), copy.cel.shapes());
        assert_eq!(a.len(), b.len());
        assert!(!a.is_empty());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.bounding_box(), y.bounding_box(), "同じ絵");
        }
        let ids: std::collections::HashSet<_> = a.iter().map(|s| s.shape_id).collect();
        assert!(
            b.iter().all(|s| !ids.contains(&s.shape_id)),
            "番号が重ならない"
        );
    }
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// N-1002（レビュー 5b822a3）：再生・スクラブで一度通ったコマの判定の形が、文書（と Undo の控え）が持つ塊に残り続けない。
/// 判定の形を生かすのは計算に使う写し（`LayerBase`）だけなので、全部のコマを1回ずつ表示して最初のコマに戻ったあと、判定の
/// 形が生きている塊は、いま見ているコマのものだけ（前は、通った全部のコマの塊が判定の形を持ち続け、作品 A では開いた直後の
/// 約3倍のメモリが、タブを閉じるまで返らなかった）
#[test]
fn passing_frames_does_not_keep_their_edge_shapes() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut rng = map_overlay::Rng(5);
    let mut app = Is2dApp::default();
    let go = |app: &mut Is2dApp, f: usize| {
        assert!(canvas_tool::clear_selection(app));
        apply(app, Action::ScrubRulerToFrame { frame: f });
    };
    // 8 コマ、どのコマも別の絵のキーフレーム
    let frames = 8;
    for f in 0..frames {
        if f > 0 {
            go(&mut app, f);
            assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: false }));
        }
        let mut shapes = Vec::new();
        for k in 0..12 {
            let p0 = Point::new(rng.f() * 400.0, rng.f() * 300.0);
            let p1 = Point::new(rng.f() * 400.0, rng.f() * 300.0);
            shapes.extend(make_stroke(
                p0,
                p1,
                3.0 + rng.f() as f32 * 4.0,
                colors[k % 3],
            ));
        }
        put_cel_shapes(&mut app, 0, f, shapes);
    }
    // 判定の形が生きている塊の数（全部のキーフレーム）と、塊の数
    let live = |app: &Is2dApp| -> Vec<(usize, usize)> {
        app.doc.root.layers[0]
            .keyframes
            .iter()
            .map(|kf| {
                kf.cel.maps().iter().fold((0, 0), |(l, t), m| {
                    (
                        l + m.stored().tiles_with_live_polys(),
                        t + m.stored().tile_count(),
                    )
                })
            })
            .collect()
    };
    for f in 0..frames {
        go(&mut app, f);
        let _ = app.scene().len();
    }
    go(&mut app, 0);
    let _ = app.scene().len();
    let counts = live(&app);
    assert_eq!(counts.len(), frames);
    assert!(counts.iter().all(|&(_, t)| t > 0), "どのコマにも絵がある");
    // いま見ていないコマの塊は、判定の形を持たない
    let others: usize = counts[1..].iter().map(|&(l, _)| l).sum();
    assert_eq!(
        others, 0,
        "通ったコマの塊が判定の形を持ち続ける：{counts:?}"
    );
}

/// レイヤーの番号は作品の数え札から配り、取り消しで戻さない：レイヤーを足す → 取り消す → また足すと、前と違う番号（設計
/// 審査 b7208d9 の 7.。前はタイムラインの数え札で、取り消すと戻り、同じ番号をまた使った）
#[test]
fn a_layer_number_is_never_given_twice_even_after_undo() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    let ids = |app: &Is2dApp| -> Vec<u64> { app.doc.root.layers.iter().map(|l| l.id).collect() };
    assert!(apply(&mut app, Action::AddLayer));
    let first = ids(&app);
    assert!(apply(&mut app, Action::Undo));
    assert!(apply(&mut app, Action::AddLayer));
    let again = ids(&app);
    let new_first: Vec<u64> = first.iter().copied().filter(|i| *i != 1).collect();
    let new_again: Vec<u64> = again.iter().copied().filter(|i| *i != 1).collect();
    assert_ne!(new_first, new_again, "取り消したあとに足したレイヤーが、前と同じ番号をまた使った");
}

#[test]
fn test_g3_5_document_single_fps_inside_symbol_undo_redo_and_save() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    let sym_id = app.doc.library().at(0).unwrap().id;

    // シンボルの中に入る
    assert!(symbol_ops::enter_symbol_from_library(&mut app, sym_id));
    assert!(!app.session.edit_path().is_empty());

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
fn test_m3_3_tween_kept_on_and_incomplete_on_draw_paste_save_delete_and_toggle_off() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
    let sym_id = app.doc.library().at(0).unwrap().id;
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
    app.set_selected_indices(vec![raw_idx]);
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

/// レイヤーの名前を変える：続けて打った文字は1つの取り消しにまとまり、Undo で前の名前に戻る。空の名前も受け付け（シンボル・
/// 音の名前と同じ）、無い番号・同じ名前は変えない（Undo も積まない）。保存して開いても名前が残る
#[test]
fn renaming_a_layer_is_one_undo_step_and_is_saved() {
    use crate::canvas_tool::CoalesceKey;
    use doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    let layer_id = app.tab().active_timeline().layers[0].id;
    let before = app.tab().active_timeline().layers[0].name.clone();
    let name = |app: &Is2dApp| app.tab().active_timeline().layers[0].name.clone();
    let undo_len = |app: &Is2dApp| app.tab().history.undo_stack.len();
    let base = undo_len(&app);

    for typed in ["背", "背景", ""] {
        assert!(apply(&mut app, Action::RenameLayer { layer_id, new_name: typed.to_owned() }));
    }
    assert_eq!(name(&app), "");
    assert!(apply(&mut app, Action::RenameLayer { layer_id, new_name: "背景".to_owned() }));
    assert!(!apply(&mut app, Action::RenameLayer { layer_id, new_name: "背景".to_owned() }));
    assert!(!apply(&mut app, Action::RenameLayer { layer_id: u64::MAX, new_name: "x".to_owned() }));
    let _ = apply(&mut app, Action::EndCoalesce { key: Some(CoalesceKey::RenameLayer(layer_id)) });
    assert_eq!(undo_len(&app), base + 1, "続けて打った文字は1つの取り消し");
    assert_eq!(name(&app), "背景");

    let opened = opened_document(
        &saved_file_bytes(&app.doc),
    )
    .unwrap();
    assert_eq!(opened.root.layers[0].name, "背景", "保存して開いても名前が残る");

    assert!(apply(&mut app, Action::Undo));
    assert_eq!(name(&app), before, "Undo で前の名前に戻る");
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
    doc_actions::timeline_extend_frame(&mut hold_app);
    doc_actions::timeline_extend_frame(&mut hold_app);
    doc_actions::switch_to_layer_and_frame(&mut hold_app, 0, 2);
    assert!(
        !doc_actions::timeline_delete_keyframe(&mut hold_app),
        "先頭キーフレームの保持フレーム（Frame 2）では Shift+F6 が無効であり、Frame 0 の絵を消去しないこと"
    );
    assert!(!hold_app.doc.root.layers[0].cel_at(0).unwrap().is_empty());

    // 2. 尺の短いレイヤーが含まれる状態で後ろのフレームで Shift+F5 を押した際、範囲外の短いレイヤーの末尾を削らないこと
    doc_actions::timeline_add_layer(&mut hold_app);
    hold_app.doc.root.layers[1].frame_count = 1;
    hold_app.doc.root.cell_selection = None;
    hold_app.doc.root.current_frame = 2;
    assert!(doc_actions::timeline_remove_frame(&mut hold_app));
    assert_eq!(hold_app.doc.root.layers[0].frame_count, 2);
    assert_eq!(
        hold_app.doc.root.layers[1].frame_count, 1,
        "現在フレーム (2) に届いていない短いレイヤー (frame_count=1) は Shift+F5 の影響を受けないこと"
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
    sel_app.set_selected_indices(vec![1]);
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
    assert!(canvas_tool::clear_selection(&mut sel_app));
    assert_eq!(scene_summary(&sel_app).len, 2);
}

#[test]
fn test_m3_2_tween_midframe_cannot_be_selected_by_any_entry_or_deleted() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
        app.session.selection().is_empty(),
        "Shift+クリックでトゥイーン途中のシンボルが選択に入らないこと"
    );
    let _ = apply(
        &mut app,
        Action::ToggleSelectionAt {
            scene_idx: draw_scene_idx,
        },
    );
    assert!(
        !app.session.selection().is_empty(),
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
        app.session.selection().is_empty(),
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
    { let mut sel = app.session.selection().clone(); sel.insert_shape(&tween_shape); app.session.replace_selection(sel); }
    let deleted = apply(&mut app, Action::DeleteSelection);
    assert!(!deleted, "消せる図形が無ければ false が返ること");
    assert_eq!(
        app.history.undo_stack.len(),
        undo_before,
        "Undo が積まれないこと"
    );
    assert!(!app.history.is_dirty(), "未保存フラグ(*)が付かないこと");
}

/// 再生がコマを進める入口（`Tab::play_to_frame`：部屋の外から見ている場所を変える、ただ1つの入口）は、持ち上げた絵がある
/// ときは変えずに断る（持ち上げた絵を貼り戻すのは作品を書くことなので、部屋の中の入口だけがする。直す前は、再生の関数が
/// コマを直接書き、持ち上げた絵が見ていないコマに結び付きえた：設計 入口の部屋の設計_27c4085 の R6）
#[test]
fn playback_frame_entry_refuses_while_pieces_are_lifted() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    assert!(apply(&mut app, Action::ExtendFrame));
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectMarquee { p0: Point::new(50.0, 50.0), p1: Point::new(200.0, 350.0), additive: false }));
    assert!(app.session.floating().is_some());
    assert!(!app.play_to_frame(1), "持ち上げた絵があるのにコマを進めた");
    assert_eq!(app.tab().active_timeline().current_frame, 0);
    assert!(apply(&mut app, Action::Deselect));
    assert!(app.play_to_frame(1));
    assert_eq!(app.tab().active_timeline().current_frame, 1);
}
