//! 速さの測り（レビューの作品で測る。ふだんは走らせない）

use super::*;

/// 計測：チップ `chip` の、M1 の見込みの倍率（レビューのきまりの表。Pro・Max・Ultra も同じ世代の倍率。Mac 以外は 1）
fn m1_factor(chip: &str) -> f64 {
    let gen = ["M5", "M4", "M3", "M2", "M1"]
        .into_iter()
        .find(|g| chip.starts_with("Apple") && chip.split_whitespace().any(|w| w == *g));
    match gen {
        Some("M5") => 2.0,
        Some("M4") => 1.7,
        Some("M3") => 1.4,
        Some("M2") => 1.2,
        _ => 1.0,
    }
}

/// 見本の作品を作る：作品 `IS2D_WORK` のルートのキーフレームのうち、`IS2D_SAMPLE_KEEP`（「レイヤーの名前:始まりのコマ」を
/// カンマで区切る。コマは 1 から）のほかを空にして、`claude/review-bench/sample.is2d` に書く。見本に残すのは、正しさの決まり（重ね合わせ
/// の許容 `FLAT_TOL`）を 0.001・0.003 に変えると外れた絵（`list_maps_failing_the_current_check` で調べた）
///
/// 実行：`IS2D_WORK=作品のパス IS2D_SAMPLE_KEEP=... cargo test --release make_sample_work -- --ignored --nocapture`
#[test]
#[ignore]
fn make_sample_work() {
    let (Ok(work), Ok(keep)) = (std::env::var("IS2D_WORK"), std::env::var("IS2D_SAMPLE_KEEP")) else {
        eprintln!("IS2D_WORK に作品のパス、IS2D_SAMPLE_KEEP に残すキーフレームを入れて実行してください");
        return;
    };
    let keep: std::collections::HashSet<(String, usize)> = keep
        .split(',')
        .filter_map(|k| {
            let (name, frame) = k.rsplit_once(':')?;
            Some((name.to_owned(), frame.parse::<usize>().ok()? - 1))
        })
        .collect();
    let mut app = Is2dApp::default();
    project_io::open_project_in_tab(&mut app, std::path::Path::new(&work), None).expect("作品を開ける");
    let mut kept = 0;
    app.tab_mut().with_active_timeline_mut(|tl, _| {
        for layer in &mut tl.layers {
            let starts: Vec<usize> = layer.keyframes_with_start().map(|(s, _)| s).collect();
            for (k, start) in starts.into_iter().enumerate() {
                if keep.contains(&(layer.name.clone(), start)) {
                    kept += 1;
                } else {
                    layer.keyframe_mut(k).cel = timeline::Cel::default();
                }
            }
        }
    });
    assert_eq!(kept, keep.len(), "残すキーフレームが見つからない");
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("claude/review-bench/sample.is2d");
    project_io::save_project_to_path(&mut app, &out).expect("書ける");
    eprintln!("見本：{}（{:.1}MB）", out.display(), std::fs::metadata(&out).unwrap().len() as f64 / 1e6);
}

/// 計測の M1 の見込みの倍率は、レビューのきまりの表どおり（Pro・Max も同じ世代。Mac 以外は 1）
#[test]
fn m1_factor_follows_the_review_table() {
    assert_eq!(m1_factor("Apple M5"), 2.0);
    assert_eq!(m1_factor("Apple M4 Pro"), 1.7);
    assert_eq!(m1_factor("Apple M3 Max"), 1.4);
    assert_eq!(m1_factor("Apple M2"), 1.2);
    assert_eq!(m1_factor("Apple M1"), 1.0);
    assert_eq!(m1_factor("Intel(R) Xeon(R) Processor @ 2.10GHz"), 1.0);
}

/// 計測（N-2002）：ライブラリのシンボルの数（0・4,000）で、Undo / Redo とペーストの1回の重さが変わるか
#[test]
#[ignore]
fn measure_undo_and_paste_with_a_large_library() {
    use std::time::Instant;
    let red = Color32::from_rgb(235, 75, 75);
    for n in [0usize, 4000] {
        let mut app = Is2dApp::default();
        let mut shapes = make_stroke(
            Point::new(100.0, 100.0),
            Point::new(200.0, 100.0),
            10.0,
            red,
        );
        shapes.extend(make_stroke(
            Point::new(100.0, 300.0),
            Point::new(200.0, 300.0),
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
        let proto = app.doc.library().at(0).unwrap().clone();
        for k in 0..n {
            let mut s = (*proto).clone();
            s.id = app.doc.next_symbol_id.take();
            s.name = format!("bench {k}");
            app.doc.add_symbol(None, s);
        }
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(50.0, 250.0),
            Point::new(250.0, 350.0),
            false,
        );
        assert!(doc_actions::copy_selected_shapes(&mut app));
        let mut undo = Vec::new();
        let mut paste = Vec::new();
        for _ in 0..30 {
            let t = Instant::now();
            assert!(doc_actions::paste_clipboard(&mut app, false));
            paste.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
            undo.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        eprintln!(
            "symbols {n}: paste {:.3}ms, undo {:.3}ms",
            med(&mut paste),
            med(&mut undo)
        );
    }
}

/// 計測：線の多いセル画へ貼り付けた片の確定（選択を外す）の重さ。片の線とセル画の線を別々に2倍にして比べる
#[test]
#[ignore]
fn measure_commit_on_dense_cels() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use std::time::Instant;
    let ctx = eframe::egui::Context::default();
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let strokes = |n: usize, seed: u64, x0: f64, w: f64| -> Vec<FillShape> {
        let mut s = seed;
        let mut rnd = move || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 33) as f64) / ((1u64 << 31) as f64)
        };
        let mut out = Vec::new();
        for k in 0..n {
            let p0 = Point::new(x0 + rnd() * w, 100.0 + rnd() * 600.0);
            let p1 = Point::new(x0 + rnd() * w, 100.0 + rnd() * 600.0);
            out.extend(make_stroke(
                p0,
                p1,
                (4.0 + rnd() * 6.0) as f32,
                colors[k % colors.len()],
            ));
        }
        out
    };
    // 片は (本数, 種, 左端, 幅, レイヤーの数)。セル画と同じ種・範囲なら、コマ全体を同じ位置へ貼り付けたのと同じ。
    // レイヤーが2つ以上なら、本数をレイヤーに分けて描き、全部を写して作業中の1レイヤーへ貼る（片の組がレイヤーの数だけ
    // できる）
    type Piece = (usize, u64, f64, f64, usize);
    let run = |cel_n: usize, (piece_n, seed, x0, w, piece_layers): Piece| -> (f64, usize, usize) {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, strokes(cel_n, 7, 100.0, 600.0));
        let cel_edges: usize = app
            .scene()
            .iter()
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum();
        crate::project_io::new_project(&mut app, &ctx);
        for l in 0..piece_layers {
            if l > 0 {
                assert!(apply(&mut app, Action::AddLayer));
            }
            let n = piece_n / piece_layers;
            put_cel_shapes(&mut app, l, 0, strokes(n, seed + l as u64, x0, w));
        }
        let piece_edges: usize = app
            .scene()
            .iter()
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum();
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Copy));
        crate::project_io::switch_to_tab(&mut app, 0, Some(&ctx));
        let mut times = Vec::new();
        for _ in 0..7 {
            assert!(apply(&mut app, Action::Paste { in_place: true }));
            let t = Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            times.push(t.elapsed().as_secs_f64() * 1e3);
            assert!(apply(&mut app, Action::Undo));
        }
        times.sort_by(f64::total_cmp);
        (times[times.len() / 2], cel_edges, piece_edges)
    };
    let only = std::env::var("IS2D_MEASURE_CASE").ok();
    let cases: [(&str, usize, Piece); 10] = [
        ("small-200-40", 200, (40, 11, 300.0, 200.0, 1)),
        ("small-400-40", 400, (40, 11, 300.0, 200.0, 1)),
        ("small-200-80", 200, (80, 11, 300.0, 200.0, 1)),
        ("small-800-40", 800, (40, 11, 300.0, 200.0, 1)),
        ("small-200-160", 200, (160, 11, 300.0, 200.0, 1)),
        // コマ全体を、絵のある別のコマへ
        ("whole-other-200", 200, (200, 11, 100.0, 600.0, 1)),
        ("whole-other-400", 400, (400, 11, 100.0, 600.0, 1)),
        // コマ全体を、同じ絵の同じ位置へ
        ("whole-same-400", 400, (400, 7, 100.0, 600.0, 1)),
        // 3 レイヤーのコマ全体を、絵のある別のコマの作業中の1レイヤーへ（片の組が3つ）
        ("whole3-other-200", 200, (210, 11, 100.0, 600.0, 3)),
        ("whole3-other-400", 400, (405, 11, 100.0, 600.0, 3)),
    ];
    for (name, cel_n, piece) in cases {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        let (ms, ce, pe) = run(cel_n, piece);
        eprintln!("{name}: cel {ce} segs, piece {pe} segs: commit {ms:.2}ms");
    }
}

