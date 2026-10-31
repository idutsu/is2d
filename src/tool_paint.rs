use eframe::egui::{self, Color32, PointerButton, Pos2, Stroke, Vec2};
use kurbo::Point;

use crate::canvas_tool::{ActiveCanvasDrag, EditorDoc, Tool};
use crate::vector_boolean::{
    boolean_operation, erase_from_canvas, find_enclosed_region_at, merge_disjoint_fill_into_canvas,
    merge_shapes_into_canvas, BoolOp,
};
use crate::vector_engine::{
    append_mesh, rects_overlap, tessellate_raw_polygon, transform_mesh_to_screen, FillShape,
    MAX_VIEW_ZOOM, MIN_VIEW_ZOOM,
};
use crate::vector_stroke::{build_smoothed_stroke_shapes_at_zoom, generate_brush_outline_at_zoom};

/// ブラシ・消しゴムドラッグ中のリアルタイムプレビューにおいて、
/// 1フレームあたりに再テッセレーションする末尾ストローク点数の上限（これを超えた前半区間は `committed_mesh` に確定させて O(1) 更新を保証する）
const STROKE_TAIL_CHUNK_LEN: usize = 24;

// ============================================================================
// ブラシ（B）・消しゴム（E）・バケツ塗り（K）・スポイト（I）の操作ロジック
// ============================================================================

