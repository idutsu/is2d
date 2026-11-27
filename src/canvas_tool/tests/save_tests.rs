//! 保存・書き出し・復旧（作品ファイル・復旧ファイル・前の版の変換・保存の形の指紋・計測用の作品・動画の書き出し）のテスト

use super::*;

/// 文書 `doc` の、まだ確かめていない地図をみな確かめ（使うときと同じ入口：`UncheckedMaps::check`）、ファイルから読んだもの
/// の知らせを返す
fn check_all_maps(doc: &symbol::Document) -> Vec<timeline::LoadNotice> {
    let _ = doc.check_maps_in_background().expect("スレッドを作れる").join();
    doc.load_notices.take()
}

#[test]
fn test_m3_1_write_project_reports_write_error() {
    let tab = crate::canvas_tool::Tab::new_empty(crate::canvas_tool::TabIds::default().fresh(), 1);
    let doc = tab.doc().clone();

    let tmp = std::env::temp_dir().join(format!("is2d_m3_1_ro_{}.tmp", std::process::id()));
    std::fs::write(&tmp, b"orig").unwrap();
    let mut ro_file = std::fs::File::open(&tmp).unwrap();
    let res = save_format::RecordWriter::default().write(&mut ro_file, &doc, None);
    let _ = std::fs::remove_file(&tmp);
    assert!(res.is_err(), "書き込みの失敗が Err として返ること");
    // 中身の途中で書けなくなっても（入りきらない書き先）、書き込みの失敗として返る
    let (app, _) = copied_symbol_with_sound();
    let mut small = [0u8; 1000];
    let err = save_format::RecordWriter::default()
        .write(&mut std::io::Cursor::new(&mut small[..]), &app.doc, None)
        .expect_err("入りきらない書き先は Err が返ること");
    assert_eq!(err.kind(), std::io::ErrorKind::WriteZero);

    #[cfg(target_os = "linux")]
    {
        let dev_full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let err = crate::project_io::write_project(dev_full, &doc)
            .expect_err("/dev/full への書き込みは Err が返ること");
        assert_eq!(err.kind(), std::io::ErrorKind::StorageFull);
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
    app.doc.stage_size = Vec2::new(800.0, 450.0);
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
    tween_app.doc.stage_size = Vec2::new(800.0, 450.0);
    let stage_left = center.x - 400.0;
    let start_cx = stage_left + 100.0;
    let end_cx = stage_left + 700.0;
    let box_shape = make_rect_shape(start_cx - 30.0, center.y - 30.0, 60.0, 60.0, red);
    put_cel_shapes(&mut tween_app, 0, 0, vec![box_shape]);
    tween_app.tab_mut().set_selected_indices(vec![0]);
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
    tween_app.tab_mut().set_selected_indices(vec![0]);
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn test_video_export_mp4_cancel_and_error() {
    use crate::canvas_tool::View;
    use crate::vector_engine::Contour;
    use crate::save_format::tmp_path_for;
    use crate::video_export::{VideoExportJob, VideoExportOutcome};

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
        let tmp_file = tmp_path_for(&out_path);
        let mut job = VideoExportJob::start(
            app.tab().committed_document_copy().unwrap(),
            out_path.clone(),
        ).expect("スレッドを作れる");
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
        let cancel_tmp = tmp_path_for(&cancel_path);
        let mut job = VideoExportJob::start(
            app.tab().committed_document_copy().unwrap(),
            cancel_path.clone(),
        ).expect("スレッドを作れる");
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
        let bad_tmp = tmp_path_for(&bad_path);
        let mut job = VideoExportJob::start(
            app.tab().committed_document_copy().unwrap(),
            bad_path.clone(),
        ).expect("スレッドを作れる");
        let outcome = job.wait();
        assert!(matches!(outcome, VideoExportOutcome::Failed(_)));
        assert!(!bad_path.exists());
        assert!(!bad_tmp.exists());
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

/// 足していくだけの列（`GrowingList`）は、ふつうの並びと同じ順・同じ中身で、番号で引ける（塊の境目をまたいでも）
#[test]
fn growing_list_matches_a_plain_vec() {
    for n in [0usize, 1, 2, 3, 4, 7, 8, 9, 100, 1023, 1024, 1025, 5000] {
        let mut g = crate::chunked_list::GrowingList::default();
        let mut v = Vec::new();
        for i in 0..n {
            g.push(i * 3);
            v.push(i * 3);
        }
        assert_eq!(g.len(), n);
        assert!(g.iter().copied().eq(v.iter().copied()));
        for (i, x) in v.iter_mut().enumerate() {
            *g.get_mut(i).unwrap() += 1;
            *x += 1;
        }
        assert!(g.get_mut(n).is_none());
        assert!(g.clone().into_iter().eq(v.into_iter()));
    }
}

/// 見本の作品（`claude/review-bench/sample.is2d`：作品 A の、正しさの決まりを少し変えると外れる絵：`make_sample_work`。
/// 作品の絵なのでリポジトリには入れない：持ち主が決めた 2026-11-27）の地図は、どれも今のコードの決まりで正しい（切り
/// 直しも何も無い地図も無い）。正しさの決まり（判定の形・許容）を変えて落ちたら、ファイルの版を上げて、テスト用の変換
/// （`convert_work_files_to_current_format`：地図も切り直す）で見本と作品を直す（審査 5616fcc の D-3 の (3)。前は決まりを
/// 変えても、前の作品の絵が次に開いたとき空になるまで気づけなかった）
#[test]
fn sample_work_maps_are_all_valid_with_the_current_code() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("claude/review-bench/sample.is2d");
    assert!(
        path.exists(),
        "見本の作品が無い：{}（作り方は `make_sample_work` の説明）",
        path.display()
    );
    let doc = read_saved_file(&path)
        .and_then(save_format::ProjectDocument::<crate::stored_map::ReadTiles>::into_document)
        .expect("見本の作品を開ける");
    let maps = std::iter::once(&doc.root)
        .chain(symbols_in_order(doc.library()).map(|s| &s.timeline))
        .flat_map(|tl| &tl.layers)
        .flat_map(|l| l.keyframes.iter())
        .map(|k| k.cel.maps().len())
        .sum::<usize>();
    assert!(maps >= 20, "見本の地図が少ない：{maps}");
    let notices = check_all_maps(&doc);
    assert!(
        notices.is_empty(),
        "見本の作品の地図が今の決まりに合わない（ファイルの版を上げ、テスト用の変換で切り直す）：{notices:?}"
    );
}

/// 作品 `IS2D_WORK` の地図をみな確かめ、切り直した・何も無い地図として見せた場所を出す（正しさの決まりを変えたとき、
/// 前の作品のどの絵が外れるかを見る）
///
/// 実行：`IS2D_WORK=作品のパス cargo test --release list_maps_failing_the_current_check -- --ignored --nocapture`
#[test]
#[ignore]
fn list_maps_failing_the_current_check() {
    let Ok(work) = std::env::var("IS2D_WORK") else {
        eprintln!("IS2D_WORK に作品のパスを入れて実行してください");
        return;
    };
    let doc = read_saved_file(std::path::Path::new(&work))
        .and_then(save_format::ProjectDocument::<crate::stored_map::ReadTiles>::into_document)
        .expect("作品を開ける");
    for n in check_all_maps(&doc) {
        eprintln!("{n:?}");
    }
}

#[test]
fn test_video_export_parallel_render_frames_and_without_mesh() {
    use crate::canvas_tool::View;
    use crate::canvas_tool::doc_actions::{apply, Action};
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
    app.tab_mut().set_selected_indices(vec![0]);
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
    app.tab_mut().set_selected_indices(vec![0]);
    assert!(apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(540.0, 0.0),
        },
    ));
    assert_eq!(app.tab().doc.root.total_frames, 10);

    let doc = app.tab().committed_document_copy().unwrap();
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
    for sym in symbols_in_order(doc.library()) {
        for layer in &sym.timeline.layers {
            for kf in &layer.keyframes {
                for s in kf.cel.shapes().iter() {
                    assert!(!s.has_cached_fill_mesh());
                }
            }
        }
    }
    let layer = &doc.root.layers[0];
    let without_mesh_shapes = layer.evaluate_shapes_at(
        2,
        layer.id,
        SymbolEvalCtx::new(doc.library()).without_mesh(),
    );
    assert!(!without_mesh_shapes.is_empty());
    assert!(
        without_mesh_shapes
            .iter()
            .all(|s| !s.has_cached_fill_mesh()),
        "書き出し用（without_mesh）の展開の図形はどれもメッシュを持たないこと"
    );

    let with_mesh_shapes =
        layer.evaluate_shapes_at(2, layer.id, SymbolEvalCtx::new(doc.library()));
    assert!(!with_mesh_shapes.is_empty());
    assert!(
        with_mesh_shapes.iter().all(|s| s.has_cached_fill_mesh()),
        "画面用（SymbolEvalCtx::new）の展開の図形はどれもメッシュを持つこと"
    );
}