/// 計測（N-2002）：ブラシの曲線化の1回の重さを、半径・長さ・入力の点の間隔ごとに測る。手描きのような乱数の線
/// 2,000 本の時間も測る（R6）
#[test]
#[ignore]
fn measure_brush_smoothing() {
    use std::time::Instant;
    for radius in [0.5f64, 2.0, 6.0, 25.0] {
        for (len, gap) in [
            (300.0f64, 1.5f64),
            (300.0, 12.0),
            (800.0, 1.5),
            (800.0, 12.0),
        ] {
            let n = (len / gap).round() as usize;
            let pts: Vec<Point> = (0..=n)
                .map(|i| {
                    let x = 100.0 + i as f64 * gap;
                    Point::new(x, 300.0 + 40.0 * (x / 60.0).sin())
                })
                .collect();
            let mut times = Vec::new();
            let mut segs = 0;
            for _ in 0..9 {
                let t = Instant::now();
                let raw = brush_line_at_zoom(&pts, radius, 1.0);
                let shapes = brush_line_shapes_at_zoom(&raw, Color32::RED, 1.0);
                times.push(t.elapsed().as_secs_f64() * 1e3);
                segs = shapes
                    .iter()
                    .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
                    .sum();
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "r {radius} len {len} gap {gap}: {:.2}ms ({segs} segs)",
                times[times.len() / 2]
            );
        }
    }
    // 手描きのような乱数の線 2,000 本（`random_hand_strokes_follow_their_centerlines` と同じ線）：点の数ごとの平均と最大
    let mut rng = fastrand::Rng::with_seed(0x0052_6000_2026);
    let zooms = [0.25, 0.5, 1.0, 2.0, 4.0];
    let mut rows: Vec<(usize, f64)> = Vec::new();
    for _ in 0..2000 {
        let zoom = zooms[rng.usize(0..zooms.len())];
        let pts = random_hand_stroke(&mut rng, zoom);
        let radius = ((0.5 + 40.0 * rng.f64().powi(2)) / zoom).clamp(0.5, 70.0);
        let t = Instant::now();
        let raw = brush_line_at_zoom(&pts, radius, zoom);
        let shapes = brush_line_shapes_at_zoom(&raw, Color32::RED, zoom);
        rows.push((pts.len(), t.elapsed().as_secs_f64() * 1e3));
        std::hint::black_box(shapes);
    }
    let total: f64 = rows.iter().map(|r| r.1).sum();
    eprintln!("乱数の線 2,000 本：合計 {total:.0}ms");
    rows.sort_by_key(|r| r.0);
    for c in rows.chunks(400) {
        eprintln!(
            "  点 {}〜{}：平均 {:.1}ms・最大 {:.1}ms",
            c[0].0,
            c[c.len() - 1].0,
            c.iter().map(|r| r.1).sum::<f64>() / c.len() as f64,
            c.iter().map(|r| r.1).fold(0.0, f64::max)
        );
    }
}

/// 計測：線の多い地図にブラシの線を重ねる1回（重ね合わせだけ）の時間
#[test]
#[ignore]
fn measure_overlay_on_dense_map() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    use std::time::Instant;
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let area = (100.0, 100.0, 600.0, 300.0);
    let mut rng = map_overlay::Rng(7);
    let mut m = PlanarMap::default();
    for k in 0..120 {
        let (r, c) = (2.0 + rng.f() * 8.0, colors[k % colors.len()]);
        let s = wiggly_stroke(&mut rng, area, r, c);
        m = m
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
            .unwrap();
    }
    let strokes: Vec<_> = (0..40)
        .map(|k| {
            let (r, c) = (2.0 + rng.f() * 8.0, colors[k % colors.len()]);
            (wiggly_stroke(&mut rng, area, r, c), c)
        })
        .collect();
    let mut times = Vec::new();
    for _ in 0..3 {
        for (s, c) in &strokes {
            let t = Instant::now();
            let r = m
                .overlay(MapInput::Shapes(s), MapRule::Paint(Some(*c)))
                .unwrap();
            times.push(t.elapsed().as_secs_f64() * 1e3);
            assert!(r.is_valid());
        }
    }
    times.sort_by(f64::total_cmp);
    eprintln!(
        "map {} edges: overlay median {:.3}ms, p90 {:.3}ms",
        m.edges.len(),
        times[times.len() / 2],
        times[times.len() * 9 / 10]
    );
}

/// 計測（N-2001）：地図の辺の数を増やしたときの、ブラシ1本の重ね合わせ・地図の確かめ（valid_faces・checked）・しまう形に
/// する（StoredMap::from_dense）の時間
#[test]
#[ignore]
fn measure_write_back_scaling() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    use crate::stored_map::StoredMap;
    use std::time::Instant;
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let mut rng = map_overlay::Rng(3);
    let mut m = PlanarMap::default();
    let mut k = 0;
    for target in [60usize, 200, 600, 1500] {
        while k < target {
            let (r, c) = (2.0 + rng.f() * 8.0, colors[k % 4]);
            let s = wiggly_stroke(&mut rng, (100.0, 100.0, 600.0, 300.0), r, c);
            m = m
                .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
                .unwrap();
            k += 1;
        }
        let strokes: Vec<_> = (0..10)
            .map(|j| {
                let (r, c) = (3.0 + rng.f() * 5.0, colors[j % 4]);
                (
                    wiggly_stroke(&mut rng, (100.0, 100.0, 600.0, 300.0), r, c),
                    c,
                )
            })
            .collect();
        let (mut ov, mut vf, mut st, mut wr) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (s, c) in &strokes {
            let t = Instant::now();
            let next = m
                .overlay(MapInput::Shapes(s), MapRule::Paint(Some(*c)))
                .unwrap();
            ov.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let faces = next.valid_faces();
            vf.push(t.elapsed().as_secs_f64() * 1e3);
            assert!(faces.is_some());
            let t = Instant::now();
            let checked = next.clone().checked();
            wr.push(t.elapsed().as_secs_f64() * 1e3);
            assert!(checked.is_some());
            let t = Instant::now();
            let stored = StoredMap::from_dense(&next, None);
            st.push(t.elapsed().as_secs_f64() * 1e3);
            drop(stored);
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        eprintln!(
            "edges {:6}: overlay {:.2}ms, valid_faces {:.2}ms, checked {:.2}ms, from_dense {:.2}ms",
            m.edges.len(),
            med(&mut ov),
            med(&mut vf),
            med(&mut wr),
            med(&mut st)
        );
    }
}

