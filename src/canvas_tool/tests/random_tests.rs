//! でたらめな操作・極端な値・長い試し・お手本と比べる試し（操作の名簿と、試しの道具の確かめを含む）

use super::*;

/// 絵として同じかを比べるための、文書の絵の要約（でたらめな操作の試しの、取り消しの1手にしない操作の確かめ）：タイムラインと
/// シンボルの、レイヤー・キーフレームごとの、面と線の色・線の太さ・外接矩形（絵の精度より粗い 1/20px に丸める）と、
/// インスタンスの置き方。図形の番号と、絵の精度より細かい座標の違い（一部だけ確定したときの、格子にそろえる計算の順の
/// 違い）は見ない
fn picture_summary_of(doc: &symbol::Document) -> Vec<String> {
    let r = |v: f64| (v * 20.0).round() as i64;
    let mut out = Vec::new();
    let mut timeline = |name: String, tl: &timeline::Timeline| {
        for (li, layer) in tl.layers.iter().enumerate() {
            for (ki, kf) in layer.keyframes.iter().enumerate() {
                let mut items: Vec<String> = Vec::new();
                for m in kf.cel.maps().iter() {
                    for f in m.base().drawn_shapes(&m.group_path) {
                        let b = f.bounding_box();
                        items.push(format!(
                            "{:?} {:?} {:?} {} {} {} {}",
                            m.group_path,
                            f.color(),
                            f.line.map(|l| (l.color, l.width)),
                            r(b.x0),
                            r(b.y0),
                            r(b.x1),
                            r(b.y1)
                        ));
                    }
                }
                for i in kf.cel.instances().iter() {
                    let c = i.binding.xform.as_coeffs().map(r);
                    items.push(format!("inst {} {:?} {:?} {:?}", i.binding.symbol_id, i.parent_scope, i.order, c));
                }
                items.sort();
                out.push(format!("{name} L{li} K{ki} start{} {items:?}", layer.start_of(ki)));
            }
        }
    };
    timeline("root".into(), &doc.root);
    for sym in symbols_in_order(doc.library()) {
        timeline(format!("sym{} {}", sym.id, sym.name), &sym.timeline);
    }
    out
}

/// 2つの文書 `a`・`b` の絵が、地図の精度の内で同じか（`picture_summary_of` が違ったときに使う詳しい比べ）。塗りとインスタンスは
/// 要約（外枠は 1/20px）がそのまま同じ。線は、レイヤー・キーフレーム・階層・色ごとに、片方の線の上の点（およそ 1px ごと）を
/// もう片方の同じ太さの線が 1/20px 以内で通る。太さだけ違ってよいのは、元の絵のその点の近くに、その太さの同じ色の線が
/// もう1本あったときだけ（ほとんど重なった2本が、別々に重ねるとき格子の目より近い所で1本にまとまる：作りの精度の内。持ち主の
/// 了解：2026-11-28）。`Err`：違う所
fn same_picture_within_precision(a: &symbol::Document, b: &symbol::Document) -> Result<(), String> {
    use kurbo::ParamCurveArclen;
    const TOL: f64 = 0.05;
    type Lines = Vec<(kurbo::QuadBez, u8)>;
    // 塗り・インスタンスの要約と、色ごとの線
    let parts = |doc: &symbol::Document| {
        let r = |v: f64| (v * 20.0).round() as i64;
        let mut fills: Vec<String> = Vec::new();
        let mut lines: std::collections::BTreeMap<String, Lines> = std::collections::BTreeMap::new();
        let mut timeline = |name: String, tl: &timeline::Timeline| {
            for (li, layer) in tl.layers.iter().enumerate() {
                for (ki, kf) in layer.keyframes.iter().enumerate() {
                    for m in kf.cel.maps().iter() {
                        let base = m.base();
                        for f in base.drawn_shapes(&m.group_path).iter().filter(|f| f.line.is_none()) {
                            let bb = f.bounding_box();
                            fills.push(format!("{name} L{li} K{ki} {:?} {:?} {} {} {} {}", m.group_path, f.color(), r(bb.x0), r(bb.y0), r(bb.x1), r(bb.y1)));
                        }
                        let map = base.map();
                        for (e, ed) in map.edges.iter().enumerate() {
                            if let Some(st) = ed.stroke {
                                lines.entry(format!("{name} L{li} K{ki} {:?} {:?}", m.group_path, st.color)).or_default().push((map.quad(e), st.width));
                            }
                        }
                    }
                    for i in kf.cel.instances().iter() {
                        fills.push(format!("{name} L{li} K{ki} inst {} {:?} {:?} {:?}", i.binding.symbol_id, i.parent_scope, i.order, i.binding.xform.as_coeffs().map(r)));
                    }
                }
            }
        };
        timeline("root".into(), &doc.root);
        for sym in symbols_in_order(doc.library()) {
            timeline(format!("sym{}", sym.id), &sym.timeline);
        }
        fills.sort();
        (fills, lines)
    };
    let ((fa, la), (fb, lb)) = (parts(a), parts(b));
    if fa != fb {
        return Err("塗りかインスタンスが違う".into());
    }
    let near = |lines: &Lines, p: Point, width: Option<u8>| {
        lines.iter().any(|(q, w)| width.is_none_or(|x| x == *w) && crate::vector_engine::nearest_on_quad(q, p).1 <= TOL * TOL)
    };
    // `from` の線の点が、`to` の線で覆われるか（太さの違いは、`from` 自身にその太さの線が近くにあるときだけ許す）
    let covered = |from: &Lines, to: &Lines, key: &str| -> Result<(), String> {
        for (q, w) in from {
            let n = (q.arclen(1e-6).ceil() as usize).max(2);
            for k in 0..=n {
                let p = q.eval(k as f64 / n as f64);
                if near(to, p, Some(*w)) {
                    continue;
                }
                let merged = to.iter().any(|(tq, tw)| crate::vector_engine::nearest_on_quad(tq, p).1 <= TOL * TOL && near(from, p, Some(*tw)));
                if !merged {
                    return Err(format!("{key}：太さ {w} の線の ({:.2}, {:.2}) が無い", p.x, p.y));
                }
            }
        }
        Ok(())
    };
    let empty = Lines::new();
    for key in la.keys().chain(lb.keys()) {
        let (x, y) = (la.get(key).unwrap_or(&empty), lb.get(key).unwrap_or(&empty));
        covered(x, y, key)?;
        covered(y, x, key)?;
    }
    Ok(())
}

