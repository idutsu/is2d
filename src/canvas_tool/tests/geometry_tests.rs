//! 地図と形（重ね合わせ・輪郭・線の形・地図の決まり）のテスト

use super::*;

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

    let red_count = canvas.iter().filter(|s| s.color() == red).count();
    let blue_count = canvas.iter().filter(|s| s.color() == blue).count();
    assert_eq!(red_count, 2, "赤シェイプが上下2つに分断されること");
    assert_eq!(blue_count, 1, "青シェイプが1つ追加されること");
}

/// R6：手で描いたような乱数の線（揺れ・折り返し・止まる・自分との交わり）を、いろいろな太さと表示の倍率で 2,000 本
/// 描き、どの線でも次が成り立つ：
/// - 中心線の節が、マウスの点の丸めの範囲に入る
/// - 離したあとの形が、中心線を半径で太らせた形と、ずれの上限（`STROKE_DEV_PX`）の中で一致する（輪郭のどの点も、
///   中心線からの距離が半径±上限。中心線の近くに余計な穴が無く、外に余計な塗りも無い）
/// - どのマウスの点も、どの中心線の点も塗られる（丸めの範囲と上限の中で）
///
/// また、そのうち 200 本を 20 本ずつ同じ所に重ねて描き（ブラシ 3 色と消しゴム）、地図が正しいままであること
/// レビュー d91cd4c の N-2001：速さが 0 に近い鋭い折り返しの先が欠けない。乱数の手描きの線で、種を変えると形が中心線から
/// ずれた線（直す前はどれも失敗：折り返しの先の外へ回るつなぎ目を、円弧でなく中心を通る折り返しにしていたので、丸い端が
/// 欠けて切れ込みができた）と、ちょうど逆向きに折り返す点のような線（外へ回るかを外積の符号だけで見ると、どちらの側も外に
/// ならず、てっぺんが平らに欠ける）
#[test]
fn sharp_turn_tips_are_not_notched() {
    let zooms = [0.25, 0.5, 1.0, 2.0, 4.0];
    for (seed, line) in [(12345u64, 1658usize), (987654321, 950), (987654321, 1456), (0x1111_2222_3333, 997), (0x1111_2222_3333, 1153), (0x4444_5555_6666, 937), (0x4444_5555_6666, 466)] {
        let mut rng = fastrand::Rng::with_seed(seed);
        for k in 0..=line {
            let zoom = zooms[rng.usize(0..zooms.len())];
            let pts = random_hand_stroke(&mut rng, zoom);
            let r_screen = 0.5 + 40.0 * rng.f64().powi(2);
            if k == line {
                let radius = (r_screen / zoom).clamp(0.5, 70.0);
                let c = check_stroke_follows_centerline(&pts, radius, zoom);
                assert!(
                    c.worst_px <= STROKE_DEV_PX && c.outside_box + c.uncovered + c.missing == 0,
                    "種 {seed:#x} の線 {line}（{} 点・半径 {radius:.2}・倍率 {zoom}）：{c:?}",
                    pts.len()
                );
            }
        }
    }
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

        let a_raw = brush_line(&a_pts, r_a);
        let a_shapes = brush_line_shapes(&a_raw, Color32::BLACK);
        assert_eq!(a_shapes.len(), 1);
        let a_shape = &a_shapes[0];

        let tip_apex = tip_start - dir_down_left * (r_a * 0.80);
        assert!(a_shape.contains_point(tip_apex));

        let valley_outside_probe = Point::new(cross_pt.x + r_a + 3.2, cross_pt.y - 5.5);
        assert!(!a_shape.contains_point(valley_outside_probe));
    }
}

#[test]
fn test_g1_1_writeback_preserves_unexpanded_and_orphan_elements() {
    use crate::canvas_tool::doc_actions::Action;

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
        .set_shapes_and_instances(Vec::new(), instances);
    app.refresh_scene_base();
    assert_eq!(app.session.base_cache().shapes().len(), 12_000);
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
        cel.instances().len(),
        120,
        "ブラシで1回描いても文書のインスタンスは120個のまま残ること"
    );
    assert_eq!(
        cel.shapes().len(),
        1,
        "描いたブラシ図形が保存されていること"
    );

    // 保存して読み込んでも120個のインスタンスが残ること
    let tmp_path = std::env::temp_dir().join(format!("is2d_m2_3_test_{}.is2d", std::process::id()));
    crate::project_io::save_project_to_path(&mut app, &tmp_path).unwrap();
    let mut reopened = Is2dApp::default();
    crate::project_io::open_project_in_tab(&mut reopened, &tmp_path, None).unwrap();
    let _ = std::fs::remove_file(&tmp_path);
    assert_eq!(
        reopened.doc.root.layers[0].keyframes[0].cel.instances().len(),
        120,
        "保存して読み込んでもインスタンスが120個残ること"
    );
    reopened.refresh_scene_base();
    assert_eq!(reopened.session.base_cache().shapes().len(), 12_001);
}

#[test]
fn test_g6_2_writeback_copies_only_geometry_caches_and_no_screen_caches() {
    use crate::canvas_tool::doc_actions::{apply, Action};

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

    // ステージ側（SceneBaseCache）の図形1に fill_mesh, hit_bez_path を生成させる（選択表示は塗りのメッシュにテクスチャを
    // 貼るだけで、図形に写しを持たない）
    {
        let stage_s1 = app.scene().get(0).unwrap();
        let _ = stage_s1.fill_mesh();
        assert!(stage_s1.contains_point(Point::new(140.0, 100.0)));
        let _ = crate::screen_mesh::shape_screen_mesh(stage_s1, 1.0, Vec2::ZERO, false);
        let _ = crate::screen_mesh::shape_selection_screen_mesh(stage_s1, 1.0, Vec2::ZERO);
        assert!(stage_s1.has_cached_fill_mesh());
        assert!(stage_s1.has_cached_hit_bez_path());
    }

    // 2. 離れた位置に図形2を描画してセル画の書き込みを発生させる
    //    未変更の図形1（kept パス）も含め、文書側の図形には fill_mesh も hit_bez_path も引き継がれないこと
    let _ = apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(400.0, 300.0), Point::new(480.0, 300.0)],
            radius: 10.0,
            color: Color32::BLUE,
        },
    );
    let doc_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
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

    // 3. 図形をグループ化して選択表示・移動（変更図形の local_s 書き戻しパス）を行ったあとも、文書側の図形が描画用メッシュを持たないこと
    app.refresh_scene_base();
    for s in app.scene().iter() {
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.25, Vec2::new(10.0, 20.0), false);
        let _ = crate::screen_mesh::shape_selection_screen_mesh(s, 1.25, Vec2::new(10.0, 20.0));
    }
    let all_ids: Vec<_> = app.scene().iter().cloned().collect();
    for s in &all_ids {
        { let mut sel = app.session.selection().clone(); sel.insert_shape(s); app.session.replace_selection(sel); }
    }
    let _ = apply(&mut app, Action::Group);
    for s in app.scene().iter() {
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.25, Vec2::new(10.0, 20.0), false);
        let _ = crate::screen_mesh::shape_selection_screen_mesh(s, 1.25, Vec2::new(10.0, 20.0));
    }
    let _ = apply(
        &mut app,
        Action::NudgeSelection {
            delta: kurbo::Vec2::new(15.0, -10.0),
        },
    );
    for s in app.doc.root.layers[0].keyframes[0].cel.shapes().iter() {
        assert!(
            !s.has_cached_fill_mesh(),
            "グループ移動後の書き戻しでも、文書側の図形が描画用メッシュを持たないこと"
        );
    }

    // 4. シンボル変換後も、シンボル定義内の文書図形が描画用メッシュを持たないこと
    for s in app.scene().iter() {
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.0, Vec2::ZERO, false);
        let _ = crate::screen_mesh::shape_selection_screen_mesh(s, 1.0, Vec2::ZERO);
    }
    let _ = apply(&mut app, Action::ConvertToSymbol);
    assert_eq!(app.doc.library().len(), 1);
    for s in app.doc.library().at(0).unwrap().timeline.layers[0].keyframes[0]
        .cel
        .shapes()
        .iter()
    {
        assert!(
            !s.has_cached_fill_mesh(),
            "シンボル定義内の文書図形も描画用メッシュを持たないこと"
        );
    }
}