/// 計測（N-2001）：線の多いセル画（今の地図から描いて作る）に、ブラシを1本描く1回（入口から文書への書き戻しまで）
#[test]
#[ignore]
fn measure_brush_on_dense_cel() {
    use std::time::Instant;
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let mut app = Is2dApp::default();
    let mut rng = map_overlay::Rng(21);
    let stroke = |rng: &mut map_overlay::Rng| -> Vec<Point> {
        let mut p = Point::new(-300.0 + rng.f() * 600.0, -150.0 + rng.f() * 300.0);
        let mut a = rng.f() * std::f64::consts::TAU;
        (0..40)
            .map(|_| {
                a += (rng.f() - 0.5) * 0.8;
                p += kurbo::Vec2::new(a.cos(), a.sin()) * 3.0;
                p
            })
            .collect()
    };
    for target in [300usize, 900] {
        while app
            .scene()
            .iter()
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum::<usize>()
            < target * 12
        {
            let points = stroke(&mut rng);
            let radius = (2.0 + rng.f() * 8.0) as f32;
            let color = colors[(rng.f() * 4.0) as usize % 4];
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::PaintStroke {
                    points,
                    radius,
                    color
                },
            ));
        }
        let segs: usize = app
            .scene()
            .iter()
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum();
        let mut times = Vec::new();
        for _ in 0..15 {
            let points = stroke(&mut rng);
            let t = Instant::now();
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::PaintStroke {
                    points,
                    radius: 5.0,
                    color: colors[0]
                },
            ));
            times.push(t.elapsed().as_secs_f64() * 1e3);
            assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "cel segments {segs}: brush median {:.2}ms, max {:.2}ms",
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
}

/// 計測（N-2001）：線の多いセル画の大きな面の上で、40×40 を矩形で選んで少し動かして確定したとき、しまう地図で新しく
/// なった辺のバイト数（前の地図と共有しない塊の辺。Undo の履歴1件の大きさにあたる）
#[test]
#[ignore]
fn measure_new_edges_after_moving_a_small_part() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let colors = [
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let mut rng = map_overlay::Rng(5);
    let mut shapes = vec![map_overlay::rect_shape(
        0.0,
        0.0,
        800.0,
        800.0,
        Color32::from_rgb(235, 75, 75),
    )];
    for k in 0..120 {
        let p0 = Point::new(rng.f() * 800.0, rng.f() * 800.0);
        let p1 = Point::new(rng.f() * 800.0, rng.f() * 800.0);
        shapes.extend(make_stroke(p0, p1, 3.0, colors[k % colors.len()]));
    }
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, shapes);
    let stored = |app: &Is2dApp| {
        app.doc.root.layers[0].keyframes[0].cel.maps()[0]
            .stored()
            .clone()
    };
    let mut counts = Vec::new();
    for k in 0..12 {
        let (x, y) = (60.0 + (k % 4) as f64 * 180.0, 80.0 + (k / 4) as f64 * 250.0);
        let before = stored(&app);
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(x, y),
            Point::new(x + 40.0, y + 40.0),
            false,
        );
        if app.session.selection().is_empty() {
            continue;
        }
        assert!(apply(
            &mut app,
            Action::NudgeSelection {
                delta: KVec2::new(3.0, 2.0)
            }
        ));
        assert!(canvas_tool::clear_selection(&mut app));
        let mut seen = std::collections::HashSet::new();
        let _ = before.bytes_unseen(&mut seen);
        let (_, e) = stored(&app).bytes_unseen(&mut seen);
        counts.push(e);
        assert!(apply(&mut app, Action::Undo));
    }
    counts.sort_unstable();
    eprintln!(
        "new edge bytes per move: {counts:?} (total {})",
        counts.iter().sum::<usize>()
    );
}

/// 計測（N-1001）：同じ色のドーナツ形（穴のある面）が n 個の地図で、全部の確かめ（`valid_faces`）とバケツ
/// （`fill_local`）の時間。点を含む外周を探すのが面の数の2乗で増えないか
#[test]
#[ignore]
fn measure_many_holes() {
    use crate::planar_map::{LayerBase, LocalEdit, PlanarMap};
    use std::time::Instant;
    let red = Color32::from_rgb(235, 75, 75);
    for n in [500usize, 1000, 2000, 4000] {
        let side = (n as f64).sqrt().ceil() as usize;
        let shapes: Vec<FillShape> = (0..n)
            .map(|i| {
                let (x, y) = ((i % side) as f64 * 30.0, (i / side) as f64 * 30.0);
                let outer = map_overlay::rect_shape(x, y, x + 20.0, y + 20.0, red);
                let inner = map_overlay::rect_shape(x + 5.0, y + 5.0, x + 15.0, y + 15.0, red);
                let ext = outer.contours().next().unwrap().clone();
                // 穴は外周と逆の向き
                let hole = crate::vector_engine::Contour::new(
                    inner
                        .contours()
                        .next()
                        .unwrap()
                        .segments
                        .iter()
                        .rev()
                        .map(|q| QuadBez::new(q.p2, q.p1, q.p0))
                        .collect(),
                );
                FillShape::new(red, ext, vec![hole])
            })
            .collect();
        let m = PlanarMap::from_faces(shapes.iter());
        let t = Instant::now();
        let faces = m.valid_faces().expect("正しい地図");
        let check = t.elapsed().as_secs_f64() * 1e3;
        assert_eq!(faces.len(), n);
        let base = LayerBase::new(&m);
        let t = Instant::now();
        // 真ん中のドーナツの穴の中（色の無い所）を青で塗る
        let c = side / 2;
        let p = Point::new(c as f64 * 30.0 + 10.0, c as f64 * 30.0 + 10.0);
        let filled = matches!(
            base.fill_local(p, Color32::BLUE, &[]),
            LocalEdit::Changed(_)
        );
        let bucket = t.elapsed().as_secs_f64() * 1e3;
        eprintln!("n {n}: valid_faces {check:.1}ms, bucket {bucket:.1}ms (filled {filled})");
    }
}