/// 種類を並べた一覧（道具 `Tool::ALL`・右のパネル `RightPanelTab::ALL`・メニュー `Menu::ALL`・拡大のつまみ
/// `ScaleHandle::ALL`）は、定義の種類を全部、定義の順に含む（メニューのアプリの項目 `Menu::App` だけは、わざと外す：
/// Mac では OS が先頭に置き、ほかでは「ファイル」に入れる）。種類を足して一覧に足し忘れると、道具のボタンやキーから黙って
/// 消えるので、ここで止める（約束の棚卸し 5616fcc の C の E4）
#[test]
fn every_kind_list_holds_all_kinds_in_order() {
    fn listed<T: std::fmt::Debug>(all: &[T]) -> Vec<String> {
        all.iter().map(|t| format!("{t:?}")).collect()
    }
    let canvas = include_str!("../../canvas_tool.rs");
    let cases = [
        ("Tool", enum_variant_names(canvas, "pub(crate) enum Tool {"), listed(&Tool::ALL)),
        (
            "ScaleHandle",
            enum_variant_names(canvas, "pub(crate) enum ScaleHandle {"),
            listed(&crate::canvas_tool::ScaleHandle::ALL),
        ),
        (
            "RightPanelTab",
            enum_variant_names(include_str!("../../panels.rs"), "pub(crate) enum RightPanelTab {"),
            listed(&crate::panels::RightPanelTab::ALL),
        ),
        (
            "Menu",
            enum_variant_names(include_str!("../../menu.rs"), "pub(crate) enum Menu {")
                .into_iter()
                .filter(|m| m != "App")
                .collect(),
            listed(&crate::menu::Menu::ALL),
        ),
    ];
    for (label, defined, listed) in cases {
        assert_eq!(listed, defined, "{label} の一覧が定義の種類と違う");
    }
}

/// 絵を地図の精度の内で比べる関数（`same_picture_within_precision`）は、消えた線と、まとまったのでない太さの違いを見逃さない
#[test]
fn same_picture_within_precision_catches_a_lost_or_thinner_line() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let with = |width: u8| {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![map_overlay::rect_shape(c.x - 100.0, c.y - 60.0, c.x + 100.0, c.y + 60.0, Color32::RED)]);
        assert!(apply(&mut app, Action::PencilStroke {
            points: vec![Point::new(c.x - 80.0, c.y), Point::new(c.x + 80.0, c.y)],
            stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width },
        }));
        app.doc.clone()
    };
    let mut none = Is2dApp::default();
    put_cel_shapes(&mut none, 0, 0, vec![map_overlay::rect_shape(c.x - 100.0, c.y - 60.0, c.x + 100.0, c.y + 60.0, Color32::RED)]);
    assert!(same_picture_within_precision(&with(3), &with(3)).is_ok());
    assert!(same_picture_within_precision(&with(3), &none.doc).is_err(), "消えた線を見逃した");
    assert!(same_picture_within_precision(&with(3), &with(5)).is_err(), "太さの違いを見逃した");
}

