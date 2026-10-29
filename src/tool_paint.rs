use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
use kurbo::Point;

use crate::canvas_tool::{ActiveCanvasDrag, Tool};
use crate::vector_boolean::{
    boolean_operation, erase_from_canvas, find_enclosed_region_at, merge_disjoint_fill_into_canvas,
    merge_shapes_into_canvas, BoolOp,
};
use crate::vector_engine::{
    append_mesh, rects_overlap, tessellate_raw_polygon, transform_mesh_to_screen, FillShape,
    MAX_VIEW_ZOOM, MIN_VIEW_ZOOM,
};
use crate::vector_stroke::{
    build_smoothed_stroke_shapes_at_zoom, generate_brush_outline_at_zoom,
};
use crate::Is2dApp;

/// ブラシ・消しゴムドラッグ中のリアルタイムプレビューにおいて、
/// 1フレームあたりに再テッセレーションする末尾ストローク点数の上限（これを超えた前半区間は `committed_mesh` に確定させて O(1) 更新を保証する）
const STROKE_TAIL_CHUNK_LEN: usize = 24;

// ============================================================================
// ブラシ（B）・消しゴム（E）・バケツ塗り（K）・スポイト（I）の操作ロジック
// ============================================================================

pub(crate) fn handle_brush_or_eraser_input(
    app: &mut Is2dApp,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    let base_r = if app.ui.tool == Tool::Brush {
        app.ui.brush_radius as f64
    } else {
        app.ui.eraser_radius as f64
    };
    let preview_color = if app.ui.tool == Tool::Brush {
        app.ui.brush_color
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 220)
    };
    let z = (app.viewport.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
    let screen_r = base_r * z;
    let min_sample_dist = (0.25 * screen_r).clamp(0.4, 1.5) / z;

    if response.drag_started() {
        if let Some(pt) = mouse_pt {
            let stroke = vec![pt];
            let tail_outline = generate_brush_outline_at_zoom(&stroke, base_r, z);
            let tail_mesh = tessellate_raw_polygon(&tail_outline, preview_color);
            app.canvas.drag = ActiveCanvasDrag::PaintStroke {
                stroke,
                committed_mesh: egui::Mesh::default(),
                tail_outline,
                tail_mesh,
                tail_start_idx: 0,
            };
        }
    } else if response.dragged() {
        if let (
            Some(pt),
            ActiveCanvasDrag::PaintStroke {
                stroke,
                committed_mesh,
                tail_outline,
                tail_mesh,
                tail_start_idx,
            },
        ) = (mouse_pt, &mut app.canvas.drag)
        {
            if stroke
                .last()
                .is_none_or(|&last| (pt - last).hypot() >= min_sample_dist)
            {
                if stroke.len().saturating_sub(*tail_start_idx) >= STROKE_TAIL_CHUNK_LEN {
                    append_mesh(committed_mesh, tail_mesh);
                    *tail_start_idx = stroke.len().saturating_sub(2);
                }
                stroke.push(pt);
                *tail_outline =
                    generate_brush_outline_at_zoom(&stroke[*tail_start_idx..], base_r, z);
                *tail_mesh = tessellate_raw_polygon(tail_outline, preview_color);
            }
        }
    } else if response.drag_stopped() {
        if let ActiveCanvasDrag::PaintStroke { stroke, .. } =
            std::mem::take(&mut app.canvas.drag)
        {
            let raw_outline = generate_brush_outline_at_zoom(&stroke, base_r, z);
            if !raw_outline.is_empty() {
                commit_raw_outline(app, &raw_outline);
            }
        }
    } else if response.clicked() {
        if let Some(pt) = mouse_pt {
            let raw = generate_brush_outline_at_zoom(&[pt], base_r, z);
            commit_raw_outline(app, &raw);
        }
    }
}

pub(crate) fn commit_raw_outline(app: &mut Is2dApp, raw: &[Point]) {
    app.mutate_canvas_with_undo(true, |app| {
        let target_lid = crate::canvas_tool::active_scope_layer_id(app);
        let stroke_shapes: Vec<FillShape> = build_smoothed_stroke_shapes_at_zoom(
            raw,
            app.ui.brush_color,
            app.viewport.zoom as f64,
        )
        .into_iter()
        .map(|s| s.with_scope(target_lid, &app.canvas.editing_group_path))
        .collect();
        if app.ui.tool == Tool::Brush {
            merge_shapes_into_canvas(&mut app.canvas.shapes, stroke_shapes);
            app.canvas.sort_shapes_by_layer_order(&app.timeline);
        } else {
            erase_from_canvas(&mut app.canvas.shapes, &stroke_shapes);
        }
    });
}