/// テッセレーション生成された Mesh（fill_mesh および直接輪郭メッシュ）の vertices / indices の
/// 確保容量（capacity）が実データ長（len）に切り詰められていることを検証する
#[test]
fn test_tessellate_lyon_path_shrinks_mesh_capacity_to_len() {
    let raw = brush_line_at_zoom(
        &[
            Point::new(100.0, 100.0),
            Point::new(140.0, 115.0),
            Point::new(180.0, 105.0),
        ],
        6.0,
        1.0,
    );
    let shapes = brush_line_shapes_at_zoom(&raw, Color32::RED, 1.0);
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

    let cl = vector_stroke::centerline(&raw.pts, 1.0);
    for outline in vector_stroke::stroke_preview_outline(&cl, 0, cl.len() - 1, raw.radius, 1.0) {
        let direct_mesh = vector_engine::tessellate_raw_polygon(&outline, Color32::BLUE);
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
}

/// M4-5 (N-1001): 穴の多い図形の一部の穴にだけ触れる線を1本描いたとき、
/// 触れていない穴の `Contour` の線分が、描く前と同じ `Arc<Vec<QuadBez>>` を指していること
#[test]
fn test_m4_5_boolean_shares_untouched_hole_contour_arcs() {
    use crate::canvas_tool::doc_actions::{apply, Action};
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

    let before_shapes = app.doc.root.layers[0].keyframes[0].cel.shapes().clone();
    let total_holes_before: usize = before_shapes.iter().map(|s| s.holes().len()).sum();
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
        .flat_map(|s| s.holes())
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

    let after_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
    for prev_arc in &untouched_hole_arcs {
        let shared = after_shapes
            .iter()
            .flat_map(|s| s.holes())
            .any(|h| std::sync::Arc::ptr_eq(&h.segments, prev_arc));
        assert!(
            shared,
            "触れていない穴の Contour は描く前と同じ Arc<Vec<QuadBez>> を共有していること"
        );
    }
}

#[test]
fn test_m5_2_n1001_contour_segments_capacity_equals_len_after_merge_insert_and_reload() {
    fn assert_all_contours_shrunk(doc: &symbol::Document, phase: &str) {
        let mut count = 0usize;
        for layer in &doc.root.layers {
            for kf in &layer.keyframes {
                for sh in kf.cel.shapes().iter() {
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
        for sym in symbols_in_order(doc.library()) {
            for layer in &sym.timeline.layers {
                for kf in &layer.keyframes {
                    for sh in kf.cel.shapes().iter() {
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
        .exterior()
        .segments[0]
        .eval(0.5);
    assert!(doc_actions::apply(
        &mut app,
        doc_actions::Action::AddAnchor { pt: edge_pt },
    ));
    assert_all_contours_shrunk(&app.tab().doc, "after AddAnchor");

    let loaded_doc = saved_project_of(&app);
    let mut reopened = Is2dApp::default();
    loaded_doc.apply_to_app(&mut reopened);
    assert_all_contours_shrunk(&reopened.tab().doc, "after save and reopen");
}

#[test]
fn test_merge_self_overlapping_batch_never_produces_duplicate_shape_ids() {
    use std::collections::HashSet;

    use crate::canvas_tool::doc_actions::{apply, Action};

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
            merge_shapes_into_canvas(&mut canvas, incoming);
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
        merge_shapes_into_canvas(&mut canvas, dots);
        put_cel_shapes(&mut app, 0, 0, canvas);

        let before_centers: Vec<Point> = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| s.bounding_box().center())
            .collect();
        assert_eq!(before_centers.len(), 3);

        let target_id = app.doc.root.layers[0].keyframes[0].cel.shapes()[0].shape_id;
        app.session.replace_selection(crate::canvas_tool::Selection::default());
        app.session.add_to_selection([target_id], std::iter::empty());
        assert!(apply(
            &mut app,
            Action::NudgeSelection {
                delta: kurbo::Vec2::new(0.0, 30.0),
            },
        ));
        assert!(apply(&mut app, Action::Deselect));

        let after_shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
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
            if app.doc.root.layers[0].keyframes[0].cel.shapes().len() > 25 {
                assert!(apply(&mut app, Action::ClearActiveFrame));
            }
            if app.doc.root.layers[0].keyframes[0].cel.shapes().len() < 3 || rng.bool() {
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
                    .shapes()
                    .iter()
                    .map(|s| s.shape_id)
                    .collect();
                app.session.replace_selection(crate::canvas_tool::Selection::default());
                for &sid in &shape_ids {
                    if rng.bool() {
                        app.session.add_to_selection([sid], std::iter::empty());
                    }
                }
                if app.session.selection().shapes.is_empty() {
                    app.session.add_to_selection([shape_ids[0]], std::iter::empty());
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

            let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
            let unique_ids: HashSet<u64> = shapes.iter().map(|s| s.shape_id).collect();
            assert_eq!(
                unique_ids.len(),
                shapes.len(),
                "step {step}: セルの図形に重複する shape_id が発生しないこと"
            );
        }
    }
}

/// 両側の色が同じ頂点で2本の辺を1本にまとめるとき、ほぼ一直線の2本でも、端を越えて折り返す曲線にしないこと
#[test]
fn test_merge_degree2_does_not_fold_nearly_straight_pair() {
    use crate::planar_map::{MapEdge, PlanarMap};
    // 実際の地図で折り返した2本（どちらも直線で、ほぼ一直線）
    let a = Point::new(296.526581476974, 305.3533702973108);
    let v = Point::new(299.65683216934156, 294.0170251015405);
    let b = Point::new(310.28849425127237, 255.5139719986355);
    let edge = |from: usize, to: usize, ctrl: Point| MapEdge {
        a: from,
        b: to,
        ctrl,
        left: Some(Color32::RED),
        right: Some(Color32::BLUE),
        left_src: None,
        right_src: None,
        stroke: None,
        line_id: None,
    };
    let mut map = PlanarMap {
        nodes: vec![a, v, b],
        edges: vec![
            edge(0, 1, Point::new(298.09170660744206, 299.68519765994796)),
            edge(1, 2, Point::new(304.9726635572615, 274.7654969283734)),
        ],
    };
    let n = map.nodes.len();
    map.merge_degree2(0..n, None);
    let hull = kurbo::Rect::from_points(a, b)
        .union_pt(v)
        .inflate(0.05, 0.05);
    for e in 0..map.edges.len() {
        let bb = kurbo::Shape::bounding_box(&map.quad(e));
        assert!(
            hull.contains(Point::new(bb.x0, bb.y0)) && hull.contains(Point::new(bb.x1, bb.y1)),
            "まとめた曲線が元の2本の範囲から出ない：{bb:?}"
        );
    }
}

/// 図形の番号をみな新しくするとき（キーフレームを写したとき：`Cel::renew_shape_ids`）は、同じ番号は同じ新しい番号に、
/// 違う番号は違う新しい番号になる（洗い出しの 11。しまった地図の辺の番号を直接替える：`StoredMap::relabeled`）
#[test]
fn renewing_labels_keeps_equal_labels_equal_and_distinct_labels_distinct() {
    let r = |x: f64, c: Color32| {
        let mut s =
            vector_engine::make_marquee_rect_shape(kurbo::Rect::new(x, 0.0, x + 10.0, 10.0));
        s.recolor(|_| c);
        s
    };
    let mut cel = crate::timeline::Cel::from_shapes_unwrap(vec![
        r(0.0, Color32::RED),
        r(20.0, Color32::BLUE),
    ]);
    let old: Vec<_> = cel.maps()[0]
        .dense()
        .edges
        .iter()
        .flat_map(|e| [e.left_src, e.right_src])
        .collect();
    cel.renew_shape_ids(&mut Default::default());
    let renewed = cel.maps()[0].dense();
    let new: Vec<_> = renewed
        .edges
        .iter()
        .flat_map(|e| [e.left_src, e.right_src])
        .collect();
    assert_eq!(old.len(), new.len());
    let mut pairs = std::collections::HashMap::new();
    for (o, n) in old.iter().zip(&new) {
        assert_eq!(o.is_some(), n.is_some());
        if let (Some(o), Some(n)) = (o, n) {
            assert_ne!(o, n);
            assert_eq!(*pairs.entry(*o).or_insert(*n), *n);
        }
    }
    let distinct_old: std::collections::HashSet<_> = pairs.keys().collect();
    let distinct_new: std::collections::HashSet<_> = pairs.values().collect();
    assert_eq!(distinct_old.len(), distinct_new.len());
    assert_eq!(distinct_old.len(), 2);
}

/// 地図の書き戻し（`Cel::put_local`）は、書くものを作ったときの地図と今の地図が違えば（先に作った計画が古い）、何も変えずに
/// 断る。直す前は、呼ぶ側が同じ地図から作ったことに頼り、古い計画をそのまま書いて、間の編集を黙って消した：約束の棚卸し
/// 5616fcc の A の P-07
#[test]
fn put_local_refuses_a_write_made_from_another_map() {
    use crate::planar_map::LocalEdit;
    let mut cel = timeline::Cel::from_shapes_unwrap(vec![
        map_overlay::rect_shape(0.0, 0.0, 100.0, 100.0, Color32::RED),
        map_overlay::rect_shape(200.0, 0.0, 300.0, 100.0, Color32::BLUE),
    ]);
    let old = cel.scope_base(&[]);
    let LocalEdit::Changed(stale) = old.fill_local(Point::new(50.0, 50.0), Color32::BLACK, &[]) else {
        panic!("塗り替えられない");
    };
    // 間に別の編集（青を緑に）を書く
    let LocalEdit::Changed(w) = old.fill_local(Point::new(250.0, 50.0), Color32::GREEN, &[]) else {
        panic!("塗り替えられない");
    };
    assert!(cel.put_local(&[], w, &old).is_ok());
    let between = cel.clone();
    // 古い地図から作った書くものは断り、間の編集を残す
    assert!(cel.put_local(&[], stale.clone(), &old).is_err(), "古い計画を書いた");
    assert!(cel.scope_stored(&[]).unwrap().same(between.scope_stored(&[]).unwrap()));
    // 今の地図から作り直したものは書ける
    let now = cel.scope_base(&[]);
    let LocalEdit::Changed(fresh) = now.fill_local(Point::new(50.0, 50.0), Color32::BLACK, &[]) else {
        panic!("塗り替えられない");
    };
    assert!(cel.put_local(&[], fresh, &now).is_ok());
}

/// N-1003：面の色を変えると、消えた境目の端の2本だけがつながる頂点は、選んだ面の塗り替え（選択中のバケツ）でも
/// バケツでも同じくまとまる（面の色を変えるのは1つの関数）
#[test]
fn recoloring_faces_merges_the_ends_of_vanished_borders() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap};
    use crate::vector_engine::ShapeId;
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let left = vec![map_overlay::rect_shape(0.0, 0.0, 100.0, 100.0, red)];
    let right = vec![map_overlay::rect_shape(100.0, 0.0, 200.0, 100.0, blue)];
    let m = PlanarMap::default()
        .overlay(MapInput::Shapes(&left), MapRule::Paint(Some(red)))
        .unwrap()
        .overlay(MapInput::Shapes(&right), MapRule::Paint(Some(blue)))
        .unwrap()
        .checked()
        .unwrap()
        .freshly_labeled()
        .into_map();
    // 赤い四角と青い四角が1本の境目で接する：頂点 6・辺 7
    assert_eq!((m.nodes.len(), m.edges.len()), (6, 7));
    let blue_ids: std::collections::HashSet<ShapeId> = m
        .edges
        .iter()
        .flat_map(|e| [(e.left, e.left_src), (e.right, e.right_src)])
        .filter(|(c, _)| *c == Some(blue))
        .filter_map(|(_, s)| s)
        .collect();
    let recolored = m.recolored(&blue_ids, red).unwrap();
    assert!(recolored.is_valid());
    // 境目が消え、その両端（上の辺・下の辺の途中）もまとまって、1つの四角になる
    assert_eq!((recolored.nodes.len(), recolored.edges.len()), (4, 4));
    let LocalEdit::Changed(w) = LayerBase::new(&m).fill_local(Point::new(150.0, 50.0), red, &[])
    else {
        panic!("塗れない");
    };
    let filled = w.stored.to_dense();
    assert_eq!(
        (filled.nodes.len(), filled.edges.len()),
        (recolored.nodes.len(), recolored.edges.len())
    );
}

/// R1（レビュー f01d518）：切った区間を曲線に戻すとき、曲線の判定の形が区間の並びとまったく同じなら、判定の形は変わらない
/// ので、置き換えてよいかを調べず、そのための索引も作らない（`ChainRestorer`）。同じ絵を同じ位置に貼ると、どの鎖もそう
/// なる（前は、鎖があればいつも地図全体の索引を作り、鎖ごとに調べていた）
#[test]
fn restoring_unchanged_chains_builds_no_index() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
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
    let before = crate::planar_map::index_builds();
    let pasted = m.overlay(MapInput::Map(&m), MapRule::Overlay).unwrap();
    assert!(pasted.is_valid());
    assert_eq!(pasted.edges.len(), m.edges.len());
    assert_eq!(
        crate::planar_map::index_builds() - before,
        0,
        "判定の形の索引を作った回数"
    );
}

/// 編集のあとの地図の計算に使う写し（画面の土台が持つもの）は、判定の形と面の輪郭を、確かめたときのものから引き継ぐ。
/// ブラシ・消しゴム・バケツをくり返しても、引き継いだものは地図から作り直したものと同じ（レビュー f01d518 の R3 段1・段2）
#[test]
fn derived_geometry_carried_across_edits_matches_a_fresh_build() {
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut app = Is2dApp::default();
    let mut rng = map_overlay::Rng(9);
    let stroke = |rng: &mut map_overlay::Rng, n: usize| -> Vec<Point> {
        let mut p = Point::new(-150.0 + rng.f() * 300.0, -90.0 + rng.f() * 180.0);
        let mut a = rng.f() * std::f64::consts::TAU;
        (0..n)
            .map(|_| {
                a += (rng.f() - 0.5) * 1.2;
                p += kurbo::Vec2::new(a.cos(), a.sin()) * 4.0;
                p
            })
            .collect()
    };
    let mut checked = 0;
    for step in 0..120 {
        let color = colors[step % 3];
        let action = match step % 5 {
            0..=2 => doc_actions::Action::PaintStroke {
                points: stroke(&mut rng, 4 + step % 20),
                radius: 2.0 + (step % 7) as f32,
                color,
            },
            3 => doc_actions::Action::EraseStroke {
                points: stroke(&mut rng, 6),
                radius: 3.0,
            },
            _ => doc_actions::Action::BucketFill {
                pt: Point::new(-150.0 + rng.f() * 300.0, -90.0 + rng.f() * 180.0),
                color,
            },
        };
        if !doc_actions::apply(&mut app, action) {
            continue;
        }
        // 写しが持っている判定の形・輪郭・輪は、作り直したものと同じ（触った所だけで書いた写しは、まだ持っていない
        // ことがある：要るときに作る）
        for m in app.doc.root.layers[0].keyframes[0].cel.maps().iter() {
            assert_ne!(m.base().derived_match_full(), Some(false), "step {step}");
            checked += 1;
        }
    }
    assert!(checked > 80, "確かめた写し {checked}");
}

/// 穴の入れ先を引き継ぐ決まり（`LocalMap::hole_owner`）：赤の面の中の青い島（赤の面の穴）のまわりに、島に触れない緑の輪を
/// 描くと、穴の入れ先が、変わらない赤の外周から新しい輪の内側に替わる。輪をそれより大きい赤の輪で塗りつぶすと、また赤の
/// 外周に戻る。消しゴムの穴（色の無い穴）のまわりも同じ。触った所だけの道の答えは、テストでは毎回お手本（同じ道を地図全体を
/// 触った所として行った答え：`whole_touched`）と比べる（レビュー 96fdc26 の N-1001）
#[test]
fn holes_follow_new_exteriors_on_the_touched_only_path() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 90, 220);
    let green = Color32::from_rgb(40, 170, 80);
    let paint = |m: &PlanarMap, s: &FillShape, c: Option<Color32>| {
        m.overlay(MapInput::Shapes(std::slice::from_ref(s)), MapRule::Paint(c))
            .unwrap()
    };
    let ring = |cx: f64, cy: f64, r0: f64, r1: f64, c: Color32| {
        let outer = map_overlay::rect_shape(cx - r1, cy - r1, cx + r1, cy + r1, c);
        let inner = map_overlay::rect_shape(cx - r0, cy - r0, cx + r0, cy + r0, c);
        let hole = crate::vector_engine::Contour::new(
            inner
                .exterior()
                .segments
                .iter()
                .rev()
                .map(|q| kurbo::QuadBez::new(q.p2, q.p1, q.p0))
                .collect(),
        );
        FillShape::new(c, outer.exterior().clone(), vec![hole])
    };
    let edit = |base: &LayerBase, s: &FillShape, c: Color32| -> std::sync::Arc<LayerBase> {
        match base.edit_local(
            &base.paint_change(std::slice::from_ref(s), MapRule::Paint(Some(c))),
            &[],
        ) {
            LocalEdit::Changed(w) => {
                assert!(w.stored.to_dense().is_valid());
                LayerBase::of_local(w.stored, &[], w.faces, base)
            }
            _ => panic!("書けるはず"),
        }
    };
    for hole_color in [Some(blue), None] {
        let mut m = paint(
            &PlanarMap::default(),
            &map_overlay::rect_shape(0.0, 0.0, 400.0, 400.0, red),
            Some(red),
        );
        m = paint(
            &m,
            &map_overlay::rect_shape(180.0, 180.0, 220.0, 220.0, blue),
            hole_color,
        );
        let base = std::sync::Arc::new(LayerBase::new(&m.checked().unwrap().uniquely_labeled().into_map()));
        let with_ring = edit(&base, &ring(200.0, 200.0, 60.0, 90.0, green), green);
        let (faces, broken) = with_ring.map().faces();
        assert_eq!(broken, 0);
        // 島（色があれば）と、緑の輪の内側の赤の面と、緑の輪と、外の赤の面
        assert_eq!(faces.len(), 3 + usize::from(hole_color.is_some()));
        let back = edit(&with_ring, &ring(200.0, 200.0, 50.0, 100.0, red), red);
        let (faces, broken) = back.map().faces();
        assert_eq!(broken, 0);
        assert_eq!(faces.len(), 1 + usize::from(hole_color.is_some()));
    }
}

/// 重ね合わせの1回で、判定の形の索引を作るのは1回：曲線に戻す所（`ChainRestorer`）が作った索引を、続く
/// `merge_degree2` もそのまま使う（設計：claude/N-2002の残り・R5・★4・★5の設計_80fe785.md。直す前は2回）
#[test]
fn overlay_builds_the_shape_index_once() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(235, 75, 75);
    let left = vec![map_overlay::rect_shape(0.0, 0.0, 100.0, 100.0, red)];
    let right = vec![map_overlay::rect_shape(50.0, 0.0, 150.0, 100.0, red)];
    let m = PlanarMap::default()
        .overlay(MapInput::Shapes(&left), MapRule::Paint(Some(red)))
        .unwrap();
    let before = crate::planar_map::index_builds();
    let m = m
        .overlay(MapInput::Shapes(&right), MapRule::Paint(Some(red)))
        .unwrap();
    assert_eq!(crate::planar_map::index_builds() - before, 1);
    // 重なった所の境目は消え、その端の切れ目もまとまって、1つの四角になる
    assert!(m.is_valid());
    assert_eq!((m.nodes.len(), m.edges.len()), (4, 4));
}

/// 選んだ面だけで作る地図（`LayerBase::only_faces_map`）は、地図全体を作ってから選んだ面と線を残した地図
/// （`PlanarMap::restricted`。お手本）と、頂点と辺まで同じ：でたらめに描いたセル画で、でたらめに選んだ図形の番号で比べる
/// （持ち上げた絵のコピー・Alt 複製・F8 が、こちらで作るようになった：全体の点検 2026-11-27）
#[test]
fn only_faces_map_matches_restricting_the_whole_map() {
    let names = ["PaintStroke", "PencilStroke", "BucketFill", "InkBottle"];
    let mut compared = 0;
    for seed in [0x0f00_0001u64, 0x0f00_0002, 0x0f00_0003] {
        let mut rng = OpRng(seed);
        let mut app = Is2dApp::default();
        for step in 0..120 {
            let action = random_action(names[rng.range(names.len())], &mut rng, &app);
            apply(&mut app, action);
            if step % 6 != 5 {
                continue;
            }
            let tl = app.tab().active_timeline();
            let Some(cel) = tl.layers[0].cel_at(tl.current_frame) else {
                continue;
            };
            for m in cel.maps().iter() {
                let base = m.base();
                let ids: Vec<crate::vector_engine::ShapeId> = base.drawn_shapes(&m.group_path).iter().map(|s| s.shape_id).collect();
                if ids.is_empty() {
                    continue;
                }
                for _ in 0..3 {
                    let pick: std::collections::HashSet<crate::vector_engine::ShapeId> = ids.iter().copied().filter(|_| rng.coin()).collect();
                    let fast = base.only_faces_map(&pick, &m.group_path);
                    let whole = base.map().restricted(&pick);
                    assert_eq!((&fast.nodes, &fast.edges), (&whole.nodes, &whole.edges), "seed={seed:#x} step={step}");
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 20, "比べた数が少ない：{compared}");
}

/// 段2：線の多いセル画（同じ区画を 4×4 個並べた、辺 約 2.4 万本）で、ブラシ・消しゴム・矩形で切る中と外・片を置く・
/// 辺の引っぱり・頂点の移動を、本番の道（窓で重ねる `edit_local`・`cut_local`）で行う。区画の中・区画をまたぐ所・大きな
/// 矩形で行う。窓の答えが地図全体で重ねた答えと、しまう形でビットまで同じことは、道の中のお手本との比べ（`overlay_diff`・
/// `cut_local`）が毎回確かめる。窓の外の辺を点の色の数えに入れる道も通る
#[test]
fn windowed_overlay_matches_full_on_large_map() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap, ReshapeEdit};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let area = (-150.0, -75.0, 300.0, 150.0);
    let mut rng = map_overlay::Rng(9);
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
    let mut m = PlanarMap::default();
    for i in 0..4 {
        for j in 0..4 {
            let off = KVec2::new(i as f64 * dx, j as f64 * dy);
            let n = m.nodes.len();
            m.nodes.extend(block.nodes.iter().map(|&p| p + off));
            m.edges
                .extend(block.edges.iter().map(|e| crate::planar_map::MapEdge {
                    a: e.a + n,
                    b: e.b + n,
                    ctrl: e.ctrl + off,
                    ..*e
                }));
        }
    }
    let mut m = m.checked().unwrap().freshly_labeled().into_map();
    let winds_before = crate::planar_map::outside_winds();
    let mut done = 0usize;
    let total = (4.0 * dx, 4.0 * dy);
    for step in 0..84 {
        let base = LayerBase::new(&m);
        let full = base.map().clone();
        let c = colors[step % 4];
        // 場所：区画の中か、区画の境目のあたり
        let at = Point::new(bb.x0 + rng.f() * total.0, bb.y0 + rng.f() * total.1);
        let kind = step % 7;
        // 次の地図（書き戻した地図。面に1つずつ別の番号）
        let next: Option<PlanarMap> = match kind {
            2 => {
                let (w, h) = (20.0 + rng.f() * 200.0, 20.0 + rng.f() * 120.0);
                let rect = vec![map_overlay::rect_shape(at.x, at.y, at.x + w, at.y + h, c)];
                base.cut_local(MapInput::Shapes(&rect))
                    .and_then(|(_, rest)| {
                        Some(rest.to_dense().checked()?.uniquely_labeled().into_map())
                    })
            }
            _ => {
                let change = match kind {
                    0 | 1 => {
                        let r = 2.0 + rng.f() * 9.0;
                        let st = wiggly_stroke(&mut rng, (at.x, at.y, 60.0, 40.0), r, c);
                        let rule = if kind == 0 {
                            MapRule::Paint(Some(c))
                        } else {
                            MapRule::Paint(None)
                        };
                        base.edit_local(&base.paint_change(&st, rule), &[])
                    }
                    // 地図に地図を重ねる（片を置く・貼り付け）：矩形で切った片を、そのままの位置（同じ線の組ができる）か、少し
                    // ずらして、または遠くへ動かして重ねる
                    5 | 6 => {
                        let (w, h) = (20.0 + rng.f() * 150.0, 20.0 + rng.f() * 100.0);
                        let rect = vec![map_overlay::rect_shape(at.x, at.y, at.x + w, at.y + h, c)];
                        let Some(piece) =
                            full.overlay(MapInput::Shapes(&rect), MapRule::KeepInside)
                        else {
                            continue;
                        };
                        let off = match (kind, step % 3) {
                            (5, _) => KVec2::ZERO,
                            (_, 0) => KVec2::new(rng.f() * 2.0, rng.f() * 2.0),
                            _ => KVec2::new(
                                (rng.f() - 0.5) * total.0 * 0.5,
                                (rng.f() - 0.5) * total.1 * 0.5,
                            ),
                        };
                        let moved = piece.transformed(kurbo::Affine::translate(off));
                        base.edit_local(&base.map_change(&moved, crate::planar_map::MapRule::Overlay), &[])
                    }
                    _ => {
                        let e = (rng.f() * full.edges.len() as f64) as usize % full.edges.len();
                        let q = full.quad(e);
                        let ang = rng.f() * std::f64::consts::TAU;
                        let d = KVec2::new(ang.cos(), ang.sin()) * (0.5 + rng.f() * 10.0);
                        let edit = if kind == 3 {
                            ReshapeEdit::PullEdge {
                                orig_seg: q,
                                grab_t: 0.5,
                                target: q.eval(0.5) + d,
                                follow: true,
                            }
                        } else {
                            ReshapeEdit::MoveVertex {
                                orig_vertex: q.p0,
                                target: q.p0 + d,
                            }
                        };
                        let change = base.reshape_change(edit).expect("辺と頂点がある");
                        base.edit_local(&change, &[])
                    }
                };
                match change {
                    LocalEdit::Changed(w) => Some(w.stored.to_dense()),
                    _ => None,
                }
            }
        };
        done += 1;
        if let Some(n) = next {
            m = n;
        }
    }
    assert!(done >= 70, "行った数 {done}");
    assert!(
        crate::planar_map::outside_winds() > winds_before,
        "点の色の数えに窓の外の辺の分を入れる道を通っていない"
    );
}

/// レビュー 983236e の N-1001：重ねるものが触る範囲（窓の範囲）は、文書の座標での中の広さでなく、線の長さと中身で決まる。
/// 何も無い所の広さだけを変えて矩形で選んでも、範囲が持つ項目の数は矩形の辺の長さほどにしか増えない（直す前は、中の
/// マスを1つずつ持ったので、中の広さに比例した。縮めたシンボルの中で最小のズームで選ぶと、数秒固まり、メモリが数 GB 増えた）
#[test]
fn touched_range_grows_with_length_not_area() {
    use crate::canvas_tool::doc_actions::Action;
    let entries = |side: f64| -> usize {
        let mut app = Is2dApp::default();
        let lid = app.doc.root.layers[0].id;
        let mut shapes = make_stroke(
            Point::new(100.0, 100.0),
            Point::new(300.0, 100.0),
            20.0,
            Color32::RED,
        );
        shapes[0].layer_id = lid;
        put_cel_shapes(&mut app, 0, 0, shapes);
        let before = crate::planar_map::range_entries();
        run(
            &mut app,
            &[Action::SelectMarquee {
                p0: Point::new(200.0 - side / 2.0, 100.0 - side / 2.0),
                p1: Point::new(200.0 + side / 2.0, 100.0 + side / 2.0),
                additive: false,
            }],
        );
        assert_eq!(
            app.session.floating().map(|_| app.session.floating_view().pieces().len()),
            Some(1),
            "線をまるごと選ぶ"
        );
        crate::planar_map::range_entries() - before
    };
    let (small, big) = (entries(1_000.0), entries(4_000.0));
    // 辺の長さは4倍、中の広さは16倍。その間（8倍）より少ない
    assert!(big < small * 8, "範囲の項目の数 {small} → {big}");
}

/// 曲線の上のいちばん近い点（`nearest_on_quad`）は、どんな形でも大域のいちばん近い点：折り返した曲線（とげ）を含む、
/// いろいろな曲線の上の点は、地図の精度（`FLAT_TOL` = 0.002px）の中で曲線の上にあり、曲線の近くの点の距離は、密に調べた
/// 距離のいちばん小さいものより地図の精度以上に遠くならない（もう一方の枝の点を返さない）
#[test]
fn nearest_on_quad_finds_the_global_nearest_point_even_on_spikes() {
    use crate::vector_engine::nearest_on_quad;
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64 * 200.0 - 100.0
    };
    let mut quads = vec![
        QuadBez::new((0.0, 0.0), (100.0, 1.0), (0.0, 2.0)),
        QuadBez::new((0.0, 0.0), (80.0, 30.0), (10.0, 5.0)),
    ];
    quads.extend((0..200).map(|_| {
        QuadBez::new((next(), next()), (next(), next()), (next(), next()))
    }));
    let flat_tol = 0.002;
    for q in &quads {
        for k in 0..=200 {
            let on = q.eval(k as f64 / 200.0);
            let (t, d) = nearest_on_quad(q, on);
            assert!(d <= flat_tol, "曲線の上の点 {on:?} が {d} 離れている（t {t}、{q:?}）");
            let off = Point::new(on.x + next() * 0.05, on.y + next() * 0.05);
            let dense = (0..=4000)
                .map(|j| q.eval(j as f64 / 4000.0).distance(off))
                .fold(f64::INFINITY, f64::min);
            assert!(nearest_on_quad(q, off).1 <= dense + flat_tol, "{off:?} の距離が大域の最小より遠い（{q:?}）");
        }
    }
}

/// 今の図形から境目の線で持つ地図を作って図形に戻しても、塗りは変わらない
#[test]
fn test_planar_map_round_trip_keeps_fills() {
    let app = three_color_junction_scene();
    let faces: Vec<FillShape> = scene_summary(&app).shapes.clone();
    let map = crate::planar_map::PlanarMap::from_faces(&faces);
    let (back, broken) = map.to_faces_checked(1, &[]);
    assert_eq!(broken, 0);
    assert_eq!(back.len(), faces.len());
    let color_at =
        |fs: &[FillShape], p: Point| fs.iter().find(|s| s.contains_point(p)).map(|s| s.color());
    let mut y = 180.0;
    while y < 420.0 {
        let mut x = 180.0;
        while x < 420.0 {
            let p = Point::new(x + 0.137, y + 0.173);
            assert_eq!(color_at(&faces, p), color_at(&back, p), "{p:?}");
            x += 2.0;
        }
        y += 2.0;
    }
}

/// 変換した地図が正しい地図にならず、面を塗り重ねて作り直しても（`realized`）、面の番号が残る：塗り重ねは、いちばん上の形
/// の番号を面の印として運ぶ（レビュー 4f4e953 の N-2001。前は作り直した面の番号がみな新しくなり、選択から外れた）
#[test]
fn painting_faces_back_keeps_their_ids() {
    let mut tried = 0;
    for seed in 0..40 {
        let map = crate::canvas_tool::tests::map_overlay::native_scene(seed, 12)
            .checked()
            .expect("正しい地図")
            .uniquely_labeled()
            .into_map();
        let ids: std::collections::HashSet<_> = map.labels().collect();
        for scale in [0.05, 0.02, 0.01] {
            let xform = Affine::scale(scale);
            if map.transformed(xform).is_valid() {
                continue;
            }
            tried += 1;
            let back = crate::planar_map::realized(&map, xform).expect("塗り重ねて作れる");
            let (faces, _) = back.map().faces();
            for f in &faces {
                assert!(
                    f.srcs.iter().any(|id| ids.contains(id)),
                    "種 {seed}・倍率 {scale}：作り直した面に元の番号が無い {:?}",
                    f.srcs
                );
            }
        }
    }
    assert!(tried > 0, "塗り重ねに回る変換が無かった");
}

/// 地図の面ごとの面積（面の輪郭の曲線を8等分した折れ線で）
fn face_areas(m: &crate::planar_map::PlanarMap) -> Vec<f64> {
    m.valid_faces()
        .unwrap()
        .iter()
        .map(|f| {
            f.cycles
                .iter()
                .map(|cyc| {
                    let pts: Vec<Point> = cyc
                        .iter()
                        .flat_map(|&(e, fwd)| {
                            let q = m.quad(e);
                            (0..8).map(move |k| {
                                let t = k as f64 / 8.0;
                                q.eval(if fwd { t } else { 1.0 - t })
                            })
                        })
                        .collect();
                    (0..pts.len())
                        .map(|i| {
                            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                            a.x * b.y - b.x * a.y
                        })
                        .sum::<f64>()
                        / 2.0
                })
                .sum::<f64>()
                .abs()
        })
        .collect()
}

/// 試し（N-1002）：ほぼ重なった線を重ねると、見えない細い欠片の面ができるか
#[test]
#[ignore]
fn probe_same_line_slivers() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    // 線の重なった地図に、同じ線を色を交互に 200 回
    for seed in [11u64, 12, 13] {
        let mut rng = map_overlay::Rng(seed);
        let colors = [red, blue, Color32::from_rgb(40, 180, 90)];
        let mut m = PlanarMap::default();
        for k in 0..60 {
            let (r, c) = (2.0 + rng.f() * 8.0, colors[k % 3]);
            let s = wiggly_stroke(&mut rng, (100.0, 100.0, 300.0, 200.0), r, c);
            m = m
                .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
                .unwrap();
        }
        let s = wiggly_stroke(&mut rng, (200.0, 150.0, 100.0, 100.0), 6.0, red);
        let tiny = |m: &PlanarMap| face_areas(m).iter().filter(|&&a| a < 0.01).count();
        let mut first = 0;
        for k in 0..200 {
            let c = if k % 2 == 0 { red } else { blue };
            m = m
                .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
                .unwrap();
            if k == 0 {
                first = tiny(&m);
            }
        }
        eprintln!(
            "dense seed {seed}: tiny faces after 1 paint {first}, after 200 {}",
            tiny(&m)
        );
    }
    for seed in [1u64, 2, 3] {
        let s = wiggly_stroke(
            &mut map_overlay::Rng(seed),
            (300.0, 200.0, 0.0, 0.0),
            6.0,
            red,
        );
        let mut m = PlanarMap::default();
        let mut first = (0, 0);
        for k in 0..200 {
            let c = if k % 2 == 0 { red } else { blue };
            m = m
                .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
                .unwrap();
            if k == 0 {
                first = (m.valid_faces().unwrap().len(), m.edges.len());
            }
        }
        eprintln!(
            "seed {seed}: same line 200 times: faces {} -> {}, edges {} -> {}",
            first.0,
            m.valid_faces().unwrap().len(),
            first.1,
            m.edges.len()
        );
        let base = PlanarMap::default()
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(red)))
            .unwrap();
        for d in [1e-5, 1e-4, 1e-3, 3e-3] {
            let moved: Vec<FillShape> = s
                .iter()
                .map(|x| {
                    let mut y = x.clone();
                    y.transform(kurbo::Affine::translate((d, d * 0.5)));
                    y
                })
                .collect();
            let m2 = base
                .overlay(MapInput::Shapes(&moved), MapRule::Paint(Some(blue)))
                .unwrap();
            eprintln!(
                "seed {seed}: offset {d:e}: faces {}, edges {} (base edges {})",
                m2.valid_faces().unwrap().len(),
                m2.edges.len(),
                base.edges.len()
            );
        }
    }
}