#[test]
fn test_degenerate_nan_infinite_stroke_and_affine_inputs_do_not_panic() {
    use crate::vector_engine::union_bounding_boxes;
    use kurbo::{Affine, Rect as KRect};

    let red = Color32::from_rgb(220, 60, 60);
    assert!(brush_line_shapes(&brush_line(&[], 8.0), red).is_empty());
    assert!(brush_line_shapes(&brush_line(&[Point::new(f64::NAN, 10.0)], 8.0), red).is_empty());
    assert!(brush_line_shapes(&brush_line(&[Point::new(10.0, 10.0)], f64::NAN), red).is_empty());
    assert!(
        brush_line_shapes(&brush_line(&[Point::new(f64::INFINITY, 0.0); 4], 8.0), red).is_empty()
    );
    assert_eq!(union_bounding_boxes(&[]), KRect::ZERO);

    let dot_outline = brush_line(&[Point::new(100.0, 100.0)], 6.0);
    let dot_shapes = brush_line_shapes(&dot_outline, red);
    assert_eq!(dot_shapes.len(), 1);

    let mut shape = dot_shapes[0].clone();
    let orig_bbox = shape.bounding_box();
    shape.translate(KVec2::new(f64::NAN, f64::INFINITY));
    shape.transform(Affine::new([f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]));
    assert_eq!(
        shape.bounding_box(),
        orig_bbox,
        "数が有限でない移動・変換はかけない"
    );
    // 潰れた変換は（有限なら）かける：面積 0 の図形になる（R7）
    shape.transform(Affine::scale(0.0));
    assert_eq!(shape.bounding_box(), KRect::ZERO);
    assert!(!shape.contains_point(Point::new(f64::NAN, f64::NAN)));
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
        let raw_outline = brush_line(&pts, radius);
        let shapes_a = brush_line_shapes(&raw_outline, col_a);
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
        let mut unioned = shapes_a.clone();
        merge_shapes_into_canvas(&mut unioned, shapes_b);
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
                                    "order": 0
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

        let doc: SavedProject = serde_json::from_str(&raw_json)
            .expect("有効なJSON構文の ProjectDocument がパースできること");
        // 読み込みの道（`into_document_noting`）が直した作品（ぐるぐる回る作品は、開く入口が断る：`open_document_in_new_tab`）
        let loaded = doc.into_document().expect("作品にできること");

        assert!(
            !loaded.root.layers.is_empty(),
            "case {case_idx}: 空レイヤー配列でも最低1レイヤーに補正されること"
        );
        assert!(
            (1..=60).contains(&loaded.fps),
            "case {case_idx}: FPS が 1..=60 にクランプされること"
        );
        assert!(
            loaded.root.current_frame < loaded.root.total_frames,
            "case {case_idx}: current_frame が total_frames 未満に収まること"
        );
        let mut has_cycle = false;
        for sym in symbols_in_order(loaded.library()) {
            assert!(
                sym.id >= 1,
                "case {case_idx}: シンボルIDが 1 以上に正規化されること"
            );
            for inst in sym.timeline.instances() {
                if loaded
                    .would_create_symbol_cycle(sym.id, inst.binding.symbol_id)
                {
                    has_cycle = true;
                }
            }
        }
        if has_cycle {
            assert!(
                loaded.symbol_nest_exceeded(),
                "case {case_idx}: 循環参照を含む場合は symbol_nest_exceeded() が true になること"
            );
        }
    }
}

