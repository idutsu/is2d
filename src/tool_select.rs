use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui::{self, Color32, CursorIcon, PointerButton, Pos2, Stroke, Vec2};
use kurbo::{ParamCurve, Point, QuadBez, Rect as KRect};

use crate::canvas_tool::{
    ActiveCanvasDrag, HoverTarget, MapOwner, MarqueeDrag, SelectDragState, SelectSubMode, Tab,
};
use crate::doc_actions::{apply, Action, ActionHost, ToolInput, ToolProof};
use crate::map_hover::scope_maps;
use crate::planar_map::{reshape_result, LayerBase, ReshapeEdit};
use crate::select_ops::{apply_click_selection, finish_selection_drag, update_selection_drag};
pub(crate) use crate::select_ops::{compute_hover_target, handle_canvas_double_click};
use crate::vector_engine::{FillShape, LayerId};

// ============================================================================
// 選択ツール（Tool::Select / V キー）・輪郭変形・アンカー編集・グループ・クリップボード
// ============================================================================

/// 選んだ生の図形（画面の図形の番号 `picked`）を、選択の色で塗る（`FillShape::selection_screen_mesh`。同じ段の図形を描き
/// 終えた所で呼ぶ：`panels::draw_editable_layer_shapes`）
pub(crate) fn draw_selection_overlays(tab: &Tab, painter: &egui::Painter, picked: &[usize]) {
    let scene = tab.scene();
    for &idx in picked {
        if let Some(shape) = scene.get(idx) {
            painter.add(egui::Shape::mesh(shape.selection_screen_mesh(tab.view.zoom(), tab.view.pan)));
        }
    }
}

/// 輪郭の表示（ワイヤーフレーム）で描く辺と頂点：画面に出している生の図形の輪郭（面の図形は外周と穴の区間、線の図形は
/// 中心の曲線）と、そのアンカー（区間の始め・線の両端）を集め、座標がまったく同じ辺・頂点は1つにする。同じ地図の面と線は、
/// 地図の同じ辺から作るので、隣の面どうし・面とその輪郭の線は同じ辺を持ち、頂点も共有する（前は図形ごとに描いたので、
/// 同じ辺を最大3回、同じ頂点を平均 3.6 回描き、線の多い絵で重かった：レビュー 41766f9 の N-3001）。描く元が画面の図形
/// なので、ドラッグ中の仮の絵（上乗せ）にもそのままついていく
#[derive(Default)]
pub(crate) struct Wireframe {
    edges: Vec<QuadBez>,
    nodes: Vec<Point>,
    seen_edges: HashSet<[u64; 6]>,
    seen_nodes: HashSet<[u64; 2]>,
}

impl Wireframe {
    /// 図形 `shape` の輪郭とアンカーを足す（まったく同じものは足さない）
    pub(crate) fn add_shape(&mut self, shape: &FillShape) {
        if let Some(c) = shape.center_curve() {
            self.add_edge(c);
            self.add_node(c.p0);
            self.add_node(c.p2);
            return;
        }
        for contour in shape.contours() {
            for seg in contour.segments.iter() {
                self.add_edge(*seg);
                self.add_node(seg.p0);
            }
        }
    }

    fn add_edge(&mut self, q: QuadBez) {
        // 向きによらず同じ辺：端の点をビットの順に並べた鍵
        let bits = |p: Point| [p.x.to_bits(), p.y.to_bits()];
        let (a, b) = if bits(q.p0) <= bits(q.p2) { (q.p0, q.p2) } else { (q.p2, q.p0) };
        let [ax, ay] = bits(a);
        let [cx, cy] = bits(q.p1);
        let [bx, by] = bits(b);
        if self.seen_edges.insert([ax, ay, cx, cy, bx, by]) {
            self.edges.push(q);
        }
    }

    fn add_node(&mut self, p: Point) {
        if self.seen_nodes.insert([p.x.to_bits(), p.y.to_bits()]) {
            self.nodes.push(p);
        }
    }