/// 計測の作品のファイル `path`。前の版なら、今の版に直したファイルを一時フォルダに作って、そのパスを返す（同じコマンドで
/// 測るため）
fn work_file_in_current_format(path: &std::path::Path) -> std::path::PathBuf {
    let bytes = std::fs::read(path).expect("作品を読めること");
    let Some((doc, version)) = old_format_document(&bytes) else {
        return path.to_path_buf();
    };
    let out = std::env::temp_dir().join("is2d_measure_work_current_format.is2d");
    save_format::write_project_file(&out, &doc.into_document().expect("作品にできること")).unwrap();
    eprintln!(
        "前の版（版 {version}）なので、今の版に直したファイルで測る：{:.1}MB → {:.1}MB（{}）",
        bytes.len() as f64 / 1e6,
        std::fs::metadata(&out).unwrap().len() as f64 / 1e6,
        out.display()
    );
    out
}

/// 計測：作品のファイルで、重さが気になる操作の時間を測る（Mac で作品 A・A-x2 を測るため）
///
/// 実行：`IS2D_WORK=作品のパス cargo test --release measure_work -- --ignored --nocapture`
/// - 測るコマは `IS2D_FRAME`（タイムラインに出る番号）。無ければ、線のいちばん多いコマ（地図の 32px のマスの数で比べる）
/// - 貼り付ける先のコマは `IS2D_PASTE_FRAME`。無ければ、測るコマのあとで、別の絵があるいちばん近いコマ
/// - どの操作も 7 回行い、中央値と最大を出す。どの回も Undo で元に戻してから次を行う
#[test]
#[ignore]
fn measure_work() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use std::time::Instant;
    let Ok(path) = std::env::var("IS2D_WORK") else {
        eprintln!("IS2D_WORK に作品のパスを入れて実行してください");
        return;
    };
    let path = work_file_in_current_format(std::path::Path::new(&path));
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    // 作品の中身と、コマごとの線の量（ファイルから読んだまま数える。地図を確かめさせない）
    let saved = saved_project_at(std::path::Path::new(&path));
    let (mut layers, mut keyframes, mut maps, mut edges) = (0usize, 0usize, 0usize, 0usize);
    // 地図の辺の数（塊の辺の数の和）
    let map_edges = |m: &crate::timeline::SavedScopeMap| -> usize {
        m.tiles
            .iter()
            .map(|&t| saved.map_tiles.tiles[t as usize].edges.len())
            .sum()
    };
    for tl in std::iter::once(&saved.timeline).chain(saved.library.iter().map(|s| &s.timeline)) {
        for layer in &tl.layers {
            layers += 1;
            for kf in &layer.keyframes {
                keyframes += 1;
                maps += kf.maps.len();
                edges += kf.maps.iter().map(map_edges).sum::<usize>();
            }
        }
    }
    // コマ f の辺の数（レイヤーごとに、f に出るキーフレームの地図の辺）
    let weight = |f: usize| -> usize {
        saved
            .timeline
            .layers
            .iter()
            .filter_map(|l| {
                l.keyframes
                    .iter()
                    .filter(|kf| kf.start_frame <= f)
                    .max_by_key(|kf| kf.start_frame)
            })
            .map(|kf| kf.maps.iter().map(map_edges).sum::<usize>())
            .sum()
    };
    let shown = |f: usize| -> Vec<Option<usize>> {
        saved
            .timeline
            .layers
            .iter()
            .map(|l| {
                l.keyframes
                    .iter()
                    .filter(|kf| kf.start_frame <= f)
                    .map(|kf| kf.start_frame)
                    .max()
            })
            .collect()
    };
    let total_frames = saved.timeline.total_frames.max(1);
    let env_frame = |key: &str| {
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|f| f.saturating_sub(1))
    };
    // 同じ量なら前のコマ
    let frame = env_frame("IS2D_FRAME").unwrap_or_else(|| {
        (0..total_frames)
            .max_by_key(|&f| (weight(f), std::cmp::Reverse(f)))
            .unwrap_or(0)
    });
    let paste_frame = env_frame("IS2D_PASTE_FRAME").or_else(|| {
        (frame + 1..total_frames)
            .chain(0..frame)
            .find(|&g| weight(g) > 0 && shown(g) != shown(frame))
    });
    drop(saved);
    let ctx = eframe::egui::Context::default();
    let mut app = Is2dApp::default();
    let t = Instant::now();
    crate::project_io::open_project_in_tab(&mut app, std::path::Path::new(&path), Some(&ctx))
        .expect("作品を開けること");
    let open_ms = ms(t);
    let goto = |app: &mut Is2dApp, f: usize| {
        assert!(canvas_tool::clear_selection(app));
        apply(app, Action::ScrubRulerToFrame { frame: f });
    };
    // 元に戻す：Undo の控えが `n` 個になるまで Undo する
    let undo_to = |app: &mut Is2dApp, n: usize| {
        while app.history.undo_stack.len() > n {
            assert!(apply(app, Action::Undo));
        }
    };
    goto(&mut app, frame);
    let shapes = app.scene().len();
    let segments: usize = app
        .scene()
        .iter()
        .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
        .sum();
    let bounds = app
        .scene()
        .iter()
        .filter_map(|s| s.geometry_bounding_box())
        .reduce(|a, b| a.union(b))
        .expect("測るコマに絵がある");
    let mut rows: Vec<(String, Vec<f64>)> = vec![("作品を開く".into(), vec![open_ms])];
    let runs = 7;

    // 矩形で選ぶ（全部・左半分）
    for (label, rect) in [
        ("矩形で全部を選ぶ", bounds.inflate(10.0, 10.0)),
        (
            "矩形で左半分を選ぶ",
            kurbo::Rect::new(
                bounds.x0 - 10.0,
                bounds.y0 - 10.0,
                bounds.center().x,
                bounds.y1 + 10.0,
            ),
        ),
    ] {
        let mut times = Vec::new();
        for _ in 0..runs {
            let n = app.history.undo_stack.len();
            let t = Instant::now();
            apply(
                &mut app,
                Action::SelectMarquee {
                    p0: Point::new(rect.x0, rect.y0),
                    p1: Point::new(rect.x1, rect.y1),
                    additive: false,
                },
            );
            times.push(ms(t));
            assert!(canvas_tool::clear_selection(&mut app));
            undo_to(&mut app, n);
        }
        rows.push((label.into(), times));
    }

    // 全部を選んで動かして確定（確定＝選択を外す）
    {
        let mut times = Vec::new();
        for _ in 0..runs {
            let n = app.history.undo_stack.len();
            if !(apply(&mut app, Action::SelectAll)
                && apply(
                    &mut app,
                    Action::NudgeSelection {
                        delta: kurbo::Vec2::new(25.0, 0.0),
                    },
                ))
            {
                break;
            }
            let t = Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            times.push(ms(t));
            undo_to(&mut app, n);
        }
        rows.push(("全部を動かして確定".into(), times));
    }

    // コマ全体を同じ位置へ貼り付けて確定
    let copied = apply(&mut app, Action::SelectAll) && apply(&mut app, Action::Copy);
    assert!(canvas_tool::clear_selection(&mut app));
    {
        let mut times = Vec::new();
        for _ in 0..runs {
            let n = app.history.undo_stack.len();
            if !(copied && apply(&mut app, Action::Paste { in_place: true })) {
                break;
            }
            let t = Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            times.push(ms(t));
            undo_to(&mut app, n);
        }
        rows.push(("コマ全体を同じ位置へ貼り付けて確定".into(), times));
    }
    // その確定の内訳：レイヤーごとの計算（重ね合わせ・書き戻しの確かめ・何を書くか）を1つずつ測り、確定と同じく並列に
    // した計算（plan_commit）と、セル画へ書く所（write_commit）も測る
    let mut stages: Vec<(Vec<f64>, f64, f64)> = Vec::new();
    for _ in 0..runs {
        let n = app.history.undo_stack.len();
        if !(copied && apply(&mut app, Action::Paste { in_place: true })) {
            break;
        }
        let tab = app.tab_mut();
        tab.refresh_scene_base();
        let Some(fl) = tab.session.take_floating() else {
            break;
        };
        let scope = tab.session.editing_group_path().to_vec();
        let mut per_layer = Vec::new();
        for l in fl.layers() {
            let Some(cel) = tab.session.base_cache().cel(l.layer_id) else {
                continue;
            };
            let base = cel.scope_base(&scope);
            let t = Instant::now();
            if let Some(next) = l.committed_local(&base) {
                let _ = base.replace_local(&next, &scope);
            }
            per_layer.push(ms(t));
        }
        let t = Instant::now();
        let plan = canvas_tool::plan_commit(tab, &fl);
        let plan_ms = ms(t);
        let t = Instant::now();
        if let Some(plan) = plan {
            let _ = canvas_tool::write_commit(tab, plan);
        }
        let write_ms = ms(t);
        drop(fl);
        tab.session.retain_existing_selection();
        stages.push((per_layer, plan_ms, write_ms));
        undo_to(&mut app, n);
    }

    // コマ全体を別のコマへ貼り付けて確定
    if let Some(g) = paste_frame {
        goto(&mut app, g);
        let mut times = Vec::new();
        for _ in 0..runs {
            let n = app.history.undo_stack.len();
            if !(copied && apply(&mut app, Action::Paste { in_place: true })) {
                break;
            }
            let t = Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            times.push(ms(t));
            undo_to(&mut app, n);
        }
        rows.push((format!("コマ全体をコマ {} へ貼り付けて確定", g + 1), times));
        goto(&mut app, frame);
    }

    // 辺のつまみ・角のドラッグの確定（グループやシンボルの外の図形の、いちばん長い区間の真ん中と端）
    let candidates: Vec<(Point, Point)> = app
        .scene()
        .iter()
        .filter(|s| s.group_path.is_empty() && s.symbol_instance.is_none())
        .filter_map(|s| {
            s.exterior()
                .segments
                .iter()
                .max_by(|a, b| (a.p2 - a.p0).hypot().total_cmp(&(b.p2 - b.p0).hypot()))
                .map(|q| (q.eval(0.5), q.p0))
        })
        .take(50)
        .collect();
    for (label, edge) in [
        ("辺をつまんで動かして確定", true),
        ("角を動かして確定", false),
    ] {
        let mut times = Vec::new();
        for &(mid, corner) in &candidates {
            let from = if edge { mid } else { corner };
            let to = from + kurbo::Vec2::new(0.0, 15.0);
            let action = |from, to| if edge { Op::PullEdge(from, to) } else { Op::MoveVertex(from, to) };
            let n = app.history.undo_stack.len();
            let t = Instant::now();
            if !op(&mut app, action(from, to)) {
                continue;
            }
            times.push(ms(t));
            undo_to(&mut app, n);
            for _ in 1..runs {
                let t = Instant::now();
                if !op(&mut app, action(from, to)) {
                    break;
                }
                times.push(ms(t));
                undo_to(&mut app, n);
            }
            break;
        }
        rows.push((label.into(), times));
    }

    // ブラシ（コマの真ん中を横切る1本）
    {
        let c = bounds.center();
        let w = bounds.width() * 0.4;
        let points: Vec<Point> = (0..=100)
            .map(|i| {
                Point::new(
                    c.x - w + 2.0 * w * i as f64 / 100.0,
                    c.y + 30.0 * (i as f64 * 0.1).sin(),
                )
            })
            .collect();
        let mut times = Vec::new();
        for _ in 0..runs {
            let n = app.history.undo_stack.len();
            let t = Instant::now();
            if !apply(
                &mut app,
                Action::PaintStroke {
                    points: points.clone(),
                    radius: 8.0,
                    color: Color32::from_rgb(200, 40, 40),
                },
            ) {
                break;
            }
            times.push(ms(t));
            undo_to(&mut app, n);
        }
        rows.push(("ブラシ 1 本".into(), times));
    }

    // 開いたあと、全部のコマを1回ずつ表示する（初めて見るコマで地図を確かめる重さ）
    let mut sweep = Vec::with_capacity(total_frames);
    for f in 0..total_frames {
        let t = Instant::now();
        goto(&mut app, f);
        let _ = app.scene().len();
        sweep.push(ms(t));
    }
    // オニオンスキン前後 10 コマで、全部のコマを1コマずつスクラブする（1回の描画 `update_ui` と、egui が描いた形を
    // 三角形に分けるまで。地図は上で確かめ終わっている：レビュー f01d518 の N-5004）
    app.view.onion_skin_enabled = true;
    app.view.onion_prev_frames = 10;
    app.view.onion_next_frames = 10;
    let screen_rect = Some(eframe::egui::Rect::from_min_size(
        eframe::egui::Pos2::ZERO,
        eframe::egui::Vec2::new(1440.0, 900.0),
    ));
    let mut onion = Vec::with_capacity(total_frames);
    for f in 0..total_frames {
        goto(&mut app, f);
        let t = Instant::now();
        let out = ctx.run(
            eframe::egui::RawInput {
                screen_rect,
                ..Default::default()
            },
            |ctx| app.update_ui(ctx),
        );
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
        onion.push(ms(t));
    }
    app.view.onion_skin_enabled = false;
    onion.sort_by(f64::total_cmp);
    // 開き直して、裏で全部の地図を確かめ終わるまで（そのあいだ画面は待たない。アプリでは開いたときに始める）
    drop(app);
    let background_ms = {
        let mut fresh = Is2dApp::default();
        crate::project_io::open_project_in_tab(&mut fresh, std::path::Path::new(&path), Some(&ctx))
            .expect("作品を開けること");
        let t = Instant::now();
        let checker = fresh.doc.check_maps_in_background();
        checker
            .expect("スレッドを作れること")
            .join()
            .expect("確かめ終わること");
        ms(t)
    };
    let sweep_total: f64 = sweep.iter().sum();
    sweep.sort_by(f64::total_cmp);

    eprintln!();
    // 測ったマシンのチップと、M1 の見込みの倍率（時間の目標は Apple M1 で守る数字。倍率はレビューのきまりと同じ：
    // claude/review-prompts/is2d-review-claude-code.md の「目標の数字」。Mac 以外は ×1）
    let chip = measured_chip();
    let factor = m1_factor(&chip);
    eprintln!("作品：{}", path.display());
    eprintln!("測ったマシン：{chip}（M1 の見込み＝測った時間×{factor}）");
    eprintln!(
        "作品の中身：レイヤー {layers} 枚（シンボルの中を含む）・キーフレーム {keyframes} 個・地図 {maps} 枚・辺 {edges} 本"
    );
    let (sweep_mid, sweep_max) = (
        sweep.get(sweep.len() / 2).copied().unwrap_or(0.0),
        sweep.last().copied().unwrap_or(0.0),
    );
    eprintln!(
        "全部のコマ（{total_frames} コマ）を1回ずつ表示：合計 {sweep_total:.0}ms（M1 {:.0}ms）・1 コマの中央値 {sweep_mid:.1}ms（M1 {:.1}ms）・最大 {sweep_max:.1}ms（M1 {:.1}ms）",
        sweep_total * factor,
        sweep_mid * factor,
        sweep_max * factor
    );
    eprintln!(
        "開いたあと、裏で全部の地図を確かめ終わるまで：{background_ms:.0}ms（M1 {:.0}ms。そのあいだ画面は待たない）",
        background_ms * factor
    );
    let pick = |v: &[f64], q: usize| v[(v.len() - 1) * q / 100];
    eprintln!(
        "オニオンスキン前後 10 コマで全部のコマをスクラブ（1コマの描画と三角形に分けるまで）：中央値 {:.1}ms（M1 {:.1}ms）・p99 {:.1}ms（M1 {:.1}ms）・最大 {:.1}ms（M1 {:.1}ms）",
        pick(&onion, 50),
        pick(&onion, 50) * factor,
        pick(&onion, 99),
        pick(&onion, 99) * factor,
        pick(&onion, 100),
        pick(&onion, 100) * factor
    );
    if !stages.is_empty() {
        let mid = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let layers_n = stages[0].0.len();
        let per_layer: Vec<String> = (0..layers_n)
            .map(|k| {
                let t = mid(stages.iter().filter_map(|s| s.0.get(k).copied()).collect());
                format!("{t:.1}ms（M1 {:.1}ms）", t * factor)
            })
            .collect();
        let plan = mid(stages.iter().map(|s| s.1).collect());
        let write = mid(stages.iter().map(|s| s.2).collect());
        eprintln!(
            "コマ全体を同じ位置へ貼り付けて確定の内訳（中央値）：レイヤーごとの計算を1つずつ [{}]・並列にした計算 {plan:.1}ms（M1 {:.1}ms）・セル画へ書く {write:.1}ms（M1 {:.1}ms）",
            per_layer.join("・"),
            plan * factor,
            write * factor
        );
    }
    eprintln!("コマ {}（図形 {shapes} 個・区間 {segments} 個）", frame + 1);
    eprintln!("| 操作 | 中央値 | 最大 | M1 の見込み（中央値／最大） | 回数 |");
    eprintln!("|---|---:|---:|---:|---:|");
    for (label, mut times) in rows {
        if times.is_empty() {
            eprintln!("| {label} | できなかった | | | 0 |");
            continue;
        }
        times.sort_by(f64::total_cmp);
        let (mid, max) = (times[times.len() / 2], times[times.len() - 1]);
        eprintln!(
            "| {label} | {mid:.1}ms | {max:.1}ms | {:.1}／{:.1}ms | {} |",
            mid * factor,
            max * factor,
            times.len()
        );
    }
}

