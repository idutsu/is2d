use eframe::egui::{self, Color32, PointerButton, Pos2, Stroke, Vec2};
use kurbo::Point;

use crate::canvas_tool::{ActiveCanvasDrag, Tab, Tool};
use crate::doc_actions::{Action, ActionHost, apply};
#[cfg(test)]
pub(crate) use crate::doc_actions::{bucket_fill_at, commit_raw_outline};
use crate::vector_engine::{
    MAX_VIEW_ZOOM, MIN_VIEW_ZOOM, append_mesh, tessellate_raw_polygon, transform_mesh_to_screen,
};
use crate::vector_stroke::generate_brush_outline_at_zoom;

/// ブラシ・消しゴムドラッグ中のリアルタイムプレビューにおいて、
/// 1フレームあたりに再テッセレーションする末尾ストローク点数の上限（これを超えた前半区間は `committed_mesh` に確定させて O(1) 更新を保証する）
const STROKE_TAIL_CHUNK_LEN: usize = 24;

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

    let base_r = if active_tool == Tool::Brush {
        brush_radius as f64
    } else {
        eraser_radius as f64
    };
    let preview_color = if active_tool == Tool::Brush {
        brush_color
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 220)
    };
    let z = (host.tab().view.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
    let screen_r = base_r * z;
    let min_sample_dist = (0.25 * screen_r).clamp(0.4, 1.5) / z;

    if response.drag_started_by(PointerButton::Primary) {
        let press_pt = response
            .ctx
            .input(|i| i.pointer.press_origin())
            .map(|p| host.tab().view.screen_to_world(p));
        if let Some(start_pt) = press_pt.or(mouse_pt) {
            let mut stroke = vec![start_pt];
            if let Some(curr_pt) = mouse_pt {
                if (curr_pt - start_pt).hypot() >= min_sample_dist {
                    stroke.push(curr_pt);
                }
            }
            let tail_outline = generate_brush_outline_at_zoom(&stroke, base_r, z);
            let tail_mesh = tessellate_raw_polygon(&tail_outline, preview_color);
            let tab = host.tab_mut();
            let zp = (tab.view.zoom, tab.view.pan);
            tab.session.drag = ActiveCanvasDrag::PaintStroke {
                stroke,
                committed_world_mesh: egui::Mesh::default(),
                committed_screen_mesh: egui::Mesh::default(),
                committed_zoom_pan: zp,
                tail_outline,
                tail_mesh,
                tail_start_idx: 0,
            };
        }
    } else if response.dragged_by(PointerButton::Primary) {
        let tab = host.tab_mut();
        let curr_zp = (tab.view.zoom, tab.view.pan);
        if let (
            Some(pt),
            ActiveCanvasDrag::PaintStroke {
                stroke,
                committed_world_mesh,
                committed_screen_mesh,
                committed_zoom_pan,
                tail_outline,
                tail_mesh,
                tail_start_idx,
            },
        ) = (mouse_pt, &mut tab.session.drag)
        {
            if *committed_zoom_pan != curr_zp {
                *committed_screen_mesh = transform_mesh_to_screen(
                    committed_world_mesh,
                    curr_zp.0,
                    curr_zp.1,
                    false,
                );
                *committed_zoom_pan = curr_zp;
            }
            if stroke
                .last()
                .is_none_or(|&last| (pt - last).hypot() >= min_sample_dist)
            {
                if stroke.len().saturating_sub(*tail_start_idx) >= STROKE_TAIL_CHUNK_LEN {
                    append_mesh(committed_world_mesh, tail_mesh);
                    let screen_chunk =
                        transform_mesh_to_screen(tail_mesh, curr_zp.0, curr_zp.1, false);
                    append_mesh(committed_screen_mesh, &screen_chunk);
                    *tail_start_idx = stroke.len().saturating_sub(2);
                }
                stroke.push(pt);
                *tail_outline =
                    generate_brush_outline_at_zoom(&stroke[*tail_start_idx..], base_r, z);
                *tail_mesh = tessellate_raw_polygon(tail_outline, preview_color);
            }
        }
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        let drag = std::mem::take(&mut host.tab_mut().session.drag);
        if let ActiveCanvasDrag::PaintStroke { stroke, .. } = drag {
            if !stroke.is_empty() {
                if active_tool == Tool::Brush {
                    apply(
                        host,
                        Action::PaintStroke {
                            points: stroke,
                            radius: brush_radius,
                            color: brush_color,
                        },
                    );
                } else {
                    apply(
                        host,
                        Action::EraseStroke {
                            points: stroke,
                            radius: eraser_radius,
                        },
                    );
                }
            }
        }
    } else if response.clicked_by(PointerButton::Primary) {
        if let Some(pt) = mouse_pt {
            if active_tool == Tool::Brush {
                apply(
                    host,
                    Action::PaintStroke {
                        points: vec![pt],
                        radius: brush_radius,
                        color: brush_color,
                    },
                );
            } else {
                apply(
                    host,
                    Action::EraseStroke {
                        points: vec![pt],
                        radius: eraser_radius,
                    },
                );
            }
        }
    }
}