pub(crate) fn handle_brush_or_eraser_input(
    app: &mut EditorDoc,
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

    if response.drag_started_by(PointerButton::Primary) {
        let press_pt = response
            .ctx
            .input(|i| i.pointer.press_origin())
            .map(|p| app.viewport.screen_to_world(p));
        if let Some(start_pt) = press_pt.or(mouse_pt) {
            let mut stroke = vec![start_pt];
            if let Some(curr_pt) = mouse_pt {
                if (curr_pt - start_pt).hypot() >= min_sample_dist {
                    stroke.push(curr_pt);
                }
            }
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
    } else if response.dragged_by(PointerButton::Primary) {
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
                    let screen_chunk = transform_mesh_to_screen(
                        tail_mesh,
                        app.viewport.zoom,
                        app.viewport.pan,
                        false,
                    );
                    append_mesh(committed_mesh, &screen_chunk);
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
        if let ActiveCanvasDrag::PaintStroke { stroke, .. } = std::mem::take(&mut app.canvas.drag) {
            let raw_outline = generate_brush_outline_at_zoom(&stroke, base_r, z);
            if !raw_outline.is_empty() {
                commit_raw_outline(app, &raw_outline);
            }
        }
    } else if response.clicked_by(PointerButton::Primary) {
        if let Some(pt) = mouse_pt {
            let raw = generate_brush_outline_at_zoom(&[pt], base_r, z);
            commit_raw_outline(app, &raw);
        }
    }
}

pub(crate) fn commit_raw_outline(app: &mut EditorDoc, raw: &[Point]) {
    let target_lid = crate::canvas_tool::active_scope_layer_id(&app.canvas, &app.timeline);
    if !app.timeline.is_layer_editable_by_id(target_lid) {
        return;
    }
    let stroke_shapes: Vec<FillShape> =
        build_smoothed_stroke_shapes_at_zoom(raw, app.ui.brush_color, app.viewport.zoom as f64)
            .into_iter()
            .map(|s| s.with_scope(target_lid, &app.canvas.editing_group_path))
            .collect();
    if stroke_shapes.is_empty() {
        return;
    }
    if app.ui.tool == Tool::Brush {
        app.mutate_canvas_with_undo(true, |app| {
            merge_shapes_into_canvas(&mut app.canvas.shapes, stroke_shapes);
            app.canvas.sort_shapes_by_layer_order(&app.timeline);
        });
    } else {
        let has_bbox_overlap = app.canvas.shapes.iter().any(|s| {
            s.layer_id == target_lid
                && s.is_raw_at_scope(&app.canvas.editing_group_path)
                && stroke_shapes
                    .iter()
                    .any(|er| rects_overlap(s.bounding_box(), er.bounding_box()))
        });
        if !has_bbox_overlap {
            return;
        }
        crate::canvas_tool::clear_selection(app);
        let prev_revs: Vec<(u64, u64)> = app
            .canvas
            .shapes
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
        erase_from_canvas(
            &mut app.canvas.shapes,
            &stroke_shapes,
            target_lid,
            &app.canvas.editing_group_path,
        );
        let unchanged = app.canvas.shapes.len() == prev_revs.len()
            && app
                .canvas
                .shapes
                .iter()
                .zip(prev_revs.iter())
                .all(|(s, &(id, rev))| s.shape_id == id && s.mesh_revision == rev);
        if !unchanged {
            app.commit_canvas_mutation(snapshot);
        }
    }
}

/// バケツツール（色流し込み）
pub(crate) fn bucket_fill_at(app: &mut EditorDoc, pt: Point) -> bool {
    let fill_color = app.ui.brush_color;
    let target_lid = crate::canvas_tool::active_scope_layer_id(&app.canvas, &app.timeline);
    if !app.timeline.is_layer_editable_by_id(target_lid) {
        return false;
    }

    // 0. 選択領域が存在する場合：選択中の「生シェイプ」のみにバケツで色を流し込む（グループは無効）
    if !app.canvas.selected_shapes.is_empty() {
        let clicked_selected_raw = app.canvas.selected_shapes.iter().copied().any(|idx| {
            app.canvas.shapes.get(idx).is_some_and(|s| {
                app.timeline.is_layer_editable_by_id(s.layer_id)
                    && s.is_raw_at_scope(&app.canvas.editing_group_path)
                    && s.contains_point(pt)
            })
        });
        if clicked_selected_raw {
            let any_color_diff = app.canvas.selected_shapes.iter().copied().any(|idx| {
                app.canvas.shapes.get(idx).is_some_and(|s| {
                    app.timeline.is_layer_editable_by_id(s.layer_id)
                        && s.is_raw_at_scope(&app.canvas.editing_group_path)
                        && s.color != fill_color
                })
            });
            if !any_color_diff {
                return false;
            }
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            for &sel_idx in &app.canvas.selected_shapes {
                if let Some(shape) = app.canvas.shapes.get_mut(sel_idx) {
                    if app.timeline.is_layer_editable_by_id(shape.layer_id)
                        && shape.is_raw_at_scope(&app.canvas.editing_group_path)
                    {
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
    let all_visible_shapes = collect_all_visible_shapes(app);
    let target_layer_ord = app.timeline.layer_index_by_id(target_lid).unwrap_or(0);

    // 1. アクティブレイヤー（target_lid）の既存の生シェイプ上をクリックした場合：色の塗り替え
    // （ただし別レイヤー・ロックレイヤー・グループの境界線で分割されている場合は分割領域の塗りを優先）
    if let Some(hit_idx) = (0..app.canvas.shapes.len()).rev().find(|&idx| {
        let s = &app.canvas.shapes[idx];
        s.layer_id == target_lid
            && s.is_in_editing_scope(&app.canvas.editing_group_path)
            && s.contains_point(pt)
    }) {
        let occluded_above = (target_layer_ord + 1..app.timeline.layers.len()).any(|l_idx| {
            let layer = &app.timeline.layers[l_idx];
            layer.visible
                && all_visible_shapes
                    .iter()
                    .any(|s| s.layer_id == layer.id && s.contains_point(pt))
        });

        let hit_shape = &app.canvas.shapes[hit_idx];
        if !occluded_above && hit_shape.is_raw_at_scope(&app.canvas.editing_group_path) {
            if hit_shape.color == fill_color {
                return false;
            }
            // 別レイヤーやグループ・シンボルの線がこの生シェイプを横切って分割している場合は、分割された部分領域のみを塗る
            let hit_bbox = hit_shape.bounding_box();
            let overlapping_walls: Vec<FillShape> = all_visible_shapes
                .iter()
                .filter(|s| {
                    (s.layer_id != target_lid
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
                        let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
                        merge_shapes_into_canvas(
                            &mut app.canvas.shapes,
                            vec![split_piece
                                .with_scope(target_lid, &app.canvas.editing_group_path)],
                        );
                        app.canvas.sort_shapes_by_layer_order(&app.timeline);
                        app.commit_canvas_mutation(snapshot);
                        return true;
                    }
                }
            }

            crate::canvas_tool::focus_active_layer_to_shape(app, hit_idx);
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            let mut target = app.canvas.shapes.remove(hit_idx);
            target.set_color(fill_color);
            merge_disjoint_fill_into_canvas(&mut app.canvas.shapes, target);
            app.canvas.sort_shapes_by_layer_order(&app.timeline);
            app.commit_canvas_mutation(snapshot);
            return true;
        }
        return false;
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
        let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
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
fn collect_all_visible_shapes(app: &EditorDoc) -> Vec<FillShape> {
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

/// ロックされたレイヤー（またはシンボル内部タイムライン）上の指定フレーム・指定座標 `pt` にヒットする最前面図形の色を、
/// 図形クローンやメッシュ再構築を行わずに逆アフィン変換で高速に判定する
fn locked_layer_color_at(
    layer: &crate::timeline::Layer,
    frame: usize,
    pt: Point,
    library: &[crate::symbol::SymbolDef],
    depth: usize,
) -> Option<Color32> {
    if depth >= crate::symbol::MAX_SYMBOL_NEST_DEPTH || !layer.visible || frame >= layer.frame_count {
        return None;
    }
    let kf_idx = layer.keyframe_index_at(frame)?;
    let kf = &layer.keyframes[kf_idx];
    if let Some((_, binding)) = layer.interpolated_tween_binding_at(frame) {
        return symbol_binding_color_at(binding, kf.start_frame, frame, pt, library, depth);
    }
    let cel = &kf.cel;
    if cel.instances.is_empty() {
        return cel.shapes.iter().rev().find(|s| s.contains_point(pt)).map(|s| s.color);
    }
    // `shape_order_index` の逆順（最前面→最背面）にインスタンスと生シェイプを検査
    let mut shape_idx = cel.shapes.len();
    for inst in cel.instances.iter().rev() {
        let target_ord = inst.shape_order_index.min(cel.shapes.len());
        while shape_idx > target_ord {
            shape_idx -= 1;
            if cel.shapes[shape_idx].contains_point(pt) {
                return Some(cel.shapes[shape_idx].color);
            }
        }
        if let Some(col) =
            symbol_binding_color_at(inst.binding, kf.start_frame, frame, pt, library, depth)
        {
            return Some(col);
        }
    }
    while shape_idx > 0 {
        shape_idx -= 1;
        if cel.shapes[shape_idx].contains_point(pt) {
            return Some(cel.shapes[shape_idx].color);
        }
    }
    None
}

fn symbol_binding_color_at(
    binding: crate::symbol::SymbolInstanceBinding,
    kf_start_frame: usize,
    parent_frame: usize,
    world_pt: Point,
    library: &[crate::symbol::SymbolDef],
    depth: usize,
) -> Option<Color32> {
    if depth >= crate::symbol::MAX_SYMBOL_NEST_DEPTH
        || binding.xform.determinant().abs() <= 1e-9
    {
        return None;
    }
    let sym = library.iter().find(|s| s.id == binding.symbol_id)?;
    let internal_frame = sym.resolve_internal_frame(binding, kf_start_frame, parent_frame);
    let local_pt = binding.xform.inverse() * world_pt;
    for l_idx in (0..sym.timeline.layers.len()).rev() {
        let layer = &sym.timeline.layers[l_idx];
        if let Some(col) = locked_layer_color_at(layer, internal_frame, local_pt, library, depth + 1)
        {
            return Some(col);
        }
    }
    None
}

/// 編集スコープおよび表示中の全レイヤーを対象に、指定座標 `pt` にヒットする最前面シェイプの色を返す（スポイト用）
pub(crate) fn topmost_visible_color_at(app: &EditorDoc, pt: Point) -> Option<Color32> {
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
            let cf = app.timeline.current_frame;
            if let Some(col) = locked_layer_color_at(layer, cf, pt, &app.symbols.library, 0) {
                return Some(col);
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
pub(crate) fn pick_color_at(app: &mut EditorDoc, pt: Point) -> Option<Color32> {
    let picked = topmost_visible_color_at(app, pt)?;
    app.ui.brush_color = picked;
    Some(picked)
}

/// 進行中のブラシ・消しゴムストロークの増分プレビューメッシュを描画する
pub(crate) fn draw_active_stroke_preview(
    app: &EditorDoc,
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
        artwork_painter.add(egui::Shape::Mesh(committed_mesh.clone()));
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