/// N-1002（地図のレビュー 4cb53aa）：地図の精度（0.002px）より近い2本は同じ線になる。同じ形をわずかにずらして重ねても、
/// 同じ線を色を変えて何度も重ねても、見えない細い欠片の面ができない。重ねる形の頂点が前の頂点から 0.002px 以内なのに
/// 格子の別の点に残ると、そこで2本の折れ線が分かれて欠片の面ができていた
#[test]
fn nearly_coincident_lines_leave_no_slivers() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    for seed in [1u64, 2] {
        let s = wiggly_stroke(
            &mut map_overlay::Rng(seed),
            (300.0, 200.0, 0.0, 0.0),
            6.0,
            red,
        );
        let base = PlanarMap::default()
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(red)))
            .unwrap();
        let n = base.edges.len();
        for d in [1e-5, 1e-4, 1e-3] {
            let shift = kurbo::Affine::translate((d, d * 0.5));
            // 形をずらして重ねる
            let moved: Vec<FillShape> = s
                .iter()
                .map(|x| {
                    let mut y = x.clone();
                    y.transform(shift);
                    y
                })
                .collect();
            let m = base
                .overlay(MapInput::Shapes(&moved), MapRule::Paint(Some(blue)))
                .unwrap();
            assert_eq!((m.valid_faces().unwrap().len(), m.edges.len()), (1, n));
        }
        // 同じ線を、色を交互に 40 回
        let mut m = base.clone();
        for k in 0..40 {
            let c = if k % 2 == 0 { blue } else { red };
            m = m
                .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
                .unwrap();
            assert_eq!((m.valid_faces().unwrap().len(), m.edges.len()), (1, n));
        }
    }
}