/// 編集スコープおよび表示中の全レイヤーを対象に、指定座標 `pt` にヒットする最前面シェイプの色を返す（スポイト用）
pub(crate) fn topmost_visible_color_at(tab: &Tab, pt: Point) -> Option<Color32> {
    let scene = tab.scene();
    let tl = tab.active_timeline();
    let num_layers = tl.layers.len();
    if num_layers == 1
        && tl
            .layers
            .first()
            .is_some_and(|l| l.visible && !l.locked)
    {
        return scene
            .iter()
            .rev()
            .find(|s| s.contains_point(pt))
            .map(|s| s.color);
    }
    for layer_idx in (0..num_layers).rev() {
        let layer = &tl.layers[layer_idx];
        if !layer.visible {
            continue;
        }
        if layer.locked {
            let world_xform = tab.session.edit_path.world_xform();
            let local_pt = if world_xform == kurbo::Affine::IDENTITY {
                pt
            } else if world_xform.determinant().abs() > 1e-9 {
                world_xform.inverse() * pt
            } else {
                continue;
            };
            let shapes = tl.evaluate_layer_shapes_at(layer_idx, tl.current_frame, &tab.doc.library);
            if let Some(hit) = shapes.iter().rev().find(|s| s.contains_point(local_pt)) {
                return Some(hit.color);
            }
        } else {
            let lid = layer.id;
            if let Some(hit) = scene
                .iter()
                .rev()
                .find(|s| s.layer_id == lid && s.contains_point(pt))
            {
                return Some(hit.color);
            }
        }
    }
    None
}

/// スポイトツール（色抽出）：指定位置 `pt` の最前面シェイプの色を抽出し、`host.tool_mut().brush_color` に設定する
pub(crate) fn pick_color_at(host: &mut impl ActionHost, pt: Point) -> Option<Color32> {
    let picked = topmost_visible_color_at(host.tab(), pt)?;
    host.tool_mut().brush_color = picked;
    Some(picked)
}

/// 進行中のブラシ・消しゴムストロークの増分プレビューメッシュを描画する
pub(crate) fn draw_active_stroke_preview(
    tab: &Tab,
    active_tool: Tool,
    artwork_painter: &egui::Painter,
    gizmo_painter: &egui::Painter,
) {
    let ActiveCanvasDrag::PaintStroke {
        committed_world_mesh,
        committed_screen_mesh,
        committed_zoom_pan,
        tail_outline,
        tail_mesh,
        ..
    } = &tab.session.drag
    else {
        return;
    };
    if tail_outline.is_empty() && committed_world_mesh.is_empty() {
        return;
    }
    if !committed_world_mesh.is_empty() {
        let mesh = if *committed_zoom_pan == (tab.view.zoom, tab.view.pan) {
            committed_screen_mesh.clone()
        } else {
            transform_mesh_to_screen(
                committed_world_mesh,
                tab.view.zoom,
                tab.view.pan,
                false,
            )
        };
        artwork_painter.add(egui::Shape::Mesh(mesh));
    }
    if !tail_mesh.is_empty() {
        artwork_painter.add(egui::Shape::Mesh(transform_mesh_to_screen(
            tail_mesh,
            tab.view.zoom,
            tab.view.pan,
            false,
        )));
    }

    if active_tool == Tool::Eraser {
        let n = tail_outline.len();
        for i in 0..n {
            gizmo_painter.line_segment(
                [
                    tab.view.world_to_screen(tail_outline[i]),
                    tab.view.world_to_screen(tail_outline[(i + 1) % n]),
                ],
                Stroke::new(1.2_f32, Color32::from_rgb(220, 60, 60)),
            );
        }
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