    /// 集めた辺と頂点のうち、画面に見えている範囲にかかるものを描き、空にする（同じ地図の生の図形を描き終えた所で呼ぶ：
    /// 上に重なる図形には、今までどおり隠される）
    pub(crate) fn draw(&mut self, tab: &Tab, painter: &egui::Painter) {
        if self.edges.is_empty() && self.nodes.is_empty() {
            return;
        }
        // 見えている範囲を、アンカーの丸がはみ出す分（半径と、半径を中心に描く縁の太さの半分）だけ広げる：画面の端に
        // かかるアンカーも描く
        let pad = f64::from(ANCHOR_RADIUS + ANCHOR_RING / 2.0) / tab.view.zoom() as f64;
        let view = tab
            .view
            .visible_world_rect(painter, tab.doc().stage_size, tab.session.edit_path().shows_stage())
            .inflate(pad, pad);
        let stroke = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(30, 80, 180, 140));
        for q in self.edges.drain(..) {
            if crate::vector_engine::rects_overlap(KRect::from_points(q.p0, q.p2).union_pt(q.p1), view) {
                painter.add(egui::Shape::line(screen_polyline(tab, q), stroke));
            }
        }
        for p in self.nodes.drain(..) {
            if view.contains(p) {
                draw_anchor_dot(tab, painter, p);
            }
        }
        self.seen_edges.clear();
        self.seen_nodes.clear();
    }
}

/// 曲線 `q`（ステージの座標）を、画面で見分けられないずれ（画素の 1/4：`FIT_TOL_PX`）の中に収まる数で等分した、画面の
/// 座標の折れ線（2次ベジェは n 等分で弦のずれが |p0 − 2·p1 + p2| / (4n²)）。輪郭の表示とカーソル合わせの強調の、曲線を
/// 折れ線にする決まりはこれ1つ
fn screen_polyline(tab: &Tab, q: QuadBez) -> Vec<Pos2> {
    let z = tab.view.zoom() as f64;
    let dd = ((q.p1 - q.p0) - (q.p2 - q.p1)).hypot() * z;
    let n = ((dd / (4.0 * crate::vector_stroke::FIT_TOL_PX)).sqrt().ceil() as usize).max(1);
    (0..=n).map(|k| tab.view.world_to_screen(q.eval(k as f64 / n as f64))).collect()
}

/// 輪郭の表示のアンカーの丸の半径（画面の点）
const ANCHOR_RADIUS: f32 = 3.0;
/// 輪郭の表示のアンカーの丸の縁の太さ（画面の点。縁は半径の所を中心に描く）
const ANCHOR_RING: f32 = 1.2;

/// 輪郭の表示のアンカー1つ（文書の座標 `p`）
fn draw_anchor_dot(tab: &Tab, painter: &egui::Painter, p: Point) {
    let a = tab.view.world_to_screen(p);
    painter.circle_filled(a, ANCHOR_RADIUS, Color32::WHITE);
    painter.circle_stroke(a, ANCHOR_RADIUS, Stroke::new(ANCHOR_RING, Color32::from_rgb(30, 120, 230)));
}