/// 書き出す画像を作れない大きさなら、白いコマを黙って渡さず、書き出しの失敗として返す（画素の大きさと原点は
/// `stage_pixel_size` の1か所で決める）。直す前は、画像を作れないと白いコマ（大きさ 0 なら空のコマ）を返して書き出しを
/// 続けた（レビュー f01d518 の N-4007）
#[test]
fn video_export_fails_instead_of_pushing_blank_frames() {
    use crate::video_export::{render_frames_in_order, VideoExportOutcome};
    use std::sync::atomic::AtomicBool;
    let app = Is2dApp::default();
    let mut pushed = 0;
    let outcome = render_frames_in_order(&app.doc, (0, 0), 1, &AtomicBool::new(false), |_, _| {
        pushed += 1;
        Ok(())
    });
    assert!(matches!(outcome, VideoExportOutcome::Failed(_)));
    assert_eq!(pushed, 0, "コマを渡さない");
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
    app.set_selected_indices(vec![0]);
    let sym_id = doc_actions::convert_selection_to_symbol(&mut app).unwrap();
    assert!(doc_actions::timeline_toggle_tween(&mut app));
    assert!(doc_actions::set_active_keyframe_tween_rotate(
        &mut app,
        crate::timeline::TweenRotate::Clockwise
    ));
    doc_actions::select_timeline_cells(&mut app, 0, 5, 0, 5);
    doc_actions::timeline_insert_keyframe(&mut app, true);
    doc_actions::nudge_selected_shapes(&mut app, kurbo::Vec2::new(150.0, 40.0));

    // Layer 2: グループ化された図形 + 生シェイプ
    doc_actions::timeline_add_layer(&mut app);
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
    app.set_selected_indices(vec![last_idx]);
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
    assert_eq!(loaded_app.doc.library().len(), 1);
    assert_eq!(
        loaded_app.doc.find_symbol(sym_id).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .shapes()
            .len(),
        2,
        "インプレイス編集中に追加した図形もシンボル定義へ保存・復元されていること"
    );
    assert!(loaded_app.doc.root.layers[0].keyframes[0].tween);
    assert_eq!(
        loaded_app.doc.root.layers[0].keyframes[0].tween_rotate,
        crate::timeline::TweenRotate::Clockwise
    );
    assert!(
        loaded_app.doc.root.layers[1].keyframes[0]
            .cel
            .shapes()
            .iter()
            .any(|s| s.group_path == vec![gid]),
        "グループパスも完全一致で復元されること"
    );
    assert!(loaded_app.doc.next_group_id.next() > gid);
    assert!(loaded_app.doc.next_symbol_id.next() > sym_id);

    let _ = std::fs::remove_file(&tmp_path);
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
    merge_app.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut merge_app,
        doc_actions::Action::NudgeSelection {
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
    assert!(canvas_tool::clear_selection(&mut merge_app));
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
    union_save_app.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut union_save_app,
        doc_actions::Action::NudgeSelection {
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
    assert!(canvas_tool::clear_selection(&mut union_save_app));
    assert_eq!(union_save_app.scene().len(), 1);
    assert!(union_save_app.session.selection().is_empty());

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
    self_union_app.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut self_union_app,
        doc_actions::Action::NudgeSelection {
            delta: KVec2::new(0.0, -80.0),
        },
    );
    self_union_app.set_selected_indices(vec![0, 1]);
    assert!(doc_actions::copy_selected_shapes(&mut self_union_app));
    doc_actions::apply(
        &mut self_union_app,
        doc_actions::Action::NudgeSelection {
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
    self_union_app.set_selected_indices(vec![0]);
    assert!(doc_actions::copy_selected_shapes(&mut self_union_app));

    doc_actions::set_layer_locked(&mut self_union_app, 0, true);
    assert!(
        !doc_actions::paste_clipboard(&mut self_union_app, true),
        "ロックされたレイヤー上へのペーストが拒否されること"
    );
    doc_actions::set_layer_locked(&mut self_union_app, 0, false);

    assert!(canvas_tool::clear_selection(&mut self_union_app));
    select_ops::apply_marquee_selection(
        &mut self_union_app,
        Point::new(90.0, 60.0),
        Point::new(150.0, 140.0),
        false,
    );
    assert!(self_union_app
        .session
        .floating()
        .is_some_and(|fl| !fl.modified()));
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
            .shapes()
            .len(),
        1,
        "マーキースライス未編集状態で push_undo が呼ばれてもタイムライン側の分割前1シェイプが維持されること"
    );
    assert!(canvas_tool::clear_selection(&mut self_union_app));
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
    undo_redo_merge_app.set_selected_indices(vec![1]);
    doc_actions::apply(
        &mut undo_redo_merge_app,
        doc_actions::Action::NudgeSelection {
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
    assert!(canvas_tool::clear_selection(&mut undo_redo_merge_app));
    assert_eq!(
        undo_redo_merge_app.scene().len(),
        1,
        "Floating 変更済み状態からの Undo -> Redo -> 選択解除後もマージ描画（Union）が確定されること"
    );
}

#[test]
fn test_corrupt_and_truncated_file_returns_error_without_modifying_app() {
    let tmp_corrupt =
        std::env::temp_dir().join(format!("is2d_corrupt_test_{}.is2d", std::process::id()));
    let mut app = Is2dApp::default();
    app.doc.stage_size = Vec2::new(960.0, 540.0);
    let good = saved_file_bytes(
        &crate::canvas_tool::Tab::new_empty(crate::canvas_tool::TabIds::default().fresh(), 9).committed_document_copy().unwrap(),
    );

    // 1. 途中で切れたファイル
    std::fs::write(&tmp_corrupt, &good[..good.len() / 2]).unwrap();
    let res1 = crate::project_io::open_project_in_tab(&mut app, &tmp_corrupt, None);
    assert!(
        res1.is_err(),
        "途中で切れたファイルはパニックせず Err を返すこと"
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
    let mut future = good.clone();
    future[4..8].copy_from_slice(&999u32.to_le_bytes());
    std::fs::write(&tmp_corrupt, &future).unwrap();
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
          "version": 5,
          "stage_size": [99999.0, 10.0],
          "next_symbol_id": 1,
          "next_group_id": 1,
          "map_tiles": {
            "colors": [[255, 0, 0, 255]],
            "tiles": [{
              "nodes": [[100.0, 100.0], [100.0, 200.0], [200.0, 200.0]],
              "edges": [
                {"a": 0, "b": 1, "ctrl": [100.0, 150.0], "left": 0, "right": 1, "left_src": 0, "right_src": 0},
                {"a": 1, "b": 2, "ctrl": [150.0, 200.0], "left": 0, "right": 1, "left_src": 0, "right_src": 0},
                {"a": 2, "b": 0, "ctrl": [150.0, 150.0], "left": 0, "right": 1, "left_src": 0, "right_src": 0}
              ]
            }]
          },
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
                    "maps": [
                      {
                        "group_path": [],
                        "tiles": [0]
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
                        "order": 0
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
                        "order": 1
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
                            "order": 0
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
                            "order": 0
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
    write_project_json_as_file(&tmp_malformed, malformed_json);
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
    write_project_json_as_file(&tmp_malformed, &malformed_no_cycle);
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
        sanitized_app
            .doc
            .root
            .evaluate_layer_shapes_at(0, 0, sanitized_app.doc.library())[0]
            .layer_id,
        sanitized_app.doc.root.layers[0].id,
        "セル画の図形は、所属レイヤーの図形として出ること"
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
            .instances()
            .len(),
        1,
        "存在しない symbol_id=9999 の孤児インスタンスが除去されていること"
    );
    assert!(
        sanitized_app.doc.root.layers[0].keyframes[0].cel.shapes()[0]
            .exterior()
            .signed_area()
            > 0.0,
        "逆向き（負面積）だった外周輪郭のワインディングが正面積に正規化されていること"
    );

    let _ = std::fs::remove_file(&tmp_malformed);
}

#[test]
fn test_saved_file_reads_back_and_undo_history_cap() {
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
    assert!(
        read_saved_file(&tmp_compact).is_ok(),
        "保存したファイルを読み戻せること"
    );
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
fn test_u64_max_entity_ids_in_crafted_json_remapped_without_collision() {
    // u64::MAX の layer.id, symbol.id, instance_gid, group_path を含む細工ファイル (#13)
    let crafted_json = format!(
        r#"{{
          "version": 5,
          "stage_size": [800.0, 450.0],
          "next_symbol_id": {max},
          "next_group_id": {max},
          "map_tiles": {{
            "colors": [[255, 0, 0, 255]],
            "tiles": [{{
              "nodes": [[100.0, 100.0], [200.0, 100.0], [200.0, 200.0]],
              "edges": [
                {{"a": 0, "b": 1, "ctrl": [150.0, 100.0], "left": 1, "right": 0, "left_src": 0, "right_src": 0}},
                {{"a": 1, "b": 2, "ctrl": [200.0, 150.0], "left": 1, "right": 0, "left_src": 0, "right_src": 0}},
                {{"a": 2, "b": 0, "ctrl": [150.0, 150.0], "left": 1, "right": 0, "left_src": 0, "right_src": 0}}
              ]
            }}]
          }},
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
                    "maps": [
                      {{
                        "group_path": [{max}],
                        "tiles": [0]
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
                        "order": 1
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

    let doc: SavedProject = serde_json::from_str(&crafted_json).unwrap();
    let mut app = Is2dApp::default();
    doc.apply_to_app(&mut app);

    // レイヤーID・シンボルID・グループIDがすべて安全範囲内に再採番され重複していないこと
    assert_ne!(app.doc.root.layers[0].id, app.doc.root.layers[1].id);
    assert!(app.doc.root.layers[0].id <= save_format::MAX_SAFE_ENTITY_ID);
    assert!(app.doc.root.layers[1].id <= save_format::MAX_SAFE_ENTITY_ID);
    assert_ne!(app.doc.library().at(0).unwrap().id, app.doc.library().at(1).unwrap().id);
    assert!(app.doc.library().at(0).unwrap().id <= save_format::MAX_SAFE_ENTITY_ID);
    assert!(app.doc.library().at(1).unwrap().id <= save_format::MAX_SAFE_ENTITY_ID);

    // 読み込み後に新規レイヤー・新規シンボル・新規グループIDを追加しても既存IDと一切衝突しないこと (#13)
    doc_actions::timeline_add_layer(&mut app);
    let layer_ids: std::collections::HashSet<u64> =
        app.doc.root.layers.iter().map(|l| l.id).collect();
    assert_eq!(
        layer_ids.len(),
        3,
        "新規レイヤーのIDが既存レイヤーと衝突しないこと (#13)"
    );

    let new_sym_id = app.doc.register_new_symbol(Vec::new());
    assert!(
        symbols_in_order(app.doc.library())
            .filter(|s| s.id == new_sym_id)
            .count()
            == 1,
        "新規シンボルのIDが既存シンボルと衝突しないこと (#13)"
    );
    let g1 = app.doc.alloc_group_id();
    let g2 = app.doc.alloc_group_id();
    assert_ne!(g1, g2, "alloc_group_id が飽和衝突しないこと (#13)");
}

/// テスト用：保存用の写し `proj` の地図 `m` の中身を、ビットまで比べられる形にしたもの（階層の番号と、塊の順の辺。色は
/// 色の表から引いた値、座標は小数の表し方（読み戻すとビットまで同じ値））
fn saved_map_content(proj: &SavedProject, m: &timeline::SavedScopeMap) -> String {
    let color = |i: u32| (i > 0).then(|| proj.map_tiles.colors[i as usize - 1]);
    let edges: Vec<_> = m
        .tiles
        .iter()
        .map(|&t| &proj.map_tiles.tiles[t as usize])
        .flat_map(|t| t.edges.iter().map(move |e| (t, e)))
        .map(|(t, e)| {
            (
                t.nodes[e.a as usize],
                t.nodes[e.b as usize],
                e.ctrl,
                color(e.left),
                color(e.right),
                e.left_src,
                e.right_src,
            )
        })
        .collect();
    format!("{:?} {edges:?}", m.group_path)
}

/// テスト用：保存用の写し `proj` の、ルートの最初のレイヤーのキーフレーム `kf` の最初の地図の、最初の塊の辺1本の片側の色を
/// 新しい色にする（面の輪郭の色がそろわない、正しくない地図になる）。塊はほかの地図と共有していることがあるので、写して
/// から変え、その地図だけが写しを指すようにする
fn break_map_colors(proj: &mut SavedProject, kf: usize) {
    let m = &mut proj.timeline.layers[0].keyframes[kf].maps[0];
    let mut tile = proj.map_tiles.tiles[m.tiles[0] as usize].clone();
    proj.map_tiles.colors.push([1, 2, 3, 255]);
    let c = proj.map_tiles.colors.len() as u32;
    let e = &mut tile.edges[0];
    if e.left != 0 {
        e.left = c;
    } else {
        e.right = c;
    }
    m.tiles[0] = proj.map_tiles.tiles.len() as u32;
    proj.map_tiles.tiles.push(tile);
}

/// テスト用：`break_map_colors` と同じ地図の、最初の塊の頂点1つを格子の外（±65,536px）へ出す（切り直しても地図にできない：
/// `planar_map::realized` が断る）
fn break_map_off_grid(proj: &mut SavedProject, kf: usize) {
    let m = &mut proj.timeline.layers[0].keyframes[kf].maps[0];
    let mut tile = proj.map_tiles.tiles[m.tiles[0] as usize].clone();
    tile.nodes[0] = [1.0e7, 1.0e7];
    m.tiles[0] = proj.map_tiles.tiles.len() as u32;
    proj.map_tiles.tiles.push(tile);
}

/// JSON で書いた保存用の型 `json` を、今の版の `.is2d` ファイルとして `path` に書く（テストで細工したファイルを作る）
fn write_project_json_as_file(path: &std::path::Path, json: &str) {
    let doc: SavedProject = serde_json::from_str(json).unwrap();
    write_crafted_file(path, &doc);
}

/// 保存の型 `pd` を、そのまま今の版の作品ファイル `path` に書く（中身を確かめずに書く：細工したファイルを作る。`crafted_bytes`）
fn write_saved_file<T: serde::Serialize>(path: &std::path::Path, pd: &save_format::ProjectDocument<T>) -> Result<(), String> {
    std::fs::write(path, crafted_bytes(pd)?).map_err(|e| e.to_string())
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
            Action::ExitGroupSteps { steps: 1 },
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
fn test_stage6b_save_inside_two_level_nested_symbol_matches_save_after_exit_to_root() {
    use crate::canvas_tool::doc_actions::Action;

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
    assert_eq!(app.doc.library().len(), 2);

    // 1段目（外側シンボル）に入り、さらに2段目（内側シンボル）へ入る
    let (outer_binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
    run(
        &mut app,
        &[Action::EnterSymbolTimeline {
            instance_gid: outer_binding.instance_gid,
        }],
    );
    assert_eq!(app.session.edit_path().len(), 1);
    run(
        &mut app,
        &[Action::EnterGroupAt {
            pt: Point::new(140.0, 100.0),
        }],
    );
    assert_eq!(app.session.edit_path().len(), 2);

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
    assert_eq!(app.session.edit_path().len(), 2);

    // ルートに出てから保存
    exit_symbols_to(&mut app, 0);
    assert!(app.session.edit_path().is_empty());
    project_io::save_project_to_path(&mut app, &path_root).unwrap();

    let file_nested = std::fs::read(&path_nested).unwrap();
    let file_root = std::fs::read(&path_root).unwrap();
    let _ = std::fs::remove_file(&path_nested);
    let _ = std::fs::remove_file(&path_root);

    assert_eq!(
        file_nested, file_root,
        "2段ネストのシンボルに入った状態で保存したファイルと、ルートに出てから保存したファイルが完全一致すること"
    );
}

/// G5-1・N-5001：F6 で写したキーフレームは、ファイルでも開いたあとも塊を共有する。ブラシ 60 本の絵を F6 で 560 キー
/// フレームに写して保存すると、ファイルは1枚分とほとんど同じ大きさで、開き直すとどのキーフレームも1枚目と塊を共有する。
/// 直す前（版 6）は、キーフレームごとに地図を全部書いたので、ファイルが 100MB を超えた（レビュー f01d518 の N-5001：
/// 480 キーフレームで1枚分の 482 倍）
#[test]
fn test_g5_1_f6_copies_share_tiles_in_the_file_and_after_reopening() {
    use crate::canvas_tool::doc_actions::Action;

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

    let first_cel_shapes = app.doc.root.layers[0].keyframes[0].cel.shapes().len();
    let first_cel_segs: usize = app.doc.root.layers[0].keyframes[0]
        .cel
        .shapes()
        .iter()
        .map(|s| {
            s.exterior().segments.len() + s.holes().iter().map(|h| h.segments.len()).sum::<usize>()
        })
        .sum();

    let one_path = std::env::temp_dir().join(format!("is2d_g5_1_one_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &one_path).unwrap();
    let one_bytes = std::fs::metadata(&one_path).unwrap().len();
    let _ = std::fs::remove_file(&one_path);

    for _ in 1..560 {
        assert!(doc_actions::apply(
            &mut app,
            Action::InsertKeyframe { copy_prev: true }
        ));
    }
    assert_eq!(app.doc.root.total_frames, 560);
    assert_eq!(app.doc.root.layers[0].keyframes.len(), 560);

    let tmp_path =
        std::env::temp_dir().join(format!("is2d_g5_1_560kf_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).expect("保存できること");
    let file_bytes = std::fs::metadata(&tmp_path).unwrap().len();
    println!(
        "[G5-1] 60 strokes ({first_cel_shapes} shapes, {first_cel_segs} segs/cel): 1 KF {one_bytes} bytes, 560 KF {file_bytes} bytes"
    );
    // 塊は1回だけ書き、キーフレームごとに増えるのは塊の番号の並びだけ（1枚分の 1/200 より小さい。塊を写していれば
    // 1枚分ずつ増える）
    let per_keyframe = (file_bytes - one_bytes) / 559;
    assert!(
        per_keyframe * 200 < one_bytes,
        "1枚分 {one_bytes} bytes に対して、キーフレーム1つで {per_keyframe} bytes 増える（{file_bytes} bytes）"
    );

    let mut reopened = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut reopened, &tmp_path, None).expect("開き直せること");
    let _ = std::fs::remove_file(&tmp_path);
    let kfs = &reopened.doc.root.layers[0].keyframes;
    let first = kfs[0].cel.maps()[0].stored();
    for k in [1, 100, 559] {
        assert_eq!(
            kfs[k].cel.maps()[0].stored().shared_tiles(first),
            first.tiles(),
            "キーフレーム {k} は1枚目と塊を共有する"
        );
    }
    assert_eq!(reopened.doc.root.total_frames, 560);
    assert_eq!(reopened.doc.root.layers[0].keyframes.len(), 560);
    assert_eq!(
        reopened.doc.root.layers[0].keyframes[479]
            .cel
            .shapes()
            .len(),
        first_cel_shapes
    );
}

/// 版 4 は地図をそのまま書く：保存して読み込むと、地図の頂点と辺はビットまで同じで、図形はその面（図形の数も同じ）。
/// 読み込んだセルの図形の Vec は余分な容量を持たない
#[test]
fn test_m4_3_saved_map_roundtrip_bit_exact_and_load_shrinks_capacities() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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
    // 1. 地図1枚を保存の形（塊の表と塊の番号の並び。postcard）にして読み戻すと、ビットまで同じ地図に戻る
    let cel = &app.doc.root.layers[0].keyframes[0].cel;
    let base0 = cel.maps()[0].base();
    let map = &base0.map();
    let mut writer = crate::stored_map::WriteTiles::default();
    let indices = writer.tiles_of(cel.maps()[0].stored());
    let bytes = postcard::to_allocvec(&writer).unwrap();
    let table: stored_map::ReadTiles = postcard::from_bytes(&bytes).unwrap();
    let back = crate::stored_map::TileReader::new(table)
        .and_then(|r| r.map(&indices))
        .expect("地図に戻る")
        .to_dense()
        .checked()
        .expect("正しい地図に戻る");
    let bits = |m: &crate::planar_map::PlanarMap| {
        (
            m.nodes
                .iter()
                .map(|p| (p.x.to_bits(), p.y.to_bits()))
                .collect::<Vec<_>>(),
            m.edges
                .iter()
                .map(|e| {
                    (
                        e.a,
                        e.b,
                        e.ctrl.x.to_bits(),
                        e.ctrl.y.to_bits(),
                        e.left,
                        e.right,
                    )
                })
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(bits(map), bits(back.map()));

    // 2. 保存して読み込むと、地図はビットまで同じで、図形の数も同じ。図形の Vec の capacity は len と同じ
    let tmp_path = std::env::temp_dir().join(format!("m4_3_shrink_{}.is2d", std::process::id()));
    project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut loaded = Is2dApp::default();
    project_io::open_project_in_tab(&mut loaded, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    let orig = &app.doc.root.layers[0].keyframes[0].cel;
    let load = &loaded.doc.root.layers[0].keyframes[0].cel;
    assert_eq!(orig.maps().len(), load.maps().len());
    for (a, b) in orig.maps().iter().zip(load.maps().iter()) {
        assert_eq!(a.group_path, b.group_path);
        assert_eq!(bits(a.base().map()), bits(b.base().map()));
    }
    assert_eq!(orig.shapes().len(), load.shapes().len());
    assert_eq!(
        load.shapes().capacity(),
        load.shapes().len(),
        "読み込んだセルの shapes Vec の capacity が len と一致すること"
    );
}

/// 復旧ファイル（設計審査 4f8ef46 の D-3）：確認なしに終わると、未保存のタブ（保存したことのある作品も、無題の作品も）の
/// 復旧ファイルが残り、次に起動したときに見つかって、開く・捨てるを選べる。確認ありで終わると何も残らない。壊れた復旧
/// ファイルは名前を変えて残し、ほかは開く
#[test]
fn test_recovery_write_find_restore_discard_and_sanitization() {
    use eframe::App as _;

    use crate::recovery::RecoveryWriter;

    let base_dir = std::env::temp_dir().join(format!("is2d_recovery_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base_dir);
    let proj_dir = base_dir.join("projects");
    let rec_dir = base_dir.join("recovery");
    std::fs::create_dir_all(&proj_dir).unwrap();
    let files_in = |dir: &std::path::Path, ext: &str| -> usize {
        std::fs::read_dir(dir)
            .map(|d| d.flatten().filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some(ext)).count())
            .unwrap_or(0)
    };

    let red = Color32::from_rgb(220, 40, 40);
    let blue = Color32::from_rgb(40, 90, 230);
    let green = Color32::from_rgb(40, 180, 90);

    // 11. Is2dApp::default() の復旧の仕組みは、何も書かない（置き場も書き手も無い）
    let mut default_app = Is2dApp::default();
    assert!(default_app.recovery.dir.is_none());
    assert!(default_app.recovery_writer.is_none());
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
    assert_eq!(
        read_saved_file(&saved_proj_path)
            .unwrap()
            .recovery_source,
        None,
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

    // 2. 確認ありで終わる（allow_viewport_close が true）：書いてあった復旧ファイルも、みな消える
    app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    app.recovery_writer.as_ref().unwrap().flush(app.tabs.iter().map(crate::recovery::TabCopy::of).collect());
    assert_eq!(files_in(&rec_dir, "is2dj"), 2, "未保存のタブ2つ（無題も）の復旧ファイルを書く");
    app.allow_viewport_close = true;
    app.on_exit(None);
    assert!(app.recovery_writer.is_none());
    assert_eq!(std::fs::read_dir(&rec_dir).unwrap().count(), 0, "確認ありで終わったのに復旧のファイルが残った");
    assert!(!proj_dir.join("RECOVER_work_a.is2d").exists());

    // 1. 確認なしに終わる（allow_viewport_close が false）：未保存のタブ0（保存先あり）とタブ1（無題）が書かれ、保存済みの
    // タブ2は書かれない。作品のフォルダには何も書かない
    app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    app.allow_viewport_close = false;
    app.on_exit(None);
    assert_eq!(files_in(&rec_dir, "is2dj"), 2);
    assert_eq!(files_in(&rec_dir, "entry"), 2);
    assert_eq!(files_in(&rec_dir, "lock"), 0, "終わったアプリの錠のファイルが残った");
    assert_eq!(std::fs::read_dir(&proj_dir).unwrap().count(), 2, "作品のフォルダに復旧ファイルを書いた");

    // 7. 指している復旧ファイルが無い一覧と .tmp ファイルは find_leftovers で黙って消える
    let stale_entry = rec_dir.join("stale-999.entry");
    std::fs::write(
        &stale_entry,
        rec_dir
            .join("RECOVER_missing.is2dj")
            .to_string_lossy()
            .as_bytes(),
    )
    .unwrap();
    let leftover_tmp = rec_dir.join(".partial.123.tmp");
    std::fs::write(&leftover_tmp, b"partial").unwrap();

    // 12. いま動いている別のアプリ（錠を持つ書き手）のファイルは、見つけない・消さない
    let running = RecoveryWriter::start(rec_dir.clone());
    let mut other = Is2dApp::default();
    assert!(apply(
        &mut other,
        Action::PaintStroke { points: vec![Point::new(10.0, 10.0), Point::new(60.0, 10.0)], radius: 6.0, color: red },
    ));
    running.flush(other.tabs.iter().map(crate::recovery::TabCopy::of).collect());
    assert_eq!(files_in(&rec_dir, "is2dj"), 3);

    // 4. 起動したときに探す: 新しい Is2dApp から find_leftovers を呼ぶと found に2件入り、まだタブは開かない
    let mut next_app = Is2dApp::default();
    next_app.recovery.dir = Some(rec_dir.clone());
    next_app.recovery.find_leftovers();
    assert!(!stale_entry.exists());
    assert!(!leftover_tmp.exists());
    assert_eq!(next_app.recovery.found.len(), 2, "動いているアプリの復旧ファイルを見つけた、または終わったアプリの分が無い");
    // 復元のダイアログに出す名前は作品の名前（復旧ファイルの名前の頭のしるし・番号・拡張子は出さない）
    let mut names: Vec<String> = next_app.recovery.found.iter().map(|f| f.work_name()).collect();
    names.sort();
    assert_eq!(names, vec!["work_a".to_owned(), format!("無題-{}", app.tabs[1].untitled_number)]);
    assert_eq!(files_in(&rec_dir, "lock"), 1, "動いているアプリの錠のファイルを消した");
    assert_eq!(next_app.tabs.len(), 1);
    assert!(crate::project_io::is_active_tab_pristine_empty(
        &next_app.tabs[0]
    ));

    // 5. 開く: 保存先のある作品はその保存先で、無題の作品は無題で、どちらも未保存のタブとして開き（最初の空の無題タブが
    // 使い回されてタブは2つ）、復旧ファイルと一覧が消える
    next_app.apply_action(UiAction::RestoreRecovered, &ctx);
    assert!(next_app.recovery.found.is_empty());
    assert_eq!(next_app.tabs.len(), 2);
    assert!(next_app.tabs.iter().all(|t| t.history.is_dirty()));
    let restored_a = next_app.tabs.iter().find(|t| t.file.as_deref() == Some(saved_proj_path.as_path())).expect("work_a を開く");
    assert_eq!(restored_a.scene().len(), tab0_shape_count);
    assert_eq!(restored_a.scene().get(0).unwrap().color(), red);
    assert_eq!(restored_a.scene().get(1).unwrap().color(), blue);
    let restored_untitled = next_app.tabs.iter().find(|t| t.file.is_none()).expect("無題の作品を開く");
    assert!(restored_untitled.untitled_number > 0, "無題の作品に無題の番号が無い");
    assert_eq!(restored_untitled.scene().get(0).unwrap().color(), green);
    assert_eq!(files_in(&rec_dir, "entry"), 1, "開いた作品の一覧が残った（残るのは動いているアプリの分だけ）");
    assert_eq!(files_in(&rec_dir, "is2dj"), 1);
    // 動いているアプリが終わる（確認あり）：その分も消える
    running.finish(true);
    assert_eq!(std::fs::read_dir(&rec_dir).unwrap().count(), 0);

    // 6. 破棄: 開いた2つのタブを確認なしに終わって書き、次の起動で捨てる：復旧ファイルと一覧が全部消え、タブは開かない
    next_app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    next_app.allow_viewport_close = false;
    next_app.on_exit(None);
    assert_eq!(files_in(&rec_dir, "is2dj"), 2);
    let mut discard_app = Is2dApp::default();
    discard_app.recovery.dir = Some(rec_dir.clone());
    discard_app.recovery.find_leftovers();
    assert_eq!(discard_app.recovery.found.len(), 2);
    discard_app.apply_action(UiAction::DiscardRecovered, &ctx);
    assert!(discard_app.recovery.found.is_empty());
    assert_eq!(discard_app.tabs.len(), 1);
    assert!(crate::project_io::is_active_tab_pristine_empty(
        &discard_app.tabs[0]
    ));
    assert_eq!(std::fs::read_dir(&rec_dir).unwrap().count(), 0);

    // 3. 元のファイルのフォルダが無くなっていても（復旧ファイルはいつもアプリの置き場に書く）、書けて、開くと元の保存先になる
    let gone_dir = base_dir.join("gone_folder");
    let gone_proj = gone_dir.join("moved_work.is2d");
    next_app.tabs[0].file = Some(gone_proj.clone());
    next_app.tabs.truncate(1);
    next_app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    next_app.on_exit(None);
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

    // 8. 壊れた復旧ファイル（中身が .is2d の形でない・書き足すファイルの頭だけで目録が無い）は .broken に名前が変わり、
    // エラーが出て、ほかのファイルは開く。前の版のアプリが書いた `.is2d` の形の復旧ファイルも開く（元のファイル場所が
    // 無ければ無題で）
    next_app.tabs[0].file = Some(saved_proj_path.clone());
    next_app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    next_app.on_exit(None);
    let bad_rec = rec_dir.join("RECOVER_corrupted.is2d");
    let bad_entry = rec_dir.join("0-9999.entry");
    std::fs::write(&bad_rec, b"not valid json").unwrap();
    std::fs::write(&bad_entry, bad_rec.to_string_lossy().as_bytes()).unwrap();
    let bad_journal = rec_dir.join("RECOVER_headonly.is2dj");
    let bad_journal_entry = rec_dir.join("0-9997.entry");
    std::fs::write(&bad_journal, save_format::file_identity(save_format::SaveKind::Journal)).unwrap();
    std::fs::write(&bad_journal_entry, bad_journal.to_string_lossy().as_bytes()).unwrap();
    let no_src_rec = rec_dir.join("RECOVER_no_source.is2d");
    let no_src_entry = rec_dir.join("0-9998.entry");
    std::fs::copy(&saved_proj_path, &no_src_rec).unwrap();
    std::fs::write(&no_src_entry, no_src_rec.to_string_lossy().as_bytes()).unwrap();

    let mut broken_app = Is2dApp::default();
    broken_app.recovery.dir = Some(rec_dir.clone());
    broken_app.recovery.find_leftovers();
    assert_eq!(broken_app.recovery.found.len(), 4);
    // 前の版のアプリの名前（`RECOVER_{作品のファイル名}`）も、作品の名前で出す
    assert!(broken_app.recovery.found.iter().any(|f| f.work_name() == "no_source"));
    broken_app.apply_action(UiAction::RestoreRecovered, &ctx);
    let broken_renamed = rec_dir.join("RECOVER_corrupted.is2d.broken");
    let journal_broken = rec_dir.join("RECOVER_headonly.is2d.broken");
    assert!(broken_renamed.is_file());
    assert!(journal_broken.is_file());
    assert!(!bad_rec.exists());
    assert!(!bad_entry.exists());
    assert!(!bad_journal.exists());
    assert!(!bad_journal_entry.exists());
    assert!(!no_src_rec.exists());
    assert!(!no_src_entry.exists());
    assert_eq!(broken_app.tabs.len(), 2);
    assert!(broken_app.tabs.iter().any(|t| t.file.as_deref() == Some(saved_proj_path.as_path())));
    assert!(broken_app.tabs.iter().any(|t| t.file.is_none() && t.history.is_dirty()), "元のファイル場所の無い復旧ファイルを無題で開いていない");
    assert_eq!(
        broken_app.notices.iter().last().map(|t| t.kind),
        Some(crate::canvas_tool::NoticeKind::Error)
    );
    assert!(broken_app
        .notices
        .iter()
        .last()
        .is_some_and(|t| t.message.contains("RECOVER_corrupted.is2d.broken")
            && t.message.contains("RECOVER_headonly.is2d.broken")));

    // 9. 壊れた数値: 輪郭に NaN がある図形を1つ含む文書を保存すると成功し、復旧ファイルの書き出しも成功する
    let _ = std::fs::remove_dir_all(&rec_dir);
    let mut nan_app = Is2dApp::default();
    let mut shapes = make_stroke(Point::new(20.0, 20.0), Point::new(80.0, 20.0), 6.0, red);
    let mut nan_shape = make_stroke(Point::new(20.0, 60.0), Point::new(80.0, 60.0), 6.0, blue)
        .into_iter()
        .next()
        .unwrap();
    let mut nan_ext = nan_shape.exterior().clone();
    std::sync::Arc::make_mut(&mut nan_ext.segments)[0].p0.x = f64::NAN;
    nan_shape = FillShape::new(blue, nan_ext, nan_shape.holes().to_vec());
    shapes.push(nan_shape);
    put_cel_shapes(&mut nan_app, 0, 0, shapes);
    nan_app.history.mark_unsaved();

    let nan_save_path = proj_dir.join("nan_saved.is2d");
    // 座標が有限でない図形がある書き込みは、地図にできないので断る（セル画は変わらない）
    assert!(nan_app.doc.root.layers[0].keyframes[0].cel.is_empty());
    crate::project_io::save_project_to_path(&mut nan_app, &nan_save_path).unwrap();
    // 読むときに数がみな有限か確かめる（`only_finite_numbers`）ので、読めれば有限でない数は入っていない
    let reloaded = read_saved_file(&nan_save_path)
        .unwrap()
        .into_document()
        .unwrap();
    assert!(reloaded.root.layers[0].keyframes[0].cel.is_empty());

    nan_app.history.mark_unsaved();
    nan_app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    nan_app.on_exit(None);
    assert_eq!(files_in(&rec_dir, "is2dj"), 1);

    let _ = std::fs::remove_dir_all(&base_dir);
}

#[test]
fn test_open_project_reports_trimmed_data_and_marks_unsaved() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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
    let mut proj_kf = saved_project_at(&normal_path);
    proj_kf.timeline.layers[0].frame_count = 1;
    proj_kf.timeline.layers[0]
        .keyframes
        .push(crate::save_format::SavedKeyframe {
            start_frame: 5,
            tween: false,
            tween_rotate: crate::timeline::TweenRotate::Auto,
            maps: Vec::new(),
            instances: Vec::new(),
            sound: None,
        });
    write_saved_file(&out_of_range_kf_path, &proj_kf).unwrap();

    let mut app_kf = Is2dApp::default();
    assert_eq!(
        open_project_in_tab(&mut app_kf, &out_of_range_kf_path, None).unwrap(),
        OpenProjectOutcome::LoadedWithTrimmedData
    );
    assert!(app_kf.history.is_dirty());
    assert_eq!(app_kf.file.as_deref(), Some(out_of_range_kf_path.as_path()));

    // 1b. 除いた分を、ほかの所で足した分が打ち消しても見逃さない（除いた所で記す：N-5009）。ここでは、外すキーフレーム
    //     1つと、レイヤーの無いシンボルに足すレイヤー1つ（空のキーフレームを1つ持つ）。直す前は、読む前と後の数を比べたので、
    //     キーフレームの数が同じになって見逃した
    let hidden_trim_path = base_dir.join("hidden_trim.is2d");
    let mut proj_hidden = proj_kf.clone();
    proj_hidden.library.push(crate::save_format::SavedSymbolDef {
        id: 77,
        name: "空".to_owned(),
        timeline: crate::save_format::SavedTimeline {
            layers: Vec::new(),
            ..proj_hidden.timeline.clone()
        },
    });
    write_saved_file(&hidden_trim_path, &proj_hidden).unwrap();
    let mut app_hidden = Is2dApp::default();
    assert_eq!(
        open_project_in_tab(&mut app_hidden, &hidden_trim_path, None).unwrap(),
        OpenProjectOutcome::LoadedWithTrimmedData
    );
    assert!(app_hidden.history.is_dirty());

    // 2. レイヤーが 1001 個あるファイルを開いても LoadedWithTrimmedData が返り、未保存になる
    let over_layers_path = base_dir.join("over_1001_layers.is2d");
    let mut proj_layers = saved_project_at(&normal_path);
    let sample_layer = proj_layers.timeline.layers[0].clone();
    while proj_layers.timeline.layers.len() < 1001 {
        let mut l = sample_layer.clone();
        l.id = (proj_layers.timeline.layers.len() + 1) as u64;
        proj_layers.timeline.layers.push(l);
    }
    write_saved_file(&over_layers_path, &proj_layers).unwrap();

    let mut app_layers = Is2dApp::default();
    assert_eq!(
        open_project_in_tab(&mut app_layers, &over_layers_path, None).unwrap(),
        OpenProjectOutcome::LoadedWithTrimmedData
    );
    assert!(app_layers.history.is_dirty());
    assert_eq!(app_layers.file.as_deref(), Some(over_layers_path.as_path()));
    assert_eq!(app_layers.doc.root.layers.len(), 1000);

    // 4. 地図に直せないファイル（塊の番号が表の外）は開かない。正しい地図でないだけ（面の輪郭の色がそろわない）の
    //    ファイルは開き、その地図は使うときに確かめて、何も無い地図にする（`broken_map_found_later_is_drawn_empty_noticed_and_saved_as_is`）
    let broken_path = base_dir.join("broken_map.is2d");
    let mut proj_broken = saved_project_at(&normal_path);
    let n = proj_broken.map_tiles.tiles.len() as u32;
    proj_broken.timeline.layers[0].keyframes[0].maps[0].tiles[0] = n + 5;
    write_saved_file(&broken_path, &proj_broken).unwrap();
    let err = open_project_in_tab(&mut Is2dApp::default(), &broken_path, None).unwrap_err();
    assert!(err.contains("地図が壊れています"), "{err}");
    let mut proj_cross = saved_project_at(&normal_path);
    break_map_colors(&mut proj_cross, 0);
    write_saved_file(&broken_path, &proj_cross).unwrap();
    let mut app_cross = Is2dApp::default();
    assert!(open_project_in_tab(&mut app_cross, &broken_path, None).is_ok());
    assert!(app_cross.scene().len() == 0, "壊れた地図は何も無い地図");

    let _ = std::fs::remove_dir_all(&base_dir);
}

/// 復旧ファイルは、作品が変わるたびに裏で書かれる（設計審査 4f8ef46 の D-3。前は終わるときにだけ書いたので、強制終了・
/// 電源断で作業が消えた）。画面の毎コマの終わりに、未保存のタブの控えを、文書が変わったときだけ控え直して裏の書き手に
/// 渡す。浮いた片は置いた形で書く。保存して未保存でなくなったタブの復旧ファイルは消える。書いている途中で止まった
/// （終わりが欠けた）ファイルも、前に書き終えた所までの作品として開ける
#[test]
fn unsaved_tabs_snapshot_writes_the_same_recovery_and_can_be_discarded() {
    use crate::recovery::{RecoveryWriter, TabCopies};
    let base_dir = std::env::temp_dir().join(format!("is2d_unsaved_snapshot_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base_dir);
    let (proj_dir, rec_dir) = (base_dir.join("projects"), base_dir.join("recovery"));
    std::fs::create_dir_all(&proj_dir).unwrap();
    let journals = |dir: &std::path::Path| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|s| s.to_str()) == Some("is2dj")).collect())
            .unwrap_or_default()
    };
    // 裏の書き手が書き終えるのを待つ（決まった時間で見切る）
    let wait_until = |what: &str, done: &dyn Fn() -> bool| {
        let start = std::time::Instant::now();
        while !done() {
            assert!(start.elapsed() < std::time::Duration::from_secs(20), "裏の書き手が {what} しない");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let mut app = Is2dApp::default();
    app.recovery.dir = Some(rec_dir.clone());
    app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    let ctx = egui::Context::default();
    let frame = |app: &mut Is2dApp| {
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 820.0)));
        let _ = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.frame(ctx));
    };
    let red = Color32::from_rgb(220, 40, 40);
    let path = proj_dir.join("work.is2d");
    let stroke = |app: &mut Is2dApp, y: f64| {
        assert!(apply(app, Action::PaintStroke { points: vec![Point::new(30.0, y), Point::new(90.0, y)], radius: 6.0, color: red }));
    };
    // 無題のまま描く：画面の1コマで、裏で書かれる（無題の作品も控える）
    stroke(&mut app, 30.0);
    frame(&mut app);
    wait_until("無題の作品を書く", &|| journals(&rec_dir).len() == 1);
    // 保存する：復旧ファイルは消さずに「保存した」の印を書き足す（続きの変更も書き足しで済む）。この形で残ったファイルは、
    // 次の起動で聞かずに消す
    let first = journals(&rec_dir)[0].clone();
    let size_before_save = std::fs::metadata(&first).unwrap().len();
    crate::project_io::save_project_to_path(&mut app, &path).unwrap();
    frame(&mut app);
    wait_until("保存した印を書く", &|| crate::recovery_journal::is_saved(&first));
    assert!(std::fs::metadata(&first).unwrap().len() - size_before_save < 64, "保存した印のほかに書いた");
    {
        let mut probe = crate::recovery::Recovery { dir: Some(rec_dir.clone()), found: Vec::new() };
        let copy = rec_dir.join("0-1.entry");
        let saved_copy = base_dir.join("saved_copy.is2dj");
        std::fs::copy(&first, &saved_copy).unwrap();
        std::fs::write(&copy, saved_copy.to_string_lossy().as_bytes()).unwrap();
        probe.find_leftovers();
        assert!(probe.found.is_empty(), "保存した印のある復旧ファイルを、復元するものとして見つけた");
        assert!(!saved_copy.exists() && !copy.exists(), "保存した印のある復旧ファイルを消していない");
    }

    let unsaved = TabCopies::default();
    // 保存したばかり：控えはあるが、未保存のタブは無い。何も変わらなければ控え直さない
    assert!(unsaved.refresh(&app.tabs));
    assert!(!unsaved.refresh(&app.tabs));
    // 描き足す：控え直す
    stroke(&mut app, 100.0);
    assert!(unsaved.refresh(&app.tabs));
    assert_eq!(unsaved.get().len(), 1);
    assert!(!unsaved.refresh(&app.tabs));
    // 絵を持ち上げて動かしたまま（浮いた片）：控えは、置いた形で書く
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(0.0, 200.0), alt_duplicate: false }));
    assert!(app.session.floating().is_some_and(|f| f.modified()));
    assert!(unsaved.refresh(&app.tabs));
    app.recovery_writer.as_ref().unwrap().flush(unsaved.get());
    let rec = journals(&rec_dir);
    assert_eq!(rec.len(), 1);
    let written = read_saved_file(&rec[0]).unwrap();
    assert_eq!(written.recovery_source.as_deref(), Some(path.as_path()));
    // 浮いた片を置いた形：動かした先（y が 200 ずれた所）に絵がある
    let doc = written.into_document().unwrap();
    let cel = doc.root.layers[0].cel_at(0).unwrap();
    let shapes = cel.maps()[0].base().drawn_shapes(&[]);
    assert!(!shapes.is_empty());
    assert!(shapes.iter().all(|s| s.bounding_box().y0 > 200.0), "浮いた片を置いた形で書いていない");
    // もう1本描いて書き足す。その書き足しの途中で止まった（終わりの記録が欠けた）ファイルは、前に書き終えた作品として
    // 開ける（全体を書くときは一時ファイルに書いてから置き換えるので、欠けるのは書き足しだけ）
    let before = shapes.len();
    stroke(&mut app, 600.0);
    assert!(unsaved.refresh(&app.tabs));
    app.recovery_writer.as_ref().unwrap().flush(unsaved.get());
    let bytes = std::fs::read(&rec[0]).unwrap();
    std::fs::write(&rec[0], &bytes[..bytes.len() - 1]).unwrap();
    let cut = read_saved_file(&rec[0]).expect("終わりが欠けたファイルを読めない").into_document().unwrap();
    assert_eq!(cut.root.layers[0].cel_at(0).unwrap().maps()[0].base().drawn_shapes(&[]).len(), before);
    // 閉じる（確認ありで終わる）：書いたものはみな消える
    app.recovery_writer.take().unwrap().finish(true);
    assert_eq!(std::fs::read_dir(&rec_dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&base_dir);
}

/// セル画の作りの決まり（グループの入れ子は `MAX_GROUP_DEPTH` 段まで など）は1つで、読み込みとセル画を書く入口の両方で
/// 使う：上限を超えるグループ化は断って知らせ、作れる作品はみな保存して開ける（設計審査 b7208d9 の 5.。前は読み込みに
/// だけ決まりがあり、`Cmd+G` を 1,001 回押した作品は保存でき、開けなかった）
#[test]
fn a_work_the_app_can_make_always_opens() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    assert!(apply(&mut app, Action::SelectAll));
    for _ in 0..crate::timeline::MAX_GROUP_DEPTH {
        assert!(apply(&mut app, Action::Group));
    }
    app.notices = Default::default();
    assert!(!apply(&mut app, Action::Group), "上限を超えるグループ化を断らない");
    assert!(
        app.notices.iter().any(|t| t.message.contains(&format!("{} 段", crate::timeline::MAX_GROUP_DEPTH))),
        "断ったわけを知らせない"
    );
    let path = scratch_file_path("deep_groups");
    save_format::write_project_file(&path, app.tab().doc()).unwrap();
    let back = read_saved_file(&path).and_then(|d| d.into_document());
    let _ = std::fs::remove_file(&path);
    assert!(back.is_ok(), "アプリで作った作品が開けない：{:?}", back.err());
}

/// 復元でも、読み込みで外したもの（上限を超えたレイヤーなど）があれば、作品を開くときと同じく知らせる（設計審査 b7208d9
/// の 5.。前は黙って外した）
#[test]
fn restoring_a_recovery_file_reports_what_was_trimmed() {
    let rec_dir = std::env::temp_dir().join(format!("is2d_restore_trimmed_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&rec_dir);
    std::fs::create_dir_all(&rec_dir).unwrap();
    let app = Is2dApp::default();
    let mut saved = saved_of(app.tab().doc());
    let extra = saved.timeline.layers[0].clone();
    while saved.timeline.layers.len() <= crate::timeline::MAX_LAYERS_PER_TIMELINE {
        let id = saved.timeline.layers.len() as u64 + 1;
        saved.timeline.layers.push(save_format::SavedLayer { id, ..extra.clone() });
    }
    saved.next_layer_id = saved.timeline.layers.len() as u64 + 1;
    let rec = rec_dir.join("0_0-1-RECOVER_無題-1.is2d");
    write_crafted_file(&rec, &saved);
    std::fs::write(rec_dir.join("0_0-1.entry"), rec.to_string_lossy().as_bytes()).unwrap();
    let ctx = egui::Context::default();
    let mut next = Is2dApp::default();
    next.recovery.dir = Some(rec_dir.clone());
    next.recovery.find_leftovers();
    assert_eq!(next.recovery.found.len(), 1);
    next.apply_action(UiAction::RestoreRecovered, &ctx);
    // まっさらなタブに開くので、タブの数は増えない
    assert_eq!(next.tab().doc().root.layers.len(), crate::timeline::MAX_LAYERS_PER_TIMELINE, "復元できない");
    assert!(next.notices.iter().any(|t| t.message.contains("除きました")), "復元で外したものを知らせない");
    let _ = std::fs::remove_dir_all(&rec_dir);
}

/// 作業の控えの書き手は、書けなかったことを知らせ、前に書けた控えを消さない（設計審査 b7208d9 の 1.。前は、書けなかった
/// ことを黙って捨て、書き足しに失敗すると控えを忘れて、次に作り直すのに失敗すると一覧の札を消し、名前が変わったタブは
/// 前の控えを先に消していた）：(A) 置き場を作れないと、わけを1回だけ知らせ、書けるようになったら書く。(B) 書き足しにも
/// 全体の書き直しにも失敗しても、前に書けた控えが復元の一覧に残り、読める。(C) 名前が変わったタブで新しい控えを作れなく
/// ても、前の控えが残る。(D) 指す札の無い控え（札を替える前に落ちた新しい控え）は、起動したときに消える
#[cfg(unix)]
#[test]
fn the_recovery_writer_reports_failures_and_keeps_the_previous_copy() {
    use crate::recovery::{Recovery, RecoveryWriter, TabCopies};
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!("is2d_writer_fail_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let stroke = |app: &mut Is2dApp, y: f64| {
        assert!(apply(
            app,
            Action::PaintStroke { points: vec![Point::new(30.0, y), Point::new(90.0, y)], radius: 6.0, color: Color32::RED },
        ));
    };
    let with_ext = |dir: &std::path::Path, ext: &str| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some(ext)).collect())
            .unwrap_or_default()
    };
    // (A) 置き場の所にファイルがあって、置き場を作れない
    let rec = base.join("a");
    std::fs::write(&rec, b"file").unwrap();
    let writer = RecoveryWriter::start(rec.clone());
    let copies = TabCopies::default();
    let mut app = Is2dApp::default();
    let flush = |app: &Is2dApp, writer: &RecoveryWriter| {
        copies.refresh(&app.tabs);
        writer.flush(copies.get());
    };
    stroke(&mut app, 30.0);
    flush(&app, &writer);
    assert!(!writer.take_failures().is_empty(), "置き場を作れないことを知らせない");
    stroke(&mut app, 40.0);
    flush(&app, &writer);
    assert!(writer.take_failures().is_empty(), "同じわけを2回知らせた");
    std::fs::remove_file(&rec).unwrap();
    stroke(&mut app, 50.0);
    flush(&app, &writer);
    assert!(writer.take_failures().is_empty());
    assert_eq!(with_ext(&rec, "is2dj").len(), 1, "書けるようになったのに書かない");
    writer.finish(true);
    // (B) 書き足しにも、全体の書き直しにも失敗する
    let rec = base.join("b");
    let writer = RecoveryWriter::start(rec.clone());
    let copies = TabCopies::default();
    let mut app = Is2dApp::default();
    stroke(&mut app, 30.0);
    copies.refresh(&app.tabs);
    writer.flush(copies.get());
    let journal = with_ext(&rec, "is2dj").pop().expect("控え");
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::create_dir(save_format::tmp_path_for(&journal)).unwrap();
    for y in [40.0, 50.0] {
        stroke(&mut app, y);
        copies.refresh(&app.tabs);
        writer.flush(copies.get());
    }
    assert!(!writer.take_failures().is_empty(), "書けなかったことを知らせない");
    writer.finish(false);
    let mut recovery = Recovery { dir: Some(rec.clone()), ..Default::default() };
    recovery.find_leftovers();
    assert_eq!(recovery.found.len(), 1, "前に書けた控えが復元の一覧から外れた");
    assert!(read_saved_file(&journal).is_ok(), "前に書けた控えが読めない");
    // (C) 名前が変わったタブで、新しい控えを作れない
    let rec = base.join("c");
    let writer = RecoveryWriter::start(rec.clone());
    let copies = TabCopies::default();
    let mut app = Is2dApp::default();
    stroke(&mut app, 30.0);
    copies.refresh(&app.tabs);
    writer.flush(copies.get());
    let old = with_ext(&rec, "is2dj").pop().expect("控え");
    app.tabs[0].file = Some(base.join("名前を付けた.is2d"));
    stroke(&mut app, 40.0);
    let renamed = rec.join(old.file_name().unwrap().to_string_lossy().replace("無題-1", "名前を付けた"));
    std::fs::create_dir(save_format::tmp_path_for(&renamed)).unwrap();
    copies.refresh(&app.tabs);
    writer.flush(copies.get());
    assert!(!writer.take_failures().is_empty(), "新しい控えを作れなかったことを知らせない");
    writer.finish(false);
    let mut recovery = Recovery { dir: Some(rec.clone()), ..Default::default() };
    recovery.find_leftovers();
    assert_eq!(recovery.found.len(), 1, "名前が変わったタブの前の控えが消えた");
    assert!(old.is_file());
    // (D) 指す札の無い控えは、起動したときに消える
    let rec = base.join("d");
    std::fs::create_dir_all(&rec).unwrap();
    let orphan = rec.join("1_2-3-RECOVER_無題-1.is2dj");
    std::fs::copy(&old, &orphan).unwrap();
    let mut recovery = Recovery { dir: Some(rec.clone()), ..Default::default() };
    recovery.find_leftovers();
    assert!(recovery.found.is_empty());
    assert!(!orphan.exists(), "指す札の無い控えが残った");
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o644)).unwrap();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn recovery_writer_writes_only_the_touched_tab() {
    use crate::recovery::{RecoveryWriter, TabCopies};
    let rec_dir = std::env::temp_dir().join(format!("is2d_writer_touched_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&rec_dir);
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    let red = Color32::from_rgb(220, 40, 40);
    let stroke = |app: &mut Is2dApp, y: f64| {
        assert!(apply(app, Action::PaintStroke { points: vec![Point::new(30.0, y), Point::new(90.0, y)], radius: 6.0, color: red }));
    };
    // タブ0：描いて、絵を持ち上げて動かしたまま（浮いた片を置いていない）
    stroke(&mut app, 30.0);
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(0.0, 200.0), alt_duplicate: false }));
    assert!(app.session.floating().is_some_and(|f| f.modified()));
    // タブ1：描く
    crate::project_io::new_project(&mut app, &ctx);
    stroke(&mut app, 60.0);
    let writer = RecoveryWriter::start(rec_dir.clone());
    let copies = TabCopies::default();
    let flush = |app: &Is2dApp| {
        assert!(copies.refresh(&app.tabs));
        writer.flush(copies.get());
    };
    flush(&app);
    let journal_of = |id: u64| -> std::path::PathBuf {
        std::fs::read_dir(&rec_dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|s| s.to_str()) == Some("is2dj") && p.to_string_lossy().contains(&format!("-{id}-")))
            .expect("タブの復旧ファイル")
    };
    let tab0 = journal_of(app.tabs[0].id);
    let size0 = std::fs::metadata(&tab0).unwrap().len();
    // タブ1だけを何度か触る：タブ0の復旧ファイルは、書くたびに見て、1バイトも変わらない（終わりだけを見ると、書き足した
    // 量が全体を超えて全体を書き直したときに、元の大きさに戻って見逃す）
    for i in 0..3 {
        stroke(&mut app, 100.0 + f64::from(i) * 20.0);
        flush(&app);
        assert_eq!(std::fs::metadata(&tab0).unwrap().len(), size0, "{i} 回目：触っていないタブの復旧ファイルに書いた");
    }
    // タブ1の復旧ファイルは、触るたびに書き足す
    let tab1 = journal_of(app.tabs[1].id);
    assert!(std::fs::metadata(&tab1).unwrap().len() > 0);
    writer.finish(true);
    let _ = std::fs::remove_dir_all(&rec_dir);
}

#[test]
fn test_recovery_fallback_same_filename_and_missing_parent_dir() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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

    app.recovery_writer = Some(crate::recovery::RecoveryWriter::start(rec_dir.clone()));
    eframe::App::on_exit(&mut app, None);

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
        tab_gone1.doc.root.layers[0].keyframes[0].cel.shapes()[0].color(),
        red
    );
    assert_eq!(
        tab_gone2.doc.root.layers[0].keyframes[0].cel.shapes()[0].color(),
        blue
    );

    // 2. 修正6（N-4002）：前の版のアプリは復旧ファイルを作品の隣に書いた。外付けの vol/RECOVER_scene.is2d を指す一覧が
    // 残っているとき → vol の名前を変える（外した）→ find_leftovers で found は 0、一覧ファイルは残る → vol の名前を戻して
    // find_leftovers → found は 1
    let vol_dir = base_dir.join("vol");
    let vol_unmounted = base_dir.join("vol_unmounted");
    std::fs::create_dir_all(&vol_dir).unwrap();
    let vol_rec = vol_dir.join("RECOVER_scene.is2d");
    let mut vol_app = Is2dApp::default();
    assert!(apply(
        &mut vol_app,
        Action::PaintStroke {
            points: vec![Point::new(80.0, 80.0), Point::new(140.0, 80.0)],
            radius: 8.0,
            color: red,
        },
    ));
    crate::project_io::save_project_to_path(&mut vol_app, &vol_rec).unwrap();
    std::fs::write(rec_dir.join("0-1.entry"), vol_rec.to_string_lossy().as_bytes()).unwrap();

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
fn test_format_v2_contour_rounding_idempotence_and_unrounded_symbol_xform() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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

    let orig_shapes = app.doc.root.layers[0].keyframes[0].cel.shapes().clone();
    assert!(!orig_shapes.is_empty());

    let path1 = std::env::temp_dir().join(format!("is2d_fmt_v2_1_{}.is2d", std::process::id()));
    let path2 = std::env::temp_dir().join(format!("is2d_fmt_v2_2_{}.is2d", std::process::id()));
    let path3 = std::env::temp_dir().join(format!("is2d_fmt_v2_3_{}.is2d", std::process::id()));

    project_io::save_project_to_path(&mut app, &path1).unwrap();
    let file1 = std::fs::read(&path1).unwrap();

    // 3. 形式：保存したファイルは、頭（印・版）のあとの中身に、文書の塊の表（色の表と、しまう辺の並びの塊）と、セル画の
    //    階層ごとの地図（塊の番号の並び）を持つ。中身は読み戻して JSON の値にして調べる
    assert!(file1.starts_with(b"IS2D"));
    assert_eq!(
        file1[4..8],
        save_format::FORMAT_VERSION.to_le_bytes()
    );
    let parsed_val = serde_json::to_value(project_from_bytes(&file1).unwrap()).unwrap();
    let kf_json = &parsed_val["timeline"]["layers"][0]["keyframes"][0];
    assert!(kf_json.get("shapes").is_none(), "図形は書かない");
    let maps_json = kf_json["maps"].as_array().unwrap();
    let cel = &app.doc.root.layers[0].keyframes[0].cel;
    assert_eq!(maps_json.len(), cel.maps().len());
    let tiles_json = parsed_val["map_tiles"]["tiles"].as_array().unwrap();
    for (m_json, m) in maps_json.iter().zip(cel.maps().iter()) {
        let edges: usize = m_json["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                tiles_json[t.as_u64().unwrap() as usize]["edges"]
                    .as_array()
                    .unwrap()
                    .len()
            })
            .sum();
        assert_eq!(
            edges,
            m.base().map().edges.len(),
            "塊の辺を合わせると地図の辺"
        );
        assert!(m_json.get("nodes").is_none(), "頂点の番号の並びは書かない");
    }

    // 1. 開くと、図形の数は同じ（図形は地図の面）
    let mut app2 = Is2dApp::default();
    project_io::open_project_in_tab(&mut app2, &path1, None).unwrap();
    let loaded_shapes = &app2.doc.root.layers[0].keyframes[0].cel.shapes();
    assert_eq!(loaded_shapes.len(), orig_shapes.len());

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

    // 4. 版 1 のファイル：版 1 の形の輪郭を持つ JSON を開くと「前の形式（JSON）」のエラーが返り、タブは増えず、落ちないこと
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
    assert_eq!(
        v1_res,
        Err("前の形式（JSON）のファイルは開けません".to_owned()),
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
    sym_app.set_selected_indices(vec![0]);
    doc_actions::convert_selection_to_symbol(&mut sym_app).unwrap();
    assert!(scale_selection(&mut sym_app, 1.3456789012345, 0.8765432109876));
    assert!(apply(
        &mut sym_app,
        Action::RotateSelection {
            angle_rad: 0.4321098765432,
        },
    ));
    let orig_coeffs = sym_app.doc.root.layers[0].keyframes[0].cel.instances()[0]
        .binding
        .xform
        .as_coeffs();
    let sym_path =
        std::env::temp_dir().join(format!("is2d_fmt_v2_sym_{}.is2d", std::process::id()));
    project_io::save_project_to_path(&mut sym_app, &sym_path).unwrap();
    let mut sym_loaded = Is2dApp::default();
    project_io::open_project_in_tab(&mut sym_loaded, &sym_path, None).unwrap();
    let _ = std::fs::remove_file(&sym_path);
    let loaded_coeffs = sym_loaded.doc.root.layers[0].keyframes[0].cel.instances()[0]
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
fn test_reshape_edge_with_floating_commits_first_and_empty_v1_file_rejected() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use kurbo::ParamCurve;

    // 1. 持ち上げた絵（Floating）があるときに輪郭の変形が来たら、入口が先に貼り戻してから変形する（操作ごとの表
    //    `Action::policy` の `CommitFirst`。前は変形の関数の中で「持ち上げた絵があれば何もしない」と守っていた。画面で
    //    輪郭の変形を始めるときは、今までも先に貼り戻している：設計 claude/約束を入口で守る設計_5616fcc.md の 1.）
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 200.0), Point::new(300.0, 200.0)],
            radius: 10.0,
            color: Color32::RED,
        },
    ));
    let orig_shape = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].clone();
    // セル画の図形はレイヤーの番号を持たないので、タイムラインのレイヤーから取る（前はセル画の図形から取ったので、
    // 持ち上げた絵が無くても「編集できないレイヤー」として断られ、狙いと違う理由で通っていた）
    let layer_id = app.doc.root.layers[0].id;
    let orig_bbox = orig_shape.bounding_box();
    let orig_seg_count = orig_shape.exterior().segments.len();
    let orig_seg = orig_shape.exterior().segments[0];
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

    assert!(apply(
        &mut app,
        Action::ReshapeEdge {
            layer_id,
            grab_t,
            target_pos: grab_pos + kurbo::Vec2::new(0.0, 50.0),
            orig_seg,
            follow: true,
        },
    ));
    assert_eq!(app.history.undo_stack.len(), undo_before + 1);
    assert!(app.session.floating().is_none());
    let after_shape = &app.doc.root.layers[0].keyframes[0].cel.shapes()[0];
    assert_ne!(after_shape.bounding_box(), orig_bbox, "貼り戻したあとに変形していない");
    // 取り消すと、持ち上げた絵がある状態に戻る（控えは貼り戻す前に取る）
    assert!(apply(&mut app, Action::Undo));
    assert!(app.session.floating().is_some());
    let undone = &app.doc.root.layers[0].keyframes[0].cel.shapes();
    assert!(undone.iter().all(|s| s.exterior().segments.len() <= orig_seg_count + 8));

    // 2. 前の版（版 2）のファイルは、図形がなくても開くと「未対応のプロジェクトバージョン」エラーが返り、タブが増えない
    let empty_doc = SavedProject {
        stage_size: [800.0, 450.0],
        timeline: (&timeline::Timeline::default()).into(),
        library: Vec::new(),
        next_symbol_id: 1,
        next_group_id: 1,
        recovery_source: None,
        map_tiles: Default::default(),
        sounds: Vec::new(),
        next_sound_id: 1,
        next_layer_id: 2,
    };
    let mut v2_bytes = crafted_bytes(&empty_doc).unwrap();
    v2_bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
    let v2_path = std::env::temp_dir().join(format!("is2d_empty_v2_{}.is2d", std::process::id()));
    std::fs::write(&v2_path, &v2_bytes).unwrap();
    let tabs_before = app.tabs.len();
    let err = project_io::open_project_in_tab(&mut app, &v2_path, None)
        .expect_err("図形のない版 2 のファイルも開けないこと");
    let _ = std::fs::remove_file(&v2_path);
    assert!(
        err.contains("未対応のプロジェクトバージョン"),
        "想定外のエラー: {err}"
    );
    assert_eq!(app.tabs.len(), tabs_before);
}