/// 段2b-7g：何も無い側の決まり（何も無い所が色のある面に入り込まない）を触った所だけで確かめた答えが、新しい地図全体を
/// 確かめた答えと同じ。入り込む地図を、わざと作って比べる：
/// - そのままの面の中に、外が何も無い島を足す（前の地図の、点を含む面を、半直線が最初に横切る辺で決める所）
/// - 前からある、外が何も無い島のまわりに、穴を開けずに色のある四角を足す（触った面の中に前の地図の辺が入る所）
/// - 面の輪郭を作り直し（触った面にし）、その中に、外が何も無い島を足す（触った面の中に増えた辺が入る所）
///
/// どれも、島や四角を面の外に置けば正しい（入り込まない）地図になるので、それも比べる
#[test]
fn local_check_catches_unpainted_inside_painted() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(235, 75, 75);
    let blue = Color32::from_rgb(55, 135, 245);
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64, c: Color32| -> PlanarMap {
        PlanarMap::default()
            .overlay(
                MapInput::Shapes(&[map_overlay::rect_shape(x0, y0, x1, y1, c)]),
                MapRule::Paint(Some(c)),
            )
            .unwrap()
    };
    // 2つの地図の辺を1つに並べる（後ろの地図の頂点は後ろへずらす。重なる頂点は無い）
    let join = |a: &PlanarMap, b: &PlanarMap| -> PlanarMap {
        let n = a.nodes.len();
        let mut m = a.clone();
        m.nodes.extend(b.nodes.iter().copied());
        m.edges
            .extend(b.edges.iter().map(|e| crate::planar_map::MapEdge {
                a: e.a + n,
                b: e.b + n,
                ..*e
            }));
        m
    };
    // 前の地図 `old` に、消す辺 `removed`（前の地図の番号）と、地図 `extra` の辺を足した地図を、触った所だけで確かめた
    // 答えと、全体で確かめた答え
    let check = |old: &PlanarMap, removed: &[usize], extra: &PlanarMap| -> (bool, bool) {
        let old = old.clone().checked().unwrap().uniquely_labeled().into_map();
        let base = LayerBase::new(&old);
        let full_map = base.map().clone();
        let kept: Vec<usize> = (0..full_map.edges.len())
            .filter(|e| !removed.contains(e))
            .collect();
        let mut result = PlanarMap {
            nodes: full_map.nodes.clone(),
            edges: kept.iter().map(|&e| full_map.edges[e]).collect(),
        };
        let n0 = result.edges.len();
        result = join(&result, extra);
        let added: Vec<usize> = (n0..result.edges.len()).collect();
        // 前の地図の辺の番号で消す辺を渡す（`apply_diff` は、増えた辺を `result` の番号で受け取る）
        let got = !matches!(
            base.apply_diff(&result, removed, &added, &[]),
            LocalEdit::Refused(_)
        );
        (got, result.is_valid())
    };
    let red_sq = rect(0.0, 0.0, 100.0, 100.0, red);
    // 1. そのままの面（赤）の中に、外が何も無い青い島を足す：入り込む。赤の左に置けば正しい
    let (got, full) = check(&red_sq, &[], &rect(40.0, 40.0, 60.0, 60.0, blue));
    assert!(!full, "赤の中の、外が何も無い島は入り込む（全体の確かめ）");
    assert_eq!(got, full, "そのままの面の中に足した島");
    let (got, full) = check(&red_sq, &[], &rect(-60.0, 40.0, -40.0, 60.0, blue));
    assert!(full, "赤の外の島は正しい");
    assert_eq!(got, full, "そのままの面の外に足した島");
    // 2. 前からある青い島のまわりに、穴を開けずに赤い四角を足す：入り込む。島から離せば正しい
    let island = rect(40.0, 40.0, 60.0, 60.0, blue);
    let (got, full) = check(&island, &[], &red_sq);
    assert!(
        !full,
        "穴の無い赤の中の、前からある島は入り込む（全体の確かめ）"
    );
    assert_eq!(got, full, "前からある島のまわりに足した四角");
    let (got, full) = check(&island, &[], &rect(100.0, 0.0, 200.0, 100.0, red));
    assert!(full, "島から離れた赤は正しい");
    assert_eq!(got, full, "前からある島から離れた四角");
    // 3. 赤の輪郭を作り直し（消して同じ辺を足す：触った面）、その中に外が何も無い島を足す：入り込む。外に置けば正しい
    let all: Vec<usize> = (0..LayerBase::new(
        &red_sq
            .clone()
            .checked()
            .unwrap()
            .uniquely_labeled()
            .into_map(),
    )
    .map()
    .edges
    .len())
        .collect();
    let redo = |island: &PlanarMap| -> PlanarMap {
        let base = LayerBase::new(
            &red_sq
                .clone()
                .checked()
                .unwrap()
                .uniquely_labeled()
                .into_map(),
        );
        join(base.map(), island)
    };
    let (got, full) = check(&red_sq, &all, &redo(&rect(40.0, 40.0, 60.0, 60.0, blue)));
    assert!(!full, "作り直した赤の中の島は入り込む（全体の確かめ）");
    assert_eq!(got, full, "触った面の中に足した島");
    let (got, full) = check(&red_sq, &all, &redo(&rect(-60.0, 40.0, -40.0, 60.0, blue)));
    assert!(full, "作り直した赤の外の島は正しい");
    assert_eq!(got, full, "触った面の外に足した島");
}