/// 計測：測ったマシンのチップ（Mac は `sysctl -n machdep.cpu.brand_string`、Linux は /proc/cpuinfo の model name）
fn measured_chip() -> String {
    let mac = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty());
    mac.or_else(|| {
        std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|c| {
            c.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_owned())
        })
    })
    .unwrap_or_else(|| "不明".to_owned())
}

/// シンボルの多い作品で、シンボルの中の操作と画面の1コマを測る（`cargo test --release measure_symbol_heavy -- --ignored
/// --nocapture`。全体の点検 2026-11-27：ライブラリの大きさで重くなる所を、直す前と後で比べる）。作品は、600 コマの
/// シンボルを1つ作り、それを複製してシンボルを `IS2D_SYMBOLS`（既定 300）個にし、ルートに置く
#[test]
#[ignore]
fn measure_symbol_heavy() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use std::time::Instant;
    let count: usize = std::env::var("IS2D_SYMBOLS").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let mut app = Is2dApp::default();
    let stroke = |app: &mut Is2dApp, i: usize| {
        let y = 300.0 + (i % 30) as f64 * 8.0;
        assert!(apply(
            app,
            Action::PaintStroke { points: vec![Point::new(800.0, y), Point::new(1100.0, y + 30.0)], radius: 5.0, color: Color32::RED },
        ));
    };
    stroke(&mut app, 0);
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let long = symbols_in_order(app.tab().doc().library()).next().unwrap().id;
    assert!(apply(&mut app, Action::EnterSymbolFromLibrary { sym_id: long }));
    for i in 1..600 {
        assert!(apply(&mut app, Action::InsertKeyframe { copy_prev: false }));
        stroke(&mut app, i);
    }
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    for _ in 1..count {
        assert!(apply(&mut app, Action::DuplicateSymbol { sym_id: long }));
    }
    eprintln!("シンボル {} 個（1つは 600 コマ）", app.tab().doc().library().len());
    let ctx = egui::Context::default();
    let frame = |app: &mut Is2dApp| {
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0)));
        let t = Instant::now();
        let _ = ctx.run(egui::RawInput { screen_rect, ..Default::default() }, |ctx| app.update_ui(ctx));
        ms(t)
    };
    // シンボルの中に入り、ライブラリのタブを出して、画面の1コマ
    assert!(apply(&mut app, Action::EnterSymbolFromLibrary { sym_id: long }));
    app.right_panel_tab = crate::panels::RightPanelTab::Library;
    for _ in 0..5 {
        frame(&mut app);
    }
    let frames: Vec<f64> = (0..30).map(|_| frame(&mut app)).collect();
    eprintln!("シンボルの中・ライブラリのタブ：画面の1コマ 中央 {:.2} ms", median(frames));
    // シンボルの中で1本描く（取り消して戻す）
    let mut draws = Vec::new();
    for i in 0..9 {
        let t = Instant::now();
        stroke(&mut app, 100 + i);
        draws.push(ms(t));
        assert!(apply(&mut app, Action::Undo));
    }
    eprintln!("シンボルの中で1本描く：中央 {:.2} ms", median(draws));
    // シンボルの中で取り消す
    let mut undos = Vec::new();
    for i in 0..9 {
        stroke(&mut app, 200 + i);
        let t = Instant::now();
        assert!(apply(&mut app, Action::Undo));
        undos.push(ms(t));
    }
    eprintln!("シンボルの中で取り消す：中央 {:.2} ms", median(undos));
    // シンボルを複製する
    let mut dups = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        assert!(apply(&mut app, Action::DuplicateSymbol { sym_id: long }));
        dups.push(ms(t));
    }
    eprintln!("シンボルを複製する：中央 {:.2} ms", median(dups));
}

