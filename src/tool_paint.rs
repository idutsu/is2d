use eframe::egui::{self, Color32, PointerButton, Pos2, Stroke, Vec2};
use kurbo::Point;

use crate::canvas_tool::{ActiveCanvasDrag, Tab, Tool};
use crate::doc_actions::{apply, topmost_visible_at, Action, ActionHost};
use crate::vector_engine::{append_mesh, tessellate_raw_polygon, transform_mesh_to_screen};
use crate::vector_stroke::{input_step, stroke_preview_outline, CenterlineBuilder};

// ============================================================================
// ブラシ（B）・消しゴム（E）・バケツ塗り（K）・スポイト（I）の操作ロジック
// ============================================================================

pub(crate) fn handle_brush_or_eraser_input(
    host: &mut impl ActionHost,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    let active_tool = host.tool().tool;
    let brush_radius = host.tool().brush_radius;
    let eraser_radius = host.tool().eraser_radius;
    let brush_color = host.tool().brush_color;
    let stroke = host.tool().stroke;

    // 描いている間の形の半径と色（鉛筆は、引く線の太さの半分と線の色）
    let (base_r, preview_color) = match active_tool {
        Tool::Brush => (brush_radius as f64, brush_color),
        Tool::Pencil => (stroke.width as f64 / 2.0, stroke.color),
        // 消しゴムは不透明な白（描いている間の形が重なる所で色が濃くならない。利用者が決めた）
        _ => (eraser_radius as f64, Color32::WHITE),
    };
    // 輪郭の区切りごとに三角形に分けて足したメッシュ（`stroke_preview_outline`）
    let preview_mesh = |rings: Vec<Vec<Point>>| {
        let mut mesh = egui::Mesh::default();
        for ring in &rings {
            append_mesh(&mut mesh, &tessellate_raw_polygon(ring, preview_color));
        }
        mesh
    };
    // 描いている間の形は、離したあとの形と同じ中心線（`CenterlineBuilder`）を太らせたもの。確定した所は確定するたびに
    // 足し、確定していない所は点が来るたびに作り直す。2つの境目は、両側の区間がどちらも確定した節で決まる、いちばん先の
    // 節（`CenterlineBuilder::settled`）で、そこでは両側の平らな端がぴったり接する（`stroke_preview_outline`）
    let z = host.tab().view.zoom() as f64;
    let ppp = response.ctx.pixels_per_point();
    host.tab_mut().view.pixels_per_point = ppp;
    let tail_of = |b: &CenterlineBuilder| {
        let cl = b.current();
        let from = b.settled();
        preview_mesh(stroke_preview_outline(&cl, from, cl.len().saturating_sub(1), base_r, z))
    };

    if response.drag_started_by(PointerButton::Primary) {
        let press_pt = host.tab().view.press_origin_world(&response.ctx);
        if let Some(start_pt) = press_pt.or(mouse_pt) {
            let mut stroke = CenterlineBuilder::new(input_step(z, ppp as f64));
            stroke.push(start_pt);
            if let Some(curr_pt) = mouse_pt {
                stroke.push(curr_pt);
            }
            let tail_mesh = tail_of(&stroke);
            let tab = host.tab_mut();
            tab.session.drag = ActiveCanvasDrag::PaintStroke {
                stroke,
                committed_world_mesh: egui::Mesh::default(),
                tail_mesh,
            };
        }
    } else if response.dragged_by(PointerButton::Primary) {
        let tab = host.tab_mut();
        if let (
            Some(pt),
            ActiveCanvasDrag::PaintStroke {
                stroke,
                committed_world_mesh,
                tail_mesh,
            },
        ) = (mouse_pt, &mut tab.session.drag)
        {
            let before = stroke.points().len();
            let settled_before = stroke.settled();
            let fixed_before = stroke.push(pt);
            if stroke.points().len() != before {
                if fixed_before.is_some() {
                    let cl = stroke.current();
                    let piece =
                        stroke_preview_outline(&cl, settled_before, stroke.settled(), base_r, z);
                    append_mesh(committed_world_mesh, &preview_mesh(piece));
                }
                *tail_mesh = tail_of(stroke);
            }
        }
    }
    // 点の列から、ブラシなら塗り、鉛筆なら線を引き、消しゴムなら消す操作を作る
    let stroke_action = |points: Vec<Point>| match active_tool {
        Tool::Brush => Action::PaintStroke {
            points,
            radius: brush_radius,
            color: brush_color,
        },
        Tool::Pencil => Action::PencilStroke { points, stroke },
        _ => Action::EraseStroke {
            points,
            radius: eraser_radius,
        },
    };
    if response.drag_stopped_by(PointerButton::Primary) {
        let drag = std::mem::take(&mut host.tab_mut().session.drag);
        if let ActiveCanvasDrag::PaintStroke { stroke, .. } = drag {
            if !stroke.points().is_empty() {
                apply(host, stroke_action(stroke.points().to_vec()));
            }
        }
    } else if response.clicked_by(PointerButton::Primary) {
        if let Some(pt) = mouse_pt {
            apply(host, stroke_action(vec![pt]));
        }
    }
}