/// 段2b-7c-2：ブラシ・消しゴムを触った所だけで行う編集（1本の道：`LayerBase::edit_local`）を、ランダムな線で何十回も重ねる。
/// 毎回、関数の中でお手本（地図全体で重ねた答え、同じ道を地図全体を触った所として行った答え：しまう形・消す図形・入れる
/// 図形）と比べ、次の編集は新しいしまう形から続ける。線の多いセル画（区画を 4×4 並べたもの）でも行う
#[test]
fn local_paint_matches_full_pipeline() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let mut rng = map_overlay::Rng(37);
    // 下ごしらえ：区画の地図と、それを 4×4 並べた地図
    let mut block = PlanarMap::default();
    let mut k = 0;
    while block.edges.len() < 800 {
        let (r, c) = (2.0 + rng.f() * 8.0, colors[k % 4]);
        let s = wiggly_stroke(&mut rng, (-150.0, -75.0, 300.0, 150.0), r, c);
        block = block
            .overlay(MapInput::Shapes(&s), MapRule::Paint(Some(c)))
            .unwrap();
        k += 1;
    }
    let block = LayerBase::new(&block).map().clone();
    let bb = block.bounding_box().unwrap();
    let (dx, dy) = (bb.width() + 20.0, bb.height() + 20.0);
    let mut big = PlanarMap::default();
    for i in 0..4 {
        for j in 0..4 {
            let off = KVec2::new(i as f64 * dx, j as f64 * dy);
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
    let (mut changed, mut same) = (0, 0);
    for (m, steps, span) in [
        (block.clone(), 60, (bb.x0, bb.y0, bb.width(), bb.height())),
        (big, 40, (bb.x0, bb.y0, 4.0 * dx, 4.0 * dy)),
    ] {
        let m = m.checked().unwrap().uniquely_labeled().into_map();
        let mut base = std::sync::Arc::new(LayerBase::new(&m));
        for step in 0..steps {
            let c = colors[step % 4];
            let at = (span.0 + rng.f() * span.2, span.1 + rng.f() * span.3);
            let r = 2.0 + rng.f() * 9.0;
            let st = wiggly_stroke(&mut rng, (at.0, at.1, 60.0, 40.0), r, c);
            let rule = if step % 3 == 2 {
                MapRule::Paint(None)
            } else {
                MapRule::Paint(Some(c))
            };
            match base.edit_local(&base.paint_change(&st, rule), &[]) {
                LocalEdit::Changed(w) => {
                    changed += 1;
                    base = LayerBase::of_stored(&w.stored);
                }
                LocalEdit::Same => same += 1,
                LocalEdit::Refused(_) => {}
            }
        }
    }
    assert!(changed > 50, "変わった {changed}・変わらない {same}");
}

/// 段2b-7c-2：触った所だけで行う編集をセル画に書く道（`Cel::edit_local`・`put_local`・`LayerBase::of_local`）。書くたびに
/// 編集の答えはお手本と比べられ（`edit_local`・`apply_diff` の中）、書いたあとのセル画は決まりどおり（正しい地図で、面に1つずつ別の
/// 番号：`maps_problem`）で、写しの面の図形の表は、地図から作り直したものと同じ
#[test]
fn local_paint_writes_through_the_cel() {
    use crate::planar_map::MapRule;
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut rng = map_overlay::Rng(43);
    let mut shapes = Vec::new();
    for k in 0..12 {
        let r = 3.0 + rng.f() * 6.0;
        shapes.extend(wiggly_stroke(
            &mut rng,
            (100.0, 100.0, 200.0, 120.0),
            r,
            colors[k % 3],
        ));
    }
    let mut app = Is2dApp::default();
    put_cel_shapes(&mut app, 0, 0, shapes);
    let mut cel = app.doc.root.layers[0].keyframes[0].cel.clone();
    let mut written = 0;
    for step in 0..40 {
        let c = colors[step % 3];
        let r = 2.0 + rng.f() * 7.0;
        let st = wiggly_stroke(&mut rng, (100.0, 100.0, 200.0, 120.0), r, c);
        let rule = if step % 4 == 3 {
            MapRule::Paint(None)
        } else {
            MapRule::Paint(Some(c))
        };
        if let Ok(Some(_)) = cel.edit_local(&[], |base| {
            base.edit_local(&base.paint_change(&st, rule), &[])
        }) {
            written += 1;
            assert_eq!(cel.maps_problem(), None, "step {step}");
            if let Some(m) = cel.maps().iter().find(|m| m.group_path.is_empty()) {
                let base = m.base();
                let fresh = crate::planar_map::LayerBase::new(base.map());
                let ids = |l: &crate::planar_map::FaceTable| -> Vec<u64> {
                    l.iter().map(|s| s.shape_id).collect()
                };
                assert_eq!(ids(&base.faces(&[])), ids(&fresh.faces(&[])), "step {step}");
            }
        }
    }
    assert!(written > 20, "書いた {written}");
}

/// 線の番号だけが違う辺は、違う辺として比べる（辺の比べ方 `==` が `Hash` と同じく線の番号まで見る）。前は `==` が線の
/// 番号を見ず `Hash` が見たので、差分を作る所（`diff_tiles` など）が、番号だけ入れ替わった辺（左右対称な線を反転した
/// ときなど）を「変わっていない」と答えたり、集まりの引き方しだいで答えが食い違ったりして、同じ辺が2本できる・線の番号が
/// 重なり、確定が「置けない」で断られた（番号の進み方しだいで、ときどき起きた：レビュー de92ceb の N-1003）
#[test]
fn edges_that_differ_only_in_line_id_are_different() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use crate::planar_map::LINE_ID_BIT;
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 4 };
    let mut app = Is2dApp::default();
    apply(&mut app, Action::SetTool { tool: Tool::Pencil });
    let pts: Vec<Point> = (0..=20).map(|i| Point::new(100.0 + 10.0 * i as f64, 200.0)).collect();
    assert!(apply(&mut app, Action::PencilStroke { points: pts, stroke: ink }));
    let cel = &app.doc.root.layers.iter().find_map(|l| l.keyframes.first().filter(|k| !k.cel.maps().is_empty())).expect("線のあるセル画").cel;
    let stored = cel.maps()[0].stored().clone();
    // 線の番号だけを替えた地図（形・色・側の番号は同じ）
    let renumbered = stored.relabeled(|id| if id & LINE_ID_BIT != 0 { id + 1 } else { id }, &mut Default::default());
    let lines = |m: &crate::stored_map::StoredMap| (0..m.edge_count()).filter_map(|e| m.edge_at(e).line_id).collect::<Vec<_>>();
    assert_ne!(lines(&stored), lines(&renumbered));
    assert!(!stored.same_content(&renumbered), "番号だけ替えた地図を、同じ中身とみなした");
    // 前の地図の塊を使い回してしまう形を作っても、新しい番号のまま（番号の違う塊を「同じ塊」として使い回さない）
    let rebuilt = crate::stored_map::StoredMap::from_dense(&renumbered.to_dense(), Some(&stored));
    assert_eq!(lines(&rebuilt), lines(&renumbered), "前の地図の塊を使い回して、線の番号が古いまま残った");
    let (removed, added) = stored.diff_tiles(&renumbered);
    assert_eq!(removed.len(), lines(&stored).len(), "番号だけ替えた辺を、変わっていないとみなした");
    assert_eq!(added.iter().filter_map(|e| e.line_id).collect::<Vec<_>>(), lines(&renumbered));
}

/// 直線の辺の両端を端に持つ曲線（D 字の形）を塗ると、その中が塗られる：曲線の両端が直線の辺の両端と同じでも、制御点が
/// 離れていれば同じ線にしない（同じ曲線かは両側の制御点で決める：`same_curve`。レビュー 4f4e953 の N-1001 ②）
#[test]
fn a_curve_sharing_both_ends_with_a_straight_edge_is_not_the_same_line() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    use crate::vector_engine::Contour;
    let mut red = vector_engine::make_marquee_rect_shape(kurbo::Rect::new(0.0, 0.0, 100.0, 100.0));
    red.recolor(|_| Color32::RED);
    let map = PlanarMap::from_faces(std::iter::once(&red));
    assert!(map.is_valid());
    let (a, b) = (Point::new(0.0, 0.0), Point::new(100.0, 0.0));
    let d = FillShape::new(
        Color32::BLUE,
        Contour::new(vec![
            QuadBez::new(a, a.midpoint(b), b),
            QuadBez::new(b, Point::new(50.0, 60.0), a),
        ]),
        Vec::new(),
    );
    let next = map
        .overlay(
            MapInput::Shapes(std::slice::from_ref(&d)),
            MapRule::Paint(Some(Color32::BLUE)),
        )
        .expect("重ねられる");
    assert!(next.is_valid());
    assert_eq!(
        next.color_at(Point::new(50.0, 15.0)),
        Some(Color32::BLUE),
        "D 字の中"
    );
    assert_eq!(
        next.color_at(Point::new(50.0, 80.0)),
        Some(Color32::RED),
        "D 字の外"
    );
    // レンズ：直線の辺の両側へふくらむ2本の曲線だけで囲まれた形
    let lens = FillShape::new(
        Color32::BLUE,
        Contour::new(vec![
            QuadBez::new(a, Point::new(50.0, -40.0), b),
            QuadBez::new(b, Point::new(50.0, 40.0), a),
        ]),
        Vec::new(),
    );
    let next = map
        .overlay(
            MapInput::Shapes(std::slice::from_ref(&lens)),
            MapRule::Paint(Some(Color32::BLUE)),
        )
        .expect("重ねられる");
    assert!(next.is_valid());
    for (p, place) in [((50.0, 10.0), "四角の中"), ((50.0, -10.0), "四角の外")] {
        assert_eq!(
            next.color_at(Point::new(p.0, p.1)),
            Some(Color32::BLUE),
            "{place}のレンズ"
        );
    }
}

/// レビュー f478c69 の N-1002：変換した地図が正しい地図にならないとき（縮めて頂点や辺が近づく）も、線が残る（前は面の形を
/// 塗り重ねて作り直したので、線がみな消え、線だけの絵は丸ごと消えた）。境目がみな線の絵では、作り直したあとも、色の違う
/// 面の境目はみな線のまま。線だけの絵は、線の辺がみな残る
#[test]
fn realizing_a_broken_transform_keeps_the_lines() {
    let ink = crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 };
    let mut tried = 0;
    for seed in 0..40 {
        for only_lines in [false, true] {
            let mut map = crate::canvas_tool::tests::map_overlay::native_scene(seed, 12);
            for e in &mut map.edges {
                e.stroke = Some(ink);
                if only_lines {
                    (e.left, e.right) = (None, None);
                }
            }
            for scale in [0.05, 0.02, 0.01] {
                let xform = Affine::scale(scale);
                if map.transformed(xform).is_valid() {
                    continue;
                }
                tried += 1;
                let back = crate::planar_map::realized(&map, xform).expect("作り直せる").into_map();
                let place = format!("種 {seed}・線だけ {only_lines}・倍率 {scale}");
                assert!(!back.edges.is_empty(), "{place}：絵が消えた");
                for e in &back.edges {
                    assert_eq!(e.stroke, Some(ink), "{place}：線の無い辺がある");
                }
            }
        }
    }
    assert!(tried > 0, "作り直しに回る変換が無かった");
}