/// でたらめな操作の試しが、`Action` の全部の種類を作れる（種類を足したら、試しにも足さないと止まる）
#[test]
fn random_actions_cover_every_action_kind() {
    let names = action_kind_names();
    assert!(names.len() > 50, "Action の種類を読めていない：{names:?}");
    let app = Is2dApp::default();
    let mut rng = OpRng(7);
    for name in &names {
        let a = random_action(name, &mut rng, &app);
        // 状態から作れないときに別の種類で代わりにする所（輪郭が無いとき）は、図形を描いた作品で確かめる
        if action_kind_name(&a) != *name {
            let mut drawn = Is2dApp::default();
            put_cel_shapes(&mut drawn, 0, 0, vec![map_overlay::rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
            let b = random_action(name, &mut rng, &drawn);
            assert_eq!(action_kind_name(&b), *name, "random_action が種類 {name} を作らない");
        }
    }
}

/// でたらめな操作の試し：`Action` の全部の種類を、でたらめな順と引数で `apply` に通し、毎手、次を確かめる（設計
/// claude/約束を入口で守る設計_5616fcc.md の 7.。前は手で並べた 26 通りだけだったので、インクボトルの書き忘れ（審査 5616fcc の
/// D-1）をすり抜けた）：
/// 1. `apply` のあと、操作の前の写し（`before`）は残らない
/// 2. 断った（`false`）ら、作品の中身・取り消しとやり直しの数・未保存の印は変わらない
/// 3. いつも保存できる（`ProjectDocument::from_app` が作れる：浮いた片を確定できる）
/// 4. 取り消しの1手になる操作（操作ごとの表 `Action::policy`）は、作品の中身（今のコマ・選んでいるレイヤーを除く）が変わったら、
///    取り消しが1手増え（まとめ中は増えない）、未保存の印が付く。作品の中身も浮いた片も変わらなければ、取り消しは増えない
/// 5. 取り消しの1手にしない操作（選ぶ・見る場所を変えるなど）は、絵を変えず、取り消しを積まない
/// 6. 取り消し・やり直しは、できるときは数が1つずつ動き、できないときは何も変えない
#[test]
fn test_m5_1_random_actions_undo_and_dirty_invariants_3_seeds() {
    use crate::save_format::ProjectDocument;
    use std::collections::BTreeMap;

    let names = action_kind_names();
    let drawing = ["PaintStroke", "PencilStroke", "BucketFill", "InkBottle", "SelectMarquee", "FinishMoveSelectionDrag"];
    let saved = |app: &Is2dApp, ctx: &str| -> ProjectDocument {
        match saved_from_app(app.tab()) {
            Ok(pd) => pd,
            Err(e) => panic!("{ctx}: 保存できない（{e}）"),
        }
    };
    let content = |pd: &ProjectDocument| -> String {
        let mut pd = pd.clone();
        pd.timeline.active_layer = 0;
        pd.timeline.current_frame = 0;
        for sym in &mut pd.library {
            sym.timeline.active_layer = 0;
            sym.timeline.current_frame = 0;
        }
        content_json(&pd)
    };
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    // 種の数は環境変数 `IS2D_RANDOM_SEEDS` で増やせる（ふだんは3つ）
    let extra: u64 = std::env::var("IS2D_RANDOM_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(3).max(3) - 3;
    let mut seeds = vec![0x1234_5678_9abc_def0u64, 0xfedc_ba98_7654_3210, 0x2026_1103_cafe_babe];
    seeds.extend((1..=extra).map(|i| 0x1234_5678_9abc_def0u64.wrapping_add(i * 0x9e37_79b9_7f4a_7c15)));
    for &seed in &seeds {
        let mut rng = OpRng(seed);
        let mut app = Is2dApp::default();
        for step in 0..1500 {
            let kind = if rng.range(10) < 3 { drawing[rng.range(drawing.len())].to_owned() } else { names[rng.range(names.len())].clone() };
            let action = random_action(&kind, &mut rng, &app);
            let op = action_kind_name(&action);
            *used.entry(op.clone()).or_insert(0) += 1;
            let ctx = format!("seed={seed:#x} step={step} op={op}");
            let before = content(&saved(&app, &ctx));
            let before_undo = app.tab().history.undo_stack.len();
            let before_redo = app.tab().history.redo_stack.len();
            let before_dirty = app.tab().history.is_dirty();
            let before_floating = app.session.floating().cloned();
            let coalesce = match action.policy(app.tab()).undo {
                crate::canvas_tool::doc_actions::UndoUse::Coalesce(key) => Some(key),
                _ => None,
            };
            let was_coalesced = coalesce.is_some() && app.tab().history.last_coalesce_key == coalesce;
            let is_history = matches!(action, Action::Undo | Action::Redo);
            let not_a_step = action.policy(app.tab()).undo == crate::canvas_tool::doc_actions::UndoUse::None;
            // 選択の一部を外す（その部分だけ確定する）操作も、線と塗りの両方を比べる（一部を確定しても見えている重なりを
            // 変えない：外した線は、持ったままの塗りの上に重ねる線 `PieceGroup::lines_over`。前は外した線が塗りの下に入り、
            // あとで塗りを確定すると消えたので、この操作のあとは線を比べから外していた：設計審査 b7208d9 の 9.）
            let doc_before = app.tab().committed_document_copy();
            let picture_before = doc_before.as_ref().map(picture_summary_of);
            let res = apply(&mut app, action);
            assert!(app.tab().history.before.is_none(), "{ctx}: 操作の前の写しが残った");
            // ライブラリの表（見せる順・名前・使われ方・子・音・高さ・動くか）は、全部から作り直したもの（お手本）と同じ
            if let Err(e) = app.tab().doc().library().check_tables() {
                panic!("{ctx}: ライブラリの表がお手本と違う：{e}");
            }
            let after = content(&saved(&app, &ctx));
            let after_undo = app.tab().history.undo_stack.len();
            let after_redo = app.tab().history.redo_stack.len();
            let after_dirty = app.tab().history.is_dirty();
            if is_history {
                if res {
                    let (fewer, more) = if op == "Undo" { (before_undo, before_redo) } else { (before_redo, before_undo) };
                    let (now_fewer, now_more) = if op == "Undo" { (after_undo, after_redo) } else { (after_redo, after_undo) };
                    assert_eq!((now_fewer + 1, now_more), (fewer, more + 1), "{ctx}: 取り消し・やり直しの数");
                } else {
                    assert_eq!((after_undo, after_redo, &after), (before_undo, before_redo, &before), "{ctx}: できない取り消し・やり直しが何かを変えた");
                }
                continue;
            }
            if !res {
                assert_eq!(after, before, "{ctx}: 断ったのに作品の中身が変わった");
                assert_eq!((after_undo, after_redo, after_dirty), (before_undo, before_redo, before_dirty), "{ctx}: 断ったのに取り消し・未保存の印が変わった");
                continue;
            }
            if not_a_step {
                // 取り消しの1手にしない操作（選ぶ・見る場所を変えるなど）は、絵を変えない（持ち上げた絵の確定は、保存する絵と
                // 同じ絵を書く。一部だけ確定すると、図形の番号の振り方と、絵の精度より細かい座標が、一度に確定したときと違う
                // ことがあるので、絵として比べる：`picture_summary`）
                let doc_after = app.tab().committed_document_copy();
                let picture_after = doc_after.as_ref().map(picture_summary_of);
                if picture_after != picture_before {
                    let (Some(a), Some(b)) = (&doc_before, &doc_after) else {
                        panic!("{ctx}: 取り消しの1手にしない操作が、置ける・置けないを変えた");
                    };
                    if let Err(e) = same_picture_within_precision(a, b) {
                        panic!("{ctx}: 取り消しの1手にしない操作が絵を変えた：{e}");
                    }
                }
                assert_eq!(after_undo, before_undo, "{ctx}: 取り消しの1手にしない操作が取り消しを積んだ");
                continue;
            }
            if after != before {
                let expected = if was_coalesced { before_undo } else { (before_undo + 1).min(100) };
                assert_eq!(after_undo, expected, "{ctx}: 中身が変わったのに取り消しが1手増えない");
                assert!(after_dirty, "{ctx}: 中身が変わったのに未保存の印が付かない");
            } else if match (&before_floating, app.session.floating()) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_content(b),
                _ => false,
            } {
                assert_eq!(after_undo, before_undo, "{ctx}: 何も変わらないのに取り消しが1手増えた");
            }
        }
    }
    let unused: Vec<&String> = names.iter().filter(|n| !used.contains_key(*n)).collect();
    assert!(unused.is_empty(), "試していない種類：{unused:?}");
}

/// ランダムな操作の長いテストの、1回ごとの確かめ：どのレイヤー・キーフレームのセル画の地図も正しく（保存して開いても
/// 読める）、面の表の図形の輪郭の辺がみなしまう形にあってその面の側にその面の番号が記してあり、面の表が地図全体から
/// 作った面の表と同じ（番号・色・輪郭がビットまで）
fn check_maps_after_step(app: &Is2dApp, tag: &str) {
    use crate::planar_map::LayerBase;
    let key = |s: &FillShape| {
        let bits: Vec<u64> = s
            .contours()
            .flat_map(|c| c.segments.iter())
            .flat_map(|q| [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y].map(f64::to_bits))
            .collect();
        (s.shape_id, s.color().to_array(), bits)
    };
    for layer in &app.tab().active_timeline().layers {
        for kf in &layer.keyframes {
            let cel = &kf.cel;
            assert_eq!(cel.maps_problem(), None, "{tag}: 地図が正しくない");
            for m in cel.maps().iter() {
                let base = m.base();
                let faces = base.faces(&m.group_path);
                for f in faces.iter() {
                    for q in f.contours().flat_map(|c| c.segments.iter()) {
                        let (e, fwd) = base.stored.find_edge(q).unwrap_or_else(|| {
                            panic!("{tag}: 面 {} の輪郭の辺が地図に無い", f.shape_id)
                        });
                        let te = base.stored.edge_at(e);
                        let side = if fwd { te.left_src } else { te.right_src };
                        assert_eq!(side, Some(f.shape_id), "{tag}: 面の表と辺の番号が食い違う");
                    }
                }
                let fresh = LayerBase::new(&m.stored().to_dense());
                let a: Vec<_> = faces.iter().map(|s| key(s)).collect();
                let b: Vec<_> = fresh.faces(&m.group_path).iter().map(|s| key(s)).collect();
                assert!(
                    a == b,
                    "{tag}: 面の表が、地図全体から作った面の表と違う（階層 {:?}）",
                    m.group_path
                );
            }
        }
    }
}

/// 画面の図形の一覧（差し替えで作り直してきたもの）が、全部作り直した一覧と同じか（番号・レイヤー・色・輪郭）
fn check_scene_matches_full_rebuild(app: &mut Is2dApp, tag: &str) {
    let snap = |app: &Is2dApp| -> Vec<(u64, u64, [u8; 4], Vec<u64>)> {
        app.scene()
            .iter()
            .map(|s| {
                let bits: Vec<u64> = s
                    .contours()
                    .flat_map(|c| c.segments.iter())
                    .flat_map(|q| {
                        [q.p0.x, q.p0.y, q.p1.x, q.p1.y, q.p2.x, q.p2.y].map(f64::to_bits)
                    })
                    .collect();
                (s.shape_id, s.layer_id, s.color().to_array(), bits)
            })
            .collect()
    };
    let before = snap(app);
    let lids: Vec<u64> = app
        .tab()
        .active_timeline()
        .layers
        .iter()
        .map(|l| l.id)
        .collect();
    crate::canvas_tool::finish_cel_writes(app.tab_mut(), &lids);
    assert!(
        before == snap(app),
        "{tag}: 画面の図形の一覧が、全部作り直した一覧と違う"
    );
}

/// ランダムな操作を `steps` 回続け、1回ごとに地図の決まり（`check_maps_after_step`）を、10回に1回画面の図形の一覧
/// （`check_scene_matches_full_rebuild`）を確かめる。操作の答えは、テストのときは毎回お手本（同じ道に地図全体を渡した答え）と比べられている。
/// 途中で止まったら、それまでの操作の並び（終わりの 30 個）を出す
fn random_operation_marathon(seed: u64, steps: usize) {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let mut app = map_overlay::dense_scene_app(seed);
    app.tab_mut().refresh_scene_base();
    let mut rng = map_overlay::Rng(seed ^ 0x5eed);
    let colors = [
        Color32::from_rgb(235, 75, 75),
        Color32::from_rgb(55, 135, 245),
        Color32::from_rgb(40, 180, 90),
        Color32::from_rgb(240, 200, 40),
    ];
    let mut log: Vec<String> = Vec::new();
    // 操作の種類ごとの（試した数, 効いた数）
    let mut tally: std::collections::BTreeMap<&'static str, (usize, usize)> = Default::default();
    for step in 0..steps {
        let pt = |rng: &mut map_overlay::Rng| {
            Point::new(-390.0 + rng.f() * 780.0, -215.0 + rng.f() * 430.0)
        };
        // 今の画面の図形の、輪郭の上の点（アンカー・頂点・辺の操作の場所）。`min_len`：辺の操作は、頂点に吸い付かない
        // 長さ（弦の長さ）の辺を選ぶ
        // `top_only`：いちばん上のレイヤーの図形だけ（下のレイヤーの所は上の図形に隠れて、カーソルが合わないことが多い）
        let on_outline = |app: &Is2dApp,
                          rng: &mut map_overlay::Rng,
                          vertex: bool,
                          min_len: f64,
                          top_only: bool|
         -> Option<Point> {
            let top = app.tab().active_timeline().layers.last().map(|l| l.id);
            let shapes: Vec<&FillShape> = app
                .scene()
                .iter()
                .filter(|s| !top_only || Some(s.layer_id) == top)
                .collect();
            if shapes.is_empty() {
                return None;
            }
            let s = shapes[(rng.f() * shapes.len() as f64) as usize % shapes.len()];
            let segs: Vec<kurbo::QuadBez> = s
                .contours()
                .flat_map(|c| c.segments.iter().copied())
                .filter(|q| (q.p2 - q.p0).hypot() >= min_len)
                .collect();
            let q = *segs.get((rng.f() * segs.len() as f64) as usize % segs.len().max(1))?;
            Some(if vertex { q.p0 } else { q.eval(0.5) })
        };
        let kind = (rng.f() * 22.0) as usize;
        let action = match kind {
            0..=2 => {
                let p0 = pt(&mut rng);
                let n = 2 + (rng.f() * 4.0) as usize;
                let points = (0..n)
                    .map(|_| p0 + kurbo::Vec2::new(rng.f() * 120.0 - 60.0, rng.f() * 80.0 - 40.0))
                    .collect();
                Op::Do(Action::PaintStroke {
                    points,
                    radius: 2.0 + rng.f() as f32 * 9.0,
                    color: colors[(rng.f() * 4.0) as usize % 4],
                })
            }
            3 => {
                let p0 = pt(&mut rng);
                Op::Do(Action::EraseStroke {
                    points: vec![
                        p0,
                        p0 + kurbo::Vec2::new(rng.f() * 80.0 - 40.0, rng.f() * 50.0 - 25.0),
                    ],
                    radius: 2.0 + rng.f() as f32 * 8.0,
                })
            }
            4 => Op::Do(Action::BucketFill {
                pt: pt(&mut rng),
                color: colors[(rng.f() * 4.0) as usize % 4],
            }),
            5 => match on_outline(&app, &mut rng, false, 0.0, false) {
                Some(p) => Op::Do(Action::AddAnchor { pt: p }),
                None => Op::Do(Action::Deselect),
            },
            6 => match on_outline(&app, &mut rng, true, 0.0, false) {
                Some(p) => Op::Do(Action::DeleteAnchor { pt: p }),
                None => Op::Do(Action::Deselect),
            },
            7 => match on_outline(&app, &mut rng, true, 0.0, false) {
                Some(p) => Op::MoveVertex(p, p + kurbo::Vec2::new(rng.f() * 30.0 - 15.0, rng.f() * 30.0 - 15.0)),
                None => Op::Do(Action::Deselect),
            },
            8 => match on_outline(&app, &mut rng, false, 20.0, true) {
                Some(p) => Op::PullEdge(p, p + kurbo::Vec2::new(rng.f() * 30.0 - 15.0, rng.f() * 30.0 - 15.0)),
                None => Op::Do(Action::Deselect),
            },
            9 | 10 => {
                let p0 = pt(&mut rng);
                Op::Do(Action::SelectMarquee {
                    p0,
                    p1: p0 + kurbo::Vec2::new(10.0 + rng.f() * 200.0, 10.0 + rng.f() * 120.0),
                    additive: rng.f() < 0.25,
                })
            }
            11 => Op::Do(Action::SelectSingleAt { pt: pt(&mut rng) }),
            12 => Op::Do(Action::NudgeSelection {
                delta: kurbo::Vec2::new(rng.f() * 60.0 - 30.0, rng.f() * 60.0 - 30.0),
            }),
            13 => match (rng.f() * 3.0) as usize {
                0 => Op::Scale(0.6 + rng.f() * 0.9, 0.6 + rng.f() * 0.9),
                1 => Op::Do(Action::RotateSelection {
                    angle_rad: rng.f() * 1.5 - 0.75,
                }),
                _ => Op::Do(Action::FlipSelection {
                    flip_x: rng.f() < 0.5,
                    flip_y: rng.f() < 0.5,
                }),
            },
            14 => Op::Do(Action::DeleteSelection),
            15 => {
                // 先にクリックで1つ選んでから（グループ化は、選んだものが1つのレイヤーだけのときに効く。解除は、選んだ
                // 図形がグループの中なら効く）
                let pre = Action::SelectSingleAt { pt: pt(&mut rng) };
                let pre_is_marquee = rng.f() < 0.5;
                log.push(format!("{step}（前）: {pre:?}"));
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(&mut app, pre)))
                        .map_err(|e| {
                            let from = log.len().saturating_sub(30);
                            eprintln!(
                                "--- seed {seed} step {step}（前）で止まった。それまでの操作:"
                            );
                            for l in &log[from..] {
                                eprintln!("{l}");
                            }
                            std::panic::resume_unwind(e)
                        });
                if pre_is_marquee {
                    Op::Do(Action::Group)
                } else {
                    Op::Do(Action::Ungroup)
                }
            }
            16 => {
                if rng.f() < 0.5 {
                    Op::Do(Action::Copy)
                } else {
                    Op::Do(Action::Paste {
                        in_place: rng.f() < 0.5,
                    })
                }
            }
            17 | 18 => Op::Do(Action::Undo),
            19 => Op::Do(Action::Redo),
            20 => Op::Do(Action::SwitchLayerAndFrame {
                layer_idx: (rng.f() * 3.0) as usize % 3,
                frame: 0,
            }),
            _ => Op::Do(Action::Deselect),
        };
        log.push(format!("{step}: {action:?}"));
        let name: &'static str = match &action {
            Op::MoveVertex(..) => "頂点",
            Op::PullEdge(..) => "引っぱり",
            Op::Scale(..) => "変形",
            Op::Do(a) => match a {
                Action::PaintStroke { .. } => "ブラシ",
                Action::EraseStroke { .. } => "消しゴム",
                Action::BucketFill { .. } => "バケツ",
                Action::AddAnchor { .. } => "アンカー+",
                Action::DeleteAnchor { .. } => "アンカー-",
                Action::SelectMarquee { .. } => "矩形",
                Action::SelectSingleAt { .. } => "クリック選択",
                Action::NudgeSelection { .. } => "動かす",
                Action::RotateSelection { .. } | Action::FlipSelection { .. } => "変形",
                Action::DeleteSelection => "削除",
                Action::Group => "グループ化",
                Action::Ungroup => "解除",
                Action::Copy => "コピー",
                Action::Paste { .. } => "貼り付け",
                Action::Undo => "取り消し",
                Action::Redo => "やり直し",
                Action::SwitchLayerAndFrame { .. } => "レイヤー",
                _ => "確定・選択解除",
            },
        };
        let tag = format!("seed {seed} step {step}");
        let mut applied = false;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            applied = op(&mut app, action);
            check_maps_after_step(&app, &tag);
            if step % 10 == 9 {
                check_scene_matches_full_rebuild(&mut app, &tag);
            }
        }));
        if let Err(e) = result {
            let from = log.len().saturating_sub(30);
            eprintln!("--- {tag} で止まった。それまでの操作（終わりの 30 個）:");
            for l in &log[from..] {
                eprintln!("{l}");
            }
            std::panic::resume_unwind(e);
        }
        let t = tally.entry(name).or_default();
        t.0 += 1;
        t.1 += usize::from(applied);
    }
    eprintln!("seed {seed}：操作ごとの（試した数, 効いた数） {tally:?}");
    // 絵を変える操作が、ちゃんと効いている（何もしない操作ばかりの確かめにしない）
    let edits: usize = [
        "ブラシ",
        "消しゴム",
        "バケツ",
        "アンカー+",
        "アンカー-",
        "頂点",
        "引っぱり",
    ]
    .iter()
    .filter_map(|k| tally.get(k))
    .map(|t| t.1)
    .sum();
    assert!(
        edits * 5 >= steps,
        "seed {seed}：絵を変える操作が効いたのは {edits} 回だけ"
    );
}