/// バケツツール（色流し込み）
pub(crate) fn bucket_fill_at(app: &mut Is2dApp, pt: Point) -> bool {
    let fill_color = app.ui.brush_color;

    // 0. 選択領域が存在する場合：選択中の「生シェイプ」のみにバケツで色を流し込む（グループは無効）
    if !app.canvas.selected_shapes.is_empty() {
        let clicked_selected_raw = app.canvas.selected_shapes.iter().copied().any(|idx| {
            app.canvas.shapes.get(idx).is_some_and(|s| {
                s.is_raw_at_scope(&app.canvas.editing_group_path) && s.contains_point(pt)
            })
        });
        if clicked_selected_raw {
            let any_color_diff = app.canvas.selected_shapes.iter().copied().any(|idx| {
                app.canvas.shapes.get(idx).is_some_and(|s| {
                    s.is_raw_at_scope(&app.canvas.editing_group_path) && s.color != fill_color
                })
            });
            if !any_color_diff {
                return false;
            }
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            for &sel_idx in &app.canvas.selected_shapes {
                if let Some(shape) = app.canvas.shapes.get_mut(sel_idx) {
                    if shape.is_raw_at_scope(&app.canvas.editing_group_path) {
                        shape.set_color(fill_color);
                    }
                }
            }
            app.canvas.selection_modified = true;
            app.commit_canvas_mutation(snapshot);
            return true;
        }
        return false;
    }

    // 全表示レイヤー（ロック中レイヤーやグループ・シンボルを含む）の解決済み図形を収集する
    let target_lid = crate::canvas_tool::active_scope_layer_id(app);
    let all_visible_shapes = collect_all_visible_shapes(app);

    // 1. 既存の生シェイプ上をクリックした場合：色の塗り替え（ただし別レイヤー・ロックレイヤー・グループの境界線で分割されている場合は分割領域の塗りを優先）
    if let Some(hit_idx) = app.canvas.topmost_shape_at(pt) {
        let hit_shape = &app.canvas.shapes[hit_idx];
        let hit_layer_id = hit_shape.layer_id;
        let hit_layer_ord = app.timeline.layer_index_by_id(hit_layer_id).unwrap_or(0);
        let occluded_by_locked_above = (hit_layer_ord + 1..app.timeline.layers.len()).any(|l_idx| {
            let layer = &app.timeline.layers[l_idx];
            layer.visible
                && layer.locked
                && all_visible_shapes
                    .iter()
                    .any(|s| s.layer_id == layer.id && s.contains_point(pt))
        });

        if !occluded_by_locked_above && hit_shape.is_raw_at_scope(&app.canvas.editing_group_path) {
            if hit_shape.color == fill_color {
                return false;
            }
            // 別レイヤーやグループ・シンボルの線がこの生シェイプを横切って分割している場合は、分割された部分領域のみを塗る
            let hit_bbox = hit_shape.bounding_box();
            let overlapping_walls: Vec<FillShape> = all_visible_shapes
                .iter()
                .filter(|s| {
                    (s.layer_id != hit_layer_id
                        || !s.is_raw_at_scope(&app.canvas.editing_group_path))
                        && !s.contains_point(pt)
                        && rects_overlap(s.bounding_box(), hit_bbox)
                })
                .cloned()
                .collect();
            if !overlapping_walls.is_empty() {
                let pieces = boolean_operation(
                    std::slice::from_ref(hit_shape),
                    &overlapping_walls,
                    BoolOp::Difference,
                    fill_color,
                );
                if pieces.len() > 1 {
                    if let Some(split_piece) = pieces.into_iter().find(|p| p.contains_point(pt)) {
                        let snapshot = app.canvas.shapes.clone();
                        merge_shapes_into_canvas(
                            &mut app.canvas.shapes,
                            vec![split_piece.with_scope(hit_layer_id, &app.canvas.editing_group_path)],
                        );
                        app.canvas.sort_shapes_by_layer_order(&app.timeline);
                        app.commit_canvas_mutation(snapshot);
                        return true;
                    }
                }
            }

            crate::canvas_tool::focus_active_layer_to_shape(app, hit_idx);
            let snapshot = app.canvas.shapes.clone();
            let mut target = app.canvas.shapes.remove(hit_idx);
            target.set_color(fill_color);
            merge_disjoint_fill_into_canvas(&mut app.canvas.shapes, target);
            app.canvas.sort_shapes_by_layer_order(&app.timeline);
            app.commit_canvas_mutation(snapshot);
            return true;
        }
        if occluded_by_locked_above || hit_layer_id == target_lid {
            return false;
        }
    }

    // 2. 全表示レイヤー（ロック中レイヤー・グループ・シンボルを含む）の境界線に囲まれた閉領域をクリックした場合：ベクター流し込み
    // （クリック地点自体を覆っている背面レイヤーのベタ塗り図形は壁から除外し、周囲の線画・輪郭を壁として使用する）
    let wall_shapes: Vec<FillShape> = all_visible_shapes
        .iter()
        .filter(|s| !s.contains_point(pt))
        .cloned()
        .collect();
    let enclosed_opt = find_enclosed_region_at(&wall_shapes, pt, fill_color).or_else(|| {
        let active_layer_raw: Vec<FillShape> = app
            .canvas
            .shapes
            .iter()
            .filter(|s| {
                s.layer_id == target_lid
                    && s.is_raw_at_scope(&app.canvas.editing_group_path)
                    && !s.contains_point(pt)
            })
            .cloned()
            .collect();
        (active_layer_raw.len() < wall_shapes.len())
            .then(|| find_enclosed_region_at(&active_layer_raw, pt, fill_color))
            .flatten()
    });
    if let Some(enclosed_fill) = enclosed_opt {
        let snapshot = app.canvas.shapes.clone();
        merge_disjoint_fill_into_canvas(
            &mut app.canvas.shapes,
            enclosed_fill.with_scope(target_lid, &app.canvas.editing_group_path),
        );
        app.canvas.sort_shapes_by_layer_order(&app.timeline);
        app.commit_canvas_mutation(snapshot);
        return true;
    }

    false
}