/// S-2：正しい地図の判定（`valid_faces`。書き戻しも読み込みも同じ）は、頂点と制御点が格子に置けることと、両側が同じ色の
/// 辺が無いことも確かめる。巨大な座標の地図も、判定の形を作らずにすぐ断る。辺を引っぱって制御点だけ格子の外へ出る
/// 確定は断る
#[test]
fn valid_maps_are_on_the_grid_with_two_colors_per_edge() {
    use crate::planar_map::{MapEdge, MapInput, MapRule, PlanarMap};
    let red = Color32::RED;
    let square = |x0: f64, y0: f64, x1: f64, y1: f64, ctrl_of: &dyn Fn(Point, Point) -> Point| {
        let nodes = vec![
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ];
        let edges = (0..4)
            .map(|i| MapEdge {
                a: i,
                b: (i + 1) % 4,
                ctrl: ctrl_of(nodes[i], nodes[(i + 1) % 4]),
                left: None,
                right: Some(red),
                left_src: None,
                right_src: None,
                stroke: None,
                line_id: None,
            })
            .collect();
        PlanarMap { nodes, edges }
    };
    let mid = |a: Point, b: Point| a.midpoint(b);
    // ふつうの正方形（向きを確かめる）
    let ok = square(0.0, 0.0, 10.0, 10.0, &mid);
    let ok = if ok.is_valid() {
        ok
    } else {
        let mut m = ok;
        for e in &mut m.edges {
            std::mem::swap(&mut e.left, &mut e.right);
        }
        m
    };
    assert!(ok.is_valid());
    // 格子の外の頂点・巨大な頂点
    let mut far = ok.clone();
    far.nodes[1].x = 70_000.0;
    far.nodes[2].x = 70_000.0;
    for (i, e) in ok.edges.iter().enumerate() {
        far.edges[i].ctrl = far.nodes[e.a].midpoint(far.nodes[e.b]);
    }
    assert!(!far.is_valid());
    let mut huge = far.clone();
    huge.nodes[1].x = 1e12;
    huge.nodes[2].x = 1e12;
    for (i, e) in ok.edges.iter().enumerate() {
        huge.edges[i].ctrl = huge.nodes[e.a].midpoint(huge.nodes[e.b]);
    }
    assert!(!huge.is_valid());
    // 制御点だけ格子の外
    let mut ctrl_far = ok.clone();
    ctrl_far.edges[0].ctrl.y = -70_000.0;
    assert!(!ctrl_far.is_valid());
    // 両側が同じ色の辺（何も無い所に浮いた線）
    let mut ghost = ok.clone();
    let a = ghost.nodes.len();
    ghost.nodes.push(Point::new(20.0, 0.0));
    ghost.nodes.push(Point::new(30.0, 0.0));
    ghost.edges.push(MapEdge {
        a,
        b: a + 1,
        ctrl: Point::new(25.0, 0.0),
        left: None,
        right: None,
        left_src: None,
        right_src: None,
        stroke: None,
        line_id: None,
    });
    assert!(!ghost.is_valid());
    // 端の近くの正方形の辺を引っぱり、曲線は格子の内でも制御点が外へ出る確定は断る
    let edge_map = ok
        .overlay(MapInput::Map(&PlanarMap::default()), MapRule::Overlay)
        .unwrap();
    let near = crate::planar_map::realized(&edge_map, kurbo::Affine::translate((65_000.0, 0.0)))
        .unwrap()
        .into_map();
    assert!(near.is_valid());
    let e = (0..near.edges.len())
        .max_by(|&i, &j| {
            let x = |k: usize| near.quad(k).eval(0.5).x;
            x(i).total_cmp(&x(j))
        })
        .unwrap();
    let q = near.quad(e);
    let target = q.eval(0.18) + kurbo::Vec2::new(300.0, 0.0);
    let ctrl = q.p1 + (target - q.eval(0.18)) / (2.0 * 0.18 * 0.82);
    assert!(ctrl.x > 65_536.0 && q.eval(0.5).x < 65_536.0);
    assert!(matches!(
        map_overlay::reshaped(
            &map_overlay::labeled_base(&near),
            crate::planar_map::ReshapeEdit::PullEdge {
                orig_seg: q,
                grab_t: 0.18,
                target,
                follow: true,
            },
        ),
        Err(crate::planar_map::LocalRefusal::OffGrid)
    ));
}

/// R-1（地図のレビュー da27aa3 の N-1001・N-2001）：2本だけがつながる頂点をまとめるのは、その操作で消えた線の端だけ。
/// 足した角はバケツで塗っても消えず、切って戻した切り口の点は重ね合わせで消える
#[test]
fn only_cuts_made_by_the_operation_are_merged() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(235, 75, 75);
    let green = Color32::from_rgb(40, 180, 90);
    let stroke = make_stroke(
        Point::new(100.0, 200.0),
        Point::new(300.0, 200.0),
        25.0,
        red,
    );
    let m = PlanarMap::default()
        .overlay(MapInput::Shapes(&stroke), MapRule::Paint(Some(red)))
        .unwrap();
    assert!(m.is_valid());

    // 1. 足した角は、その辺に接する面をバケツで塗り替えても残る
    let e = (0..m.edges.len())
        .max_by(|&i, &j| {
            let len = |k: usize| (m.quad(k).p2 - m.quad(k).p0).hypot();
            len(i).total_cmp(&len(j))
        })
        .unwrap();
    let anchored = m.with_anchor_added(e, 0.5).unwrap();
    let corner = anchored.quad(e).eval(1.0);
    let filled = map_overlay::bucket(&anchored, Point::new(200.0, 200.0), green).unwrap();
    assert_eq!(filled.nodes.len(), anchored.nodes.len());
    assert!(filled.nodes.iter().any(|p| (*p - corner).hypot() < 1e-9));

    // 2. 矩形で切って、中をそのまま重ね直すと、切り口の点は残らない（辺の数が元に戻る）
    let rect = vec![map_overlay::rect_shape(150.0, 150.0, 250.0, 250.0, red)];
    let inside = m
        .overlay(MapInput::Shapes(&rect), MapRule::KeepInside)
        .unwrap();
    let rest = m
        .overlay(MapInput::Shapes(&rect), MapRule::Paint(None))
        .unwrap();
    let back = rest
        .overlay(MapInput::Map(&inside), MapRule::Overlay)
        .unwrap();
    assert!(back.is_valid());
    assert_eq!(back.edges.len(), m.edges.len());
}

/// R2（レビュー f01d518）：重ね合わせでは、i_overlay に1回だけ切らせる。切れ目の頂点は、切った曲線から
/// `FLAT_TOL + SPLIT_REACH` 以内にあり、戻すときはその許容で2本を1本にまとめる（`merge_degree2`）ので、曲線の本当の交点を
/// 折れ線に足して切り直す2回目の切りは要らない（前は、2本の曲線が交わる所があると、交点をニュートン法で求めて足し、
/// もう1回切っていた）
#[test]
fn overlay_cuts_once_where_curves_cross() {
    use crate::planar_map::{MapInput, MapRule, PlanarMap};
    let red = Color32::from_rgb(220, 60, 60);
    let blue = Color32::from_rgb(60, 90, 220);
    let a = vec![map_overlay::rect_shape(0.0, 0.0, 100.0, 60.0, red)];
    let m = PlanarMap::default()
        .overlay(MapInput::Shapes(&a), MapRule::Paint(Some(red)))
        .unwrap();
    let b = vec![map_overlay::rect_shape(50.0, 30.0, 150.0, 90.0, blue)];
    let before = crate::planar_map::cuts();
    let next = m
        .overlay(MapInput::Shapes(&b), MapRule::Paint(Some(blue)))
        .unwrap();
    assert!(next.is_valid());
    assert_eq!(
        crate::planar_map::cuts() - before,
        1,
        "i_overlay に切らせた回数"
    );
}