/// 浮いた片の作り直し（設計審査 b7208d9 の 9.）のお手本：決まった種のでたらめな操作（浮いた片の操作を多めに）のあと、
/// 毎回「置いた文書」（`committed_document_copy`）の絵を、レイヤー・キーフレーム・階層・色・線かごとの塗った所（面積と外枠）
/// で記す。`IS2D_GOLDEN_WRITE=1` で記録をファイル（`IS2D_GOLDEN`）に書き、無ければ記録と比べる（面積は 1px²・外枠は
/// 1/20px まで同じとみなす：切り口の曲線は地図の精度の中でずれうる）。図形の番号は数え札で変わるので比べない
#[test]
#[ignore]
fn floating_commits_match_the_recorded_golden() {
    let path = std::path::PathBuf::from(std::env::var("IS2D_GOLDEN").expect("IS2D_GOLDEN に記録のファイルを入れる"));
    let write = std::env::var("IS2D_GOLDEN_WRITE").is_ok();
    let mut lines: Vec<String> = Vec::new();
    floating_random_run(|seed, step, kind, app, action| {
        let ok = crate::canvas_tool::doc_actions::apply(app, action);
        lines.push(format!("# {seed:#x} {step} {kind}"));
        lines.extend(committed_picture(app));
        ok
    });
    if write {
        std::fs::write(&path, lines.join("\n")).unwrap();
        return;
    }
    let want: Vec<String> = std::fs::read_to_string(&path).expect("記録がある").lines().map(str::to_owned).collect();
    let mut step = String::new();
    for (k, (got, w)) in lines.iter().zip(&want).enumerate() {
        if got.starts_with('#') {
            step.clone_from(got);
        }
        assert!(picture_line_near(got, w), "{step}（{k} 行目）：記録と違う\n  記録：{w}\n  今　：{got}");
    }
    assert_eq!(lines.len(), want.len(), "記録と行の数が違う");
}