pub(crate) fn handle_select_tool_input(
    host: &mut ToolInput<'_, impl ActionHost>,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    let sub_mode = host.tool().select_sub_mode;
    match sub_mode {
        SelectSubMode::AddAnchor => {
            handle_anchor_edit_input(host, ui, response, mouse_pt, true);
        }
        SelectSubMode::DeleteAnchor => {
            handle_anchor_edit_input(host, ui, response, mouse_pt, false);
        }
        SelectSubMode::Reshape => {
            let shift_held = ui.input(|i| i.modifiers.shift);
            let alt_held = ui.input(|i| i.modifiers.alt);
            if host.tab_mut().view.double_clicked(ui.ctx(), response) {
                if let Some(pt) = mouse_pt {
                    handle_canvas_double_click(host, pt, shift_held);
                }
                return;
            }
            if response.drag_started_by(PointerButton::Primary) {
                if let Some(start_pt) = host.tab().view.press_origin_world(ui.ctx()).or(mouse_pt) {
                    start_reshape_drag(host, start_pt, shift_held, alt_held);
                    if let Some(curr_pt) = mouse_pt {
                        if crate::canvas_tool::View::moved_since_press(start_pt, curr_pt) {
                            update_reshape_drag(host, curr_pt);
                        }
                    }
                }
            } else if response.dragged_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    update_reshape_drag(host, pt);
                }
            }
            if response.drag_stopped_by(PointerButton::Primary) {
                finish_reshape_drag(host, mouse_pt);
            } else if response.clicked_by(PointerButton::Primary) {
                if let Some(pt) = mouse_pt {
                    apply_click_selection(host, pt, shift_held, true);
                }
            }
        }
    }
    // アンカーの追加・削除のモードではドラッグを始めないので、どのモードでも同じ条件でよい
    let tab = host.tab_mut();
    if matches!(tab.session.drag, ActiveCanvasDrag::None) {
        let zoom = tab.view.zoom();
        let hover = mouse_pt
            .map(|pt| compute_hover_target(tab.scene(), &tab.session, pt, zoom, sub_mode))
            .unwrap_or(HoverTarget::None);
        tab.session.hover_target = hover;
    }
}

fn handle_anchor_edit_input(
    host: &mut impl ActionHost,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
    is_add: bool,
) {
    let just_pressed = response.hovered() && ui.input(|i| i.pointer.primary_pressed());
    if just_pressed {
        if let Some(pt) = mouse_pt {
            let action = if is_add {
                Action::AddAnchor { pt }
            } else {
                Action::DeleteAnchor { pt }
            };
            apply(host, action);
        }
    }
}

/// 地図での変形のドラッグを始める：レイヤー `layer_id` の地図から、ドラッグのあいだ使う元の地図と階層を作り、仮表示を空にする
/// （選択は空のとき：呼ぶ側が先に外す。選択を外すのは `clear_selection` だけ）
fn begin_map_drag(tab: &mut Tab, proof: ToolProof, layer_id: LayerId) -> Option<(Arc<LayerBase>, Vec<u64>)> {
    let maps = scope_maps(tab.scene(), &tab.session);
    let lm = maps.maps.iter().find(|m| m.layer_id == layer_id)?;
    let (base, group_path) = (lm.base.clone(), lm.group_path.clone());
    tab.session
        .set_reshape_overlay(proof, &[], Vec::new(), layer_id, &group_path);
    Some((base, group_path))
}