/// 段0（claude/触った所だけを計算する地図の設計_22fbbbe.md）：地図の頂点と辺の番号をでたらめに並べ替え、辺の向きも
/// でたらめに裏返した地図に、同じ編集（ブラシ・消しゴム・バケツ・頂点の移動・辺の引っぱり・アンカーの追加と削除・
/// 矩形で切る中と外）をしても、結果の地図（しまう形）と、面に記す番号と、書き戻しの差分が同じになる。新しい作りでは、
/// 窓ごとに番号の違う小さな地図で計算するので、結果が番号によらないことが前提になる
#[test]
fn edits_do_not_depend_on_edge_and_node_numbering() {
    use crate::planar_map::{LayerBase, LocalEdit, MapEdge, MapInput, MapRule, PlanarMap};
    use crate::stored_map::StoredMap;
    use std::collections::HashSet;

    /// 頂点と辺の番号を並べ替えた地図と、元の頂点 → 新しい頂点、元の辺 → 新しい辺（辺の向きは変えない）
    fn shuffled(m: &PlanarMap, rng: &mut map_overlay::Rng) -> (PlanarMap, Vec<usize>, Vec<usize>) {
        let perm = |n: usize, rng: &mut map_overlay::Rng| {
            let mut p: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                let j = (rng.f() * (i + 1) as f64) as usize % (i + 1);
                p.swap(i, j);
            }
            p
        };
        let node_to = perm(m.nodes.len(), rng);
        let mut nodes = vec![Point::ZERO; m.nodes.len()];
        for (v, &w) in node_to.iter().enumerate() {
            nodes[w] = m.nodes[v];
        }
        let edge_to = perm(m.edges.len(), rng);
        let mut edges = m.edges.clone();
        for (e, &f) in edge_to.iter().enumerate() {
            let ed = m.edges[e];
            edges[f] = MapEdge {
                a: node_to[ed.a],
                b: node_to[ed.b],
                ..ed
            };
        }
        (PlanarMap { nodes, edges }, node_to, edge_to)
    }

    /// 地図のしまう形（番号によらない形）。`fresh` でない番号はそのまま、`fresh` な番号（編集で新しく振った番号）は
    /// 無いことにする（新しく振る番号は回ごとに違うので、どの面が元の番号を引き継いだかだけを比べる）
    fn canonical(m: &PlanarMap, keep: &HashSet<u64>) -> StoredMap {
        StoredMap::from_dense(m, None).relabeled(
            |id| if keep.contains(&id) { id } else { 0 },
            &mut Default::default(),
        )
    }

    // 診断：形の違う辺の数、色・番号だけ違う辺の数、形の違いの大きさ
    fn diag(a: &PlanarMap, b: &PlanarMap) -> String {
        let key = |m: &PlanarMap, e: usize| {
            let ed = &m.edges[e];
            let (p, q) = (m.nodes[ed.a], m.nodes[ed.b]);
            let (p, q, l, r, ls, rs) = if (p.x, p.y) <= (q.x, q.y) {
                (p, q, ed.left, ed.right, ed.left_src, ed.right_src)
            } else {
                (q, p, ed.right, ed.left, ed.right_src, ed.left_src)
            };
            (
                [p.x, p.y, ed.ctrl.x, ed.ctrl.y, q.x, q.y].map(f64::to_bits),
                (l, r),
                (ls, rs),
            )
        };
        let ka: Vec<_> = (0..a.edges.len()).map(|e| key(a, e)).collect();
        let kb: Vec<_> = (0..b.edges.len()).map(|e| key(b, e)).collect();
        let ga: HashSet<_> = ka.iter().map(|k| k.0).collect();
        let gb: HashSet<_> = kb.iter().map(|k| k.0).collect();
        let only_a: Vec<_> = ga.difference(&gb).copied().collect();
        let only_b: Vec<_> = gb.difference(&ga).copied().collect();
        let col_a: HashSet<_> = ka.iter().map(|k| (k.0, k.1)).collect();
        let col_b: HashSet<_> = kb.iter().map(|k| (k.0, k.1)).collect();
        let col_diff = col_a
            .difference(&col_b)
            .filter(|k| gb.contains(&k.0))
            .count();
        let src_a: HashSet<_> = ka.iter().copied().collect();
        let src_diff = kb
            .iter()
            .filter(|k| !src_a.contains(k) && ga.contains(&k.0) && col_a.contains(&(k.0, k.1)))
            .count();
        // 形の違う辺どうしの、いちばん近い相手との差（端と制御点の最大）
        let f = |k: [u64; 6]| k.map(f64::from_bits);
        let mut worst = 0.0f64;
        for x in &only_a {
            let x = f(*x);
            let best = only_b
                .iter()
                .map(|y| {
                    let y = f(*y);
                    (0..6).map(|i| (x[i] - y[i]).abs()).fold(0.0, f64::max)
                })
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(best);
        }
        let ends_a: HashSet<_> = only_a.iter().map(|k| [k[0], k[1], k[4], k[5]]).collect();
        let ends_same = only_b
            .iter()
            .filter(|k| ends_a.contains(&[k[0], k[1], k[4], k[5]]))
            .count();
        format!("辺 {}/{} 形だけ違う {}/{}（うち端が同じで制御点だけ違う {}） 色だけ {} 番号だけ {} 形の差の最大 {:.3e}",
            a.edges.len(), b.edges.len(), only_a.len(), only_b.len(), ends_same, col_diff, src_diff, worst)
    }

    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let area = (100.0, 100.0, 300.0, 200.0);
    let (mut compared, mut differed) = (0usize, Vec::<String>::new());
    for seed in [23, 41, 59] {
        let mut rng = map_overlay::Rng(seed);
        let mut m = PlanarMap::default();
        for step in 0..300 {
            // 計算に渡す地図の辺は、いつもしまう形の決まった向き（今の本番の写し `StoredMap::to_dense` と同じ。新しい作りの
            // 窓も塊から作るので同じ）。辺の向きを裏返すと、曲線を切り出す・まとめる所の丸めで、制御点が最小の刻み1つ分
            // ずれることがある（形は地図の精度の中で同じ）ので、向きはそろえたまま番号だけを並べ替える
            m = LayerBase::new(&m).map().clone();
            let (s, node_to, edge_to) = shuffled(&m, &mut rng);
            let c = colors[step % 4];
            // 同じ編集を、元の地図 `m` と並べ替えた地図 `s` に
            type Edit<'a> = Box<dyn Fn(&PlanarMap, bool) -> Vec<Option<PlanarMap>> + 'a>;
            let kind = step % 8;
            let edit: Edit = match kind {
                0 | 1 => {
                    let r = 2.0 + rng.f() * 9.0;
                    let st = wiggly_stroke(&mut rng, area, r, c);
                    let rule = if kind == 0 {
                        MapRule::Paint(Some(c))
                    } else {
                        MapRule::Paint(None)
                    };
                    Box::new(move |x, _| vec![x.overlay(MapInput::Shapes(&st), rule)])
                }
                2 => {
                    // バケツ（1本の道 `fill_local`。地図はしまう形にしてから塗る）
                    let p = Point::new(100.0 + rng.f() * 300.0, 100.0 + rng.f() * 200.0);
                    Box::new(move |x, _| {
                        vec![match LayerBase::new(x).fill_local(p, c, &[]) {
                            LocalEdit::Changed(w) => Some(w.stored.to_dense()),
                            _ => None,
                        }]
                    })
                }
                3..=6 if !m.edges.is_empty() => {
                    let v = (rng.f() * m.nodes.len() as f64) as usize % m.nodes.len();
                    let e = (rng.f() * m.edges.len() as f64) as usize % m.edges.len();
                    let a = rng.f() * std::f64::consts::TAU;
                    let d = kurbo::Vec2::new(a.cos(), a.sin()) * (0.5 + rng.f() * 6.0);
                    let t = 0.2 + rng.f() * 0.6;
                    let (vs, es) = (node_to[v], edge_to[e]);
                    let mv = m.nodes[v] + d;
                    let pull = m.quad(e).eval(t) + d;
                    Box::new(move |x, shuf| {
                        let (v, e) = if shuf { (vs, es) } else { (v, e) };
                        // 輪郭の変形は本番の1本の道（`reshape_change` → `edit_local`。地図はしまう形にしてから編集する）
                        let reshape = |edit| match map_overlay::reshaped(&LayerBase::new(x), edit) {
                            Ok(Some(next)) => Some(next),
                            Ok(None) => Some(x.clone()),
                            Err(_) => None,
                        };
                        vec![match kind {
                            3 => reshape(crate::planar_map::ReshapeEdit::MoveVertex {
                                orig_vertex: x.nodes[v],
                                target: mv,
                            }),
                            4 => reshape(crate::planar_map::ReshapeEdit::PullEdge {
                                orig_seg: x.quad(e),
                                grab_t: t,
                                target: pull,
                                follow: true,
                            }),
                            5 => x.with_anchor_added(e, t),
                            _ => x.with_anchor_deleted(v),
                        }]
                    })
                }
                _ => {
                    let (x0, y0) = (100.0 + rng.f() * 150.0, 100.0 + rng.f() * 100.0);
                    let rect = vec![map_overlay::rect_shape(x0, y0, x0 + 80.0, y0 + 60.0, c)];
                    Box::new(move |x, _| {
                        match x.overlay_rules(
                            MapInput::Shapes(&rect),
                            [MapRule::KeepInside, MapRule::Paint(None)],
                        ) {
                            Some([inside, rest]) => vec![Some(inside), Some(rest)],
                            None => vec![None, None],
                        }
                    })
                }
            };
            let (rs, rt) = (edit(&m, false), edit(&s, true));
            // 元の地図の面の番号（編集のあとも残りうる番号）
            let keep: HashSet<u64> = m.labels().collect();
            let base = LayerBase::new(&m);
            // セル画に書き戻す差分（1本の道 `replace_local`）：（消す図形, 書き戻した地図）
            let write = |x: &PlanarMap| match base
                .replace_local(&StoredMap::from_dense(x, Some(&base.stored)), &[])
            {
                LocalEdit::Changed(w) => {
                    let mut removed = w.removed.clone();
                    removed.sort_unstable();
                    Some((removed, w.stored.to_dense()))
                }
                _ => None,
            };
            let mut next_m = None;
            for (i, (a, b)) in rs.into_iter().zip(rt).enumerate() {
                let (a, b) = match (a, b) {
                    (Some(a), Some(b)) => (a, b),
                    (None, None) => continue,
                    _ => {
                        differed.push(format!(
                            "seed {seed} step {step} kind {kind}: 片方だけ重ねられなかった"
                        ));
                        continue;
                    }
                };
                compared += 1;
                // 1. 重ね合わせの結果の地図
                if !canonical(&a, &keep).same_content(&canonical(&b, &keep)) {
                    differed.push(format!(
                        "seed {seed} step {step} kind {kind} [{i}]: 結果の地図が違う {}",
                        diag(&a, &b)
                    ));
                    continue;
                }
                if !a.is_valid() || !b.is_valid() {
                    continue;
                }
                // 2. 面に1つずつ別の番号を記した地図（セル画にしまう地図）
                let (la, lb) = (
                    a.clone().checked().unwrap().uniquely_labeled(),
                    b.clone().checked().unwrap().uniquely_labeled(),
                );
                if !canonical(la.map(), &keep).same_content(&canonical(lb.map(), &keep)) {
                    differed.push(format!(
                        "seed {seed} step {step} kind {kind} [{i}]: 面の番号の記し方が違う"
                    ));
                }
                // 3. セル画に書き戻す差分（変わらない面・番号を引き継ぐ面の決め方）
                let same_write = match (write(&a), write(&b)) {
                    (Some((ra, ma)), Some((rb, mb))) => {
                        ra == rb && canonical(&ma, &keep).same_content(&canonical(&mb, &keep))
                    }
                    (None, None) => true,
                    _ => false,
                };
                if !same_write {
                    differed.push(format!(
                        "seed {seed} step {step} kind {kind} [{i}]: 書き戻しの差分が違う"
                    ));
                }
                // 次の地図は最後の結果（矩形で切るときは外）
                next_m = Some(la.into_map());
            }
            if let Some(n) = next_m {
                m = n;
            }
        }
    }
    assert!(compared > 600, "比べた数 {compared}");
    assert!(
        differed.is_empty(),
        "番号の並べ方で結果が変わった（{} / {compared} 件）:\n{}",
        differed.len(),
        differed.join("\n")
    );
}