/// 入れ子の深いシンボル（8段・各段 `IS2D_WIDTH`（既定 3）個のシンボルが、次の段のシンボルを全部使う）を別のタブへ2回
/// 貼る時間を測る（`cargo test --release measure_deep_nested_paste -- --ignored --nocapture`。2回目は、前に移したシンボルを
/// 使い回せるかを確かめる：全体の点検 2026-11-27）
#[test]
#[ignore]
fn measure_deep_nested_paste() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::symbol::SymbolDef;
    use crate::timeline::SymbolInstance;
    use crate::symbol_binding::SymbolInstanceBinding;
    use std::time::Instant;
    let width: u64 = std::env::var("IS2D_WIDTH").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let levels = 7u64;
    let mut app = Is2dApp::default();
    let id = |level: u64, k: u64| 1 + level * width + k;
    let mut gid = 1000u64;
    let mut inst = |sym: u64| {
        gid += 1;
        SymbolInstance { binding: SymbolInstanceBinding::new(gid, sym, kurbo::Affine::IDENTITY), layer_id: 1, parent_scope: Vec::new(), order: 0 }
    };
    for level in (0..levels).rev() {
        for k in 0..width {
            // 絵は持たない（貼り付けの判断の手間だけを測る。絵があると、入れ子の数だけ並ぶ絵を作る手間が乗る）
            let mut def = SymbolDef::new(id(level, k), format!("L{level}-{k}"), Vec::new()).unwrap();
            if level + 1 < levels {
                let children: Vec<SymbolInstance> = (0..width).map(|c| inst(id(level + 1, c))).collect();
                def.timeline.layers[0].keyframe_mut(0).cel = crate::timeline::Cel::new_with_instances(Vec::new(), children);
            }
            app.doc.add_symbol(None, def);
        }
    }
    app.doc.next_symbol_id = crate::symbol::Counter::starting_at(id(levels, 0) + 1);
    let top = inst(id(0, 0));
    app.doc.next_group_id = crate::symbol::Counter::starting_at(gid + 1);
    app.doc.root.layers[0].keyframe_mut(0).cel = crate::timeline::Cel::new_with_instances(Vec::new(), vec![top]);
    app.refresh_scene_base();
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));
    let ctx = egui::Context::default();
    crate::project_io::new_project(&mut app, &ctx);
    for round in 1..=3 {
        let t = Instant::now();
        assert!(apply(&mut app, Action::Paste { in_place: true }));
        eprintln!("貼り付け {round} 回目：{:.2} ms（シンボル {} 個）", t.elapsed().as_secs_f64() * 1e3, app.tab().doc().library().len());
    }
}