pub(crate) fn start_reshape_drag(
    host: &mut ToolInput<'_, impl ActionHost>,
    pt: Point,
    shift_held: bool,
    alt_held: bool,
) {
    let proof = host.proof();
    let sub_mode = host.tool().select_sub_mode;
    if shift_held {
        host.tab_mut().session.drag = ActiveCanvasDrag::Marquee(MarqueeDrag::new(pt, true));
        return;
    }
    let mut target = compute_hover_target(
        host.tab().scene(),
        &host.tab().session,
        pt,
        host.tab().view.zoom(),
        sub_mode,
    );
    if alt_held {
        if let Some(shape_id) = target.shape_id() {
            let selected = host.tab().session.is_shape_id_selected(shape_id);
            crate::select_ops::grab_and_move(host, pt, selected, true);
            return;
        }
    }
    if matches!(
        target,
        HoverTarget::Vertex { .. } | HoverTarget::Edge { .. }
    ) && (!host.tab().session.selection().is_empty() || host.tab().session.floating().is_some())
    {
        // 浮いた片の確定を断られたら、何も始めない（浮いた片はみな選ばれたまま）
        if !apply(host, Action::Deselect) {
            return;
        }
        target = compute_hover_target(
            host.tab().scene(),
            &host.tab().session,
            pt,
            host.tab().view.zoom(),
            sub_mode,
        );
    }
    match target {
        HoverTarget::Vertex {
            map_node: Some((MapOwner::Layer(layer_id), orig_vertex)),
            ..
        } => {
            let tab = host.tab_mut();
            let Some((base, group_path)) = begin_map_drag(tab, proof, layer_id) else {
                return;
            };
            let follow = drag_follows(tab);
            tab.session.drag = ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
                layer_id,
                group_path,
                base,
                orig_vertex,
                target: orig_vertex,
                follow,
            });
        }
        HoverTarget::Edge {
            t,
            seg,
            map_edge: Some((MapOwner::Layer(layer_id), orig_seg)),
            ..
        } => {
            let tab = host.tab_mut();
            let Some((base, group_path)) = begin_map_drag(tab, proof, layer_id) else {
                return;
            };
            let grab_t = crate::planar_map::grab_t(t);
            // マウス pt から、つかむ所（丸めた grab_t の所）までのずれ（ステージの座標）
            let grab_offset = seg.eval(grab_t) - pt;
            let follow = drag_follows(tab);
            tab.session.drag = ActiveCanvasDrag::Select(SelectDragState::PullEdge {
                layer_id,
                group_path,
                base,
                grab_t,
                grab_offset,
                target: orig_seg.eval(grab_t),
                orig_seg,
                follow,
            });
        }
        HoverTarget::Vertex { .. } | HoverTarget::Edge { .. } => {}
        HoverTarget::Fill { shape_id } => {
            let selected = host.tab().session.is_shape_id_selected(shape_id);
            crate::select_ops::grab_and_move(host, pt, selected, false);
        }
        HoverTarget::None => {
            host.tab_mut().session.drag = ActiveCanvasDrag::Marquee(MarqueeDrag::new(pt, false));
        }
    }
}

/// 輪郭をつかんで動かすとき、まわりがついてくる引っぱりにするか（線の上をつかんだときも、アンカーをつかんだときも、決める
/// のはこれ1つ）：アンカーを表示していないときは、まわりのアンカーも離れるほど小さく動きながらついてくる（ざっくりした
/// 変形。Animate の選択ツール）。表示しているときは、つかんだ所だけ（線の上ならその1区間、アンカーならそのアンカー：細かい
/// 変形。Animate の部分選択ツールの代わり。利用者が決めた）
pub(crate) fn drag_follows(tab: &Tab) -> bool {
    !tab.view.show_wireframe
}

pub(crate) fn update_reshape_drag(host: &mut ToolInput<'_, impl ActionHost>, pt: Point) {
    let proof = host.proof();
    if update_selection_drag(host, pt) {
        return;
    }
    let tab = host.tab_mut();
    // 地図は文書の座標。マウスの位置は、変換の逆で文書の座標へ写す（ほかの頂点に吸い付かない。利用者が決めた）
    let xform = tab.session.edit_path().world_xform();
    // ドラッグ中も、離したときと同じ計算（交われば辺を切って色を決め直す）をして、その結果を見せる
    let update = match &mut tab.session.drag {
        ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
            layer_id,
            group_path,
            base,
            orig_vertex,
            target,
            follow,
        }) => {
            *target = xform.inverse() * pt;
            Some((
                base.clone(),
                ReshapeEdit::vertex(*orig_vertex, *target, *follow),
                *layer_id,
                group_path.clone(),
            ))
        }
        ActiveCanvasDrag::Select(SelectDragState::PullEdge {
            layer_id,
            group_path,
            base,
            grab_t,
            grab_offset,
            target,
            orig_seg,
            follow,
        }) => {
            *target = xform.inverse() * (pt + *grab_offset);
            let target = *target;
            Some((
                base.clone(),
                ReshapeEdit::PullEdge {
                    orig_seg: *orig_seg,
                    grab_t: *grab_t,
                    target,
                    follow: *follow,
                },
                *layer_id,
                group_path.clone(),
            ))
        }
        _ => None,
    };
    let Some((base, edit, layer_id, group_path)) = update else {
        return;
    };
    let (removed, added) =
        reshape_result(&base, edit, layer_id, &group_path, xform).unwrap_or_default();
    tab.session
        .set_reshape_overlay(proof, &removed, added, layer_id, &group_path);
}