/// 段2b-5：前の地図に、わざと決まりを破りうる辺（ランダムな直線・曲線。ほかの辺と交わる・両側が同じ色・ほかの
/// 頂点と同じ格子の点に端がある、など）や、絵の無い所に向きをそろえた三角形を足した地図で、触った所だけで確かめた答え
/// （1本の道の差分から先 `apply_diff` が書けるか）が、新しい地図全体を確かめた答え（本番の判定 `check`：`is_valid`）と
/// 同じ。正しい答えと正しくない答えの両方が出ることを確かめる
#[test]
fn local_edge_rules_match_full_check() {
    use crate::planar_map::{LayerBase, LocalEdit, MapEdge, MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let mut rng = map_overlay::Rng(29);
    let mut m = PlanarMap::default();
    for k in 0..40 {
        let c = colors[k % 3];
        let r = 2.0 + rng.f() * 8.0;
        let st = wiggly_stroke(&mut rng, (100.0, 100.0, 300.0, 200.0), r, c);
        if let Some(n) = m
            .overlay(MapInput::Shapes(&st), MapRule::Paint(Some(c)))
            .and_then(|n| n.checked())
        {
            m = n.uniquely_labeled().into_map();
        }
    }
    let base = LayerBase::new(&m);
    let full_map = base.map().clone();
    let color = |rng: &mut map_overlay::Rng| Some(colors[(rng.f() * 3.0) as usize % 3]);
    let (mut good, mut bad) = (0, 0);
    let (mut edges_bad, mut cycles_bad, mut cycles_good) = (0, 0, 0);
    for _ in 0..400 {
        // 足す辺：（始点, 終点, 制御点, 左の色, 右の色）
        type Added = (Point, Point, Point, Option<Color32>, Option<Color32>);
        let mut adds: Vec<Added> = Vec::new();
        let empty = rng.f() < 0.4;
        if empty && rng.f() < 0.5 {
            // 絵の無い所（左の帯）に、向きをそろえた三角形（左が同じ色、右が何も無い）
            let c = color(&mut rng);
            let p: Vec<Point> = (0..3)
                .map(|_| Point::new(10.0 + rng.f() * 60.0, 100.0 + rng.f() * 200.0))
                .collect();
            for k in 0..3 {
                let (a, b) = (p[k], p[(k + 1) % 3]);
                adds.push((a, b, a.midpoint(b), c, None));
            }
        } else {
            let pick = |rng: &mut map_overlay::Rng| -> Point {
                if empty {
                    Point::new(10.0 + rng.f() * 60.0, 100.0 + rng.f() * 200.0)
                } else if rng.f() < 0.5 {
                    full_map.nodes
                        [(rng.f() * full_map.nodes.len() as f64) as usize % full_map.nodes.len()]
                } else {
                    Point::new(100.0 + rng.f() * 300.0, 100.0 + rng.f() * 200.0)
                }
            };
            let (a, b) = (pick(&mut rng), pick(&mut rng));
            if a == b {
                continue;
            }
            let ctrl = if rng.f() < 0.5 {
                a.midpoint(b)
            } else {
                a.midpoint(b) + KVec2::new(rng.f() * 20.0 - 10.0, rng.f() * 20.0 - 10.0)
            };
            let l = color(&mut rng);
            let r = if rng.f() < 0.15 {
                None
            } else {
                color(&mut rng)
            };
            adds.push((a, b, ctrl, l, r));
        }
        // 窓の結果：前の地図の全部の辺と、足す辺。新しい地図：同じ座標の頂点は1つにしたもの
        let mut result = full_map.clone();
        let mut joined = full_map.clone();
        let mut added: Vec<usize> = Vec::new();
        let at = |p: Point, map: &mut PlanarMap| -> usize {
            match map.nodes.iter().position(|&q| q == p) {
                Some(v) => v,
                None => {
                    map.nodes.push(p);
                    map.nodes.len() - 1
                }
            }
        };
        for &(a, b, ctrl, l, r) in &adds {
            let edge = |a: usize, b: usize| MapEdge {
                a,
                b,
                ctrl,
                left: l,
                right: r,
                left_src: None,
                right_src: None,
                stroke: None,
                line_id: None,
            };
            let (ra, rb) = (at(a, &mut result), at(b, &mut result));
            result.edges.push(edge(ra, rb));
            added.push(result.edges.len() - 1);
            let (ja, jb) = (at(a, &mut joined), at(b, &mut joined));
            joined.edges.push(edge(ja, jb));
        }
        let got = !matches!(
            base.apply_diff(&result, &[], &added, &[]),
            LocalEdit::Refused(_)
        );
        assert_eq!(got, joined.is_valid(), "足した辺 {adds:?}");
        // どの決まりで断ったか（1本の道が確かめる順：増えた辺の決まり → 触った輪郭の決まり → 穴の入れ先と何も無い側）
        let local = base.local_map(&result, &[], &added);
        let edges_ok = local.added_edges_ok();
        let cycles_ok = edges_ok
            && local
                .touched_cycles_ok(&mut crate::planar_map::CycleMemo::default())
                .is_some();
        assert!(
            edges_ok || !got,
            "増えた辺の決まりを破るのに書けた：足した辺 {adds:?}"
        );
        assert!(
            cycles_ok || !got,
            "輪郭の決まりを破るのに書けた：足した辺 {adds:?}"
        );
        match (edges_ok, cycles_ok) {
            (false, _) => edges_bad += 1,
            (true, false) => cycles_bad += 1,
            (true, true) => cycles_good += 1,
        }
        if got {
            good += 1;
        } else {
            bad += 1;
        }
    }
    // 正しい答えと正しくない答えが両方出て、増えた辺の決まりで断る場合と、辺の決まりは満たすが輪郭の決まりで断る場合と、
    // 輪郭の決まりまで満たす場合が、どれも出る
    assert!(good > 0 && bad > 10, "正しい {good}・正しくない {bad}");
    assert!(
        edges_bad > 10 && cycles_bad > 5 && cycles_good > 5,
        "辺の決まりで断る {edges_bad}・辺は正しいが輪郭で断る {cycles_bad}・輪郭まで正しい {cycles_good}"
    );
}

/// 段2b-5c：前の地図から辺をランダムに 1〜3 本消した地図（面の外周が開く・穴の入れ先が無くなる・何も無い側の輪郭が
/// 色のある面に入り込む、などが起きる）で、触った所だけで確かめた答え（1本の道の差分から先 `apply_diff` が書けるか）が、
/// 新しい地図全体を確かめた答え（`is_valid`）と同じ。正しい答えと正しくない答えの両方が出る
#[test]
fn local_check_matches_full_check_when_edges_are_removed() {
    use crate::planar_map::{LayerBase, LocalEdit, MapInput, MapRule, PlanarMap};
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
    ];
    let (mut good, mut bad) = (0, 0);
    for seed in [3u64, 17, 31] {
        let mut rng = map_overlay::Rng(seed);
        let mut m = PlanarMap::default();
        for k in 0..30 {
            let c = colors[k % 3];
            let r = 2.0 + rng.f() * 8.0;
            let st = wiggly_stroke(&mut rng, (100.0, 100.0, 300.0, 200.0), r, c);
            let rule = if k % 4 == 3 {
                MapRule::Paint(None)
            } else {
                MapRule::Paint(Some(c))
            };
            if let Some(n) = m
                .overlay(MapInput::Shapes(&st), rule)
                .and_then(|n| n.checked())
            {
                m = n.uniquely_labeled().into_map();
            }
        }
        let base = LayerBase::new(&m);
        let full_map = base.map().clone();
        let (faces, _) = full_map.faces();
        // 輪郭の辺の反対側がみな何も無い面（輪郭ごと消しても、ほかの面の色はそろったまま）と、穴を持つ面
        let none_other = |c: &Vec<(usize, bool)>| {
            c.iter().all(|&(e, f)| {
                let ed = full_map.edges[e];
                (if f { ed.right } else { ed.left }).is_none()
            })
        };
        let lonely: Vec<usize> = (0..faces.len())
            .filter(|&i| faces[i].cycles.iter().all(none_other))
            .collect();
        let holed: Vec<usize> = (0..faces.len())
            .filter(|&i| faces[i].cycles.len() > 1 && none_other(&faces[i].cycles[0]))
            .collect();
        for _ in 0..100 {
            let mode = rng.f();
            let mut removed: Vec<usize> = if mode < 0.35 && !lonely.is_empty() {
                // 周りが何も無い面を、輪郭ごと全部消す
                let f = &faces[lonely[(rng.f() * lonely.len() as f64) as usize % lonely.len()]];
                f.cycles.iter().flatten().map(|h| h.0).collect()
            } else if mode < 0.6 && !holed.is_empty() {
                // 穴を持つ面の外周だけを消す（穴の入れ先が無くなる）
                let f = &faces[holed[(rng.f() * holed.len() as f64) as usize % holed.len()]];
                f.cycles[0].iter().map(|h| h.0).collect()
            } else {
                let n = 1 + (rng.f() * 3.0) as usize;
                (0..n)
                    .map(|_| {
                        (rng.f() * full_map.edges.len() as f64) as usize % full_map.edges.len()
                    })
                    .collect()
            };
            removed.sort_unstable();
            removed.dedup();
            let mut joined = full_map.clone();
            let mut k = 0;
            joined.edges.retain(|_| {
                k += 1;
                !removed.contains(&(k - 1))
            });
            let got = !matches!(
                base.apply_diff(&full_map, &removed, &[], &[]),
                LocalEdit::Refused(_)
            );
            assert_eq!(got, joined.is_valid(), "seed {seed} 消した辺 {removed:?}");
            if got {
                good += 1;
            } else {
                bad += 1;
            }
        }
    }
    assert!(good > 5 && bad > 5, "正しい {good}・正しくない {bad}");
}

#[test]
fn test_micro_shapes_union_and_difference_on_map_at_16x_zoom() {
    let red = Color32::from_rgb(220, 50, 50);
    // 1600%（zoom = 16.0）拡大時の極小シェイプ同士を地図で重ねても（塗る / 消す）消失しないこと
    let micro_raw_a = brush_line_at_zoom(
        &[Point::new(100.0, 100.0), Point::new(100.3, 100.0)],
        0.12,
        16.0,
    );
    let micro_raw_b = brush_line_at_zoom(
        &[Point::new(100.15, 99.9), Point::new(100.15, 100.1)],
        0.08,
        16.0,
    );
    let micro_a = brush_line_shapes_at_zoom(&micro_raw_a, red, 16.0);
    let micro_b = brush_line_shapes_at_zoom(&micro_raw_b, Color32::BLUE, 16.0);
    assert!(!micro_a.is_empty() && !micro_b.is_empty());
    let area_a: f64 = micro_a.iter().map(|s| s.exterior().signed_area()).sum();
    let area_b: f64 = micro_b.iter().map(|s| s.exterior().signed_area()).sum();
    use crate::planar_map::{MapInput, MapRule};
    let map_a = crate::planar_map::painted_map_pub(&micro_a).unwrap();
    let faces_of = |m: &crate::planar_map::PlanarMap| -> Vec<FillShape> {
        m.faces()
            .0
            .iter()
            .map(|f| m.face_shape(f, 1, &[]))
            .collect()
    };
    let micro_union = faces_of(
        &map_a
            .overlay(MapInput::Shapes(&micro_b), MapRule::Paint(Some(red)))
            .unwrap(),
    );
    let area_union: f64 = micro_union.iter().map(|s| s.exterior().signed_area()).sum();
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
    let micro_diff = faces_of(
        &map_a
            .overlay(MapInput::Shapes(&micro_b), MapRule::Paint(None))
            .unwrap(),
    );
    let area_diff: f64 = micro_diff.iter().map(|s| s.exterior().signed_area()).sum();
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
    doc_actions::timeline_add_layer(&mut sort_app);
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
    sort_app.set_selected_indices(vec![0]);
    assert!(sort_app.session.selection().shapes.contains(&top_shape_id));
    put_cel_shapes(&mut sort_app, 0, 0, vec![shape_bottom]);
    assert_eq!(scene_summary(&sort_app).shapes[0].layer_id, lid_bottom);
    assert_eq!(scene_summary(&sort_app).shapes[1].layer_id, lid_top);
    assert!(sort_app.session.selection().shapes.contains(&top_shape_id));
    assert_eq!(
        sort_app.session.selected_indices(),
        vec![1],
        "下位レイヤーへの図形追加後も shape_id に基づいて選択インデックスが新しい位置へ追従すること"
    );

    // 空シンボルマーカーのデータモデル不変条件（contours=0, geometry_bounding_box=None, スナップ非対象）
    let empty_marker = FillShape::new_empty_symbol_marker(
        crate::symbol_binding::SymbolInstanceBinding::new(777, 1, kurbo::Affine::translate((50.0, 50.0))),
        lid_bottom,
        &[],
    );
    assert_eq!(empty_marker.contours().count(), 0);
    assert!(empty_marker.get_contour(0).is_none());
    assert!(empty_marker.geometry_bounding_box().is_none());
    // 空のシンボルのインスタンスを置く（画面の土台が、上の印と同じ空白のマーカーを作る）
    let empty_sym = sort_app.doc.register_new_symbol(Vec::new());
    sort_app.doc.root.layers[0].keyframe_mut(0).cel.instances_mut().push(timeline::SymbolInstance {
        binding: crate::symbol_binding::SymbolInstanceBinding::new(777, empty_sym, kurbo::Affine::translate((50.0, 50.0))),
        layer_id: lid_bottom,
        parent_scope: Vec::new(),
        order: 0,
    });
    sort_app.refresh_scene_base();
    assert!(
        sort_app.scene().iter().any(|s| s.symbol_instance.is_some_and(|b| b.instance_gid == 777) && s.contours().count() == 0),
        "空のシンボルのマーカーが画面にある"
    );
    let maps = crate::map_hover::scope_maps(sort_app.scene(), &sort_app.session);
    assert!(
        maps.maps
            .iter()
            .all(|m| m.base.map().nodes.iter().all(|p| (*p - Point::new(50.0, 50.0)).hypot() > 1.0)),
        "空白フレームのシンボルマーカーの基準点が、輪郭の変形の頂点（カーソル合わせでつかめる頂点）にならないこと"
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
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.0, Vec2::ZERO, false);
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
        let _ = crate::screen_mesh::shape_screen_mesh(s, 1.0, Vec2::ZERO, false);
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

    /// 同じ階層の生の図形が正しい地図になるか（重ならない）と、同じ色で接する図形が別々に残っていないか
    /// （地図の辺で、左右が同じ色のもの）。戻り値の2つ目は使わない（0）
    fn check_scope_invariants(canvas: &[FillShape]) -> (usize, f64, usize) {
        let map = crate::planar_map::PlanarMap::from_faces(
            canvas.iter().filter(|s| s.symbol_instance.is_none()),
        );
        let overlap_viol = usize::from(!map.is_valid());
        let unmerged_viol = map.edges.iter().filter(|e| e.left == e.right).count();
        (overlap_viol, 0.0, unmerged_viol)
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
            let raw = brush_line_at_zoom(&pts, 6.0, 1.0);
            let shapes = brush_line_shapes_at_zoom(&raw, colors[s_i % 3], 1.0);
            merge_shapes_into_canvas(&mut canvas, shapes);
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
        let raw = brush_line_at_zoom(&stroke_pts, radius, 1.0);
        let stroke_shapes = brush_line_shapes_at_zoom(&raw, brush_col, 1.0);
        if stroke_shapes.is_empty() {
            continue;
        }

        if is_eraser {
            erase_from_canvas(&mut canvas, &stroke_shapes, 1, &[]);
        } else {
            merge_shapes_into_canvas(&mut canvas, stroke_shapes.clone());
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
                .map(|s| s.color());
            let new_col = canvas
                .iter()
                .rev()
                .find(|s| s.contains_point(pt))
                .map(|s| s.color());
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
    // 地図で行う今の作りで 182,616 セグメント（ブール演算の作りでは 174,097。境目が頂点で区切られる分だけ多い）。
    // 段階1（d90cabe）の 182,462 より 137 多い：曲線に戻すかを判定の形（格子の上の折れ線）で決めるようになり、
    // 折れ線がほかの辺に触れる所（この試しで 193 か所。前は曲線にして、判定の形では触れる地図になっていた）を曲線にしない。
    // 08c15dd レビューの段階4でさらに 17 多い：置いた線のうち、前の線に触れない辺を切り直さずにそのまま写すようになり、
    // 1,050 通りのうち 5 通りで図形の分かれ方が変わった（−5〜+18）
    assert!(
        total_segs <= 182_616,
        "結果の輪郭のセグメント数が増えていないこと: {}",
        total_segs
    );
}