/// 編集スコープおよび表示中の全レイヤーを対象に、指定座標 `pt` にヒットする最前面シェイプの色を返す（スポイト用）
pub(crate) fn topmost_visible_color_at(tab: &Tab, pt: Point) -> Option<Color32> {
    topmost_visible_at(tab, pt).map(|h| h.color)
}

/// スポイトツール（色抽出）：指定位置 `pt` でいちばん手前に見えているもの（`topmost_visible_at`）が線なら、その線の色と
/// 太さをインクボトルの線に、塗りならその色を `host.tool_mut().brush_color` に設定する
pub(crate) fn pick_color_at(host: &mut impl ActionHost, pt: Point) -> Option<Color32> {
    let seen = topmost_visible_at(host.tab(), pt)?;
    match seen.line {
        Some(stroke) => host.tool_mut().stroke = stroke,
        None => host.tool_mut().brush_color = seen.color,
    }
    Some(seen.color)
}

/// 進行中のブラシ・消しゴムストロークの増分プレビューメッシュを描画する
pub(crate) fn draw_active_stroke_preview(tab: &Tab, artwork_painter: &egui::Painter) {
    let ActiveCanvasDrag::PaintStroke {
        committed_world_mesh,
        tail_mesh,
        ..
    } = &tab.session.drag
    else {
        return;
    };
    if !committed_world_mesh.is_empty() {
        artwork_painter.add(egui::Shape::mesh(transform_mesh_to_screen(
            committed_world_mesh.clone(),
            tab.view.zoom(),
            tab.view.pan,
            None,
        )));
    }
    if !tail_mesh.is_empty() {
        artwork_painter.add(egui::Shape::mesh(transform_mesh_to_screen(
            tail_mesh.clone(),
            tab.view.zoom(),
            tab.view.pan,
            None,
        )));
    }
}

/// スポイトツール（`I`）のカラーサンプルリング＆抽出色プレビュー円形バッジを描画
pub(crate) fn draw_eyedropper_cursor_badge(
    painter: &egui::Painter,
    cursor: Pos2,
    sample_color: Color32,
    is_over_shape: bool,
) {
    // カーソル先端のターゲットドット
    painter.circle_filled(cursor, 2.5, sample_color);
    painter.circle_stroke(cursor, 2.5, Stroke::new(1.2_f32, Color32::WHITE));

    // 右下に抽出色プレビューバッジを表示
    let badge = cursor + Vec2::new(16.0, 16.0);
    let r = if is_over_shape { 9.5 } else { 7.5 };
    painter.circle_filled(badge, r, sample_color);
    painter.circle_stroke(badge, r, Stroke::new(2.2_f32, Color32::WHITE));
    painter.circle_stroke(
        badge,
        r + 1.2,
        Stroke::new(
            1.2_f32,
            if is_over_shape {
                Color32::from_rgb(0, 145, 255)
            } else {
                Color32::from_rgb(30, 35, 45)
            },
        ),
    );
}