pub(crate) fn finish_reshape_drag(host: &mut ToolInput<'_, impl ActionHost>, mouse_pt: Option<Point>) {
    if finish_selection_drag(host, mouse_pt) {
        return;
    }
    let ActiveCanvasDrag::Select(finished_drag) =
        std::mem::replace(&mut host.tab_mut().session.drag, ActiveCanvasDrag::None)
    else {
        return;
    };
    match finished_drag {
        SelectDragState::PullEdge {
            layer_id,
            grab_t,
            target,
            orig_seg,
            follow,
            ..
        } => {
            apply(
                host,
                Action::ReshapeEdge {
                    layer_id,
                    grab_t,
                    target_pos: target,
                    orig_seg,
                    follow,
                },
            );
        }
        SelectDragState::MoveVertex {
            layer_id,
            orig_vertex,
            target,
            follow,
            ..
        } => {
            apply(
                host,
                Action::MoveVertex {
                    layer_id,
                    orig_vertex,
                    target_pos: target,
                    follow,
                },
            );
        }
    }
}

// ------------------------------------------------------------------------
// 3. 選択ツールカーソル描画
// ------------------------------------------------------------------------

pub(crate) fn draw_select_tool_cursor(
    tab: &Tab,
    select_sub_mode: SelectSubMode,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    ctx.set_cursor_icon(CursorIcon::Default);
    match &tab.session.drag {
        ActiveCanvasDrag::Select(SelectDragState::PullEdge {
            base,
            orig_seg,
            grab_t,
            target,
            follow,
            ..
        }) => {
            if tab.view.show_wireframe {
                // 引っぱっている辺（ついてくる辺も）の今の形（確定と同じ `pulled_edges`）を、ステージに置いて見せる。格子の
                // 外へ出る形は、塗りの仮表示と同じく元の形のまま（確定は断る）
                let grab = crate::planar_map::PullGrab::Edge {
                    orig_seg: *orig_seg,
                    grab_t: *grab_t,
                    follow: *follow,
                };
                let moved = base.pulled_edges(grab, *target).unwrap_or_default();
                let on_grid = moved
                    .iter()
                    .all(|(_, q)| crate::planar_map::on_grid([q.p0, q.p1, q.p2].into_iter()));
                let xform = tab.session.edit_path().world_xform();
                if on_grid && !moved.is_empty() {
                    for (_, q) in moved {
                        draw_highlighted_quad_segment(tab, painter, xform * q);
                    }
                } else {
                    draw_highlighted_quad_segment(tab, painter, xform * *orig_seg);
                }
            }
        }
        ActiveCanvasDrag::Select(SelectDragState::MoveVertex { .. })
        | ActiveCanvasDrag::MoveSelection(_)
        | ActiveCanvasDrag::Marquee(_) => {}
        _ => draw_select_sub_mode_cursor(tab, select_sub_mode, painter, pos),
    }
}

fn draw_select_sub_mode_cursor(
    tab: &Tab,
    select_sub_mode: SelectSubMode,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match select_sub_mode {
        SelectSubMode::AddAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, true);
            if let HoverTarget::Edge {
                seg, closest_pos, ..
            } = tab.session.hover_target
            {
                draw_highlighted_quad_segment(tab, painter, seg);
                let cp = tab.view.world_to_screen(closest_pos);
                painter.circle_filled(cp, 5.5, Color32::from_rgb(30, 200, 95));
                painter.circle_stroke(cp, 5.5, Stroke::new(2.0_f32, Color32::WHITE));
            }
        }
        SelectSubMode::DeleteAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, false);
            if let HoverTarget::Vertex { pos: v_pos, .. } = tab.session.hover_target {
                let vp = tab.view.world_to_screen(v_pos);
                painter.circle_filled(vp, 6.0, Color32::from_rgb(235, 55, 55));
                painter.circle_stroke(vp, 6.0, Stroke::new(2.0_f32, Color32::WHITE));
            }
        }
        SelectSubMode::Reshape => draw_reshape_hover_cursor(tab, painter),
    }
}

