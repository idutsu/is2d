//! テストの部屋（`canvas_tool::tests`）：いくつものファイルで使う道具と、仕事ごとのテストのファイル（設計：claude/層の分け方の直しの設計.md の 8.。テストが確かめる本番の仕事で分ける）

use crate::canvas_tool::doc_actions::{apply, Action, ActionHost};
use crate::canvas_tool::{clipboard_ops, doc_actions, select_ops, symbol_ops, SelectSubMode, Tab, Tool};
use crate::panels::UiAction;
use crate::project_io::{PendingCloseConfirm, ProjectWorkspace};
use crate::vector_engine::FillShape;
use crate::Is2dApp;
use crate::{
    canvas_tool, menu, panels, project_io, recovery, save_format, stored_map, symbol, symbol_binding, timeline,
    timeline_ui, tool_paint, tool_select, tool_transform, vector_engine, vector_stroke,
};
use eframe::egui::{self, Color32, Pos2, Vec2};
use kurbo::Point;
use crate::canvas_tool::HoverTarget;
use crate::canvas_tool::select_ops::ArrangeMode;
use kurbo::{Affine, ParamCurve, ParamCurveDeriv, QuadBez, Vec2 as KVec2};
use vector_engine::{left_normal, normalize_vec};

/// テスト用：ブラシで描く線（マウスの点・半径・表示の倍率）。`brush_line_shapes` で離したあとの形にし、
/// `commit_brush_line` で今のツール（ブラシか消しゴム）で確定する
#[derive(Clone, Debug)]
struct BrushLine {
    pts: Vec<Point>,
    radius: f64,
    zoom: f64,
}

/// 表示の倍率 1 で、点 `pts` を半径 `radius` で描く線
fn brush_line(pts: &[Point], radius: f64) -> BrushLine {
    brush_line_at_zoom(pts, radius, 1.0)
}

/// 表示の倍率 `zoom` で、点 `pts`（文書の座標）を半径 `radius` で描く線
fn brush_line_at_zoom(pts: &[Point], radius: f64, zoom: f64) -> BrushLine {
    BrushLine {
        pts: pts.to_vec(),
        radius,
        zoom,
    }
}

/// 線 `line` を離したあとの形（画面の倍率 1）
fn brush_line_shapes(line: &BrushLine, color: Color32) -> Vec<FillShape> {
    brush_line_shapes_at_zoom(line, color, line.zoom)
}

/// 線 `line` を、表示の倍率 `zoom` で離したあとの形（画面の倍率 1）
fn brush_line_shapes_at_zoom(line: &BrushLine, color: Color32, zoom: f64) -> Vec<FillShape> {
    let step = vector_stroke::input_step(zoom, 1.0);
    vector_stroke::stroke_shapes(&line.pts, line.radius, step, zoom, color)
}

/// 線 `line` を、今のツール（ブラシなら今の色で塗る、消しゴムなら消す）で確定する（表示の倍率は線のもの）
fn commit_brush_line(app: &mut Is2dApp, line: &BrushLine) {
    let prev_zoom = app.tab().view.zoom();
    app.tab_mut().view.set_zoom(line.zoom as f32);
    let action = if app.tool.tool == Tool::Eraser {
        Action::EraseStroke {
            points: line.pts.clone(),
            radius: line.radius as f32,
        }
    } else {
        Action::PaintStroke {
            points: line.pts.clone(),
            radius: line.radius as f32,
            color: app.tool.brush_color,
        }
    };
    doc_actions::apply(app, action);
    app.tab_mut().view.set_zoom(prev_zoom);
}

/// テスト用：点 `points` をブラシ（半径 `radius`、表示と画面の倍率 1）で描いている途中のドラッグ
fn paint_drag(points: &[Point], radius: f64) -> canvas_tool::ActiveCanvasDrag {
    let mut stroke = vector_stroke::CenterlineBuilder::new(1.0);
    for &p in points {
        stroke.push(p);
    }
    let cl = stroke.current();
    let mut tail_mesh = egui::Mesh::default();
    for ring in vector_stroke::stroke_preview_outline(&cl, 0, cl.len() - 1, radius, 1.0) {
        vector_engine::append_mesh(&mut tail_mesh, &vector_engine::tessellate_raw_polygon(&ring, Color32::BLACK));
    }
    canvas_tool::ActiveCanvasDrag::PaintStroke {
        stroke,
        committed_world_mesh: egui::Mesh::default(),
        tail_mesh,
    }
}

/// テスト用：区間の列 `segs` の輪郭を、面積が正の向きにした輪郭
fn positive_contour(segs: Vec<QuadBez>) -> vector_engine::Contour {
    let mut c = vector_engine::Contour::new(segs);
    if c.signed_area() < 0.0 {
        c.reverse();
    }
    c
}

/// 図形の並び `canvas`（文書の無い、ただの並び）のレイヤー `layer_id`・階層 `scope` の生の図形を地図にし（図形がそのまま
/// 正しい地図にならなければ、並びの順に塗り重ねて作る）、1本の道の編集 `edit`（`edit_local` など）の答えを並びに書き戻す。
/// 塗り重ねて作った地図なら、地図の面と図形は1対1でないので、その階層の生の図形を全部書き換える
fn edit_canvas(
    canvas: &mut Vec<FillShape>,
    layer_id: u64,
    scope: &[u64],
    edit: impl FnOnce(&crate::planar_map::LayerBase) -> crate::planar_map::LocalEdit,
) {
    use crate::planar_map::{is_map_shape, painted_map_pub, LayerBase, LocalEdit, PlanarMap};
    let own: Vec<FillShape> = canvas
        .iter()
        .filter(|s| s.layer_id == layer_id && is_map_shape(s, scope))
        .cloned()
        .collect();
    let map = PlanarMap::from_faces(own.iter());
    let one_to_one = map.is_valid();
    let Some(map) = (if one_to_one {
        Some(map)
    } else {
        painted_map_pub(&own)
    }) else {
        return;
    };
    let Some(map) = map.checked() else {
        return;
    };
    let base = LayerBase::new(&map.uniquely_labeled().into_map());
    let LocalEdit::Changed(w) = edit(&base) else {
        return;
    };
    let (removed, added) = if one_to_one {
        (w.removed, w.added)
    } else {
        let all = w.faces.iter().map(|s| s.shape_id).collect();
        (own.iter().map(|s| s.shape_id).collect(), all)
    };
    let added: Vec<FillShape> = added
        .iter()
        .filter_map(|id| w.faces.by_id(*id))
        .map(|s| {
            let mut s = (**s).clone();
            s.layer_id = layer_id;
            s
        })
        .collect();
    apply_shape_diff(canvas, &removed, added, layer_id, scope);
}

/// 差分を図形の並びに入れる。新しい形は、消した図形のうち最初のものの位置に入れる。
/// 何も消さないとき（何も無い所へのブラシなど）は、レイヤー `layer_id`・階層 `group_path` の生の図形の置き場
/// （グループ・シンボルより下）に入れる
fn apply_shape_diff(
    shapes: &mut Vec<FillShape>,
    removed: &[crate::vector_engine::ShapeId],
    added: Vec<FillShape>,
    layer_id: u64,
    group_path: &[u64],
) {
    let at = shapes.iter().position(|s| removed.contains(&s.shape_id));
    shapes.retain(|s| !removed.contains(&s.shape_id));
    let at = at
        .unwrap_or_else(|| raw_scope_insert_index(shapes, layer_id, group_path))
        .min(shapes.len());
    shapes.splice(at..at, added);
}

/// レイヤー `target_layer_id`・階層 `target_group_path` の生の図形の置き場（その階層の生の図形の後ろ。グループ・シンボルより下）
fn raw_scope_insert_index(
    canvas: &[FillShape],
    target_layer_id: u64,
    target_group_path: &[u64],
) -> usize {
    let at = |i: usize| &canvas[i];
    let n = canvas.len();
    (0..n)
        .rev()
        .find(|&i| {
            let s = at(i);
            s.layer_id == target_layer_id
                && s.group_path == target_group_path
                && s.symbol_instance.is_none()
        })
        .map(|pos| pos + 1)
        .or_else(|| {
            (0..n).find(|&i| {
                let s = at(i);
                s.layer_id == target_layer_id
                    && s.is_in_editing_scope(target_group_path)
                    && !s.is_raw_at_scope(target_group_path)
            })
        })
        .or_else(|| {
            (0..n)
                .rev()
                .find(|&i| {
                    let s = at(i);
                    s.layer_id == target_layer_id && s.is_in_editing_scope(target_group_path)
                })
                .map(|pos| pos + 1)
        })
        .or_else(|| {
            (0..n)
                .rev()
                .find(|&i| at(i).layer_id == target_layer_id)
                .map(|pos| pos + 1)
        })
        .unwrap_or(n)
}

/// 図形を、レイヤー・階層ごとに地図で上に重ねて足す（浮いている片を置くのと同じ：1本の道 `edit_local`）
fn merge_shapes_into_canvas(canvas: &mut Vec<FillShape>, incoming: Vec<FillShape>) {
    let mut scopes: Vec<(u64, Vec<u64>)> = Vec::new();
    for p in &incoming {
        if !scopes
            .iter()
            .any(|x| x.0 == p.layer_id && x.1 == p.group_path)
        {
            scopes.push((p.layer_id, p.group_path.clone()));
        }
    }
    for (layer_id, scope) in scopes {
        let mine: Vec<&FillShape> = incoming
            .iter()
            .filter(|p| p.layer_id == layer_id && p.group_path == scope)
            .collect();
        let Some(piece) = crate::planar_map::LayerBase::of(&mine) else {
            continue;
        };
        edit_canvas(canvas, layer_id, &scope, |b| {
            b.edit_local(&b.map_change(piece.map(), crate::planar_map::MapRule::Overlay), &scope)
        });
    }
}

/// レイヤー `layer_id`・階層 `scope` の生の図形から、`erasers` の形を消す（消しゴムと同じく地図で：1本の道 `edit_local`）
fn erase_from_canvas(
    canvas: &mut Vec<FillShape>,
    erasers: &[FillShape],
    layer_id: u64,
    scope: &[u64],
) {
    use crate::planar_map::MapRule;
    for e in erasers {
        edit_canvas(canvas, layer_id, scope, |b| {
            b.edit_local(
                &b.paint_change(std::slice::from_ref(e), MapRule::Paint(None)),
                scope,
            )
        });
    }
}

fn make_stroke(p0: Point, p1: Point, radius: f32, color: Color32) -> Vec<FillShape> {
    let pts: Vec<Point> = (0..=12).map(|i| p0.lerp(p1, i as f64 / 12.0)).collect();
    let raw = brush_line(&pts, radius as f64);
    brush_line_shapes(&raw, color)
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
        self.register_symbol(
            symbol::SymbolDef::new(sym_id, String::new(), local_shapes).expect("地図にできる図形"),
        );
        sym_id
    }
}

/// 編集中のタイムラインの指定レイヤー・指定フレームに、セル図形列から作ったセル画を置くテスト補助関数
/// テスト用：画面に見えている範囲として渡す、地図の範囲の全部（セル画の写しの画面の結合メッシュ。倍率 1 では使わない）
const WHOLE_VIEW: kurbo::Rect = kurbo::Rect::new(-65_536.0, -65_536.0, 65_536.0, 65_536.0);

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
    // 下ごしらえなので、図形から地図を作ってセル画にする（本番の書き込みは、地図の面か浮いた片しか書かない）。ステージの
    // 座標の図形を、今の階層の文書の座標へ戻す
    let world = app.session.edit_path().world_xform();
    for s in &mut shapes {
        s.layer_id = lid;
        if world != kurbo::Affine::IDENTITY {
            s.transform(world.inverse());
        }
        s.strip_transient_caches();
    }
    let cel = crate::timeline::Cel::from_shapes(shapes);
    let written = app.tab_mut().with_active_timeline_mut(|tl, _lib| {
        let layer = tl.layers.get_mut(layer_idx)?;
        if !layer.is_editable_at(frame) {
            return None;
        }
        let kf = layer.keyframe_index_at(frame)?;
        match cel {
            Ok(cel) => {
                layer.keyframe_mut(kf).cel = cel;
                Some(true)
            }
            Err(_) => Some(false),
        }
    });
    if written == Some(false) {
        app.session.refusal = Some(canvas_tool::Refusal::OffGrid);
    }
    canvas_tool::finish_cel_writes(app.tab_mut(), &[lid]);
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

/// テスト用：図形 `shapes` の輪郭（外周と穴）を、ずれ `tol` 以内の閉じた折れ線にしたもの
fn shape_outline_polylines(shapes: &[FillShape], tol: f64) -> Vec<Vec<Point>> {
    shapes
        .iter()
        .flat_map(|s| s.contours())
        .map(|c| {
            let mut pts = Vec::new();
            for q in c.segments.iter() {
                // 2次ベジェを n 等分した弦のずれは |p0 − 2p1 + p2| / (4n²) 以下
                let dd = (q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2()).hypot();
                let n = ((dd / (4.0 * tol)).sqrt().ceil() as usize).max(1);
                pts.extend((0..n).map(|i| q.eval(i as f64 / n as f64)));
            }
            pts
        })
        .collect()
}

/// テスト用：線分を格子に入れて、点からの近さを速く調べる
struct SegGrid {
    cell: f64,
    cells: std::collections::HashMap<(i64, i64), Vec<(Point, Point)>>,
}

impl SegGrid {
    /// 折れ線 `lines`（`closed` なら閉じたもの）の線分を、幅 `cell` の格子に入れる
    fn new(lines: &[Vec<Point>], closed: bool, cell: f64) -> Self {
        let mut cells: std::collections::HashMap<(i64, i64), Vec<(Point, Point)>> =
            Default::default();
        for l in lines {
            let n = l.len();
            let segs: Vec<(Point, Point)> = match n {
                0 => Vec::new(),
                1 => vec![(l[0], l[0])],
                _ if closed => (0..n).map(|i| (l[i], l[(i + 1) % n])).collect(),
                _ => (0..n - 1).map(|i| (l[i], l[i + 1])).collect(),
            };
            for (a, b) in segs {
                let k = |v: f64| (v / cell).floor() as i64;
                for x in k(a.x.min(b.x))..=k(a.x.max(b.x)) {
                    for y in k(a.y.min(b.y))..=k(a.y.max(b.y)) {
                        cells.entry((x, y)).or_default().push((a, b));
                    }
                }
            }
        }
        Self { cell, cells }
    }

    /// 点 `p` から `reach` 以内にある線分までの、いちばん近い距離（`reach` 以内に無ければ無限大）
    fn dist(&self, p: Point, reach: f64) -> f64 {
        let k = |v: f64| (v / self.cell).floor() as i64;
        let mut best = f64::INFINITY;
        for x in k(p.x - reach)..=k(p.x + reach) {
            for y in k(p.y - reach)..=k(p.y + reach) {
                for &(a, b) in self.cells.get(&(x, y)).into_iter().flatten() {
                    best = best.min(vector_engine::point_to_segment_dist(p, a, b));
                }
            }
        }
        if best <= reach {
            best
        } else {
            f64::INFINITY
        }
    }
}

/// 当てはめの許容（画面の px）から決まる、離したあとの形と「中心線を太らせた形」のずれの上限：当てはめの許容 0.25
/// （`vector_stroke::FIT_TOL_PX`）＋ 太らせた輪郭を折れ線にした弦のずれ 0.025 ＋ 当てはめを確かめるときに曲線を折れ線に
/// したずれ 0.025 ＋ このテストで輪郭と中心線を折れ線にするずれ 0.005×2
const STROKE_DEV_PX: f64 = 0.31;

/// テスト用：点 `points` を半径 `radius`・表示の倍率 `zoom`（画面の倍率 1）で描いて離したあとの形を、中心線と比べた結果
#[derive(Debug, Default)]
struct StrokeCheck {
    /// 輪郭の点の、中心線からの距離と半径の差のいちばん大きいもの（画面の px）
    worst_px: f64,
    /// 丸めの範囲に入っていない中心線の節の数
    outside_box: usize,
    /// 塗られていない（形の中にも、輪郭から「丸めの範囲＋ずれの上限」以内にもない）マウスの点の数
    uncovered: usize,
    /// 塗られていない（形の中にも、輪郭からずれの上限以内にもない）中心線の点の数
    missing: usize,
}

fn check_stroke_follows_centerline(points: &[Point], radius: f64, zoom: f64) -> StrokeCheck {
    let step = vector_stroke::input_step(zoom, 1.0);
    let tol = STROKE_DEV_PX / zoom;
    let mut b = vector_stroke::CenterlineBuilder::new(step);
    for &p in points {
        b.push(p);
    }
    let cl = b.current();
    let half = 0.5 * step * (1.0 + 1e-5) + 1e-9;
    let outside_box = cl
        .knots()
        .iter()
        .zip(b.points())
        .filter(|(k, p)| (k.x - p.x).abs() > half || (k.y - p.y).abs() > half)
        .count();
    let shapes = vector_stroke::stroke_shapes(points, radius, step, zoom, Color32::BLACK);
    let center = cl.sample_points(0.005 / zoom);
    let cell = radius + 1.0 / zoom;
    let near_center = SegGrid::new(std::slice::from_ref(&center), false, cell);
    let outline = shape_outline_polylines(&shapes, 0.005 / zoom);
    let near_outline = SegGrid::new(&outline, true, cell);
    let inside = |p: Point| shapes.iter().any(|s| s.contains_point(p));
    let worst = outline
        .iter()
        .flatten()
        .map(|&q| (near_center.dist(q, radius + 10.0 / zoom) - radius).abs())
        .fold(0.0f64, f64::max);
    let painted = |p: Point, slack: f64| inside(p) || near_outline.dist(p, slack).is_finite();
    let corner = std::f64::consts::FRAC_1_SQRT_2 * step;
    StrokeCheck {
        worst_px: worst * zoom,
        outside_box,
        uncovered: b
            .points()
            .iter()
            .filter(|&&p| !painted(p, corner + tol))
            .count(),
        // 中心線の点は、10 点に 1 点（どれも隣と弦のずれ 0.005px の細かさ）と終わりの点を見る
        missing: center
            .iter()
            .step_by(10)
            .chain(center.last())
            .filter(|&&p| !painted(p, tol))
            .count(),
    }
}

/// テスト用：手で描いたような乱数の線。画面の整数の点を、表示の倍率 `zoom` で文書の座標に直したもの。向きと速さが
/// ゆっくり変わり、ときどき折り返し（向きを大きく変える）、止まる（同じ点が続く）こともある
fn random_hand_stroke(rng: &mut fastrand::Rng, zoom: f64) -> Vec<Point> {
    use std::f64::consts::{PI, TAU};
    let n = match rng.u32(0..10) {
        0 => rng.usize(1..4),
        1..=6 => rng.usize(4..120),
        _ => rng.usize(120..600),
    };
    let (mut x, mut y) = (100.0 + 600.0 * rng.f64(), 100.0 + 400.0 * rng.f64());
    let (mut dir, mut turn) = (TAU * rng.f64(), 0.0);
    let mut speed = 0.5 + 8.0 * rng.f64();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(Point::new(x.round() / zoom, y.round() / zoom));
        turn = 0.9 * turn + 0.4 * (rng.f64() - 0.5);
        if rng.u32(0..40) == 0 {
            let side = if rng.bool() { 1.0 } else { -1.0 };
            dir += side * PI * (0.6 + 0.4 * rng.f64());
        }
        dir += turn;
        speed = (speed + 2.0 * (rng.f64() - 0.5)).clamp(0.0, 15.0);
        x += speed * dir.cos();
        y += speed * dir.sin();
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
struct InstanceSummary {
    instance_gid: u64,
    symbol_id: u64,
    xform_coeffs: [f64; 6],
    pivot: Point,
    loop_mode: crate::symbol_binding::SymbolLoopMode,
    first_frame: usize,
}

#[derive(Clone, Debug)]
struct CelSummary {
    exists: bool,
    is_keyframe_start: bool,
    tween: bool,
    tween_rotate: crate::timeline::TweenRotate,
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
            tween_rotate: crate::timeline::TweenRotate::Auto,
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
            tween_rotate: crate::timeline::TweenRotate::Auto,
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
    let shapes: Vec<FillShape> = cel.shapes().iter().map(|s| (**s).clone()).collect();
    let shapes = shapes.as_slice();
    let colors = shapes.iter().map(|s| s.color()).collect();
    let total_area = shapes
        .iter()
        .map(|s| {
            let ext = s.exterior().signed_area().abs();
            let holes: f64 = s.holes().iter().map(|h| h.signed_area().abs()).sum();
            (ext - holes).max(0.0)
        })
        .sum();
    let bbox = (!shapes.is_empty()).then(|| vector_engine::union_bounding_boxes(shapes));
    let total_segments = shapes
        .iter()
        .map(|s| {
            s.exterior().segments.len() + s.holes().iter().map(|h| h.segments.len()).sum::<usize>()
        })
        .sum();
    let hole_count = shapes.iter().map(|s| s.holes().len()).sum();
    let group_paths = shapes.iter().map(|s| s.group_path.clone()).collect();
    let instances = cel
        .instances()
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
        .evaluate_layer_shapes_at(layer_idx, frame, doc.library());
    let evaluated_shape_count = eval_shapes.len();
    let evaluated_total_area = eval_shapes
        .iter()
        .map(|s| {
            let ext = s.exterior().signed_area().abs();
            let holes: f64 = s.holes().iter().map(|h| h.signed_area().abs()).sum();
            (ext - holes).max(0.0)
        })
        .sum();
    let evaluated_bbox =
        (!eval_shapes.is_empty()).then(|| vector_engine::union_bounding_boxes(&eval_shapes));

    CelSummary {
        exists: true,
        is_keyframe_start: layer.start_of(kf_idx) == frame,
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
/// テスト用：保存用の写し `pd` の中身の JSON。地図は塊の番号の並びの代わりに、塊の辺を並べたもの（色は色の表から引いた値）
/// にし、面の図形の番号は書かない（浮いた片を写しの上で確定する保存は、呼ぶたびに片に新しい番号を振り、塊の共有の仕方も
/// 変わるので、同じ中身でも塊の表が同じとは限らない）
fn content_json(pd: &save_format::ProjectDocument) -> String {
    let mut v = serde_json::to_value(pd).unwrap();
    let tiles = v["map_tiles"]["tiles"].take();
    let colors = v["map_tiles"]["colors"].take();
    // 線の色も、色の表の番号でなく色で比べる（色の表の並びは書くたびに決まるので、番号だけでは、違う色が同じ番号になる
    // ことがある）。線の番号の印は、側の図形の印と同じく比べない
    if let Some(strokes) = v["map_tiles"]["strokes"].as_array_mut() {
        for st in strokes {
            let i = st["color"].as_u64().unwrap() as usize;
            st["color"] = colors[i - 1].clone();
            st["id"].take();
        }
    }
    let expand = |m: &mut serde_json::Value| {
        let edges: Vec<serde_json::Value> = m["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| {
                let tile = &tiles[t.as_u64().unwrap() as usize];
                let nodes = tile["nodes"].clone();
                tile["edges"]
                    .as_array()
                    .unwrap()
                    .clone()
                    .into_iter()
                    .map(move |mut e| {
                        for end in ["a", "b"] {
                            e[end] = nodes[e[end].as_u64().unwrap() as usize].clone();
                        }
                        e
                    })
            })
            .map(|mut e| {
                for side in ["left", "right"] {
                    let i = e[side].as_u64().unwrap() as usize;
                    e[side] = if i == 0 {
                        serde_json::Value::Null
                    } else {
                        colors[i - 1].clone()
                    };
                    e[format!("{side}_src")].take();
                }
                e
            })
            .collect();
        m["tiles"] = serde_json::Value::Array(edges);
    };
    let timelines = std::iter::once(v["timeline"].clone())
        .chain(
            v["library"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["timeline"].clone()),
        )
        .collect::<Vec<_>>();
    let mut out_timelines = Vec::new();
    for mut tl in timelines {
        for layer in tl["layers"].as_array_mut().unwrap() {
            for kf in layer["keyframes"].as_array_mut().unwrap() {
                for m in kf["maps"].as_array_mut().unwrap() {
                    expand(m);
                }
            }
        }
        out_timelines.push(tl);
    }
    v["timeline"] = out_timelines[0].clone();
    for (s, tl) in v["library"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(&out_timelines[1..])
    {
        s["timeline"] = tl.clone();
    }
    v.to_string()
}

/// 保存用の型 `doc` を、中身を確かめずに今の版の `.is2d` ファイルとして `path` に書く（テストで細工したファイルを作る：
/// `crafted_bytes`）
fn write_crafted_file<T: serde::Serialize>(path: &std::path::Path, doc: &save_format::ProjectDocument<T>) {
    std::fs::write(path, crafted_bytes(doc).unwrap()).unwrap();
}

/// `.is2d` ファイルの中身 `bytes` を、アプリが開くのと同じ道で文書にする（`read_project`・`into_document`）
fn opened_document(bytes: &[u8]) -> Result<crate::symbol::Document, String> {
    let path = scratch_file_path("open");
    std::fs::write(&path, bytes).unwrap();
    let read = read_saved_file(&path);
    let _ = std::fs::remove_file(&path);
    read?.into_document()
}

/// テストで一時に書くファイルの置き場（呼ぶたびに別の名前）
fn scratch_file_path(stem: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("is2d_{stem}_{}_{n}.is2d", std::process::id()))
}

/// 文書の地図の塊の表の保存の形：色の表と、塊の表（ファイルの塊の表の並び。アプリはこの形を丸ごと持たない：書くときは
/// 文書の塊から流す `WriteTiles`、読むときは読みながら塊にする `ReadTiles`）。テストは、中身を見たり細工したりするために、
/// この形で読み書きする
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SavedTiles {
    pub(crate) colors: Vec<[u8; 4]>,
    pub(crate) tiles: Vec<stored_map::SavedTile>,
    #[serde(default)]
    pub(crate) strokes: Vec<stored_map::SavedStroke>,
}

/// 塊の表を保存の形（`SavedTiles`）のまま持つ保存用の型（中身を見たり細工したりする）
type SavedProject = save_format::ProjectDocument<SavedTiles>;

/// `.is2d` ファイルの中身 `bytes` を、塊の表を保存の形のまま読む（アプリと同じ読み込みの道 `read_project`）
fn project_from_bytes(bytes: &[u8]) -> Result<SavedProject, String> {
    let doc = opened_document(bytes)?;
    format_v11::read::<SavedTiles>(&format_v11::bytes(&saved_of(&doc))?[..])
}

/// 作品のファイル `path` を、塊の表を保存の形のまま読む（中身を見たり細工したりする：`project_from_bytes`）
fn saved_project_at(path: &std::path::Path) -> SavedProject {
    project_from_bytes(&std::fs::read(path).unwrap()).unwrap()
}

/// テスト用：今のタブを保存した形（浮いた片を置いた作品を、本番の書き手で書いて読み戻したもの）
fn saved_project_of(app: &Is2dApp) -> SavedProject {
    let doc = app.tab().committed_document_copy().expect("保存できる");
    project_from_bytes(&saved_file_bytes(&doc)).unwrap()
}

/// 輪郭の辺を点 `from`（ステージの座標）でつかんで `to` へ引っぱる（本番の輪郭のドラッグ `reshape_drag`）。辺に当たらない・
/// 作品が変わらない（断られた）なら `false`
fn pull_edge(host: &mut impl doc_actions::ActionHost, from: Point, to: Point) -> bool {
    reshape_drag(host, from, to, false)
}

/// 輪郭の頂点を点 `from`（ステージの座標）でつかんで `to` へ動かす（本番の輪郭のドラッグ `reshape_drag`）。頂点に当たらない・
/// 作品が変わらない（断られた）なら `false`
fn move_vertex(host: &mut impl doc_actions::ActionHost, from: Point, to: Point) -> bool {
    reshape_drag(host, from, to, true)
}

/// 本番の選択ツールの輪郭のドラッグ：点 `from` に、画面のカーソルと同じ当たり（`compute_hover_target`）で頂点（`vertex`）か
/// 辺があれば、ドラッグを始め（`tool_select::start_reshape_drag`。選んでいれば外してから、つかみ直す）、`to` へ動かし
/// （`update_reshape_drag`）、離す（`finish_reshape_drag`）。当たらなければ何もせず `false`。離したときに作品が変わった
/// （中身の世代が進んだ）なら `true`
fn reshape_drag(host: &mut impl doc_actions::ActionHost, from: Point, to: Point, vertex: bool) -> bool {
    use crate::canvas_tool::{ActiveCanvasDrag, HoverTarget, SelectDragState};
    let tab = host.tab();
    let hover = crate::canvas_tool::select_ops::compute_hover_target(tab.scene(), &tab.session, from, tab.view.zoom(), host.tool().select_sub_mode);
    let on = match hover {
        HoverTarget::Vertex { map_node, .. } => vertex && map_node.is_some(),
        HoverTarget::Edge { map_edge, .. } => !vertex && map_edge.is_some(),
        _ => false,
    };
    if !on {
        return false;
    }
    crate::tool_select::start_reshape_drag(host, from, false, false);
    let started = match &host.tab().session.drag {
        ActiveCanvasDrag::Select(SelectDragState::MoveVertex { .. }) => vertex,
        ActiveCanvasDrag::Select(SelectDragState::PullEdge { .. }) => !vertex,
        _ => false,
    };
    if !started {
        crate::canvas_tool::cancel_active_drag(host.tab_mut());
        return false;
    }
    let before = host.tab().history.content_gen;
    crate::tool_select::update_reshape_drag(host.tab_mut(), to);
    crate::tool_select::finish_reshape_drag(host, Some(to));
    host.tab().history.content_gen != before
}

/// 選んでいるものを、変形の中心のまわりに `sx`・`sy` 倍する（本番の自由変形のドラッグを離したときと同じ `Action::CommitTransformDrag`）
fn scale_selection(host: &mut impl doc_actions::ActionHost, sx: f64, sy: f64) -> bool {
    let tab = host.tab();
    let Some((_, pivot)) = tab.session.selection_bbox_and_pivot(tab.view.zoom()) else {
        return false;
    };
    let had_custom_pivot = tab.session.transform_pivot.is_some();
    doc_actions::apply(
        host,
        doc_actions::Action::CommitTransformDrag {
            affine: crate::vector_engine::affine_scale_around(pivot, sx, sy),
            orig_pivot: pivot,
            had_custom_pivot,
            changed: true,
        },
    )
}

/// 変形の中心を `pivot`（ステージの座標）にする・戻す（本番の中心のドラッグを離したとき `Action::CommitPivotDrag`、戻すのは
/// `Action::ResetTransformPivot`）
fn set_pivot(host: &mut impl doc_actions::ActionHost, pivot: Option<Point>) -> bool {
    let Some(new_pivot) = pivot else {
        return doc_actions::apply(host, doc_actions::Action::ResetTransformPivot);
    };
    let tab = host.tab();
    let orig_pivot = tab.session.selection_bbox_and_pivot(tab.view.zoom()).map_or(new_pivot, |(_, p)| p);
    let had_custom_pivot = tab.session.transform_pivot.is_some();
    doc_actions::apply(host, doc_actions::Action::CommitPivotDrag { new_pivot, orig_pivot, had_custom_pivot, changed: true })
}

/// シンボルの中から、入れ子の深さ `depth` まで出る（本番のパンくずと同じ `Action::ExitGroupSteps`。今の階層のグループからも出る）
fn exit_symbols_to(host: &mut impl doc_actions::ActionHost, depth: usize) -> bool {
    let mut exited = false;
    while host.tab().session.edit_path().len() > depth {
        if !doc_actions::apply(host, doc_actions::Action::ExitGroupSteps { steps: 1 }) {
            break;
        }
        exited = true;
    }
    exited
}

/// まっすぐな線の区間 `segs`（ステージの座標）を、1本ずつ鉛筆で引く（本番の `Action::PencilStroke`：2点の鉛筆はその区間の線）
fn draw_lines(host: &mut impl doc_actions::ActionHost, segs: Vec<crate::planar_map::LineSeg>) -> bool {
    let mut all = true;
    for (q, stroke) in segs {
        all &= doc_actions::apply(host, doc_actions::Action::PencilStroke { points: vec![q.p0, q.p2], stroke });
    }
    all
}

/// ライブラリのシンボルを見せる順に（本番の `Library::at` で引く）
fn symbols_in_order(lib: &crate::symbol::Library) -> impl Iterator<Item = &std::sync::Arc<crate::symbol::SymbolDef>> + '_ {
    (0..lib.len()).filter_map(|i| lib.at(i))
}

/// 作品 `doc` の保存の型（中を見る・細工するため。本番の保存は記録の書き手 `save_format::RecordWriter` が作品から直に書く）
fn saved_of(doc: &crate::symbol::Document) -> save_format::ProjectDocument {
    let mut saved_timeline: save_format::SavedTimeline = (&doc.root).into();
    saved_timeline.fps = save_format::saved_fps(doc);
    let mut out = save_format::ProjectDocument {
        stage_size: [doc.stage_size.x, doc.stage_size.y],
        timeline: saved_timeline,
        library: symbols_in_order(doc.library()).map(|s| save_format::SavedSymbolDef::from(&**s)).collect(),
        next_symbol_id: doc.next_symbol_id.next(),
        next_group_id: doc.next_group_id.next(),
        recovery_source: None,
        map_tiles: crate::stored_map::WriteTiles::default(),
        sounds: doc.sounds.iter().map(|s| save_format::SavedSound::from(&**s)).collect(),
        next_sound_id: doc.next_sound_id.next(),
        next_layer_id: doc.next_layer_id.next(),
    };
    write_map_tiles(&mut out);
    out
}

/// 地図のしまう形を、塊の表の塊の番号の並びにする（`saved_of` のあとに地図を細工したとき）
fn write_map_tiles(pd: &mut save_format::ProjectDocument) {
    let mut writer = std::mem::take(&mut pd.map_tiles);
    pd.for_each_map(|m| m.write_tiles(&mut writer));
    pd.map_tiles = writer;
}

/// タブの今（浮いた片を置いた文書）の保存の型。置けなければ `Err`（本番の保存と同じ断り）
fn saved_from_app(tab: &Tab) -> Result<save_format::ProjectDocument, String> {
    let doc = tab.committed_document_copy().ok_or_else(|| {
        format!("選んだ図形を置けないため保存できません。{}", crate::menu::undo_exit_text())
    })?;
    Ok(saved_of(&doc))
}

/// 保存の型から作品を組み立てる（本番の読み込みの道 `into_document_noting`。書く形・テストの保存の形は、版 11 の入れ物で
/// 書いて読み戻してから：中身の並びは同じ）
trait IntoDocument {
    fn into_document(self) -> Result<crate::symbol::Document, String>;
    /// 組み立てた作品をタブに開く（本番の作品を開く道 `open_document_in_new_tab`：まっさらなタブなら、そのタブに入れる）
    fn apply_to_app(self, ws: &mut impl project_io::ProjectWorkspace)
    where
        Self: Sized,
    {
        let doc = self.into_document().expect("正しい地図の文書");
        project_io::open_document_in_new_tab(ws, doc, None).expect("作品を開ける");
    }
}

impl IntoDocument for save_format::ProjectDocument<crate::stored_map::ReadTiles> {
    fn into_document(self) -> Result<crate::symbol::Document, String> {
        self.into_document_noting(&mut false)
    }
}

impl<T: serde::Serialize> IntoDocument for save_format::ProjectDocument<T> {
    fn into_document(self) -> Result<crate::symbol::Document, String> {
        format_v11::read::<crate::stored_map::ReadTiles>(&format_v11::bytes(&self)?[..])?.into_document()
    }
}

/// 保存の型 `pd` を、中身を確かめずに今の版の作品ファイルのバイトにする（テストで壊れた中身のファイルを作る。本番の記録の型と
/// 枠で書く：`save_format::saved_project_bytes`。塊の表は保存の形に読み直してから）。正しい作品の保存は `saved_file_bytes`
fn crafted_bytes<T: serde::Serialize>(pd: &save_format::ProjectDocument<T>) -> Result<Vec<u8>, String> {
    let mut pd: SavedProject = format_v11::read(&format_v11::bytes(pd)?[..])?;
    let SavedTiles { colors, tiles, strokes } = std::mem::take(&mut pd.map_tiles);
    Ok(save_format::saved_project_bytes(&pd, colors, tiles, strokes))
}

/// 作品 `doc` を、本番の保存と同じ書き手で書いた作品ファイルのバイト（`save_format::write_project_file`）
fn saved_file_bytes(doc: &crate::symbol::Document) -> Vec<u8> {
    let path = scratch_file_path("save");
    save_format::write_project_file(&path, doc).expect("保存できる");
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    bytes
}

/// 作品ファイル・復旧ファイル `path` を本番の読み手で読んだ保存の型
fn read_saved_file(path: &std::path::Path) -> Result<save_format::ProjectDocument<crate::stored_map::ReadTiles>, String> {
    save_format::read_file(path).map(|(_, d)| d)
}

/// 版 11 までの作品ファイルの入れ物（頭：印・版 11・中身の長さ・中身の CRC32、そのあと postcard の保存の型1つ）。前の版の
/// ファイルを直すテスト用の変換と、保存の型から作品を組み立てるテストの道具が使う（本番は版 12 の記録の列だけを読み書きする）
mod format_v11 {
    use crate::save_format::{read_streamed, ProjectDocument, Streamed};

    const HEADER_LEN: usize = 20;

    /// 保存の型 `doc` を版 11 の入れ物で書いたバイト
    pub(super) fn bytes<T: serde::Serialize>(doc: &ProjectDocument<T>) -> Result<Vec<u8>, String> {
        let body = postcard::to_allocvec(doc).map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(HEADER_LEN + body.len());
        out.extend_from_slice(b"IS2D");
        out.extend_from_slice(&11u32.to_le_bytes());
        out.extend_from_slice(&(body.len() as u64).to_le_bytes());
        out.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// 版 11 の入れ物を読んだ保存の型（頭・長さ・CRC32・余りを確かめる）
    pub(super) fn read<T: serde::de::DeserializeOwned + Default>(input: &[u8]) -> Result<ProjectDocument<T>, String> {
        if input.len() < HEADER_LEN || &input[..4] != b"IS2D" || input[4..8] != 11u32.to_le_bytes() {
            return Err("版 11 の作品ファイルではありません".to_owned());
        }
        let body_len = u64::from_le_bytes(input[8..16].try_into().expect("8 バイト"));
        let crc = u32::from_le_bytes(input[16..20].try_into().expect("4 バイト"));
        let Streamed { value, len, crc: got, left } = read_streamed::<ProjectDocument<T>>(&input[HEADER_LEN..])?;
        if len != body_len {
            return Err("ファイルが壊れています（長さが合いません）".to_owned());
        }
        if got != crc {
            return Err("ファイルが壊れています（中身が変わっています）".to_owned());
        }
        match value {
            Ok(doc) if left == 0 => Ok(doc),
            Ok(_) => Err("プロジェクトファイルの形式が正しくありません（余りがあります）".to_owned()),
            Err(e) => Err(format!("プロジェクトファイルの形式が正しくありません: {e}")),
        }
    }
}

/// テストの手順1つ：本番の操作か、本番の操作を組み合わせた手順（`pull_edge`・`move_vertex`・`scale_selection`）
#[derive(Clone, Debug)]
enum Op {
    Do(doc_actions::Action),
    PullEdge(Point, Point),
    MoveVertex(Point, Point),
    Scale(f64, f64),
}

impl From<doc_actions::Action> for Op {
    fn from(a: doc_actions::Action) -> Self {
        Op::Do(a)
    }
}

/// 手順 `o` を行う（効いたか）
fn op(host: &mut impl doc_actions::ActionHost, o: Op) -> bool {
    match o {
        Op::Do(a) => doc_actions::apply(host, a),
        Op::PullEdge(from, to) => pull_edge(host, from, to),
        Op::MoveVertex(from, to) => move_vertex(host, from, to),
        Op::Scale(sx, sy) => scale_selection(host, sx, sy),
    }
}

fn run(host: &mut impl doc_actions::ActionHost, actions: &[doc_actions::Action]) {
    for action in actions {
        doc_actions::apply(host, action.clone());
    }
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

/// でたらめな操作の試しの乱数（同じ種からは同じ並び）
struct OpRng(u64);

impl OpRng {
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
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
    fn coin(&mut self) -> bool {
        self.range(2) == 0
    }
}

/// `doc_actions::Action` の種類の名前を、定義（doc_actions.rs）から全部読む。でたらめな操作の試しが全部の種類を作れることを、
/// 手で書いた名簿でなく定義と突き合わせて確かめるため（種類を足すと、試しが作れるようにするまでテストが止まる：設計
/// claude/約束を入口で守る設計_5616fcc.md の 7.）
fn action_kind_names() -> Vec<String> {
    enum_variant_names(include_str!("../doc_actions.rs"), "pub(crate) enum Action {")
}

/// ソース `src` の、見出し `header`（`pub(crate) enum 名前 {`）で始まる列挙の種類の名前を、定義の順に全部読む
fn enum_variant_names(src: &str, header: &str) -> Vec<String> {
    let start = src.find(header).expect("列挙の定義");
    let mut depth = 0i32;
    let mut names = Vec::new();
    for line in src[start..].lines().skip(1) {
        let t = line.trim();
        if depth == 0 {
            if t == "}" {
                break;
            }
            if let Some(c) = t.chars().next() {
                if c.is_ascii_uppercase() {
                    let name: String = t.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                    names.push(name);
                }
            }
        }
        depth += t.matches('{').count() as i32 - t.matches('}').count() as i32;
    }
    names
}

/// 種類の名前（`Debug` の頭）
fn action_kind_name(action: &Action) -> String {
    format!("{action:?}").chars().take_while(|c| c.is_ascii_alphanumeric()).collect()
}

/// 今の作品の状態から選んだ、種類 `kind` のでたらめな操作（断られてもよい：断られたときに何も変えないことを確かめる）
fn random_action(kind: &str, rng: &mut OpRng, app: &Is2dApp) -> Action {
    use crate::canvas_tool::SelectSubMode;
    use crate::planar_map::EdgeStroke;
    use crate::canvas_tool::select_ops::ArrangeMode;
    use crate::timeline::{TimelineCellSelection, TweenRotate};
    use crate::symbol_binding::{ColorEffect, SymbolLoopMode};
    let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
    let tab = app.tab();
    let tl = tab.active_timeline();
    let pt = |rng: &mut OpRng| Point::new(rng.f64_range(c.x - 130.0, c.x + 130.0), rng.f64_range(c.y - 100.0, c.y + 100.0));
    let colors = [Color32::from_rgb(220, 50, 50), Color32::from_rgb(50, 160, 80), Color32::from_rgb(50, 100, 220)];
    let color = |rng: &mut OpRng| colors[rng.range(colors.len())];
    let stroke = |rng: &mut OpRng| EdgeStroke { color: colors[rng.range(colors.len())], width: 2 + rng.range(5) as u8 };
    // 画面の図形の輪郭の点（輪郭の変形・アンカー・インクボトルが当たるように）
    let scene = tab.scene();
    let shapes: Vec<FillShape> = scene.iter().cloned().collect();
    let on_contour = |rng: &mut OpRng| -> Option<(FillShape, QuadBez, f64)> {
        let s = shapes.get(rng.range(shapes.len()))?;
        let segs: Vec<QuadBez> = s.contours().flat_map(|c| c.segments.iter().copied()).collect();
        let q = *segs.get(rng.range(segs.len()))?;
        Some((s.clone(), q, rng.f64_range(0.1, 0.9)))
    };
    let contour_pt = |rng: &mut OpRng| on_contour(rng).map_or_else(|| pt(rng), |(_, q, t)| q.eval(t));
    let sym_id = |rng: &mut OpRng| tab.doc.library().at(rng.range(tab.doc.library().len())).map_or(999, |s| s.id);
    let sound_id = |rng: &mut OpRng| tab.doc.sounds.at(rng.range(tab.doc.sounds.len())).map_or(999, |s| s.id);
    let layer = |rng: &mut OpRng| rng.range(tl.layers.len() + 1);
    let frame = |rng: &mut OpRng| rng.range(tl.total_frames + 2);
    let selected = tab.session.selected_indices();
    let pivot = tab.session.transform_pivot.unwrap_or(c);
    match kind {
        "PaintStroke" => {
            let p0 = pt(rng);
            Action::PaintStroke { points: vec![p0, p0 + KVec2::new(rng.f64_range(20.0, 70.0), rng.f64_range(-40.0, 40.0))], radius: 6.0, color: color(rng) }
        }
        "EraseStroke" => {
            let p0 = pt(rng);
            Action::EraseStroke { points: vec![p0, p0 + KVec2::new(50.0, 20.0)], radius: 8.0 }
        }
        "BucketFill" => Action::BucketFill { pt: pt(rng), color: color(rng) },
        "PencilStroke" => {
            let p0 = pt(rng);
            Action::PencilStroke { points: vec![p0, p0 + KVec2::new(30.0, 10.0), p0 + KVec2::new(60.0, 35.0)], stroke: stroke(rng) }
        }
        "InkBottle" => Action::InkBottle { pt: contour_pt(rng), stroke: stroke(rng) },
        "SelectSingleAt" => {
            let p = shapes.get(rng.range(shapes.len())).map_or_else(|| pt(rng), |s| s.bounding_box().center());
            Action::SelectSingleAt { pt: p }
        }
        "ToggleSelectionAt" => Action::ToggleSelectionAt { scene_idx: rng.range(shapes.len().max(1)) },
        "RetainSelection" => Action::RetainSelection { keep_indices: selected.iter().copied().filter(|_| rng.coin()).collect() },
        "SelectMarquee" => {
            let p0 = Point::new(rng.f64_range(c.x - 140.0, c.x + 20.0), rng.f64_range(c.y - 100.0, c.y + 20.0));
            Action::SelectMarquee { p0, p1: p0 + KVec2::new(rng.f64_range(30.0, 160.0), rng.f64_range(30.0, 120.0)), additive: rng.coin() }
        }
        "SelectAll" => Action::SelectAll,
        "Deselect" => Action::Deselect,
        "CommitFloating" => Action::Deselect,
        "MoveSelected" => Action::NudgeSelection { delta: KVec2::new(rng.f64_range(-20.0, 20.0), rng.f64_range(-20.0, 20.0)) },
        "NudgeSelection" => Action::NudgeSelection { delta: KVec2::new([-1.0, 1.0, 10.0][rng.range(3)], 0.0) },
        "FinishMoveSelectionDrag" => Action::FinishMoveSelectionDrag { total_delta: KVec2::new(15.0, 12.0), alt_duplicate: rng.coin() },
        "DeleteSelection" => Action::DeleteSelection,
        "ClearActiveFrame" => Action::ClearActiveFrame,
        "Group" => Action::Group,
        "Ungroup" => Action::Ungroup,
        "Arrange" => Action::Arrange { mode: [ArrangeMode::ToFront, ArrangeMode::Forward, ArrangeMode::Backward, ArrangeMode::ToBack][rng.range(4)] },
        "EnterGroupAt" => Action::EnterGroupAt { pt: contour_pt(rng) },
        "SelectConnectedAt" => Action::SelectConnectedAt { pt: contour_pt(rng), add: rng.coin() },
        "ExitGroupSteps" => Action::ExitGroupSteps { steps: 1 + rng.range(2) },
        "ReshapeEdge" => match on_contour(rng) {
            Some((s, q, t)) => Action::ReshapeEdge { layer_id: s.layer_id, grab_t: t, target_pos: q.eval(t) + KVec2::new(8.0, -6.0), orig_seg: q, follow: rng.coin() },
            None => Action::Deselect,
        },
        "MoveVertex" => match on_contour(rng) {
            Some((s, q, _)) => Action::MoveVertex { layer_id: s.layer_id, orig_vertex: q.p0, target_pos: q.p0 + KVec2::new(-7.0, 9.0), follow: rng.coin() },
            None => Action::Deselect,
        },
        "AddAnchor" => Action::AddAnchor { pt: contour_pt(rng) },
        "DeleteAnchor" => Action::DeleteAnchor { pt: on_contour(rng).map_or_else(|| pt(rng), |(_, q, _)| q.p0) },
        "FlipSelection" => Action::FlipSelection { flip_x: rng.coin(), flip_y: rng.coin() },
        "RotateSelection" => Action::RotateSelection { angle_rad: rng.f64_range(-1.0, 1.0) },
        "ResetTransformPivot" => Action::ResetTransformPivot,
        "CommitTransformDrag" => {
            let s = rng.f64_range(0.7, 1.4);
            let affine = kurbo::Affine::translate(pivot.to_vec2()) * kurbo::Affine::scale(s) * kurbo::Affine::translate(-pivot.to_vec2());
            Action::CommitTransformDrag { affine, orig_pivot: pivot, had_custom_pivot: tab.session.transform_pivot.is_some(), changed: true }
        }
        "CommitPivotDrag" => Action::CommitPivotDrag { new_pivot: pt(rng), orig_pivot: pivot, had_custom_pivot: tab.session.transform_pivot.is_some(), changed: true },
        "Copy" => Action::Copy,
        "Cut" => Action::Cut,
        "Paste" => Action::Paste { in_place: rng.coin() },
        "ConvertToSymbol" => Action::ConvertToSymbol,
        "PlaceSymbol" => Action::PlaceSymbol { sym_id: sym_id(rng) },
        "EnterSymbolTimeline" => {
            let gids: Vec<u64> = tl.layers.iter().filter_map(|l| l.keyframe_index_at(tl.current_frame).and_then(|k| l.keyframes.get(k))).flat_map(|k| k.cel.instances().iter().map(|i| i.binding.instance_gid)).collect();
            Action::EnterSymbolTimeline { instance_gid: gids.get(rng.range(gids.len())).copied().unwrap_or(999) }
        }
        "EnterSymbolFromLibrary" => Action::EnterSymbolFromLibrary { sym_id: sym_id(rng) },
        "RenameSymbol" => Action::RenameSymbol { sym_id: sym_id(rng), new_name: format!("S{}", rng.range(5)) },
        "DeleteSymbol" => Action::DeleteSymbol { sym_id: sym_id(rng) },
        "DuplicateSymbol" => Action::DuplicateSymbol { sym_id: sym_id(rng) },
        "SwapSymbol" => Action::SwapSymbol { sym_id: sym_id(rng) },
        "SetSymbolPlayback" => Action::SetSymbolPlayback {
            loop_mode: rng.coin().then(|| [SymbolLoopMode::Loop, SymbolLoopMode::PlayOnce, SymbolLoopMode::SingleFrame][rng.range(3)]),
            first_frame: rng.coin().then(|| rng.range(4)),
        },
        "SetSymbolColorEffect" => Action::SetSymbolColorEffect { effect: ColorEffect { alpha: rng.f64_range(0.2, 1.0) as f32, brightness: rng.f64_range(-0.5, 0.5) as f32 } },
        "ToggleTween" => Action::ToggleTween,
        "SetTweenRotate" => Action::SetTweenRotate { rotate: [TweenRotate::Auto, TweenRotate::Clockwise, TweenRotate::CounterClockwise][rng.range(3)] },
        "SwitchLayerAndFrame" => Action::SwitchLayerAndFrame { layer_idx: layer(rng), frame: frame(rng) },
        "ScrubRulerToFrame" => Action::ScrubRulerToFrame { frame: frame(rng) },
        "SelectTimelineCells" => Action::SelectTimelineCells { l0: layer(rng), f0: frame(rng), l1: layer(rng), f1: frame(rng) },
        "ClearTimelineCells" => Action::ClearTimelineCells,
        "ImportSound" => Action::ImportSound { name: "テスト.wav".to_owned(), bytes: short_wav().into() },
        "SetKeyframeSound" => Action::SetKeyframeSound { sound: rng.coin().then(|| sound_id(rng)) },
        "RenameSound" => Action::RenameSound { sound_id: sound_id(rng), new_name: format!("音{}", rng.range(5)) },
        "DeleteSound" => Action::DeleteSound { sound_id: sound_id(rng) },
        "TrimSound" => {
            let (l, kf_start) = tl.active_sound_slot().map_or((0, 0), |(l, k)| (l, tl.layers[l].start_of(k)));
            Action::TrimSound { layer: l, kf_start, edge: if rng.coin() { crate::sound::SoundEdge::Start } else { crate::sound::SoundEdge::End }, frame: frame(rng) }
        }
        "InsertKeyframe" => Action::InsertKeyframe { copy_prev: rng.coin() },
        "DeleteKeyframe" => Action::DeleteKeyframe,
        "ExtendFrame" => Action::ExtendFrame,
        "RemoveFrame" => Action::RemoveFrame,
        "MoveOrCopyFrames" => {
            let sel = tl.cell_selection.unwrap_or(TimelineCellSelection::single(tl.active_layer, tl.current_frame));
            Action::MoveOrCopyFrames { sel, layer_delta: rng.range(3) as isize - 1, frame_delta: rng.range(5) as isize - 2, copy_mode: rng.coin() }
        }
        "AddLayer" => Action::AddLayer,
        "DeleteActiveLayer" => Action::DeleteActiveLayer,
        "ReorderLayer" => Action::ReorderLayer { src_idx: layer(rng), dst_idx: layer(rng) },
        "SetLayerVisible" => Action::SetLayerVisible { layer_idx: layer(rng), visible: rng.range(4) != 0 },
        "SetLayerLocked" => Action::SetLayerLocked { layer_idx: layer(rng), locked: rng.range(4) == 0 },
        "RenameLayer" => Action::RenameLayer { layer_id: tl.layers.get(rng.range(tl.layers.len())).map_or(999, |l| l.id), new_name: format!("L{}", rng.range(5)) },
        "SetFps" => Action::SetFps { fps: [12, 24, 30][rng.range(3)] },
        "SetStageSize" => Action::SetStageSize { size: Vec2::new([640.0, 800.0, 1280.0][rng.range(3)], [480.0, 600.0, 720.0][rng.range(3)]) },
        "SetTool" => Action::SetTool { tool: Tool::ALL[rng.range(Tool::ALL.len())] },
        "ToggleSelectSubMode" => Action::ToggleSelectSubMode { mode: [SelectSubMode::Reshape, SelectSubMode::AddAnchor, SelectSubMode::DeleteAnchor][rng.range(3)] },
        "Undo" => Action::Undo,
        "Redo" => Action::Redo,
        "EndCoalesce" => Action::EndCoalesce { key: None },
        "EndCoalesceOn" => Action::EndCoalesceOn { end: crate::canvas_tool::CoalesceEnd::KeyPressedAgain },
        other => panic!("でたらめな操作の試しが、種類 {other} を作れない（random_action に足す）"),
    }
}

/// 黒い円の線（中心 (300,300)、半径 100、太さ 16）の内側を、バケツで赤に塗る
fn ring_with_bucket_fill() -> Is2dApp {
    let mut app = Is2dApp::default();
    let c = Point::new(300.0, 300.0);
    let pts: Vec<Point> = (0..=240)
        .map(|i| {
            let a = i as f64 / 240.0 * std::f64::consts::TAU;
            Point::new(c.x + 100.0 * a.cos(), c.y + 100.0 * a.sin())
        })
        .collect();
    run(
        &mut app,
        &[Action::PaintStroke {
            points: pts,
            radius: 8.0,
            color: Color32::BLACK,
        }],
    );
    run(
        &mut app,
        &[Action::BucketFill {
            pt: c,
            color: Color32::RED,
        }],
    );
    app
}

/// 黒い円の線・円をまたぐ赤の横線・上下をバケツで青と緑（3色が接する点が4つある）
fn three_color_junction_scene() -> Is2dApp {
    let mut app = ring_with_bucket_fill();
    run(&mut app, &[Action::Undo]);
    let line: Vec<Point> = (0..=150)
        .map(|i| Point::new(195.0 + 210.0 * i as f64 / 150.0, 300.0))
        .collect();
    run(
        &mut app,
        &[
            Action::PaintStroke {
                points: line,
                radius: 6.0,
                color: Color32::RED,
            },
            Action::BucketFill {
                pt: Point::new(300.0, 250.0),
                color: Color32::BLUE,
            },
            Action::BucketFill {
                pt: Point::new(300.0, 350.0),
                color: Color32::GREEN,
            },
        ],
    );
    app
}

fn layer_map_is_valid(app: &Is2dApp) -> bool {
    crate::planar_map::PlanarMap::from_faces(app.scene().iter()).is_valid()
}

mod map_overlay {
    use super::*;
    use crate::planar_map::{
        LayerBase, LocalEdit, LocalRefusal, MapInput, MapRule, PlanarMap, ReshapeEdit,
    };
    use crate::vector_engine::nearest_on_quad;

    /// 地図 `m`（正しい地図）の、本番と同じ計算に使う写し（面に1つずつ別の番号を記したもの）
    pub(super) fn labeled_base(m: &PlanarMap) -> LayerBase {
        LayerBase::new(
            &m.clone()
                .checked()
                .expect("正しい地図")
                .uniquely_labeled()
                .into_map(),
        )
    }

    /// 輪郭の変形（本番の1本の道：`reshape_change` → `edit_local`）を写し `base` にかけた答え：`Ok(Some(地図))`（書き戻した
    /// 地図）、`Ok(None)`（変わらない）、`Err`（断った：格子の外 `OffGrid`、正しい地図にならない `Unwritable`）。辺・頂点は
    /// いつもある
    pub(super) fn reshaped(
        base: &LayerBase,
        edit: ReshapeEdit,
    ) -> Result<Option<PlanarMap>, LocalRefusal> {
        let change = base.reshape_change(edit).expect("辺・頂点がある");
        match base.edit_local(&change, &[]) {
            LocalEdit::Changed(w) => Ok(Some(w.stored.to_dense())),
            LocalEdit::Same => Ok(None),
            LocalEdit::Refused(r) => Err(r),
        }
    }

    /// バケツ（1本の道 `fill_local`）：地図 `m`（正しい地図）の、点 `p` を含む面を `color` にした地図（面に1つずつ別の番号を
    /// 記した、書き戻した地図）。変わらない・書けないときは `None`
    pub(super) fn bucket(m: &PlanarMap, p: Point, color: Color32) -> Option<PlanarMap> {
        let base = LayerBase::new(&m.clone().checked()?.uniquely_labeled().into_map());
        match base.fill_local(p, color, &[]) {
            LocalEdit::Changed(w) => Some(w.stored.to_dense()),
            _ => None,
        }
    }

    /// 小さな乱数（同じ種なら同じ並び）
    pub(super) struct Rng(pub(super) u64);
    impl Rng {
        pub(super) fn f(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
    }

    pub(super) fn rect_shape(x0: f64, y0: f64, x1: f64, y1: f64, color: Color32) -> FillShape {
        let mut s = crate::vector_engine::make_marquee_rect_shape(kurbo::Rect::new(x0, y0, x1, y1));
        s.recolor(|_| color);
        s
    }

    /// 地図が壊れていないか（`is_valid`：判定の形で、辺が触れない・輪郭が閉じて色がそろう・面が取り出せる）
    pub(super) fn valid(m: &PlanarMap) -> bool {
        m.is_valid()
    }

    /// 2つの図形の並び（後ろほど上）の色を、格子の点で比べる。境目から 0.3px 以内の点は数えない
    pub(super) fn color_diff(a: &[FillShape], b: &[FillShape], step: f64) -> usize {
        let mut diff = 0;
        let mut y = 90.0;
        while y <= 510.0 {
            let mut x = 90.0;
            while x <= 510.0 {
                let p = Point::new(x + 0.0137, y + 0.0173);
                let ca = a
                    .iter()
                    .rev()
                    .find(|s| s.contains_point(p))
                    .map(|s| s.color());
                let cb = b
                    .iter()
                    .rev()
                    .find(|s| s.contains_point(p))
                    .map(|s| s.color());
                if ca != cb {
                    let near = a.iter().chain(b.iter()).any(|s| {
                        s.contours().any(|c| {
                            c.segments.iter().any(|q| {
                                kurbo::Shape::bounding_box(q).inflate(0.3, 0.3).contains(p)
                                    && nearest_on_quad(q, p).1 <= 0.3
                            })
                        })
                    });
                    if !near {
                        diff += 1;
                    }
                }
                x += step;
            }
            y += step;
        }
        diff
    }

    /// 地図だけで描いた絵（ブラシの形を重ね、ときどき消しゴム・バケツ）
    pub(super) fn native_scene(seed: u64, n: usize) -> PlanarMap {
        let mut r = Rng(seed);
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let mut m = PlanarMap {
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        for k in 0..n {
            let next = if k % 4 == 3 {
                let p = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
                bucket(&m, p, colors[(r.f() * 4.0) as usize % 4])
            } else {
                let cnt = 2 + (r.f() * 4.0) as usize;
                let mut pts = Vec::new();
                let mut p = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
                for _ in 0..cnt {
                    let q = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
                    for j in 0..10 {
                        pts.push(p.lerp(q, j as f64 / 10.0));
                    }
                    p = q;
                }
                pts.push(p);
                let radius = 3.0 + r.f() * 10.0;
                let color = colors[(r.f() * 4.0) as usize % 4];
                let stroke =
                    brush_line_shapes_at_zoom(&brush_line_at_zoom(&pts, radius, 1.0), color, 1.0);
                let rule = MapRule::Paint(if k % 6 == 5 { None } else { Some(color) });
                m.overlay(MapInput::Shapes(&stroke), rule)
            };
            if let Some(next) = next {
                m = next;
            }
        }
        m
    }

    /// ブラシ `strokes` 本で描いた地図を、倍率 `scale` で縮めた地図（その座標の格子で正しい地図にしたもの：`realized`）。
    /// とても小さい形の試し（拡大したシンボルの中など。特徴の大きさが格子の目の数十個ほど）
    fn tiny_brush_map(seed: u64, strokes: usize, scale: f64) -> PlanarMap {
        let mut r = Rng(seed);
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let mut m = PlanarMap::default();
        for _ in 0..strokes {
            let cnt = 2 + (r.f() * 4.0) as usize;
            let mut pts = Vec::new();
            let mut p = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
            for _ in 0..cnt {
                let q = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
                for j in 0..10 {
                    pts.push(p.lerp(q, j as f64 / 10.0));
                }
                p = q;
            }
            pts.push(p);
            let color = colors[(r.f() * 4.0) as usize % 4];
            let stroke = brush_line_shapes_at_zoom(
                &brush_line_at_zoom(&pts, 3.0 + r.f() * 10.0, 1.0),
                color,
                1.0,
            );
            if let Some(next) = m.overlay(MapInput::Shapes(&stroke), MapRule::Paint(Some(color))) {
                m = next;
            }
        }
        crate::planar_map::realized(&m, kurbo::Affine::scale(scale))
            .expect("縮めた地図")
            .into_map()
    }

    /// 地図 `m` で、頂点の移動と辺のつまみ（ずらす量は `reach`）を交互に `n` 回確定し、（確定できた回, 正しくない地図の説明）
    fn reshape_failures(m: &PlanarMap, seed: u64, n: usize, reach: f64) -> (usize, Vec<String>) {
        reshapes_of(m, seed, 0..n, reach)
    }

    /// `reshape_failures` の、`k` 回目（`ks` の範囲）だけを確定するもの（乱数は 0 回目から同じ順に引く）
    fn reshapes_of(
        m: &PlanarMap,
        seed: u64,
        ks: std::ops::Range<usize>,
        reach: f64,
    ) -> (usize, Vec<String>) {
        let mut r = Rng(seed);
        let base = labeled_base(m);
        let (mut bad, mut done) = (Vec::new(), 0);
        for k in 0..ks.end {
            let d = kurbo::Vec2::new(r.f() - 0.5, r.f() - 0.5) * reach;
            let pick = r.f();
            if k < ks.start {
                continue;
            }
            let (edit, what) = if k % 2 == 0 {
                let v = (pick * m.nodes.len() as f64) as usize % m.nodes.len();
                let p = m.nodes[v];
                (
                    ReshapeEdit::MoveVertex {
                        orig_vertex: p,
                        target: p + d,
                    },
                    format!("頂点 {v}"),
                )
            } else {
                let e = (pick * m.edges.len() as f64) as usize % m.edges.len();
                let q = m.quad(e);
                (
                    ReshapeEdit::PullEdge {
                        orig_seg: q,
                        grab_t: 0.5,
                        target: q.eval(0.5) + d,
                        follow: true,
                    },
                    format!("辺 {e}"),
                )
            };
            // 確定できた回：書いたか、変わらない。書けない（正しい地図にならない）回は確定できたうえで壊れた回として数える
            // （1本の道は、正しい地図でない結果を書かずに断る：`apply_diff`。お手本との比べは道の中で毎回）。格子の外は数えない
            match reshaped(&base, edit) {
                Ok(next) => {
                    done += 1;
                    if let Some(next) = next {
                        assert!(next.is_valid(), "書いた地図は正しい地図");
                    }
                }
                Err(LocalRefusal::Unwritable) => {
                    done += 1;
                    bad.push(format!("{k}（{what}）：正しい地図にならず書けない"));
                }
                Err(LocalRefusal::OffGrid) => {}
            }
        }
        (done, bad)
    }

    /// 中心 `c`・半径 `r` の、角度 `a` から `b` への弧の2次ベジェ（両端の接線の交点を制御点にする）
    fn arc_quad(c: Point, r: f64, a: f64, b: f64) -> QuadBez {
        let at = |t: f64, rr: f64| c + kurbo::Vec2::new(t.cos(), t.sin()) * rr;
        QuadBez::new(at(a, r), at((a + b) / 2.0, r / ((b - a) / 2.0).cos()), at(b, r))
    }

    /// 輪郭 `segs` の塗りの地図の、本番と同じ計算に使う写し
    fn base_of_outline(segs: Vec<QuadBez>) -> LayerBase {
        let shape = FillShape::new(Color32::RED, crate::vector_engine::Contour::new(segs), vec![]);
        let m = PlanarMap::default()
            .overlay(MapInput::Shapes(&[shape]), MapRule::Paint(Some(Color32::RED)))
            .expect("地図にできる");
        labeled_base(&m)
    }

    /// 地図の辺のうち、点 `p` にいちばん近い辺の曲線
    fn edge_near(base: &LayerBase, p: Point) -> QuadBez {
        let m = base.stored.to_dense();
        (0..m.edges.len())
            .map(|e| m.quad(e))
            .min_by(|a, b| nearest_on_quad(a, p).1.total_cmp(&nearest_on_quad(b, p).1))
            .expect("辺がある")
    }

    /// 引っぱりで、まわりの辺が、つかんだ所から離れるほど小さく動きながらついてくる（利用者が決めた）。丸の上の辺を引くと、
    /// つかんだ点はちょうどカーソルの所へ行き、頂点の動きは、つかんだ所から離れるほど小さく、向きが `PULL_REACH_TURN`
    /// 回った先（下半分）は動かない。確定（1本の道。お手本との比べは道の中で毎回）もできる
    #[test]
    fn pulling_a_circle_drags_its_neighbours_less_and_less_but_not_the_far_side() {
        let c = Point::new(300.0, 300.0);
        let n = 16;
        let step = std::f64::consts::TAU / n as f64;
        let base = base_of_outline((0..n).map(|k| arc_quad(c, 100.0, k as f64 * step, (k + 1) as f64 * step)).collect());
        let top = Point::new(300.0, 200.0);
        let q = edge_near(&base, top + kurbo::Vec2::new(10.0, 0.0));
        let t = 0.4;
        let target = q.eval(t) + kurbo::Vec2::new(0.0, -30.0);
        let moved = base.pulled_edges(crate::planar_map::PullGrab::Edge { orig_seg: q, grab_t: t, follow: true }, target).expect("辺がある");
        assert!(moved.len() > 1, "まわりの辺がついてこない");
        // つかんだ点はカーソルの所
        let (e0, grabbed) = moved[0];
        let tt = if base.stored.edge_at(e0).quad().p0 == q.p0 { t } else { 1.0 - t };
        let got = grabbed.eval(tt);
        assert!((got - target).hypot() < 1e-9, "つかんだ点がカーソルの所へ行かない：{got:?} {target:?}");
        // 頂点の動き：つかんだ所から（丸の上で）離れるほど小さく、下半分は動かない
        let grab_angle = (q.eval(t) - c).atan2();
        let mut moves: Vec<(f64, f64)> = Vec::new();
        for &(e, after) in &moved {
            let before = base.stored.edge_at(e).quad();
            for (b, a) in [(before.p0, after.p0), (before.p2, after.p2)] {
                let off = ((b - c).atan2() - grab_angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                moves.push((off, (a - b).hypot()));
                if b.y > c.y {
                    assert_eq!(a, b, "下半分の頂点が動いた");
                }
            }
        }
        for side in [1.0, -1.0] {
            let mut v: Vec<(f64, f64)> = moves.iter().filter(|(o, _)| o * side > 0.0).map(|&(o, d)| (o.abs(), d)).collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert!(v.windows(2).all(|w| w[1].1 <= w[0].1 + 1e-12), "離れるほど小さくならない：{v:?}");
            assert!(v.first().is_some_and(|f| f.1 > 0.0), "片側の頂点が動かない");
            assert_eq!(v.last().map(|l| l.1), Some(0.0), "ついてくる範囲の端の頂点が動いた");
        }
        assert!(matches!(reshaped(&base, ReshapeEdit::PullEdge { orig_seg: q, grab_t: t, target, follow: true }), Ok(Some(_))));
    }

    /// 角（向きが `PULL_REACH_TURN` 以上変わる頂点）から先はついてこない。半円の直径（両端が角の1本の辺）を引くと、その辺
    /// だけが前の決まり（両端を動かさず制御点を Δ / (2t(1−t)) 動かす）どおりに動く。角の隣の弧を引くと、角の頂点と直径は
    /// 動かない
    #[test]
    fn pulling_stops_at_corners_and_a_lone_edge_moves_as_before() {
        let c = Point::new(300.0, 300.0);
        let n = 8;
        let step = std::f64::consts::PI / n as f64;
        let mut segs: Vec<QuadBez> = (0..n)
            .map(|k| arc_quad(c, 100.0, std::f64::consts::PI + k as f64 * step, std::f64::consts::PI + (k + 1) as f64 * step))
            .collect();
        segs.push(QuadBez::new(Point::new(400.0, 300.0), Point::new(300.0, 300.0), Point::new(200.0, 300.0)));
        let base = base_of_outline(segs);
        // 直径：1本だけ、前の決まりどおり
        let d = edge_near(&base, Point::new(260.0, 300.0));
        let t = 0.3;
        let target = d.eval(t) + kurbo::Vec2::new(5.0, 25.0);
        let moved = base.pulled_edges(crate::planar_map::PullGrab::Edge { orig_seg: d, grab_t: t, follow: true }, target).expect("辺がある");
        assert_eq!(moved.len(), 1, "角の先の辺がついてきた");
        let (e, after) = moved[0];
        let before = base.stored.edge_at(e).quad();
        let tt = if before.p0 == d.p0 { t } else { 1.0 - t };
        let want = before.p1 + (target - before.eval(tt)) / (2.0 * tt * (1.0 - tt));
        assert_eq!((after.p0, after.p2), (before.p0, before.p2), "両端の角が動いた");
        assert!((after.p1 - want).hypot() < 1e-9, "前の決まりと違う：{:?} {want:?}", after.p1);
        // 角（左端）の隣の弧：角と直径は動かない
        let corner = Point::new(200.0, 300.0);
        let a = edge_near(&base, Point::new(203.0, 285.0));
        let moved = base.pulled_edges(crate::planar_map::PullGrab::Edge { orig_seg: a, grab_t: 0.5, follow: true }, a.eval(0.5) + kurbo::Vec2::new(-10.0, -10.0)).expect("辺がある");
        assert!(moved.len() > 1, "弧の隣の弧がついてこない");
        // まわりがついてこない引っぱり（アンカーを表示しているとき）：なめらかにつながった弧の途中でも、つかんだ1区間だけが
        // 前の決まりどおりに曲がる
        let target = a.eval(0.5) + kurbo::Vec2::new(-10.0, -10.0);
        let alone = base
            .pulled_edges(crate::planar_map::PullGrab::Edge { orig_seg: a, grab_t: 0.5, follow: false }, target)
            .expect("辺がある");
        assert_eq!(alone.len(), 1, "ついてこない引っぱりで、ほかの辺が動いた");
        let (e, after) = alone[0];
        let before = base.stored.edge_at(e).quad();
        let want = before.p1 + (target - before.eval(0.5)) / 0.5;
        assert_eq!((after.p0, after.p2), (before.p0, before.p2));
        assert!((after.p1 - want).hypot() < 1e-9, "前の決まりと違う：{:?} {want:?}", after.p1);
        for &(e, after) in &moved {
            let before = base.stored.edge_at(e).quad();
            assert!(before.p0.y < 300.0 - 1e-6 || before.p2.y < 300.0 - 1e-6, "直径がついてきた");
            for (b, a) in [(before.p0, after.p0), (before.p2, after.p2)] {
                if (b - corner).hypot() < 1e-3 {
                    assert_eq!(a, b, "角の頂点が動いた");
                }
            }
        }
    }

    /// アンカー（頂点）を引っぱると、線の上を引っぱるのと同じく、まわりがついてくる（アンカーを表示していないとき。利用者が
    /// 決めた）。丸の頂点を引くと、その頂点はちょうどカーソルの所へ行き、まわりは離れるほど小さく動き、下半分は動かない
    #[test]
    fn pulling_a_circle_vertex_drags_its_neighbours_less_and_less() {
        let c = Point::new(300.0, 300.0);
        let n = 16;
        let step = std::f64::consts::TAU / n as f64;
        let base = base_of_outline((0..n).map(|k| arc_quad(c, 100.0, k as f64 * step, (k + 1) as f64 * step)).collect());
        let m = base.stored.to_dense();
        let v = *m.nodes.iter().min_by(|a, b| a.y.total_cmp(&b.y)).expect("頂点がある");
        let target = v + kurbo::Vec2::new(8.0, -30.0);
        let moved = base.pulled_edges(crate::planar_map::PullGrab::Vertex(v), target).expect("頂点がある");
        assert!(moved.len() > 2, "まわりの辺がついてこない");
        let grab_angle = (v - c).atan2();
        let mut moves: Vec<(f64, f64)> = Vec::new();
        for &(e, after) in &moved {
            let before = base.stored.edge_at(e).quad();
            for (b, a) in [(before.p0, after.p0), (before.p2, after.p2)] {
                if b == v {
                    assert_eq!(a, target, "つかんだ頂点がカーソルの所へ行かない");
                    continue;
                }
                let off = ((b - c).atan2() - grab_angle + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                moves.push((off, (a - b).hypot()));
                if b.y > c.y {
                    assert_eq!(a, b, "下半分の頂点が動いた");
                }
            }
        }
        for side in [1.0, -1.0] {
            let mut d: Vec<(f64, f64)> = moves.iter().filter(|(o, _)| o * side > 0.0).map(|&(o, d)| (o.abs(), d)).collect();
            d.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert!(d.windows(2).all(|w| w[1].1 <= w[0].1 + 1e-12), "離れるほど小さくならない：{d:?}");
            assert!(d.first().is_some_and(|f| f.1 > 0.0), "片側の頂点が動かない");
            assert_eq!(d.last().map(|l| l.1), Some(0.0), "ついてくる範囲の端の頂点が動いた");
        }
        assert!(matches!(reshaped(&base, ReshapeEdit::PullVertex { orig_vertex: v, target }), Ok(Some(_))));
    }

    /// 頂点から出る辺が、みな隣の角で止まるとき（正方形の角）、頂点の引っぱりは、頂点だけを動かす決まり（`moved_quad`：
    /// 頂点を動かし、制御点を半分動かす）と同じ曲線になる（アンカーを表示しているときの動かし方と、表示していないときの
    /// 動かし方が、ここでは同じ形）
    #[test]
    fn pulling_a_square_corner_moves_it_like_moving_the_vertex_alone() {
        let p = [Point::new(200.0, 200.0), Point::new(400.0, 200.0), Point::new(400.0, 400.0), Point::new(200.0, 400.0)];
        let base = base_of_outline((0..4).map(|k| { let (a, b) = (p[k], p[(k + 1) % 4]); QuadBez::new(a, a.midpoint(b), b) }).collect());
        let v = p[0];
        let target = Point::new(185.0, 190.0);
        let mut moved = base.pulled_edges(crate::planar_map::PullGrab::Vertex(v), target).expect("頂点がある");
        assert_eq!(moved.len(), 2, "角の先の辺がついてきた");
        moved.sort_by_key(|&(e, _)| e);
        for (e, after) in moved {
            let want = crate::planar_map::moved_quad(base.stored.edge_at(e).quad(), v, target);
            for (a, b) in [(after.p0, want.p0), (after.p1, want.p1), (after.p2, want.p2)] {
                assert!((a - b).hypot() < 1e-9, "頂点だけを動かす決まりと違う：{after:?} {want:?}");
            }
        }
    }

    /// とても小さい形でも、頂点の移動・辺のつまみの結果は正しい地図（レビュー 7f63b04 の N-1002）。直す前（N-1003 の前）は、
    /// 倍率 1e-4 で 200 回中 58 回が壊れた地図になり、確定が黙って断られた。原因は3つあった：
    /// - 重ねる線の端に入った t の幅 0 の区間で、区間の向きが決まらなかった（N-1003 の `Span` で作らなくなった）
    /// - i_overlay が交点を区間の端の外へ寄せると、切った区間の t の幅が 0 になり、同じく向きが決まらなかった（区間の向きを
    ///   `Prov::fwd` で持つ。曲線の折り返しの打ち消し：種 99・倍率 1e-3 の 114 回目。左右の色の材料：種 17・倍率 5e-5 の
    ///   218・296 回目）
    /// - 穴だけで囲まれた所の色を、穴の輪の材料だけで決めていた（それを囲む所の色にする：`hole_only_groups`。種 17・
    ///   倍率 1e-4 の 1 回目）
    #[test]
    fn tiny_scale_reshapes_keep_map_valid() {
        let m = tiny_brush_map(99, 40, 1e-4);
        let (done, bad) = reshape_failures(&m, 7, 200, 6e-4);
        assert!(done > 180, "確定できた回 {done}");
        assert!(bad.is_empty(), "正しくない地図 {} 回：{:?}", bad.len(), bad);
        // 直す前に壊れた回（種, 倍率, 何回目）
        for (seed, scale, k) in [
            (99, 1e-3, 114),
            (17, 1e-4, 1),
            (17, 5e-5, 218),
            (17, 5e-5, 296),
        ] {
            let m = tiny_brush_map(seed, 40, scale);
            assert!(valid(&m), "{}", m.validity_report());
            let (done, bad) = reshapes_of(&m, seed + 1, k..k + 1, 6.0 * scale);
            assert_eq!(done, 1, "種 {seed}・倍率 {scale}・{k} 回目を確定できない");
            assert!(
                bad.is_empty(),
                "種 {seed}・倍率 {scale}：正しくない地図 {} 回：{:?}",
                bad.len(),
                bad
            );
        }
    }

    /// とても小さい形の頂点の移動・辺のつまみを、種と倍率を変えて数える
    #[test]
    #[ignore]
    fn probe_tiny_scale_reshapes() {
        for seed in [99, 5, 17, 23] {
            for scale in [1e-3, 3e-4, 1e-4, 5e-5] {
                let m = tiny_brush_map(seed, 40, scale);
                let (done, bad) = reshape_failures(&m, seed + 1, 400, 6.0 * scale);
                eprintln!(
                    "PROBE 種 {seed}・倍率 {scale:e}：辺 {}・確定 {done}・正しくない地図 {}",
                    m.edges.len(),
                    bad.len()
                );
                if std::env::var("PROBE_DETAIL").is_ok() {
                    for b in &bad {
                        eprintln!(
                            "PROBE   {}",
                            b.replace('\n', " | ").chars().take(900).collect::<String>()
                        );
                    }
                }
            }
        }
    }

    /// 地図だけで描く試し（`native_scene` と同じ作り方）を種 `seed` で `steps` 手行い、結果が正しい地図でなくなった最初の手
    /// （無ければ `None`）
    fn native_steps(seed: u64, steps: usize) -> Option<String> {
        native_run(seed, steps).err()
    }

    /// `native_steps` と同じ試しで、できた地図（途中で正しい地図でなくなれば、その手の説明）
    fn native_run(seed: u64, steps: usize) -> Result<PlanarMap, String> {
        let mut r = Rng(seed);
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let mut m = PlanarMap {
            nodes: Vec::new(),
            edges: Vec::new(),
        };
        for k in 0..steps {
            let next = if k % 4 == 3 {
                let p = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
                bucket(&m, p, colors[(r.f() * 4.0) as usize % 4])
            } else {
                let cnt = 2 + (r.f() * 4.0) as usize;
                let mut pts = Vec::new();
                let mut p = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
                for _ in 0..cnt {
                    let q = Point::new(100.0 + r.f() * 400.0, 100.0 + r.f() * 400.0);
                    for j in 0..10 {
                        pts.push(p.lerp(q, j as f64 / 10.0));
                    }
                    p = q;
                }
                pts.push(p);
                let radius = 3.0 + r.f() * 10.0;
                let color = colors[(r.f() * 4.0) as usize % 4];
                let stroke =
                    brush_line_shapes_at_zoom(&brush_line_at_zoom(&pts, radius, 1.0), color, 1.0);
                let rule = MapRule::Paint(if k % 6 == 5 { None } else { Some(color) });
                m.overlay(MapInput::Shapes(&stroke), rule)
            };
            if let Some(next) = next {
                if !next.is_valid() {
                    return Err(format!("seed {seed} step {k}: {}", next.validity_report()));
                }
                m = next;
            }
        }
        Ok(m)
    }

    /// 地図だけで描いた地図（種 `seed`）に、頂点のすぐそば（0〜1e-3px）を通る縁の形と、前に重ねた形を 0〜1e-3px ずらした形を
    /// 交互に `steps` 回重ね、（重ねた回数, 正しい地図でない結果の数, 正しくない結果の説明）を返す（レビュー 4a9ea60 の N-1001）
    pub(super) fn near_vertex_run(seed: u64, steps: usize) -> (usize, usize, Vec<String>) {
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        // 中心 `c`、向き `u`、長さ 2·len・幅 w の長方形（直線の縁）
        let bar = |c: Point, u: KVec2, len: f64, w: f64, color: Color32| {
            let n = KVec2::new(-u.y, u.x);
            let pts = [
                c - u * len,
                c + u * len,
                c + u * len + n * w,
                c - u * len + n * w,
            ];
            let segs: Vec<QuadBez> = (0..4)
                .map(|i| {
                    let (a, b) = (pts[i], pts[(i + 1) % 4]);
                    QuadBez::new(a, a.midpoint(b), b)
                })
                .collect();
            FillShape::new(color, crate::vector_engine::Contour::new(segs), Vec::new())
        };
        let (mut n, mut bad) = (0, 0);
        let mut notes = Vec::new();
        let Ok(mut m) = native_run(seed, 40) else {
            return (0, 0, notes);
        };
        let mut r = Rng(seed * 104_729 + 7);
        let mut last: Vec<FillShape> = Vec::new();
        for k in 0..steps {
            if m.edges.is_empty() {
                break;
            }
            let color = colors[(r.f() * 4.0) as usize % 4];
            let shapes: Vec<FillShape> = if k % 2 == 0 || last.is_empty() {
                // 頂点のすぐそばを通る縁（直線の縁か、ブラシの縁。縁のいちばん近い点が、頂点から d の所に来るよう動かす）
                let v = m.edges[(r.f() * m.edges.len() as f64) as usize % m.edges.len()].a;
                let ang = r.f() * std::f64::consts::TAU;
                let u = KVec2::new(ang.cos(), ang.sin());
                let d = [0.0, 1e-6, 1e-5, 1e-4, 1e-3][(r.f() * 5.0) as usize % 5];
                let shape = if k % 4 == 0 {
                    bar(
                        m.nodes[v],
                        u,
                        20.0 + r.f() * 60.0,
                        2.0 + r.f() * 10.0,
                        color,
                    )
                } else {
                    let p0 = m.nodes[v] + u * (r.f() * 40.0 - 20.0);
                    let pts: Vec<Point> = (0..=12)
                        .map(|j| {
                            let t = j as f64 / 12.0;
                            p0 + u * (t * 80.0) + KVec2::new(-u.y, u.x) * ((t * 6.0).sin() * 8.0)
                        })
                        .collect();
                    brush_line_shapes_at_zoom(
                        &brush_line_at_zoom(&pts, 2.0 + r.f() * 8.0, 1.0),
                        color,
                        1.0,
                    )
                    .remove(0)
                };
                let near = shape
                    .contours()
                    .flat_map(|c| c.segments.iter())
                    .map(|q| q.eval(kurbo::ParamCurveNearest::nearest(q, m.nodes[v], 1e-12).t))
                    .min_by(|a, b| {
                        (*a - m.nodes[v])
                            .hypot()
                            .total_cmp(&(*b - m.nodes[v]).hypot())
                    })
                    .unwrap_or(m.nodes[v]);
                let ang2 = r.f() * std::f64::consts::TAU;
                let t = kurbo::Affine::translate(
                    (m.nodes[v] - near) + KVec2::new(ang2.cos(), ang2.sin()) * d,
                );
                let mut shape = shape;
                shape.transform(t);
                vec![shape]
            } else {
                // 前に重ねた形を、ごくわずかずらしてなぞり直す
                let ang = r.f() * std::f64::consts::TAU;
                let d = r.f() * 1e-3;
                let t = kurbo::Affine::translate(KVec2::new(ang.cos(), ang.sin()) * d);
                last.iter()
                    .map(|s| {
                        let mut s = s.clone();
                        s.transform(t);
                        s
                    })
                    .collect()
            };
            let rule = MapRule::Paint(if k % 5 == 4 { None } else { Some(color) });
            let Some(next) = m.overlay(MapInput::Shapes(&shapes), rule) else {
                continue;
            };
            n += 1;
            if !next.is_valid() {
                bad += 1;
                if notes.len() < 3 {
                    notes.push(format!("seed {seed} step {k}: {}", next.validity_report()));
                }
                continue;
            }
            last = shapes;
            m = next;
        }
        (n, bad, notes)
    }

    /// 地図だけで描いた地図に、頂点のすぐそば（1e-6〜1e-3px）を通る直線の縁の形と、前に重ねた形を 0〜1e-3px ずらした形を
    /// 重ね、正しい地図でない結果を数える（レビュー 4a9ea60 の N-1001）
    #[test]
    #[ignore]
    fn probe_near_vertex_overlays() {
        let seeds: u64 = std::env::var("PROBE_SEEDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        let steps: usize = std::env::var("PROBE_STEPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        let results = crate::planar_map::parallel_map(&(0..seeds).collect::<Vec<_>>(), |&s| {
            near_vertex_run(s, steps)
        });
        let (n, bad) = results.iter().fold((0, 0), |a, x| (a.0 + x.0, a.1 + x.1));
        for x in &results {
            for note in &x.2 {
                eprintln!("PROBE   {note}");
            }
        }
        eprintln!("PROBE 頂点のそば・ずらしたなぞり直し {n} 回：正しくない地図 {bad}");
    }

    /// 頂点のすぐそばを通る形と、ずらしたなぞり直しを重ねても、正しい地図のまま（直す前のコードでは、これらの種で、
    /// 入れなかった短い辺の色が、色の変わったとなりの面と食い違った。レビュー 4a9ea60 の N-1001）
    #[test]
    fn near_vertex_overlays_keep_face_colors() {
        for seed in [20, 39, 55, 70] {
            let (n, bad, notes) = near_vertex_run(seed, 100);
            assert!(n > 0, "種 {seed}：重ねられなかった");
            assert_eq!(bad, 0, "種 {seed}：{notes:?}");
        }
    }

    /// 地図だけで描いた地図で、画面から届く輪郭の変形（頂点の移動と、両端から 9px より遠い所の引っぱり。2〜62px）を確定し、
    /// 正しい地図でない結果・取り消し（結果が元のまま）・動かした所の外で色が変わった所を数える（レビュー 08c15dd の N-1002）
    #[test]
    #[ignore]
    fn probe_reshape_commits() {
        let maps: u64 = std::env::var("PROBE_MAPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        let per: usize = std::env::var("PROBE_EDITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40);
        let run = |seed: u64| -> (usize, usize, usize, usize, Vec<String>) {
            let (mut n, mut invalid, mut cancelled, mut outside) = (0, 0, 0, 0);
            let mut notes = Vec::new();
            let Ok(m) = native_run(seed, if seed.is_multiple_of(2) { 30 } else { 70 }) else {
                return (0, 0, 0, 0, notes);
            };
            if m.edges.is_empty() {
                return (0, 0, 0, 0, notes);
            }
            let before = m.color_locator();
            let base = labeled_base(&m);
            let mut r = Rng(seed * 7919 + 1);
            for i in 0..per {
                let dist = 2.0 + r.f() * 60.0;
                let ang = r.f() * std::f64::consts::TAU;
                let d = KVec2::new(ang.cos(), ang.sin()) * dist;
                let (edit, moved): (ReshapeEdit, Vec<usize>) = if i % 2 == 0 {
                    let v = (r.f() * m.nodes.len() as f64) as usize % m.nodes.len();
                    let p = m.nodes[v];
                    (
                        ReshapeEdit::MoveVertex {
                            orig_vertex: p,
                            target: p + d,
                        },
                        m.edges_at(v),
                    )
                } else {
                    let e = (r.f() * m.edges.len() as f64) as usize % m.edges.len();
                    let t = 0.18 + r.f() * 0.64;
                    let q = m.quad(e);
                    let p = q.eval(t);
                    if (p - q.p0).hypot() <= 9.0 || (p - q.p2).hypot() <= 9.0 {
                        continue;
                    }
                    (
                        ReshapeEdit::PullEdge {
                            orig_seg: q,
                            grab_t: t,
                            target: p + d,
                            follow: true,
                        },
                        vec![e],
                    )
                };
                n += 1;
                // 変わらない・格子の外は取り消し、正しい地図にならず書けないものは正しくない結果
                let next = match reshaped(&base, edit) {
                    Ok(Some(next)) => next,
                    Ok(None) | Err(LocalRefusal::OffGrid) => {
                        cancelled += 1;
                        continue;
                    }
                    Err(LocalRefusal::Unwritable) => {
                        invalid += 1;
                        if notes.len() < 3 {
                            notes.push(format!("seed {seed} edit {i}: 正しい地図にならず書けない"));
                        }
                        continue;
                    }
                };
                // 動かした辺が届く所（元の外接矩形を、動かした量の2倍と 1px 広げる）の外で、色が変わった点（4px おき）
                let reach = moved
                    .iter()
                    .map(|&e| kurbo::ParamCurveExtrema::bounding_box(&m.quad(e)))
                    .reduce(|a, b| a.union(b))
                    .unwrap_or_default()
                    .inflate(2.0 * dist + 1.0, 2.0 * dist + 1.0);
                let mut changed = 0;
                let after = next.color_locator();
                for gx in 0..150 {
                    for gy in 0..150 {
                        let p = Point::new(
                            100.0 + gx as f64 * 4.0 + 0.37,
                            100.0 + gy as f64 * 4.0 + 0.61,
                        );
                        if !reach.contains(p) && before(p) != after(p) {
                            changed += 1;
                        }
                    }
                }
                if changed > 0 {
                    outside += 1;
                    if notes.len() < 3 {
                        notes.push(format!(
                            "seed {seed} edit {i}: 外で色が変わった点 {changed}"
                        ));
                    }
                }
            }
            (n, invalid, cancelled, outside, notes)
        };
        let t0 = std::time::Instant::now();
        let res: Vec<(usize, usize, usize, usize, Vec<String>)> = std::thread::scope(|sc| {
            // テストの計算を並べて走らせる範囲のスレッド（止まったらテストへ伝える）
            #[allow(clippy::disallowed_methods)]
            let hs: Vec<_> = (0..2u64)
                .map(|t| {
                    sc.spawn(move || {
                        (0..maps)
                            .filter(|s| s % 2 == t)
                            .map(run)
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        type Row = (usize, usize, usize, usize, Vec<String>);
        let sum = |f: fn(&Row) -> usize| res.iter().map(f).sum::<usize>();
        for r in &res {
            for n in &r.4 {
                eprintln!("PROBE {n}");
            }
        }
        eprintln!(
            "PROBE 変形の確定 {} 回：正しくない結果 {}・取り消し {}・外で色が変わった回 {}（{:.1}s）",
            sum(|r| r.0),
            sum(|r| r.1),
            sum(|r| r.2),
            sum(|r| r.3),
            t0.elapsed().as_secs_f64()
        );
    }

    /// 重ね合わせが正しい地図を保つ（レビュー 08c15dd の N-1001 の再現）。種 31 の 64 手目：中の辺の内外を浮動小数の半直線で
    /// 決めて反対の側を読んでいた。種 2342 の 28 手目：i_overlay が、向きが逆になって重なった2本の区間の数を打ち消して、
    /// 区間ごと消していた
    #[test]
    fn native_overlay_seeds_31_and_2342_stay_valid() {
        assert_eq!(native_steps(31, 65), None);
        assert_eq!(native_steps(2342, 29), None);
    }

    /// 点のある所の色（判定の形の整数の位置決め）：半直線が頂点や水平な区間を通るとき、点が辺の上にあるとき
    #[test]
    fn color_at_through_vertices_and_on_edges() {
        // 赤の正方形の中に、頂点を共有する青の三角形（頂点 (200,150) を通る横の半直線、底辺が水平）
        let red = rect_shape(100.0, 100.0, 300.0, 300.0, Color32::RED);
        let tri = [
            Point::new(150.0, 150.0),
            Point::new(250.0, 150.0),
            Point::new(200.0, 250.0),
        ];
        let segs = (0..3)
            .map(|i| {
                let (a, b) = (tri[i], tri[(i + 1) % 3]);
                QuadBez::new(a, a.midpoint(b), b)
            })
            .collect();
        let blue = FillShape::new(
            Color32::BLUE,
            crate::vector_engine::Contour::new(segs),
            Vec::new(),
        );
        let m = PlanarMap::from_faces(std::iter::once(&red)).overlay(
            MapInput::Shapes(std::slice::from_ref(&blue)),
            MapRule::Paint(Some(Color32::BLUE)),
        );
        let m = m.expect("重ねられる");
        assert!(valid(&m));
        for (p, want) in [
            ((120.0, 150.0), Some(Color32::RED)), // 横の半直線が三角形の底辺（水平）と右上の正方形の角を通る
            ((175.0, 150.0), Some(Color32::BLUE)), // 底辺の上（右上へずらした所＝三角形の中）
            ((120.0, 250.0), Some(Color32::RED)), // 半直線が三角形の頂点 (200,250) を通る
            ((200.0, 200.0), Some(Color32::BLUE)),
            ((50.0, 150.0), None), // 外（半直線が正方形の左の辺・三角形を横切る）
            ((300.0, 200.0), None), // 正方形の右の辺の上（右へずらした所＝外）
            ((100.0, 200.0), Some(Color32::RED)), // 正方形の左の辺の上（右へずらした所＝中）
        ] {
            assert_eq!(m.color_at(Point::new(p.0, p.1)), want, "{p:?}");
        }
    }

    /// 地図だけで描く試し（`native_scene` と同じ作り方）を多くの種で行い、重ね合わせの結果が正しい地図でなくなった所を数える
    /// （レビュー 08c15dd の N-1001 の再現）
    #[test]
    #[ignore]
    fn probe_native_overlay_failures() {
        let seeds: u64 = std::env::var("PROBE_SEEDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3000);
        let steps: usize = std::env::var("PROBE_STEPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(70);
        let run = |seed: u64| -> Vec<String> { native_steps(seed, steps).into_iter().collect() };
        let threads = 2u64;
        let t0 = std::time::Instant::now();
        let fails: Vec<String> = std::thread::scope(|sc| {
            // テストの計算を並べて走らせる範囲のスレッド（止まったらテストへ伝える）
            #[allow(clippy::disallowed_methods)]
            let hs: Vec<_> = (0..threads)
                .map(|t| {
                    sc.spawn(move || {
                        // PROBE_ONLY：その種だけ回す
                        let only: Option<u64> = std::env::var("PROBE_ONLY")
                            .ok()
                            .and_then(|v| v.parse().ok());
                        (0..seeds)
                            .filter(|s| only.is_none_or(|o| *s == o))
                            .filter(|s| s % threads == t)
                            .flat_map(run)
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        });
        for f in &fails {
            eprintln!("PROBE {}", f.chars().take(300).collect::<String>());
        }
        eprintln!(
            "PROBE 種 {seeds}・{steps} 手：正しくない地図になった種 {}（{:.1}s）",
            fails.len(),
            t0.elapsed().as_secs_f64()
        );
    }

    #[test]
    fn paint_square_over_square() {
        let base = PlanarMap::from_faces(std::iter::once(&rect_shape(
            100.0,
            100.0,
            300.0,
            300.0,
            Color32::RED,
        )));
        let blue = rect_shape(200.0, 200.0, 400.0, 400.0, Color32::BLUE);
        let m = base
            .overlay(
                MapInput::Shapes(std::slice::from_ref(&blue)),
                MapRule::Paint(Some(Color32::BLUE)),
            )
            .unwrap();
        assert!(valid(&m));
        let (faces, _) = m.to_faces_checked(1, &[]);
        let at = |p: Point| faces.iter().find(|s| s.contains_point(p)).map(|s| s.color());
        assert_eq!(at(Point::new(150.0, 150.0)), Some(Color32::RED));
        assert_eq!(at(Point::new(250.0, 250.0)), Some(Color32::BLUE));
        assert_eq!(at(Point::new(350.0, 350.0)), Some(Color32::BLUE));
        assert_eq!(at(Point::new(350.0, 150.0)), None);
        // 消しゴム
        let erased = m
            .overlay(
                MapInput::Shapes(&[rect_shape(120.0, 120.0, 180.0, 180.0, Color32::WHITE)]),
                MapRule::Paint(None),
            )
            .unwrap();
        let (faces, _) = erased.to_faces_checked(1, &[]);
        assert_eq!(
            faces
                .iter()
                .find(|s| s.contains_point(Point::new(150.0, 150.0)))
                .map(|s| s.color()),
            None
        );
        assert_eq!(
            faces
                .iter()
                .find(|s| s.contains_point(Point::new(110.0, 110.0)))
                .map(|s| s.color()),
            Some(Color32::RED)
        );
    }

    #[test]
    fn same_color_paint_merges_without_new_edges() {
        let base = PlanarMap::from_faces(std::iter::once(&rect_shape(
            100.0,
            100.0,
            300.0,
            300.0,
            Color32::RED,
        )));
        let inner = rect_shape(150.0, 150.0, 250.0, 250.0, Color32::RED);
        let m = base
            .overlay(
                MapInput::Shapes(std::slice::from_ref(&inner)),
                MapRule::Paint(Some(Color32::RED)),
            )
            .unwrap();
        assert!(valid(&m));
        assert_eq!(m.edges.len(), base.edges.len());
        // 元の辺と元の図形の印がそのまま残る（図形を書き直さずに済む）
        assert!(m
            .edges
            .iter()
            .all(|e| e.left_src.is_some() || e.right_src.is_some()));
    }

    /// 矩形で中を取り出して空けたあと、そのまま上に置くと元の色に戻る
    #[test]
    fn marquee_lift_and_drop_back() {
        let base = native_scene(1, 24);
        let (orig, _) = base.to_faces_checked(1, &[]);
        let rect = PlanarMap::from_faces(std::iter::once(&rect_shape(
            180.0,
            170.0,
            330.0,
            290.0,
            Color32::WHITE,
        )));
        let inside = base
            .overlay(MapInput::Map(&rect), MapRule::KeepInside)
            .unwrap();
        let rest = base
            .overlay(MapInput::Map(&rect), MapRule::Paint(None))
            .unwrap();
        assert!(valid(&inside) && valid(&rest));
        let back = rest
            .overlay(MapInput::Map(&inside), MapRule::Overlay)
            .unwrap();
        assert!(valid(&back));
        let (faces, _) = back.to_faces_checked(1, &[]);
        assert_eq!(color_diff(&orig, &faces, 3.0), 0);
    }

    /// 塗りの色だけを替えた写し（となり合う面は、替えたあとも違う色）
    fn recolored(m: &PlanarMap) -> PlanarMap {
        let shift = |c: Option<Color32>| {
            c.map(|c| match c {
                Color32::RED => Color32::BLUE,
                Color32::BLUE => Color32::GREEN,
                Color32::GREEN => Color32::BLACK,
                Color32::BLACK => Color32::RED,
                c => c,
            })
        };
        let mut out = m.clone();
        for e in &mut out.edges {
            e.left = shift(e.left);
            e.right = shift(e.right);
        }
        out
    }

    fn face_area(s: &FillShape) -> f64 {
        s.exterior().signed_area().abs() - s.holes().iter().map(|h| h.signed_area().abs()).sum::<f64>()
    }

    /// 切り出した片を、切る前の（元の線が残っている）地図へその場で重ねても、境目に沿った見えない欠片の面ができず、辺も
    /// 増えない：重なる線は同じ線として、2本とも同じ点の折れ線にする（`Span`。レビュー 7f63b04 の N-1003。直す前は、色を
    /// 替えた片で面積 0.001px² 未満の面が百個以上、辺が 1.4〜2.3 倍になった）
    #[test]
    fn piece_pasted_onto_uncut_map_adds_no_slivers() {
        for seed in [3, 7] {
            let base = native_scene(seed, 30);
            let rect = PlanarMap::from_faces(std::iter::once(&rect_shape(
                170.0,
                160.0,
                390.0,
                340.0,
                Color32::WHITE,
            )));
            let piece = base
                .overlay(MapInput::Map(&rect), MapRule::KeepInside)
                .unwrap();
            let rest = base
                .overlay(MapInput::Map(&rect), MapRule::Paint(None))
                .unwrap();
            // 同じ色の片は元の地図のまま。色を替えた片は、切って空けた所へ置いたとき（重なる線が無い）と同じ地図
            let into_hole =
                |p: &PlanarMap| rest.overlay(MapInput::Map(p), MapRule::Overlay).unwrap();
            let other = recolored(&piece);
            for (name, p, expected) in [
                ("同じ色", &piece, base.clone()),
                ("色を替えた", &other, into_hole(&other)),
            ] {
                let m = base.overlay(MapInput::Map(p), MapRule::Overlay).unwrap();
                assert!(valid(&m), "種 {seed} {name}: {}", m.validity_report());
                let (faces, _) = m.to_faces_checked(1, &[]);
                let tiny = faces.iter().filter(|s| face_area(s) < 0.001).count();
                assert_eq!(tiny, 0, "種 {seed} {name}：欠片の面");
                assert_eq!(
                    m.edges.len(),
                    expected.edges.len(),
                    "種 {seed} {name}：辺の数"
                );
                let (want, _) = expected.to_faces_checked(1, &[]);
                assert_eq!(color_diff(&want, &faces, 3.0), 0, "種 {seed} {name}");
            }
        }
    }

    /// 形の線が前からある辺の一部（De Casteljau で切り出した曲線）に乗るとき、辺は細かい直線に割れず、重なる所の端で
    /// 切れるだけ（レビュー 7f63b04 の N-1003。直す前は、境目1本が 260〜330 本の直線に割れた）
    #[test]
    fn shape_on_part_of_an_edge_cuts_it_only_at_the_ends() {
        use crate::vector_engine::Contour;
        let line = |a: Point, b: Point| QuadBez::new(a, a.midpoint(b), b);
        for (t0, t1) in [(0.2, 0.7), (0.0, 0.45), (0.31, 1.0)] {
            let arc = QuadBez::new(
                Point::new(100.0, 300.0),
                Point::new(250.0, 100.0),
                Point::new(400.0, 300.0),
            );
            let (a, b, c, d) = (
                arc.p2,
                Point::new(400.0, 450.0),
                Point::new(100.0, 450.0),
                arc.p0,
            );
            let face = FillShape::new(
                Color32::RED,
                Contour::new(vec![arc, line(a, b), line(b, c), line(c, d)]),
                vec![],
            );
            let base = PlanarMap::from_faces(std::iter::once(&face));
            let sub = arc.subsegment(t0..t1);
            let (e, f) = (Point::new(sub.p2.x, 420.0), Point::new(sub.p0.x, 420.0));
            for reversed in [false, true] {
                let mut segs = vec![sub, line(sub.p2, e), line(e, f), line(f, sub.p0)];
                if reversed {
                    segs = segs
                        .iter()
                        .rev()
                        .map(|q| QuadBez::new(q.p2, q.p1, q.p0))
                        .collect();
                }
                let inner = FillShape::new(Color32::BLUE, Contour::new(segs), vec![]);
                let m = base
                    .overlay(
                        MapInput::Shapes(std::slice::from_ref(&inner)),
                        MapRule::Paint(Some(Color32::BLUE)),
                    )
                    .unwrap();
                assert!(valid(&m), "{t0}..{t1}: {}", m.validity_report());
                // 弧は重なる所の端で切れ（端が弧の端なら切れない）、青い形の残りの3本が足される
                let cuts = [t0, t1].iter().filter(|&&t| t > 0.0 && t < 1.0).count();
                assert_eq!(
                    m.edges.len(),
                    base.edges.len() + cuts + 3,
                    "{t0}..{t1} 逆向き {reversed}"
                );
                let (faces, _) = m.to_faces_checked(1, &[]);
                assert_eq!(faces.len(), 2, "{t0}..{t1} 逆向き {reversed}");
            }
        }
    }

    /// 面をほんの少しずらして置く（ずらす量が地図の許容のあたり）。取り消されず、色も合う
    #[test]
    fn move_face_by_tiny_offsets() {
        let base = native_scene(2, 24);
        let (faces, _) = base.to_faces_checked(1, &[]);
        let mut biggest: Vec<usize> = (0..faces.len()).collect();
        biggest.sort_by_key(|&k| {
            std::cmp::Reverse(faces[k].contours().map(|c| c.segments.len()).sum::<usize>())
        });
        for &k in biggest.iter().take(3) {
            for d in [0.0, 0.01, 0.049, 0.051, 0.07, 1.0] {
                let piece = PlanarMap::from_faces(std::iter::once(&faces[k]));
                let rest = base
                    .overlay(MapInput::Map(&piece), MapRule::Paint(None))
                    .unwrap();
                let moved = piece.transformed(kurbo::Affine::translate((d, d * 0.5)));
                let m = rest
                    .overlay(MapInput::Map(&moved), MapRule::Overlay)
                    .unwrap();
                assert!(valid(&m), "面{k} ずらす量 {d}: {}", m.validity_report());
                let mut expected = rest.to_faces_checked(1, &[]).0;
                expected.extend(moved.to_faces_checked(1, &[]).0);
                let (out, _) = m.to_faces_checked(1, &[]);
                assert_eq!(color_diff(&expected, &out, 3.0), 0, "面{k} ずらす量 {d}");
            }
        }
    }

    /// 地図から書き出した図形を `from_faces` で読み直すと、同じように壊れていない地図になる
    /// （重ねると 0.05px より近い頂点ができることがあるので、許容はほぼ 0 にしてある）
    #[test]
    fn faces_read_back_stays_valid() {
        let m = native_scene(13, 36);
        assert!(valid(&m));
        let (faces, broken) = m.to_faces_checked(1, &[]);
        assert_eq!(broken, 0);
        let back = PlanarMap::from_faces(faces.iter());
        assert!(valid(&back));
        assert_eq!(back.edges.len(), m.edges.len());
    }

    /// 2本の辺を1本にまとめると制御点が両端の外に出る（曲線が端を越えて折り返す）ときも、まとめてよいかは判定の形で
    /// 決める（折り返しが、ほかの辺に触れず、頂点のまわりの順番も変えなければ、まとめてよい）。まとめた後も正しい地図で、
    /// 塗りは変わらない
    #[test]
    fn merge_degree2_folding_merge_keeps_map_valid() {
        use crate::planar_map::MapEdge;
        // A→B は、制御点が両端の外にある（端を越えて右へ出て、折り返して戻る）1本の曲線。それを V で切った2本（V は
        // 両側が同じ色の、2本だけがつながる頂点）
        let whole = QuadBez::new(
            Point::new(0.0, 0.0),
            Point::new(20.0, 10.0),
            Point::new(0.0, 10.0),
        );
        let (q1, q2) = (whole.subsegment(0.0..0.6), whole.subsegment(0.6..1.0));
        let (a, v, b, c) = (whole.p0, q1.p2, whole.p2, Point::new(-10.0, 5.0));
        let edge = |x: usize, y: usize, ctrl: Point| MapEdge {
            a: x,
            b: y,
            ctrl,
            left: Some(Color32::RED),
            right: None,
            left_src: None,
            right_src: None,
            stroke: None,
            line_id: None,
        };
        let mut m = PlanarMap {
            nodes: vec![a, v, b, c],
            edges: vec![
                edge(0, 1, q1.p1),
                edge(1, 2, q2.p1),
                edge(2, 3, b.midpoint(c)),
                edge(3, 0, c.midpoint(a)),
            ],
        };
        assert!(valid(&m));
        let before = m.clone();
        let n = m.nodes.len();
        m.merge_degree2(0..n, None);
        assert_eq!(
            m.edges.len(),
            3,
            "折り返しはほかの辺に触れないので、まとめる"
        );
        assert!(valid(&m));
        // まとめた曲線は元の曲線（塗りは変わらない）
        for (x, y) in [
            (5.0, 5.0),
            (-5.0, 5.0),
            (9.0, 5.0),
            (12.0, 5.0),
            (-11.0, 5.0),
            (5.0, -1.0),
            (5.0, 11.0),
        ] {
            let p = Point::new(x, y);
            assert_eq!(m.color_at(p), before.color_at(p), "{p:?}");
        }
    }

    /// 2本の辺を1本にまとめるのは、1本の2次ベジェを切ったものであることを式（de Casteljau）で確かめたときだけ：
    /// 同じ直線の上の2本の直線もまとまり、まとめた曲線は元の2本から `FLAT_TOL` 以内。1本の曲線を切ったものから頂点が
    /// `FLAT_TOL` より離れていれば、見た目が近くてもまとめない（レビュー 7f63b04 の N-1005。直す前は、直線どうしは
    /// まとまらず、曲線は 0.01px 以内に見えればまとめていた）
    #[test]
    fn merge_degree2_merges_only_a_split_quadratic() {
        use crate::planar_map::MapEdge;
        let square = |q1: QuadBez, q2: QuadBez| {
            let c = Point::new(0.0, 80.0);
            let edge = |x: usize, y: usize, ctrl: Point| MapEdge {
                a: x,
                b: y,
                ctrl,
                left: Some(Color32::RED),
                right: None,
                left_src: None,
                right_src: None,
                stroke: None,
                line_id: None,
            };
            PlanarMap {
                nodes: vec![q1.p0, q1.p2, q2.p2, c],
                edges: vec![
                    edge(0, 1, q1.p1),
                    edge(1, 2, q2.p1),
                    edge(2, 3, q2.p2.midpoint(c)),
                    edge(3, 0, c.midpoint(q1.p0)),
                ],
            }
        };
        let merged = |m: &PlanarMap| {
            let mut m = m.clone();
            let n = m.nodes.len();
            m.merge_degree2(0..n, None);
            assert!(valid(&m), "{}", m.validity_report());
            m
        };
        // 同じ直線の上の2本の直線（制御点は真ん中）は、1本の直線になる
        let (p, v, r) = (
            Point::new(0.0, 0.0),
            Point::new(30.0, 0.0),
            Point::new(100.0, 0.0),
        );
        let lines = merged(&square(
            QuadBez::new(p, p.midpoint(v), v),
            QuadBez::new(v, v.midpoint(r), r),
        ));
        assert_eq!(lines.edges.len(), 3, "直線どうしがまとまらない");
        assert!(lines
            .edges
            .iter()
            .any(|e| (e.ctrl - p.midpoint(r)).hypot() < 1e-9));
        // 曲線を切った2本は元の曲線に戻る。頂点を曲線から離すと、`FLAT_TOL` 以内ならまとまって元の2本から
        // `FLAT_TOL` 以内、それより離れればまとまらない
        let whole = QuadBez::new(
            Point::new(0.0, 0.0),
            Point::new(60.0, -70.0),
            Point::new(120.0, 0.0),
        );
        for (off, merges) in [(0.0, true), (0.0015, true), (0.003, false), (0.008, false)] {
            let (mut q1, mut q2) = (whole.subsegment(0.0..0.4), whole.subsegment(0.4..1.0));
            let n = (q2.p1 - q1.p1).normalize();
            let w = q1.p2 + kurbo::Vec2::new(-n.y, n.x) * off;
            q1.p2 = w;
            q2.p0 = w;
            let m = merged(&square(q1, q2));
            assert_eq!(m.edges.len() == 3, merges, "ずらし {off}");
            if !merges {
                continue;
            }
            let arc = m.quad(
                (0..m.edges.len())
                    .find(|&e| [m.edges[e].a, m.edges[e].b] == [0, 2])
                    .expect("まとめた曲線"),
            );
            for q in [q1, q2] {
                for k in 0..=64 {
                    let d = crate::vector_engine::nearest_on_quad(&arc, q.eval(k as f64 / 64.0)).1;
                    assert!(d <= 0.002 + 1e-9, "ずらし {off}：元の2本から {d} 離れた");
                }
            }
            if off == 0.0 {
                assert!((arc.p1 - whole.p1).hypot() < 1e-9, "元の曲線に戻らない");
            }
        }
    }

    /// 頂点の重なり（レビュー 08c15dd の N-2004）：頂点を別の面の頂点の座標ちょうどへ動かして確定すると、2つの頂点は1つになり、
    /// 地図は正しい（書き戻しの座標の一致に頼らない）
    #[test]
    fn move_vertex_onto_other_vertex_merges_them() {
        let red = rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED);
        let blue = rect_shape(250.0, 100.0, 350.0, 200.0, Color32::BLUE);
        let m = PlanarMap::from_faces([&red, &blue]);
        assert!(valid(&m));
        let v = (0..m.nodes.len())
            .find(|&v| m.nodes[v] == Point::new(200.0, 100.0))
            .expect("赤の右上の頂点");
        let w = Point::new(250.0, 100.0);
        let next = reshaped(
            &labeled_base(&m),
            ReshapeEdit::MoveVertex {
                orig_vertex: m.nodes[v],
                target: w,
            },
        )
        .expect("確定できる")
        .expect("変わる");
        assert!(valid(&next));
        assert_eq!(
            next.nodes.iter().filter(|&&p| p == w).count(),
            1,
            "頂点は1つ"
        );
        assert_eq!(
            next.edges_at(next.nodes.iter().position(|&p| p == w).unwrap())
                .len(),
            4
        );
        // 赤は (100,100)・(250,100)・(200,200)・(100,200) の四角形になる
        assert_eq!(next.color_at(Point::new(230.0, 110.0)), Some(Color32::RED));
        assert_eq!(next.color_at(Point::new(230.0, 190.0)), None);
        assert_eq!(next.color_at(Point::new(300.0, 150.0)), Some(Color32::BLUE));
    }

    /// カーソル合わせの頂点（`hit_node`）：置いた座標で `radius` 以内のいちばん近い頂点
    #[test]
    fn hit_node_finds_the_nearest_vertex_in_placed_coordinates() {
        let red = rect_shape(100.0, 100.0, 200.0, 200.0, Color32::RED);
        let m = crate::planar_map::LayerBase::new(&PlanarMap::from_faces([&red]));
        let id = Affine::IDENTITY;
        assert_eq!(m.hit_node(Point::new(201.0, 101.0), 3.0, id), Some(Point::new(200.0, 100.0)));
        assert_eq!(m.hit_node(Point::new(205.0, 100.0), 3.0, id), None);
        // 2倍で置くと、距離は置いた座標で測る
        let x2 = Affine::scale(2.0);
        assert_eq!(m.hit_node(Point::new(402.0, 202.0), 3.0, x2), Some(Point::new(200.0, 100.0)));
        assert_eq!(m.hit_node(Point::new(201.0, 101.0), 3.0, x2), None);
    }

    /// ほぼ重なった2本の辺（間がごく細い面）の上に重ねても壊れない。間が格子の目（1/4096px）より細いと、2本の
    /// 判定の形が重なるので、正しい地図ではない（重ね合わせの結果には、そういう面はできない）
    #[test]
    fn overlay_across_hairline_lens() {
        // 真ん中に、幅 `width` の黄色の細い面（同じ2頂点を結ぶ、ほぼ同じ2本の曲線）を持つ赤の矩形
        let lens = |width: f64| {
            let mut m = PlanarMap::from_faces(std::iter::once(&rect_shape(
                100.0,
                100.0,
                300.0,
                300.0,
                Color32::RED,
            )));
            let (a, b) = (m.nodes.len(), m.nodes.len() + 1);
            m.nodes.push(Point::new(150.0, 150.0));
            m.nodes.push(Point::new(250.0, 200.0));
            let yellow = Color32::YELLOW;
            // 2本目（制御点が 2 × width 下。曲線の間は真ん中で width）が左にあるので、間（1本目の左・2本目の右）が黄色
            m.edges.push(crate::planar_map::MapEdge {
                a,
                b,
                ctrl: Point::new(200.0, 160.0),
                left: Some(yellow),
                right: Some(Color32::RED),
                left_src: None,
                right_src: None,
                stroke: None,
                line_id: None,
            });
            m.edges.push(crate::planar_map::MapEdge {
                a,
                b,
                ctrl: Point::new(200.0, 160.0 + 2.0 * width),
                left: Some(Color32::RED),
                right: Some(yellow),
                left_src: None,
                right_src: None,
                stroke: None,
                line_id: None,
            });
            m
        };
        assert!(
            !valid(&lens(0.00006)),
            "格子の目より細い面は正しい地図ではない"
        );
        // 細い面（真ん中の幅 0.025px。両端の頂点のすぐ近くの、折れ線の最初の点でも、格子の目の数倍離れる）
        let m = lens(0.025);
        assert!(valid(&m));
        let stroke = rect_shape(140.0, 140.0, 260.0, 175.0, Color32::BLUE);
        let out = m.overlay(
            MapInput::Shapes(std::slice::from_ref(&stroke)),
            MapRule::Paint(Some(Color32::BLUE)),
        );
        let out = out.expect("細い面にかかっても重ねられる");
        assert!(valid(&out));
    }

    /// レイヤー `layer` の生の図形から作った地図が壊れていないか
    fn layer_map_valid(app: &Is2dApp, layer: usize) -> bool {
        let shapes: Vec<FillShape> = app.doc.root.layers[layer].keyframes[0]
            .cel
            .shapes()
            .iter()
            .filter(|s| s.is_raw_at_scope(&[]))
            .map(|s| (**s).clone())
            .collect();
        PlanarMap::from_faces(shapes.iter()).is_valid()
    }

    /// 前の版の結合で描いたような、境目が両側の図形でそろっていない図形：正しい地図の図形の、1つおきに、輪郭の曲線の
    /// 制御点を少しずらす（頂点は同じで、となりの図形と曲線だけが食い違う）
    fn misaligned(shapes: Vec<FillShape>) -> Vec<FillShape> {
        shapes
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                if i % 2 == 1 {
                    return s;
                }
                let bend = |c: &crate::vector_engine::Contour| {
                    crate::vector_engine::Contour::new(
                        c.segments
                            .iter()
                            .map(|q| QuadBez::new(q.p0, q.p1 + KVec2::new(1e-4, 1e-4), q.p2))
                            .collect(),
                    )
                };
                let mut out = FillShape::new(
                    s.color(),
                    bend(s.exterior()),
                    s.holes().iter().map(bend).collect(),
                );
                out.shape_id = s.shape_id;
                out.with_scope(s.layer_id, &s.group_path)
            })
            .collect()
    }

    /// 点 `p` が、図形 `shapes` の輪郭から `r` 以内にあるか（輪郭のすぐそばの点は、地図に置き直すと側が変わりうる）
    fn near_boundary(shapes: &[FillShape], p: Point, r: f64) -> bool {
        shapes.iter().flat_map(|s| s.contours()).any(|c| {
            c.segments.iter().any(|q| {
                kurbo::ParamCurveExtrema::bounding_box(q)
                    .inflate(r, r)
                    .contains(p)
                    && kurbo::ParamCurveNearest::nearest(q, p, 1e-9).distance_sq < r * r
            })
        })
    }

    /// ブラシ・消しゴムを、少しずつずらしながら重ねても、生の図形はいつも地図として壊れていない
    /// （境目は両側の図形でビットまで同じ曲線になるので、図形から地図を作り直しても許容が要らない）
    #[test]
    fn brush_and_eraser_keep_layer_map_valid() {
        let mut app = Is2dApp::default();
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN];
        let mut r = Rng(5);
        for k in 0..24 {
            app.tool.tool = if k % 5 == 4 {
                Tool::Eraser
            } else {
                Tool::Brush
            };
            app.tool.brush_color = colors[k % 3];
            let y = 150.0 + (k % 6) as f64 * 0.051;
            let a = Point::new(100.0 + r.f() * 50.0, y + r.f() * 80.0);
            let b = Point::new(300.0 + r.f() * 50.0, y + r.f() * 80.0);
            let raw = brush_line(&[a, a.midpoint(b), b], 8.0 + r.f() * 6.0);
            commit_brush_line(&mut app, &raw);
            assert!(layer_map_valid(&app, 0), "{k} 回目");
        }
    }

    /// 何も無い所にブラシで描いた図形は、同じレイヤーのグループより下に入る
    #[test]
    fn brush_into_empty_area_goes_below_groups() {
        let mut app = Is2dApp::default();
        app.tool.brush_color = Color32::RED;
        commit_brush_line(
            &mut app,
            &brush_line(&[Point::new(100.0, 100.0), Point::new(200.0, 100.0)], 10.0),
        );
        app.set_selected_indices(vec![0]);
        let gid = doc_actions::group_selected_shapes(&mut app).unwrap();
        assert!(canvas_tool::clear_selection(&mut app));
        app.tool.brush_color = Color32::BLUE;
        commit_brush_line(
            &mut app,
            &brush_line(&[Point::new(100.0, 300.0), Point::new(200.0, 300.0)], 10.0),
        );
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
        let raw = shapes.iter().position(|s| s.group_path.is_empty()).unwrap();
        let grouped = shapes
            .iter()
            .position(|s| s.group_path == vec![gid])
            .unwrap();
        assert!(
            raw < grouped,
            "生の図形（{raw}）がグループ（{grouped}）より下"
        );
    }

    /// バケツは、同じレイヤーの生の図形の線で囲まれた所を塗る。グループの図形は壁にしない（Animate と同じ）
    #[test]
    fn bucket_uses_only_raw_lines_of_the_layer_as_walls() {
        let mut app = Is2dApp::default();
        app.tool.brush_color = Color32::BLACK;
        let center = Point::new(200.0, 200.0);
        let ring: Vec<Point> = (0..=48)
            .map(|i| {
                let a = (i as f64 / 48.0) * std::f64::consts::TAU;
                Point::new(center.x + 60.0 * a.cos(), center.y + 60.0 * a.sin())
            })
            .collect();
        commit_brush_line(&mut app, &brush_line(&ring, 6.0));
        // 円環の中を横切る線を、同じレイヤーのグループとして置く
        let mut shapes: Vec<FillShape> = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| (**s).clone())
            .collect::<Vec<FillShape>>();
        for mut g in make_stroke(
            Point::new(200.0, 120.0),
            Point::new(200.0, 280.0),
            6.0,
            Color32::GREEN,
        ) {
            g.group_path = vec![77];
            shapes.push(g);
        }
        put_cel_shapes(&mut app, 0, 0, shapes);
        app.tool.brush_color = Color32::RED;
        assert!(doc_actions::bucket_fill_at(
            &mut app,
            Point::new(170.0, 200.0)
        ));
        let shapes = app.doc.root.layers[0].keyframes[0].cel.shapes();
        let red: Vec<&FillShape> = shapes
            .iter()
            .filter(|s| s.color() == Color32::RED)
            .map(|s| &**s)
            .collect();
        assert_eq!(red.len(), 1);
        assert!(
            red[0].contains_point(Point::new(230.0, 200.0)),
            "グループの線は壁にならない"
        );
        assert!(
            !red[0].contains_point(Point::new(100.0, 200.0)),
            "円環の外は塗らない"
        );
        assert!(layer_map_valid(&app, 0));
    }

    /// 3色がとなり合う絵（ブラシで描いた3本の太い線が重なる）を作る
    fn three_band_app() -> Is2dApp {
        let mut app = Is2dApp::default();
        for (y, c) in [
            (180.0, Color32::RED),
            (200.0, Color32::BLUE),
            (220.0, Color32::GREEN),
        ] {
            app.tool.brush_color = c;
            commit_brush_line(
                &mut app,
                &brush_line(&[Point::new(100.0, y), Point::new(300.0, y + 3.0)], 14.0),
            );
        }
        app
    }

    /// 画面の点の色（いちばん上の図形の色）
    fn color_at(app: &Is2dApp, p: Point) -> Option<Color32> {
        app.scene()
            .iter()
            .rev()
            .find(|s| s.contains_point(p))
            .map(|s| s.color())
    }

    /// 3色にまたがる矩形で選び、ほんの少しずらす・回す・拡大して離しても、レイヤーの地図は壊れず、片はちゃんと動く
    #[test]
    fn marquee_move_rotate_scale_keeps_layer_map_valid() {
        let probes = [
            Point::new(160.0, 182.0),
            Point::new(160.0, 201.0),
            Point::new(160.0, 221.0),
        ];
        for (k, action) in [
            Op::Do(doc_actions::Action::NudgeSelection {
                delta: KVec2::new(0.051, 0.0),
            }),
            Op::Do(doc_actions::Action::NudgeSelection {
                delta: KVec2::new(0.0, 0.01),
            }),
            Op::Do(doc_actions::Action::NudgeSelection {
                delta: KVec2::new(30.0, 0.0),
            }),
            Op::Do(doc_actions::Action::RotateSelection { angle_rad: 0.003 }),
            Op::Scale(1.02, 1.02),
        ]
        .into_iter()
        .enumerate()
        {
            let mut app = three_band_app();
            let before: Vec<Option<Color32>> = probes.iter().map(|p| color_at(&app, *p)).collect();
            select_ops::apply_marquee_selection(
                &mut app,
                Point::new(140.0, 150.0),
                Point::new(200.0, 250.0),
                false,
            );
            assert!(!app.session.selection().is_empty(), "{k}");
            assert!(op(&mut app, action));
            assert!(canvas_tool::clear_selection(&mut app));
            assert!(layer_map_valid(&app, 0), "{k}");
            if k < 2 {
                // ごくわずかに動かしただけなので、色はそのまま
                let after: Vec<Option<Color32>> =
                    probes.iter().map(|p| color_at(&app, *p)).collect();
                assert_eq!(before, after, "{k}");
            }
            if k == 2 {
                // 30px 右へ動いた（矩形の左の所は空き、右へ 30px の所に片がある）
                assert_eq!(color_at(&app, Point::new(150.0, 201.0)), None);
                assert_eq!(
                    color_at(&app, Point::new(180.0, 201.0)),
                    Some(Color32::BLUE)
                );
            }
        }
    }

    /// 矩形で選んで、動かさずに選択を外すと、元の図形がそのまま戻る
    #[test]
    fn marquee_without_moving_restores_original_shapes() {
        let mut app = three_band_app();
        let before: Vec<(u64, Color32)> = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| (s.shape_id, s.color()))
            .collect();
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(140.0, 150.0),
            Point::new(200.0, 250.0),
            false,
        );
        assert!(canvas_tool::clear_selection(&mut app));
        let after: Vec<(u64, Color32)> = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| (s.shape_id, s.color()))
            .collect();
        assert_eq!(before, after);
    }

    /// 貼り付けた片を、今の図形に重なる所へ置いても、レイヤーの地図は壊れない
    #[test]
    fn paste_over_existing_shapes_keeps_layer_map_valid() {
        let mut app = three_band_app();
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(140.0, 150.0),
            Point::new(200.0, 250.0),
            false,
        );
        assert!(doc_actions::copy_selected_shapes(&mut app));
        assert!(canvas_tool::clear_selection(&mut app));
        assert!(doc_actions::paste_clipboard(&mut app, true));
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(37.0, 9.0)
            }
        ));
        assert!(canvas_tool::clear_selection(&mut app));
        assert!(layer_map_valid(&app, 0));
        // 貼り付けた片の上の色（元の 160,201 の青が 197,210 に来る）
        assert_eq!(
            color_at(&app, Point::new(197.0, 210.0)),
            Some(Color32::BLUE)
        );
    }

    /// 強さの試し（時間がかかるので、ふだんは流さない）：地図だけで描いた絵の面を、ずらして置く。
    /// 環境変数 PROBE_SEEDS（例 "1,2,3"）・PROBE_FACES（面の数、ふだん 60）
    #[test]
    #[ignore]
    fn probe_move_faces() {
        let offsets = [0.0, 0.01, 0.03, 0.049, 0.051, 0.07, 0.2, 1.0, 10.0, 30.0];
        let seeds: Vec<u64> = std::env::var("PROBE_SEEDS")
            .map(|v| v.split(',').map(|x| x.parse().unwrap()).collect())
            .unwrap_or(vec![1, 2, 3]);
        let nfaces: usize = std::env::var("PROBE_FACES")
            .map(|v| v.parse().unwrap())
            .unwrap_or(60);
        let (mut total, mut refused, mut broken, mut diffs, mut t_sum, mut t_max) =
            (0, 0, 0, 0, 0f64, 0f64);
        for seed in seeds {
            let base = native_scene(seed, 48);
            let (faces, _) = base.to_faces_checked(1, &[]);
            for (k, face) in faces.iter().enumerate().take(nfaces) {
                for (oi, &o) in offsets.iter().enumerate() {
                    let d = if oi % 2 == 0 { (o, 0.0) } else { (0.0, o) };
                    let piece = PlanarMap::from_faces(std::iter::once(face));
                    let t = std::time::Instant::now();
                    let rest = base.overlay(MapInput::Map(&piece), MapRule::Paint(None));
                    let moved = piece.transformed(kurbo::Affine::translate(d));
                    let m = rest
                        .as_ref()
                        .and_then(|r| r.overlay(MapInput::Map(&moved), MapRule::Overlay));
                    let el = t.elapsed().as_secs_f64() * 1e3;
                    total += 1;
                    t_sum += el;
                    t_max = t_max.max(el);
                    let (Some(rest), Some(m)) = (rest, m) else {
                        refused += 1;
                        continue;
                    };
                    if !valid(&m) {
                        broken += 1;
                    }
                    if k % 6 == 0 {
                        let mut expected = rest.to_faces_checked(1, &[]).0;
                        expected.extend(moved.to_faces_checked(1, &[]).0);
                        if color_diff(&expected, &m.to_faces_checked(1, &[]).0, 1.0) > 0 {
                            diffs += 1;
                        }
                    }
                }
            }
        }
        eprintln!("PROBE 面をずらして置く {total} 回（持ち上げと置くの2回の重ね）｜取り消し {refused}｜壊れた地図 {broken}｜色の違う回 {diffs}｜1回 平均 {:.1}ms・最大 {t_max:.1}ms", t_sum / total as f64);
    }

    /// 回して縦横別に拡大したインスタンスからシンボルの中に入って編集しても、文書（シンボルの座標）の図形は、いつもその座標で
    /// 正しい地図の面のまま（地図は文書の座標で作り、書き戻しで逆変換しない。レビュー 6cca852 の N-2001。直す前は、同じ
    /// 操作で 300 回中 15 回、正しい地図でなくなった）。別の変換のインスタンスから入って、どの図形にもかからない所に描いても、
    /// ほかの図形は番号も形もそのまま
    #[test]
    fn symbol_edit_in_rotated_scaled_instance_keeps_local_map_valid() {
        if let Err(e) = symbol_random_ops(2, 220) {
            panic!("{e}");
        }
    }

    /// 強さの試し（ふだんは流さない）：`symbol_edit_in_rotated_scaled_instance_keeps_local_map_valid` の操作を、種と回数を
    /// 変えて続ける。環境変数 PROBE_SEEDS・PROBE_STEPS（ふだん 300）
    #[test]
    #[ignore]
    fn probe_symbol_random_ops() {
        let seeds: Vec<u64> = std::env::var("PROBE_SEEDS")
            .map(|v| v.split(',').map(|x| x.parse().unwrap()).collect())
            .unwrap_or((1..=10).collect());
        let steps: usize = std::env::var("PROBE_STEPS")
            .map(|v| v.parse().unwrap())
            .unwrap_or(300);
        for seed in seeds {
            eprintln!("PROBE 種{seed}：{:?}", symbol_random_ops(seed, steps));
        }
    }

    /// 1つのインスタンスを回して縦横別に拡大し、中に入って、種 `seed` の乱数で描画の操作を `steps` 回続ける。毎回、文書の
    /// 図形がその座標で正しい地図か確かめる。最後に、別の変換のインスタンスから入って、どの図形にもかからない所に描き、
    /// ほかの図形が番号も形もそのままか確かめる
    fn symbol_random_ops(seed: u64, steps: usize) -> Result<(), String> {
        use crate::canvas_tool::doc_actions::Action;
        let mut app = Is2dApp::default();
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let stroke = |app: &mut Is2dApp, pts: &[Point], radius: f64| {
            let mut dense = Vec::new();
            for w in pts.windows(2) {
                for j in 0..10 {
                    dense.push(w[0].lerp(w[1], j as f64 / 10.0));
                }
            }
            dense.push(*pts.last().unwrap());
            commit_brush_line(app, &brush_line(&dense, radius));
        };
        stroke(
            &mut app,
            &[Point::new(200.0, 300.0), Point::new(400.0, 300.0)],
            30.0,
        );
        app.set_selected_indices(vec![0]);
        assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
        let place_instance = |app: &mut Is2dApp, angle: f64, sx: f64, sy: f64| {
            let c = KVec2::new(300.0, 300.0);
            let xf = Affine::translate(c)
                * Affine::rotate(angle)
                * Affine::scale_non_uniform(sx, sy)
                * Affine::translate(-c);
            let b = &mut app.doc.root.layers[0].keyframe_mut(0).cel.instances_mut()[0].binding;
            b.xform = xf * b.xform;
            app.doc.revision = crate::serial::next_mesh_revision();
            app.refresh_scene_base();
        };
        let enter = |app: &mut Is2dApp| {
            app.set_selected_indices(vec![0]);
            let (binding, _, _) = symbol_ops::selected_single_symbol_info(app).unwrap();
            assert!(symbol_ops::enter_symbol_timeline(app, binding.instance_gid));
        };
        let local = |app: &Is2dApp| -> Vec<FillShape> {
            app.active_timeline().layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .map(|s| (**s).clone())
                .collect()
        };
        place_instance(&mut app, 0.4, 1.3, 0.8);
        enter(&mut app);
        let mut r = Rng(seed);
        let rp = |r: &mut Rng| Point::new(180.0 + r.f() * 240.0, 200.0 + r.f() * 200.0);
        for step in 0..steps {
            let kind = (r.f() * 10.0) as usize % 10;
            match kind {
                0 | 1 => {
                    app.tool.tool = if kind == 0 { Tool::Brush } else { Tool::Eraser };
                    let pts: Vec<Point> = (0..3).map(|_| rp(&mut r)).collect();
                    stroke(&mut app, &pts, 4.0 + r.f() * 10.0);
                    app.tool.tool = Tool::Brush;
                }
                2 => {
                    app.tool.brush_color = colors[(r.f() * 4.0) as usize % 4];
                    let _ = doc_actions::bucket_fill_at(&mut app, rp(&mut r));
                }
                3..=5 => {
                    let a = rp(&mut r);
                    let b = a + KVec2::new(20.0 + r.f() * 80.0, 20.0 + r.f() * 80.0);
                    select_ops::apply_marquee_selection(&mut app, a, b, false);
                    if !app.session.selection().is_empty() {
                        let action = match kind {
                            3 => Op::Do(Action::NudgeSelection {
                                delta: KVec2::new(r.f() * 30.0 - 15.0, r.f() * 30.0 - 15.0),
                            }),
                            4 => Op::Do(Action::RotateSelection {
                                angle_rad: (r.f() - 0.5) * 0.6,
                            }),
                            _ => Op::Scale(0.7 + r.f() * 0.6, 0.7 + r.f() * 0.6),
                        };
                        let _ = op(&mut app, action);
                    }
                    assert!(canvas_tool::clear_selection(&mut app));
                }
                6 | 7 => {
                    let from = rp(&mut r);
                    let to = from + KVec2::new(r.f() * 30.0 - 15.0, r.f() * 30.0 - 15.0);
                    let action = if kind == 6 { Op::PullEdge(from, to) } else { Op::MoveVertex(from, to) };
                    let _ = op(&mut app, action);
                }
                8 => {
                    let _ = doc_actions::apply(&mut app, Action::AddAnchor { pt: rp(&mut r) });
                }
                _ => {
                    let _ = doc_actions::apply(&mut app, Action::DeleteAnchor { pt: rp(&mut r) });
                }
            }
            let shapes = local(&app);
            let m = PlanarMap::from_faces(shapes.iter());
            if !m.is_valid() {
                return Err(format!(
                    "{step} 回目（種類 {kind}）のあと、シンボルの中の図形が正しい地図でない: {}",
                    m.validity_report()
                ));
            }
        }
        // 別の変換のインスタンスから入り、どの図形にもかからない所（ステージの左上の隅）に小さく描く
        let before: Vec<(u64, u64)> = local(&app)
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        assert!(symbol_ops::exit_symbol_timeline(&mut app));
        place_instance(&mut app, -1.1, 0.6, 1.4);
        enter(&mut app);
        let corner = app.session.edit_path().world_xform() * Point::new(-2000.0, -2000.0);
        stroke(&mut app, &[corner, corner + KVec2::new(10.0, 0.0)], 3.0);
        let after: Vec<(u64, u64)> = local(&app)
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        if after.len() != before.len() + 1 || !before.iter().all(|b| after.contains(b)) {
            return Err(format!(
                "別のインスタンスから入って描いたら、触っていない図形が書き直された（{} → {} 個）",
                before.len(),
                after.len()
            ));
        }
        Ok(())
    }

    /// 強さの試し（ふだんは流さない）：アプリの操作（ブラシ・消しゴム・バケツ・矩形で選んで動かす・回す・貼り付け・ドラッグ）を
    /// ランダムに続け、毎回レイヤーの地図が壊れていないか（地図で行えずに今までのやり方へ回ると、境目がずれて壊れる）を見る。
    /// 環境変数 PROBE_SEEDS・PROBE_STEPS（ふだん 200）
    #[test]
    #[ignore]
    fn probe_random_app_ops() {
        let seeds: Vec<u64> = std::env::var("PROBE_SEEDS")
            .map(|v| v.split(',').map(|x| x.parse().unwrap()).collect())
            .unwrap_or(vec![11, 12, 13, 14]);
        let steps: usize = std::env::var("PROBE_STEPS")
            .map(|v| v.parse().unwrap())
            .unwrap_or(200);
        let colors = [
            Color32::RED,
            Color32::BLUE,
            Color32::GREEN,
            Color32::BLACK,
            Color32::YELLOW,
        ];
        let names = [
            "ブラシ",
            "消しゴム",
            "バケツ",
            "矩形で動かす",
            "矩形で回す",
            "貼り付け",
            "ドラッグ",
        ];
        for seed in seeds {
            let mut app = Is2dApp::default();
            let mut r = Rng(seed);
            let mut counts = [0usize; 7];
            let mut t_max = [0f64; 7];
            let mut first_bad: Option<String> = None;
            for step in 0..steps {
                let kind = (r.f() * 7.0) as usize % 7;
                let rp = |r: &mut Rng| Point::new(120.0 + r.f() * 360.0, 120.0 + r.f() * 360.0);
                let t = std::time::Instant::now();
                match kind {
                    0 | 1 => {
                        app.tool.tool = if kind == 0 { Tool::Brush } else { Tool::Eraser };
                        app.tool.brush_color = colors[(r.f() * 5.0) as usize % 5];
                        let n = 2 + (r.f() * 4.0) as usize;
                        let pts: Vec<Point> = (0..n).map(|_| rp(&mut r)).collect();
                        let mut dense = Vec::new();
                        for w in pts.windows(2) {
                            for j in 0..10 {
                                dense.push(w[0].lerp(w[1], j as f64 / 10.0));
                            }
                        }
                        dense.push(*pts.last().unwrap());
                        commit_brush_line(&mut app, &brush_line(&dense, 3.0 + r.f() * 12.0));
                        app.tool.tool = Tool::Brush;
                    }
                    2 => {
                        app.tool.brush_color = colors[(r.f() * 5.0) as usize % 5];
                        let _ = doc_actions::bucket_fill_at(&mut app, rp(&mut r));
                    }
                    3 | 4 => {
                        let a = rp(&mut r);
                        let b = a + KVec2::new(20.0 + r.f() * 120.0, 20.0 + r.f() * 120.0);
                        select_ops::apply_marquee_selection(&mut app, a, b, false);
                        if !app.session.selection().is_empty() {
                            let action = if kind == 3 {
                                let o =
                                    [0.0, 0.01, 0.051, 0.2, 1.0, 15.0][(r.f() * 6.0) as usize % 6];
                                doc_actions::Action::NudgeSelection {
                                    delta: KVec2::new(o, o * 0.5),
                                }
                            } else {
                                doc_actions::Action::RotateSelection {
                                    angle_rad: (r.f() - 0.5) * 0.4,
                                }
                            };
                            let _ = doc_actions::apply(&mut app, action);
                        }
                        assert!(canvas_tool::clear_selection(&mut app));
                    }
                    5 => {
                        let a = rp(&mut r);
                        select_ops::apply_marquee_selection(
                            &mut app,
                            a,
                            a + KVec2::new(80.0, 80.0),
                            false,
                        );
                        if doc_actions::copy_selected_shapes(&mut app) {
                            assert!(canvas_tool::clear_selection(&mut app));
                            if doc_actions::paste_clipboard(&mut app, true) {
                                let _ = doc_actions::apply(
                                    &mut app,
                                    doc_actions::Action::NudgeSelection {
                                        delta: KVec2::new(r.f() * 40.0 - 20.0, r.f() * 40.0 - 20.0),
                                    },
                                );
                            }
                        }
                        assert!(canvas_tool::clear_selection(&mut app));
                    }
                    _ => {
                        let a = rp(&mut r);
                        let to = a + KVec2::new(r.f() * 30.0 - 15.0, r.f() * 30.0 - 15.0);
                        let _ = pull_edge(&mut app, a, to);
                    }
                }
                let el = t.elapsed().as_secs_f64() * 1e3;
                counts[kind] += 1;
                t_max[kind] = t_max[kind].max(el);
                if first_bad.is_none() && !layer_map_valid(&app, 0) {
                    first_bad = Some(format!("{step} 回目（{}）", names[kind]));
                }
            }
            let ts: Vec<String> = (0..7)
                .map(|k| format!("{} {} 回・最大 {:.0}ms", names[k], counts[k], t_max[k]))
                .collect();
            eprintln!(
                "PROBE 種{seed}：{}｜地図が最初に壊れた所 {:?}｜図形 {}",
                ts.join("、"),
                first_bad,
                app.doc.root.layers[0].keyframes[0].cel.shapes().len()
            );
        }
    }

    /// レビュー（e0769c0）と同じ密度の絵：3レイヤー × ランダムなブラシ 60 本（4色・半径 2〜10・ステージ 800×450 全体）
    pub(super) fn dense_scene_app(seed: u64) -> Is2dApp {
        let mut app = Is2dApp::default();
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let mut r = Rng(seed);
        for layer in 0..3 {
            if layer > 0 {
                doc_actions::timeline_add_layer(&mut app);
            }
            for _ in 0..60 {
                app.tool.brush_color = colors[(r.f() * 4.0) as usize % 4];
                let n = 2 + (r.f() * 3.0) as usize;
                let pts: Vec<Point> = (0..n)
                    .map(|_| Point::new(-390.0 + r.f() * 780.0, -215.0 + r.f() * 430.0))
                    .collect();
                let mut dense = Vec::new();
                for w in pts.windows(2) {
                    for j in 0..12 {
                        dense.push(w[0].lerp(w[1], j as f64 / 12.0));
                    }
                }
                dense.push(*pts.last().unwrap());
                commit_brush_line(&mut app, &brush_line(&dense, 2.0 + r.f() * 8.0));
            }
        }
        app
    }

    /// 何かを選んでいても、色の違う2つの図形の境目にアンカーを足す・消すと、両側の図形が一緒に変わる（地図で編集する）。
    /// 浮いている片（動かした図形）のアンカーは、その片だけが変わる
    #[test]
    fn anchor_edit_on_shared_border_changes_both_sides_even_with_selection() {
        let mut app = Is2dApp::default();
        let shapes = vec![
            rect_shape(0.0, 0.0, 100.0, 100.0, Color32::RED),
            rect_shape(100.0, 0.0, 200.0, 100.0, Color32::BLUE),
            rect_shape(300.0, 0.0, 400.0, 100.0, Color32::GREEN),
        ];
        put_cel_shapes(&mut app, 0, 0, shapes);
        let border_count = |app: &Is2dApp| {
            // 境目 x = 100 の上にある頂点の数（赤と青それぞれ）
            let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
            [Color32::RED, Color32::BLUE].map(|c| {
                shapes
                    .iter()
                    .filter(|s| s.color() == c)
                    .flat_map(|s| {
                        s.contours()
                            .flat_map(|k| k.segments.iter().map(|q| q.p0))
                            .collect::<Vec<_>>()
                    })
                    .filter(|p| (p.x - 100.0).abs() < 1e-6)
                    .count()
            })
        };
        let before = border_count(&app);
        // 緑を選んでおく
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectSingleAt {
                pt: Point::new(350.0, 50.0)
            }
        ));
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::AddAnchor {
                pt: Point::new(100.0, 50.0)
            }
        ));
        let added = border_count(&app);
        assert_eq!(
            added,
            [before[0] + 1, before[1] + 1],
            "赤と青の両方にアンカーが足される"
        );
        assert!(layer_map_valid(&app, 0));
        // また緑を選んで、足したアンカーを消す
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectSingleAt {
                pt: Point::new(350.0, 50.0)
            }
        ));
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::DeleteAnchor {
                pt: Point::new(100.0, 50.0)
            }
        ));
        assert_eq!(border_count(&app), before, "赤と青の両方から消える");
        assert!(layer_map_valid(&app, 0));
        // 緑を持ち上げて動かす（浮いている片）：その辺にアンカーを足すと、片だけが変わる
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectSingleAt {
                pt: Point::new(350.0, 50.0)
            }
        ));
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(0.0, 150.0)
            }
        ));
        let piece_segs = |app: &Is2dApp| {
            app.session.floating().map(|_| {
                app.session.floating_view().pieces()
                    .iter()
                    .map(|p| p.exterior().segments.len())
                    .sum::<usize>()
            })
        };
        let n0 = piece_segs(&app).expect("緑は浮いている");
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::AddAnchor {
                pt: Point::new(350.0, 150.0)
            }
        ));
        assert_eq!(
            piece_segs(&app),
            Some(n0 + 1),
            "浮いている片だけが変わり、確定はしない"
        );
    }

    /// 重なり合う片（別のレイヤーの絵を1枚のレイヤーへまとめて貼ったときなど）は1つの地図にならないので、
    /// 同じ地図へ片を下から1つずつ塗る。結果のレイヤーは地図として壊れず、上の片が上に来て、下の絵は残る
    #[test]
    fn overlapping_pieces_are_painted_one_by_one_on_the_map() {
        use crate::serial::next_shape_id;
        // 線 60 本の絵と、それを (13, 7) ずらした写しを、1枚のレイヤーの片にする（片どうしが重なる）
        let app = review_repeat_scene(5);
        let drawn: Vec<FillShape> = app.doc.root.layers[0].keyframes[0]
            .cel
            .shapes()
            .iter()
            .map(|s| (**s).clone())
            .collect();
        let lid = drawn[0].layer_id;
        let mut pieces = drawn.clone();
        pieces.extend(drawn.iter().map(|s| {
            let mut c = s.clone();
            c.shape_id = next_shape_id();
            c.translate(KVec2::new(13.0, 7.0));
            c
        }));
        let mut shapes =
            vec![rect_shape(-300.0, -150.0, 300.0, 150.0, Color32::RED).with_scope(lid, &[])];
        let top = pieces.last().unwrap().clone();
        // 片に覆われていない赤の点
        let red_at = (0..2000)
            .map(|i| {
                Point::new(
                    -295.0 + (i % 50) as f64 * 11.9,
                    -145.0 + (i / 50) as f64 * 7.3,
                )
            })
            .find(|p| !pieces.iter().any(|s| s.contains_point(*p)))
            .expect("覆われていない点");
        let t = std::time::Instant::now();
        merge_shapes_into_canvas(&mut shapes, pieces);
        eprintln!(
            "PROBE 重なり合う片の確定 {:.1}ms",
            t.elapsed().as_secs_f64() * 1e3
        );
        let m = PlanarMap::from_faces(shapes.iter());
        assert!(m.is_valid());
        // 片に覆われていない所は、下の赤のまま
        assert_eq!(
            shapes
                .iter()
                .filter(|s| s.contains_point(red_at))
                .map(|s| s.color())
                .collect::<Vec<_>>(),
            vec![Color32::RED]
        );
        // いちばん上の片の中の点は、その片の色
        let p = (0..64)
            .map(|i| {
                top.bounding_box().center() + KVec2::new((i % 8) as f64 - 4.0, (i / 8) as f64 - 4.0)
            })
            .find(|p| top.contains_point(*p))
            .expect("片の中の点");
        let found: Vec<Color32> = shapes
            .iter()
            .filter(|s| s.contains_point(p))
            .map(|s| s.color())
            .collect();
        assert_eq!(found, vec![top.color()], "後の片が上");
    }

    /// 同じレイヤーに重なった生の図形がある（前の版のファイルなど）セルで、離れた所にブラシを描いても、ほかの所の絵が
    /// 変わらない。重なった図形は、頂点を共有しないと輪郭は閉じて色もそろうので、交わりまで見て（`is_valid`）見つける（N-1004）
    #[test]
    fn overlapping_raw_shapes_are_repainted_before_edit() {
        let top_color = |shapes: &[FillShape], p: Point| {
            shapes
                .iter()
                .rev()
                .find(|s| s.contains_point(p))
                .map(|s| s.color())
        };
        for seed in 0..11u64 {
            let mut r = Rng(seed + 100);
            let mut app = Is2dApp::default();
            let lid = app.doc.root.layers[0].id;
            let rnd = |r: &mut Rng, x0: f64, x1: f64| x0 + r.f() * (x1 - x0);
            let red = make_stroke(
                Point::new(rnd(&mut r, -200.0, -120.0), rnd(&mut r, -60.0, 60.0)),
                Point::new(rnd(&mut r, 120.0, 200.0), rnd(&mut r, -60.0, 60.0)),
                (8.0 + r.f() * 12.0) as f32,
                Color32::RED,
            );
            let blue = make_stroke(
                Point::new(rnd(&mut r, -60.0, 60.0), rnd(&mut r, -150.0, -100.0)),
                Point::new(rnd(&mut r, -60.0, 60.0), rnd(&mut r, 100.0, 150.0)),
                (8.0 + r.f() * 12.0) as f32,
                Color32::BLUE,
            );
            let shapes: Vec<FillShape> = red
                .into_iter()
                .chain(blue)
                .map(|s| s.with_scope(lid, &[]))
                .collect();
            put_cel_shapes(&mut app, 0, 0, shapes.clone());
            assert!(
                !PlanarMap::from_faces(shapes.iter()).is_valid(),
                "seed {seed}: 重なった図形が正しい地図になっている"
            );
            app.tool.brush_color = Color32::GREEN;
            let green: Vec<Point> = (0..=10)
                .map(|i| Point::new(-200.0 + 40.0 * i as f64, 170.0 + 4.0 * (i % 2) as f64))
                .collect();
            commit_brush_line(&mut app, &brush_line(&green, 6.0));
            let after: Vec<FillShape> = app.scene().iter().cloned().collect();
            let mut changed = Vec::new();
            for i in 0..100 {
                for j in 0..80 {
                    let p = Point::new(
                        -200.0 + 4.0 * i as f64 + 0.31,
                        -160.0 + 4.0 * j as f64 + 0.27,
                    );
                    let now = top_color(&after, p);
                    if now != Some(Color32::GREEN)
                        && now != top_color(&shapes, p)
                        && !near_boundary(&shapes, p, 0.25)
                    {
                        changed.push(p);
                    }
                }
            }
            assert!(
                changed.is_empty(),
                "seed {seed}: 色が変わった点 {}",
                changed.len()
            );
            assert!(layer_map_valid(&app, 0), "seed {seed}: 地図が壊れた");
        }
    }

    /// 前の版の結合（境目がビットまで同じにならない）で描いた図形のセル（地図が壊れている）でも、ブラシ・消しゴム・バケツ・
    /// 矩形で選んで動かす、が正しく効き、ほかの所の絵が消えない（レイヤーを塗り重ねて作り直してから編集する）
    #[test]
    fn broken_layer_edits_repaint_layer_and_keep_other_paint() {
        let broken_app = || {
            let mut app = Is2dApp::default();
            let lid = app.doc.root.layers[0].id;
            let mut r = Rng(3);
            let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
            let mut shapes: Vec<FillShape> = Vec::new();
            for _ in 0..30 {
                let p0 = Point::new(-300.0 + r.f() * 600.0, -180.0 + r.f() * 360.0);
                let p1 = Point::new(-300.0 + r.f() * 600.0, -180.0 + r.f() * 360.0);
                let color = colors[(r.f() * 4.0) as usize % 4];
                let stroke = make_stroke(p0, p1, (2.0 + r.f() * 8.0) as f32, color);
                merge_shapes_into_canvas(
                    &mut shapes,
                    stroke.into_iter().map(|s| s.with_scope(lid, &[])).collect(),
                );
            }
            let input = misaligned(shapes);
            assert!(
                !PlanarMap::from_faces(input.iter()).is_valid(),
                "境目がそろっていない図形は、地図として壊れている"
            );
            // セル画に入れると、並びの順に塗り重ねて作り直した地図の面になる
            put_cel_shapes(&mut app, 0, 0, input);
            assert!(layer_map_valid(&app, 0));
            app
        };
        let cel = |app: &Is2dApp| {
            app.doc.root.layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .map(|s| (**s).clone())
                .collect::<Vec<FillShape>>()
        };
        let colors_at = |shapes: &[FillShape], p: Point| {
            shapes
                .iter()
                .filter(|s| s.contains_point(p))
                .map(|s| s.color())
                .collect::<Vec<_>>()
        };
        // 編集した所（`touched` の矩形）から離れた格子の点の色が、前と同じ
        let same_elsewhere =
            |before: &[FillShape], after: &[FillShape], touched: &[kurbo::Rect]| {
                let mut n = 0;
                for i in 0..60 {
                    for j in 0..36 {
                        let p = Point::new(
                            -300.0 + 10.0 * i as f64 + 0.37,
                            -180.0 + 10.0 * j as f64 + 0.41,
                        );
                        if touched.iter().any(|r| r.inflate(3.0, 3.0).contains(p)) {
                            continue;
                        }
                        // 塗り重ねて作り直した所の、上の図形の色（前は、ずれて重なった所に色が2つあることがある）
                        let (b, a) = (colors_at(before, p), colors_at(after, p));
                        assert!(
                            b.last() == a.last() || near_boundary(before, p, 0.25),
                            "編集していない所 {p:?} の色が変わった：{b:?} → {a:?}"
                        );
                        n += 1;
                    }
                }
                assert!(n > 500);
            };
        let cyan = Color32::from_rgb(10, 200, 200);
        let line: Vec<Point> = (0..=12)
            .map(|i| Point::new(-200.0 + 30.0 * i as f64, 10.0))
            .collect();
        let line_box = kurbo::Rect::new(-200.0, 4.0, 160.0, 16.0);
        // ブラシ
        let mut app = broken_app();
        let before = cel(&app);
        app.tool.brush_color = cyan;
        commit_brush_line(&mut app, &brush_line(&line, 6.0));
        let after = cel(&app);
        assert_eq!(colors_at(&after, Point::new(-50.0, 10.0)), vec![cyan]);
        same_elsewhere(&before, &after, &[line_box]);
        assert!(layer_map_valid(&app, 0), "塗り重ねて作り直した地図は正しい");
        // 消しゴム
        let mut app = broken_app();
        let before = cel(&app);
        app.tool.tool = Tool::Eraser;
        commit_brush_line(&mut app, &brush_line(&line, 6.0));
        let after = cel(&app);
        assert!(colors_at(&after, Point::new(-50.0, 10.0)).is_empty());
        same_elsewhere(&before, &after, &[line_box]);
        assert!(layer_map_valid(&app, 0));
        // 矩形で選んで動かし、選択を外す：矩形の外（と、動かした先の外）の絵は消えない
        let mut app = broken_app();
        let before = cel(&app);
        let (a, b) = (Point::new(-100.0, -50.0), Point::new(100.0, 100.0));
        select_ops::apply_marquee_selection(&mut app, a, b, false);
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(10.0, 0.0)
            }
        ));
        assert!(canvas_tool::clear_selection(&mut app));
        let after = cel(&app);
        same_elsewhere(
            &before,
            &after,
            &[
                kurbo::Rect::from_points(a, b),
                kurbo::Rect::from_points(a, b + KVec2::new(10.0, 0.0)),
            ],
        );
        // バケツ：何かの色の面の中の点を塗る
        let mut app = broken_app();
        let shapes = cel(&app);
        let p = (0..400)
            .map(|i| {
                Point::new(
                    -200.0 + (i % 20) as f64 * 20.0,
                    -150.0 + (i / 20) as f64 * 15.0,
                )
            })
            .find(|p| colors_at(&shapes, *p).len() == 1)
            .expect("塗られた点");
        app.tool.brush_color = cyan;
        assert!(doc_actions::bucket_fill_at(&mut app, p));
        assert_eq!(colors_at(&cel(&app), p), vec![cyan]);
        assert!(layer_map_valid(&app, 0));
    }

    /// 境目が両側の図形でそろっていない（前の版の結合で描いたような）レイヤーを、空の地図へ並びの順に塗り重ねて
    /// 作り直しても、地図は壊れず、色も変わらない。重ねる地図の中にある、ほぼ同じ形で向きが逆の2本の辺が、格子の上で
    /// 打ち消し合って消えない。2本の辺の間の細い面が格子の上で裏返っても、面の色が食い違わない
    #[test]
    fn painted_map_of_misaligned_layer_is_valid_and_keeps_colors() {
        let colors_at = |shapes: &[FillShape], p: Point| {
            shapes
                .iter()
                .filter(|s| s.contains_point(p))
                .map(|s| s.color())
                .collect::<Vec<_>>()
        };
        for seed in [3u64, 5, 11] {
            let mut r = Rng(seed);
            let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
            let mut shapes: Vec<FillShape> = Vec::new();
            for _ in 0..30 {
                let p0 = Point::new(-300.0 + r.f() * 600.0, -180.0 + r.f() * 360.0);
                let p1 = Point::new(-300.0 + r.f() * 600.0, -180.0 + r.f() * 360.0);
                let color = colors[(r.f() * 4.0) as usize % 4];
                let stroke = make_stroke(p0, p1, (2.0 + r.f() * 8.0) as f32, color);
                merge_shapes_into_canvas(
                    &mut shapes,
                    stroke.into_iter().map(|s| s.with_scope(1, &[])).collect(),
                );
            }
            let shapes = misaligned(shapes);
            assert!(
                !PlanarMap::from_faces(shapes.iter()).is_valid(),
                "seed {seed}: 作った図形が、地図として壊れていない"
            );
            let pm = crate::planar_map::painted_map_pub(&shapes).expect("塗り重ねた地図");
            assert!(pm.is_valid(), "seed {seed}: 塗り重ねた地図が壊れた");
            let faces: Vec<FillShape> = pm
                .faces()
                .0
                .iter()
                .map(|f| pm.face_shape(f, 1, &[]))
                .collect();
            let mut lost = Vec::new();
            for i in 0..60 {
                for j in 0..36 {
                    let p = Point::new(
                        -300.0 + 10.0 * i as f64 + 0.37,
                        -180.0 + 10.0 * j as f64 + 0.41,
                    );
                    if colors_at(&shapes, p).last() != colors_at(&faces, p).last()
                        && !near_boundary(&shapes, p, 0.25)
                    {
                        lost.push(p);
                    }
                }
            }
            assert!(lost.is_empty(), "seed {seed}: 色が変わった点 {lost:?}");
        }
    }

    /// 地図が壊れたセル（境目がそろっていない）で、矩形で選んで動かして戻し、選択を外しても、絵が消えない。
    /// 壊れた地図は、矩形で選ぶときに塗り重ねて作り直す（輪郭から 0.25px 以内の点は、置き直すと側が変わりうるので見ない）
    #[test]
    fn marquee_on_broken_cell_loses_no_paint() {
        let colors_at = |shapes: &[FillShape], p: Point| {
            shapes
                .iter()
                .filter(|s| s.contains_point(p))
                .map(|s| s.color())
                .collect::<Vec<_>>()
        };
        for seed in [3u64, 7, 8] {
            let mut app = Is2dApp::default();
            let lid = app.doc.root.layers[0].id;
            let mut r = Rng(seed);
            let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
            let mut shapes: Vec<FillShape> = Vec::new();
            for _ in 0..60 {
                let p0 = Point::new(-390.0 + r.f() * 780.0, -215.0 + r.f() * 430.0);
                let (len, ang) = (50.0 + r.f() * 250.0, r.f() * std::f64::consts::TAU);
                let p1 = p0 + KVec2::new(ang.cos(), ang.sin()) * len;
                let color = colors[(r.f() * 4.0) as usize % 4];
                let stroke = make_stroke(p0, p1, (1.0 + r.f() * 7.0) as f32, color);
                merge_shapes_into_canvas(
                    &mut shapes,
                    stroke.into_iter().map(|s| s.with_scope(lid, &[])).collect(),
                );
            }
            let input = misaligned(shapes);
            assert!(!PlanarMap::from_faces(input.iter()).is_valid());
            put_cel_shapes(&mut app, 0, 0, input);
            assert!(layer_map_valid(&app, 0));
            let before = app.doc.root.layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .map(|s| (**s).clone())
                .collect::<Vec<FillShape>>();
            let (a, b) = (Point::new(100.0, 50.0), Point::new(300.0, 200.0));
            select_ops::apply_marquee_selection(&mut app, a, b, false);
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::NudgeSelection {
                    delta: KVec2::new(10.0, 0.0)
                }
            ));
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::NudgeSelection {
                    delta: KVec2::new(-10.0, 0.0)
                }
            ));
            assert!(canvas_tool::clear_selection(&mut app));
            let after = app.doc.root.layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .map(|s| (**s).clone())
                .collect::<Vec<FillShape>>();
            let (mut lost_in, mut lost_out) = (0, 0);
            for i in 0..390 {
                for j in 0..215 {
                    let p = Point::new(
                        -390.0 + 2.0 * i as f64 + 0.31,
                        -215.0 + 2.0 * j as f64 + 0.27,
                    );
                    if !colors_at(&before, p).is_empty()
                        && colors_at(&after, p).is_empty()
                        && !near_boundary(&before, p, 0.25)
                    {
                        if kurbo::Rect::from_points(a, b).inflate(2.0, 2.0).contains(p) {
                            lost_in += 1
                        } else {
                            lost_out += 1
                        }
                    }
                }
            }
            assert_eq!(
                (lost_in, lost_out),
                (0, 0),
                "種 {seed}：消えた点（矩形の中, 外）"
            );
            assert!(
                layer_map_valid(&app, 0),
                "種 {seed}：塗り重ねて作り直した地図は正しい"
            );
        }
    }

    /// 地図で調べた点の色（`color_at`）が、地図を作った図形で調べた色と同じ
    #[test]
    fn color_at_matches_source_shapes() {
        let app = dense_scene_app(3);
        let mut r = Rng(11);
        let mut checked = 0;
        for layer in 0..3 {
            let shapes: Vec<FillShape> = app.doc.root.layers[layer].keyframes[0]
                .cel
                .shapes()
                .iter()
                .filter(|s| s.is_raw_at_scope(&[]))
                .map(|s| (**s).clone())
                .collect();
            let m = PlanarMap::from_faces(shapes.iter());
            for _ in 0..300 {
                let p = Point::new(-420.0 + r.f() * 840.0, -245.0 + r.f() * 490.0);
                // 辺にごく近い点は、どちらの答えも当てにならないので除く
                let near = (0..m.edges.len()).any(|e| {
                    let q = QuadBez::new(
                        m.nodes[m.edges[e].a],
                        m.edges[e].ctrl,
                        m.nodes[m.edges[e].b],
                    );
                    kurbo::ParamCurveExtrema::bounding_box(&q)
                        .inflate(0.01, 0.01)
                        .contains(p)
                        && nearest_on_quad(&q, p).1 < 0.01
                });
                if near {
                    continue;
                }
                let from_shapes = shapes
                    .iter()
                    .rev()
                    .find(|s| s.contains_point(p))
                    .map(|s| s.color());
                assert_eq!(m.color_at(p), from_shapes, "レイヤー {layer} の点 {p:?}");
                checked += 1;
            }
        }
        assert!(checked > 800);
    }

    /// 速さの試し（ふだんは流さない）：今の版で線を多く描いたセルでの、バケツ（塗り替え）の時間
    #[test]
    #[ignore]
    fn probe_bucket_on_dense_cell() {
        let mut app = Is2dApp::default();
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let mut r = Rng(21);
        let mut drawn = 0;
        for target in [300usize, 1000] {
            while drawn < target {
                app.tool.brush_color = colors[(r.f() * 4.0) as usize % 4];
                let p0 = Point::new(-390.0 + r.f() * 780.0, -215.0 + r.f() * 430.0);
                let (len, ang) = (50.0 + r.f() * 250.0, r.f() * std::f64::consts::TAU);
                let bend = KVec2::new(-ang.sin(), ang.cos()) * (r.f() - 0.5) * 80.0;
                let pts: Vec<Point> = (0..=24)
                    .map(|j| {
                        let t = j as f64 / 24.0;
                        p0 + KVec2::new(ang.cos(), ang.sin()) * (len * t)
                            + bend * (4.0 * t * (1.0 - t))
                    })
                    .collect();
                commit_brush_line(&mut app, &brush_line(&pts, 2.0 + r.f() * 6.0));
                drawn += 1;
            }
            let mut times = Vec::new();
            for k in 0..30 {
                app.tool.brush_color = colors[k % 4];
                let p = Point::new(-380.0 + r.f() * 760.0, -210.0 + r.f() * 420.0);
                let t = std::time::Instant::now();
                doc_actions::bucket_fill_at(&mut app, p);
                times.push(t.elapsed().as_secs_f64() * 1e3);
            }
            let avg = times.iter().sum::<f64>() / times.len() as f64;
            let max = times.iter().cloned().fold(0.0, f64::max);
            let n = app.doc.root.layers[0].keyframes[0].cel.shapes().len();
            eprintln!("PROBE 線 {target} 本（図形 {n}）：バケツ 30 回 平均 {avg:.1}ms・最大 {max:.1}ms｜地図 {}", if layer_map_valid(&app, 0) { "正常" } else { "壊れた" });
        }
    }

    /// レビュー（a116357 N-1003）：線を1本描いても、セルの変わらない図形は Undo の控えと共有したまま（控えの大きさが
    /// セルの図形の数でなく、変えた図形の分になる）
    #[test]
    fn undo_snapshot_shares_unchanged_shapes() {
        let mut app = review_repeat_scene(5);
        let before: Vec<std::sync::Arc<FillShape>> =
            app.doc.root.layers[0].keyframes[0].cel.shapes();
        app.tool.brush_color = Color32::RED;
        let pts: Vec<Point> = (0..=8)
            .map(|i| Point::new(-390.0 + 2.0 * i as f64, -220.0))
            .collect();
        commit_brush_line(&mut app, &brush_line(&pts, 1.5));
        let after = &app.doc.root.layers[0].keyframes[0].cel.shapes();
        let shared = after
            .iter()
            .filter(|a| before.iter().any(|b| std::sync::Arc::ptr_eq(a, b)))
            .count();
        assert!(before.len() > 40);
        assert!(
            shared + 3 >= before.len(),
            "共有 {shared} / 前 {}・後 {}",
            before.len(),
            after.len()
        );
    }

    /// レビュー（e0769c0 第4回）の手順：60 本の線（4色・太さ 1〜8・長さ 50〜300、800×450 全体）を描いたセルで、
    /// 矩形選択 → (7, 3) 動かす → 選択を外す、を矩形を変えながら 60 回繰り返す（120 角 ×20、400 角を左右から縮めて ×20、
    /// 800 角 ×20）。毎回、地図が壊れていないこと
    fn review_repeat_scene(seed: u64) -> Is2dApp {
        let mut app = Is2dApp::default();
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let mut r = Rng(seed);
        for _ in 0..60 {
            app.tool.brush_color = colors[(r.f() * 4.0) as usize % 4];
            let p0 = Point::new(-400.0 + r.f() * 800.0, -225.0 + r.f() * 450.0);
            let (len, ang) = (50.0 + r.f() * 250.0, r.f() * std::f64::consts::TAU);
            let p1 = p0 + KVec2::new(ang.cos(), ang.sin()) * len;
            let pts: Vec<Point> = (0..=16).map(|j| p0.lerp(p1, j as f64 / 16.0)).collect();
            commit_brush_line(&mut app, &brush_line(&pts, 0.5 + r.f() * 3.5));
        }
        app
    }

    fn review_repeat_rects() -> Vec<(Point, Point)> {
        let mut rects = Vec::new();
        for i in 0..20 {
            let (x, y) = (-400.0 + 36.0 * i as f64, -225.0 + 16.0 * i as f64);
            rects.push((Point::new(x, y), Point::new(x + 120.0, y + 120.0)));
        }
        for i in 0..20 {
            let d = 9.0 * i as f64;
            rects.push((Point::new(-200.0 + d, -200.0), Point::new(200.0 - d, 200.0)));
        }
        for i in 0..20 {
            let x = -420.0 + 5.0 * i as f64;
            rects.push((Point::new(x, -400.0), Point::new(x + 800.0, 400.0)));
        }
        rects
    }

    #[test]
    fn repeated_marquee_move_deselect_keeps_layer_map_valid() {
        let mut app = review_repeat_scene(5);
        for (step, (a, b)) in review_repeat_rects().into_iter().enumerate() {
            select_ops::apply_marquee_selection(&mut app, a, b, false);
            doc_actions::apply(
                &mut app,
                doc_actions::Action::NudgeSelection {
                    delta: KVec2::new(7.0, 3.0),
                },
            );
            assert!(canvas_tool::clear_selection(&mut app));
            assert!(layer_map_valid(&app, 0), "{} 回目で地図が壊れた", step + 1);
        }
    }

    /// 強さの試し（ふだんは流さない）：上の繰り返しを、種を変えて何通りも行う
    #[test]
    #[ignore]
    fn probe_repeated_marquee_many_seeds() {
        let n: u64 = std::env::var("PROBE_SEEDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let mut worst = 0.0f64;
        for seed in 1..=n {
            let mut app = review_repeat_scene(seed);
            for (step, (a, b)) in review_repeat_rects().into_iter().enumerate() {
                let t = std::time::Instant::now();
                select_ops::apply_marquee_selection(&mut app, a, b, false);
                doc_actions::apply(
                    &mut app,
                    doc_actions::Action::NudgeSelection {
                        delta: KVec2::new(7.0, 3.0),
                    },
                );
                assert!(canvas_tool::clear_selection(&mut app));
                worst = worst.max(t.elapsed().as_secs_f64() * 1e3);
                if !layer_map_valid(&app, 0) {
                    eprintln!("PROBE 種 {seed}：{} 回目で地図が壊れた", step + 1);
                    break;
                }
            }
        }
        eprintln!("PROBE 1回（選択＋移動＋確定）の最大 {worst:.1}ms");
    }

    /// 速さの試し（ふだんは流さない）：前の版の結合で描いたセル（地図が壊れている）での、矩形選択・確定・ブラシの時間と、
    /// 区間の数
    /// ほぼ重なった線：線 S を赤、S を ε ずらした線を青、もう一度 S を赤で描く（ε は 1e-4〜1e-2 の13通り × 線 `lines` 本）。
    /// （正しくない地図の数, S の中で赤でない所が出た数, 試した数）
    pub(super) fn near_coincident_retrace(lines: usize) -> (usize, usize, usize) {
        let mut r = Rng(2024);
        let (mut invalid, mut lost, mut total) = (0, 0, 0);
        for _ in 0..lines {
            let p0 = Point::new(-150.0 + r.f() * 60.0, -60.0 + r.f() * 120.0);
            let bend = Point::new(r.f() * 160.0 - 80.0, r.f() * 160.0 - 80.0);
            let p1 = Point::new(90.0 + r.f() * 60.0, -60.0 + r.f() * 120.0);
            let pts: Vec<Point> = (0..=16)
                .map(|i| {
                    let t = i as f64 / 16.0;
                    let a = p0.lerp(bend, t);
                    a.lerp(bend.lerp(p1, t), t)
                })
                .collect();
            let radius = 2.0 + r.f() * 6.0;
            let raw = brush_line(&pts, radius);
            let s_shapes = brush_line_shapes_at_zoom(&raw, Color32::RED, 1.0);
            for k in 0..13 {
                let eps = 1e-4 * 10f64.powf(k as f64 / 6.0);
                let shifted = BrushLine {
                    pts: raw
                        .pts
                        .iter()
                        .map(|p| *p + KVec2::new(eps, eps * 0.7))
                        .collect(),
                    ..raw.clone()
                };
                let mut app = Is2dApp::default();
                let mut ok = true;
                for (outline, color) in [
                    (&raw, Color32::RED),
                    (&shifted, Color32::BLUE),
                    (&raw, Color32::RED),
                ] {
                    // 重ね合わせの結果そのものが正しい地図か（図形に書き戻す前）
                    let before: Vec<FillShape> = app.scene().iter().cloned().collect();
                    let stroke = brush_line_shapes_at_zoom(outline, color, 1.0);
                    let base = PlanarMap::from_faces(before.iter());
                    if let Some(m) = base.overlay(
                        crate::planar_map::MapInput::Shapes(&stroke),
                        crate::planar_map::MapRule::Paint(Some(color)),
                    ) {
                        ok &= m.is_valid();
                    }
                    app.tool.brush_color = color;
                    commit_brush_line(&mut app, outline);
                    ok &= layer_map_valid(&app, 0);
                }
                total += 1;
                if !ok {
                    invalid += 1;
                }
                let scene: Vec<FillShape> = app.scene().iter().cloned().collect();
                let bbox = s_shapes
                    .iter()
                    .map(|s| s.bounding_box())
                    .reduce(|a, b| a.union(b))
                    .unwrap();
                let mut bad = false;
                'grid: for i in 0..120 {
                    for j in 0..120 {
                        let p = Point::new(
                            bbox.x0 + bbox.width() * (i as f64 + 0.5) / 120.0,
                            bbox.y0 + bbox.height() * (j as f64 + 0.5) / 120.0,
                        );
                        if !s_shapes.iter().any(|s| s.contains_point(p))
                            || near_boundary(&s_shapes, p, 0.05)
                        {
                            continue;
                        }
                        if scene
                            .iter()
                            .rev()
                            .find(|s| s.contains_point(p))
                            .map(|s| s.color())
                            != Some(Color32::RED)
                        {
                            bad = true;
                            break 'grid;
                        }
                    }
                }
                if bad {
                    lost += 1;
                }
            }
        }
        (invalid, lost, total)
    }

    /// 線 S を赤、ε（1e-4〜1e-2）ずらした S を青、もう一度 S を赤で描いても、重ね合わせの結果とレイヤーの地図は
    /// いつも正しく、S の中は赤（判定の形と重ね合わせの形が同じ。レビュー N-1001）
    #[test]
    fn near_coincident_retrace_keeps_map_valid_and_paint() {
        let (invalid, lost, total) = near_coincident_retrace(3);
        assert_eq!(
            (invalid, lost),
            (0, 0),
            "正しくない地図・赤でない所（{total}通り中）"
        );
    }

    /// 回転・拡大縮小したシンボルの中で、重なる線を何本も描いても、レイヤーの地図（ステージの座標で作る）はいつも正しく、
    /// 塗り重ねて作り直すことにならない（図形はシンボルの座標で持つので、座標を行き来する丸めが入る。設計 段階2 の5章）
    #[test]
    fn drawing_inside_rotated_scaled_symbol_keeps_layer_map_valid() {
        use doc_actions::Action;
        let mut app = Is2dApp::default();
        let colors = [
            Color32::from_rgb(220, 60, 60),
            Color32::from_rgb(60, 120, 220),
            Color32::from_rgb(40, 180, 90),
        ];
        run(&mut app, &[Action::PaintStroke {
                    points: vec![Point::new(100.0, 100.0), Point::new(220.0, 115.0)],
                    radius: 14.0,
                    color: colors[0],
                }, Action::SelectSingleAt {
                    pt: Point::new(160.0, 107.0),
                }, Action::ConvertToSymbol]);
        scale_selection(&mut app, 1.73, 0.61);
        run(&mut app, &[Action::RotateSelection { angle_rad: 0.47 }]);
        let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
        run(
            &mut app,
            &[Action::EnterSymbolTimeline {
                instance_gid: binding.instance_gid,
            }],
        );
        // 編集中の階層の生の図形（ステージの座標）から作った地図
        let map_of = |app: &Is2dApp| {
            let scope = app.session.editing_group_path().to_vec();
            let shapes: Vec<FillShape> = app
                .scene()
                .iter()
                .filter(|s| s.is_raw_at_scope(&scope) && s.symbol_instance.is_none())
                .cloned()
                .collect();
            PlanarMap::from_faces(shapes.iter())
        };
        let mut r = Rng(7);
        let mut invalid = 0;
        for i in 0..40 {
            let a = Point::new(60.0 + r.f() * 220.0, 20.0 + r.f() * 220.0);
            let b = Point::new(60.0 + r.f() * 220.0, 20.0 + r.f() * 220.0);
            let bend = KVec2::new(r.f() * 60.0 - 30.0, r.f() * 60.0 - 30.0);
            run(
                &mut app,
                &[Action::PaintStroke {
                    points: vec![a, a.midpoint(b) + bend, b],
                    radius: (3.0 + r.f() * 6.0) as f32,
                    color: colors[i % 3],
                }],
            );
            if !map_of(&app).is_valid() {
                invalid += 1;
            }
        }
        let m = map_of(&app);
        assert!(m.edges.len() > 200, "線が重なって境目が多い");
        assert_eq!(invalid, 0, "塗り重ねて作り直すことになった回");
    }

    #[test]
    #[ignore]
    fn probe_near_coincident_retrace() {
        let (invalid, lost, total) = near_coincident_retrace(20);
        eprintln!("PROBE ほぼ重なった線：正しくない地図 {invalid}/{total}・S の中が赤でない {lost}/{total}");
    }

    /// 同じ点列から作ったブラシを色を替えて何度も重ねても、見えない欠片の図形が溜まらない（重ねる線と同じ線になっている
    /// 前からの辺は、同じ点の折れ線にする：`Span`。レビュー 6cca852 の N-1001。直す前は、12 回目に壊れた地図ができ、それまでに
    /// 図形が 24 個から 1,779 個に増えた）
    #[test]
    fn same_stroke_repeated_leaves_no_slivers() {
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
        let mut app = Is2dApp::default();
        let mut r = Rng(5);
        let line = |r: &mut Rng| -> Vec<Point> {
            let pts: Vec<Point> = (0..3)
                .map(|_| Point::new(-200.0 + r.f() * 400.0, -150.0 + r.f() * 300.0))
                .collect();
            let mut dense = Vec::new();
            for w in pts.windows(2) {
                for j in 0..12 {
                    dense.push(w[0].lerp(w[1], j as f64 / 12.0));
                }
            }
            dense.push(*pts.last().unwrap());
            dense
        };
        for k in 0..20 {
            app.tool.brush_color = colors[k % 4];
            let pts = line(&mut r);
            commit_brush_line(&mut app, &brush_line(&pts, 4.0 + r.f() * 8.0));
        }
        let same = brush_line(&line(&mut r), 9.0);
        let area = |s: &FillShape| {
            s.exterior().signed_area().abs()
                - s.holes().iter().map(|h| h.signed_area().abs()).sum::<f64>()
        };
        let tiny = |app: &Is2dApp| {
            app.doc.root.layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .filter(|s| area(s) < 0.01)
                .count()
        };
        let mut after_first = None;
        for k in 0..12 {
            app.tool.brush_color = colors[k % 4];
            let undo = app.history.undo_stack.len();
            commit_brush_line(&mut app, &same);
            assert_eq!(
                app.history.undo_stack.len(),
                undo + 1,
                "{k} 回目が何もしなかった"
            );
            assert!(layer_map_valid(&app, 0), "{k} 回目で地図が正しくない");
            let n = tiny(&app);
            assert!(
                n <= *after_first.get_or_insert(n),
                "{k} 回目で欠片が {n} 個に増えた"
            );
        }
    }

    /// 同じ点列から作ったブラシを、色を替えて何度も重ねたときの欠片（レビュー 6cca852 の N-1001）を数える：20 本描いた
    /// セルに 30 回重ね、図形の数・面積 0.01px² 未満の図形の数・1回の時間・断った回数。ずらし（`PROBE_SHIFT`、px）を
    /// 交互に足すこともできる
    #[test]
    #[ignore]
    fn probe_same_stroke_repeated() {
        let shift: f64 = std::env::var("PROBE_SHIFT")
            .map(|v| v.parse().unwrap())
            .unwrap_or(0.0);
        let colors = [
            Color32::RED,
            Color32::BLUE,
            Color32::GREEN,
            Color32::BLACK,
            Color32::YELLOW,
        ];
        let mut app = Is2dApp::default();
        let mut r = Rng(5);
        let line = |r: &mut Rng| -> Vec<Point> {
            let pts: Vec<Point> = (0..3)
                .map(|_| Point::new(-200.0 + r.f() * 400.0, -150.0 + r.f() * 300.0))
                .collect();
            let mut dense = Vec::new();
            for w in pts.windows(2) {
                for j in 0..12 {
                    dense.push(w[0].lerp(w[1], j as f64 / 12.0));
                }
            }
            dense.push(*pts.last().unwrap());
            dense
        };
        for k in 0..20 {
            app.tool.brush_color = colors[k % 5];
            let pts = line(&mut r);
            commit_brush_line(&mut app, &brush_line(&pts, 4.0 + r.f() * 8.0));
        }
        let same = line(&mut r);
        let area = |s: &FillShape| {
            s.exterior().signed_area().abs()
                - s.holes().iter().map(|h| h.signed_area().abs()).sum::<f64>()
        };
        let mut t_max = 0f64;
        let mut refused = 0;
        for k in 0..30 {
            app.tool.brush_color = colors[k % 5];
            let d = if k % 2 == 1 { shift } else { 0.0 };
            let pts: Vec<Point> = same.iter().map(|p| *p + KVec2::new(d, d)).collect();
            let undo = app.history.undo_stack.len();
            let t = std::time::Instant::now();
            commit_brush_line(&mut app, &brush_line(&pts, 9.0));
            let ms = t.elapsed().as_secs_f64() * 1e3;
            t_max = t_max.max(ms);
            if app.history.undo_stack.len() == undo {
                refused += 1;
            }
            eprintln!(
                "PROBE {k} 回目：図形 {}・{ms:.1}ms",
                app.doc.root.layers[0].keyframes[0].cel.shapes().len()
            );
        }
        let shapes = &app.doc.root.layers[0].keyframes[0].cel.shapes();
        let tiny = shapes.iter().filter(|s| area(s) < 0.01).count();
        let segs: usize = shapes
            .iter()
            .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
            .sum();
        eprintln!(
            "PROBE 同じ線を30回（ずらし {shift}px）：図形 {}（0.01px² 未満 {tiny}）・区間 {segs}・1回の最大 {t_max:.1}ms・断った回 {refused}・地図 {}",
            shapes.len(),
            if layer_map_valid(&app, 0) { "正しい" } else { "正しくない" }
        );
    }

    /// 同じ線を2色交互に手ぶれ付きでなぞったときの、地図の健全さを数える
    #[test]
    #[ignore]
    fn probe_review_two_color_retrace() {
        for seed in [1u64, 2, 3] {
            let mut app = Is2dApp::default();
            let mut r = Rng(seed);
            let mut bad = 0;
            let mut first_bad = None;
            for k in 0..100 {
                app.tool.brush_color = if k % 2 == 0 {
                    Color32::RED
                } else {
                    Color32::GREEN
                };
                let pts: Vec<Point> = (0..31)
                    .map(|i| {
                        let x = -250.0 + 500.0 * i as f64 / 30.0;
                        let y = 80.0 * (x / 500.0 * std::f64::consts::TAU).sin();
                        Point::new(x + r.f() - 0.5, y + r.f() - 0.5)
                    })
                    .collect();
                let w = 5.0 + r.f() * 2.0;
                commit_brush_line(&mut app, &brush_line(&pts, w));
                if !layer_map_valid(&app, 0) {
                    bad += 1;
                    first_bad.get_or_insert(k + 1);
                }
            }
            let segs: usize = app
                .scene()
                .iter()
                .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
                .sum();
            eprintln!("PROBE seed {seed}: 壊れた地図 {bad}/100（最初 {first_bad:?}）・図形 {}・区間 {segs}", app.scene().len());
        }
        // 手ぶれなし、太さを少しずつ変える
        let mut app = Is2dApp::default();
        let pts: Vec<Point> = (0..31)
            .map(|i| {
                let x = -250.0 + 500.0 * i as f64 / 30.0;
                Point::new(x, 80.0 * (x / 500.0 * std::f64::consts::TAU).sin())
            })
            .collect();
        let mut bad = 0;
        for k in 0..8 {
            app.tool.brush_color = if k % 2 == 0 {
                Color32::RED
            } else {
                Color32::GREEN
            };
            commit_brush_line(&mut app, &brush_line(&pts, 6.0 + 0.37 * k as f64));
            if !layer_map_valid(&app, 0) {
                bad += 1;
            }
        }
        eprintln!("PROBE 手ぶれなし・太さを変える 8 本：壊れた地図 {bad}/8");
    }

    /// 選び足し（レビュー 08c15dd の N-2003）：前の矩形とつながる所を Shift で選び足すと、つながった同じ色の片は1つになり、
    /// 動かして確定しても、レイヤーは正しい地図のまま（塗り重ねの作り直しに回らない）
    #[test]
    fn shift_marquee_merges_connected_pieces_and_keeps_layer_valid() {
        let mut app = Is2dApp::default();
        for (y, c) in [
            (-40.0, Color32::RED),
            (0.0, Color32::BLUE),
            (40.0, Color32::GREEN),
        ] {
            app.tool.brush_color = c;
            let pts: Vec<Point> = (0..=10)
                .map(|i| Point::new(-200.0 + 40.0 * i as f64, y))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 36.0));
        }
        let marquee = |app: &mut Is2dApp, p0: Point, p1: Point, additive: bool| {
            assert!(doc_actions::apply(
                app,
                doc_actions::Action::SelectMarquee { p0, p1, additive },
            ));
        };
        marquee(
            &mut app,
            Point::new(-260.0, -130.0),
            Point::new(0.0, 130.0),
            false,
        );
        marquee(
            &mut app,
            Point::new(-30.0, -100.0),
            Point::new(80.0, 20.0),
            true,
        );
        app.session.floating().expect("選択");
        let colors: Vec<Color32> = app.session.floating_view().pieces().iter().map(|p| p.color()).collect();
        for c in [Color32::RED, Color32::BLUE, Color32::GREEN] {
            assert_eq!(
                colors.iter().filter(|&&x| x == c).count(),
                1,
                "{c:?} の片は1つ"
            );
        }
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(0.0, 200.0)
            }
        ));
        assert!(canvas_tool::clear_selection(&mut app));
        assert!(layer_map_valid(&app, 0));
    }

    /// 動かして浮いている片で、境目の角を消す・辺に角を足すのをランダムにくり返し、片どうしが正しい地図でなくなった回
    /// （片側だけ変わって隙間や重なりができた回）を数える（レビュー 4a9ea60 の N-2002）。環境変数 PROBE_CASES（ふだん 400）
    #[test]
    #[ignore]
    fn probe_floating_anchor_edits() {
        let cases: usize = std::env::var("PROBE_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(400);
        let (mut edits, mut bad) = (0, 0);
        let mut r = Rng(17);
        for _ in 0..cases {
            let mut app = Is2dApp::default();
            for (a, b, c) in [
                (
                    Point::new(-200.0, -40.0),
                    Point::new(200.0, -40.0),
                    Color32::RED,
                ),
                (
                    Point::new(-200.0, 0.0),
                    Point::new(200.0, 0.0),
                    Color32::BLUE,
                ),
                (
                    Point::new(-200.0, 40.0),
                    Point::new(200.0, 40.0),
                    Color32::GREEN,
                ),
                (
                    Point::new(-200.0, -80.0),
                    Point::new(200.0, 80.0),
                    Color32::RED,
                ),
            ] {
                app.tool.brush_color = c;
                let pts: Vec<Point> = (0..=10).map(|i| a.lerp(b, i as f64 / 10.0)).collect();
                commit_brush_line(&mut app, &brush_line(&pts, 36.0));
            }
            let x0 = -150.0 + r.f() * 100.0;
            for action in [
                doc_actions::Action::SelectMarquee {
                    p0: Point::new(x0, -130.0),
                    p1: Point::new(x0 + 150.0 + r.f() * 100.0, 130.0),
                    additive: false,
                },
                doc_actions::Action::NudgeSelection {
                    delta: KVec2::new(0.0, 7.0),
                },
            ] {
                doc_actions::apply(&mut app, action);
            }
            for _ in 0..20 {
                let Some(_) = app.session.floating() else {
                    break;
                };
                let segs: Vec<QuadBez> = app.session.floating_view()
                    .pieces()
                    .iter()
                    .flat_map(|s| s.contours())
                    .flat_map(|c| c.segments.iter().copied())
                    .collect();
                if segs.is_empty() {
                    break;
                }
                let q = segs[(r.f() * segs.len() as f64) as usize % segs.len()];
                let action = if r.f() < 0.6 {
                    doc_actions::Action::DeleteAnchor { pt: q.p0 }
                } else {
                    doc_actions::Action::AddAnchor { pt: q.eval(0.5) }
                };
                if !doc_actions::apply(&mut app, action) {
                    continue;
                }
                // 片の外（レイヤーの図形）の角だったときは、選択を外してレイヤーの地図で編集している
                let Some(_) = app.session.floating() else {
                    break;
                };
                edits += 1;
                if !PlanarMap::from_faces(app.session.floating_view().pieces().iter()).is_valid() {
                    bad += 1;
                }
            }
        }
        eprintln!(
            "PROBE 浮いた片のアンカーの編集 {edits} 回：片どうしが正しい地図でなくなった回 {bad}"
        );
    }

    /// 矩形で選んだあと、境目をまたぐ矩形を Shift で選び足したときの、同じ色の片どうしの接し方を数える
    #[test]
    #[ignore]
    fn probe_review_shift_marquee_merges() {
        let mut seams_total = 0;
        let mut cases_with_seam = 0;
        let mut r = Rng(5);
        for case in 0..40 {
            let mut app = Is2dApp::default();
            let strokes = [
                (
                    Point::new(-200.0, -40.0),
                    Point::new(200.0, -40.0),
                    Color32::RED,
                ),
                (
                    Point::new(-200.0, 0.0),
                    Point::new(200.0, 0.0),
                    Color32::BLUE,
                ),
                (
                    Point::new(-200.0, 40.0),
                    Point::new(200.0, 40.0),
                    Color32::GREEN,
                ),
                (
                    Point::new(-200.0, -80.0),
                    Point::new(200.0, 80.0),
                    Color32::RED,
                ),
            ];
            for (a, b, c) in strokes {
                app.tool.brush_color = c;
                let pts: Vec<Point> = (0..=10).map(|i| a.lerp(b, i as f64 / 10.0)).collect();
                commit_brush_line(&mut app, &brush_line(&pts, 36.0));
            }
            let x1 = -100.0 + r.f() * 150.0;
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::SelectMarquee {
                    p0: Point::new(-260.0, -130.0),
                    p1: Point::new(x1, 130.0),
                    additive: false,
                },
            ));
            let before = app.session.floating().map_or(0, |_| app.session.floating_view().pieces().len());
            let (y0, y1) = (-130.0 + r.f() * 80.0, 30.0 + r.f() * 100.0);
            let x2 = x1 + 20.0 + r.f() * 80.0;
            doc_actions::apply(
                &mut app,
                doc_actions::Action::SelectMarquee {
                    p0: Point::new(x1 - 30.0, y0),
                    p1: Point::new(x2, y1),
                    additive: true,
                },
            );
            app.session.floating().expect("選択");
            let pieces: Vec<FillShape> = app.session.floating_view().pieces().to_vec();
            // 同じ色の片どうしが接している所：片の輪郭の区間の真ん中から、
            // 片の外へ 0.05px 出た点が、同じ色の別の片に入るか
            let mut seams = 0;
            for (ai, a) in pieces.iter().enumerate() {
                for c in a.contours() {
                    for q in c.segments.iter() {
                        let m = q.eval(0.5);
                        let d = q.deriv().eval(0.5).to_vec2();
                        if d.hypot() == 0.0 {
                            continue;
                        }
                        let n = KVec2::new(-d.y, d.x) / d.hypot() * 0.05;
                        for p in [m + n, m - n] {
                            if a.contains_point(p) {
                                continue;
                            }
                            if pieces.iter().enumerate().any(|(bi, b)| {
                                bi != ai && b.color() == a.color() && b.contains_point(p)
                            }) {
                                seams += 1;
                            }
                        }
                    }
                }
            }
            if seams > 0 {
                cases_with_seam += 1;
            }
            seams_total += seams;
            if case < 5 || seams > 0 {
                eprintln!(
                    "PROBE case {case}: 片 {before} → {}・同じ色の片と接する区間 {seams}",
                    pieces.len()
                );
            }
        }
        eprintln!(
            "PROBE 同じ色の片どうしが接した回 {cases_with_seam}/40（区間 合わせて {seams_total}）"
        );
    }

    /// 3レイヤーの絵を1レイヤーに貼って確定する時間（片どうしが重なる）
    #[test]
    #[ignore]
    fn probe_review_paste_three_layers_into_one() {
        for seed in [7u64, 8] {
            let app = dense_scene_app(seed);
            let lid0 = app.doc.root.layers[0].id;
            let base: Vec<FillShape> = app.scene().iter().cloned().collect();
            // 3レイヤーの生の図形を、下のレイヤーから順に、レイヤー 0 の片にする（新しい番号）
            let mut pieces: Vec<FillShape> = Vec::new();
            for l in 0..3 {
                let lid = app.doc.root.layers[l].id;
                for s in base
                    .iter()
                    .filter(|s| s.layer_id == lid && s.is_raw_at_scope(&[]))
                {
                    let mut p = s.clone().with_scope(lid0, &[]);
                    p.shape_id = crate::serial::next_shape_id();
                    pieces.push(p);
                }
            }
            for _ in 0..3 {
                let t = std::time::Instant::now();
                let mut out = base.clone();
                merge_shapes_into_canvas(&mut out, pieces.clone());
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let ok = out.len() != base.len();
                eprintln!(
                    "PROBE seed {seed}: 片 {}・{ms:.1}ms・置けた {ok}",
                    pieces.len()
                );
            }
        }
    }

    /// 計測の試し（ふだんは流さない）：重なった線の図形 30・60・120・240・480 個を塗り重ねる時間（`painted_map`。
    /// 前の版の、境目のそろっていないセルを最初に編集するときに作る地図）
    #[test]
    #[ignore]
    fn probe_painted_map_scaling() {
        for n in [30usize, 60, 120, 240, 480] {
            let mut r = Rng(5);
            let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
            let shapes: Vec<FillShape> = (0..n)
                .flat_map(|_| {
                    let p0 = Point::new(-390.0 + r.f() * 780.0, -215.0 + r.f() * 430.0);
                    let (len, ang) = (50.0 + r.f() * 250.0, r.f() * std::f64::consts::TAU);
                    let p1 = p0 + KVec2::new(ang.cos(), ang.sin()) * len;
                    let color = colors[(r.f() * 4.0) as usize % 4];
                    make_stroke(p0, p1, (1.0 + r.f() * 7.0) as f32, color)
                })
                .collect();
            let t = std::time::Instant::now();
            let m = crate::planar_map::painted_map_pub(&shapes).expect("塗り重ねた地図");
            let el = t.elapsed().as_secs_f64() * 1e3;
            assert!(m.is_valid());
            eprintln!(
                "PROBE 重なった図形 {}：塗り重ね {el:.1}ms｜辺 {}",
                shapes.len(),
                m.edges.len()
            );
        }
    }

    /// 計測の試し（ふだんは流さない）：大きい斜めの三角形（一辺 5,000・20,000・60,000px）のあるレイヤーに、ブラシを1本
    /// 描く時間（辺の数は同じで、形の大きさだけが違う）
    #[test]
    #[ignore]
    fn probe_large_shape_cost() {
        for size in [5_000.0f64, 20_000.0, 60_000.0] {
            let mut app = Is2dApp::default();
            let pts = [
                Point::new(0.0, 0.0),
                Point::new(size, size * 0.6),
                Point::new(size * 0.2, size),
            ];
            let segs: Vec<QuadBez> = (0..3)
                .map(|i| {
                    let (a, b) = (pts[i], pts[(i + 1) % 3]);
                    QuadBez::new(a, a.midpoint(b), b)
                })
                .collect();
            let tri = FillShape::new(
                Color32::RED,
                crate::vector_engine::Contour::new(segs),
                Vec::new(),
            );
            put_cel_shapes(&mut app, 0, 0, vec![tri]);
            // 斜めの辺の真ん中を横切るブラシ
            let m = pts[0].midpoint(pts[1]);
            let line: Vec<Point> = (0..=8)
                .map(|i| Point::new(m.x - 40.0 + 10.0 * i as f64, m.y + 30.0 - 7.5 * i as f64))
                .collect();
            app.tool.brush_color = Color32::BLUE;
            let t = std::time::Instant::now();
            commit_brush_line(&mut app, &brush_line(&line, 6.0));
            let el = t.elapsed().as_secs_f64() * 1e3;
            assert!(layer_map_valid(&app, 0));
            eprintln!("PROBE 三角形 {size}px：ブラシ 1 本 {el:.1}ms");
        }
    }

    #[test]
    #[ignore]
    fn probe_broken_cell_cost() {
        for n in [60usize, 150] {
            let mut app = Is2dApp::default();
            let lid = app.doc.root.layers[0].id;
            let mut r = Rng(9);
            let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
            let mut shapes: Vec<FillShape> = Vec::new();
            for _ in 0..n {
                let p0 = Point::new(-390.0 + r.f() * 780.0, -215.0 + r.f() * 430.0);
                let (len, ang) = (50.0 + r.f() * 250.0, r.f() * std::f64::consts::TAU);
                let p1 = p0 + KVec2::new(ang.cos(), ang.sin()) * len;
                let color = colors[(r.f() * 4.0) as usize % 4];
                let stroke = make_stroke(p0, p1, (1.0 + r.f() * 7.0) as f32, color);
                merge_shapes_into_canvas(
                    &mut shapes,
                    stroke.into_iter().map(|s| s.with_scope(lid, &[])).collect(),
                );
            }
            // 前の版の結合で描いたような、境目が両側の図形でそろっていない図形にする
            let shapes = misaligned(shapes);
            let count = shapes.len();
            put_cel_shapes(&mut app, 0, 0, shapes);
            let segs = |app: &Is2dApp| {
                app.doc.root.layers[0].keyframes[0]
                    .cel
                    .shapes()
                    .iter()
                    .map(|s| s.contours().map(|c| c.segments.len()).sum::<usize>())
                    .sum::<usize>()
            };
            let seg0 = segs(&app);
            let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
            // 400 角の矩形選択 → 10px 動かす → 選択を外す
            let t = std::time::Instant::now();
            select_ops::apply_marquee_selection(
                &mut app,
                Point::new(-200.0, -200.0),
                Point::new(200.0, 200.0),
                false,
            );
            let t_sel = ms(t);
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::NudgeSelection {
                    delta: KVec2::new(10.0, 0.0)
                }
            ));
            let t = std::time::Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            let t_commit = ms(t);
            // 全部を選んで、同じ位置に貼り付けて確定
            select_ops::apply_marquee_selection(
                &mut app,
                Point::new(-420.0, -240.0),
                Point::new(420.0, 240.0),
                false,
            );
            assert!(doc_actions::apply(&mut app, doc_actions::Action::Copy));
            assert!(canvas_tool::clear_selection(&mut app));
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::Paste { in_place: true }
            ));
            let t = std::time::Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            let t_paste = ms(t);
            // ブラシ 2 本
            app.tool.brush_color = Color32::from_rgb(10, 200, 200);
            let mut t_brush = Vec::new();
            for k in 0..2 {
                let y = -100.0 + 80.0 * k as f64;
                let pts: Vec<Point> = (0..=12)
                    .map(|i| Point::new(-200.0 + 30.0 * i as f64, y))
                    .collect();
                let t = std::time::Instant::now();
                commit_brush_line(&mut app, &brush_line(&pts, 6.0));
                t_brush.push(ms(t));
            }
            eprintln!(
                "PROBE 線 {n} 本（図形 {count}）：矩形選択 {t_sel:.1}ms｜動かして確定 {t_commit:.1}ms｜同じ位置に貼って確定 {t_paste:.1}ms｜ブラシ {:.1}・{:.1}ms｜区間 {seg0} → {}",
                t_brush[0], t_brush[1], segs(&app)
            );
        }
    }

    /// 全部を `Alt` 複製してずらして確定する、をくり返しても、地図は壊れず、直線の区間がたまらない
    /// （重ねた所の曲線は、元の曲線の一部のまま残る。直線に戻るのは、ほかの辺や頂点がすぐ近くにある所だけ。
    /// 前の作りでは、当て直せない辺を丸ごと細かい直線にしたので、3回目で区間の6割が 0.5px 未満の直線になった）
    #[test]
    fn repeated_alt_duplicate_keeps_curves() {
        let stage = (Point::new(-410.0, -235.0), Point::new(410.0, 235.0));
        let mut app = dense_scene_app(7);
        let straight = |app: &Is2dApp| {
            let mut n = 0usize;
            for l in 0..3 {
                for s in app.doc.root.layers[l].keyframes[0].cel.shapes().iter() {
                    for c in s.contours() {
                        n += c
                            .segments
                            .iter()
                            .filter(|q| q.p1 == q.p0.midpoint(q.p2))
                            .count();
                    }
                }
            }
            n
        };
        let stats = |app: &Is2dApp| {
            let (mut total, mut short) = (0usize, 0usize);
            for l in 0..3 {
                for s in app.doc.root.layers[l].keyframes[0].cel.shapes().iter() {
                    for c in s.contours() {
                        for q in c.segments.iter() {
                            total += 1;
                            if (q.p2 - q.p0).hypot() < 0.5 {
                                short += 1;
                            }
                        }
                    }
                }
            }
            (total, short)
        };
        let first = stats(&app);
        eprintln!(
            "はじめ：区間 {}（弦 0.5px 未満 {}、直線 {}）",
            first.0,
            first.1,
            straight(&app)
        );
        for k in 0..3 {
            select_ops::apply_marquee_selection(&mut app, stage.0, stage.1, false);
            assert!(doc_actions::apply(
                &mut app,
                doc_actions::Action::FinishMoveSelectionDrag {
                    total_delta: KVec2::new(6.0, 4.0),
                    alt_duplicate: true
                }
            ));
            let t = std::time::Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            eprintln!(
                "Alt 複製 {} 回目の確定 {:.1}ms",
                k + 1,
                t.elapsed().as_secs_f64() * 1e3
            );
            for l in 0..3 {
                assert!(layer_map_valid(&app, l), "{} 回目、レイヤー {l}", k + 1);
            }
            let (total, short) = stats(&app);
            let n_straight = straight(&app);
            eprintln!(
                "Alt 複製 {} 回目：区間 {total}（弦 0.5px 未満 {short}、直線 {n_straight}）",
                k + 1
            );
            assert!(
                n_straight * 100 < total,
                "{} 回目：区間 {total}・直線 {n_straight}",
                k + 1
            );
        }
    }

    /// 強さ・速さの試し（ふだんは流さない）：レビューと同じ条件で、矩形選択・動かして離す・`Alt` 複製・貼り付けの時間を測る。
    /// 結果の各レイヤーの地図が壊れていないかも見る
    /// Undo の控えのメモリ（2-2 の設計 3章）：図形の多い場面で、ブラシを 100 本描いたあとの Undo の控えが持つ地図と図形の
    /// 大きさ（同じ `Arc` は1回だけ数える）
    #[test]
    #[ignore]
    fn probe_undo_memory() {
        let mut app = dense_scene_app(7);
        let mut r = Rng(11);
        let t0 = std::time::Instant::now();
        for _ in 0..100 {
            app.tool.brush_color = Color32::from_rgb((r.f() * 255.0) as u8, 90, 160);
            let a = Point::new(-350.0 + r.f() * 700.0, -200.0 + r.f() * 400.0);
            let pts: Vec<Point> = (0..=12)
                .map(|j| a + KVec2::new(j as f64 * 4.0, 20.0 * (j as f64 * 0.4).sin()))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 3.0 + r.f() * 5.0));
        }
        let elapsed = t0.elapsed().as_secs_f64() * 1e3;
        let mut seen_maps: std::collections::HashSet<usize> = Default::default();
        let mut seen_shapes: std::collections::HashSet<usize> = Default::default();
        let (mut map_bytes, mut index_bytes, mut shape_bytes, mut edges, mut tiles) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut count = |tl: &timeline::Timeline| {
            for kf in tl.layers.iter().flat_map(|l| &l.keyframes) {
                for m in kf.cel.maps().iter() {
                    let (i, e) = m.stored().bytes_unseen(&mut seen_maps);
                    index_bytes += i;
                    map_bytes += e;
                }
            }
        };
        // 面の図形は文書に無い（地図から作る写し）。前の作りで文書が持っていた分の目安として、今のセルの面の図形の大きさ
        for kf in app.doc.root.layers.iter().flat_map(|l| &l.keyframes) {
            for sh in kf.cel.shapes().iter() {
                if seen_shapes.insert(std::sync::Arc::as_ptr(sh) as usize) {
                    shape_bytes +=
                        sh.contours().map(|c| c.segments.len() * 48).sum::<usize>() + 288;
                }
            }
        }
        let steps = app.history.undo_stack.len();
        for e in app.history.undo_stack.iter() {
            count(&e.timeline);
        }
        count(&app.doc.root);
        for kf in app.doc.root.layers.iter().flat_map(|l| &l.keyframes) {
            for m in kf.cel.maps().iter() {
                edges += m.base().map().edges.len();
                tiles += m.stored().tiles();
            }
        }
        eprintln!(
            "PROBE Undo の控え {steps} 件：地図の辺 {:.2}MB・塊の並び {:.2}MB（同じものは1回。面の図形は持たない。今のセルの面の図形は {:.2}MB）。今の地図の辺 {edges} 本・塊 {tiles} 個。辺1本 {} バイト。100 本で {elapsed:.0}ms",
            map_bytes as f64 / 1e6,
            index_bytes as f64 / 1e6,
            shape_bytes as f64 / 1e6,
            crate::stored_map::edge_size()
        );
    }

    /// 計算に使う写しの持ち主（画面の土台・カーソル合わせの地図）を空にする（どのセル画の写しも誰も持たない：初めて出す
    /// セルと同じ）。画面の土台は、次の `refresh_scene_base` で全部のレイヤーを作り直す
    fn drop_map_copies(app: &mut Is2dApp) {
        app.session.drop_scene_base();
        *app.session.planar_maps.lock() = None;
    }

    /// 色ごとの塗りの面積（図形の並び）
    fn areas_by_color<'a>(shapes: impl IntoIterator<Item = &'a FillShape>) -> Vec<(Color32, f64)> {
        let mut out: Vec<(Color32, f64)> = Vec::new();
        for s in shapes {
            let a: f64 = s.contours().map(|k| k.signed_area()).sum::<f64>().abs();
            match out.iter_mut().find(|x| x.0 == s.color()) {
                Some(x) => x.1 += a,
                None => out.push((s.color(), a)),
            }
        }
        out.sort_by_key(|x| x.0.to_array());
        out
    }

    fn same_areas(a: &[(Color32, f64)], b: &[(Color32, f64)], tol: f64) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.0 == y.0 && (x.1 - y.1).abs() <= tol * x.1.max(1.0))
    }

    /// 新しい中身は地図だけで運ぶ：コピー・カット・貼り付け・Alt 複製・シンボル化・シンボルの分解・ライブラリからの配置の
    /// どれでも、図形から地図を作らない（`LayerBase::of` はテストの下ごしらえだけ）。どのセル画の地図も決まりどおり
    #[test]
    fn new_content_never_builds_maps_from_shapes() {
        let mut app = dense_scene_app(11);
        let built = crate::planar_map::from_shape_builds();
        let act = |app: &mut Is2dApp, a: doc_actions::Action| doc_actions::apply(app, a);
        // 全部を選んでコピー → 貼り付け（ずらす）→ 確定
        assert!(act(&mut app, doc_actions::Action::SelectAll));
        assert!(doc_actions::copy_selected_shapes(&mut app));
        assert!(doc_actions::paste_clipboard(&mut app, false));
        assert!(app
            .session
            .floating()
            .is_some_and(|_| !app.session.floating_view().pieces().is_empty()));
        act(&mut app, doc_actions::Action::Deselect);
        // 矩形で切った片を Alt 複製で動かす → 確定
        assert!(act(
            &mut app,
            doc_actions::Action::SelectMarquee {
                p0: Point::new(-100.0, -80.0),
                p1: Point::new(60.0, 50.0),
                additive: false,
            }
        ));
        assert!(act(
            &mut app,
            doc_actions::Action::FinishMoveSelectionDrag {
                total_delta: KVec2::new(37.0, 21.0),
                alt_duplicate: true,
            }
        ));
        act(&mut app, doc_actions::Action::Deselect);
        // 1枚目のレイヤーをグループ化 → 1枚目のレイヤーの全部をシンボル化 → 分解（グループとして戻る）→ グループの解除 → 確定
        let lid0 = app.doc.root.layers[0].id;
        let first: Vec<usize> = (0..app.scene().len())
            .filter(|&i| app.scene().get(i).unwrap().layer_id == lid0)
            .collect();
        app.set_selected_indices(first);
        assert!(doc_actions::group_selected_shapes(&mut app).is_some());
        let first: Vec<usize> = (0..app.scene().len())
            .filter(|&i| app.scene().get(i).unwrap().layer_id == lid0)
            .collect();
        app.set_selected_indices(first);
        let sym = doc_actions::convert_selection_to_symbol(&mut app).expect("シンボル化");
        assert!(
            doc_actions::ungroup_selected_shapes(&mut app),
            "シンボルの分解"
        );
        assert!(
            doc_actions::ungroup_selected_shapes(&mut app),
            "グループの解除"
        );
        act(&mut app, doc_actions::Action::Deselect);
        // ライブラリから置く → カット → その場に貼り付け
        assert!(doc_actions::place_symbol_instance_from_library(
            &mut app, sym
        ));
        assert!(doc_actions::cut_selected_shapes(&mut app));
        assert!(doc_actions::paste_clipboard(&mut app, true));
        assert_eq!(
            crate::planar_map::from_shape_builds(),
            built,
            "図形から地図を作らない"
        );
        for layer in &app.doc.root.layers {
            for kf in &layer.keyframes {
                assert!(kf.cel.maps_problem().is_none());
            }
        }
    }

    /// 貼り付けた中身は元と同じ形（色ごとの面積・面の数）。シンボルの中へ（今の階層の座標が違う）その場に貼っても、
    /// シンボルの中からルートへ貼っても、ステージで同じ所・同じ大きさに置かれる
    #[test]
    fn pasted_content_keeps_its_shape_across_coordinate_spaces() {
        let mut app = Is2dApp::default();
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN];
        for (i, x) in [-120.0, 0.0, 120.0].into_iter().enumerate() {
            app.tool.brush_color = colors[i];
            let pts: Vec<Point> = (0..=10)
                .map(|j| Point::new(x + j as f64 * 9.0, 20.0 + (j as f64 * 0.7).sin() * 30.0))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 11.0));
        }
        // 元の形（ステージ）
        let original: Vec<FillShape> = app.scene().iter().cloned().collect();
        let areas = areas_by_color(&original);
        let bbox = crate::vector_engine::union_bounding_boxes(&original);

        // 同じ所へその場に貼る：片は元と同じ形
        assert!(doc_actions::apply(&mut app, doc_actions::Action::SelectAll));
        assert!(doc_actions::copy_selected_shapes(&mut app));
        assert!(doc_actions::paste_clipboard(&mut app, true));
        let pieces = app.session.floating_view().pieces().to_vec();
        assert_eq!(pieces.len(), original.len());
        assert!(same_areas(&areas_by_color(&pieces), &areas, 1e-9));
        doc_actions::apply(&mut app, doc_actions::Action::Undo);

        // 回して縮めたシンボルの中へ、その場に貼る：ステージで同じ所・同じ形
        put_cel_shapes(
            &mut app,
            0,
            0,
            [
                original.clone(),
                vec![rect_shape(300.0, 300.0, 340.0, 330.0, Color32::BLACK)],
            ]
            .concat(),
        );
        let black = app
            .scene()
            .iter()
            .position(|s| s.color() == Color32::BLACK)
            .unwrap();
        app.set_selected_indices(vec![black]);
        assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
        assert!(doc_actions::rotate_selected_shapes(&mut app, 0.6));
        assert!(scale_selection(&mut app, 0.45, 0.8));
        let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
        assert!(symbol_ops::enter_symbol_timeline(
            &mut app,
            binding.instance_gid
        ));
        assert!(doc_actions::paste_clipboard(&mut app, true));
        let pieces = app.session.floating_view().pieces().to_vec();
        let b2 = crate::vector_engine::union_bounding_boxes(&pieces);
        assert!(
            (b2.x0 - bbox.x0).abs() < 1e-6
                && (b2.y0 - bbox.y0).abs() < 1e-6
                && (b2.x1 - bbox.x1).abs() < 1e-6
                && (b2.y1 - bbox.y1).abs() < 1e-6,
            "ステージで同じ所：{bbox:?} と {b2:?}"
        );
        assert!(same_areas(&areas_by_color(&pieces), &areas, 1e-6));
        // 確定すると、シンボルの中の地図（変換をかけて、その座標の格子で正しい地図にしたもの）
        doc_actions::apply(&mut app, doc_actions::Action::Deselect);
        let inside: Vec<FillShape> = app
            .scene()
            .iter()
            .filter(|s| s.color() != Color32::BLACK)
            .cloned()
            .collect();
        assert!(same_areas(&areas_by_color(&inside), &areas, 1e-4));

        // シンボルの中から写して、ルートへその場に貼る：ステージで同じ所
        assert!(doc_actions::apply(&mut app, doc_actions::Action::SelectAll));
        assert!(doc_actions::copy_selected_shapes(&mut app));
        let inside_bbox = crate::vector_engine::union_bounding_boxes(
            &app.scene()
                .iter()
                .filter(|s| app.session.selection().contains_shape(s))
                .cloned()
                .collect::<Vec<_>>(),
        );
        exit_symbols_to(&mut app, 0);
        let lid = app.doc.root.layers[0].id;
        assert!(doc_actions::paste_clipboard(&mut app, true));
        let pieces = app.session.floating_view().pieces().to_vec();
        assert!(pieces.iter().all(|p| p.layer_id == lid));
        let b3 = crate::vector_engine::union_bounding_boxes(&pieces);
        assert!(
            (b3.x0 - inside_bbox.x0).abs() < 1e-6 && (b3.y1 - inside_bbox.y1).abs() < 1e-6,
            "ステージで同じ所：{inside_bbox:?} と {b3:?}"
        );
    }

    /// 地図にできない所（格子の外）へ置く貼り付け・シンボルの分解は、文書を変えずに断る
    #[test]
    fn placing_content_off_the_grid_is_refused_without_changes() {
        // 貼り付け：ルートの x=60,000 にある形を、半分に縮めたシンボルの中へその場に貼ると、中の座標は 120,000 になる
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                rect_shape(60_000.0, 0.0, 60_040.0, 30.0, Color32::RED),
                rect_shape(0.0, 0.0, 40.0, 30.0, Color32::BLUE),
            ],
        );
        let red = app
            .scene()
            .iter()
            .position(|s| s.color() == Color32::RED)
            .unwrap();
        app.set_selected_indices(vec![red]);
        assert!(doc_actions::copy_selected_shapes(&mut app));
        let blue = app
            .scene()
            .iter()
            .position(|s| s.color() == Color32::BLUE)
            .unwrap();
        app.set_selected_indices(vec![blue]);
        assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
        let (binding, _, _) = symbol_ops::selected_single_symbol_info(&app).unwrap();
        // インスタンスを原点のまわりで半分にする
        let gid = binding.instance_gid;
        app.doc.root.layers[0]
            .keyframe_mut(0)
            .cel
            .instances_mut()
            .iter_mut()
            .find(|i| i.binding.instance_gid == gid)
            .unwrap()
            .binding
            .xform = kurbo::Affine::scale(0.5);
        app.refresh_scene_base();
        assert!(symbol_ops::enter_symbol_timeline(
            &mut app,
            binding.instance_gid
        ));
        let before = app.doc.library().at(0).unwrap().timeline.layers[0].keyframes[0]
            .cel
            .clone();
        let undo = app.history.undo_stack.len();
        assert!(!doc_actions::paste_clipboard(&mut app, true));
        let after = &app.doc.library().at(0).unwrap().timeline.layers[0].keyframes[0].cel;
        assert!(std::sync::Arc::ptr_eq(before.maps(), after.maps()));
        assert_eq!(after.revision(), before.revision());
        assert_eq!(app.history.undo_stack.len(), undo);
        assert!(app.session.floating().is_none());

        // シンボルの分解：中身が ±20,000px の横長の形を、3倍にしたインスタンスで分解すると 60,000px を超える
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![rect_shape(0.0, 0.0, 40_000.0, 10.0, Color32::RED)],
        );
        app.set_selected_indices(vec![0]);
        assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
        assert!(scale_selection(&mut app, 3.0, 3.0));
        let before = app.doc.root.layers[0].keyframes[0].cel.clone();
        let undo = app.history.undo_stack.len();
        assert!(!doc_actions::ungroup_selected_shapes(&mut app));
        let after = &app.doc.root.layers[0].keyframes[0].cel;
        assert!(std::sync::Arc::ptr_eq(before.instances(), after.instances()));
        assert_eq!(after.revision(), before.revision());
        assert_eq!(app.history.undo_stack.len(), undo);
        assert!(app.session.floating().is_none());
    }

    /// トゥイーンの途中のインスタンスを写すと、そのコマの結び付け（中割りの位置・再生位置）が中身になる。別のレイヤーへ
    /// その場に貼ると、同じ所に置かれる
    #[test]
    fn copying_a_tweened_instance_takes_the_inbetween_binding() {
        use crate::symbol_binding::SymbolLoopMode;
        let mut app = Is2dApp::default();
        let s0 = make_stroke(
            Point::new(-10.0, 0.0),
            Point::new(10.0, 0.0),
            6.0,
            Color32::RED,
        );
        let sym_id = app.doc.register_new_symbol(s0);
        extend_frame_at(&mut app.doc.root, 0, 7);
        app.doc.root.sync_total_frames();
        app.doc.next_group_id = crate::symbol::Counter::starting_at(2);
        let inst = |x: f64, first_frame: usize| timeline::SymbolInstance {
            binding: symbol_binding::SymbolInstanceBinding {
                instance_gid: 1,
                symbol_id: sym_id,
                xform: kurbo::Affine::translate((x, 100.0)),
                pivot: Point::ZERO,
                loop_mode: SymbolLoopMode::Loop,
                first_frame,
                color_effect: symbol_binding::ColorEffect::default(),
            },
            layer_id: 1,
            parent_scope: Vec::new(),
            order: 0,
        };
        app.doc.root.layers[0]
            .keyframe_mut(0)
            .cel
            .set_shapes_and_instances(Vec::new(), vec![inst(100.0, 0)]);
        let _ = doc_actions::apply(&mut app, doc_actions::Action::ToggleTween);
        doc_actions::switch_to_layer_and_frame(&mut app, 0, 4);
        doc_actions::timeline_insert_keyframe(&mut app, true);
        app.doc.root.layers[0]
            .keyframe_mut(1)
            .cel
            .set_shapes_and_instances(Vec::new(), vec![inst(300.0, 0)]);
        doc_actions::timeline_add_layer(&mut app);
        doc_actions::switch_to_layer_and_frame(&mut app, 0, 2);
        let tweened = app.doc.root.layers[0]
            .interpolated_tween_binding_at(2)
            .expect("トゥイーンの途中");
        let shape = app
            .scene()
            .iter()
            .position(|s| s.symbol_instance.is_some())
            .unwrap();
        app.set_selected_indices(vec![shape]);
        assert!(doc_actions::copy_selected_shapes(&mut app));
        let copied = &app.clipboard.content.as_ref().unwrap().layers[0]
            .1
            .instances()[0];
        assert_eq!(copied.binding.xform, tweened.xform);
        assert_eq!(copied.binding.first_frame, tweened.first_frame);
        // 新しいレイヤー（編集できる）へ、その場に貼る
        let new_lid = app.doc.root.layers[1].id;
        doc_actions::switch_to_layer_and_frame(&mut app, 1, 2);
        assert!(doc_actions::paste_clipboard(&mut app, true));
        let pasted = app.doc.root.layers[1]
            .cel_at(2)
            .unwrap()
            .instances()
            .first()
            .cloned()
            .expect("貼ったインスタンス");
        assert_eq!(pasted.layer_id, new_lid);
        assert_eq!(pasted.binding.xform, tweened.xform);
        assert_ne!(pasted.binding.instance_gid, tweened.instance_gid);
    }

    /// シンボルの分解は、そのコマのシンボルの中身を、インスタンスのあった重なりの位置に入れる（中のグループは、上にある別の
    /// インスタンスより下のまま。生の図形は浮いた片）
    #[test]
    fn breaking_apart_keeps_the_instance_stacking_position() {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                rect_shape(0.0, 0.0, 40.0, 40.0, Color32::RED),
                rect_shape(20.0, 20.0, 60.0, 60.0, Color32::BLUE),
            ],
        );
        let at = |app: &Is2dApp, c: Color32| app.scene().iter().position(|s| s.color() == c).unwrap();
        // 赤をグループにしてからシンボルにする（中身はグループ）。青もシンボルにする（上に重なる）
        let i = at(&app, Color32::RED);
        app.set_selected_indices(vec![i]);
        assert!(doc_actions::group_selected_shapes(&mut app).is_some());
        assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
        let i = at(&app, Color32::BLUE);
        app.set_selected_indices(vec![i]);
        assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
        let cel = app.doc.root.layers[0].keyframes[0].cel.clone();
        assert_eq!(cel.instances().len(), 2);
        let blue_gid = cel.instances()[1].binding.instance_gid;
        // 下のインスタンス（赤）を分解する
        let i = at(&app, Color32::RED);
        app.set_selected_indices(vec![i]);
        let built = crate::planar_map::from_shape_builds();
        assert!(doc_actions::ungroup_selected_shapes(&mut app));
        assert_eq!(crate::planar_map::from_shape_builds(), built);
        let cel = &app.doc.root.layers[0].keyframes[0].cel;
        assert!(cel.maps_problem().is_none());
        assert_eq!(cel.instances().len(), 1);
        assert_eq!(cel.instances()[0].binding.instance_gid, blue_gid);
        let group = cel
            .maps()
            .iter()
            .position(|m| m.group_path.len() == 1)
            .expect("分解して出たグループ");
        assert!(
            cel.instances()[0].order > group,
            "青のインスタンスは、分解して出たグループより上のまま"
        );
        // 画面の重なり：赤（グループ）の上に青
        let colors: Vec<Color32> = app.scene().iter().map(|s| s.color()).collect();
        assert_eq!(colors, vec![Color32::RED, Color32::BLUE]);
    }

    /// クリップボードの中身は、写した元の地図と塊を共有する（図形の写しを丸ごと持たない）
    #[test]
    fn clipboard_maps_share_tiles_with_the_source() {
        let mut app = dense_scene_app(5);
        assert!(doc_actions::apply(&mut app, doc_actions::Action::SelectAll));
        assert!(doc_actions::copy_selected_shapes(&mut app));
        let content = app.clipboard.content.as_ref().unwrap();
        assert_eq!(content.layers.len(), 3);
        for (lid, cel) in &content.layers {
            let layer = app.doc.root.layers.iter().find(|l| l.id == *lid).unwrap();
            let src = &layer.keyframes[0].cel.maps()[0];
            assert!(
                cel.maps()[0].stored().same(src.stored()),
                "丸ごと選んだ地図は、元の地図そのもの"
            );
        }
        // 矩形で切った片を写すと、片を重ねた地図（元の地図とは別の塊）
        doc_actions::apply(&mut app, doc_actions::Action::Deselect);
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectMarquee {
                p0: Point::new(-100.0, -80.0),
                p1: Point::new(60.0, 50.0),
                additive: false,
            }
        ));
        assert!(doc_actions::copy_selected_shapes(&mut app));
        let content = app.clipboard.content.as_ref().unwrap();
        for (_, cel) in &content.layers {
            assert!(cel.maps_problem().is_none());
        }
    }

    /// Undo・Redo の時間：図形の多い場面（3 レイヤー・線 180 本）で、ブラシ1本のあとの Undo と Redo（画面の土台を
    /// 作り直すまで）
    #[test]
    #[ignore]
    fn probe_undo_redo_timing() {
        let mut app = dense_scene_app(7);
        let mut r = Rng(21);
        let (mut undo, mut redo) = (Vec::new(), Vec::new());
        for _ in 0..30 {
            app.tool.brush_color = Color32::from_rgb((r.f() * 255.0) as u8, 80, 160);
            let a = Point::new(-300.0 + r.f() * 600.0, -200.0 + r.f() * 400.0);
            let pts: Vec<Point> = (0..=10)
                .map(|j| a + KVec2::new(j as f64 * 5.0, 0.0))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 6.0));
            let t = std::time::Instant::now();
            app.undo();
            app.refresh_scene_base();
            undo.push(t.elapsed().as_secs_f64() * 1e3);
            let t = std::time::Instant::now();
            app.redo();
            app.refresh_scene_base();
            redo.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let stat = |v: &[f64]| {
            (
                v.iter().sum::<f64>() / v.len() as f64,
                v.iter().cloned().fold(0.0, f64::max),
            )
        };
        let ((ua, um), (ra, rm)) = (stat(&undo), stat(&redo));
        eprintln!("PROBE Undo 平均 {ua:.2}ms・最大 {um:.2}ms｜Redo 平均 {ra:.2}ms・最大 {rm:.2}ms");
    }

    /// 再生・スクラブで初めて出すセルの手間：図形の多い場面（3 レイヤー・線 180 本）で、計算に使う写しの持ち主（画面の
    /// 土台・カーソル合わせの地図）を空にして（初めて出すセルと同じ）画面の土台を作り直す時間と、再生の1コマ（レイヤーごとの
    /// 結合メッシュ）の時間。あわせて、キーフレームを 30 個に増やして保存した作品を開く時間
    #[test]
    #[ignore]
    fn probe_unseen_cel_display() {
        let mut app = dense_scene_app(7);
        let n = app.scene().len();
        let ids: Vec<u64> = app.doc.root.layers.iter().map(|l| l.id).collect();
        let (mut rebuild, mut playback) = (Vec::new(), Vec::new());
        for _ in 0..20 {
            drop_map_copies(&mut app);
            let t = std::time::Instant::now();
            app.refresh_scene_base();
            rebuild.push(t.elapsed().as_secs_f64() * 1e3);
            drop_map_copies(&mut app);
            let t = std::time::Instant::now();
            for i in 0..ids.len() {
                let _ = app.doc.root.build_cel_world(
                    i,
                    0,
                    kurbo::Affine::IDENTITY,
                    None,
                    symbol::SymbolEvalCtx::new(app.doc.library()),
                );
            }
            playback.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let stat = |v: &[f64]| {
            (
                v.iter().sum::<f64>() / v.len() as f64,
                v.iter().cloned().fold(0.0, f64::max),
            )
        };
        let (ra, rm) = stat(&rebuild);
        let (pa, pm) = stat(&playback);
        eprintln!(
            "PROBE 初めて出すセル（図形 {n}）：画面の土台 平均 {ra:.1}ms・最大 {rm:.1}ms｜再生の1コマ 平均 {pa:.1}ms・最大 {pm:.1}ms"
        );
        for l in 0..ids.len() {
            doc_actions::switch_to_layer_and_frame(&mut app, l, 0);
            for f in 1..30 {
                doc_actions::switch_to_layer_and_frame(&mut app, l, f);
                doc_actions::timeline_insert_keyframe(&mut app, true);
            }
        }
        let dir = std::env::temp_dir().join(format!("is2d-probe-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("open.is2d");
        crate::project_io::save_project_to_path(&mut app, &path).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let t = std::time::Instant::now();
        let doc = read_saved_file(&path)
            .unwrap()
            .into_document()
            .unwrap();
        let open_ms = t.elapsed().as_secs_f64() * 1e3;
        let cels: usize = doc.root.layers.iter().map(|l| l.keyframes.len()).sum();
        eprintln!(
            "PROBE 開く：セル {cels}・ファイル {:.1}MB・{open_ms:.0}ms",
            size as f64 / 1e6
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 画面に出しているセル画の計算に使う写しの持ち主は画面の土台：ほかのセル画（保存・再生・書き出しのように、写しの
    /// 持ち主のいない地図）をいくつ使っても、写しは同じもののままで、ブラシ1本のあとも変わらない面の図形は版が同じ
    /// （描画キャッシュを引き継ぐ）。アプリ全体の置き場が無いので、ほかの処理に左右されない（レビュー 7f63b04 の N-1001。
    /// 直す前は、ほかのセル画を 64 個通すと写しが置き場から押し出され、面の図形がみな新しい版になった）
    #[test]
    fn shown_cel_map_copy_does_not_depend_on_other_cels() {
        let mut app = Is2dApp::default();
        let mut r = Rng(3);
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN];
        for i in 0..20 {
            app.tool.brush_color = colors[i % 3];
            let a = Point::new(-300.0 + r.f() * 600.0, -200.0 + r.f() * 400.0);
            let b = Point::new(-300.0 + r.f() * 600.0, -200.0 + r.f() * 400.0);
            let pts: Vec<Point> = (0..=12).map(|j| a.lerp(b, j as f64 / 12.0)).collect();
            commit_brush_line(&mut app, &brush_line(&pts, 5.0));
        }
        let lid = app.doc.root.layers[0].id;
        let held = |app: &Is2dApp| app.session.base_cache().cel(lid).unwrap().scope_base(&[]);
        let before = held(&app);
        let versions: Vec<(u64, u64)> = app
            .scene()
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        for seed in 0..100 {
            let cel = crate::timeline::Cel::from_saved(
                vec![crate::timeline::SavedScopeMap {
                    group_path: Vec::new(),
                    tiles: Vec::new(),
                    stored: Some(crate::stored_map::StoredMap::from_dense(
                        &native_scene(seed, 4),
                        None,
                    )),
                }],
                Vec::new(),
            );
            assert!(!cel.shapes().is_empty());
        }
        assert!(
            std::sync::Arc::ptr_eq(&before, &held(&app)),
            "ほかのセル画を使っても、画面の土台の写しは同じ"
        );
        // 何も無い所にブラシ1本：前からの面の図形は、みな同じ版のまま
        app.tool.brush_color = Color32::BLACK;
        let pts: Vec<Point> = (0..=6)
            .map(|j| Point::new(500.0 + j as f64 * 4.0, 300.0))
            .collect();
        commit_brush_line(&mut app, &brush_line(&pts, 4.0));
        let after: std::collections::HashSet<(u64, u64)> = app
            .scene()
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        assert!(
            versions.iter().all(|v| after.contains(v)),
            "変わらない面の図形の版が変わった"
        );
        assert_eq!(after.len(), versions.len() + 1);
    }

    /// 矩形で選んで動かしても、Undo の控えの浮いた片は、しまう形だけを持ち（計算に使う写しは持たない）、残りの地図は
    /// セル画の地図の、切り口で変わらなかった塊を共有する（レビュー 7f63b04 の N-2002。直す前は、控え1件が残りの地図を、
    /// 計算に使う形と、塊を共有しないしまう形の2つで丸ごと持った）。セッションの浮いた片の箱も預かり物を持たず、控えは
    /// その箱を分け合う（設計：claude/浮いた片の中身と写しを分ける設計_cb00886.md）
    #[test]
    fn floating_undo_snapshots_hold_only_stored_maps() {
        let mut app = dense_scene_app(7);
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(-60.0, -40.0),
            Point::new(60.0, 40.0),
            false,
        );
        for _ in 0..3 {
            doc_actions::finish_move_selection_drag(&mut app, KVec2::new(5.0, 3.0), false);
        }
        // 次の操作の控えは、セッションの浮いた片の箱を分け合う（動かしたあと、もう1回動かすと、その前の控えに入る）
        let now = app.session.shared_floating().expect("浮いた片");
        doc_actions::finish_move_selection_drag(&mut app, KVec2::new(5.0, 3.0), false);
        let last = app.history.undo_stack.back().and_then(|e| e.floating.clone());
        assert!(
            last.is_some_and(|l| std::sync::Arc::ptr_eq(&l, &now)),
            "控えが、セッションの浮いた片の箱を分け合っていない"
        );
        let cel_map = |lid: u64| {
            let l = app.doc.root.layers.iter().find(|l| l.id == lid).unwrap();
            l.keyframes[0].cel.maps()[0].stored().clone()
        };
        let mut rests = 0;
        let session_box = app.session.shared_floating();
        for fl in app.history.undo_stack.iter().filter_map(|e| e.floating.as_ref()).chain(session_box.as_ref()) {
            for (lid, rest, groups) in fl.stored_maps() {
                assert!(groups.iter().all(|(_, held)| !held), "控えの片が写しを持つ");
                let Some((rest, held)) = rest else {
                    continue;
                };
                assert!(!held, "控えの残りが写しを持つ");
                let cel = cel_map(lid);
                let shared = rest.shared_tiles(&cel);
                let mut seen = std::collections::HashSet::new();
                let _ = cel.bytes_unseen(&mut seen);
                let own = rest.bytes_unseen(&mut seen);
                let whole = rest.bytes_unseen(&mut std::collections::HashSet::new());
                eprintln!(
                    "PROBE 残り 塊 {}・共有 {shared}｜控えが持つ分 {:.1}KB（丸ごとなら {:.1}KB）",
                    rest.tiles(),
                    (own.0 + own.1) as f64 / 1e3,
                    (whole.0 + whole.1) as f64 / 1e3
                );
                assert!(
                    shared * 4 >= rest.tiles() * 3,
                    "残りの塊 {} 個のうち、セル画と共有するのが {shared} 個",
                    rest.tiles()
                );
                rests += 1;
            }
        }
        assert!(rests > 0, "控えに浮いた片の残りがある");
    }

    /// セル画の地図は場所ごとの塊でしまう：ブラシ1本で新しくなるのは、線の近くの塊だけ（ほかは前の地図・Undo の控えと
    /// 共有する）。計算に使う形は塊から決まった順で作るので、地図のしまい場所が空になっても、作り直した地図はビットまで
    /// 同じで、編集はセル画の地図から始まる（図形から作り直さない）。Undo で戻ると、前の地図そのもの（同じ塊）に戻る
    #[test]
    fn stored_map_shares_tiles_and_rebuilds_identically() {
        let mut app = Is2dApp::default();
        let mut r = Rng(5);
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN];
        for i in 0..40 {
            app.tool.brush_color = colors[i % 3];
            let a = Point::new(-380.0 + r.f() * 760.0, -200.0 + r.f() * 400.0);
            let b = Point::new(-380.0 + r.f() * 760.0, -200.0 + r.f() * 400.0);
            let pts: Vec<Point> = (0..=16).map(|j| a.lerp(b, j as f64 / 16.0)).collect();
            commit_brush_line(&mut app, &brush_line(&pts, 4.0 + r.f() * 6.0));
        }
        let scope = |app: &Is2dApp| app.doc.root.layers[0].keyframes[0].cel.maps()[0].clone();
        let bits = |m: &PlanarMap| {
            (
                m.nodes
                    .iter()
                    .map(|p| [p.x.to_bits(), p.y.to_bits()])
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
                            e.left_src,
                            e.right_src,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let before = scope(&app);
        let before_bits = bits(before.base().map());
        assert!(before.stored().tiles() > 50, "塊が十分ある場面");

        // 塊から作り直した地図はビットまで同じで、操作が読む地図（画面の土台が持つ写し）も同じ
        assert_eq!(bits(&before.dense()), before_bits);
        let base = app
            .session
            .base_cache()
            .cel(app.doc.root.layers[0].id)
            .map(|c| c.scope_base(&[]))
            .unwrap();
        assert!(
            base.stored.same(before.stored()),
            "画面の土台のセル画の地図"
        );
        assert_eq!(bits(base.map()), before_bits);

        // ブラシ1本（小さな線）で新しくなるのは、線の近くの塊だけ。写しの持ち主がいなくても、図形から作り直さない
        drop_map_copies(&mut app);
        let built = crate::planar_map::from_shape_builds();
        app.tool.brush_color = Color32::BLACK;
        let pts: Vec<Point> = (0..=8)
            .map(|j| Point::new(10.0 + j as f64 * 3.0, 5.0))
            .collect();
        commit_brush_line(&mut app, &brush_line(&pts, 4.0));
        assert_eq!(
            crate::planar_map::from_shape_builds(),
            built,
            "図形から作り直さない"
        );
        let after = scope(&app);
        let (n, shared) = (
            after.stored().tiles(),
            after.stored().shared_tiles(before.stored()),
        );
        assert!(
            n - shared <= 12,
            "新しい塊は線の近くだけ：{} 個のうち {} 個",
            n,
            n - shared
        );

        // Undo で、前の地図そのものに戻る
        app.undo();
        let undone = scope(&app);
        assert!(undone.stored().same(before.stored()));
        assert_eq!(bits(&undone.dense()), before_bits);

        // 塊を持つのは地図だけ：前の地図と共有した塊も、前の地図を捨てれば持ち主はその地図1つに戻る（アプリ全体の
        // 置き場は無い）
        use crate::stored_map::StoredMap;
        let map = before.base().map().clone();
        let first = StoredMap::from_dense(&map, None);
        assert!(first.tile_owners().iter().all(|&n| n == 1));
        let same = StoredMap::from_dense(&map, Some(&first));
        assert!(same.same_content(&first));
        assert_eq!(same.shared_tiles(&first), first.tiles());
        let ids: std::collections::HashSet<u64> = before
            .base()
            .map()
            .edges
            .iter()
            .filter_map(|e| e.left_src)
            .take(3)
            .collect();
        let fewer = StoredMap::from_dense(&map.restricted(&ids), Some(&first));
        assert!(fewer.shared_tiles(&first) > 0);
        drop((first, same));
        assert!(fewer.tile_owners().iter().all(|&n| n == 1));
    }

    /// 面を外す操作（削除・グループ化）のあとの地図は、前の地図からその面を外した地図（図形から作り直さない）。外した面の
    /// まわりのほかの塊は、前の地図と共有したまま
    #[test]
    fn delete_and_group_restrict_the_stored_map() {
        let mut app = Is2dApp::default();
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN];
        for (i, x) in [-200.0, 0.0, 200.0].into_iter().enumerate() {
            app.tool.brush_color = colors[i];
            let pts: Vec<Point> = (0..=8)
                .map(|j| Point::new(x + j as f64 * 6.0, 0.0))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 12.0));
        }
        let cel = |app: &Is2dApp| app.doc.root.layers[0].keyframes[0].cel.clone();
        let index_of = |app: &Is2dApp, c: Color32| {
            scene_summary(app)
                .shapes
                .iter()
                .position(|s| s.color() == c)
                .unwrap()
        };
        let before = cel(&app);
        assert_eq!(before.maps().len(), 1);
        let restricted = |keep: &[Color32]| {
            let ids: std::collections::HashSet<u64> = before
                .shapes()
                .iter()
                .filter(|s| keep.contains(&s.color()))
                .map(|s| s.shape_id)
                .collect();
            crate::stored_map::StoredMap::from_dense(
                &before.maps()[0].base().map().restricted(&ids),
                None,
            )
        };

        // 削除：残った面だけの地図
        let built = crate::planar_map::from_shape_builds();
        let green = index_of(&app, Color32::GREEN);
        app.set_selected_indices(vec![green]);
        assert!(doc_actions::delete_selected_shapes(&mut app));
        assert_eq!(
            crate::planar_map::from_shape_builds(),
            built,
            "図形から作り直さない"
        );
        let after = cel(&app);
        assert!(after.maps()[0]
            .stored()
            .same_content(&restricted(&[Color32::RED, Color32::BLUE])));
        assert!(after.maps()[0].stored().shared_tiles(before.maps()[0].stored()) > 0);

        // グループ化：ルートは残りの面、グループは選んだ面だけの地図
        app.undo();
        let red = index_of(&app, Color32::RED);
        let blue = index_of(&app, Color32::BLUE);
        app.set_selected_indices(vec![red, blue]);
        let built = crate::planar_map::from_shape_builds();
        let gid = doc_actions::group_selected_shapes(&mut app).expect("グループ化");
        assert_eq!(
            crate::planar_map::from_shape_builds(),
            built,
            "図形から作り直さない"
        );
        let after = cel(&app);
        let map_of = |path: &[u64]| {
            after
                .maps()
                .iter()
                .find(|m| m.group_path == path)
                .unwrap()
                .stored()
                .clone()
        };
        assert!(map_of(&[]).same_content(&restricted(&[Color32::GREEN])));
        assert!(map_of(&[gid]).same_content(&restricted(&[Color32::RED, Color32::BLUE])));
    }

    /// 選んだグループを動かす・回す・裏返すと、グループの中の地図に変換をかけた地図になる（図形はその面。図形から地図を
    /// 作り直さない）
    #[test]
    fn transforming_a_group_transforms_its_map() {
        let mut app = Is2dApp::default();
        for (i, x) in [-60.0, 0.0, 60.0].into_iter().enumerate() {
            app.tool.brush_color = [Color32::RED, Color32::BLUE, Color32::GREEN][i];
            let pts: Vec<Point> = (0..=8)
                .map(|j| Point::new(x + j as f64 * 4.0, j as f64 * 3.0))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 9.0));
        }
        assert!(doc_actions::apply(&mut app, doc_actions::Action::SelectAll));
        let gid = doc_actions::group_selected_shapes(&mut app).expect("グループ化");
        let built = crate::planar_map::from_shape_builds();
        let group_map = |app: &Is2dApp| {
            app.doc.root.layers[0].keyframes[0]
                .cel
                .maps()
                .iter()
                .find(|m| m.group_path == [gid])
                .unwrap()
                .base()
        };
        let edges = group_map(&app).map().edges.len();
        let ids = |app: &Is2dApp| {
            let mut v: Vec<_> = app.doc.root.layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .filter(|s| s.group_path == [gid])
                .map(|s| s.shape_id)
                .collect();
            v.sort_unstable();
            v
        };
        let before = ids(&app);
        assert_eq!(before.len(), 3);
        for action in [
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(13.0, -7.0),
            },
            doc_actions::Action::RotateSelection { angle_rad: 0.7 },
            doc_actions::Action::FlipSelection {
                flip_x: true,
                flip_y: false,
            },
        ] {
            assert!(doc_actions::apply(&mut app, action));
            let cel = &app.doc.root.layers[0].keyframes[0].cel;
            assert!(cel.maps_problem().is_none());
            assert_eq!(
                group_map(&app).map().edges.len(),
                edges,
                "つながりはそのまま"
            );
            assert_eq!(ids(&app), before, "図形の番号は引き継ぐ");
        }
        assert_eq!(
            crate::planar_map::from_shape_builds(),
            built,
            "図形から地図を作り直さない"
        );
        // 大きく縮めて頂点が近づいても（塗り重ねて作る：`realized`）、正しい地図で、図形の番号は引き継ぐ。Undo で戻る
        let n = app.history.undo_stack.len();
        assert!(scale_selection(&mut app, 0.02, 0.02));
        assert!(app.doc.root.layers[0].keyframes[0].cel.maps_problem().is_none());
        assert!(ids(&app).iter().all(|id| before.contains(id)));
        while app.history.undo_stack.len() > n {
            assert!(doc_actions::apply(&mut app, doc_actions::Action::Undo));
        }
        assert_eq!(ids(&app), before);
        assert_eq!(group_map(&app).map().edges.len(), edges);
    }

    /// 矩形で一部を切った片と、丸ごと選んだ別の生の図形を一緒にグループにしても、どちらも消えない（選んだ生の図形はみな
    /// 片に持ち上げてから、グループの階層へ片の地図として1回で書く）
    #[test]
    fn grouping_a_marquee_piece_with_a_whole_shape_keeps_both() {
        let mut app = Is2dApp::default();
        for (i, y) in [100.0, 300.0].into_iter().enumerate() {
            app.tool.brush_color = [Color32::RED, Color32::BLUE][i];
            let pts: Vec<Point> = (0..=20)
                .map(|j| Point::new(100.0 + j as f64 * 10.0, y))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 10.0));
        }
        let area = |app: &Is2dApp, c: Color32| -> f64 {
            app.doc.root.layers[0].keyframes[0]
                .cel
                .shapes()
                .iter()
                .filter(|s| s.color() == c)
                .map(|s| s.contours().map(|k| k.signed_area()).sum::<f64>().abs())
                .sum()
        };
        let (red, blue) = (area(&app, Color32::RED), area(&app, Color32::BLUE));
        // 赤い線の右半分を矩形で切り、青い線を Shift クリックで丸ごと足す
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectMarquee {
                p0: Point::new(200.0, 80.0),
                p1: Point::new(330.0, 120.0),
                additive: false,
            },
        ));
        let blue_shape = app
            .scene()
            .iter()
            .find(|s| s.color() == Color32::BLUE)
            .unwrap()
            .clone();
        { let mut sel = app.session.selection().clone(); sel.insert_shape(&blue_shape); app.session.replace_selection(sel); }
        doc_actions::group_selected_shapes(&mut app).expect("グループ化");
        doc_actions::apply(&mut app, doc_actions::Action::Deselect);
        let all = |app: &Is2dApp, c: Color32| -> f64 {
            app.doc.root.layers[0].keyframes[0]
                .cel
                .maps()
                .iter()
                .flat_map(|m| m.base().faces(&m.group_path).iter().cloned().collect::<Vec<_>>())
                .filter(|s| s.color() == c)
                .map(|s| s.contours().map(|k| k.signed_area()).sum::<f64>().abs())
                .sum()
        };
        // 面積は折れ線で近づけて測るので、切り口で曲線が分かれる分だけ、ごくわずかに変わる
        assert!(
            (all(&app, Color32::RED) - red).abs() < red * 1e-3,
            "赤い線の面積"
        );
        assert!(
            (all(&app, Color32::BLUE) - blue).abs() < blue * 1e-3,
            "青い線の面積"
        );
        assert!(
            app.doc.root.layers[0].keyframes[0]
                .cel
                .maps()
                .iter()
                .any(|m| !m.group_path.is_empty()),
            "グループの階層ができる"
        );
    }

    /// シンボルの中（インスタンスに平行移動・回転・拡大がかかっている）で矩形で選ぶと、囲んだ所（ステージ座標）が切られる：
    /// 切る形は文書の座標へ写して地図に渡す
    #[test]
    fn marquee_inside_transformed_symbol_cuts_the_enclosed_part() {
        let mut app = Is2dApp::default();
        let pts: Vec<Point> = (0..=20)
            .map(|j| Point::new(100.0 + j as f64 * 11.0, 100.0))
            .collect();
        commit_brush_line(&mut app, &brush_line(&pts, 10.0));
        assert!(doc_actions::apply(&mut app, doc_actions::Action::SelectAll));
        doc_actions::convert_selection_to_symbol(&mut app).expect("シンボル化");
        for action in [
            Op::Do(doc_actions::Action::NudgeSelection {
                delta: KVec2::new(300.0, 0.0),
            }),
            Op::Do(doc_actions::Action::RotateSelection { angle_rad: 0.3 }),
            Op::Scale(1.4, 0.8),
        ] {
            assert!(op(&mut app, action));
        }
        let binding = app.scene().iter().find_map(|s| s.symbol_instance).unwrap();
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::EnterSymbolTimeline {
                instance_gid: binding.instance_gid,
            },
        ));
        let line = app
            .scene()
            .iter()
            .map(|s| s.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap();
        // 線の右半分を囲む（ステージ座標）
        let rect = kurbo::Rect::new(
            line.center().x,
            line.y0 - 20.0,
            line.x1 + 20.0,
            line.y1 + 20.0,
        );
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::SelectMarquee {
                p0: Point::new(rect.x0, rect.y0),
                p1: Point::new(rect.x1, rect.y1),
                additive: false,
            },
        ));
        let pieces = app
            .session
            .floating()
            .map(|_| app.session.floating_view().pieces().to_vec())
            .unwrap_or_default();
        assert!(!pieces.is_empty(), "囲んだ所が片になる");
        // 切り口の頂点は文書の座標の格子（1/4096px）に置くので、ステージではその拡大の分（1.4 倍）まで外へ出うる
        let eps = 1e-3;
        for p in &pieces {
            let b = p.bounding_box();
            assert!(
                b.x0 >= rect.x0 - eps
                    && b.x1 <= rect.x1 + eps
                    && b.y0 >= rect.y0 - eps
                    && b.y1 <= rect.y1 + eps,
                "片 {b:?} が矩形 {rect:?} の中にある"
            );
        }
        let area: f64 = pieces.iter().map(|p| p.bounding_box().area()).sum();
        assert!(area > 0.0);
    }

    /// セル画は階層ごとの地図とインスタンスだけを持ち、生の図形はその階層のいちばん下（Adobe Animate と同じ）。図形の
    /// 並びでインスタンスより上に渡した生の図形も、インスタンスの下に並ぶ。面の図形は地図から作る写しで、番号は面の印
    #[test]
    fn cel_keeps_maps_with_raw_shapes_below_instances() {
        use crate::timeline::{Cel, Part};
        let stroke =
            |x: f64, c: Color32| make_stroke(Point::new(x, 0.0), Point::new(x + 30.0, 0.0), 6.0, c);
        let inst = |gid: u64| crate::timeline::SymbolInstance {
            binding: crate::symbol_binding::SymbolInstanceBinding::new(gid, 1, kurbo::Affine::IDENTITY),
            layer_id: 1,
            parent_scope: Vec::new(),
            order: 0,
        };
        let mut parts = vec![Part::Instance(inst(7))];
        parts.extend(
            stroke(0.0, Color32::RED)
                .into_iter()
                .map(|s| Part::Shape(std::sync::Arc::new(s))),
        );
        parts.push(Part::Instance(inst(8)));
        parts.extend(
            stroke(60.0, Color32::BLUE)
                .into_iter()
                .map(|s| Part::Shape(std::sync::Arc::new(s))),
        );
        let mut cel = Cel::default();
        cel.set_new_parts(parts).unwrap();
        assert!(cel.maps_problem().is_none());
        assert_eq!(cel.maps().len(), 1);
        assert_eq!(
            cel.instances()
                .iter()
                .map(|i| (i.binding.instance_gid, i.order))
                .collect::<Vec<_>>(),
            vec![(7, 1), (8, 1)],
            "インスタンスは生の図形の上に、渡した順で並ぶ"
        );
        let kinds: Vec<bool> = cel
            .parts()
            .iter()
            .map(|p| matches!(p, Part::Shape(_)))
            .collect();
        assert_eq!(kinds, vec![true, true, false, false]);
        // 面の図形の番号は地図の面の印で、作り直しても同じ（このセル画の写しは誰も持たないので、呼ぶたびに作り直す）
        let ids: Vec<u64> = cel.shapes().iter().map(|s| s.shape_id).collect();
        assert_eq!(
            cel.shapes().iter().map(|s| s.shape_id).collect::<Vec<_>>(),
            ids
        );
    }

    /// 地図にできない形（格子の外 ±65,536px）を作る編集は、文書を変える前に断って知らせる：グループの変形（グループが
    /// 裂けない）、シンボルの分解（中身が消えない）、図形の書き込み（地図の無い階層を作らない）
    #[test]
    fn off_grid_edits_are_refused_before_changing_the_document() {
        let warned = |app: &mut Is2dApp| {
            std::mem::take(&mut app.notices)
                .iter()
                .last()
                .is_some_and(|t| t.message.contains("±65,536px"))
        };
        let mut app = Is2dApp::default();
        for (i, x) in [-60.0, 0.0, 60.0].into_iter().enumerate() {
            app.tool.brush_color = [Color32::RED, Color32::BLUE, Color32::GREEN][i];
            let pts: Vec<Point> = (0..=8)
                .map(|j| Point::new(x + j as f64 * 4.0, j as f64 * 3.0))
                .collect();
            commit_brush_line(&mut app, &brush_line(&pts, 9.0));
        }
        // 緑の線をシンボルにして、3つをまとめてグループにする（グループの中に生の図形とインスタンス）
        let green = app
            .scene()
            .iter()
            .position(|s| s.color() == Color32::GREEN)
            .unwrap();
        app.set_selected_indices(vec![green]);
        doc_actions::convert_selection_to_symbol(&mut app).expect("シンボル化");
        assert!(doc_actions::apply(&mut app, doc_actions::Action::SelectAll));
        doc_actions::group_selected_shapes(&mut app).expect("グループ化");
        let cel = |app: &Is2dApp| app.doc.root.layers[0].keyframes[0].cel.clone();
        let before = cel(&app);
        app.notices = Default::default();

        // グループの変形：格子の外へ出るなら何も動かさない
        assert!(!doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(100_000.0, 0.0),
            },
        ));
        assert!(warned(&mut app));
        let after = cel(&app);
        assert!(std::sync::Arc::ptr_eq(before.maps(), after.maps()));
        assert!(std::sync::Arc::ptr_eq(before.instances(), after.instances()));

        // シンボルの分解：インスタンスだけを格子の外の近くへ動かし（インスタンスは地図を持たないので動かせる）、中身が
        // 格子の外にかかるなら分解しない（断った変形は Undo の1手にならないので、1回でグループ化の前に戻る）
        app.undo();
        let inst_idx = app
            .scene()
            .iter()
            .position(|s| s.symbol_instance.is_some())
            .unwrap();
        app.set_selected_indices(vec![inst_idx]);
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(65_530.0, 0.0),
            },
        ));
        let before = cel(&app);
        app.notices = Default::default();
        assert!(!doc_actions::ungroup_selected_shapes(&mut app));
        assert!(warned(&mut app));
        let after = cel(&app);
        assert!(std::sync::Arc::ptr_eq(before.maps(), after.maps()));
        assert!(std::sync::Arc::ptr_eq(before.instances(), after.instances()));
        assert!(app.session.floating().is_none());
    }

    /// 浮いた片は地図で持つ：矩形で選ぶ・選び足す・動かす・確定する・一部を外す、のどれでも図形から地図を作らない
    /// （片と残りは、セル画の地図を切った地図。確定は、残りの地図に片の地図を重ねる）
    #[test]
    fn floating_ops_never_build_maps_from_shapes() {
        let mut app = Is2dApp::default();
        let mut r = Rng(3);
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN];
        for i in 0..30 {
            app.tool.brush_color = colors[i % 3];
            let a = Point::new(-300.0 + r.f() * 600.0, -150.0 + r.f() * 300.0);
            let b = Point::new(-300.0 + r.f() * 600.0, -150.0 + r.f() * 300.0);
            let pts: Vec<Point> = (0..=12).map(|j| a.lerp(b, j as f64 / 12.0)).collect();
            commit_brush_line(&mut app, &brush_line(&pts, 5.0 + r.f() * 5.0));
        }
        let built = crate::planar_map::from_shape_builds();
        // 矩形で選んで、選び足して、動かして、確定する
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(-120.0, -80.0),
            Point::new(0.0, 40.0),
            false,
        );
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(-20.0, -60.0),
            Point::new(90.0, 60.0),
            true,
        );
        assert!(app
            .session
            .floating()
            .is_some_and(|_| !app.session.floating_view().pieces().is_empty()));
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(37.0, -21.0),
            },
        ));
        assert!(canvas_tool::clear_selection(&mut app));
        assert!(app.session.floating().is_none());
        // 矩形で選んで、動かさずに一部だけ外す（外した片は残りに戻る）
        select_ops::apply_marquee_selection(
            &mut app,
            Point::new(-250.0, -120.0),
            Point::new(250.0, 120.0),
            false,
        );
        let keep: Vec<usize> = app.session.selected_indices().into_iter().take(1).collect();
        assert!(crate::canvas_tool::commit_floating_except(
            app.tab_mut(),
            &keep
        ));
        assert!(canvas_tool::clear_selection(&mut app));
        assert_eq!(
            crate::planar_map::from_shape_builds(),
            built,
            "浮いた片の操作で、図形から地図を作らない"
        );
        let cel = &app.doc.root.layers[0].keyframes[0].cel;
        assert!(cel.maps_problem().is_none());
    }

    /// 計測の試し（ふだんは流さない）：図形の多い場面（`dense_scene_app`）で、矩形で選ぶ・選び足す・動かす・確定するの時間
    #[test]
    #[ignore]
    fn probe_floating_timing() {
        let mut app = dense_scene_app(7);
        let mut r = Rng(21);
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        let (mut cut, mut add, mut mv, mut commit) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for _ in 0..12 {
            let c = Point::new(-300.0 + r.f() * 600.0, -150.0 + r.f() * 300.0);
            let t = std::time::Instant::now();
            select_ops::apply_marquee_selection(&mut app, c, c + KVec2::new(90.0, 70.0), false);
            cut.push(ms(t));
            let t = std::time::Instant::now();
            select_ops::apply_marquee_selection(
                &mut app,
                c + KVec2::new(60.0, 40.0),
                c + KVec2::new(150.0, 110.0),
                true,
            );
            add.push(ms(t));
            let t = std::time::Instant::now();
            doc_actions::apply(
                &mut app,
                doc_actions::Action::NudgeSelection {
                    delta: KVec2::new(23.0, -17.0),
                },
            );
            mv.push(ms(t));
            let t = std::time::Instant::now();
            assert!(canvas_tool::clear_selection(&mut app));
            commit.push(ms(t));
        }
        let stat = |v: &[f64]| {
            (
                v.iter().sum::<f64>() / v.len() as f64,
                v.iter().cloned().fold(0.0, f64::max),
            )
        };
        for (name, v) in [
            ("矩形で選ぶ", &cut),
            ("選び足す", &add),
            ("動かす", &mv),
            ("確定", &commit),
        ] {
            let (a, m) = stat(v);
            eprintln!("PROBE {name}：平均 {a:.1}ms・最大 {m:.1}ms");
        }
    }

    /// 強さの試し（ふだんは流さない）：新しい中身の操作（矩形で選んでコピー → 貼り付け、Alt 複製、シンボル化、分解、
    /// グループ化と解除、回して縮めたシンボルの中へその場に貼る）を、種ごとに乱数で続ける。セル画に書くたびに地図を確かめ
    /// （テストの決まり）、図形から地図を作った回数が増えないことと、最後にどのセル画も決まりどおりかを見る
    #[test]
    #[ignore]
    fn probe_random_content_ops() {
        use crate::canvas_tool::doc_actions::Action;
        let seeds: Vec<u64> = std::env::var("PROBE_SEEDS")
            .map(|v| v.split(',').map(|x| x.parse().unwrap()).collect())
            .unwrap_or((1..=6).collect());
        let steps: usize = std::env::var("PROBE_STEPS")
            .map(|v| v.parse().unwrap())
            .unwrap_or(60);
        for seed in seeds {
            let mut app = Is2dApp::default();
            let mut r = Rng(seed);
            let colors = [Color32::RED, Color32::BLUE, Color32::GREEN, Color32::BLACK];
            for _ in 0..25 {
                app.tool.brush_color = colors[(r.f() * 4.0) as usize % 4];
                let a = Point::new(-300.0 + r.f() * 600.0, -180.0 + r.f() * 360.0);
                let pts: Vec<Point> = (0..=12)
                    .map(|j| a + KVec2::new(j as f64 * 9.0, 25.0 * (j as f64 * 0.5 + r.f()).sin()))
                    .collect();
                commit_brush_line(&mut app, &brush_line(&pts, 3.0 + r.f() * 9.0));
            }
            let built = crate::planar_map::from_shape_builds();
            let mut done = [0usize; 6];
            let mut t_max = [0f64; 6];
            for _ in 0..steps {
                let kind = (r.f() * 6.0) as usize % 6;
                let c = Point::new(-300.0 + r.f() * 500.0, -180.0 + r.f() * 300.0);
                // 重なったインスタンスをまとめて写すと、1回で倍に増える。絵が大きくなりすぎたら、増やす操作はしない
                if app.scene().len() > 3000 && kind != 2 {
                    continue;
                }
                let t = std::time::Instant::now();
                let ok = match kind {
                    0 => {
                        select_ops::apply_marquee_selection(
                            &mut app,
                            c,
                            c + KVec2::new(140.0, 110.0),
                            false,
                        );
                        let ok = doc_actions::copy_selected_shapes(&mut app)
                            && doc_actions::paste_clipboard(&mut app, r.f() < 0.5);
                        assert!(canvas_tool::clear_selection(&mut app));
                        ok
                    }
                    1 => {
                        select_ops::apply_marquee_selection(
                            &mut app,
                            c,
                            c + KVec2::new(120.0, 90.0),
                            false,
                        );
                        let ok = doc_actions::apply(
                            &mut app,
                            Action::FinishMoveSelectionDrag {
                                total_delta: KVec2::new(r.f() * 60.0 - 30.0, r.f() * 60.0 - 30.0),
                                alt_duplicate: true,
                            },
                        );
                        assert!(canvas_tool::clear_selection(&mut app));
                        ok
                    }
                    2 => {
                        select_ops::apply_marquee_selection(
                            &mut app,
                            c,
                            c + KVec2::new(160.0, 120.0),
                            false,
                        );
                        // インスタンスを含む所はシンボルにしない（入れ子が掛け算で増え、試しが絵の量で止まるため）
                        let ok = app.session.selection().instances.is_empty()
                            && doc_actions::convert_selection_to_symbol(&mut app).is_some();
                        if ok && r.f() < 0.7 {
                            let _ = doc_actions::rotate_selected_shapes(&mut app, r.f() - 0.5);
                            let _ = scale_selection(
                                &mut app,
                                0.5 + r.f(),
                                0.5 + r.f(),
                            );
                        }
                        assert!(canvas_tool::clear_selection(&mut app));
                        ok
                    }
                    3 => {
                        let inst = app.scene().iter().position(|s| s.symbol_instance.is_some());
                        let ok = inst.is_some_and(|i| {
                            app.set_selected_indices(vec![i]);
                            doc_actions::ungroup_selected_shapes(&mut app)
                        });
                        assert!(canvas_tool::clear_selection(&mut app));
                        ok
                    }
                    4 => {
                        select_ops::apply_marquee_selection(
                            &mut app,
                            c,
                            c + KVec2::new(150.0, 120.0),
                            false,
                        );
                        let ok = if r.f() < 0.5 {
                            doc_actions::group_selected_shapes(&mut app).is_some()
                        } else {
                            doc_actions::ungroup_selected_shapes(&mut app)
                        };
                        assert!(canvas_tool::clear_selection(&mut app));
                        ok
                    }
                    _ => {
                        // 写して、シンボルの中へその場に貼る（インスタンスの変換が違う所）
                        select_ops::apply_marquee_selection(
                            &mut app,
                            c,
                            c + KVec2::new(100.0, 80.0),
                            false,
                        );
                        let copied = doc_actions::copy_selected_shapes(&mut app);
                        assert!(canvas_tool::clear_selection(&mut app));
                        let binding = app.scene().iter().find_map(|s| s.symbol_instance);
                        let ok = copied
                            && binding.is_some_and(|b| {
                                symbol_ops::enter_symbol_timeline(&mut app, b.instance_gid)
                                    && doc_actions::paste_clipboard(&mut app, true)
                            });
                        assert!(canvas_tool::clear_selection(&mut app));
                        while !app.session.edit_path().is_empty() {
                            symbol_ops::exit_symbol_timeline(&mut app);
                        }
                        ok
                    }
                };
                let el = t.elapsed().as_secs_f64() * 1e3;
                if ok {
                    done[kind] += 1;
                }
                t_max[kind] = t_max[kind].max(el);
            }
            let broken = std::iter::once(&app.doc.root)
                .chain(symbols_in_order(app.doc.library()).map(|s| &s.timeline))
                .flat_map(|tl| tl.layers.iter().flat_map(|l| l.keyframes.iter()))
                .filter(|k| k.cel.maps_problem().is_some())
                .count();
            eprintln!(
                "PROBE 種{seed}：できた回 {done:?}・最大 {:?}ms｜図形から作った地図 {}｜決まりどおりでないセル画 {broken}｜シンボル {}",
                t_max.iter().map(|t| t.round() as i64).collect::<Vec<_>>(),
                crate::planar_map::from_shape_builds() - built,
                app.doc.library().len()
            );
        }
    }

    /// 計測の試し（ふだんは流さない）：新しい中身の時間（図形の多い場面：3 レイヤー・図形 1,060）。コピー、くり返しの
    /// 貼り付け（同じ中身を5回。Undo で戻しながら）と確定、Alt 複製と確定、シンボル化、シンボルの分解と確定。クリップボード
    /// の中身が元の地図と共有する塊も数える
    #[test]
    #[ignore]
    fn probe_new_content_timing() {
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        let mut app = dense_scene_app(7);
        let faces = app.scene().len();
        let act = |app: &mut Is2dApp, a: doc_actions::Action| doc_actions::apply(app, a);
        assert!(act(&mut app, doc_actions::Action::SelectAll));
        let t = std::time::Instant::now();
        assert!(doc_actions::copy_selected_shapes(&mut app));
        let t_copy = ms(t);
        let content = app.clipboard.content.clone().unwrap();
        let (mut tiles, mut shared) = (0, 0);
        for (lid, cel) in &content.layers {
            let layer = app.doc.root.layers.iter().find(|l| l.id == *lid).unwrap();
            let src = layer.keyframes[0].cel.maps()[0].stored();
            tiles += cel.maps()[0].stored().tiles();
            shared += cel.maps()[0].stored().shared_tiles(src);
        }
        act(&mut app, doc_actions::Action::Deselect);
        let mut pastes = Vec::new();
        for _ in 0..5 {
            let t = std::time::Instant::now();
            assert!(doc_actions::paste_clipboard(&mut app, false));
            pastes.push(ms(t));
            act(&mut app, doc_actions::Action::Undo);
        }
        assert!(doc_actions::paste_clipboard(&mut app, false));
        let t = std::time::Instant::now();
        act(&mut app, doc_actions::Action::Deselect);
        let t_paste_commit = ms(t);
        act(&mut app, doc_actions::Action::Undo);
        // Alt 複製（全部を選び、Alt を押して 6px 動かす）と確定
        let mut app = dense_scene_app(7);
        assert!(act(&mut app, doc_actions::Action::SelectAll));
        let t = std::time::Instant::now();
        assert!(act(
            &mut app,
            doc_actions::Action::FinishMoveSelectionDrag {
                total_delta: KVec2::new(6.0, 0.0),
                alt_duplicate: true,
            }
        ));
        let t_alt = ms(t);
        let t = std::time::Instant::now();
        act(&mut app, doc_actions::Action::Deselect);
        let t_alt_commit = ms(t);
        // シンボル化と分解（全部）、確定
        let mut app = dense_scene_app(7);
        assert!(act(&mut app, doc_actions::Action::SelectAll));
        let t = std::time::Instant::now();
        assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
        let t_sym = ms(t);
        let t = std::time::Instant::now();
        assert!(doc_actions::ungroup_selected_shapes(&mut app));
        let t_break = ms(t);
        let t = std::time::Instant::now();
        act(&mut app, doc_actions::Action::Deselect);
        let t_break_commit = ms(t);
        let valid = app
            .doc
            .root
            .layers
            .iter()
            .all(|l| l.keyframes.iter().all(|k| k.cel.maps_problem().is_none()));
        eprintln!(
            "PROBE 図形 {faces}：コピー {t_copy:.1}ms（塊 {tiles} 個のうち元と共有 {shared} 個）｜貼り付け 5回 {}ms・確定 {t_paste_commit:.1}ms｜Alt 複製 {t_alt:.1}ms・確定 {t_alt_commit:.1}ms｜シンボル化 {t_sym:.1}ms｜分解 {t_break:.1}ms・確定 {t_break_commit:.1}ms｜地図 {}",
            pastes
                .iter()
                .map(|p| format!("{p:.1}"))
                .collect::<Vec<_>>()
                .join("・"),
            if valid { "正常" } else { "壊れた" }
        );
    }

    /// 図形の多い場面（`dense_scene_app`）での、ブラシ・消しゴム1本の時間（30 本ずつ、平均と最大）
    #[test]
    #[ignore]
    fn probe_dense_brush_timing() {
        let mut app = dense_scene_app(7);
        let mut r = Rng(9);
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        for tool in [Tool::Brush, Tool::Eraser] {
            app.tool.tool = tool;
            let mut times = Vec::new();
            for _ in 0..30 {
                app.tool.brush_color = Color32::from_rgb((r.f() * 255.0) as u8, 80, 160);
                let a = Point::new(-350.0 + r.f() * 700.0, -200.0 + r.f() * 400.0);
                let pts: Vec<Point> = (0..=24)
                    .map(|j| a + KVec2::new(j as f64 * 4.0, 30.0 * (j as f64 * 0.3).sin()))
                    .collect();
                let outline = brush_line(&pts, 3.0 + r.f() * 6.0);
                let t = std::time::Instant::now();
                commit_brush_line(&mut app, &outline);
                times.push(ms(t));
            }
            let avg = times.iter().sum::<f64>() / times.len() as f64;
            let max = times.iter().copied().fold(0.0, f64::max);
            eprintln!("PROBE {tool:?} 30 本：平均 {avg:.2}ms・最大 {max:.2}ms");
        }
        app.tool.tool = Tool::Brush;
    }

    #[test]
    #[ignore]
    fn probe_dense_scene_selection_timing() {
        let rects = [
            (
                "ステージ全体",
                Point::new(-410.0, -235.0),
                Point::new(410.0, 235.0),
            ),
            ("左半分", Point::new(-410.0, -235.0), Point::new(0.0, 235.0)),
            (
                "中央 500×300",
                Point::new(-250.0, -150.0),
                Point::new(250.0, 150.0),
            ),
            (
                "200×150",
                Point::new(-100.0, -75.0),
                Point::new(100.0, 75.0),
            ),
        ];
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        let all_valid = |app: &Is2dApp| (0..3).all(|l| layer_map_valid(app, l));
        for (name, a, b) in rects {
            // 3回測って、いちばん短いもの
            let (mut t_sel, mut t_commit, mut valid) = (f64::MAX, f64::MAX, true);
            for _ in 0..3 {
                let mut app = dense_scene_app(7);
                let t = std::time::Instant::now();
                select_ops::apply_marquee_selection(&mut app, a, b, false);
                t_sel = t_sel.min(ms(t));
                assert!(doc_actions::apply(
                    &mut app,
                    doc_actions::Action::NudgeSelection {
                        delta: KVec2::new(25.0, 0.0)
                    }
                ));
                let t = std::time::Instant::now();
                assert!(canvas_tool::clear_selection(&mut app));
                t_commit = t_commit.min(ms(t));
                valid &= all_valid(&app);
            }
            eprintln!(
                "PROBE {name}：矩形選択 {t_sel:.1}ms｜25px 動かして離す {t_commit:.1}ms｜地図 {}",
                if valid { "正常" } else { "壊れた" }
            );
        }
        // 全部を Alt 複製して 6px ずらして確定
        let mut app = dense_scene_app(7);
        select_ops::apply_marquee_selection(&mut app, rects[0].1, rects[0].2, false);
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::FinishMoveSelectionDrag {
                total_delta: KVec2::new(6.0, 0.0),
                alt_duplicate: true
            }
        ));
        let t = std::time::Instant::now();
        assert!(canvas_tool::clear_selection(&mut app));
        eprintln!(
            "PROBE 全部を Alt 複製して確定 {:.1}ms｜地図 {}",
            ms(t),
            if all_valid(&app) {
                "正常"
            } else {
                "壊れた"
            }
        );
        // 全部をコピーして貼り付け、16px ずらして確定
        let mut app = dense_scene_app(7);
        select_ops::apply_marquee_selection(&mut app, rects[0].1, rects[0].2, false);
        assert!(doc_actions::copy_selected_shapes(&mut app));
        assert!(canvas_tool::clear_selection(&mut app));
        assert!(doc_actions::paste_clipboard(&mut app, true));
        assert!(doc_actions::apply(
            &mut app,
            doc_actions::Action::NudgeSelection {
                delta: KVec2::new(16.0, 16.0)
            }
        ));
        let t = std::time::Instant::now();
        assert!(canvas_tool::clear_selection(&mut app));
        eprintln!(
            "PROBE 全部をコピーして貼り付け、確定 {:.1}ms｜地図 {}",
            ms(t),
            if all_valid(&app) {
                "正常"
            } else {
                "壊れた"
            }
        );
    }

    /// 計測の試し（ふだんは流さない）：輪郭の変形のドラッグ1回分（`reshape_result`。離したときの確定も同じ計算）の時間。
    /// 図形の多い場面の3レイヤーで、辺の真ん中の引っぱりと頂点の移動を、5・20・60px ずつ 50回ずつ
    #[test]
    #[ignore]
    fn probe_reshape_timing() {
        use crate::planar_map::{reshape_result, LayerBase, ReshapeEdit};
        let app = dense_scene_app(7);
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        let stats = |v: &[f64]| {
            let mut s = v.to_vec();
            s.sort_by(f64::total_cmp);
            (
                s.iter().sum::<f64>() / s.len() as f64,
                s[s.len() * 9 / 10],
                s[s.len() - 1],
            )
        };
        for layer in 0..3 {
            let shapes: Vec<FillShape> = app.doc.root.layers[layer].keyframes[0]
                .cel
                .shapes()
                .iter()
                .filter(|s| s.is_raw_at_scope(&[]))
                .map(|s| (**s).clone())
                .collect();
            let map = PlanarMap::from_faces(shapes.iter());
            assert!(map.is_valid(), "レイヤー {layer} の地図");
            let base = LayerBase::new(&map);
            let layer_id = shapes[0].layer_id;
            let mut r = Rng(layer as u64 + 1);
            for dist in [5.0, 20.0, 60.0] {
                let (mut pull, mut moved, mut commit) = (Vec::new(), Vec::new(), Vec::new());
                for i in 0..100 {
                    let ang = r.f() * std::f64::consts::TAU;
                    let d = KVec2::new(ang.cos(), ang.sin()) * dist;
                    let edit = if i % 2 == 0 {
                        let e = (r.f() * map.edges.len() as f64) as usize % map.edges.len();
                        let q = map.quad(e);
                        ReshapeEdit::PullEdge {
                            orig_seg: q,
                            grab_t: 0.5,
                            target: q.eval(0.5) + d,
                            follow: true,
                        }
                    } else {
                        let v = (r.f() * map.nodes.len() as f64) as usize % map.nodes.len();
                        ReshapeEdit::MoveVertex {
                            orig_vertex: map.nodes[v],
                            target: map.nodes[v] + d,
                        }
                    };
                    let t = std::time::Instant::now();
                    let out = reshape_result(&base, edit, layer_id, &[], Affine::IDENTITY);
                    std::hint::black_box(out);
                    if i % 2 == 0 {
                        pull.push(ms(t));
                    } else {
                        moved.push(ms(t));
                    }
                    // 離したときの確定（1本の道：`reshape_change` → `edit_local`）
                    let t = std::time::Instant::now();
                    let out = base.reshape_change(edit).map(|c| base.edit_local(&c, &[]));
                    std::hint::black_box(out);
                    commit.push(ms(t));
                }
                let (pa, p9, pm) = stats(&pull);
                let (ma, m9, mm) = stats(&moved);
                let (ca, c9, cm) = stats(&commit);
                eprintln!(
                    "PROBE レイヤー {layer}（辺 {}）{dist}px：辺の引っぱり 平均 {pa:.2}・9割 {p9:.2}・最大 {pm:.2}ms｜頂点の移動 平均 {ma:.2}・9割 {m9:.2}・最大 {mm:.2}ms｜確定 平均 {ca:.2}・9割 {c9:.2}・最大 {cm:.2}ms",
                    map.edges.len()
                );
            }
        }
    }

    /// 計測の試し（ふだんは流さない）：レイヤーの地図が正しいかを調べる時間
    #[test]
    #[ignore]
    fn probe_is_valid_cost() {
        for seed in [7u64, 8] {
            let app = dense_scene_app(seed);
            for l in 0..3 {
                // セル画の地図（しまった地図から作る写し）。面の図形は付けない
                for m in app.doc.root.layers[l].keyframes[0].cel.maps().iter() {
                    let map = m.dense();
                    // 計る機械の揺れを除くため、15回のうちいちばん短い時間
                    let least = |f: &dyn Fn()| {
                        (0..15)
                            .map(|_| {
                                let t = std::time::Instant::now();
                                f();
                                t.elapsed().as_secs_f64() * 1e3
                            })
                            .fold(f64::INFINITY, f64::min)
                    };
                    let ok = map.is_valid();
                    let n = map.faces().0.len();
                    let valid_ms = least(&|| {
                        std::hint::black_box(map.is_valid());
                    });
                    let faces_ms = least(&|| {
                        std::hint::black_box(map.faces());
                    });
                    println!(
                        "PROBE 種{seed} レイヤー{l}：辺 {}・面 {n}・正しい {ok}・判定 {valid_ms:.2}ms・面の取り出し {faces_ms:.2}ms",
                        map.edges.len(),
                    );
                }
            }
        }
    }

    /// 計測の試し（ふだんは流さない）：トゥイーンのレイヤー6枚（各40本のシンボル、36コマごとに動く）で、
    /// オニオンスキン前後10コマを付けて24コマずつスクラブしたときの、オニオンスキンを作る時間。
    /// シンボルの中身のメモを使う（今の作り）と、毎回捨てる（前の作り）を比べる
    #[test]
    #[ignore]
    fn probe_onion_skin_tween_scrub() {
        use doc_actions::Action;
        let mut app = Is2dApp::default();
        let mut rng = fastrand::Rng::with_seed(5);
        let frames = 288;
        for l in 0..6 {
            if l > 0 {
                doc_actions::timeline_add_layer(&mut app);
            }
            let layer_idx = app.doc.root.active_layer;
            let lid = app.doc.root.layers[layer_idx].id;
            doc_actions::switch_to_layer_and_frame(&mut app, layer_idx, 0);
            for _ in 0..40 {
                let c = Point::new(rng.f64() * 200.0 + 300.0, rng.f64() * 200.0 + 200.0);
                let pts: Vec<Point> = (0..12)
                    .map(|i| c + KVec2::new(i as f64 * 6.0, (i as f64 * 0.7).sin() * 20.0))
                    .collect();
                let color = Color32::from_rgb(rng.u8(..), rng.u8(..), rng.u8(..));
                run(
                    &mut app,
                    &[Action::PaintStroke {
                        points: pts,
                        radius: 3.0 + rng.f32() * 5.0,
                        color,
                    }],
                );
            }
            let idx: Vec<usize> = (0..app.scene().len())
                .filter(|&i| app.scene().get(i).unwrap().layer_id == lid)
                .collect();
            app.set_selected_indices(idx);
            assert!(doc_actions::convert_selection_to_symbol(&mut app).is_some());
            assert!(doc_actions::timeline_toggle_tween(&mut app));
            while app.doc.root.total_frames < frames {
                assert!(doc_actions::timeline_extend_frame(&mut app));
            }
            for k in 1..frames / 36 {
                doc_actions::switch_to_layer_and_frame(&mut app, layer_idx, k * 36);
                assert!(doc_actions::timeline_insert_keyframe(&mut app, true));
                let idx: Vec<usize> = (0..app.scene().len())
                    .filter(|&i| app.scene().get(i).unwrap().layer_id == lid)
                    .collect();
                app.set_selected_indices(idx);
                doc_actions::nudge_selected_shapes(
                    &mut app,
                    KVec2::new(rng.f64() * 60.0 - 30.0, rng.f64() * 60.0 - 30.0),
                );
                assert!(canvas_tool::clear_selection(&mut app));
            }
        }
        let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
        for (span, step) in [(2usize, 1usize), (5, 4), (10, 1), (10, 24)] {
            app.view.onion_prev_frames = span;
            app.view.onion_next_frames = span;
            for keep_memo in [false, true] {
                let mut times = Vec::new();
                app.clear_onion_skin_cache();
                *app.session.symbol_memo.lock() = Default::default();
                for f in (0..frames).step_by(step) {
                    doc_actions::switch_to_layer_and_frame(&mut app, 0, f);
                    if !keep_memo {
                        *app.session.symbol_memo.lock() = Default::default();
                    }
                    let t = std::time::Instant::now();
                    app.start_draw_pass();
                    let tl = app.active_timeline();
                    let (lo, hi) = (f.saturating_sub(span), (f + span).min(tl.total_frames - 1));
                    let n_layers = tl.layers.len();
                    for g in lo..=hi {
                        if g == f {
                            continue;
                        }
                        for layer_idx in 0..n_layers {
                            let _ = app.cached_onion_cel_screen_mesh(
                                layer_idx,
                                g,
                                1.0,
                                eframe::egui::Vec2::ZERO,
                                WHOLE_VIEW,
                                ([0, 0, 0], 80),
                            );
                        }
                    }
                    times.push(ms(t));
                }
                let avg = times.iter().sum::<f64>() / times.len() as f64;
                let max = times.iter().copied().fold(0.0, f64::max);
                println!(
                    "前後{span}コマ・{step}コマずつ・{}: 平均 {avg:.2}ms・最大 {max:.2}ms（{} 回）",
                    if keep_memo {
                        "メモを使う"
                    } else {
                        "1回ごとに捨てる"
                    },
                    times.len()
                );
            }
        }
    }
}

/// くねったブラシの線（40 点、点の間隔 3px）。始点は `x0..x0+w`・`y0..y0+h` の中
fn wiggly_stroke(
    rng: &mut map_overlay::Rng,
    (x0, y0, w, h): (f64, f64, f64, f64),
    radius: f64,
    color: Color32,
) -> Vec<FillShape> {
    let mut p = Point::new(x0 + rng.f() * w, y0 + rng.f() * h);
    let mut a = rng.f() * std::f64::consts::TAU;
    let mut pts = vec![p];
    for _ in 0..40 {
        a += (rng.f() - 0.5) * 0.8;
        p += kurbo::Vec2::new(a.cos(), a.sin()) * 3.0;
        pts.push(p);
    }
    let raw = brush_line_at_zoom(&pts, radius, 1.0);
    brush_line_shapes_at_zoom(&raw, color, 1.0)
}

/// 版 7 の保存用の型（版 8 で、キーフレームの音と音のライブラリを足す前のもの）。計測用の作品を今の版に直すためだけに
/// 使う（アプリは前の版を開かない）。中身は今の型と同じで、音の項目だけが無い
/// 版 8 の保存の形（塊の表に線の並びが無い）。版 9 との違いは塊の表だけなので、塊の表だけを版 8 の形で読み、線の無い塊の表にする
/// 版 6〜版 10 の保存用の型のうち、版 11 で変わった所（タイムラインごとにレイヤーの番号の数え札を持った）。版 8〜版 10 の
/// ファイルはこの型で読んでから、今の版にする（版 11 で、レイヤーの番号の数え札を作品に1つにした）
mod format_v10 {
    use crate::save_format::{ProjectDocument, SavedLayer, SavedSound, SavedSymbolDef, SavedTimeline};
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub(super) struct Timeline {
        pub(super) fps: u32,
        pub(super) total_frames: usize,
        pub(super) current_frame: usize,
        pub(super) active_layer: usize,
        pub(super) next_layer_number: usize,
        pub(super) next_layer_id: u64,
        pub(super) layers: Vec<SavedLayer>,
    }

    #[derive(Deserialize, Serialize)]
    pub(super) struct Symbol {
        pub(super) id: u64,
        pub(super) name: String,
        pub(super) timeline: Timeline,
    }

    #[derive(Deserialize, Serialize)]
    pub(super) struct Document<T> {
        pub(super) stage_size: [f32; 2],
        pub(super) timeline: Timeline,
        pub(super) library: Vec<Symbol>,
        pub(super) next_symbol_id: u64,
        pub(super) next_group_id: u64,
        pub(super) recovery_source: Option<std::path::PathBuf>,
        pub(super) map_tiles: T,
        pub(super) sounds: Vec<SavedSound>,
        pub(super) next_sound_id: u64,
    }

    impl Timeline {
        pub(super) fn current(self) -> SavedTimeline {
            SavedTimeline {
                fps: self.fps,
                total_frames: self.total_frames,
                current_frame: self.current_frame,
                active_layer: self.active_layer,
                next_layer_number: self.next_layer_number,
                layers: self.layers,
            }
        }
    }

    impl<T> Document<T> {
        /// 塊の表を `f` で今の版の形にしたもの
        pub(super) fn with_tiles<U>(self, f: impl FnOnce(T) -> U) -> Document<U> {
            Document {
                stage_size: self.stage_size,
                timeline: self.timeline,
                library: self.library,
                next_symbol_id: self.next_symbol_id,
                next_group_id: self.next_group_id,
                recovery_source: self.recovery_source,
                map_tiles: f(self.map_tiles),
                sounds: self.sounds,
                next_sound_id: self.next_sound_id,
            }
        }

        /// 今の版の保存用の型（作品のレイヤーの番号の数え札は、どのタイムラインの数え札よりも後。読み込みが、どのレイヤーの
        /// 番号よりも後にそろえる）
        pub(super) fn current(self) -> ProjectDocument<T> {
            let next_layer_id = std::iter::once(&self.timeline)
                .chain(self.library.iter().map(|s| &s.timeline))
                .map(|t| t.next_layer_id)
                .max()
                .unwrap_or(1);
            ProjectDocument {
                stage_size: self.stage_size,
                timeline: self.timeline.current(),
                library: self
                    .library
                    .into_iter()
                    .map(|s| SavedSymbolDef { id: s.id, name: s.name, timeline: s.timeline.current() })
                    .collect(),
                next_symbol_id: self.next_symbol_id,
                next_group_id: self.next_group_id,
                recovery_source: self.recovery_source,
                map_tiles: self.map_tiles,
                sounds: self.sounds,
                next_sound_id: self.next_sound_id,
                next_layer_id,
            }
        }
    }
}

mod format_v9 {
    use crate::save_format::ProjectDocument;
    use crate::stored_map::{SavedStroke, SavedTile};
    use crate::canvas_tool::tests::SavedTiles;
    use serde::Deserialize;

    #[derive(Default, Deserialize)]
    pub(super) struct Tiles {
        colors: Vec<[u8; 4]>,
        tiles: Vec<SavedTile>,
        strokes: Vec<Stroke>,
    }

    /// 版 9 の線（番号が無い）
    #[derive(Deserialize)]
    struct Stroke {
        tile: u32,
        edge: u32,
        color: u32,
        width: u8,
    }

    /// 版 9 の文書を、今の版の保存用の型にする（線に、文書の中で重ならない新しい印を振る：側の図形の印のいちばん大きいものの
    /// 次から）
    pub(super) fn current(v9: super::format_v10::Document<Tiles>) -> ProjectDocument<SavedTiles> {
        let last = v9
            .map_tiles
            .tiles
            .iter()
            .flat_map(|t| t.edges.iter().map(|e| e.left_src.max(e.right_src)))
            .max()
            .unwrap_or(0);
        let strokes = v9
            .map_tiles
            .strokes
            .iter()
            .enumerate()
            .map(|(k, s)| SavedStroke {
                tile: s.tile,
                edge: s.edge,
                color: s.color,
                width: s.width,
                id: last + k as u64 + 1,
            })
            .collect();
        v9.with_tiles(|t| SavedTiles { colors: t.colors, tiles: t.tiles, strokes }).current()
    }
}

mod format_v8 {
    use crate::save_format::ProjectDocument;
    use crate::stored_map::{SavedEdge, SavedTile};
    use crate::canvas_tool::tests::SavedTiles;
    use serde::Deserialize;

    #[derive(Default, Deserialize)]
    pub(super) struct Tiles {
        colors: Vec<[u8; 4]>,
        tiles: Vec<Tile>,
    }

    #[derive(Deserialize)]
    struct Tile {
        nodes: Vec<[f64; 2]>,
        edges: Vec<SavedEdge>,
    }

    /// 版 8 の文書を、今の版の保存用の型にする（辺はみな線なし）
    pub(super) fn current(v8: super::format_v10::Document<Tiles>) -> ProjectDocument<SavedTiles> {
        v8.with_tiles(tiles).current()
    }

    /// 版 8（と版 7）の塊の表を、今の版の塊の表にする（辺はみな線なし）
    pub(super) fn tiles(t: Tiles) -> SavedTiles {
        let tiles = t
            .tiles
            .into_iter()
            .map(|t| SavedTile {
                nodes: t.nodes,
                edges: t.edges,
            })
            .collect();
        SavedTiles {
            colors: t.colors,
            tiles,
            strokes: Vec::new(),
        }
    }
}

mod format_v7 {
    use crate::save_format::{ProjectDocument, SavedKeyframe, SavedLayer, SavedSymbolInstance};
    use crate::canvas_tool::tests::SavedTiles;
    use crate::timeline::SavedScopeMap;
    use crate::timeline::TweenRotate;
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct Document {
        stage_size: [f32; 2],
        timeline: Timeline,
        library: Vec<SymbolDef>,
        next_symbol_id: u64,
        next_group_id: u64,
        recovery_source: Option<std::path::PathBuf>,
        map_tiles: super::format_v8::Tiles,
    }

    #[derive(Deserialize)]
    struct Timeline {
        fps: u32,
        total_frames: usize,
        current_frame: usize,
        active_layer: usize,
        next_layer_number: usize,
        next_layer_id: u64,
        layers: Vec<Layer>,
    }

    #[derive(Deserialize)]
    struct Layer {
        id: u64,
        name: String,
        visible: bool,
        locked: bool,
        frame_count: usize,
        keyframes: Vec<Keyframe>,
    }

    #[derive(Deserialize)]
    struct Keyframe {
        start_frame: usize,
        tween: bool,
        tween_rotate: TweenRotate,
        maps: Vec<SavedScopeMap>,
        instances: Vec<SavedSymbolInstance>,
    }

    #[derive(Deserialize)]
    struct SymbolDef {
        id: u64,
        name: String,
        timeline: Timeline,
    }

    impl Timeline {
        fn current(self) -> super::format_v10::Timeline {
            super::format_v10::Timeline {
                fps: self.fps,
                total_frames: self.total_frames,
                current_frame: self.current_frame,
                active_layer: self.active_layer,
                next_layer_number: self.next_layer_number,
                next_layer_id: self.next_layer_id,
                layers: self
                    .layers
                    .into_iter()
                    .map(|l| SavedLayer {
                        id: l.id,
                        name: l.name,
                        visible: l.visible,
                        locked: l.locked,
                        frame_count: l.frame_count,
                        keyframes: l
                            .keyframes
                            .into_iter()
                            .map(|k| SavedKeyframe {
                                start_frame: k.start_frame,
                                tween: k.tween,
                                tween_rotate: k.tween_rotate,
                                maps: k.maps,
                                instances: k.instances,
                                sound: None,
                            })
                            .collect(),
                    })
                    .collect(),
            }
        }
    }

    impl Document {
        /// 今の版の保存用の型（音は無い）
        pub(super) fn current(self) -> ProjectDocument<SavedTiles> {
            super::format_v10::Document {
                stage_size: self.stage_size,
                timeline: self.timeline.current(),
                library: self
                    .library
                    .into_iter()
                    .map(|s| super::format_v10::Symbol { id: s.id, name: s.name, timeline: s.timeline.current() })
                    .collect(),
                next_symbol_id: self.next_symbol_id,
                next_group_id: self.next_group_id,
                recovery_source: self.recovery_source,
                map_tiles: super::format_v8::tiles(self.map_tiles),
                sounds: Vec::new(),
                next_sound_id: 1,
            }
            .current()
        }
    }
}

/// 前の版（版 5 の JSON・版 6）の保存用の型。計測用の作品を今の版に直すためだけに使う（アプリは前の版を開かない）。地図は
/// 頂点と辺で書き、面の図形の番号は書かない
mod old_format {
    use crate::save_format::SavedSymbolInstance;
    use crate::timeline::TweenRotate;
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct Map {
        #[serde(default)]
        pub(super) group_path: Vec<u64>,
        pub(super) colors: Vec<[u8; 4]>,
        pub(super) nodes: Vec<f64>,
        pub(super) edges: Vec<(usize, usize, f64, f64, usize, usize)>,
    }

    #[derive(Deserialize)]
    pub(super) struct Keyframe {
        pub(super) start_frame: usize,
        #[serde(default)]
        pub(super) tween: bool,
        #[serde(default)]
        pub(super) tween_rotate: TweenRotate,
        #[serde(default)]
        pub(super) maps: Vec<Map>,
        #[serde(default)]
        pub(super) instances: Vec<SavedSymbolInstance>,
    }

    fn visible() -> bool {
        true
    }

    #[derive(Deserialize)]
    pub(super) struct Layer {
        pub(super) id: u64,
        pub(super) name: String,
        #[serde(default = "visible")]
        pub(super) visible: bool,
        #[serde(default)]
        pub(super) locked: bool,
        pub(super) frame_count: usize,
        #[serde(default)]
        pub(super) keyframes: Vec<Keyframe>,
    }

    #[derive(Deserialize)]
    pub(super) struct Timeline {
        pub(super) fps: u32,
        pub(super) total_frames: usize,
        pub(super) current_frame: usize,
        pub(super) active_layer: usize,
        #[serde(default)]
        pub(super) next_layer_number: usize,
        #[serde(default)]
        pub(super) next_layer_id: u64,
        pub(super) layers: Vec<Layer>,
    }

    #[derive(Deserialize)]
    pub(super) struct Symbol {
        pub(super) id: u64,
        pub(super) name: String,
        pub(super) timeline: Timeline,
    }

    #[derive(Deserialize)]
    pub(super) struct Document {
        pub(super) stage_size: [f32; 2],
        pub(super) timeline: Timeline,
        #[serde(default)]
        pub(super) library: Vec<Symbol>,
        #[serde(default)]
        pub(super) next_symbol_id: u64,
        #[serde(default)]
        pub(super) next_group_id: u64,
        #[serde(default)]
        pub(super) recovery_source: Option<std::path::PathBuf>,
    }
}

/// 計測の作品のファイルの中身 `bytes` が前の版（版 5 の JSON・版 6）なら、今の版の保存用の型に直したものと、前の版の番号
/// （計測用の作品を作り直さずに使うため）。今の版のファイルなら `None`
fn old_format_document(bytes: &[u8]) -> Option<(SavedProject, u32)> {
    let (old, version): (old_format::Document, u32) = if bytes.first() == Some(&b'{') {
        assert!(
            bytes.starts_with(br#"{"version":5,"#),
            "直せるのは、版 5 の JSON と版 6〜版 11 のファイルだけ"
        );
        (
            serde_json::from_slice(bytes).expect("版 5 の JSON として読めること"),
            5,
        )
    } else if bytes.len() >= 20 && &bytes[..4] == b"IS2D" && bytes[4..8] == 11u32.to_le_bytes() {
        // 版 11：頭＋中身1つの入れ物（保存の型は今と同じ）
        let v11: SavedProject = format_v11::read(bytes).expect("版 11 の作品ファイルとして読めること");
        return Some((v11, 11));
    } else if bytes.len() >= 20 && &bytes[..4] == b"IS2D" && bytes[4..8] == 10u32.to_le_bytes() {
        let v10: format_v10::Document<SavedTiles> =
            postcard::from_bytes(&bytes[20..]).expect("版 10 の中身として読めること");
        return Some((v10.current(), 10));
    } else if bytes.len() >= 20 && &bytes[..4] == b"IS2D" && bytes[4..8] == 9u32.to_le_bytes() {
        let v9: format_v10::Document<format_v9::Tiles> =
            postcard::from_bytes(&bytes[20..]).expect("版 9 の中身として読めること");
        return Some((format_v9::current(v9), 9));
    } else if bytes.len() >= 20 && &bytes[..4] == b"IS2D" && bytes[4..8] == 8u32.to_le_bytes() {
        let v8: format_v10::Document<format_v8::Tiles> =
            postcard::from_bytes(&bytes[20..]).expect("版 8 の中身として読めること");
        return Some((format_v8::current(v8), 8));
    } else if bytes.len() >= 20 && &bytes[..4] == b"IS2D" && bytes[4..8] == 7u32.to_le_bytes() {
        let v7: format_v7::Document =
            postcard::from_bytes(&bytes[20..]).expect("版 7 の中身として読めること");
        return Some((v7.current(), 7));
    } else if bytes.len() >= 20 && &bytes[..4] == b"IS2D" && bytes[4..8] == 6u32.to_le_bytes() {
        (
            postcard::from_bytes(&bytes[20..]).expect("版 6 の中身として読めること"),
            6,
        )
    } else {
        return None;
    };
    let doc = current_document_of(old).into_document().expect("作品にできること");
    Some((project_from_bytes(&saved_file_bytes(&doc)).expect("読み戻せること"), version))
}

/// 前の版の保存用の型 `old` を、今の版の保存用の型にする。地図は確かめて面に1つずつ別の番号を記し（`uniquely_labeled`）、
/// 同じレイヤーの直前のキーフレームの同じ階層の地図と塊を共有させる（アプリで写したのと同じ）。壊れた地図は読んだまま
fn current_document_of(old: old_format::Document) -> save_format::ProjectDocument {
    use crate::stored_map::StoredMap;
    let stored_of = |m: &old_format::Map, prev: Option<&StoredMap>| -> StoredMap {
        let color = |i: usize| {
            (i > 0).then(|| {
                let c = m.colors[i - 1];
                Color32::from_rgba_premultiplied(c[0], c[1], c[2], c[3])
            })
        };
        let map = crate::planar_map::PlanarMap {
            nodes: m.nodes.chunks(2).map(|p| Point::new(p[0], p[1])).collect(),
            edges: m
                .edges
                .iter()
                .map(|&(a, b, cx, cy, l, r)| crate::planar_map::MapEdge {
                    a,
                    b,
                    ctrl: Point::new(cx, cy),
                    left: color(l),
                    right: color(r),
                    left_src: None,
                    right_src: None,
                    stroke: None,
                    line_id: None,
                })
                .collect(),
        };
        match map.clone().checked() {
            Some(c) => StoredMap::from_dense(c.uniquely_labeled().map(), prev),
            None => StoredMap::from_dense(&map, None),
        }
    };
    let timeline = |tl: old_format::Timeline| format_v10::Timeline {
        fps: tl.fps,
        total_frames: tl.total_frames,
        current_frame: tl.current_frame,
        active_layer: tl.active_layer,
        next_layer_number: tl.next_layer_number,
        next_layer_id: tl.next_layer_id,
        layers: tl
            .layers
            .into_iter()
            .map(|l| {
                let mut prev: std::collections::HashMap<Vec<u64>, StoredMap> =
                    std::collections::HashMap::new();
                save_format::SavedLayer {
                    id: l.id,
                    name: l.name,
                    visible: l.visible,
                    locked: l.locked,
                    frame_count: l.frame_count,
                    keyframes: l
                        .keyframes
                        .into_iter()
                        .map(|k| save_format::SavedKeyframe {
                            start_frame: k.start_frame,
                            tween: k.tween,
                            tween_rotate: k.tween_rotate,
                            maps: k
                                .maps
                                .iter()
                                .map(|m| {
                                    let stored = stored_of(m, prev.get(&m.group_path));
                                    prev.insert(m.group_path.clone(), stored.clone());
                                    timeline::SavedScopeMap {
                                        group_path: m.group_path.clone(),
                                        tiles: Vec::new(),
                                        stored: Some(stored),
                                    }
                                })
                                .collect(),
                            instances: k.instances,
                            sound: None,
                        })
                        .collect(),
                }
            })
            .collect(),
    };
    let mut pd: save_format::ProjectDocument = format_v10::Document {
        stage_size: old.stage_size,
        timeline: timeline(old.timeline),
        library: old
            .library
            .into_iter()
            .map(|s| format_v10::Symbol { id: s.id, name: s.name, timeline: timeline(s.timeline) })
            .collect(),
        next_symbol_id: old.next_symbol_id,
        next_group_id: old.next_group_id,
        recovery_source: old.recovery_source,
        map_tiles: Default::default(),
        sounds: Vec::new(),
        next_sound_id: 1,
    }
    .current();
    write_map_tiles(&mut pd);
    pd
}

/// 地図の確かめを使うときに行う（設計：claude/地図の確かめを使うときに行う設計_開く時間_0fe3fcb.md）ためのテスト用の作品：
/// 1 枚のレイヤーに、それぞれ違う絵のキーフレームを `n` 個（コマ 1, 3, 5, …）。保存したファイルのパス
fn save_many_keyframes(name: &str, n: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("is2d_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("many.is2d");
    let mut app = Is2dApp::default();
    for k in 0..n {
        if k > 0 {
            app.tab_mut().with_active_timeline_mut(|tl, _| {
                assert!(insert_keyframe_at(tl, 0, 2 * k, false, 24));
                tl.sync_total_frames();
            });
        }
        let y = 100.0 + 30.0 * k as f64;
        put_cel_shapes(
            &mut app,
            0,
            2 * k,
            make_stroke(
                Point::new(100.0, y),
                Point::new(300.0, y + 40.0),
                8.0,
                Color32::from_rgb(200, 40, 40),
            ),
        );
    }
    crate::project_io::save_project_to_path(&mut app, &path).unwrap();
    path
}

/// 浮いた片の操作を多めにした、決まった種のでたらめな操作の流れ（種 0x9001〜0x9006・400 手ずつ。下絵は、2枚のレイヤーに
/// 重なる塗りと線）。1手ごとに `step(種, 手, 操作の種類, 作品, 操作)` を呼ぶ（操作をするのは `step`。効いたかを返す）
fn floating_random_run(mut step_fn: impl FnMut(u64, usize, &str, &mut Is2dApp, crate::canvas_tool::doc_actions::Action) -> bool) {
    use crate::canvas_tool::doc_actions::{apply, Action};
    let names = action_kind_names();
    let floating = [
        "SelectMarquee", "FinishMoveSelectionDrag", "RetainSelection", "Deselect", "SelectAll", "DeleteSelection",
        "NudgeSelection", "FlipSelection", "RotateSelection", "Copy", "Paste", "Group", "Ungroup", "PaintStroke",
        "BucketFill", "InkBottle", "PencilStroke",
    ];
    let (mut effective, mut floated) = (0usize, 0usize);
    for seed in 0x9001u64..=0x9006 {
        let mut rng = OpRng(seed);
        let mut app = Is2dApp::default();
        let c = crate::canvas_tool::View::DEFAULT_STAGE_CENTER;
        for (i, col) in [Color32::RED, Color32::BLUE, Color32::GREEN].into_iter().enumerate() {
            let y = c.y - 80.0 + i as f64 * 60.0;
            assert!(apply(
                &mut app,
                Action::PaintStroke { points: vec![Point::new(c.x - 120.0, y), Point::new(c.x + 120.0, y + 40.0)], radius: 35.0, color: col },
            ));
        }
        assert!(apply(&mut app, Action::AddLayer));
        assert!(apply(
            &mut app,
            Action::PencilStroke {
                points: (0..20).map(|k| Point::new(c.x - 120.0 + k as f64 * 12.0, c.y + (k % 3) as f64 * 15.0)).collect(),
                stroke: crate::planar_map::EdgeStroke { color: Color32::BLACK, width: 3 },
            },
        ));
        for step in 0..400 {
            // 選んでいなければ、まず選ぶ（浮いた片を作る）ことを多めに
            let kind = if app.session.selection().is_empty() && rng.range(10) < 6 {
                ["SelectMarquee", "SelectAll"][rng.range(2)].to_owned()
            } else if rng.range(10) < 6 {
                floating[rng.range(floating.len())].to_owned()
            } else {
                names[rng.range(names.len())].clone()
            };
            let action = random_action(&kind, &mut rng, &app);
            let ok = step_fn(seed, step, &kind, &mut app, action);
            effective += usize::from(ok);
            // 浮いた片を動かす・変える操作が効いた手（次に確定される）
            let moves = ["FinishMoveSelectionDrag", "NudgeSelection", "FlipSelection", "RotateSelection", "DeleteSelection", "Paste", "RetainSelection"];
            floated += usize::from(ok && moves.contains(&kind.as_str()));
        }
    }
    assert!(effective >= 200 && floated >= 100, "浮いた片の操作を試せていない：効いた {effective}・動かした {floated}");
}

/// 今みな確定したときの絵（`committed_document_copy`）の要約：レイヤー・キーフレーム・階層・色・線かごとの塗った所（面積と
/// 外枠）と、インスタンスの数。図形の番号は数え札で変わるので入れない
fn committed_picture(app: &Is2dApp) -> Vec<String> {
    let Some(doc) = app.tab().committed_document_copy() else {
        return vec!["置けない".to_owned()];
    };
    let mut out = Vec::new();
    for (li, l) in doc.root.layers.iter().enumerate() {
        for (ki, kf) in l.keyframes.iter().enumerate() {
            for m in kf.cel.maps().iter() {
                let mut painted: std::collections::BTreeMap<String, (f64, [f64; 4])> = std::collections::BTreeMap::new();
                for f in m.base().drawn_shapes(&m.group_path) {
                    let key = format!("{li}/{ki}/{:?}/{:?}/{}", m.group_path, f.color().to_array(), f.line.is_some())
                        .replace(' ', "");
                    let area = f.exterior().signed_area().abs() - f.holes().iter().map(|h| h.signed_area().abs()).sum::<f64>();
                    let b = f.bounding_box();
                    let e = painted.entry(key).or_insert((0.0, [f64::MAX, f64::MAX, f64::MIN, f64::MIN]));
                    e.0 += area;
                    e.1 = [e.1[0].min(b.x0), e.1[1].min(b.y0), e.1[2].max(b.x1), e.1[3].max(b.y1)];
                }
                for (k, (a, b)) in painted {
                    out.push(format!("{k} {a:.3} {:.3} {:.3} {:.3} {:.3}", b[0], b[1], b[2], b[3]));
                }
            }
            out.push(format!("{li}/{ki} 印 {}", kf.cel.instances().len()));
        }
    }
    out
}

/// 絵の要約の1行どうしが同じとみなせるか（面積は 1px²・外枠は 1/20px まで：切り口の曲線は地図の精度の中でずれうる）
fn picture_line_near(a: &str, b: &str) -> bool {
    let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    if pa.len() != pb.len() || pa.len() < 6 || pa[0] != pb[0] {
        return a == b;
    }
    let num = |v: &[&str]| -> Vec<f64> { v[1..].iter().map(|x| x.parse().unwrap()).collect() };
    let (na, nb) = (num(&pa), num(&pb));
    (na[0] - nb[0]).abs() <= 1.0 && na[1..].iter().zip(&nb[1..]).all(|(x, y)| (x - y).abs() <= 0.05)
}

/// テスト用：16bit の PCM の WAV のバイト（`channels` 個ずつ並べた `samples`）
fn wav_bytes(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// テスト用：タイムライン `tl` のレイヤー `layer` のコマ `frame` を伸ばす（F5 と同じ入口 `Timeline::extend_frame_on_targets`。
/// 全体のコマ数も合わせる。今のコマ・コマの選択は元に戻す）
fn extend_frame_at(tl: &mut timeline::Timeline, layer: usize, frame: usize) {
    let (cur, sel) = (tl.current_frame, tl.cell_selection.take());
    tl.current_frame = frame;
    tl.extend_frame_on_targets(&[layer]);
    (tl.current_frame, tl.cell_selection) = (cur, sel);
}

/// テスト用：タイムライン `tl` のレイヤー `layer` のコマ `frame` にキーフレームを入れる（F6・F7 と同じ入口
/// `Timeline::insert_keyframe_on_targets`。全体のコマ数も合わせる。今のコマ・コマの選択は元に戻す）。そのコマで始まる
/// キーフレームがもうあれば入れずに `false`
/// テスト用：タイムライン `tl` のレイヤー `layer` のコマ `first` から `count` コマを消す（Shift+F5 と同じ入口
/// `Timeline::remove_frame_on_targets`。2 コマ以上はコマの範囲を選んで消す。今のコマ・コマの選択は元に戻す）
fn remove_frames_at(tl: &mut timeline::Timeline, layer: usize, first: usize, count: usize, fps: u32) -> bool {
    if count == 0 {
        return false;
    }
    let (cur, sel) = (tl.current_frame, tl.cell_selection.take());
    tl.current_frame = first;
    tl.cell_selection = (count > 1)
        .then(|| timeline::TimelineCellSelection::from_corners(layer, first, layer, first + count - 1));
    let changed = tl.remove_frame_on_targets(&[layer], fps);
    (tl.current_frame, tl.cell_selection) = (cur.min(tl.total_frames.saturating_sub(1)), sel);
    changed
}

fn insert_keyframe_at(tl: &mut timeline::Timeline, layer: usize, frame: usize, copy_prev: bool, fps: u32) -> bool {
    if tl.layers[layer].keyframe_index_starting_at(frame).is_some() {
        return false;
    }
    let (cur, sel) = (tl.current_frame, tl.cell_selection.take());
    tl.current_frame = frame;
    tl.insert_keyframe_on_targets(&[layer], copy_prev, fps);
    (tl.current_frame, tl.cell_selection) = (cur, sel);
    tl.layers[layer].keyframe_index_starting_at(frame).is_some()
}

/// テスト用：ライブラリの番号 `id` のシンボルのタイムラインを、中身を書く入口（`Document::write_symbol_timeline`：版を
/// 上げる）で書く
fn write_symbol(doc: &mut symbol::Document, id: u64, f: impl FnOnce(&mut timeline::Timeline)) {
    assert!(doc.library().get(id).is_some(), "シンボル");
    doc.write_symbol_timeline(id, |tl, _| f(tl));
}

/// テスト用：0.1 秒の 1 つの音の WAV（44.1kHz）
fn short_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..4410).map(|i| ((i % 100) * 300 - 15000) as i16).collect();
    wav_bytes(44_100, 1, &samples)
}

fn import_sound(app: &mut Is2dApp, bytes: Vec<u8>) -> bool {
    doc_actions::apply(
        app,
        doc_actions::Action::ImportSound {
            name: "テスト.wav".to_owned(),
            bytes: bytes.into(),
        },
    )
}

/// テスト用：線を描いてシンボルにし、その中に足したレイヤーに音を読み込んでから、ルートで全選択してコピーしたアプリと、
/// そのシンボルの番号
fn copied_symbol_with_sound() -> (Is2dApp, u64) {
    use doc_actions::{apply, Action};
    let mut app = Is2dApp::default();
    assert!(apply(
        &mut app,
        Action::PaintStroke {
            points: vec![Point::new(50.0, 50.0), Point::new(120.0, 50.0)],
            radius: 8.0,
            color: Color32::RED,
        },
    ));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::ConvertToSymbol));
    let sym_id = app.doc.library().at(0).unwrap().id;
    assert!(apply(&mut app, Action::EnterSymbolFromLibrary { sym_id }));
    assert!(apply(&mut app, Action::AddLayer));
    assert!(import_sound(&mut app, short_wav()));
    assert!(apply(&mut app, Action::ExitGroupSteps { steps: 1 }));
    assert!(apply(&mut app, Action::SelectAll));
    assert!(apply(&mut app, Action::Copy));
    (app, sym_id)
}

/// テスト用：文書 `doc` を保存したバイトを開き直した文書（開けなければ `Err`）
fn reopened(doc: &symbol::Document) -> Result<symbol::Document, String> {
    opened_document(&saved_file_bytes(doc))
}

fn trim(doc: &mut symbol::Document, kf_start: usize, edge: crate::sound::SoundEdge, frame: usize) -> bool {
    let layer = &mut doc.root.layers[0];
    let k = layer.keyframe_index_starting_at(kf_start).unwrap();
    layer.trim_sound(k, edge, frame, &doc.sounds, doc.fps)
}

/// 線（1段目：塗りの輪郭に線を乗せる。設計：claude/線の設計_68a596e.md）
mod strokes {
    use super::*;
    use crate::planar_map::{EdgeStroke, LayerBase, MapInput, MapRule, PlanarMap};
    use crate::stored_map::StoredMap;
    use doc_actions::{apply, Action};
    use kurbo::ParamCurveArclen;

    const INK: EdgeStroke = EdgeStroke {
        color: Color32::BLACK,
        width: 4,
    };

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64, color: Color32) -> FillShape {
        let mut s = vector_engine::make_marquee_rect_shape(kurbo::Rect::new(x0, y0, x1, y1));
        s.recolor(|_| color);
        s
    }

    /// 赤い正方形（100〜300）で、輪郭の辺みなに線 `INK` を乗せた地図
    fn inked_square() -> PlanarMap {
        let mut m = PlanarMap::default()
            .overlay(
                MapInput::Shapes(&[rect(100.0, 100.0, 300.0, 300.0, Color32::RED)]),
                MapRule::Paint(Some(Color32::RED)),
            )
            .unwrap();
        for e in &mut m.edges {
            e.stroke = Some(INK);
        }
        assert!(m.is_valid());
        m
    }

    /// 線を乗せた辺の長さの合計
    fn stroked_length(m: &PlanarMap) -> f64 {
        (0..m.edges.len())
            .filter(|&e| m.edges[e].stroke.is_some())
            .map(|e| m.quad(e).arclen(1e-6))
            .sum()
    }

    /// 線を乗せた辺が、みな元の正方形の輪郭の上にある
    fn strokes_on_square(m: &PlanarMap) -> bool {
        let on = |p: Point| {
            let edge = |v: f64| (v - 100.0).abs() < 1e-6 || (v - 300.0).abs() < 1e-6;
            edge(p.x) || edge(p.y)
        };
        (0..m.edges.len())
            .filter(|&e| m.edges[e].stroke.is_some())
            .all(|e| on(m.quad(e).eval(0.5)))
    }

    #[test]
    fn brush_over_a_line_removes_it_and_cut_pieces_keep_it() {
        let m = inked_square();
        // ブラシ（青）が右の辺の 150〜250 を覆う：覆った所の線は消え、残りは交わりで切れても線を引き継ぐ
        let painted = m
            .overlay(
                MapInput::Shapes(&[rect(200.0, 150.0, 400.0, 250.0, Color32::BLUE)]),
                MapRule::Paint(Some(Color32::BLUE)),
            )
            .unwrap();
        assert!(painted.is_valid());
        assert!((stroked_length(&painted) - 700.0).abs() < 1e-3);
        assert!(strokes_on_square(&painted));
        // ブラシの輪郭（新しい境目）には線が無い
        assert!(painted
            .edges
            .iter()
            .filter(|e| e.left == Some(Color32::BLUE) || e.right == Some(Color32::BLUE))
            .filter(|e| e.left != Some(Color32::RED) && e.right != Some(Color32::RED))
            .all(|e| e.stroke.is_none()));
        // 矩形で切る：中の片は中の線を持ち、切り口には線が無い。外の残りは外の線を持つ
        let cut = vector_engine::make_marquee_rect_shape(kurbo::Rect::new(50.0, 50.0, 200.0, 200.0));
        let inside = m
            .overlay(MapInput::Shapes(std::slice::from_ref(&cut)), MapRule::KeepInside)
            .unwrap();
        let outside = m
            .overlay(MapInput::Shapes(std::slice::from_ref(&cut)), MapRule::Paint(None))
            .unwrap();
        assert!(inside.is_valid() && outside.is_valid());
        assert!((stroked_length(&inside) - 200.0).abs() < 1e-3);
        assert!((stroked_length(&outside) - 600.0).abs() < 1e-3);
        assert!(strokes_on_square(&inside) && strokes_on_square(&outside));
        // 消しゴム（何も無いにする）で覆った所の線も消える
        let erased = m
            .overlay(
                MapInput::Shapes(&[rect(200.0, 150.0, 400.0, 250.0, Color32::BLUE)]),
                MapRule::Paint(None),
            )
            .unwrap();
        assert!((stroked_length(&erased) - 700.0).abs() < 1e-3);
    }

    #[test]
    fn placing_a_piece_keeps_its_lines_and_covers_lines_under_it() {
        let under = inked_square();
        // 線の無い青い片を、正方形の右下にかぶせて置く：片の色のある所の下の線は消える
        let piece = PlanarMap::default()
            .overlay(
                MapInput::Shapes(&[rect(250.0, 250.0, 350.0, 350.0, Color32::BLUE)]),
                MapRule::Paint(Some(Color32::BLUE)),
            )
            .unwrap();
        let placed = under.overlay(MapInput::Map(&piece), MapRule::Overlay).unwrap();
        assert!(placed.is_valid());
        assert!((stroked_length(&placed) - 700.0).abs() < 1e-3);
        // 線のある片を、何も無い所に置く：片の線はそのまま入る
        let mut inked_piece = piece.clone();
        for e in &mut inked_piece.edges {
            e.stroke = Some(EdgeStroke {
                color: Color32::GREEN,
                width: 1,
            });
        }
        let far = inked_piece.transformed(Affine::translate((500.0, 0.0)));
        let both = under.overlay(MapInput::Map(&far), MapRule::Overlay).unwrap();
        let green: f64 = (0..both.edges.len())
            .filter(|&e| both.edges[e].stroke.is_some_and(|s| s.color == Color32::GREEN))
            .map(|e| both.quad(e).arclen(1e-6))
            .sum();
        assert!((green - 400.0).abs() < 1e-3);
        assert!((stroked_length(&both) - 1200.0).abs() < 1e-3);
    }

    #[test]
    fn two_edges_merge_only_when_their_lines_match() {
        let mut m = inked_square();
        // 上の辺（y = 100）だけ線を外す。上の辺の両端の角は、線の違う2本がつながるので消せない。下の2つの角は消せる
        for i in 0..m.edges.len() {
            let q = m.quad(i);
            if q.p0.y == 100.0 && q.p2.y == 100.0 {
                m.edges[i].stroke = None;
            }
        }
        let corner = |p: Point| m.nodes.iter().position(|&n| n == p).unwrap();
        assert!(m.with_anchor_deleted(corner(Point::new(100.0, 100.0))).is_none());
        assert!(m.with_anchor_deleted(corner(Point::new(300.0, 100.0))).is_none());
        assert!(m.with_anchor_deleted(corner(Point::new(300.0, 300.0))).is_some());
    }

    #[test]
    fn lines_are_their_own_shapes_drawn_after_the_fills() {
        let m = inked_square().checked().unwrap();
        let base = LayerBase::of_stored(&StoredMap::from_dense(m.uniquely_labeled().map(), None));
        // 正方形の輪郭は辺が4本：線の図形は辺1本ずつ（アンカーとアンカーの間）で4つ
        let lines = base.lines(&[]);
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|l| l.line == Some(INK) && crate::planar_map::is_line_id(l.shape_id)));
        // 上の辺の線：線の太さの半分だけ外と中に広がる
        let line = lines.iter().find(|l| l.contains_point(Point::new(200.0, 99.0))).unwrap();
        assert_eq!(line.color(), INK.color);
        assert!((line.bounding_box().y1 - 102.0).abs() < 0.05);
        // 4本を合わせた外形は、太さの半分だけ広がる。角は丸いつなぎ目：角のすぐ外は塗られ、四角い角の先は塗られない
        let all = lines.iter().map(|l| l.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
        assert!((all.x0 - 98.0).abs() < 0.05 && (all.x1 - 302.0).abs() < 0.05);
        assert!((all.y0 - 98.0).abs() < 0.05 && (all.y1 - 302.0).abs() < 0.05);
        assert!(lines.iter().any(|l| l.contains_point(Point::new(99.0, 99.0))));
        assert!(!lines.iter().any(|l| l.contains_point(Point::new(98.3, 98.3))));
        // 角では、2本のうち1本だけが扇で埋める（丸い端を2つ重ねない）
        assert_eq!(lines.iter().filter(|l| l.contains_point(Point::new(99.0, 99.0))).count(), 1);
        // 輪郭の表示とアンカーは、太らせた形の輪郭でなく、線の中心を通る曲線（地図の辺）で描く
        let wire: Vec<QuadBez> = lines
            .iter()
            .filter_map(|l| l.center_curve())
            .collect();
        assert_eq!(wire.len(), base.map().edges.len());
        assert!(wire.iter().all(|q| {
            let on = |p: Point| (p.x - 100.0).abs() < 1e-9 || (p.x - 300.0).abs() < 1e-9 || (p.y - 100.0).abs() < 1e-9 || (p.y - 300.0).abs() < 1e-9;
            on(q.p0) && on(q.p2)
        }));
        assert!(line.contains_point(Point::new(200.0, 99.0)));
        assert!(line.contains_point(Point::new(200.0, 101.5)));
        assert!(!line.contains_point(Point::new(200.0, 105.0)));
        // 面の図形は塗りだけ
        let faces = base.faces(&[]);
        let face = faces.iter().next().unwrap();
        assert!(face.line.is_none());
        let fb = face.bounding_box();
        assert!((fb.x0 - 100.0).abs() < 1e-9 && (fb.x1 - 300.0).abs() < 1e-9);
        // 描く順は、面の図形を全部描いたあとに線の図形
        let drawn = base.drawn_shapes(&[]);
        assert_eq!(drawn.len(), 5);
        assert!(drawn[0].line.is_none() && drawn[1..].iter().all(|s| s.line.is_some()));
        // 線の番号は形と線で決まる：同じ地図から作り直しても同じ番号
        let again = LayerBase::of_stored(&StoredMap::from_dense(base.map(), None));
        let ids = |l: &crate::planar_map::LineTable| l.iter().map(|s| s.shape_id).collect::<Vec<_>>();
        assert_eq!(ids(&again.lines(&[])), ids(&lines));
    }

    /// 作業中のレイヤーの今のコマの地図の辺
    fn cel_edges(app: &Is2dApp) -> Vec<crate::planar_map::MapEdge> {
        let tl = app.tab().active_timeline();
        let cel = tl.layers[tl.active_layer].cel_at(tl.current_frame).unwrap();
        cel.maps().iter().flat_map(|m| m.base().map().edges.clone()).collect()
    }

    #[test]
    fn ink_bottle_outlines_a_fill_changes_one_line_and_survives_save() {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![rect(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
        // 塗りの中をクリック：輪郭の辺みなに線が乗る
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 200.0), stroke: INK }));
        assert!(cel_edges(&app).iter().all(|e| e.stroke == Some(INK)));
        // 同じ線でもう一度：替わらない
        assert!(!apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 200.0), stroke: INK }));
        // 画面に線の図形がある
        assert!(app.scene().iter().any(|s| s.line == Some(INK)));
        // 保存して開き直しても、線はそのまま
        let reopened_doc = reopened(&app.doc).unwrap();
        let cel = reopened_doc.root.layers[0].cel_at(0).unwrap();
        let edges: Vec<_> = cel.maps().iter().flat_map(|m| m.base().map().edges.clone()).collect();
        assert!(!edges.is_empty() && edges.iter().all(|e| e.stroke == Some(INK)));
        // 線の上をクリック：その線の辺1本（上の辺）だけが新しい線になる
        let thin = EdgeStroke {
            color: Color32::GREEN,
            width: 1,
        };
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 101.0), stroke: thin }));
        let edges = cel_edges(&app);
        assert!(edges.iter().any(|e| e.stroke == Some(thin)));
        assert!(edges.iter().any(|e| e.stroke == Some(INK)));
        let m = cel_map(&app);
        for e in 0..m.edges.len() {
            let q = m.quad(e);
            let top = (q.p0.y - 100.0).abs() < 1e-9 && (q.p2.y - 100.0).abs() < 1e-9;
            assert_eq!(m.edges[e].stroke, Some(if top { thin } else { INK }));
        }
        // Undo で戻る
        assert!(apply(&mut app, Action::Undo));
        assert!(cel_edges(&app).iter().all(|e| e.stroke == Some(INK)));
        assert!(apply(&mut app, Action::Undo));
        assert!(cel_edges(&app).iter().all(|e| e.stroke.is_none()));
        // 何も無い所：乗せない
        assert!(!apply(&mut app, Action::InkBottle { pt: Point::new(600.0, 600.0), stroke: INK }));
    }

    #[test]
    fn ink_bottle_on_one_of_two_touching_fills_lines_their_shared_edge() {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                rect(100.0, 100.0, 200.0, 200.0, Color32::RED),
                rect(200.0, 100.0, 300.0, 200.0, Color32::BLUE),
            ],
        );
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(150.0, 150.0), stroke: INK }));
        let edges = cel_edges(&app);
        // 赤の輪郭（共有する境目を含む）に線が乗り、青だけの辺には乗らない
        for e in &edges {
            let red = e.left == Some(Color32::RED) || e.right == Some(Color32::RED);
            assert_eq!(e.stroke.is_some(), red);
        }
        // 赤の輪郭は辺が4本（共有する境目も1本）：線の図形は4つ
        let lines = app.scene().iter().filter(|s| s.line.is_some()).count();
        assert_eq!(lines, 4);
        // スポイトで線の上を取ると、線の色と太さを拾う
        app.tool.stroke = EdgeStroke {
            color: Color32::WHITE,
            width: 9,
        };
        crate::tool_paint::pick_color_at(&mut app, Point::new(150.0, 99.0));
        assert_eq!(app.tool.stroke, INK);
    }

    /// 作業中のレイヤーの今のコマの地図（無ければ空の地図）
    fn cel_map_or_empty(app: &Is2dApp) -> PlanarMap {
        let tl = app.tab().active_timeline();
        tl.layers[tl.active_layer]
            .cel_at(tl.current_frame)
            .and_then(|cel| cel.maps().first().map(|m| m.base().map().clone()))
            .unwrap_or_default()
    }

    /// 作業中のレイヤーの今のコマの地図
    fn cel_map(app: &Is2dApp) -> PlanarMap {
        let tl = app.tab().active_timeline();
        let cel = tl.layers[tl.active_layer].cel_at(tl.current_frame).unwrap();
        let base = cel.maps()[0].base();
        base.map().clone()
    }

    #[test]
    fn a_line_between_two_fills_stays_when_they_become_one_color() {
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![
                rect(100.0, 100.0, 200.0, 200.0, Color32::RED),
                rect(200.0, 100.0, 300.0, 200.0, Color32::BLUE),
            ],
        );
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(150.0, 150.0), stroke: INK }));
        // 青を赤で塗る：間の線は残り、線で分かれた赤い面が2つになる（Animate と同じ）
        assert!(apply(&mut app, Action::BucketFill { pt: Point::new(250.0, 150.0), color: Color32::RED }));
        let m = cel_map(&app);
        let shared: Vec<_> = m
            .edges
            .iter()
            .filter(|e| e.left == Some(Color32::RED) && e.right == Some(Color32::RED))
            .collect();
        assert!(!shared.is_empty() && shared.iter().all(|e| e.stroke == Some(INK)));
        let (faces, broken) = m.faces();
        assert_eq!((faces.len(), broken), (2, 0));
        assert!(faces.iter().all(|f| f.color == Color32::RED));
        // 片方を緑に塗る：もう片方は赤のまま
        assert!(apply(&mut app, Action::BucketFill { pt: Point::new(150.0, 150.0), color: Color32::GREEN }));
        let m = cel_map(&app);
        let mut colors: Vec<_> = m.faces().0.iter().map(|f| f.color.to_array()).collect();
        colors.sort();
        let mut expected = vec![Color32::RED.to_array(), Color32::GREEN.to_array()];
        expected.sort();
        assert_eq!(colors, expected);
        // 赤のブラシで、間の線の真ん中を塗る：塗った所の線だけが消え、2つの赤い面の間が塗りでつながる
        assert!(apply(&mut app, Action::BucketFill { pt: Point::new(150.0, 150.0), color: Color32::RED }));
        apply(&mut app, Action::SetTool { tool: Tool::Brush });
        app.tool.brush_color = Color32::RED;
        commit_brush_line(&mut app, &brush_line(&[Point::new(180.0, 150.0), Point::new(220.0, 150.0)], 10.0));
        let m = cel_map(&app);
        assert!(m.is_valid());
        for e in 0..m.edges.len() {
            if m.edges[e].stroke.is_some() {
                let mid = m.quad(e).eval(0.5);
                assert!(!((mid.x - 200.0).abs() < 1.0 && (mid.y - 150.0).abs() < 8.0));
            }
        }
        // 線がつながっていないので、2つの赤い面は1つになる
        assert_eq!(m.faces().0.len(), 1);
    }

    /// 直線の線の区間（`a` から `b`）
    fn line(a: Point, b: Point, stroke: EdgeStroke) -> crate::planar_map::LineSeg {
        (QuadBez::new(a, a.midpoint(b), b), stroke)
    }

    #[test]
    fn lines_can_be_drawn_on_nothing_across_fills_and_inside_fills() {
        let mut app = Is2dApp::default();
        // 何も無い所の線：両側が何も無く、線のある辺が1本（塗りは無い）
        let seg = line(Point::new(500.0, 100.0), Point::new(700.0, 150.0), INK);
        assert!(draw_lines(&mut app, vec![seg]));
        let m = cel_map(&app);
        assert_eq!(m.edges.len(), 1);
        assert_eq!((m.edges[0].left, m.edges[0].right, m.edges[0].stroke), (None, None, Some(INK)));
        assert!(m.faces().0.is_empty());
        // 何も無い所の線も、線の図形として画面に出る
        assert_eq!(app.scene().iter().filter(|s| s.line == Some(INK)).count(), 1);
        // 離れた所にもう1本引き、1本目の上をブラシで塗る：触っていない2本目の線の図形は、作り直さずに使い回す
        let far = line(Point::new(500.0, 400.0), Point::new(700.0, 450.0), INK);
        assert!(draw_lines(&mut app, vec![far]));
        let lines_of = |app: &Is2dApp| {
            let tl = app.tab().active_timeline();
            let cel = tl.layers[0].cel_at(0).unwrap();
            cel.maps()[0].base().lines(&[])
        };
        let before = lines_of(&app);
        let kept = before.iter().find(|s| s.bounding_box().y0 > 300.0).unwrap().clone();
        apply(&mut app, Action::SetTool { tool: Tool::Brush });
        app.tool.brush_color = Color32::BLUE;
        commit_brush_line(&mut app, &brush_line(&[Point::new(600.0, 100.0), Point::new(600.0, 160.0)], 12.0));
        let after = lines_of(&app);
        assert_eq!(after.len(), 3);
        assert!(after.iter().any(|s| std::sync::Arc::ptr_eq(s, &kept)));
        // 赤い正方形を横切る線：赤が2つの面に分かれ、正方形の外の線は何も無い所の線になる
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![rect(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
        let across = line(Point::new(50.0, 200.0), Point::new(350.0, 200.0), INK);
        assert!(draw_lines(&mut app, vec![across]));
        let m = cel_map(&app);
        let (faces, broken) = m.faces();
        assert_eq!((faces.len(), broken), (2, 0));
        assert!((stroked_length(&m) - 300.0).abs() < 1e-3);
        // 線は、塗りの境目と交わった所で辺が分かれる（アンカーができる）：線の図形は辺1本ずつで3つ
        assert_eq!(counts(&app).1, 3);
        // 塗りの中で止まる線（行き止まり）：面の数は変わらない
        let spur = line(Point::new(150.0, 100.0), Point::new(150.0, 150.0), INK);
        assert!(draw_lines(&mut app, vec![spur]));
        let m = cel_map(&app);
        assert_eq!(m.faces().0.len(), 2);
        assert!(m.is_valid());
        // 塗りの中に浮いた線（どこにもつながらない）：面積 0 の穴として、それを含む面に入る
        let island = line(Point::new(200.0, 250.0), Point::new(260.0, 270.0), INK);
        assert!(draw_lines(&mut app, vec![island]));
        let m = cel_map(&app);
        let (faces, broken) = m.faces();
        assert_eq!((faces.len(), broken), (2, 0));
        assert!(m.is_valid());
        // 浮いた線の上をバケツ：浮いた線を穴に持つ面だけが塗り替わる
        assert!(apply(&mut app, Action::BucketFill { pt: Point::new(150.0, 270.0), color: Color32::BLUE }));
        let m = cel_map(&app);
        let mut colors: Vec<_> = m.faces().0.iter().map(|f| f.color.to_array()).collect();
        colors.sort();
        let mut expected = vec![Color32::RED.to_array(), Color32::BLUE.to_array()];
        expected.sort();
        assert_eq!(colors, expected);
        // 線で閉じた何も無い所を、バケツで塗れる
        let mut app = Is2dApp::default();
        let (a, b, c, d) = (
            Point::new(500.0, 500.0),
            Point::new(600.0, 500.0),
            Point::new(600.0, 600.0),
            Point::new(500.0, 600.0),
        );
        let square = vec![line(a, b, INK), line(b, c, INK), line(c, d, INK), line(d, a, INK)];
        assert!(draw_lines(&mut app, square));
        assert!(apply(&mut app, Action::BucketFill { pt: Point::new(550.0, 550.0), color: Color32::GREEN }));
        let m = cel_map(&app);
        let (faces, _) = m.faces();
        assert_eq!(faces.len(), 1);
        assert!(m.edges.iter().all(|e| e.stroke == Some(INK)));
    }

    /// 線の図形と面の図形の数（画面の図形）
    fn counts(app: &Is2dApp) -> (usize, usize) {
        let shapes: Vec<_> = app.scene().iter().cloned().collect();
        let lines = shapes.iter().filter(|s| s.line.is_some()).count();
        (shapes.len() - lines, lines)
    }

    /// 赤い正方形（100〜300）の輪郭に線 `INK` を乗せたアプリ
    fn inked_square_app() -> Is2dApp {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![rect(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 200.0), stroke: INK }));
        assert_eq!(counts(&app), (1, 4));
        app
    }

    #[test]
    fn clicking_selects_a_fill_or_a_line_separately() {
        // 塗りをクリックして消す：線は残る（Animate と同じ）
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(200.0, 200.0) }));
        assert_eq!(app.session.selection().shapes.len(), 1);
        assert!(apply(&mut app, Action::DeleteSelection));
        assert_eq!(counts(&app), (0, 4));
        let m = cel_map(&app);
        assert!(m.edges.iter().all(|e| e.left.is_none() && e.right.is_none() && e.stroke == Some(INK)));
        // 線（上の辺）をクリックして消す：塗りと、ほかの3辺の線は残る
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(200.0, 100.5) }));
        let picked: Vec<_> = app.session.selection().shapes.iter().copied().collect();
        assert!(picked.len() == 1 && crate::planar_map::is_line_id(picked[0]));
        assert!(apply(&mut app, Action::DeleteSelection));
        assert_eq!(counts(&app), (1, 3));
        let m = cel_map(&app);
        for e in 0..m.edges.len() {
            let q = m.quad(e);
            let top = (q.p0.y - 100.0).abs() < 1e-9 && (q.p2.y - 100.0).abs() < 1e-9;
            assert_eq!(m.edges[e].stroke.is_none(), top);
        }
        // 線をダブルクリック：角でつながった4辺をまとめて選ぶ
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectConnectedAt { pt: Point::new(200.0, 100.5), add: false }));
        assert_eq!(app.session.selection().shapes.len(), 4);
        // ダブルクリックで塗りと輪郭の線を選んで消す：何も残らない
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectConnectedAt { pt: Point::new(200.0, 200.0), add: false }));
        assert_eq!(app.session.selection().shapes.len(), 5);
        assert!(apply(&mut app, Action::DeleteSelection));
        assert_eq!(counts(&app), (0, 0));
        // 塗りだけを選んで動かす：線は元の場所に残り、塗りだけが動く
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(200.0, 200.0) }));
        run(
            &mut app,
            &[
                Action::FinishMoveSelectionDrag {
                    total_delta: KVec2::new(400.0, 0.0),
                    alt_duplicate: false,
                },
                Action::Deselect,
            ],
        );
        let m = cel_map(&app);
        let (faces, _) = m.faces();
        assert_eq!(faces.len(), 1);
        let line = app
            .scene()
            .iter()
            .filter(|s| s.line.is_some())
            .map(|s| s.bounding_box())
            .reduce(|a, b| a.union(b))
            .unwrap();
        assert!((line.x0 - 98.0).abs() < 0.05);
        let fill = app.scene().iter().find(|s| s.line.is_none()).unwrap().bounding_box();
        assert!((fill.x0 - 500.0).abs() < 1e-6);
        // 線だけを選んで動かす：塗りは残り、線だけが動く
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(200.0, 100.5) }));
        run(
            &mut app,
            &[
                Action::FinishMoveSelectionDrag {
                    total_delta: KVec2::new(0.0, 400.0),
                    alt_duplicate: false,
                },
                Action::Deselect,
            ],
        );
        assert_eq!(counts(&app), (1, 4));
        let moved = app
            .scene()
            .iter()
            .filter(|s| s.line.is_some() && s.bounding_box().y0 > 400.0)
            .count();
        assert_eq!(moved, 1);
        let line = app.scene().iter().find(|s| s.line.is_some() && s.bounding_box().y0 > 400.0).unwrap().bounding_box();
        assert!((line.y0 - 498.0).abs() < 0.05);
        let fill = app.scene().iter().find(|s| s.line.is_none()).unwrap().bounding_box();
        assert!((fill.y0 - 100.0).abs() < 1e-6);
    }

    #[test]
    fn double_clicking_a_line_selects_the_connected_same_lines() {
        let mut app = Is2dApp::default();
        let c = Point::new(300.0, 300.0);
        let thin = EdgeStroke {
            color: Color32::BLUE,
            width: 2,
        };
        // 3本が1点で交わる（辺は3本）。離れた所の別の線と、つながっているが線の違う線
        let segs = vec![
            line(Point::new(200.0, 300.0), c, INK),
            line(c, Point::new(400.0, 300.0), INK),
            line(c, Point::new(300.0, 200.0), INK),
        ];
        assert!(draw_lines(&mut app, segs));
        assert!(draw_lines(&mut app, vec![line(c, Point::new(300.0, 400.0), thin)]));
        assert!(draw_lines(&mut app, vec![line(Point::new(600.0, 300.0), Point::new(700.0, 300.0), INK)]));
        assert_eq!(counts(&app).1, 5);
        assert!(apply(&mut app, Action::SelectConnectedAt { pt: Point::new(250.0, 300.5), add: false }));
        assert_eq!(app.session.selection().shapes.len(), 3);
        // クリックは、辺1本だけ
        assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(250.0, 300.5) }));
        assert_eq!(app.session.selection().shapes.len(), 1);
    }

    #[test]
    fn a_line_copied_to_another_layer_is_a_different_line() {
        let mut app = Is2dApp::default();
        let seg = line(Point::new(100.0, 100.0), Point::new(300.0, 150.0), INK);
        assert!(draw_lines(&mut app, vec![seg]));
        // 同じ所に貼り付けて、別のレイヤーへ
        run(&mut app, &[Action::SelectAll, Action::Copy, Action::AddLayer, Action::Paste { in_place: true }, Action::Deselect]);
        let lines: Vec<_> = app.scene().iter().filter(|s| s.line.is_some()).map(|s| (s.layer_id, s.shape_id)).collect();
        assert_eq!(lines.len(), 2);
        assert_ne!(lines[0].0, lines[1].0);
        assert_ne!(lines[0].1, lines[1].1);
        // 上のレイヤーの線をクリックしても、下のレイヤーの線は選ばれない
        assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(200.0, 125.5) }));
        assert_eq!(app.session.selection().shapes.len(), 1);
    }

    /// 画面の図形のうち、線がみな塗りより上（あとに描く）にある
    fn lines_above_fills(app: &Is2dApp) -> bool {
        let shapes: Vec<_> = app.scene().iter().map(|s| s.line.is_some()).collect();
        let first_line = shapes.iter().position(|&l| l).unwrap_or(shapes.len());
        shapes[first_line..].iter().all(|&l| l)
    }

    #[test]
    fn grabbing_anywhere_on_a_visible_line_reshapes_its_edge_at_any_zoom() {
        // 上の辺（y=100。太さ 4 なので、中心から 2 までが見えている線）。拡大して線が合わせる幅より太く見えても、線の上なら
        // どこをつかんでも辺の変形になる（線の図形をつかんで線が外れない）。線の外は、塗りか何も無い所
        let app = inked_square_app();
        let hover = |y: f64, zoom: f32| {
            crate::canvas_tool::select_ops::compute_hover_target(
                app.scene(),
                &app.session,
                Point::new(200.0, y),
                zoom,
                SelectSubMode::Reshape,
            )
        };
        for zoom in [1.0f32, 2.0, 4.0, 8.0, 16.0] {
            for y in [98.2, 99.0, 100.0, 101.0, 101.8] {
                assert!(matches!(hover(y, zoom), HoverTarget::Edge { .. }), "倍率 {zoom}、y={y} は線の上");
            }
        }
        assert!(matches!(hover(97.0, 16.0), HoverTarget::None));
        match hover(103.0, 16.0) {
            HoverTarget::Fill { shape_id } => assert!(!crate::planar_map::is_line_id(shape_id)),
            other => panic!("線の内側の外は塗り：{other:?}"),
        }
    }

    #[test]
    fn ink_bottle_on_a_boundary_lines_only_that_edge() {
        // 線の無い赤い正方形（100〜300）と、右に接する青い正方形。境界線の上か近く（選択ツールで辺が光る距離：塗りの側は
        // 画面で 5.5px、何も無い側は 9.5px まで）を押すと、その辺1本だけに線を乗せる。塗りの中の離れた所は輪郭の全部
        let fresh = |zoom: f32| {
            let mut app = Is2dApp::default();
            put_cel_shapes(
                &mut app,
                0,
                0,
                vec![rect(100.0, 100.0, 300.0, 300.0, Color32::RED), rect(300.0, 100.0, 500.0, 300.0, Color32::BLUE)],
            );
            app.tab_mut().view.set_zoom(zoom);
            app
        };
        // 線を乗せた辺（中点）の並び
        let lined = |app: &Is2dApp| -> Vec<(i64, i64)> {
            let m = cel_map(app);
            let mut v: Vec<_> = (0..m.edges.len())
                .filter(|&e| m.edges[e].stroke.is_some())
                .map(|e| {
                    let c = m.quad(e).eval(0.5);
                    (c.x.round() as i64, c.y.round() as i64)
                })
                .collect();
            v.sort();
            v
        };
        for zoom in [1.0f32, 4.0] {
            let px = |n: f64| n / zoom as f64;
            let ink = |pt: Point| {
                let mut app = fresh(zoom);
                let changed = apply(&mut app, Action::InkBottle { pt, stroke: INK });
                (changed, lined(&app))
            };
            // 上の辺：塗りの側 3px、何も無い側 8px、辺の上
            for y in [100.0 + px(3.0), 100.0 - px(8.0), 100.0] {
                assert_eq!(ink(Point::new(200.0, y)), (true, vec![(200, 100)]), "倍率 {zoom}、y={y}");
            }
            // 赤と青の境目：どちらの塗りの側でも 3px なら、境目の1本だけ
            for x in [300.0 - px(3.0), 300.0 + px(3.0)] {
                assert_eq!(ink(Point::new(x, 200.0)), (true, vec![(300, 200)]), "倍率 {zoom}、x={x}");
            }
            // 塗りの中の離れた所（塗りの側 8px）：赤の輪郭の全部（4本）
            let (changed, all) = ink(Point::new(200.0, 100.0 + px(8.0)));
            assert!(changed);
            assert_eq!(all, vec![(100, 200), (200, 100), (200, 300), (300, 200)]);
            // 何も無い側の遠い所（12px）：何も変わらない
            assert_eq!(ink(Point::new(200.0, 100.0 - px(12.0))), (false, vec![]));
        }
    }

    #[test]
    fn clicking_near_a_line_selects_the_line_not_the_fill_beyond_it() {
        // 線の外の近く（輪郭の変形で辺が光る所：何も無い側は画面で 9.5px まで）をクリックすると、その線を選ぶ。塗りの輪郭の
        // 線の外側でも、線の向こうの塗りは選ばない。塗りの上は塗り。遠い所は何も選ばない
        let thin = EdgeStroke { color: Color32::BLACK, width: 1 };
        let picked = |y: f64, zoom: f32| {
            let mut app = inked_square_app();
            let seg = line(Point::new(100.0, 600.0), Point::new(300.0, 600.0), thin);
            assert!(draw_lines(&mut app, vec![seg]));
            app.tab_mut().view.set_zoom(zoom);
            let _ = apply(&mut app, Action::SelectSingleAt { pt: Point::new(200.0, y) });
            let ids: Vec<_> = app.session.selection().shapes.iter().copied().collect();
            match ids.as_slice() {
                [] => "なし",
                [id] if crate::planar_map::is_line_id(*id) => "線",
                [_] => "塗り",
                _ => "複数",
            }
        };
        for zoom in [1.0f32, 4.0] {
            let px = |n: f64| n / zoom as f64;
            for n in [1.0, 4.0, 8.0, 9.0] {
                assert_eq!(picked(100.0 - px(n), zoom), "線", "倍率 {zoom}：輪郭の線の外 {n}px");
                assert_eq!(picked(600.0 - px(n), zoom), "線", "倍率 {zoom}：太さ 1 の線の外 {n}px");
            }
            assert_eq!(picked(100.0 - px(12.0), zoom), "なし");
            assert_eq!(picked(600.0 - px(12.0), zoom), "なし");
            assert_eq!(picked(100.0 + px(12.0), zoom), "塗り");
        }
    }

    #[test]
    fn lines_redrawn_while_a_piece_floats_stay_above_the_fills() {
        // 線を乗せた赤い正方形の角から外へ出るヒゲ。ヒゲを持ち上げると、角の線のつなぎ目が変わり、角の輪郭の線を作り
        // 直して出す（浮いた片の残り）。作り直した線も、塗りより上に描く
        let mut app = inked_square_app();
        let whisker = line(Point::new(100.0, 100.0), Point::new(40.0, 60.0), INK);
        assert!(draw_lines(&mut app, vec![whisker]));
        assert!(lines_above_fills(&app));
        assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(70.0, 80.0) }));
        let picked: Vec<_> = app.session.selection().shapes.iter().copied().collect();
        assert!(picked.len() == 1 && crate::planar_map::is_line_id(picked[0]));
        let stage = app.stage();
        assert!(app.session.lift_selected_raw_shapes_to_floating(&stage));
        let stage = app.stage();
        app.session.begin_move_overlay(&stage, false, None, Vec::new());
        let remade = app.session.floating_view().remainder().filter(|s| s.line.is_some()).count();
        assert!(remade > 0, "角の輪郭の線を作り直す");
        assert!(lines_above_fills(&app));
        // 輪郭の変形の仮表示も同じ：線のある辺を引っぱっている間、作り直した線は塗りより上。赤の上の辺を引っぱると、
        // 角でつながった隣の青の上の辺の線も作り直す（青の塗りは変わらない）
        let mut app = Is2dApp::default();
        put_cel_shapes(
            &mut app,
            0,
            0,
            vec![rect(100.0, 100.0, 300.0, 300.0, Color32::RED), rect(300.0, 100.0, 500.0, 300.0, Color32::BLUE)],
        );
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 200.0), stroke: INK }));
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(400.0, 200.0), stroke: INK }));
        assert_eq!(counts(&app), (2, 7));
        let lid = app.tab().active_timeline().layers[0].id;
        let (removed, added) = {
            let tl = app.tab().active_timeline();
            let base = tl.layers[0].cel_at(0).unwrap().maps()[0].base();
            let orig = QuadBez::new(Point::new(100.0, 100.0), Point::new(200.0, 100.0), Point::new(300.0, 100.0));
            crate::planar_map::reshape_result(
                &base,
                crate::planar_map::ReshapeEdit::PullEdge { orig_seg: orig, grab_t: 0.5, target: Point::new(200.0, 60.0), follow: true },
                lid,
                &[],
                Affine::IDENTITY,
            )
            .unwrap()
        };
        assert!(added.iter().any(|s| s.line.is_some()));
        app.session.set_reshape_overlay(&removed, added, lid, &[]);
        assert!(lines_above_fills(&app));
    }

    #[test]
    fn a_slightly_bent_joint_has_no_crack_at_any_zoom() {
        // 太さ 4 の線を、つなぎ目でわずかに（0.01 ラジアン）曲げた2本の辺。外側のすき間の幅は 0.02px で、描画の許容より
        // 小さいが、拡大すると見える。つなぎ目の外側（曲がりの二等分の向きに半径の 0.99 倍）が、どちらかの線で塗られる
        // （前は、すき間が許容より小さければ埋めず、拡大すると白い切れ目が見えた）
        let mut app = Is2dApp::default();
        let (a, b) = (Point::new(100.0, 100.0), Point::new(200.0, 100.0));
        let th: f64 = 0.01;
        let c = b + KVec2::new(th.cos(), th.sin()) * 100.0;
        assert!(draw_lines(&mut app, vec![line(a, b, INK), line(b, c, INK)]));
        assert_eq!(counts(&app).1, 2);
        let r = INK.width as f64 / 2.0;
        // 曲がりは時計回り（y が下向きの座標で下へ）なので、外側は上（−y）の側
        let outer = KVec2::new((th / 2.0).sin(), -(th / 2.0).cos());
        let p = b + outer * (0.99 * r);
        assert!(
            app.scene().iter().any(|s| s.line.is_some() && s.contains_point(p)),
            "つなぎ目の外側にすき間がある"
        );
    }

    #[test]
    fn line_ends_are_round_while_dragging_a_line() {
        // 線を乗せた正方形の上の辺の線だけを選んで、下へ 50 動かすドラッグの途中。動かす線の両端と、残る左右の辺の線の、
        // 上の辺とつながっていた端は、どれも丸い（前は、動かす前の形をずらしただけなので、つなぎ目の端が平らに切れた
        // まま四角く見えた。離すと作り直して丸くなっていた）
        let covered = |app: &Is2dApp, p: Point| app.scene().iter().any(|s| s.line.is_some() && s.contains_point(p));
        for transform in [false, true] {
            let mut app = inked_square_app();
            assert!(apply(&mut app, Action::SelectSingleAt { pt: Point::new(200.0, 100.5) }));
            if transform {
                let stage = app.stage();
                let orig = app.session.begin_transform_overlay(&stage);
                app.session.update_transform_overlay(&orig, Affine::translate((0.0, 50.0)));
            } else {
                let stage = app.stage();
                app.session.begin_move_overlay(&stage, false, None, Vec::new());
                app.session.translate_move_overlay(KVec2::new(0.0, 50.0));
            }
            let place = if transform { "変形のドラッグ" } else { "移動のドラッグ" };
            // 動かす線の両端の丸（端の点から外へ 1.4。半径は 2）
            assert!(covered(&app, Point::new(98.6, 150.0)), "{place}：動かす線の左の端");
            assert!(covered(&app, Point::new(301.4, 150.0)), "{place}：動かす線の右の端");
            // 残る左右の辺の線の、上の端の丸
            assert!(covered(&app, Point::new(100.0, 98.6)), "{place}：残る左の辺の線の上の端");
            assert!(covered(&app, Point::new(300.0, 98.6)), "{place}：残る右の辺の線の上の端");
            // 動かした所には、元の線が残っていない
            assert!(!covered(&app, Point::new(200.0, 100.0)), "{place}：元の線が残った");
        }
    }

    #[test]
    fn a_moved_or_scaled_group_keeps_its_lines_selected() {
        // 線を乗せた正方形をグループにして、動かす・拡大する。グループの線はみな選ばれたままで、選択の外接矩形は線の太さ
        // まで入る（前は、線の番号が形の場所から決まるので、動かすと線が選択から外れ、外接矩形が塗りだけになって、線の
        // 太さの真ん中を通った）
        for scale in [false, true] {
            let mut app = inked_square_app();
            assert!(apply(&mut app, Action::SelectAll));
            assert!(apply(&mut app, Action::Group));
            if scale {
                assert!(scale_selection(&mut app, 2.0, 2.0));
            } else {
                assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(30.0, 20.0), alt_duplicate: false }));
            }
            let place = if scale { "拡大" } else { "移動" };
            for s in app.session.scene().iter() {
                assert!(app.session.is_shape_id_selected(s.shape_id), "{place}：選択から外れた図形がある（線 {}）", s.line.is_some());
            }
            let (b, _) = app.session.selection_bbox_and_pivot(1.0).unwrap();
            // 線の図形（太さまで）をみな合わせた外形
            let moved = app.session.scene().iter().map(|s| s.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
            assert!(app.session.scene().iter().any(|s| s.line.is_some()));
            let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
            assert!(
                near(b.x0, moved.x0) && near(b.y0, moved.y0) && near(b.x1, moved.x1) && near(b.y1, moved.y1),
                "{place}：選択の外接矩形が線の太さまで入らない：{b:?}"
            );
        }
    }

    #[test]
    fn scaling_keeps_line_width_except_in_symbols() {
        // 線（太さ 4）を乗せた正方形を2倍にする。生の絵とグループの線は、拡大しても太さが変わらない：選んでいる間（浮いた片）
        // も、拡大のドラッグ中も、確定したあとも 4。シンボルの線は一緒に太くなる（8）。前は、選んでいる間とドラッグ中だけ、
        // 線の図形をそのまま拡大して太く見えた（確定すると 4 に戻った）
        let thickness = |app: &Is2dApp| -> Vec<f64> {
            let mut v: Vec<f64> = app.scene().iter().filter(|s| s.line.is_some()).map(|s| {
                let b = s.bounding_box();
                b.width().min(b.height())
            }).collect();
            v.sort_by(f64::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            v
        };
        let near = |v: &[f64], w: f64| v.iter().all(|t| (t - w).abs() < 1e-6) && !v.is_empty();
        // 生の絵
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectAll));
        assert!(scale_selection(&mut app, 2.0, 2.0));
        assert!(near(&thickness(&app), 4.0), "生の絵：選んでいる間 {:?}", thickness(&app));
        assert!(apply(&mut app, Action::Deselect));
        assert!(near(&thickness(&app), 4.0), "生の絵：確定したあと {:?}", thickness(&app));
        // グループの拡大のドラッグ中
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::Group));
        let stage = app.stage();
        let orig = app.session.begin_transform_overlay(&stage);
        app.session.update_transform_overlay(&orig, Affine::scale_about(2.0, Point::new(200.0, 200.0)));
        assert!(near(&thickness(&app), 4.0), "グループ：ドラッグ中 {:?}", thickness(&app));
        // シンボル
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::ConvertToSymbol));
        assert!(scale_selection(&mut app, 2.0, 2.0));
        assert!(near(&thickness(&app), 8.0), "シンボル：拡大したあと {:?}", thickness(&app));
    }

    #[test]
    fn a_line_inside_a_moved_symbol_keeps_its_center_curve() {
        // 線を乗せた正方形をシンボルにして動かし、シンボルの中に入る。画面の線の中心の曲線（アンカーの表示に使う開いた曲線）
        // は、辺の両端のまま（長さ 200）。前は、図形を写すときに開いた曲線も閉じた輪としてつなぎ直したので、両端が真ん中に
        // 寄って潰れ、塗りの中に「辺の両端の中点から曲線の真ん中へ伸びる」幽霊のアンカーが見えた
        use crate::canvas_tool::doc_actions::{apply, Action};
        let mut app = inked_square_app();
        assert!(apply(&mut app, Action::SelectAll));
        assert!(apply(&mut app, Action::ConvertToSymbol));
        assert!(apply(&mut app, Action::FinishMoveSelectionDrag { total_delta: KVec2::new(37.0, 21.0), alt_duplicate: false }));
        let gid = app.tab().active_timeline().instances().next().unwrap().binding.instance_gid;
        assert!(apply(&mut app, Action::EnterSymbolTimeline { instance_gid: gid }));
        let mut n = 0;
        for s in app.scene().iter().filter(|s| s.line.is_some()) {
            let q = s.center_curve().unwrap();
            assert!(((q.p2 - q.p0).hypot() - 200.0).abs() < 1e-6, "線の中心の曲線が潰れた：{q:?}");
            n += 1;
        }
        assert_eq!(n, 4);
    }

    #[test]
    fn the_pencil_draws_a_smooth_line_with_few_curves() {
        let mut app = Is2dApp::default();
        apply(&mut app, Action::SetTool { tool: Tool::Pencil });
        // まっすぐな線：1本の直線になる
        let straight: Vec<Point> = (0..=100).map(|i| Point::new(100.0 + 3.0 * i as f64, 100.0)).collect();
        assert!(apply(&mut app, Action::PencilStroke { points: straight, stroke: INK }));
        let m = cel_map(&app);
        assert_eq!(m.edges.len(), 1);
        assert!((m.quad(0).p0 - Point::new(100.0, 100.0)).hypot() < 1.0);
        assert!((m.quad(0).p2 - Point::new(400.0, 100.0)).hypot() < 1.0);
        // 半円：少ない曲線で、点の近くを通る
        let arc: Vec<Point> = (0..=180)
            .map(|d| {
                let a = (d as f64).to_radians();
                Point::new(300.0 + 100.0 * a.cos(), 400.0 + 100.0 * a.sin())
            })
            .collect();
        assert!(apply(&mut app, Action::PencilStroke { points: arc.clone(), stroke: INK }));
        let m = cel_map(&app);
        let curves: Vec<QuadBez> = (0..m.edges.len()).map(|e| m.quad(e)).filter(|q| q.p0.y > 300.0 || q.p2.y > 300.0).collect();
        assert!(!curves.is_empty() && curves.len() <= 8, "曲線 {} 本", curves.len());
        // 線の図形は辺1本ずつ：まっすぐな線の1本と、半円の曲線の数
        assert_eq!(counts(&app).1, 1 + curves.len());
        for p in arc.iter().step_by(10) {
            let d = curves.iter().map(|q| crate::vector_engine::nearest_on_quad(q, *p).1).fold(f64::MAX, f64::min);
            assert!(d < 1.0, "点 {p:?} から {d}");
        }
        // 点が1つだけ（クリック）なら引かない
        assert!(!apply(&mut app, Action::PencilStroke { points: vec![Point::new(50.0, 50.0)], stroke: INK }));
    }

    /// 乱数の絵に、インクボトル・バケツ・ブラシ・消しゴムをくり返しかける（書くたびに、触った所だけの計算と地図全体で
    /// 計算した答えを比べる確かめが、テストでは毎回動く：`test_checks`）
    #[test]
    fn random_edits_with_lines_keep_the_map_valid() {
        let mut rng = fastrand::Rng::with_seed(0x11AE_2026_1123);
        let colors = [Color32::RED, Color32::BLUE, Color32::GREEN];
        for round in 0..12 {
            let mut app = Is2dApp::default();
            let shapes: Vec<FillShape> = (0..4)
                .map(|_| {
                    let (x, y) = (rng.f64() * 300.0 + 100.0, rng.f64() * 300.0 + 100.0);
                    let (w, h) = (rng.f64() * 150.0 + 40.0, rng.f64() * 150.0 + 40.0);
                    rect(x, y, x + w, y + h, colors[rng.usize(..colors.len())])
                })
                .collect();
            put_cel_shapes(&mut app, 0, 0, shapes);
            let pt = |rng: &mut fastrand::Rng| Point::new(rng.f64() * 400.0 + 100.0, rng.f64() * 400.0 + 100.0);
            for step in 0..40 {
                match rng.usize(..8) {
                    6 | 7 => {
                        // 輪郭の変形：地図の辺をつまんで引っぱる・角を動かす（線の辺も含む）
                        let m = cel_map_or_empty(&app);
                        if m.edges.is_empty() {
                            continue;
                        }
                        let e = rng.usize(..m.edges.len());
                        let lid = app.tab().active_timeline().layers[0].id;
                        let target = pt(&mut rng);
                        let action = if rng.bool() {
                            Action::ReshapeEdge {
                                layer_id: lid,
                                grab_t: 0.2 + rng.f64() * 0.6,
                                target_pos: m.nodes[m.edges[e].a].lerp(target, 0.3),
                                orig_seg: m.quad(e),
                                follow: true,
                            }
                        } else {
                            Action::MoveVertex {
                                layer_id: lid,
                                orig_vertex: m.nodes[m.edges[e].a],
                                target_pos: m.nodes[m.edges[e].a].lerp(target, 0.2),
                                // 頂点だけの移動と、まわりがついてくる引っぱりの両方（乱数は引かない）
                                follow: e.is_multiple_of(2),
                            }
                        };
                        apply(&mut app, action);
                    }
                    5 => {
                        let stroke = EdgeStroke {
                            color: colors[rng.usize(..colors.len())],
                            width: rng.u8(1..=10),
                        };
                        let (a, b, c) = (pt(&mut rng), pt(&mut rng), pt(&mut rng));
                        let segs = vec![
                            (QuadBez::new(a, a.lerp(b, rng.f64()), b), stroke),
                            (QuadBez::new(b, b.lerp(c, rng.f64()), c), stroke),
                        ];
                        draw_lines(&mut app, segs);
                    }
                    0 | 1 => {
                        let stroke = EdgeStroke {
                            color: colors[rng.usize(..colors.len())],
                            width: rng.u8(1..=10),
                        };
                        apply(&mut app, Action::InkBottle { pt: pt(&mut rng), stroke });
                    }
                    2 => {
                        let color = colors[rng.usize(..colors.len())];
                        apply(&mut app, Action::BucketFill { pt: pt(&mut rng), color });
                    }
                    k => {
                        apply(&mut app, Action::SetTool { tool: if k == 3 { Tool::Brush } else { Tool::Eraser } });
                        app.tool.brush_color = colors[rng.usize(..colors.len())];
                        let (a, b) = (pt(&mut rng), pt(&mut rng));
                        commit_brush_line(&mut app, &brush_line(&[a, a.midpoint(b), b], rng.f64() * 20.0 + 3.0));
                    }
                }
                // 画面の図形を毎回作る（線の図形の表を、変わった所だけで作る道と、そのお手本の確かめを通す）
                let _ = app.scene().len();
                let tl = app.tab().active_timeline();
                if let Some(cel) = tl.layers[0].cel_at(0) {
                    for m in cel.maps().iter() {
                        let map = m.base().map().clone();
                        assert!(map.is_valid(), "round {round} step {step}");
                        // 線のラベル（色の無い側の番号）は、線を乗せた辺にだけあり、面の番号とぶつからない
                        let faces: std::collections::HashSet<_> = map.labels().collect();
                        for e in &map.edges {
                            for (c, src) in [(e.left, e.left_src), (e.right, e.right_src)] {
                                if c.is_none() {
                                    if let Some(l) = src {
                                        assert!(e.stroke.is_some(), "round {round} step {step}");
                                        assert!(!faces.contains(&l), "round {round} step {step}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn brushing_over_an_inked_outline_removes_the_covered_line() {
        let mut app = Is2dApp::default();
        put_cel_shapes(&mut app, 0, 0, vec![rect(100.0, 100.0, 300.0, 300.0, Color32::RED)]);
        assert!(apply(&mut app, Action::InkBottle { pt: Point::new(200.0, 200.0), stroke: INK }));
        apply(&mut app, Action::SetTool { tool: Tool::Brush });
        app.tool.brush_color = Color32::BLUE;
        commit_brush_line(&mut app, &brush_line(&[Point::new(300.0, 120.0), Point::new(300.0, 280.0)], 15.0));
        // 右の辺の、ブラシが覆った所（y がおよそ 105〜295）には線が残らない
        let tl = app.tab().active_timeline();
        let cel = tl.layers[0].cel_at(0).unwrap();
        let base = cel.maps()[0].base();
        let m = base.map();
        for e in 0..m.edges.len() {
            if m.edges[e].stroke.is_some() {
                let mid = m.quad(e).eval(0.5);
                assert!(!((mid.x - 300.0).abs() < 1.0 && mid.y > 110.0 && mid.y < 290.0));
            }
        }
        assert!(stroked_length(m) > 400.0);
        // 画面の線の図形も、塗った所の線が無い（前の線の図形を使い回さない）
        let covered = Point::new(300.0, 200.0);
        assert!(!app.scene().iter().any(|s| s.line.is_some() && s.contains_point(covered)));
        assert!(app.scene().iter().any(|s| s.line.is_some() && s.contains_point(Point::new(200.0, 100.0))));
    }
}

/// テスト用：タイムラインのレイヤー `layer_idx`・コマ `frame` の画面用の結合メッシュを、写しを使わずにその場で作る（本番は
/// 写しから：`CelMeshCache::screen_mesh`。同じ `build_cel_world` と `screen_mesh::tinted_screen_mesh`）
trait CelScreenMesh {
    #[allow(clippy::too_many_arguments)]
    fn resolved_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
        world_xform: Affine,
        ctx: crate::symbol::SymbolEvalCtx<'_>,
    ) -> Option<std::sync::Arc<egui::Mesh>>;
}

impl CelScreenMesh for timeline::Timeline {
    fn resolved_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
        world_xform: Affine,
        ctx: crate::symbol::SymbolEvalCtx<'_>,
    ) -> Option<std::sync::Arc<egui::Mesh>> {
        let (world_batch, _) = self.build_cel_world(layer_idx, frame, world_xform, None, ctx)?;
        let tint = if dim_backdrop { crate::screen_mesh::CelTint::Dim } else { crate::screen_mesh::CelTint::Plain };
        Some(std::sync::Arc::new(crate::screen_mesh::tinted_screen_mesh(&world_batch, zoom, pan, tint)))
    }
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
    action: impl Into<Op>,
    label: &str,
) {
    op(app, action.into());
    let before: Vec<FillShape> = app.scene().iter().cloned().collect();

    // 1. 全部のレイヤーを作り直す（計算に使う写しの持ち主は画面の土台のまま）：図形は版まで同じ
    let ids: Vec<u64> = app.tabs[app.active]
        .active_timeline()
        .layers
        .iter()
        .map(|l| l.id)
        .collect();
    crate::canvas_tool::finish_cel_writes(&mut app.tabs[app.active], &ids);
    app.tabs[app.active].session.clear_overlay();
    let after: Vec<FillShape> = app.scene().iter().cloned().collect();
    assert_same_scene(&before, &after, label, true);

    // 2. 土台を空から作り直す（写しの持ち主がいないので、面の図形は作り直す）：形・色・番号は同じ（版は新しくなってよい）
    app.tabs[app.active].session.drop_scene_base();
    app.tabs[app.active].refresh_scene_base();
    app.tabs[app.active].session.clear_overlay();
    let after: Vec<FillShape> = app.scene().iter().cloned().collect();
    assert_same_scene(&before, &after, label, false);
}

/// 場面の図形 `before` と `after` が同じ（形はビットまで。`same_revision` なら生の図形の版も）
fn assert_same_scene(before: &[FillShape], after: &[FillShape], label: &str, same_revision: bool) {
    assert_eq!(
        before.len(),
        after.len(),
        "[{label}] Scene の図形数がキャッシュ再構築前後で一致すること"
    );
    for (idx, (a, b)) in before.iter().zip(after.iter()).enumerate() {
        assert_eq!(a.color(), b.color(), "[{label} #{idx}] color mismatch");
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
            contour_bits(a.exterior()),
            contour_bits(b.exterior()),
            "[{label} #{idx}] exterior bit-level mismatch"
        );
        assert_eq!(
            a.holes().len(),
            b.holes().len(),
            "[{label} #{idx}] holes len mismatch"
        );
        for (ha, hb) in a.holes().iter().zip(b.holes().iter()) {
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
            if same_revision {
                assert_eq!(
                    a.mesh_revision, b.mesh_revision,
                    "[{label} #{idx}] mesh_revision mismatch"
                );
            }
        }
    }
}

mod geometry_tests;
mod drawing_tests;
mod selection_tests;
mod timeline_tests;
mod symbols_tests;
mod save_tests;
mod sound_tests;
mod ui_tests;
mod perf_tests;
mod random_tests;