/// 復旧の書き足すファイルを、レビューの作品で測る（`IS2D_WORK=<作品> cargo test --release measure_recovery_journal --
/// --ignored --nocapture`）：全体を書く時間と大きさ、1本描くたびに書き足す時間と大きさ、保存した印を書く時間
#[test]
#[ignore]
fn measure_recovery_journal() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::recovery_journal::Journal;
    use std::time::Instant;
    let Ok(path) = std::env::var("IS2D_WORK") else {
        eprintln!("IS2D_WORK に作品のパスを入れて実行してください");
        return;
    };
    let path = work_file_in_current_format(std::path::Path::new(&path));
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let ctx = egui::Context::default();
    let mut app = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut app, std::path::Path::new(&path), Some(&ctx)).expect("作品を開く");
    let dir = std::env::temp_dir().join(format!("is2d_measure_journal_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let jpath = dir.join("work.is2dj");
    let size = || std::fs::metadata(&jpath).unwrap().len();
    let doc = app.tab().committed_document_copy().unwrap();
    let t = Instant::now();
    let mut journal = Journal::create(&jpath, &doc, Some(&path)).unwrap();
    eprintln!("全体を書く: {:.1} ms, {:.1} MB", ms(t), size() as f64 / 1e6);
    let total = app.tab().active_timeline().total_frames.max(1);
    let mut times = Vec::new();
    let mut bytes = Vec::new();
    for i in 0..20 {
        let frame = (i * 97) % total;
        assert!(canvas_tool::clear_selection(&mut app));
        apply(&mut app, Action::ScrubRulerToFrame { frame });
        let y = 300.0 + i as f64 * 10.0;
        assert!(apply(
            &mut app,
            Action::PaintStroke { points: vec![Point::new(400.0, y), Point::new(700.0, y + 40.0)], radius: 6.0, color: Color32::RED },
        ));
        let before = size();
        let doc = app.tab().committed_document_copy().unwrap();
        let t = Instant::now();
        journal.write(&doc, Some(&path)).unwrap();
        times.push(ms(t));
        bytes.push(size().saturating_sub(before));
    }
    times.sort_by(f64::total_cmp);
    bytes.sort_unstable();
    eprintln!(
        "1本描いて書き足す（20回）: 中央 {:.2} ms・最大 {:.2} ms, 中央 {:.1} KB・最大 {:.1} KB",
        times[10],
        times[19],
        bytes[10] as f64 / 1e3,
        bytes[19] as f64 / 1e3
    );
    let t = Instant::now();
    journal.mark_saved().unwrap();
    eprintln!("保存した印: {:.2} ms", ms(t));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 段0の計測（claude/触った所だけを計算する地図の設計_22fbbbe.md）：セル画の辺の数を変えて、同じ場所での同じ編集の
/// 時間を測る。同じ中身の区画（辺 約 1,500 本）を、触れ合わないように 1・4・16・64 個並べたセル画を作り、どの大きさでも
/// 左上の区画の中で、同じ乱数で同じ編集をする（触る所は同じで、セル画全体の大きさだけが違う）。編集は入口
/// （`doc_actions::apply`）から文書への書き戻しまで。ブラシは手順ごとの内訳も測る。
/// `cargo test --release measure_edit_scaling_by_cel_size -- --ignored --nocapture`（速さだけを見るなら
/// `IS2D_NO_CHECKS=1` を付ける：テストだけの確かめを外す）
#[test]
#[ignore]
fn measure_edit_scaling_by_cel_size() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::{reshape_result, LayerBase, MapInput, MapRule, PlanarMap, ReshapeEdit};
    use std::time::Instant;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
    let med = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    // 1つの区画：(-150, -75) から 300×150 の所に、辺が 1,500 本を超えるまで線を描く
    let area = (-150.0, -75.0, 300.0, 150.0);
    let mut rng = map_overlay::Rng(5);
    let mut block = PlanarMap::default();
    let mut k = 0;
    while block.edges.len() < 1_500 {
        let (r, c) = (2.0 + rng.f() * 8.0, colors[k % 4]);
        let s = wiggly_stroke(&mut rng, area, r, c);
        block = block
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
            .unwrap();
        k += 1;
    }
    let block = LayerBase::new(&block).map().clone();
    let bb = block.bounding_box().unwrap();
    let (dx, dy) = (bb.width() + 20.0, bb.height() + 20.0);
    eprintln!(
        "区画：辺 {} 本・頂点 {} 個（線 {k} 本）",
        block.edges.len(),
        block.nodes.len()
    );
    eprintln!(
        "{:>7} | {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>8} {:>7} {:>7} {:>7} {:>7}",
        "辺",
        "ブラシ",
        "消しゴム",
        "バケツ",
        "アンカー+",
        "アンカー-",
        "引っぱり",
        "頂点",
        "矩形",
        "片を置く",
        "ドラッグ中",
        "取り消し",
        "取消最大",
        "土台",
        "描いた後1コマ"
    );
    for side in [1usize, 2, 4, 8] {
        // 区画を side × side 個並べた地図
        let mut big = PlanarMap::default();
        for i in 0..side {
            for j in 0..side {
                let off = kurbo::Vec2::new(i as f64 * dx, j as f64 * dy);
                let n = big.nodes.len();
                big.nodes.extend(block.nodes.iter().map(|&p| p + off));
                big.edges
                    .extend(block.edges.iter().map(|e| crate::planar_map::MapEdge {
                        a: e.a + n,
                        b: e.b + n,
                        ctrl: e.ctrl + off,
                        ..*e
                    }));
            }
        }
        let big = big.checked().unwrap().freshly_labeled().into_map();
        let shapes: Vec<FillShape> = LayerBase::new(&big)
            .faces(&[])
            .iter()
            .map(|s| (**s).clone())
            .collect();
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, shapes);
        let lid = app.doc.root.layers[0].id;
        // 画面の1コマ（描画の準備まで：図形の塗りの三角形分割・描く命令を作る。GPU に送る所は含まない）
        let ctx = egui::Context::default();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        // 地図全体が画面に入る倍率（いちばん重い：毎コマ全部の図形を描く）
        {
            let (w, h) = (side as f64 * dx, side as f64 * dy);
            let zoom = (1100.0 / w).min(700.0 / h).min(1.0) as f32;
            let view = &mut app.tab_mut().view;
            view.set_zoom(zoom);
            let zoom = view.zoom();
            view.pan = egui::vec2(-(bb.x0 as f32) * zoom + 40.0, -(bb.y0 as f32) * zoom + 60.0);
        }
        for _ in 0..2 {
            let _ = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
        }
        let edges = app.doc.root.layers[0].keyframes[0]
            .cel
            .scope_base(&[])
            .map()
            .edges
            .len();
        // どの大きさでも同じ乱数（左上の区画の中で、同じ編集）
        let mut r = map_overlay::Rng(11);
        let pt = |r: &mut map_overlay::Rng| {
            Point::new(
                area.0 + 30.0 + r.f() * (area.2 - 60.0),
                area.1 + 20.0 + r.f() * (area.3 - 40.0),
            )
        };
        let stroke = |r: &mut map_overlay::Rng, p: Point| -> Vec<Point> {
            let mut p = p;
            let mut a = r.f() * std::f64::consts::TAU;
            (0..20)
                .map(|_| {
                    a += (r.f() - 0.5) * 0.8;
                    p += kurbo::Vec2::new(a.cos(), a.sin()) * 3.0;
                    p
                })
                .collect()
        };
        let pick_edge = |r: &mut map_overlay::Rng| {
            block.quad((r.f() * block.edges.len() as f64) as usize % block.edges.len())
        };
        let mut t: [Vec<f64>; 13] = Default::default();
        for _ in 0..9 {
            // 1. ブラシ
            let p0 = pt(&mut r);
            let points = stroke(&mut r, p0);
            let s = Instant::now();
            assert!(apply(
                &mut app,
                Action::PaintStroke {
                    points: points.clone(),
                    radius: 4.0,
                    color: colors[0]
                }
            ));
            t[0].push(ms(s));
            // ブラシのあとの1コマ（画面を組み立て、egui が描く三角形にまとめるまで）
            let s = Instant::now();
            let out = ctx.run(raw_input.clone(), |ctx| app.update_ui(ctx));
            let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
            t[12].push(ms(s));
            // 取り消し（画面の土台を作り直すまで）
            let s = Instant::now();
            assert!(apply(&mut app, Action::Undo));
            t[11].push(ms(s));
            // 2. 消しゴム
            let p0 = pt(&mut r);
            let ep = stroke(&mut r, p0);
            let s = Instant::now();
            let erased = apply(
                &mut app,
                Action::EraseStroke {
                    points: ep,
                    radius: 4.0,
                },
            );
            t[1].push(ms(s));
            if erased {
                assert!(apply(&mut app, Action::Undo));
            }
            // 3. バケツ
            let bp = pt(&mut r);
            let s = Instant::now();
            let filled = apply(
                &mut app,
                Action::BucketFill {
                    pt: bp,
                    color: colors[3],
                },
            );
            t[2].push(ms(s));
            if filled {
                assert!(apply(&mut app, Action::Undo));
            }
            // 4・5. アンカーを足して、同じ所を消す
            let q = pick_edge(&mut r);
            let ap = q.eval(0.5);
            let s = Instant::now();
            let added = apply(&mut app, Action::AddAnchor { pt: ap });
            t[3].push(ms(s));
            if added {
                let s = Instant::now();
                let deleted = apply(&mut app, Action::DeleteAnchor { pt: ap });
                t[4].push(ms(s));
                if deleted {
                    assert!(apply(&mut app, Action::Undo));
                }
                assert!(apply(&mut app, Action::Undo));
            }
            // 6. 辺の引っぱり
            let q = pick_edge(&mut r);
            let from = q.eval(0.5);
            let s = Instant::now();
            let pulled = pull_edge(&mut app, from, from + kurbo::Vec2::new(3.0, 2.0));
            t[5].push(ms(s));
            if pulled {
                assert!(apply(&mut app, Action::Undo));
            }
            // 7. 頂点の移動
            let q = pick_edge(&mut r);
            let s = Instant::now();
            let moved = move_vertex(&mut app, q.p0, q.p0 + kurbo::Vec2::new(2.0, 1.0));
            t[6].push(ms(s));
            if moved {
                assert!(apply(&mut app, Action::Undo));
            }
            // 8・9. 矩形で切って、少し動かして置く
            let m0 = pt(&mut r);
            let s = Instant::now();
            apply(
                &mut app,
                Action::SelectMarquee {
                    p0: m0,
                    p1: m0 + kurbo::Vec2::new(40.0, 30.0),
                    additive: false,
                },
            );
            t[7].push(ms(s));
            let moved = apply(
                &mut app,
                Action::NudgeSelection {
                    delta: KVec2::new(5.0, 4.0),
                },
            );
            let s = Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            t[8].push(ms(s));
            if moved {
                assert!(apply(&mut app, Action::Undo));
                assert!(canvas_tool::clear_selection(&mut app));
            }
            // 10. 輪郭の変形のドラッグ中の1コマ（離したときと同じ計算：`reshape_result`）
            let cel = app.doc.root.layers[0].keyframes[0].cel.clone();
            let base = cel.scope_base(&[]);
            let q = pick_edge(&mut r);
            let s = Instant::now();
            std::hint::black_box(reshape_result(
                &base,
                ReshapeEdit::PullEdge {
                    orig_seg: q,
                    grab_t: 0.5,
                    target: q.eval(0.5) + kurbo::Vec2::new(4.0, 3.0),
                    follow: true,
                },
                lid,
                &[],
                Affine::IDENTITY,
            ));
            t[9].push(ms(s));
            // 画面の土台の作り直し（レイヤーの図形を取り出し直す）
            let s = Instant::now();
            crate::canvas_tool::finish_cel_writes(app.tab_mut(), &[lid]);
            t[10].push(ms(s));
        }
        let undo_max = t[11].iter().copied().fold(0.0, f64::max);
        let m: Vec<f64> = t
            .into_iter()
            .map(|v| if v.is_empty() { f64::NAN } else { med(v) })
            .collect();
        eprintln!(
            "{edges:>7} | {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>8.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2}",
            m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8], m[9], m[11], undo_max, m[10], m[12]
        );
    }
}