fn draw_reshape_hover_cursor(tab: &Tab, painter: &egui::Painter) {
    if !tab.view.show_wireframe {
        return;
    }
    match tab.session.hover_target {
        HoverTarget::Vertex {
            pos: v_pos,
            shape_id,
            contour_idx,
            ..
        } => {
            if let Some(contour) = tab
                .scene()
                .shape_by_id(shape_id)
                .and_then(|s| s.get_contour(contour_idx))
            {
                for seg in contour.segments.iter() {
                    let p = tab.view.world_to_screen(seg.p0);
                    painter.circle_filled(p, 3.2, Color32::WHITE);
                    painter.circle_stroke(
                        p,
                        3.2,
                        Stroke::new(1.2_f32, Color32::from_rgb(30, 130, 240)),
                    );
                }
            }
            let vp = tab.view.world_to_screen(v_pos);
            painter.circle_filled(vp, 5.5, Color32::from_rgb(255, 120, 40));
            painter.circle_stroke(vp, 5.5, Stroke::new(2.0_f32, Color32::WHITE));
        }
        HoverTarget::Edge {
            seg, closest_pos, ..
        } => {
            draw_highlighted_quad_segment(tab, painter, seg);
            painter.circle_filled(
                tab.view.world_to_screen(closest_pos),
                4.0,
                Color32::from_rgb(0, 175, 255),
            );
        }
        HoverTarget::Fill { .. } | HoverTarget::None => {}
    }
}

/// ホバー・ドラッグ中の2次ベジェ曲線セグメントとその両端アンカーを強調描画（ハンドルは表示しない）
pub(crate) fn draw_highlighted_quad_segment(tab: &Tab, painter: &egui::Painter, seg: QuadBez) {
    painter.add(egui::Shape::line(
        screen_polyline(tab, seg),
        Stroke::new(3.0_f32, Color32::from_rgb(0, 175, 255)),
    ));
    for end_pt in [seg.p0, seg.p2] {
        let p = tab.view.world_to_screen(end_pt);
        painter.circle_filled(p, 4.0, Color32::WHITE);
        painter.circle_stroke(p, 4.0, Stroke::new(1.8_f32, Color32::from_rgb(0, 140, 230)));
    }
}

/// アンカー追加（`is_plus = true`: 緑「＋」）およびアンカー削除（`is_plus = false`: 赤「－」）のカーソルバッジを描画
fn draw_anchor_edit_cursor_badge(painter: &egui::Painter, cursor: Pos2, is_plus: bool) {
    let base = cursor + Vec2::new(15.0, 15.0);
    let fill = if is_plus {
        Color32::from_rgb(30, 190, 90)
    } else {
        Color32::from_rgb(230, 55, 55)
    };
    painter.circle_filled(base, 8.0, fill);
    painter.circle_stroke(base, 8.0, Stroke::new(1.5_f32, Color32::WHITE));
    let stroke = Stroke::new(2.2_f32, Color32::WHITE);
    painter.line_segment(
        [
            Pos2::new(base.x - 4.5, base.y),
            Pos2::new(base.x + 4.5, base.y),
        ],
        stroke,
    );
    if is_plus {
        painter.line_segment(
            [
                Pos2::new(base.x, base.y - 4.5),
                Pos2::new(base.x, base.y + 4.5),
            ],
            stroke,
        );
    }
}