#[test]
fn test_tiny_shape_survives_save_and_reopen() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::project_io::OpenProjectOutcome;
    use crate::vector_engine::FillShape;
    use kurbo::QuadBez;

    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 100.0)],
            radius: 6.0,
            color: Color32::RED,
        },
    ));
    let lid = app.doc.root.layers[0].id;
    let pts = [
        Point::new(300.0, 300.0),
        Point::new(300.02, 300.0),
        Point::new(300.0, 300.002),
    ];
    let segs: Vec<QuadBez> = (0..3)
        .map(|i| {
            let p0 = pts[i];
            let p2 = pts[(i + 1) % 3];
            QuadBez::new(p0, p0.lerp(p2, 0.5), p2)
        })
        .collect();
    let tiny = FillShape::new(Color32::BLUE, positive_contour(segs), Vec::new()).with_layer_id(lid);
    let cel = &mut app.doc.root.layers[0].keyframe_mut(0).cel;
    let mut shapes: Vec<FillShape> = cel.shapes().iter().map(|s| (**s).clone()).collect();
    shapes.push(tiny);
    let instances = cel.instances().as_ref().clone();
    cel.set_shapes_and_instances(shapes, instances);
    app.refresh_scene_base();
    assert_eq!(app.doc.root.layers[0].keyframes[0].cel.shapes().len(), 2);

    let dir = std::env::temp_dir().join(format!("is2d_tiny_shape_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("tiny.is2d");
    project_io::save_project_to_path(&mut app, &path).expect("save tiny project");

    let mut reopened = Is2dApp::default();
    let outcome = project_io::open_project_in_tab(&mut reopened, &path, None).expect("reopen");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(outcome, OpenProjectOutcome::Loaded);
    assert_eq!(
        reopened.doc.root.layers[0].keyframes[0].cel.shapes().len(),
        2
    );
}

/// 地図で描いた図形を保存して開き直すと、輪郭はビットまで同じで、地図も壊れない（保存で座標を丸めない）
#[test]
fn test_save_and_open_keeps_contours_bit_exact() {
    let mut app = Is2dApp::default();
    let mut r = map_overlay::Rng(17);
    let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
    for k in 0..40 {
        app.tool.brush_color = colors[k % 4];
        let p0 = Point::new(-300.0 + r.f() * 600.0, -180.0 + r.f() * 360.0);
        let p1 = Point::new(-300.0 + r.f() * 600.0, -180.0 + r.f() * 360.0);
        let pts: Vec<Point> = (0..=12).map(|i| p0.lerp(p1, i as f64 / 12.0)).collect();
        commit_brush_line(&mut app, &brush_line(&pts, 2.0 + r.f() * 10.0));
    }
    assert!(layer_map_is_valid(&app), "保存する前の地図が壊れている");
    // 輪郭の曲線を全部（図形の並びと輪郭の始まりは、読むときに地図の面から作るので比べない）
    let bits = |app: &Is2dApp| {
        let mut v: Vec<(Color32, [u64; 6])> = app
            .scene()
            .iter()
            .flat_map(|s| {
                let color = s.color();
                s.contours()
                    .flat_map(|c| c.segments.iter().copied().collect::<Vec<_>>())
                    .map(move |q| {
                        (
                            color,
                            [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y].map(f64::to_bits),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        v.sort_by_key(|x| (x.0.to_array(), x.1));
        v
    };
    let before = bits(&app);
    let path = std::env::temp_dir().join(format!("is2d_bit_exact_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &path).expect("保存");
    let mut loaded = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut loaded, &path, None).expect("読み込み");
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        bits(&loaded),
        before,
        "保存と読み込みで輪郭の座標が変わった"
    );
    assert!(layer_map_is_valid(&loaded), "開き直した地図が壊れている");
}

/// S-1：ファイルを読み込むと、同じレイヤーの前後のキーフレームで、形と色が同じ辺は同じ番号を持ち、地図の塊を共有する
#[test]
fn loaded_keyframes_share_map_tiles() {
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
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
    // 次のコマに、前のキーフレームを写したキーフレームを作り、遠い所に線を1本足す
    doc_actions::apply(
        &mut app,
        doc_actions::Action::SwitchLayerAndFrame {
            layer_idx: 0,
            frame: 1,
        },
    );
    doc_actions::timeline_insert_keyframe(&mut app, true);
    app.tool.brush_color = red;
    let pts: Vec<Point> = (0..=8)
        .map(|j| Point::new(600.0 + j as f64 * 4.0, 500.0))
        .collect();
    commit_brush_line(&mut app, &brush_line(&pts, 4.0));
    let loaded = saved_project_of(&app);
    let doc = loaded.into_document().unwrap();
    let kfs = &doc.root.layers[0].keyframes;
    assert_eq!(kfs.len(), 2);
    let (first, second) = (kfs[0].cel.maps()[0].stored(), kfs[1].cel.maps()[0].stored());
    assert!(first.tiles() > 10);
    // 足した線の所のほかは、みな同じ塊（同じ `Arc`）
    let shared = second.shared_tiles(first);
    assert!(shared + 3 >= first.tiles(), "{shared} / {}", first.tiles());
}

/// A-5：読み込みで、深すぎる階層・同じグループが2つの所にある階層は壊れたファイルとして断る。ただ1つのキーフレームを
/// 捨てて空のキーフレームに替えたときも、捨てたと知らせる。長いレイヤーは切り詰めない（コマ数に上限は無い：レビュー
/// f01d518 の N-2004）
#[test]
fn loading_checks_scopes_and_reports_every_trim() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::project_io::{open_project_in_tab, save_project_to_path, OpenProjectOutcome};
    let dir = std::env::temp_dir().join(format!("is2d_load_scopes_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let normal = dir.join("normal.is2d");
    let mut src = Is2dApp::default();
    assert!(apply(
        &mut src,
        Action::PaintStroke {
            points: vec![Point::new(40.0, 40.0), Point::new(120.0, 40.0)],
            radius: 6.0,
            color: Color32::from_rgb(220, 50, 50),
        },
    ));
    save_project_to_path(&mut src, &normal).unwrap();
    let open = |name: &str, edit: &dyn Fn(&mut SavedProject)| {
        let mut proj = saved_project_at(&normal);
        edit(&mut proj);
        let path = dir.join(name);
        write_saved_file(&path, &proj).unwrap();
        let mut app = Is2dApp::default();
        open_project_in_tab(&mut app, &path, None)
    };
    // 深すぎる階層
    assert!(open("deep.is2d", &|p| {
        p.timeline.layers[0].keyframes[0].maps[0].group_path = (1..=1_001).collect();
    })
    .is_err());
    // 同じグループ 5 が、グループ 1 と 2 の中にある
    assert!(open("twice.is2d", &|p| {
        let kf = &mut p.timeline.layers[0].keyframes[0];
        let mut m = kf.maps[0].clone();
        kf.maps[0].group_path = vec![1, 5];
        m.group_path = vec![2, 5];
        kf.maps.push(m);
    })
    .is_err());
    // 長いレイヤー（切り詰めない）
    assert_eq!(
        open("long.is2d", &|p| {
            p.timeline.layers[0].frame_count = 200_000;
        })
        .unwrap(),
        OpenProjectOutcome::Loaded
    );
    // ただ1つのキーフレームが範囲の外（空のキーフレームに替わる）
    assert_eq!(
        open("replaced.is2d", &|p| {
            let l = &mut p.timeline.layers[0];
            l.frame_count = 1;
            l.keyframes[0].start_frame = 5;
        })
        .unwrap(),
        OpenProjectOutcome::LoadedWithTrimmedData
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 10万コマより後ろのキーフレームも、保存して開き直すと残る（編集で作れる長さはみな読み戻す）。直す前は、読み込みだけが
/// コマ数を 100,000 に丸め、その後ろのキーフレームを黙って捨てた（レビュー f01d518 の N-2004）
#[test]
fn keyframes_after_frame_100000_survive_saving_and_opening() {
    let dir = std::env::temp_dir().join(format!("is2d_long_frames_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("long.is2d");
    let mut app = Is2dApp::default();
    app.tab_mut().with_active_timeline_mut(|tl, _| {
        assert!(insert_keyframe_at(tl, 0, 100_005, false, 24));
        tl.sync_total_frames();
    });
    put_cel_shapes(
        &mut app,
        0,
        100_005,
        make_stroke(
            Point::new(100.0, 100.0),
            Point::new(300.0, 140.0),
            8.0,
            Color32::from_rgb(200, 40, 40),
        ),
    );
    crate::project_io::save_project_to_path(&mut app, &path).unwrap();
    let mut opened = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut opened, &path, None).unwrap();
    let layer = &opened.doc.root.layers[0];
    assert_eq!(layer.frame_count, 100_006);
    let starts: Vec<usize> = layer.keyframes_with_start().map(|(s, _)| s).collect();
    assert_eq!(starts, vec![0, 100_005], "後ろのキーフレームが残る");
    assert!(!layer.keyframes[1].cel.shapes().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 計測用の作品のファイルを、その場で今の版に直す（ゼロベースレビューの前に `claude/review-bench/A.is2d`・`B.is2d` を
/// 直すため）。直すのは前の版（版 5 の JSON・版 6〜版 9）のファイルだけで、元のファイルは同じフォルダに `<名前>.v<版>.is2d`
/// として残す。そのあと（今の版のファイルも）、地図をみな今のコードの決まりで確かめ、合わない地図があれば切り直して書き直す
/// （正しさの決まりを変えたら、版を上げてこれで直す：審査 5616fcc の D-3 の (3)）。切り直す前のファイルは
/// `<名前>.before-recut.is2d` として残す。切り直しても作れない地図があれば止まる。直す所が無ければ変えない（何度流してもよい）
///
/// 実行：`IS2D_CONVERT=作品のパス,作品のパス cargo test --release convert_work_files_to_current_format -- --ignored --nocapture`
#[test]
#[ignore]
fn convert_work_files_to_current_format() {
    let Ok(list) = std::env::var("IS2D_CONVERT") else {
        eprintln!(
            "IS2D_CONVERT に作品のパスを（いくつもあればカンマで区切って）入れて実行してください"
        );
        return;
    };
    let opens = |path: &std::path::Path| {
        read_saved_file(path)
            .and_then(save_format::ProjectDocument::<crate::stored_map::ReadTiles>::into_document)
            .is_ok()
    };
    for path in list
        .split(',')
        .filter(|p| !p.is_empty())
        .map(std::path::Path::new)
    {
        let bytes = std::fs::read(path).expect("作品を読めること");
        // 前の版の復旧ファイル（版 1・版 2）は、隣に今の版の作品ファイル（`<名前>.is2d`）を書く（元のファイルはそのまま）
        if journal_v1::is_old(&bytes) {
            let target = path.with_extension("is2d");
            assert!(!target.exists(), "直した作品を書く所に、もうファイルがある：{}", target.display());
            journal_v1::convert(path, &target);
            assert!(opens(&target), "直した作品を開けること");
            eprintln!("直した（前の版の復旧ファイル → 今の版の作品ファイル）：{}", target.display());
            continue;
        }
        let Some((doc, version)) = old_format_document(&bytes) else {
            assert!(opens(path), "今の版として開けること：{}", path.display());
            recut_work_file(path);
            continue;
        };
        let backup = path.with_extension(format!("v{version}.is2d"));
        assert!(
            !backup.exists(),
            "元のファイルを残す所に、もうファイルがある：{}",
            backup.display()
        );
        // 直したファイルを隣に書いて開けることを確かめてから、元のファイルを残し、直したファイルに名前を替える
        // （途中で失敗しても、元のファイルは消えない）
        let converted = path.with_extension("converting.is2d");
        // 本番の書き手で書く（作品にしてから：本番が書くのと同じ形）
        save_format::write_project_file(&converted, &doc.into_document().expect("作品にできること")).unwrap();
        assert!(opens(&converted), "直したファイルを開けること");
        std::fs::rename(path, &backup).expect("元のファイルを残せること");
        std::fs::rename(&converted, path).expect("直したファイルの名前を替えられること");
        eprintln!(
            "直した（版 {version} → 今の版）：{}（{:.1}MB → {:.1}MB）。元のファイル：{}",
            path.display(),
            bytes.len() as f64 / 1e6,
            std::fs::metadata(path).unwrap().len() as f64 / 1e6,
            backup.display()
        );
        recut_work_file(path);
    }
}

/// 今の版の作品 `path` の地図をみな確かめ（`check_all_maps`）、今の決まりに合わない地図があれば、切り直した地図で書き直す
/// （元のファイルは `<名前>.before-recut.is2d`）。切り直しても作れない地図があれば止まる
fn recut_work_file(path: &std::path::Path) {
    let mut app = Is2dApp::default();
    project_io::open_project_in_tab(&mut app, path, None).expect("開ける");
    let notices = check_all_maps(&app.doc);
    let blank: Vec<_> = notices.iter().filter(|n| matches!(n, timeline::LoadNotice::Blank(_))).collect();
    assert!(blank.is_empty(), "切り直しても作れない地図がある：{}：{blank:?}", path.display());
    let recut = notices.iter().filter(|n| matches!(n, timeline::LoadNotice::Repaired(_))).count();
    if recut == 0 {
        eprintln!("地図はどれも今の決まりに合う：{}", path.display());
        return;
    }
    let backup = path.with_extension("before-recut.is2d");
    assert!(!backup.exists(), "元のファイルを残す所に、もうファイルがある：{}", backup.display());
    let written = path.with_extension("recutting.is2d");
    project_io::save_project_to_path(&mut app, &written).expect("書ける");
    std::fs::rename(path, &backup).expect("元のファイルを残せること");
    std::fs::rename(&written, path).expect("書き直したファイルの名前を替えられること");
    eprintln!("地図を {recut} 枚切り直した：{}。元のファイル：{}", path.display(), backup.display());
}

/// 作品を開くと、表示するコマの地図だけを確かめ、ほかのキーフレームは初めて使うときに確かめる。見ていないキーフレームは
/// 確かめないまま。直す前は、開くときに全部のキーフレームの地図を確かめた
#[test]
fn opening_checks_only_the_maps_of_the_shown_frame() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let path = save_many_keyframes("lazy_open", 6);
    let mut app = Is2dApp::default();
    let before = crate::planar_map::full_checks();
    crate::project_io::open_project_in_tab(&mut app, &path, None).unwrap();
    // 開くと、開いた直後に表示するコマ 1 の地図だけを確かめる（画面の土台を作るとき）
    assert_eq!(
        crate::planar_map::full_checks() - before,
        1,
        "コマ 1 の地図だけ"
    );
    assert!(!app.doc.root.layers[0].keyframes[0].cel.maps()[0].is_unchecked());
    assert!(app.scene().len() > 0);
    assert_eq!(crate::planar_map::full_checks() - before, 1, "描いても同じ");
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 4 }));
    assert!(app.scene().len() > 0);
    assert_eq!(
        crate::planar_map::full_checks() - before,
        2,
        "コマ 5 の地図を足して"
    );
    let unchecked = app.doc.root.layers[0]
        .keyframes
        .iter()
        .filter(|kf| kf.cel.maps().iter().all(|m| m.is_unchecked()))
        .count();
    assert_eq!(unchecked, 4, "見ていないキーフレームは確かめないまま");
    // 画面を描いても（タイムラインのキーフレームの印など）、見ていないキーフレームは確かめない
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
    assert_eq!(
        crate::planar_map::full_checks() - before,
        2,
        "画面を描いても増えない"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// まだ確かめていない地図を含む作品を保存すると、読んだままの地図をそのまま書く（確かめない）。直す前は、開くときに
/// 全部確かめた（地図も、しまった形から書き直した）
#[test]
fn saving_an_unviewed_file_writes_the_same_maps_without_checking() {
    let path = save_many_keyframes("lazy_save", 4);
    let out = path.with_file_name("saved_again.is2d");
    let mut app = Is2dApp::default();
    let before = crate::planar_map::full_checks();
    crate::project_io::open_project_in_tab(&mut app, &path, None).unwrap();
    let opened = crate::planar_map::full_checks() - before;
    assert_eq!(opened, 1, "開くと、表示するコマ 1 の地図だけを確かめる");
    crate::project_io::save_project_to_path(&mut app, &out).unwrap();
    assert_eq!(
        crate::planar_map::full_checks() - before,
        opened,
        "保存では確かめない"
    );
    let unchecked = app.doc.root.layers[0]
        .keyframes
        .iter()
        .filter(|kf| kf.cel.maps().iter().all(|m| m.is_unchecked()))
        .count();
    assert_eq!(
        unchecked, 3,
        "保存しても、見ていないキーフレームは確かめないまま"
    );
    // 地図を、ビットまで比べられる形にする
    let maps = |p: &std::path::Path| -> Vec<String> {
        let d = saved_project_at(p);
        d.timeline.layers[0]
            .keyframes
            .iter()
            .flat_map(|kf| kf.maps.iter())
            .map(|m| saved_map_content(&d, m))
            .collect()
    };
    let (a, b) = (maps(&path), maps(&out));
    assert_eq!(a.len(), 4);
    assert_eq!(a, b, "地図がビットまで同じ");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// 開いたあとで壊れた地図（正しい地図でない）が見つかったら、その階層は何も無いものとして描き、場所を知らせる。保存すると
/// 読んだままの地図を書く（利用者と決めた案 1）。直す前は、ファイル全体を開かなかった
#[test]
fn broken_map_found_later_is_drawn_empty_noticed_and_saved_as_is() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let path = save_many_keyframes("lazy_broken", 3);
    // コマ 3 のキーフレームの地図の頂点1つを格子の外へ出す（切り直しても地図にできない）
    let mut proj = saved_project_at(&path);
    break_map_off_grid(&mut proj, 1);
    let broken = saved_map_content(&proj, &proj.timeline.layers[0].keyframes[1].maps[0]);
    write_saved_file(&path, &proj).unwrap();

    let mut app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut app, &path, None).expect("開ける");
    assert!(app.scene().len() > 0);
    assert!(app.doc.load_notices.take().is_empty());
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 2 }));
    assert!(app.scene().len() == 0, "壊れた地図は何も無いものとして描く");
    assert!(app.doc.root.layers[0].keyframes[1].cel.maps()[0].is_broken());
    assert_eq!(
        app.doc.load_notices.take(),
        vec![timeline::LoadNotice::Blank("レイヤー「Layer 1」のコマ 3".to_owned())]
    );
    assert!(app.doc.load_notices.take().is_empty(), "知らせるのは1回");
    // 何も無いものとして見せている絵には描けない（描くと、元の地図を黙って置き換える：審査 5616fcc の D-3）
    let before = saved_map_content(&saved_project_of(&app), &saved_project_of(&app).timeline.layers[0].keyframes[1].maps[0]);
    assert!(!apply(
        &mut app,
        Action::PaintStroke { points: vec![Point::new(100.0, 100.0), Point::new(160.0, 100.0)], radius: 8.0, color: Color32::BLUE },
    ));
    assert_eq!(
        saved_map_content(&saved_project_of(&app), &saved_project_of(&app).timeline.layers[0].keyframes[1].maps[0]),
        before,
        "壊れた絵に描いた"
    );
    // 保存すると、壊れた地図は読んだままに書く
    let out = path.with_file_name("saved_again.is2d");
    crate::project_io::save_project_to_path(&mut app, &out).unwrap();
    let back = saved_project_at(&out);
    assert_eq!(
        saved_map_content(&back, &back.timeline.layers[0].keyframes[1].maps[0]),
        broken
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// 復旧の書き足すファイル（`recovery_journal`）：でたらめな操作のたびに書き足し、読み戻した作品は、そのときの作品を丸ごと
/// 保存したものと同じ（全体から作った答えと比べる：変わった所だけで作る仕組みにはお手本を付ける決まり）。書き足した量が
/// 全体の量を超えて全体を書き直したあとも同じ。書いている途中で止まった（最後の記録が欠けた）ファイルは、最後の完全な
/// 目録までの作品として読める（設計審査 4f8ef46 の D-3）
#[test]
fn recovery_journal_reads_back_the_same_work_after_every_edit() {
    use crate::recovery_journal::Journal;
    let read = read_saved_file;
    let dir = std::env::temp_dir().join(format!("is2d_journal_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("work.is2dj");
    let names = action_kind_names();
    // よく選ぶ操作：描く・選んで動かすに加えて、シンボル（ライブラリの塊）と音（音のライブラリ）も書き足すように
    let drawing = [
        "PaintStroke", "PencilStroke", "BucketFill", "InkBottle", "SelectMarquee", "FinishMoveSelectionDrag", "ConvertToSymbol",
        "EnterSymbolFromLibrary", "ImportSound", "SetKeyframeSound",
    ];
    let canonical = |pd: save_format::ProjectDocument| content_json(&pd);
    for seed in [0x4a52_0001u64, 0x4a52_0002] {
        let mut rng = OpRng(seed);
        let mut app = Is2dApp::default();
        let mut journal: Option<Journal> = None;
        let mut last_len = 0u64;
        let (mut saw_sound, mut saw_symbol, mut saw_rewrite) = (false, false, false);
        for step in 0..300 {
            let kind = if rng.range(10) < 4 { drawing[rng.range(drawing.len())].to_owned() } else { names[rng.range(names.len())].clone() };
            let action = random_action(&kind, &mut rng, &app);
            apply(&mut app, action);
            // ときどき、全部を選んでシンボルにする（でたらめな選択では、シンボルにできる絵を選ぶことが少ない）
            if step % 40 == 39 {
                apply(&mut app, Action::SelectAll);
                apply(&mut app, Action::ConvertToSymbol);
            }
            let Some(doc) = app.tab().committed_document_copy() else {
                continue;
            };
            saw_sound |= !doc.sounds.is_empty();
            saw_symbol |= !doc.library().is_empty();
            let len_before = std::fs::metadata(&path).map_or(0, |m| m.len());
            match &mut journal {
                None => journal = Some(Journal::create(&path, &doc, None).unwrap()),
                Some(j) => j.write(&doc, None).unwrap(),
            }
            // 全体を書き直した（ファイルが前より短くなった）
            saw_rewrite |= std::fs::metadata(&path).unwrap().len() < len_before;
            let back = read(&path).expect("読める");
            let mut trimmed = false;
            let restored = back.into_document_noting(&mut trimmed).expect("文書にできる");
            assert!(!trimmed, "seed={seed:#x} step={step}: 読み戻しで何かを外した");
            // お手本：同じ作品を、ふつうの保存の形で丸ごと書いて読み戻したもの（読み込みの道が直す所も同じに直る）
            let whole = saved_of(&doc).into_document().expect("丸ごと保存して読める");
            assert_eq!(
                canonical(saved_of(&restored)),
                canonical(saved_of(&whole)),
                "seed={seed:#x} step={step}: 読み戻した作品が違う"
            );
            last_len = std::fs::metadata(&path).unwrap().len();
        }
        // 書いている途中で止まった：最後の記録の途中まで切る。読めて、前の目録までの作品になる
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        assert!(read(&path).is_ok(), "最後の記録が欠けたファイルを読めない");
        assert!(last_len > 0);
        assert!(saw_sound && saw_symbol && saw_rewrite, "seed={seed:#x}: 音 {saw_sound}・シンボル {saw_symbol}・全体の書き直し {saw_rewrite} を試していない");
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 復旧の書き足すファイルは、1回の編集で、作品全体でなく変わった所だけを書き足す：キーフレームの多い作品で、1つのコマに
/// 1本描いたときに書き足す量は、全体を書いた量よりずっと小さい（設計審査 4f8ef46 の D-3。大事にする順番の 3.）
#[test]
fn recovery_journal_appends_only_what_changed() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::recovery_journal::Journal;
    let dir = std::env::temp_dir().join(format!("is2d_journal_size_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("work.is2dj");
    let mut app = Is2dApp::default();
    // 200 コマのキーフレームに、それぞれ絵を描く
    for f in 0..200 {
        if f > 0 {
            assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: false }));
        }
        let y = 100.0 + (f % 20) as f64 * 10.0;
        assert!(apply(
            &mut app,
            Action::PaintStroke { points: vec![Point::new(100.0, y), Point::new(500.0, y + 50.0)], radius: 12.0, color: Color32::RED },
        ));
    }
    let doc = app.tab().committed_document_copy().unwrap();
    let mut journal = Journal::create(&path, &doc, None).unwrap();
    let snapshot = std::fs::metadata(&path).unwrap().len();
    assert!(apply(&mut app, Action::SwitchLayerAndFrame { layer_idx: 0, frame: 50 }));
    assert!(apply(
        &mut app,
        Action::PaintStroke { points: vec![Point::new(300.0, 300.0), Point::new(320.0, 310.0)], radius: 6.0, color: Color32::BLUE },
    ));
    journal.write(&app.tab().committed_document_copy().unwrap(), None).unwrap();
    let appended = std::fs::metadata(&path).unwrap().len() - snapshot;
    assert!(appended * 20 < snapshot, "1本描いて書き足した量 {appended} が、全体 {snapshot} に比べて大きい");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ファイルの頭（種類の印と版）は、作品ファイルと復旧ファイルで同じ決まり（`file_identity`・`read_identity`）。復旧ファイルの
/// 版がこのアプリと違えば、読まずに名前を替えて中身を残し、版の違いを知らせる（前は復旧ファイルに版が無く、保存の形の違う
/// アプリで読むと、壊れたファイルとしか分からなかった：設計審査 6fb4faa の D-1）
#[test]
fn a_recovery_file_of_another_version_is_kept_and_reported() {
    use crate::recovery::RecoveryWriter;
    use save_format::{file_identity, read_identity, SaveKind, FORMAT_VERSION};
    // 頭の決まり：作品ファイルも復旧ファイルも、印と保存の形の版（版は1つ：設計審査 01e4283 の D-3）
    let mut project = b"IS2D".to_vec();
    project.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    assert_eq!(file_identity(SaveKind::Project).to_vec(), project);
    let journal = file_identity(SaveKind::Journal);
    assert_eq!(&journal[..4], b"IS2J");
    assert_eq!(read_identity(&journal), Ok(SaveKind::Journal));
    assert_eq!(read_identity(&project), Ok(SaveKind::Project));
    let mut other = journal;
    other[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert!(read_identity(&other).is_err());
    assert!(read_identity(b"IS2DJRN1").is_err(), "版を持たない前の復旧ファイル");
    // 版の違う復旧ファイルを復元すると、名前を替えて中身を残し、版の違いを知らせる
    let rec_dir = std::env::temp_dir().join(format!("is2d_journal_version_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&rec_dir);
    let mut app = Is2dApp::default();
    assert!(crate::canvas_tool::doc_actions::apply(
        &mut app,
        crate::canvas_tool::doc_actions::Action::PaintStroke { points: vec![Point::new(100.0, 100.0), Point::new(200.0, 120.0)], radius: 8.0, color: Color32::RED },
    ));
    app.recovery_writer = Some(RecoveryWriter::start(rec_dir.clone()));
    eframe::App::on_exit(&mut app, None);
    let written: Vec<std::path::PathBuf> = std::fs::read_dir(&rec_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("is2dj"))
        .collect();
    assert_eq!(written.len(), 1);
    let mut bytes = std::fs::read(&written[0]).unwrap();
    bytes[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    std::fs::write(&written[0], &bytes).unwrap();
    let ctx = egui::Context::default();
    let mut next = Is2dApp::default();
    next.recovery.dir = Some(rec_dir.clone());
    next.recovery.find_leftovers();
    assert_eq!(next.recovery.found.len(), 1);
    next.apply_action(UiAction::RestoreRecovered, &ctx);
    let kept = written[0].with_extension("is2d.broken");
    assert_eq!(std::fs::read(&kept).ok(), Some(bytes), "版の違う復旧ファイルの中身が残っていない");
    assert!(next.notices.iter().last().is_some_and(|t| t.message.contains("未対応の復旧ファイルの版")), "版の違いを知らせない");
    assert_eq!(next.tabs.len(), 1, "版の違う復旧ファイルを開いた");
    let _ = std::fs::remove_dir_all(&rec_dir);
}

/// 前の版（版 1。頭が `IS2DJRN1`）の復旧ファイルは、テスト用の変換（`convert_work_files_to_current_format`）で今の版の作品
/// ファイルに直せ、同じ作品が開ける（版 1 の復旧ファイルは、今の版の復旧ファイルから音の記録を版 1 の形に戻して作る）
#[test]
fn an_old_recovery_file_converts_to_the_current_project_file() {
    use crate::recovery_journal::Journal;
    let dir = std::env::temp_dir().join(format!("is2d_journal_v1_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = Is2dApp::default();
    assert!(import_sound(&mut app, short_wav()));
    put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
    let doc = app.tab().committed_document_copy().unwrap();
    let current = dir.join("work.is2dj");
    Journal::create(&current, &doc, None).unwrap();
    let old = dir.join("old.is2dj");
    std::fs::write(&old, journal_v1::from_current(&std::fs::read(&current).unwrap())).unwrap();
    let converted = dir.join("old.is2d");
    journal_v1::convert(&old, &converted);
    let want = read_saved_file(&current).unwrap().into_document().unwrap();
    let got = read_saved_file(&converted).unwrap().into_document().unwrap();
    assert_eq!(
        content_json(&saved_of(&got)),
        content_json(&saved_of(&want)),
        "直した作品が、今の版の復旧ファイルの作品と違う"
    );
    assert_eq!(got.sounds.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 前の版の復旧ファイルを、今の版（記録の版 3）にする変換（テスト用の変換の道具 `convert_work_files_to_current_format` が
/// 使う）。記録の並びはどの版も同じで、違うのは：版 1（頭が `IS2DJRN1`）は音の記録が名前とデータを1つにした `SavedSound`
/// で、音のデータの記録（並びの10番目）が無い。版 2 まで（保存の形の版 10）は、タイムラインの頭にレイヤーの番号の数え札を
/// 持ち、目録に作品の数え札が無い（ライブラリの塊（並びの6番目）と目録（並びの8番目）が違う）
mod journal_v1 {
    use super::*;
    use serde::{Deserialize, Serialize};

    /// 版 1 の頭
    pub(super) const HEAD: &[u8; 8] = b"IS2DJRN1";
    /// 記録の並びの番号（postcard は列挙の何番目かを先頭に書く）
    const SOUND: u8 = 1;
    const SYMBOLS: u8 = 5;
    const MANIFEST: u8 = 7;
    const SOUND_BYTES: u8 = 9;

    /// 版 2 の頭（保存の形の版 10・記録の版 2）
    fn head_v2() -> Vec<u8> {
        let mut h = b"IS2J".to_vec();
        h.extend_from_slice(&10u32.to_le_bytes());
        h.extend_from_slice(&2u32.to_le_bytes());
        h
    }

    /// 版 3 の頭（保存の形の版 11・記録の版 3。記録の枠は長さ・CRC32 だけ）
    fn head_v3() -> Vec<u8> {
        let mut h = b"IS2J".to_vec();
        h.extend_from_slice(&11u32.to_le_bytes());
        h.extend_from_slice(&3u32.to_le_bytes());
        h
    }

    /// 前の版の復旧ファイルか（版 1・版 2・版 3）
    pub(super) fn is_old(bytes: &[u8]) -> bool {
        bytes.starts_with(HEAD) || bytes.starts_with(&head_v2()) || bytes.starts_with(&head_v3())
    }

    #[derive(Serialize, Deserialize)]
    struct Layer {
        header: save_format::SavedLayer,
        keyframes: u64,
    }

    #[derive(Serialize, Deserialize)]
    struct Timeline<H> {
        header: H,
        layers: Vec<Layer>,
    }

    #[derive(Serialize, Deserialize)]
    struct Symbol<H> {
        id: u64,
        name: String,
        timeline: Timeline<H>,
    }

    #[derive(Serialize, Deserialize)]
    struct ManifestV2 {
        stage_size: [f32; 2],
        fps: u32,
        next_symbol_id: u64,
        next_group_id: u64,
        next_sound_id: u64,
        recovery_source: Option<std::path::PathBuf>,
        timeline: Timeline<format_v10::Timeline>,
        library: Vec<u64>,
        order: Vec<u64>,
        sounds: u64,
    }

    #[derive(Serialize, Deserialize)]
    struct ManifestV3 {
        stage_size: [f32; 2],
        fps: u32,
        next_symbol_id: u64,
        next_group_id: u64,
        next_sound_id: u64,
        next_layer_id: u64,
        recovery_source: Option<std::path::PathBuf>,
        timeline: Timeline<save_format::SavedTimeline>,
        library: Vec<u64>,
        order: Vec<u64>,
        sounds: u64,
    }

    /// 記録の列（長さ・CRC32・中身）を読む
    fn records(mut at: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while at.len() >= 8 {
            let len = u32::from_le_bytes(at[..4].try_into().unwrap()) as usize;
            out.push(at[8..8 + len].to_vec());
            at = &at[8 + len..];
        }
        out
    }

    fn frame(out: &mut Vec<u8>, body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&crc32fast::hash(body).to_le_bytes());
        out.extend_from_slice(body);
    }

    /// 今の版（版 12）の記録の列（枠：長さ・CRC32・種類・番号）の中身を読む
    fn records_v12(mut at: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while at.len() >= 17 {
            let len = u32::from_le_bytes(at[..4].try_into().unwrap()) as usize;
            out.push(at[17..17 + len].to_vec());
            at = &at[17 + len..];
        }
        out
    }

    /// 中身 `body`（前の版の決まりの postcard の記録：先頭の1バイトが列挙の何番目か、番号のある記録は次が番号）を、今の版の
    /// 枠（長さ・CRC32・種類・番号）で書く
    fn frame_v12(out: &mut Vec<u8>, body: &[u8]) {
        let kind = body[0];
        let id = match kind {
            1..=6 | 9 => {
                let mut v = 0u64;
                for (i, b) in body[1..].iter().enumerate() {
                    v |= u64::from(b & 0x7f) << (7 * i);
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                v
            }
            _ => 0,
        };
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&crc32fast::hash(body).to_le_bytes());
        out.push(kind);
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(body);
    }

    fn record(kind: u8, payload: &impl Serialize) -> Vec<u8> {
        let mut body = vec![kind];
        body.extend(postcard::to_allocvec(payload).unwrap());
        body
    }

    fn timeline_v3(t: Timeline<format_v10::Timeline>) -> Timeline<save_format::SavedTimeline> {
        Timeline { header: t.header.current(), layers: t.layers }
    }

    fn timeline_v2(t: Timeline<save_format::SavedTimeline>, next_layer_id: u64) -> Timeline<format_v10::Timeline> {
        let h = t.header;
        Timeline {
            header: format_v10::Timeline {
                fps: h.fps,
                total_frames: h.total_frames,
                current_frame: h.current_frame,
                active_layer: h.active_layer,
                next_layer_number: h.next_layer_number,
                next_layer_id,
                layers: h.layers,
            },
            layers: t.layers,
        }
    }

    /// 版 1 の記録の列を、版 2 の記録の列にする（音の記録を、音のデータの記録と音の記録に分ける）
    fn v1_to_v2(rest: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for body in records(rest) {
            if body[0] == SOUND {
                let s: save_format::SavedSound = postcard::from_bytes(&body[1..]).expect("版 1 の音の記録");
                // データの記録の番号は音の番号を使う（データの記録の番号は、データの記録の中で重ならなければよい）
                out.push(record(SOUND_BYTES, &(s.id, &*s.bytes)));
                out.push(record(SOUND, &(s.id, &s.name, s.id)));
            } else {
                out.push(body);
            }
        }
        out
    }

    /// 版 2 の記録の列を、今の版の記録の列にする（タイムラインの頭から数え札を外し、目録に作品の数え札を足す：どの
    /// タイムラインの数え札よりも後）
    fn v2_to_v3(bodies: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        let mut most = 1u64;
        let mut out = Vec::new();
        for body in bodies {
            match body[0] {
                SYMBOLS => {
                    let (id, syms): (u64, Vec<Symbol<format_v10::Timeline>>) = postcard::from_bytes(&body[1..]).unwrap();
                    let syms: Vec<Symbol<save_format::SavedTimeline>> = syms
                        .into_iter()
                        .map(|s| {
                            most = most.max(s.timeline.header.next_layer_id);
                            Symbol { id: s.id, name: s.name, timeline: timeline_v3(s.timeline) }
                        })
                        .collect();
                    out.push(record(SYMBOLS, &(id, syms)));
                }
                MANIFEST => {
                    let m: ManifestV2 = postcard::from_bytes(&body[1..]).unwrap();
                    most = most.max(m.timeline.header.next_layer_id);
                    out.push(record(
                        MANIFEST,
                        &ManifestV3 {
                            stage_size: m.stage_size,
                            fps: m.fps,
                            next_symbol_id: m.next_symbol_id,
                            next_group_id: m.next_group_id,
                            next_sound_id: m.next_sound_id,
                            next_layer_id: most,
                            recovery_source: m.recovery_source,
                            timeline: timeline_v3(m.timeline),
                            library: m.library,
                            order: m.order,
                            sounds: m.sounds,
                        },
                    ));
                }
                _ => out.push(body),
            }
        }
        out
    }

    /// 前の版（版 1・版 2・版 3）の復旧ファイルの中身を、今の版の中身にする
    pub(super) fn to_current(bytes: &[u8]) -> Vec<u8> {
        let v3 = match bytes.strip_prefix(head_v3().as_slice()) {
            Some(rest) => records(rest),
            None => v2_to_v3(match bytes.strip_prefix(HEAD.as_slice()) {
                Some(rest) => v1_to_v2(rest),
                None => records(bytes.strip_prefix(head_v2().as_slice()).expect("版 2 の頭")),
            }),
        };
        let mut out = save_format::file_identity(save_format::SaveKind::Journal).to_vec();
        for body in v3 {
            frame_v12(&mut out, &body);
        }
        out
    }

    /// 今の版の復旧ファイルの中身から、版 1 の中身を作る（変換を確かめるテストの材料。タイムラインの数え札は目録の作品の
    /// 数え札にする）
    pub(super) fn from_current(bytes: &[u8]) -> Vec<u8> {
        let bodies = records_v12(&bytes[save_format::IDENTITY_LEN..]);
        let most = bodies
            .iter()
            .filter(|b| b[0] == MANIFEST)
            .map(|b| postcard::from_bytes::<ManifestV3>(&b[1..]).unwrap().next_layer_id)
            .max()
            .unwrap_or(1);
        let mut data: std::collections::HashMap<u64, Vec<u8>> = std::collections::HashMap::new();
        let mut out = HEAD.to_vec();
        for body in bodies {
            let body = match body[0] {
                SOUND_BYTES => {
                    let (id, b): (u64, Vec<u8>) = postcard::from_bytes(&body[1..]).unwrap();
                    data.insert(id, b);
                    continue;
                }
                SOUND => {
                    let (id, name, b): (u64, String, u64) = postcard::from_bytes(&body[1..]).unwrap();
                    record(SOUND, &save_format::SavedSound { id, name, bytes: data[&b].clone().into() })
                }
                SYMBOLS => {
                    let (id, syms): (u64, Vec<Symbol<save_format::SavedTimeline>>) = postcard::from_bytes(&body[1..]).unwrap();
                    let syms: Vec<Symbol<format_v10::Timeline>> = syms
                        .into_iter()
                        .map(|s| Symbol { id: s.id, name: s.name, timeline: timeline_v2(s.timeline, most) })
                        .collect();
                    record(SYMBOLS, &(id, syms))
                }
                MANIFEST => {
                    let m: ManifestV3 = postcard::from_bytes(&body[1..]).unwrap();
                    record(
                        MANIFEST,
                        &ManifestV2 {
                            stage_size: m.stage_size,
                            fps: m.fps,
                            next_symbol_id: m.next_symbol_id,
                            next_group_id: m.next_group_id,
                            next_sound_id: m.next_sound_id,
                            recovery_source: m.recovery_source,
                            timeline: timeline_v2(m.timeline, m.next_layer_id),
                            library: m.library,
                            order: m.order,
                            sounds: m.sounds,
                        },
                    )
                }
                _ => body,
            };
            frame(&mut out, &body);
        }
        out
    }

    /// 前の版の復旧ファイル `old` を、今の版の作品ファイル `target` にする
    pub(super) fn convert(old: &std::path::Path, target: &std::path::Path) {
        let current = target.with_extension("converting.is2dj");
        std::fs::write(&current, to_current(&std::fs::read(old).unwrap())).unwrap();
        let doc = read_saved_file(&current).expect("今の版の復旧ファイルとして読める").into_document().unwrap();
        let _ = std::fs::remove_file(&current);
        save_format::write_project_file(target, &doc).unwrap();
    }
}

/// 今の判定に合わない地図（ここでは辺1本の片側の色がそろわない）は、開いて使うときに、変換・貼り付けと同じ決まり
/// （`planar_map::realized`）で切り直して見せ、知らせる。保存すると切り直した地図を書き、次に開くと知らせない（審査
/// 5616fcc の D-3。直す前は、何も無いものとして見せた）
#[test]
fn a_map_failing_the_current_check_is_recut_when_opened() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let path = save_many_keyframes("recut", 3);
    let mut proj = saved_project_at(&path);
    break_map_colors(&mut proj, 1);
    let broken = saved_map_content(&proj, &proj.timeline.layers[0].keyframes[1].maps[0]);
    write_saved_file(&path, &proj).unwrap();

    let mut app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut app, &path, None).expect("開ける");
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 2 }));
    assert!(app.scene().len() > 0, "切り直した絵を見せる");
    assert!(!app.doc.root.layers[0].keyframes[1].cel.maps()[0].is_broken());
    assert_eq!(
        app.doc.load_notices.take(),
        vec![timeline::LoadNotice::Repaired("レイヤー「Layer 1」のコマ 3".to_owned())]
    );
    // 切り直した絵には描ける
    assert!(apply(
        &mut app,
        Action::PaintStroke { points: vec![Point::new(300.0, 300.0), Point::new(360.0, 300.0)], radius: 8.0, color: Color32::BLUE },
    ));
    // 保存すると切り直した地図を書き、次に開くと正しい地図なので知らせない
    let out = path.with_file_name("recut_saved.is2d");
    crate::project_io::save_project_to_path(&mut app, &out).unwrap();
    let back = saved_project_at(&out);
    assert_ne!(saved_map_content(&back, &back.timeline.layers[0].keyframes[1].maps[0]), broken);
    let mut again = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut again, &out, None).expect("開ける");
    assert!(apply(&mut again, Action::ScrubRulerToFrame { frame: 2 }));
    assert!(again.doc.load_notices.take().is_empty(), "切り直して保存した地図で、また知らせた");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// 開いたあと、まだ確かめていない地図を裏のスレッドで全部確かめる（初めて見るコマで待たない）。壊れた地図は、使うときに
/// 見つけたときと同じく、何も無いものとして描き、場所を1回知らせる。閉じた文書の地図は確かめない（弱い参照で持つ）。
/// 直す前は、見るまで確かめなかった（レビュー f01d518 の R4）
#[test]
fn maps_are_checked_in_the_background_after_opening() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let path = save_many_keyframes("background_check", 4);
    let mut proj = saved_project_at(&path);
    break_map_off_grid(&mut proj, 1);
    write_saved_file(&path, &proj).unwrap();

    let mut app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut app, &path, None).expect("開ける");
    let maps = |app: &Is2dApp| -> Vec<(bool, bool)> {
        app.doc.root.layers[0]
            .keyframes
            .iter()
            .flat_map(|k| k.cel.maps().iter())
            .map(|m| (m.is_unchecked(), m.is_broken()))
            .collect()
    };
    assert_eq!(
        maps(&app),
        vec![(false, false), (true, false), (true, false), (true, false)],
        "開いただけでは、表示するコマ 1 のほかは確かめない"
    );
    let checker = app
        .doc
        .check_maps_in_background()
        .expect("スレッドを作れる");
    assert_eq!(
        checker.join().unwrap(),
        Some(3),
        "まだ確かめていない地図だけを確かめる"
    );
    assert_eq!(
        maps(&app),
        vec![
            (false, false),
            (false, true),
            (false, false),
            (false, false)
        ]
    );
    assert_eq!(
        app.doc.load_notices.take(),
        vec![timeline::LoadNotice::Blank("レイヤー「Layer 1」のコマ 3".to_owned())]
    );
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 2 }));
    assert!(app.scene().len() == 0, "壊れた地図は何も無いものとして描く");
    assert!(app.doc.load_notices.take().is_empty(), "知らせるのは1回");
    assert!(apply(&mut app, Action::ScrubRulerToFrame { frame: 4 }));
    assert!(app.scene().len() > 0);

    // 閉じた文書の地図は確かめない
    let doc = read_saved_file(&path)
        .unwrap()
        .into_document()
        .unwrap();
    let pending = crate::timeline::UncheckedMaps::of([&doc.root]);
    drop(doc);
    assert_eq!(pending.check(), 0);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// ファイルに書いた図形の印は、そのまま番号にしない（読むたびに新しい番号を続けて取り、同じ印は同じ番号にする）。辺の
/// 側の数より大きい印は、壊れたファイルとして断る。直す前（fa2e16a）は、印をそのまま番号にして、これから振る番号を
/// その後ろにしたので、u64 の終わり近くの印を書いたファイルを開くと、振る番号が 0 に戻り、どのタブでも今ある番号と
/// 重なった
#[test]
fn shape_labels_in_a_file_never_become_shape_numbers() {
    let path = save_many_keyframes("crafted_labels", 2);
    let read = || {
        read_saved_file(&path)
            .unwrap()
            .into_document()
            .unwrap()
    };
    let labels = |d: &symbol::Document| -> Vec<Vec<u64>> {
        d.root.layers[0]
            .keyframes
            .iter()
            .map(|k| {
                let mut v: Vec<u64> = k.cel.maps()[0].stored().to_dense().labels().collect();
                v.sort_unstable();
                v.dedup();
                v
            })
            .collect()
    };
    let (first, second) = (labels(&read()), labels(&read()));
    assert_eq!(
        first.iter().map(Vec::len).collect::<Vec<_>>(),
        second.iter().map(Vec::len).collect::<Vec<_>>()
    );
    assert!(first.iter().all(|v| !v.is_empty()));
    let seen: std::collections::HashSet<u64> = first.iter().flatten().copied().collect();
    assert!(
        second.iter().flatten().all(|id| !seen.contains(id)),
        "読むたびに新しい番号"
    );

    let mut proj = saved_project_at(&path);
    proj.map_tiles.tiles[0].edges[0].left_src = u64::MAX - 1;
    write_saved_file(&path, &proj).unwrap();
    let mut app = Is2dApp::default();
    let before = crate::serial::next_shape_id();
    let opened = crate::project_io::open_project_in_tab(&mut app, &path, None);
    let after = crate::serial::next_shape_id();
    assert!(
        before < after && after < before + 1_000_000,
        "振る番号は先へ少し進むだけ：{before} → {after}"
    );
    assert_eq!(
        opened.unwrap_err(),
        "プロジェクトファイルの地図が壊れています"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn project_file_round_trip_keeps_every_bit() {
    use doc_actions::Action;
    let mut app = Is2dApp::default();
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: vec![
                    Point::new(100.123456789, 200.987654321),
                    Point::new(180.5, 260.25),
                    Point::new(260.0001, 190.3),
                ],
                radius: 13.7,
                color: Color32::from_rgb(220, 60, 60),
            },
            Action::SelectSingleAt {
                pt: Point::new(180.5, 260.25),
            },
            Action::ConvertToSymbol,
            Action::SetSymbolColorEffect {
                effect: crate::symbol_binding::ColorEffect::new(0.37, -0.21),
            },
            Action::RenameSymbol {
                sym_id: 1,
                new_name: "顔のパーツ（目）".to_owned(),
            },
            Action::Deselect,
            Action::PaintStroke {
                points: vec![Point::new(400.0, 300.0), Point::new(520.75, 333.3333)],
                radius: 9.0,
                color: Color32::from_rgba_premultiplied(30, 120, 200, 200),
            },
        ],
    );
    let doc = app.tab().committed_document_copy().unwrap();
    let path = scratch_file_path("round_trip");
    save_format::write_project_file(&path, &doc).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"IS2D");
    assert_eq!(bytes[4..8], save_format::FORMAT_VERSION.to_le_bytes());
    let back = read_saved_file(&path).unwrap().into_document().unwrap();
    let _ = std::fs::remove_file(&path);
    let bits = |d: &crate::symbol::Document| format_v11::bytes(&saved_of(d)).unwrap();
    assert_eq!(bits(&back), bits(&doc), "書いて読み戻すと保存の型が変わった");
    // 復旧ファイル：復旧元のパスも戻る
    let jpath = scratch_file_path("round_trip_journal");
    let src = std::path::PathBuf::from("/tmp/作品/場面1.is2d");
    crate::recovery_journal::Journal::create(&jpath, &doc, Some(&src)).unwrap();
    let jback = read_saved_file(&jpath).unwrap();
    let _ = std::fs::remove_file(&jpath);
    assert_eq!(jback.recovery_source.as_deref(), Some(src.as_path()));
    assert_eq!(bits(&jback.into_document().unwrap()), bits(&doc));
}

/// 保存の形（版 12）を読むときは、頭から順に確かめ、違うものをそれぞれの知らせで断る：前の形式（JSON）・is2d でない・版が
/// 違う・壊れている（途中で切れた・中身が変わった：記録の CRC32 が合わない所から後は読まず、最後の目録まで読めなければ断る）・
/// 有限でない数。後ろに余りがあっても、最後の目録までの作品は開く（作品ファイルも復旧ファイルも同じ読み手：設計審査 01e4283 の D-3）
#[test]
fn project_file_refuses_wrong_or_broken_files() {
    let doc = crate::canvas_tool::Tab::new_empty(crate::canvas_tool::TabIds::default().fresh(), 1).doc().clone();
    let path = scratch_file_path("refuse");
    save_format::write_project_file(&path, &doc).unwrap();
    let good = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(opened_document(&good).is_ok());
    let err = |bytes: &[u8]| opened_document(bytes).expect_err("断る");
    let json = serde_json::to_vec(&saved_of(&doc)).unwrap();
    assert_eq!(err(&json), "前の形式（JSON）のファイルは開けません");
    assert_eq!(err(b"PK\x03\x04 zip file ........"), "is2d のファイルではありません");
    assert_eq!(err(b"IS2D"), "is2d のファイルではありません", "頭より短い");
    assert_eq!(err(b""), "is2d のファイルではありません", "空のファイル");
    let with_version = |v: u32| {
        let mut b = good.clone();
        b[4..8].copy_from_slice(&v.to_le_bytes());
        b
    };
    let next = save_format::FORMAT_VERSION + 1;
    assert_eq!(err(&with_version(next)), format!("未対応のプロジェクトバージョンです (version: {next})"));
    assert_eq!(err(&with_version(11)), "未対応のプロジェクトバージョンです (version: 11)", "前の版（頭＋中身1つ）は開かない");
    assert_eq!(err(&good[..good.len() - 1]), "ファイルが壊れています", "途中で切れた");
    let mut flipped = good.clone();
    let last = flipped.len() - 3;
    flipped[last] ^= 0x10;
    assert_eq!(err(&flipped), "ファイルが壊れています", "中身が変わった");
    let mut longer = good.clone();
    longer.push(0);
    assert!(opened_document(&longer).is_ok(), "後ろに余りがあっても、最後の目録までの作品は開く");
    // 有限でない数（JSON では書けなかった）：頂点の座標・ステージの大きさ
    let mut app = Is2dApp::default();
    run(
        &mut app,
        &[doc_actions::Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 120.0)],
            radius: 10.0,
            color: Color32::RED,
        }],
    );
    let drawn = saved_project_of(&app);
    let mut nan_node = drawn.clone();
    nan_node.map_tiles.tiles[0].nodes[0][0] = f64::NAN;
    let mut inf_stage = drawn.clone();
    inf_stage.stage_size[1] = f32::INFINITY;
    for broken in [nan_node, inf_stage] {
        assert_eq!(err(&crafted_bytes(&broken).unwrap()), "ファイルが壊れています（有限でない数があります）");
    }
}

/// レビュー e90f27c の N-5002：ファイルは流しながら読み、記録の CRC32 は読み終えてから確かめるので、壊れたファイルの長さの数
/// （塊・色・音の数、文字列の長さ）がどれだけ大きくても、その数で前もって取らない（取るのは読めたバイトの分だけ）。CRC32 を
/// 合わせても読めないので「壊れています」で断る。大きな記録（塊の表：2回目に流して読む）と小さな記録（目録：1回目に読む）の
/// どちらでも同じ
#[test]
fn project_file_with_huge_counts_or_cut_body_is_refused_without_taking_memory() {
    let mut app = Is2dApp::default();
    run(
        &mut app,
        &[doc_actions::Action::PaintStroke {
            points: vec![Point::new(100.0, 100.0), Point::new(200.0, 120.0)],
            radius: 10.0,
            color: Color32::RED,
        }],
    );
    let doc = app.doc.clone();
    let path = scratch_file_path("huge");
    save_format::write_project_file(&path, &doc).unwrap();
    let good = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    // 記録の枠（長さ・CRC32・種類・番号）と中身の場所
    let mut frames: Vec<(usize, u8, std::ops::Range<usize>)> = Vec::new();
    let mut at = save_format::IDENTITY_LEN;
    while at + 17 <= good.len() {
        let len = u32::from_le_bytes(good[at..at + 4].try_into().unwrap()) as usize;
        frames.push((at, good[at + 8], at + 17..at + 17 + len));
        at += 17 + len;
    }
    // 記録 `k` の中身を `body` にしたファイル（枠の長さと CRC32 を合わせる）
    let with_body = |k: usize, body: &[u8]| -> Vec<u8> {
        let (at, _, ref r) = frames[k];
        let mut b = good[..at].to_vec();
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(&crc32fast::hash(body).to_le_bytes());
        b.extend_from_slice(&good[at + 8..at + 17]);
        b.extend_from_slice(body);
        b.extend_from_slice(&good[r.end..]);
        b
    };
    let tiles = frames.iter().position(|f| f.1 == 0).expect("塊の表の記録");
    let manifest = frames.iter().rposition(|f| f.1 == 7).expect("目録の記録");
    for huge in [usize::MAX, 1 << 40] {
        let count = postcard::to_allocvec(&huge).unwrap();
        for k in [tiles, manifest] {
            let r = frames[k].2.clone();
            // 記録の初め（種類の番号）のあとの数を大きくし、その後ろは無い
            let broken = [&good[r.start..r.start + 1], &count].concat();
            assert_eq!(opened_document(&with_body(k, &broken)).expect_err("断る"), "ファイルが壊れています");
        }
    }
}

/// 保存の形の見本の作品（`testdata/format_sample.json`。保存の型を名前で書いた形）を、本番の操作で組み立てて書き出す。見本は
/// 保存の形の全部の項目と種類を使う（塗り・線・グループ・音のあるシンボル2つ・繰り返しの種類3つ・色効果・中心点・
/// 最初のコマ・トゥイーンの回転3つ・隠した／ロックしたレイヤー。テスト `format_sample_uses_every_field` が確かめる）。保存の
/// 型の形を変えて版を上げたら、これで見本を作り直し、指紋 `save_format::FORMAT_FINGERPRINT` を書き替える
///
/// 実行：`IS2D_WRITE_FORMAT_SAMPLE=1 cargo test --release write_format_sample -- --ignored --nocapture`
#[test]
#[ignore]
fn write_format_sample() {
    use crate::timeline::TweenRotate;
    use crate::symbol_binding::SymbolLoopMode;
    use doc_actions::{apply, Action};
    if std::env::var("IS2D_WRITE_FORMAT_SAMPLE").is_err() {
        eprintln!("IS2D_WRITE_FORMAT_SAMPLE=1 を入れて実行してください");
        return;
    }
    let mut app = Is2dApp::default();
    let run = |app: &mut Is2dApp, actions: Vec<Action>| {
        for a in actions {
            let name = format!("{a:?}");
            assert!(apply(app, a), "見本の操作が通ること：{name}");
        }
    };
    // レイヤー 1：塗り・線・グループ
    run(
        &mut app,
        vec![
            Action::PaintStroke { points: vec![Point::new(80.0, 80.0), Point::new(200.0, 120.0)], radius: 10.0, color: Color32::RED },
            Action::PencilStroke {
                points: vec![Point::new(60.0, 160.0), Point::new(140.0, 200.0), Point::new(220.0, 160.0)],
                stroke: crate::planar_map::EdgeStroke { color: Color32::BLUE, width: 3 },
            },
            Action::PaintStroke { points: vec![Point::new(300.0, 100.0), Point::new(360.0, 100.0)], radius: 12.0, color: Color32::GREEN },
            Action::SelectSingleAt { pt: Point::new(330.0, 100.0) },
            Action::Group,
            Action::Deselect,
        ],
    );
    // 音のあるシンボルと、その写し（ライブラリに2つ）
    run(
        &mut app,
        vec![
            Action::PaintStroke { points: vec![Point::new(600.0, 100.0), Point::new(640.0, 100.0)], radius: 8.0, color: Color32::YELLOW },
            Action::SelectSingleAt { pt: Point::new(620.0, 100.0) },
            Action::ConvertToSymbol,
        ],
    );
    let sym = app.doc.library().at(0).unwrap().id;
    run(&mut app, vec![Action::Deselect, Action::EnterSymbolFromLibrary { sym_id: sym }, Action::AddLayer]);
    let wav = wav_bytes(8000, 1, &(0..400).map(|i| ((i % 40) * 500 - 10000) as i16).collect::<Vec<_>>());
    assert!(import_sound(&mut app, wav));
    run(&mut app, vec![Action::ExitGroupSteps { steps: 1 }, Action::DuplicateSymbol { sym_id: sym }]);
    // レイヤー 2〜4：1つずつインスタンス（繰り返しの種類3つ・最初のコマ。3つめに色効果と中心点）。トゥイーン（インスタンスが
    // 1つのキーフレームだけに付けられる）の回転3つは、レイヤー 2 に
    let mut tween_layer = None;
    for (k, mode) in [SymbolLoopMode::Loop, SymbolLoopMode::PlayOnce, SymbolLoopMode::SingleFrame].into_iter().enumerate() {
        run(
            &mut app,
            vec![
                Action::Deselect,
                Action::AddLayer,
                Action::PlaceSymbol { sym_id: sym },
                Action::NudgeSelection { delta: KVec2::new(100.0 * k as f64, 200.0) },
            ],
        );
        let tl = &app.doc.root;
        tween_layer.get_or_insert(tl.layers[tl.active_layer].id);
        // 1つめは、置いたときの設定（繰り返し・最初のコマ 0）のまま
        if k > 0 {
            run(&mut app, vec![Action::SetSymbolPlayback { loop_mode: Some(mode), first_frame: Some(k) }]);
        }
    }
    run(&mut app, vec![Action::SetSymbolColorEffect { effect: crate::symbol_binding::ColorEffect { alpha: 0.6, brightness: -0.3 } }]);
    assert!(set_pivot(&mut app, Some(Point::new(250.0, 330.0))));
    run(&mut app, vec![Action::Deselect]);
    let at = |app: &Is2dApp, frame: usize| Action::SwitchLayerAndFrame {
        layer_idx: app.doc.root.layers.iter().position(|l| Some(l.id) == tween_layer).unwrap(),
        frame,
    };
    let go = at(&app, 0);
    run(&mut app, vec![go]);
    run(&mut app, (0..12).map(|_| Action::ExtendFrame).collect());
    for frame in [4, 8, 12] {
        let go = at(&app, frame);
        run(&mut app, vec![go, Action::InsertKeyframe { copy_prev: true }]);
    }
    for (frame, rotate) in [(0, TweenRotate::Auto), (4, TweenRotate::Clockwise), (8, TweenRotate::CounterClockwise)] {
        let go = at(&app, frame);
        run(&mut app, vec![go, Action::ToggleTween]);
        if rotate != TweenRotate::Auto {
            run(&mut app, vec![Action::SetTweenRotate { rotate }]);
        }
    }
    // 隠したレイヤーと、ロックしたレイヤー
    for hidden in [true, false] {
        let active = app.doc.root.active_layer;
        run(
            &mut app,
            vec![
                Action::SwitchLayerAndFrame { layer_idx: active, frame: 0 },
                Action::AddLayer,
                Action::PaintStroke { points: vec![Point::new(500.0, 500.0), Point::new(560.0, 520.0)], radius: 6.0, color: Color32::BLACK },
            ],
        );
        let layer_idx = app.doc.root.active_layer;
        run(
            &mut app,
            vec![if hidden { Action::SetLayerVisible { layer_idx, visible: false } } else { Action::SetLayerLocked { layer_idx, locked: true } }],
        );
    }
    let doc = app.tab().committed_document_copy().expect("置ける");
    let saved: SavedProject = format_v11::read(&format_v11::bytes(&saved_of(&doc)).unwrap()[..]).unwrap();
    format_sample_uses_every_field(&saved);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("format_sample.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let json = serde_json::to_string_pretty(&saved).unwrap();
    std::fs::write(&path, &json).unwrap();
    eprintln!("見本：{}（{} バイト）。指紋 FORMAT_FINGERPRINT = {:#010x}", path.display(), json.len(), format_fingerprint(&json));
}

/// 保存の形の見本が、保存の形の全部の項目と種類を使う（無いこともある項目は有る形で、いくつかから選ぶ項目は全部の種類で。
/// 指紋が項目の形・並び・種類の並びの変化に気づけるように）
fn format_sample_uses_every_field(pd: &SavedProject) {
    use crate::timeline::TweenRotate;
    use crate::symbol_binding::SymbolLoopMode;
    let timelines: Vec<&save_format::SavedTimeline> =
        std::iter::once(&pd.timeline).chain(pd.library.iter().map(|s| &s.timeline)).collect();
    let layers: Vec<&save_format::SavedLayer> = timelines.iter().flat_map(|t| &t.layers).collect();
    let keyframes: Vec<&save_format::SavedKeyframe> = layers.iter().flat_map(|l| &l.keyframes).collect();
    let instances: Vec<&save_format::SavedSymbolInstance> = keyframes.iter().flat_map(|k| &k.instances).collect();
    for mode in [SymbolLoopMode::Loop, SymbolLoopMode::PlayOnce, SymbolLoopMode::SingleFrame] {
        assert!(instances.iter().any(|i| i.binding.loop_mode == mode), "見本に繰り返しの種類 {mode:?} が無い");
    }
    for rotate in [TweenRotate::Auto, TweenRotate::Clockwise, TweenRotate::CounterClockwise] {
        assert!(keyframes.iter().any(|k| k.tween && k.tween_rotate == rotate), "見本にトゥイーンの回転 {rotate:?} が無い");
    }
    let checks = [
        ("音の区間", keyframes.iter().any(|k| k.sound.is_some())),
        ("隠したレイヤー", layers.iter().any(|l| !l.visible)),
        ("ロックしたレイヤー", layers.iter().any(|l| l.locked)),
        ("グループの中の地図", keyframes.iter().any(|k| k.maps.iter().any(|m| !m.group_path.is_empty()))),
        ("色効果", instances.iter().any(|i| i.binding.color_effect != crate::symbol_binding::ColorEffect::default())),
        ("中心点", instances.iter().any(|i| i.binding.pivot != [0.0, 0.0])),
        ("最初のコマ", instances.iter().any(|i| i.binding.first_frame > 0)),
        ("線", !pd.map_tiles.strokes.is_empty()),
        ("左右の図形の印", pd.map_tiles.tiles.iter().any(|t| t.edges.iter().any(|e| e.left_src != 0 && e.right_src != 0))),
        ("シンボル2つ", pd.library.len() >= 2),
        ("音", !pd.sounds.is_empty()),
    ];
    for (name, ok) in checks {
        assert!(ok, "見本に{name}が無い");
    }
}

/// 保存の形の指紋：見本の作品 `json`（`testdata/format_sample.json` の中身）を作品にし、本番の書き手で書いた作品ファイルと
/// 復旧ファイル（元のファイルの場所つき・「保存した」の印つき）のバイト列の CRC32
fn format_fingerprint(json: &str) -> u32 {
    let pd: SavedProject = serde_json::from_str(json).expect("見本を読める");
    format_sample_uses_every_field(&pd);
    let doc = pd.into_document().expect("見本を作品にできる");
    let dir = scratch_file_path("format_fingerprint");
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("sample.is2d");
    save_format::write_project_file(&project, &doc).unwrap();
    let journal = dir.join("sample.is2dj");
    let mut j = crate::recovery_journal::Journal::create(&journal, &doc, Some(std::path::Path::new("sample.is2d"))).unwrap();
    j.mark_saved().unwrap();
    let mut bytes = std::fs::read(&project).unwrap();
    bytes.extend(std::fs::read(&journal).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    crc32fast::hash(&bytes)
}

/// 保存の型の形（項目を足す・消す・型を変える・並びを変える・種類の並びを変える）を変えたのに版を上げていなければ止まる：
/// 見本の作品を今の書き手で書いたバイト列が、今の版の指紋と同じ。番号の配り方（アプリ全体の番号がどこまで進んでいるか）に
/// よらない（2回目は、1回目が番号を配ったあと）
#[test]
fn the_format_fingerprint_matches_the_version() {
    let advice = "保存の型の形が変わった：版 save_format::FORMAT_VERSION を上げ、前の版の変換をテスト（convert_work_files_to_current_format）に\
                  足し、見本（write_format_sample）と指紋 FORMAT_FINGERPRINT を作り直す";
    let json = include_str!("../../../testdata/format_sample.json");
    let first = format_fingerprint(json);
    assert_eq!(first, format_fingerprint(json), "指紋が書くたびに変わる");
    assert_eq!(first, save_format::FORMAT_FINGERPRINT, "{advice}（今の指紋 {first:#010x}）");
}