/// 現在表示されているすべてのレイヤー（ロック中レイヤーを含む）の現在フレーム解決済み図形を背面→前面順で収集する
fn collect_all_visible_shapes(app: &Is2dApp) -> Vec<FillShape> {
    let mut out = Vec::with_capacity(app.canvas.shapes.len());
    for (layer_idx, layer) in app.timeline.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        if layer.locked {
            out.extend(app.timeline.evaluate_layer_shapes_at(
                layer_idx,
                app.timeline.current_frame,
                &app.symbols.library,
            ));
        } else {
            let lid = layer.id;
            out.extend(
                app.canvas
                    .shapes
                    .iter()
                    .filter(|s| s.layer_id == lid)
                    .cloned(),
            );
        }
    }
    out
}

/// 編集スコープおよび表示中の全レイヤーを対象に、指定座標 `pt` にヒットする最前面シェイプの色を返す（スポイト用）
pub(crate) fn topmost_visible_color_at(app: &Is2dApp, pt: Point) -> Option<Color32> {
    let num_layers = app.timeline.layers.len();
    if num_layers == 1
        && app
            .timeline
            .layers
            .first()
            .is_some_and(|l| l.visible && !l.locked)
    {
        return app
            .canvas
            .shapes
            .iter()
            .rev()
            .find(|s| s.contains_point(pt))
            .map(|s| s.color);
    }
    for layer_idx in (0..num_layers).rev() {
        let layer = &app.timeline.layers[layer_idx];
        if !layer.visible {
            continue;
        }
        if layer.locked {
            let evaluated = app.timeline.evaluate_layer_shapes_at(
                layer_idx,
                app.timeline.current_frame,
                &app.symbols.library,
            );
            if let Some(hit) = evaluated.iter().rev().find(|s| s.contains_point(pt)) {
                return Some(hit.color);
            }
        } else {
            let lid = layer.id;
            if let Some(hit) = app
                .canvas
                .shapes
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

/// スポイトツール（色抽出）：指定位置 `pt` の最前面シェイプの色を抽出し、`app.ui.brush_color` に設定する
pub(crate) fn pick_color_at(app: &mut Is2dApp, pt: Point) -> Option<Color32> {
    let picked = topmost_visible_color_at(app, pt)?;
    app.ui.brush_color = picked;
    Some(picked)
}

/// 進行中のブラシ・消しゴムストロークの増分プレビューメッシュを描画する
pub(crate) fn draw_active_stroke_preview(
    app: &Is2dApp,
    artwork_painter: &egui::Painter,
    gizmo_painter: &egui::Painter,
) {
    let ActiveCanvasDrag::PaintStroke {
        committed_mesh,
        tail_outline,
        tail_mesh,
        ..
    } = &app.canvas.drag
    else {
        return;
    };
    if tail_outline.is_empty() && committed_mesh.is_empty() {
        return;
    }
    if !committed_mesh.is_empty() {
        artwork_painter.add(egui::Shape::Mesh(transform_mesh_to_screen(
            committed_mesh,
            app.viewport.zoom,
            app.viewport.pan,
            false,
        )));
    }
    if !tail_mesh.is_empty() {
        artwork_painter.add(egui::Shape::Mesh(transform_mesh_to_screen(
            tail_mesh,
            app.viewport.zoom,
            app.viewport.pan,
            false,
        )));
    }

    if app.ui.tool == Tool::Eraser {
        let n = tail_outline.len();
        for i in 0..n {
            gizmo_painter.line_segment(
                [
                    app.viewport.world_to_screen(tail_outline[i]),
                    app.viewport.world_to_screen(tail_outline[(i + 1) % n]),
                ],
                Stroke::new(1.2, Color32::from_rgb(220, 60, 60)),
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
    painter.circle_stroke(cursor, 2.5, Stroke::new(1.2, Color32::WHITE));

    // 右下に抽出色プレビューバッジを表示
    let badge = cursor + Vec2::new(16.0, 16.0);
    let r = if is_over_shape { 9.5 } else { 7.5 };
    painter.circle_filled(badge, r, sample_color);
    painter.circle_stroke(badge, r, Stroke::new(2.2, Color32::WHITE));
    painter.circle_stroke(
        badge,
        r + 1.2,
        Stroke::new(
            1.2,
            if is_over_shape {
                Color32::from_rgb(0, 145, 255)
            } else {
                Color32::from_rgb(30, 35, 45)
            },
        ),
    );
}