/// ランダムな操作を続けても、地図の決まりと画面の図形の一覧が崩れない（短い版。長い版は
/// `random_operation_marathon_long`）
#[test]
fn random_operations_keep_maps_and_scene_consistent() {
    random_operation_marathon(1, 150);
}

/// ランダムな操作の長い版（必要なときに走らせる：`IS2D_MARATHON_STEPS`・`IS2D_MARATHON_SEEDS` で回数と種の数を変えられる）
#[test]
#[ignore]
fn random_operation_marathon_long() {
    let steps: usize = std::env::var("IS2D_MARATHON_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let seeds: u64 = std::env::var("IS2D_MARATHON_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    for seed in 1..=seeds {
        random_operation_marathon(seed * 7919, steps);
    }
}

/// 極端な値（とても大きい・とても小さい座標・太さ・縮め方・回転）の操作・道具のドラッグ・置いた写し作りを、でたらめに
/// 大量に試しても、どこでも計算が止まらない（止まった場所を集めて、無いことを確かめる。`IS2D_FUZZ_SEEDS`・
/// `IS2D_FUZZ_STEPS` で増やせる。止まった所を見張るのに、テスト全体で1つの見張り（panic hook）を替えるので、ほかの
/// テストと同時には走らせない：`--ignored`。設計審査 392e9af の D-1 の確かめ：ブラシの線が格子の外で形を作り始めて固まる
/// 道を、これで見つけた）
#[test]
#[ignore]
fn extreme_inputs_never_stop_a_computation() {
    use crate::canvas_tool::doc_actions::{apply, Action};
    use std::sync::{Arc, Mutex};
    let found: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = found.clone();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        sink.lock().unwrap().push(format!("{loc} {}", msg.chars().take(120).collect::<String>()));
    }));
    let scales = [1.0, 1e3, 1e6, 1e9, 1e12, 1e15, 1e18, 1e30, 1e100, 1e300, 1e-6, 1e-12, 0.0];
    let seeds: u64 = std::env::var("IS2D_FUZZ_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    let steps: usize = std::env::var("IS2D_FUZZ_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(400);
    let names = action_kind_names();
    let mut caught = std::collections::BTreeMap::<String, usize>::new();
    for seed in 0..seeds {
        let mut rng = OpRng(0xf022_0000 + seed);
        let mut app = Is2dApp::default();
        for _ in 0..steps {
            let k = rng.range(12);
            let s = scales[rng.range(scales.len())];
            let p = |rng: &mut OpRng| Point::new(rng.f64_range(-1.0, 1.0) * s + 960.0, rng.f64_range(-1.0, 1.0) * s + 540.0);
            let before = found.lock().unwrap().len();
            match k {
                0 => { let (a, b) = (p(&mut rng), p(&mut rng)); apply(&mut app, Action::PaintStroke { points: vec![a, b], radius: ((s * rng.f64_range(0.0, 1.0)).max(1e-9)) as f32, color: Color32::RED }); }
                1 => { let q = p(&mut rng); apply(&mut app, Action::BucketFill { pt: q, color: Color32::BLUE }); }
                2 => { let (a, b) = (p(&mut rng), p(&mut rng)); apply(&mut app, Action::SelectMarquee { p0: a, p1: b, additive: rng.coin() }); }
                3 => { apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(rng.f64_range(-1.0, 1.0) * s, rng.f64_range(-1.0, 1.0) * s), alt_duplicate: rng.coin() }); }
                4 => { scale_selection(&mut app, rng.f64_range(-2.0, 2.0) * s, rng.f64_range(-2.0, 2.0) * s); }
                5 => { apply(&mut app, Action::RotateSelection { angle_rad: rng.f64_range(-1.0, 1.0) * s }); }
                6 => { let q = p(&mut rng); apply(&mut app, Action::AddAnchor { pt: q }); }
                7 => { let (a, b) = (p(&mut rng), p(&mut rng)); app.run_frame(|t| { crate::tool_select::start_reshape_drag(t, a, false, false); crate::tool_select::update_reshape_drag(t, b); crate::tool_select::finish_reshape_drag(t, Some(b)); }); if found.lock().unwrap().len() > before { *caught.entry("道具の入力".into()).or_default() += 1; } }
                8 => { let (a, b) = (p(&mut rng), p(&mut rng)); let constrain = rng.coin(); let (shift, alt) = (rng.coin(), rng.coin()); app.run_frame(|t| { crate::tool_transform::start_transform_drag(t, a, false, constrain); crate::tool_transform::update_transform_drag(t, b, shift, alt); crate::tool_transform::finish_transform_drag(t, Some(b)); }); }
                9 => app.run_frame(|t| { let _ = t.tab().committed_document_copy(); }),
                _ => { let kind = names[rng.range(names.len())].clone(); let action = random_action(&kind, &mut rng, &app); apply(&mut app, action); }
            }
            let after = found.lock().unwrap().len();
            if after > before { *caught.entry(format!("k={k}")).or_default() += after - before; }
        }
    }
    std::panic::set_hook(prev);
    let mut uniq: Vec<String> = found.lock().unwrap().clone();
    uniq.sort(); uniq.dedup();
    eprintln!("止まった場所 {} 種:", uniq.len());
    for u in uniq.iter().take(40) { eprintln!("  {u}"); }
    eprintln!("受け止めた所: {caught:?}");
    assert!(uniq.is_empty(), "計算が止まる道がある：{uniq:?}");
}
