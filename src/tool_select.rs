use std::collections::HashSet;

use eframe::egui::{self, Color32, CursorIcon, Pos2, Stroke, Vec2};
use kurbo::{ParamCurve, ParamCurveNearest, Point, QuadBez, Rect as KRect, Vec2 as KVec2};

use crate::canvas_tool::{ActiveCanvasDrag, CanvasState, MarqueeDrag, MoveSelectionDrag, Tool};
use crate::vector_boolean::{
    boolean_operation, merge_shapes_into_canvas, resolve_deformed_shape,
    slice_shape_with_marquee_rect, BoolOp,
};
use crate::vector_engine::{
    arc_mesh_to_shape, make_marquee_rect_shape, next_shape_id, normalize_vec, rects_overlap,
    to_pos2, FillShape,
};
use crate::Is2dApp;

// ============================================================================
// 選択ツール（Tool::Select / V キー）・輪郭変形・アンカー編集・グループ・クリップボード
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SelectSubMode {
    Reshape,      // 通常の輪郭変形・頂点移動・図形移動（V キー）
    AddAnchor,    // ＋ アンカー追加モード（+ キー）
    DeleteAnchor, // － アンカー削除モード（- キー）
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) enum HoverTarget {
    #[default]
    None,
    Vertex {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        pos: Point,
    },
    Edge {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        t: f64,
        closest_pos: Point,
    },
    Fill {
        shape_idx: usize,
    },
}

impl HoverTarget {
    pub(crate) fn shape_idx(&self) -> Option<usize> {
        match *self {
            HoverTarget::Vertex { shape_idx, .. }
            | HoverTarget::Edge { shape_idx, .. }
            | HoverTarget::Fill { shape_idx } => Some(shape_idx),
            HoverTarget::None => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SelectDragState {
    MoveShape(MoveSelectionDrag),
    MarqueeSelect(MarqueeDrag),
    PullEdge {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        grab_t: f64,
        grab_pos: Point,
        last_pos: Point,
        orig_seg: QuadBez,
        undo_snapshot: Vec<FillShape>,
    },
    MoveVertex {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        orig_vertex: Point,
        last_pos: Point,
        orig_prev_ctrl: Point,
        orig_curr_ctrl: Point,
        snapped_to: Option<Point>,
        undo_snapshot: Vec<FillShape>,
    },
}

/// 向こう側の輪郭エッジ付近でドロップされた際に確実に貫通して分割されるよう僅かに押し出す。
pub(crate) fn punch_through_opposite_edge(
    shape: &FillShape,
    contour_idx: usize,
    ignore_segs: &[usize],
    (anchor_a, anchor_b): (Point, Point),
    (start_pt, drag_pt): (Point, Point),
) -> Point {
    let drag_vec = drag_pt - start_pt;
    if drag_vec.hypot() <= 6.0
        || (drag_pt - anchor_a).hypot() <= 10.0
        || (drag_pt - anchor_b).hypot() <= 10.0
    {
        return drag_pt;
    }

    let drag_dir = normalize_vec(drag_vec);
    let mut best_near: Option<(f64, Point)> = None;
    for (c_idx, c) in shape.contours().enumerate() {
        for (s_idx, seg) in c.segments.iter().enumerate() {
            if (c_idx == contour_idx && ignore_segs.contains(&s_idx))
                || !quad_control_bbox_contains(seg, drag_pt, 6.0)
            {
                continue;
            }
            let near = seg.nearest(drag_pt, 0.05);
            let d = near.distance_sq.sqrt();
            if d <= 6.0 {
                let near_pt = seg.eval(near.t);
                if (near_pt - anchor_a).hypot() > 8.0
                    && (near_pt - anchor_b).hypot() > 8.0
                    && best_near.is_none_or(|(bd, _)| d < bd)
                {
                    best_near = Some((d, near_pt));
                }
            }
        }
    }
    if let Some((_, near_pt)) = best_near {
        let punched = near_pt + drag_dir * 4.5;
        if (punched - start_pt).dot(drag_dir) > (drag_pt - start_pt).dot(drag_dir) {
            return punched;
        }
    }
    drag_pt
}

/// エッジつまみ変形（PullEdge）の制御点を計算して反映する（リアルタイムプレビュー用）。
pub(crate) fn set_pulled_edge(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    grab_t: f64,
    orig_seg: QuadBez,
    target_pt: Point,
) {
    let grab_pos = orig_seg.eval(grab_t);
    let delta = target_pt - grab_pos;
    let denom = 2.0 * grab_t * (1.0 - grab_t);
    let new_p1 = orig_seg.p1 + delta / denom;
    if let Some(contour) = shape.get_contour_mut(contour_idx) {
        if seg_idx < contour.segments.len() {
            contour.segments_mut()[seg_idx].p1 = new_p1;
        }
    }
}

/// エッジつまみ変形（PullEdge）を対向エッジ貫通補正込みで確定適用する。
pub(crate) fn apply_pull_edge_to_shape(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    grab_t: f64,
    orig_seg: QuadBez,
    drag_pt: Point,
) {
    let grab_pos = orig_seg.eval(grab_t);
    let effective_pt = punch_through_opposite_edge(
        shape,
        contour_idx,
        &[seg_idx],
        (orig_seg.p0, orig_seg.p2),
        (grab_pos, drag_pt),
    );
    set_pulled_edge(
        shape,
        (contour_idx, seg_idx),
        grab_t,
        orig_seg,
        effective_pt,
    );
}

/// 頂点ドラッグ変形（MoveVertex）の頂点と前後制御点を計算して反映する（リアルタイムプレビュー用）。
pub(crate) fn set_moved_vertex(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    orig_vertex: Point,
    orig_ctrls: (Point, Point),
    target_pt: Point,
) {
    let delta = target_pt - orig_vertex;
    if let Some(contour) = shape.get_contour_mut(contour_idx) {
        let n = contour.segments.len();
        if n > 0 && seg_idx < n {
            let prev_idx = (seg_idx + n - 1) % n;
            let segs = contour.segments_mut();
            segs[prev_idx].p2 = target_pt;
            segs[seg_idx].p0 = target_pt;
            segs[prev_idx].p1 = orig_ctrls.0 + delta * 0.5;
            segs[seg_idx].p1 = orig_ctrls.1 + delta * 0.5;
        }
    }
}

/// 頂点ドラッグ変形（MoveVertex）を対向エッジ貫通補正込みで確定適用する。
pub(crate) fn apply_move_vertex_to_shape(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    orig_vertex: Point,
    orig_ctrls: (Point, Point),
    drag_pt: Point,
) {
    let Some((prev_idx, prev_p0, curr_p2)) = shape.get_contour(contour_idx).and_then(|c| {
        let n = c.segments.len();
        if n == 0 || seg_idx >= n {
            return None;
        }
        let prev_idx = (seg_idx + n - 1) % n;
        Some((prev_idx, c.segments[prev_idx].p0, c.segments[seg_idx].p2))
    }) else {
        return;
    };

    let effective_pt = punch_through_opposite_edge(
        shape,
        contour_idx,
        &[prev_idx, seg_idx],
        (prev_p0, curr_p2),
        (orig_vertex, drag_pt),
    );
    set_moved_vertex(
        shape,
        (contour_idx, seg_idx),
        orig_vertex,
        orig_ctrls,
        effective_pt,
    );
}

// ------------------------------------------------------------------------
// 2. 選択管理・グループ階層操作・クリップボード
// ------------------------------------------------------------------------

/// `selection_modified == true` の選択中生シェイプを同一レイヤー・同一スコープのキャンバス図形へ結合・切り抜き（Merge Drawing）確定し、
/// タイムラインへ同期する（選択状態そのもののクリアは呼び出し側で行う）
pub(crate) fn flush_modified_raw_selection_merge(app: &mut Is2dApp) {
    if !app.canvas.selection_modified || app.canvas.selected_shapes.is_empty() {
        app.canvas.selection_modified = false;
        return;
    }
    let (is_sel, _) = app.canvas.selected_shape_mask();
    let mut base_canvas = Vec::with_capacity(app.canvas.shapes.len());
    let mut modified_raw_shapes = Vec::with_capacity(app.canvas.selected_shapes.len());
    for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
        // 現在のスコープ直下の生シェイプのみ同一レイヤー内で結合・切り抜き（Merge Drawing）を行い、
        // グループ化されているオブジェクトや別レイヤーの絵は削らずにそのまま残す
        if is_sel[idx] && shape.is_raw_at_scope(&app.canvas.editing_group_path) {
            modified_raw_shapes.push(shape);
        } else {
            base_canvas.push(shape);
        }
    }
    merge_shapes_into_canvas(&mut base_canvas, modified_raw_shapes);
    app.canvas.shapes = base_canvas;
    app.canvas.sort_shapes_by_layer_order(&app.timeline);
    crate::canvas_tool::sync_shapes_to_timeline(app);
    app.canvas.selection_modified = false;
}

pub(crate) fn clear_selection(app: &mut Is2dApp) {
    crate::canvas_tool::rollback_shape_drag(app);
    if let Some(unsliced) = app.canvas.take_unsliced_snapshot() {
        let selected_ids = app.canvas.selected_shape_ids();
        app.canvas.shapes = unsliced;
        app.canvas.restore_selection_by_shape_ids(&selected_ids);
    }
    if app.canvas.selection_modified && !app.canvas.selected_shapes.is_empty() {
        flush_modified_raw_selection_merge(app);
    }
    app.canvas.selection_modified = false;
    app.canvas.selected_shapes.clear();
    app.canvas.transform_pivot = None;
    app.ui.inspect_timeline_frame = false;
}

/// 選択中のオブジェクト（生シェイプ・既存グループ問わず）を新しい1つのグループにまとめる（`Cmd+G` / 無限入れ子対応）
pub(crate) fn group_selected_shapes(app: &mut Is2dApp) -> Option<u64> {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    if app.canvas.selected_shapes.is_empty() {
        return None;
    }
    let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
    let new_gid = app.symbols.alloc_group_id();

    let target_lid =
        crate::canvas_tool::topmost_layer_id_for_indices(app, &app.canvas.selected_shapes);

    let depth = app.canvas.editing_group_path.len();
    let mut any_grouped = false;
    for &sel_idx in &app.canvas.selected_shapes {
        if let Some(shape) = app.canvas.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&app.canvas.editing_group_path) {
                shape.layer_id = target_lid;
                shape.group_path.insert(depth, new_gid);
                any_grouped = true;
            }
        }
    }
    if !any_grouped {
        app.canvas.restore_unsliced_snapshot(snapshot);
        return None;
    }

    if let Some(l_idx) = app.timeline.layer_index_by_id(target_lid) {
        app.timeline.active_layer = l_idx;
    }
    commit_moved_selection(app, snapshot);
    app.canvas.selection_modified = false;
    app.canvas.transform_pivot = None;
    Some(new_gid)
}

/// 選択中のグループまたはシンボルインスタンスを1段階だけ解除（分離）する（`Cmd+B`）。
/// 多段入れ子の場合は内側の子グループ構造を維持したまま最外殻のグループのみを外す。
pub(crate) fn ungroup_selected_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    if !app.canvas.selection_has_group() {
        return false;
    }
    let snapshot = app.canvas.take_or_clone_unsliced_snapshot();

    let depth = app.canvas.editing_group_path.len();
    let mut any_became_raw = false;
    for &sel_idx in &app.canvas.selected_shapes {
        if let Some(shape) = app.canvas.shapes.get_mut(sel_idx) {
            if shape.is_in_editing_scope(&app.canvas.editing_group_path)
                && shape.group_path.len() > depth
            {
                let removed_gid = shape.group_path.remove(depth);
                if shape
                    .symbol_instance
                    .is_some_and(|b| b.instance_gid == removed_gid)
                {
                    shape.symbol_instance = None;
                }
                if shape.group_path.len() == depth && !shape.exterior.segments.is_empty() {
                    any_became_raw = true;
                }
            }
        }
    }
    let (mut is_sel, _) = app.canvas.selected_shape_mask();
    let mut write_idx = 0;
    for read_idx in 0..app.canvas.shapes.len() {
        let keep = !app.canvas.shapes[read_idx].exterior.segments.is_empty()
            || app.canvas.shapes[read_idx].symbol_instance.is_some();
        if keep {
            if write_idx != read_idx {
                app.canvas.shapes.swap(write_idx, read_idx);
                is_sel[write_idx] = is_sel[read_idx];
            }
            write_idx += 1;
        }
    }
    if write_idx < app.canvas.shapes.len() {
        app.canvas.shapes.truncate(write_idx);
        app.canvas.selected_shapes = (0..write_idx).filter(|&i| is_sel[i]).collect();
    }
    if any_became_raw {
        app.canvas.selection_modified = true;
    }
    app.canvas.transform_pivot = None;
    app.commit_canvas_mutation(snapshot);
    true
}

/// ダブルクリック位置 `pt` にシンボルがあればそのシンボルの独自タイムラインへ入り、グループがあればインプレイス編集モードへ移行する
pub(crate) fn enter_group_at(app: &mut Is2dApp, pt: Point) -> bool {
    let Some(hit_idx) = app.canvas.topmost_shape_at(pt) else {
        return false;
    };
    let Some(gid) =
        app.canvas.shapes[hit_idx].active_group_id_at_scope(&app.canvas.editing_group_path)
    else {
        return false;
    };
    if let Some(binding) = app.canvas.shapes[hit_idx]
        .symbol_instance
        .filter(|b| b.instance_gid == gid)
    {
        return crate::tool_transform::enter_symbol_timeline(app, binding);
    }
    clear_selection(app);
    app.canvas.editing_group_path.push(gid);
    true
}

/// 現在入っているグループまたはシンボルタイムライン編集階層から1段階上の親階層へ戻る
pub(crate) fn exit_current_group(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    if !app.canvas.editing_group_path.is_empty() {
        clear_selection(app);
        let Some(exited_gid) = app.canvas.editing_group_path.pop() else {
            return false;
        };
        app.canvas.selected_shapes = (0..app.canvas.shapes.len())
            .filter(|&i| {
                app.canvas.shapes[i].active_group_id_at_scope(&app.canvas.editing_group_path)
                    == Some(exited_gid)
            })
            .collect();
        app.canvas.transform_pivot = None;
        return true;
    }
    crate::tool_transform::exit_symbol_timeline(app)
}

/// Shift クリック等で指定シェイプ（またはグループ塊全体）の選択状態をトグルし、変形ピボットをリセットする
pub(crate) fn toggle_shape_selection(app: &mut Is2dApp, shape_idx: usize) {
    let unit = app.canvas.expand_to_group_unit(shape_idx);
    let (sel_mask, _) = app.canvas.selected_shape_mask();
    if sel_mask.get(shape_idx).copied().unwrap_or(false) {
        app.canvas
            .selected_shapes
            .retain(|i| unit.binary_search(i).is_err());
        if app.canvas.selected_shapes.is_empty() && !app.canvas.selection_modified {
            if let Some(unsliced) = app.canvas.take_unsliced_snapshot() {
                app.canvas.shapes = unsliced;
            }
        }
    } else {
        for idx in unit {
            if !sel_mask.get(idx).copied().unwrap_or(false) {
                app.canvas.selected_shapes.push(idx);
            }
        }
    }
    app.canvas.transform_pivot = None;
    crate::canvas_tool::sync_timeline_layer_from_canvas_selection(app);
}

/// 既存の選択を確定解除した上で、指定座標 `pt` にある最前面シェイプ（塗り・境界線・頂点、またはグループ塊全体）を選択する
pub(crate) fn select_single_shape_at(app: &mut Is2dApp, pt: Point) -> bool {
    clear_selection(app);
    let zoom = app.viewport.zoom;
    let sub_mode = app.ui.select_sub_mode;
    if let Some(hit_idx) = app
        .canvas
        .topmost_shape_at(pt)
        .or_else(|| compute_hover_target(&app.canvas, pt, zoom, sub_mode).shape_idx())
    {
        app.canvas.selected_shapes = app.canvas.expand_to_group_unit(hit_idx);
        crate::canvas_tool::focus_active_layer_to_shape(app, hit_idx);
        crate::canvas_tool::sync_timeline_layer_from_canvas_selection(app);
        true
    } else {
        false
    }
}

/// 選択中のオブジェクトを十字キー等で `delta` だけ移動し、Undo 履歴記録・最前面整列・タイムライン同期を行う
pub(crate) fn nudge_selected_shapes(app: &mut Is2dApp, delta: KVec2) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    let (_, any_valid) = app.canvas.selected_shape_mask();
    if !any_valid {
        return false;
    }
    let undo_snapshot = app.canvas.snapshot_for_drag();
    app.canvas.translate_selected(delta);
    commit_moved_selection(app, undo_snapshot);
    true
}

/// Adobe Animate 風の矩形選択（Marquee Selection）
pub(crate) fn apply_marquee_selection(app: &mut Is2dApp, p0: Point, p1: Point, additive: bool) {
    let min_x = p0.x.min(p1.x);
    let max_x = p0.x.max(p1.x);
    let min_y = p0.y.min(p1.y);
    let max_y = p0.y.max(p1.y);

    let min_marquee = 2.0 / (app.viewport.zoom as f64);
    if (max_x - min_x) < min_marquee || (max_y - min_y) < min_marquee {
        if !additive {
            clear_selection(app);
        }
        return;
    }

    if !additive {
        clear_selection(app);
    } else if app.canvas.selection_modified && !app.canvas.selected_shapes.is_empty() {
        let selected_ids = app.canvas.selected_shape_ids();
        flush_modified_raw_selection_merge(app);
        app.canvas.restore_selection_by_shape_ids(&selected_ids);
    }

    let rect = KRect::new(min_x, min_y, max_x, max_y);
    let clip_rect_shape = make_marquee_rect_shape(rect);
    let (was_selected, _) = if additive {
        app.canvas.selected_shape_mask()
    } else {
        (vec![false; app.canvas.shapes.len()], false)
    };
    let hit_group_ids = collect_marquee_hit_groups(&app.canvas, rect, &clip_rect_shape);
    let pre_slice_snapshot =
        (!app.canvas.has_unsliced_snapshot()).then(|| app.canvas.shapes.clone());

    let mut any_sliced = false;
    let mut new_shapes = Vec::with_capacity(app.canvas.shapes.len() + 4);
    let mut new_selected = Vec::new();

    for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
        if was_selected[idx] {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
            continue;
        }
        if !shape.is_in_editing_scope(&app.canvas.editing_group_path) {
            new_shapes.push(shape);
            continue;
        }
        if let Some(gid) = shape.active_group_id_at_scope(&app.canvas.editing_group_path) {
            if hit_group_ids.contains(&gid) {
                new_selected.push(new_shapes.len());
            }
            new_shapes.push(shape);
            continue;
        }
        let bbox = shape.bounding_box();
        if !rects_overlap(bbox, rect) {
            new_shapes.push(shape);
            continue;
        }
        if bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1 {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
            continue;
        }

        let lid = shape.layer_id;
        let gp = shape.group_path.clone();
        let (inside, outside) = slice_shape_with_marquee_rect(&shape, &clip_rect_shape);
        if inside.is_empty() {
            new_shapes.push(shape);
        } else if outside.is_empty() {
            new_selected.push(new_shapes.len());
            new_shapes.push(shape);
        } else {
            any_sliced = true;
            new_shapes.extend(outside.into_iter().map(|s| s.with_scope(lid, &gp)));
            for piece in inside {
                new_selected.push(new_shapes.len());
                new_shapes.push(piece.with_scope(lid, &gp));
            }
        }
    }

    if any_sliced && !app.canvas.has_unsliced_snapshot() {
        if let Some(snap) = pre_slice_snapshot {
            app.canvas.restore_unsliced_snapshot(snap);
        }
    }
    app.canvas.shapes = new_shapes;
    app.canvas.selected_shapes = new_selected;
    if app.canvas.selected_shapes.is_empty() && !app.canvas.selection_modified {
        if let Some(unsliced) = app.canvas.take_unsliced_snapshot() {
            app.canvas.shapes = unsliced;
        }
    }
    app.canvas.transform_pivot = None;
    crate::canvas_tool::sync_timeline_layer_from_canvas_selection(app);
}

pub(crate) fn delete_selected_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    if app.canvas.selected_shapes.is_empty() {
        return false;
    }
    let (is_sel, any_valid) = app.canvas.selected_shape_mask();
    if !any_valid {
        app.canvas.selected_shapes.clear();
        app.canvas.clear_unsliced_snapshot();
        app.canvas.transform_pivot = None;
        return false;
    }

    let snapshot = app.canvas.take_or_clone_unsliced_snapshot();

    let mut remaining = Vec::with_capacity(app.canvas.shapes.len());
    for (idx, shape) in app.canvas.shapes.drain(..).enumerate() {
        if !is_sel[idx] {
            remaining.push(shape);
        }
    }
    app.canvas.shapes = remaining;
    app.canvas.selection_modified = false;
    app.canvas.selected_shapes.clear();
    app.canvas.transform_pivot = None;
    app.commit_canvas_mutation(snapshot);
    true
}

/// 現在の編集スコープ（グループ編集時はそのグループ内、通常時は現在フレームの全編集可能レイヤー）の図形をすべて消去する
pub(crate) fn clear_active_frame_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    let has_any_in_scope = app
        .canvas
        .shapes
        .iter()
        .any(|s| s.is_in_editing_scope(&app.canvas.editing_group_path));
    if !has_any_in_scope {
        return false;
    }
    app.mutate_canvas_with_undo(true, |app| {
        app.canvas
            .shapes
            .retain(|s| !s.is_in_editing_scope(&app.canvas.editing_group_path));
    });
    true
}

/// 相対グループパスを持つシェイプ群に対し、新しいグループID・シンボルインスタンスIDを採番して現在の編集スコープ用に変換する
fn instantiate_relative_shapes_in_scope(
    app: &mut Is2dApp,
    rel_shapes: &[FillShape],
    offset: KVec2,
    preserve_original_layer: bool,
) -> (Vec<FillShape>, bool) {
    let active_lid = crate::canvas_tool::active_scope_layer_id(app);
    let mut gid_map: Vec<(u64, u64)> = Vec::new();
    let mut seen_instance_gids: HashSet<u64> = HashSet::new();
    let mut instantiated = Vec::with_capacity(rel_shapes.len());
    let mut any_raw = false;

    for item in rel_shapes {
        let target_lid =
            if preserve_original_layer && app.timeline.is_layer_editable_by_id(item.layer_id) {
                item.layer_id
            } else {
                active_lid
            };

        if let Some(orig_binding) = item
            .symbol_instance
            .filter(|b| item.group_path.contains(&b.instance_gid))
        {
            if !seen_instance_gids.insert(orig_binding.instance_gid) {
                continue;
            }
            let mut new_parent_scope = app.canvas.editing_group_path.clone();
            for &old_gid in orig_binding.parent_scope_from_path(&item.group_path) {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = app.symbols.alloc_group_id();
                    gid_map.push((old_gid, alloc));
                    alloc
                };
                new_parent_scope.push(new_gid);
            }
            let mapped_inst_gid = if let Some(&(_, mapped)) = gid_map
                .iter()
                .find(|&&(orig, _)| orig == orig_binding.instance_gid)
            {
                mapped
            } else {
                let alloc = app.symbols.alloc_group_id();
                gid_map.push((orig_binding.instance_gid, alloc));
                alloc
            };

            let mut new_binding = orig_binding;
            new_binding.instance_gid = mapped_inst_gid;
            if offset != KVec2::ZERO {
                new_binding.xform = kurbo::Affine::translate(offset) * new_binding.xform;
            }

            if let Some(sym) = app.symbols.find_symbol(new_binding.symbol_id) {
                let current_frame = app.timeline.current_frame;
                let kf_start_frame = app
                    .timeline
                    .layer_index_by_id(target_lid)
                    .and_then(|l_idx| {
                        let layer = &app.timeline.layers[l_idx];
                        layer
                            .keyframe_index_at(current_frame)
                            .map(|kf_idx| layer.keyframes[kf_idx].start_frame)
                    })
                    .unwrap_or(current_frame);
                let local_shapes = sym.evaluated_local_shapes_for_instance(
                    new_binding,
                    kf_start_frame,
                    current_frame,
                    crate::symbol::SymbolEvalCtx::new(&app.symbols.library),
                );
                if local_shapes.is_empty() {
                    instantiated.push(FillShape::new_empty_symbol_marker(
                        new_binding,
                        target_lid,
                        &new_parent_scope,
                    ));
                } else {
                    instantiated.extend(new_binding.instantiate_shapes(
                        &local_shapes,
                        target_lid,
                        &new_parent_scope,
                    ));
                }
                continue;
            }
        }

        let mut new_path = app.canvas.editing_group_path.clone();
        if item.group_path.is_empty() {
            any_raw = true;
        } else {
            for &old_gid in &item.group_path {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = app.symbols.alloc_group_id();
                    gid_map.push((old_gid, alloc));
                    alloc
                };
                new_path.push(new_gid);
            }
        }
        let mut cloned = item
            .clone()
            .with_layer_id(target_lid)
            .with_group_path(new_path);
        cloned.shape_id = next_shape_id();
        if let Some(mut binding) = cloned.symbol_instance {
            if let Some(&(_, mapped_inst_gid)) = gid_map
                .iter()
                .find(|&&(orig, _)| orig == binding.instance_gid)
            {
                binding.instance_gid = mapped_inst_gid;
                cloned.symbol_instance = Some(binding);
            } else {
                cloned.symbol_instance = None;
            }
        }
        if offset != KVec2::ZERO {
            cloned.translate(offset);
        }
        instantiated.push(cloned);
    }

    (instantiated, any_raw)
}

/// 選択中のオブジェクト（生シェイプ・部分スライス領域・グループ問わず）を内部クリップボードにコピーする（`Cmd+C`）
pub(crate) fn copy_selected_shapes(app: &mut Is2dApp) -> bool {
    crate::canvas_tool::stop_playback(app);
    crate::canvas_tool::cancel_active_drag(app);
    let mut extracted = app.canvas.extract_selected_relative_to_scope();
    if extracted.is_empty() {
        return false;
    }
    for s in &mut extracted {
        s.strip_transient_caches();
    }
    app.canvas.clipboard = extracted;
    app.canvas.paste_count = 0;
    crate::project_io::capture_clipboard_symbols(app);
    true
}

/// 選択中のオブジェクトを内部クリップボードにコピーした上でキャンバスから削除する（`Cmd+X`）
pub(crate) fn cut_selected_shapes(app: &mut Is2dApp) -> bool {
    if !copy_selected_shapes(app) {
        return false;
    }
    delete_selected_shapes(app)
}

/// 内部クリップボードのオブジェクトを現在の編集スコープへ貼り付ける。
pub(crate) fn paste_clipboard(app: &mut Is2dApp, in_place: bool) -> bool {
    crate::canvas_tool::stop_playback(app);
    if app.canvas.clipboard.is_empty() {
        return false;
    }
    let target_lid = crate::canvas_tool::active_scope_layer_id(app);
    if !app.timeline.is_layer_editable_by_id(target_lid) {
        app.notify_warning("現在のレイヤーまたはフレームは編集できないためペーストできません");
        return false;
    }
    if crate::project_io::would_clipboard_paste_all_cycle(app) {
        app.notify_warning("編集中のシンボル自身や親シンボルは循環参照になるため配置できません");
        return false;
    }

    app.mutate_canvas_with_undo(true, |app| {
        let (resolved, had_cycle) = crate::project_io::resolve_clipboard_for_paste(app);
        if had_cycle {
            app.notify_warning(
                "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
            );
        }
        if resolved.is_empty() {
            return;
        }

        let offset = if in_place {
            KVec2::ZERO
        } else {
            app.canvas.paste_count += 1;
            let step = 16.0 / (app.viewport.zoom as f64);
            let k = app.canvas.paste_count as f64;
            KVec2::new(step * k, step * k)
        };

        let same_timeline_context = app.tabs.clipboard_source_doc_id == app.tabs.active_doc_id()
            && app.tabs.clipboard_source_symbol_id
                == app.symbols.edit_stack.last().map(|s| s.symbol_id)
            && app.canvas.editing_group_path.is_empty();
        let multi_layer_copy = same_timeline_context
            && resolved
                .first()
                .is_some_and(|first| resolved.iter().any(|s| s.layer_id != first.layer_id));
        let (pasted_shapes, any_raw) =
            instantiate_relative_shapes_in_scope(app, &resolved, offset, multi_layer_copy);

        insert_and_select_shapes_in_scope(app, pasted_shapes, any_raw);
    });
    true
}

/// 現在選択中のオブジェクト（生シェイプ・グループ・シンボル）をその場に複製し、
/// 複製側を選択状態に切り替えた上でドラッグ移動用の `undo_snapshot` を返す（`Alt + ドラッグ` 複製用）
pub(crate) fn duplicate_selected_shapes_for_alt_drag(app: &mut Is2dApp) -> Vec<FillShape> {
    let rel_shapes = app.canvas.extract_selected_relative_to_scope();
    if rel_shapes.is_empty() {
        return app.canvas.snapshot_for_drag();
    }
    let saved_pivot = app.canvas.transform_pivot;
    clear_selection(app);
    let undo_snapshot = app.canvas.snapshot_for_drag();
    let (duplicated, _) = instantiate_relative_shapes_in_scope(app, &rel_shapes, KVec2::ZERO, true);
    insert_and_select_shapes_in_scope(app, duplicated, false);
    app.canvas.transform_pivot = saved_pivot;
    crate::canvas_tool::sync_timeline_layer_from_canvas_selection(app);
    undo_snapshot
}

/// 現在の編集スコープの最前面挿入位置へ `new_shapes` を挿入し、それらを選択状態にしてレイヤー順に整列させる
pub(crate) fn insert_and_select_shapes_in_scope(
    app: &mut Is2dApp,
    new_shapes: Vec<FillShape>,
    mark_modified: bool,
) {
    let insert_pos = app.canvas.scope_insert_index();
    let count = new_shapes.len();
    app.canvas.shapes.splice(insert_pos..insert_pos, new_shapes);
    app.canvas.selected_shapes = (insert_pos..insert_pos + count).collect();
    app.canvas.sort_shapes_by_layer_order(&app.timeline);
    app.canvas.selection_modified = mark_modified;
    app.canvas.transform_pivot = None;
    if !matches!(app.ui.tool, Tool::Select | Tool::Transform) {
        crate::canvas_tool::set_tool(app, Tool::Select);
    }
}

/// 選択中の図形群を現在の編集スコープ内の最前面へ並べ替え、レイヤー順に整列させる
pub(crate) fn reorder_selected_to_scope_front(app: &mut Is2dApp) -> bool {
    let (is_sel, any_valid) = app.canvas.selected_shape_mask();
    if !any_valid {
        return false;
    }

    let insert_limit = app.canvas.scope_insert_index();
    let mut unselected = Vec::with_capacity(app.canvas.shapes.len());
    let mut moved_shapes = Vec::with_capacity(app.canvas.selected_shapes.len());
    let mut trailing_outside = Vec::new();
    for (idx, mut shape) in app.canvas.shapes.drain(..).enumerate() {
        if is_sel[idx] {
            // 生シェイプおよび通常グループ図形は移動・回転・拡縮のインプレース f32 頂点変換による丸め誤差の蓄積を解消するため、
            // 確定時に f64 の Contour からメッシュキャッシュを再構築する（シンボル展開図形はキャッシュ済み頂点をそのまま維持）
            if shape.symbol_instance.is_none() {
                shape.rebuild_cache();
            }
            moved_shapes.push(shape);
        } else if idx < insert_limit {
            unselected.push(shape);
        } else {
            trailing_outside.push(shape);
        }
    }

    let start_sel = unselected.len();
    let end_sel = start_sel + moved_shapes.len();
    unselected.extend(moved_shapes);
    unselected.extend(trailing_outside);
    app.canvas.shapes = unselected;
    app.canvas.selected_shapes = (start_sel..end_sel).collect();
    app.canvas.sort_shapes_by_layer_order(&app.timeline);
    true
}

pub(crate) fn commit_moved_selection(app: &mut Is2dApp, undo_snapshot: Vec<FillShape>) {
    if !reorder_selected_to_scope_front(app) {
        return;
    }
    let snapshot = app.canvas.take_unsliced_snapshot().unwrap_or(undo_snapshot);
    app.canvas.selection_modified = true;
    app.commit_canvas_mutation(snapshot);
}

/// 選択オブジェクトの平行移動ドラッグ終了時の確定・ロールバック共通処理
pub(crate) fn finish_move_selection_drag(
    app: &mut Is2dApp,
    total_delta: KVec2,
    undo_snapshot: Vec<FillShape>,
) {
    let min_move = 1.0 / (app.viewport.zoom as f64);
    if total_delta.hypot() > min_move && !app.canvas.selected_shapes.is_empty() {
        commit_moved_selection(app, undo_snapshot);
    } else if (total_delta.hypot() > 0.0 || app.canvas.shapes.len() != undo_snapshot.len())
        && app.canvas.restore_drag_snapshot(undo_snapshot)
    {
        crate::canvas_tool::sync_shapes_to_timeline(app);
    }
}

/// 輪郭エッジつまみ（PullEdge）または頂点移動（MoveVertex）ドラッグ終了時の確定・図形分割・ロールバック共通処理
pub(crate) fn finish_deform_shape_drag(
    app: &mut Is2dApp,
    shape_idx: usize,
    drag_dist: f64,
    undo_snapshot: Vec<FillShape>,
    apply_deform: impl FnOnce(&mut FillShape),
) {
    let min_deform = 0.5 / (app.viewport.zoom as f64);
    if drag_dist > min_deform && shape_idx < app.canvas.shapes.len() {
        crate::canvas_tool::focus_active_layer_to_shape(app, shape_idx);
        let mut deformed = app.canvas.shapes.remove(shape_idx);
        apply_deform(&mut deformed);
        let resolved = resolve_deformed_shape(&deformed);
        merge_shapes_into_canvas(&mut app.canvas.shapes, resolved);
        app.canvas.sort_shapes_by_layer_order(&app.timeline);
        app.commit_canvas_mutation(undo_snapshot);
    } else {
        app.canvas.restore_drag_snapshot(undo_snapshot);
    }
}

/// Flash風ドット柄（Polka-Dot）選択オーバーレイを描画
pub(crate) fn draw_flash_selection_overlay(
    app: &mut Is2dApp,
    painter: &egui::Painter,
    shape_idx: usize,
) {
    let zoom = app.viewport.zoom;
    let pan = app.viewport.pan;
    let Some(shape) = app.canvas.shapes.get_mut(shape_idx) else {
        return;
    };
    let sel_screen_mesh = shape.cached_selection_screen_mesh(zoom, pan);
    painter.add(arc_mesh_to_shape(sel_screen_mesh));
    if let Some(shape_ref) = app.canvas.shapes.get(shape_idx) {
        draw_shape_contour_lines(
            app,
            painter,
            shape_ref,
            Stroke::new(1.5, Color32::from_rgb(0, 175, 255)),
            false,
        );
    }
}

pub(crate) fn draw_shape_contour_lines(
    app: &Is2dApp,
    painter: &egui::Painter,
    shape: &FillShape,
    stroke: Stroke,
    show_anchors: bool,
) {
    let z = app.viewport.zoom as f64;
    let pad = 4.0 / z;
    let view_rect = app
        .viewport
        .visible_world_rect(painter, !app.symbols.edit_stack.is_empty())
        .inflate(pad, pad);
    for contour in shape.contours() {
        for seg in contour.segments.iter() {
            if seg.p0.x.max(seg.p1.x).max(seg.p2.x) < view_rect.x0
                || seg.p0.x.min(seg.p1.x).min(seg.p2.x) > view_rect.x1
                || seg.p0.y.max(seg.p1.y).max(seg.p2.y) < view_rect.y0
                || seg.p0.y.min(seg.p1.y).min(seg.p2.y) > view_rect.y1
            {
                continue;
            }
            let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot() * z;
            if mid_dev <= 0.15 {
                painter.line_segment(
                    [
                        app.viewport.world_to_screen(seg.p0),
                        app.viewport.world_to_screen(seg.p2),
                    ],
                    stroke,
                );
            } else {
                let mut prev = seg.p0;
                let approx_len = ((seg.p1 - seg.p0).hypot() + (seg.p2 - seg.p1).hypot()) * z;
                let steps = ((approx_len / 8.0).ceil() as usize).clamp(3, 18);
                for k in 1..=steps {
                    let curr = seg.eval(k as f64 / steps as f64);
                    painter.line_segment(
                        [
                            app.viewport.world_to_screen(prev),
                            app.viewport.world_to_screen(curr),
                        ],
                        stroke,
                    );
                    prev = curr;
                }
            }
            if show_anchors {
                let a = app.viewport.world_to_screen(seg.p0);
                painter.circle_filled(a, 3.0, Color32::WHITE);
                painter.circle_stroke(a, 3.0, Stroke::new(1.2, Color32::from_rgb(30, 120, 230)));
            }
        }
    }
}

pub(crate) fn handle_select_tool_input(
    app: &mut Is2dApp,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
) {
    match app.ui.select_sub_mode {
        SelectSubMode::AddAnchor => {
            handle_anchor_edit_input(app, ui, response, mouse_pt, true);
        }
        SelectSubMode::DeleteAnchor => {
            handle_anchor_edit_input(app, ui, response, mouse_pt, false);
        }
        SelectSubMode::Reshape => {
            let shift_held = ui.input(|i| i.modifiers.shift);
            let alt_held = ui.input(|i| i.modifiers.alt);
            if response.double_clicked() {
                if let Some(pt) = mouse_pt {
                    if !enter_group_at(app, pt) && app.canvas.topmost_shape_at(pt).is_none() {
                        exit_current_group(app);
                    }
                }
                return;
            }
            if response.drag_started() {
                if let Some(pt) = mouse_pt {
                    start_reshape_drag(app, pt, shift_held, alt_held);
                }
            } else if response.dragged() {
                if let Some(pt) = mouse_pt {
                    update_reshape_drag(app, pt);
                }
            } else if response.drag_stopped() {
                finish_reshape_drag(app, mouse_pt);
            } else if response.clicked() {
                if let Some(pt) = mouse_pt {
                    handle_reshape_click(app, pt, shift_held);
                }
            }

            if !matches!(app.canvas.drag, ActiveCanvasDrag::Select(_)) {
                let zoom = app.viewport.zoom;
                let sub_mode = app.ui.select_sub_mode;
                app.canvas.hover_target = mouse_pt
                    .map(|pt| compute_hover_target(&app.canvas, pt, zoom, sub_mode))
                    .unwrap_or(HoverTarget::None);
            }
        }
    }
}

fn handle_anchor_edit_input(
    app: &mut Is2dApp,
    ui: &egui::Ui,
    response: &egui::Response,
    mouse_pt: Option<Point>,
    is_add: bool,
) {
    let zoom = app.viewport.zoom;
    let sub_mode = app.ui.select_sub_mode;
    let just_pressed = response.hovered() && ui.input(|i| i.pointer.primary_pressed());
    if just_pressed {
        if let Some(pt) = mouse_pt {
            let target = compute_hover_target(&app.canvas, pt, zoom, sub_mode);
            let had_unsliced = app.canvas.has_unsliced_snapshot();
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            let modified_shape = match (is_add, target) {
                (
                    true,
                    HoverTarget::Edge {
                        shape_idx,
                        contour_idx,
                        seg_idx,
                        t,
                        ..
                    },
                ) => app
                    .canvas
                    .shapes
                    .get_mut(shape_idx)
                    .and_then(|s| s.get_contour_mut(contour_idx))
                    .is_some_and(|c| c.insert_anchor_at(seg_idx, t))
                    .then_some(shape_idx),
                (
                    false,
                    HoverTarget::Vertex {
                        shape_idx,
                        contour_idx,
                        seg_idx,
                        ..
                    },
                ) => app
                    .canvas
                    .shapes
                    .get_mut(shape_idx)
                    .and_then(|s| s.get_contour_mut(contour_idx))
                    .is_some_and(|c| c.delete_anchor_at(seg_idx))
                    .then_some(shape_idx),
                _ => None,
            };
            if let Some(s_idx) = modified_shape {
                if let Some(shape) = app.canvas.shapes.get_mut(s_idx) {
                    shape.rebuild_cache();
                }
                app.commit_canvas_mutation(snapshot);
            } else if had_unsliced {
                app.canvas.restore_unsliced_snapshot(snapshot);
            }
        }
    }
    app.canvas.hover_target = mouse_pt
        .map(|pt| compute_hover_target(&app.canvas, pt, zoom, sub_mode))
        .unwrap_or(HoverTarget::None);
}

/// 選択オブジェクトの移動ドラッグ（`Alt` 複製移動を含む）の初期状態を構築する共通ヘルパー
pub(crate) fn begin_move_selection_drag(
    app: &mut Is2dApp,
    pt: Point,
    alt_held: bool,
) -> MoveSelectionDrag {
    let undo_snapshot = if alt_held {
        duplicate_selected_shapes_for_alt_drag(app)
    } else {
        app.canvas.snapshot_for_drag()
    };
    MoveSelectionDrag::new(pt, undo_snapshot)
}

fn start_reshape_drag(app: &mut Is2dApp, pt: Point, shift_held: bool, alt_held: bool) {
    app.ui.inspect_timeline_frame = false;
    if shift_held {
        app.canvas.drag =
            ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect(MarqueeDrag::new(pt, true)));
        return;
    }
    let mut target =
        compute_hover_target(&app.canvas, pt, app.viewport.zoom, app.ui.select_sub_mode);
    if alt_held {
        if let Some(shape_idx) = target.shape_idx() {
            if !app.canvas.selected_shapes.contains(&shape_idx) {
                select_single_shape_at(app, pt);
            }
            let move_drag = begin_move_selection_drag(app, pt, true);
            app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MoveShape(move_drag));
            return;
        }
    }
    if matches!(
        target,
        HoverTarget::Vertex { .. } | HoverTarget::Edge { .. }
    ) && (!app.canvas.selected_shapes.is_empty() || app.canvas.has_unsliced_snapshot())
    {
        clear_selection(app);
        target = compute_hover_target(&app.canvas, pt, app.viewport.zoom, app.ui.select_sub_mode);
    }
    match target {
        HoverTarget::Vertex {
            shape_idx,
            contour_idx,
            seg_idx,
            pos,
        } => {
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            if let Some((orig_prev_ctrl, orig_curr_ctrl)) = app
                .canvas
                .shapes
                .get(shape_idx)
                .and_then(|s| s.get_contour(contour_idx))
                .and_then(|contour| {
                    let n = contour.segments.len();
                    if n == 0 || seg_idx >= n {
                        return None;
                    }
                    let prev_idx = (seg_idx + n - 1) % n;
                    Some((contour.segments[prev_idx].p1, contour.segments[seg_idx].p1))
                })
            {
                app.canvas.selected_shapes.clear();
                app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
                    shape_idx,
                    contour_idx,
                    seg_idx,
                    orig_vertex: pos,
                    last_pos: pos,
                    orig_prev_ctrl,
                    orig_curr_ctrl,
                    snapped_to: None,
                    undo_snapshot: snapshot,
                });
            }
        }
        HoverTarget::Edge {
            shape_idx,
            contour_idx,
            seg_idx,
            t,
            closest_pos,
        } => {
            let snapshot = app.canvas.take_or_clone_unsliced_snapshot();
            if let Some(orig_seg) = app
                .canvas
                .get_contour_segment(shape_idx, contour_idx, seg_idx)
            {
                app.canvas.selected_shapes.clear();
                app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::PullEdge {
                    shape_idx,
                    contour_idx,
                    seg_idx,
                    grab_t: t.clamp(0.18, 0.82),
                    grab_pos: closest_pos,
                    last_pos: closest_pos,
                    orig_seg,
                    undo_snapshot: snapshot,
                });
            }
        }
        HoverTarget::Fill { shape_idx } => {
            if !app.canvas.selected_shapes.contains(&shape_idx) {
                select_single_shape_at(app, pt);
            }
            let move_drag = begin_move_selection_drag(app, pt, false);
            app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MoveShape(move_drag));
        }
        HoverTarget::None => {
            app.canvas.drag = ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect(
                MarqueeDrag::new(pt, false),
            ));
        }
    }
}

fn update_reshape_drag(app: &mut Is2dApp, pt: Point) {
    let vertex_drag_info = if let ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
        shape_idx,
        contour_idx,
        seg_idx,
        orig_vertex,
        orig_prev_ctrl,
        orig_curr_ctrl,
        ..
    }) = &app.canvas.drag
    {
        Some((
            *shape_idx,
            *contour_idx,
            *seg_idx,
            *orig_vertex,
            *orig_prev_ctrl,
            *orig_curr_ctrl,
        ))
    } else {
        None
    };

    if let Some((s_idx, c_idx, g_idx, orig_v, prev_c, curr_c)) = vertex_drag_info {
        let snap_opt = find_snap_vertex(&app.canvas, pt, app.viewport.zoom, s_idx, c_idx, g_idx);
        let target_pt = snap_opt.unwrap_or(pt);
        if let ActiveCanvasDrag::Select(SelectDragState::MoveVertex {
            snapped_to,
            last_pos,
            ..
        }) = &mut app.canvas.drag
        {
            *snapped_to = snap_opt;
            *last_pos = target_pt;
        }
        if let Some(shape) = app.canvas.shapes.get_mut(s_idx) {
            set_moved_vertex(shape, (c_idx, g_idx), orig_v, (prev_c, curr_c), target_pt);
            shape.rebuild_cache();
        }
        return;
    }

    let mut move_delta: Option<KVec2> = None;
    if let ActiveCanvasDrag::Select(select_drag) = &mut app.canvas.drag {
        match select_drag {
            SelectDragState::MoveShape(drag) => {
                move_delta = Some(drag.step(pt));
            }
            SelectDragState::MarqueeSelect(m) => {
                m.current_pos = pt;
            }
            SelectDragState::PullEdge {
                shape_idx,
                contour_idx,
                seg_idx,
                grab_t,
                last_pos,
                orig_seg,
                ..
            } => {
                *last_pos = pt;
                if let Some(shape) = app.canvas.shapes.get_mut(*shape_idx) {
                    set_pulled_edge(shape, (*contour_idx, *seg_idx), *grab_t, *orig_seg, pt);
                    shape.rebuild_cache();
                }
            }
            _ => {}
        }
    }
    if let Some(delta) = move_delta {
        app.canvas.translate_selected(delta);
    }
}

fn finish_reshape_drag(app: &mut Is2dApp, mouse_pt: Option<Point>) {
    let ActiveCanvasDrag::Select(finished_drag) =
        std::mem::replace(&mut app.canvas.drag, ActiveCanvasDrag::None)
    else {
        return;
    };
    match finished_drag {
        SelectDragState::MoveShape(drag) => {
            finish_move_selection_drag(app, drag.total_delta, drag.undo_snapshot);
        }
        SelectDragState::MarqueeSelect(m) => {
            let end_pos = mouse_pt.unwrap_or(m.current_pos);
            apply_marquee_selection(app, m.start_pos, end_pos, m.additive);
        }
        SelectDragState::PullEdge {
            shape_idx,
            contour_idx,
            seg_idx,
            grab_t,
            grab_pos,
            last_pos,
            orig_seg,
            undo_snapshot,
        } => {
            finish_deform_shape_drag(
                app,
                shape_idx,
                (last_pos - grab_pos).hypot(),
                undo_snapshot,
                |deformed| {
                    apply_pull_edge_to_shape(
                        deformed,
                        (contour_idx, seg_idx),
                        grab_t,
                        orig_seg,
                        last_pos,
                    );
                },
            );
        }
        SelectDragState::MoveVertex {
            shape_idx,
            contour_idx,
            seg_idx,
            orig_vertex,
            last_pos,
            orig_prev_ctrl,
            orig_curr_ctrl,
            undo_snapshot,
            ..
        } => {
            finish_deform_shape_drag(
                app,
                shape_idx,
                (last_pos - orig_vertex).hypot(),
                undo_snapshot,
                |deformed| {
                    apply_move_vertex_to_shape(
                        deformed,
                        (contour_idx, seg_idx),
                        orig_vertex,
                        (orig_prev_ctrl, orig_curr_ctrl),
                        last_pos,
                    );
                },
            );
        }
    }
}

fn handle_reshape_click(app: &mut Is2dApp, pt: Point, shift_held: bool) {
    app.ui.inspect_timeline_frame = false;
    let zoom = app.viewport.zoom;
    let sub_mode = app.ui.select_sub_mode;
    let hit_shape = app
        .canvas
        .topmost_shape_at(pt)
        .or_else(|| compute_hover_target(&app.canvas, pt, zoom, sub_mode).shape_idx());
    if shift_held {
        if let Some(shape_idx) = hit_shape {
            toggle_shape_selection(app, shape_idx);
        }
    } else {
        match hit_shape {
            Some(shape_idx) if app.canvas.selected_shapes.contains(&shape_idx) => {
                app.canvas.selected_shapes = app.canvas.expand_to_group_unit(shape_idx);
                app.canvas.transform_pivot = None;
            }
            Some(_) => {
                select_single_shape_at(app, pt);
            }
            None => {
                clear_selection(app);
            }
        }
    }
}

// ------------------------------------------------------------------------
// 3. ヒットテスト・頂点スナップ・選択ツールカーソル描画
// ------------------------------------------------------------------------

/// 2次ベジェ曲線の3制御点 `(p0, p1, p2)` の凸包AABB（余白 `margin` 付き）に点 `pt` が含まれるかを高速判定する
#[inline]
pub(crate) fn quad_control_bbox_contains(seg: &QuadBez, pt: Point, margin: f64) -> bool {
    let min_x = seg.p0.x.min(seg.p1.x).min(seg.p2.x) - margin;
    let max_x = seg.p0.x.max(seg.p1.x).max(seg.p2.x) + margin;
    let min_y = seg.p0.y.min(seg.p1.y).min(seg.p2.y) - margin;
    let max_y = seg.p0.y.max(seg.p1.y).max(seg.p2.y) + margin;
    pt.x >= min_x && pt.x <= max_x && pt.y >= min_y && pt.y <= max_y
}

/// 指定座標 `pt` がシェイプの輪郭（外周・穴）から距離 `radius` 以内にあるかを判定する
fn point_within_shape_contours(shape: &FillShape, pt: Point, radius: f64, inv_z: f64) -> bool {
    let r_sq = radius * radius;
    for contour in shape.contours() {
        for seg in contour.segments.iter() {
            if !quad_control_bbox_contains(seg, pt, radius) {
                continue;
            }
            if seg.nearest(pt, 0.05 * inv_z).distance_sq <= r_sq {
                return true;
            }
        }
    }
    false
}

/// 矩形 `rect` と交差する現在の編集スコープ直下の子グループID一覧を収集する
pub(crate) fn collect_marquee_hit_groups(
    canvas: &CanvasState,
    rect: KRect,
    clip_rect_shape: &FillShape,
) -> HashSet<u64> {
    let mut hit_group_ids: HashSet<u64> = HashSet::new();
    for shape in &canvas.shapes {
        let Some(gid) = shape.active_group_id_at_scope(&canvas.editing_group_path) else {
            continue;
        };
        if hit_group_ids.contains(&gid) {
            continue;
        }
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !rects_overlap(bbox, rect) {
            continue;
        }
        let in_rect =
            bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1;
        if in_rect
            || !boolean_operation(
                std::slice::from_ref(shape),
                std::slice::from_ref(clip_rect_shape),
                BoolOp::Intersection,
                shape.color,
            )
            .is_empty()
        {
            hit_group_ids.insert(gid);
        }
    }
    hit_group_ids
}

/// アルゴリズム 4: マウス位置に対する頂点・輪郭エッジ・塗りのヒットテスト
pub(crate) fn compute_hover_target(
    canvas: &CanvasState,
    mouse_pt: Point,
    zoom: f32,
    sub_mode: SelectSubMode,
) -> HoverTarget {
    let inv_z = 1.0 / (zoom as f64);
    let vertex_hit_radius = 9.0 * inv_z;
    let edge_hit_outer_radius = 9.5 * inv_z;
    let edge_hit_inner_radius = 5.5 * inv_z;
    let anchor_edit_hit_radius = 12.0 * inv_z;
    let selected_grab_radius = 6.0 * inv_z;

    let (is_sel, has_sel) = canvas.selected_shape_mask();

    // 0. 通常の選択モード（Reshape）では、選択済みシェイプ（極細線の外周マージン含む）へのホバーを
    //    すべて移動対象（HoverTarget::Fill）として最優先し、選択済みシェイプの変形暴発を防ぐ
    if sub_mode == SelectSubMode::Reshape && has_sel {
        for &s_idx in canvas.selected_shapes.iter().rev() {
            let Some(shape) = canvas.shapes.get(s_idx) else {
                continue;
            };
            if !shape.is_in_editing_scope(&canvas.editing_group_path)
                || !shape
                    .bounding_box()
                    .inflate(selected_grab_radius, selected_grab_radius)
                    .contains(mouse_pt)
            {
                continue;
            }
            if shape.contains_point(mouse_pt)
                || point_within_shape_contours(shape, mouse_pt, selected_grab_radius, inv_z)
            {
                return HoverTarget::Fill { shape_idx: s_idx };
            }
        }
    }

    // 1. 輪郭の角（アンカーポイント）への近接判定
    if sub_mode != SelectSubMode::AddAnchor {
        let v_radius = if sub_mode == SelectSubMode::DeleteAnchor {
            anchor_edit_hit_radius
        } else {
            vertex_hit_radius
        };
        let mut best_vertex: Option<(f64, HoverTarget)> = None;
        for (s_idx, shape) in canvas.shapes.iter().enumerate().rev() {
            let Some(bbox) = shape.geometry_bounding_box() else {
                continue;
            };
            if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
                || !shape.is_raw_at_scope(&canvas.editing_group_path)
                || !bbox.inflate(v_radius, v_radius).contains(mouse_pt)
            {
                continue;
            }
            for (c_idx, contour) in shape.contours().enumerate() {
                if sub_mode == SelectSubMode::DeleteAnchor && contour.segments.len() <= 2 {
                    continue;
                }
                for (seg_idx, seg) in contour.segments.iter().enumerate() {
                    let dist = (mouse_pt - seg.p0).hypot();
                    if dist <= v_radius && best_vertex.as_ref().is_none_or(|(d, _)| dist < *d) {
                        best_vertex = Some((
                            dist,
                            HoverTarget::Vertex {
                                shape_idx: s_idx,
                                contour_idx: c_idx,
                                seg_idx,
                                pos: seg.p0,
                            },
                        ));
                    }
                }
            }
        }
        if let Some((_, target)) = best_vertex {
            return target;
        }
        if sub_mode == SelectSubMode::DeleteAnchor {
            return HoverTarget::None;
        }
    }

    // 2. 輪郭の2次ベジェ曲線セグメント（エッジ）への近接判定
    let mut best_edge: Option<(f64, HoverTarget)> = None;
    for (s_idx, shape) in canvas.shapes.iter().enumerate().rev() {
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
            || !shape.is_raw_at_scope(&canvas.editing_group_path)
            || !bbox
                .inflate(anchor_edit_hit_radius, anchor_edit_hit_radius)
                .contains(mouse_pt)
        {
            continue;
        }
        let max_dist = if sub_mode == SelectSubMode::AddAnchor {
            anchor_edit_hit_radius
        } else if shape.contains_point(mouse_pt) {
            edge_hit_inner_radius
        } else {
            edge_hit_outer_radius
        };

        for (c_idx, contour) in shape.contours().enumerate() {
            for (seg_idx, seg) in contour.segments.iter().enumerate() {
                if !quad_control_bbox_contains(seg, mouse_pt, max_dist) {
                    continue;
                }
                let nearest = seg.nearest(mouse_pt, 0.05 * inv_z);
                let t_clamped = nearest.t.clamp(0.08, 0.92);
                let eval_pt = if sub_mode == SelectSubMode::AddAnchor {
                    seg.eval(t_clamped)
                } else {
                    seg.eval(nearest.t)
                };
                let dist = nearest.distance_sq.sqrt();
                if dist <= max_dist && best_edge.as_ref().is_none_or(|(d, _)| dist < *d) {
                    best_edge = Some((
                        dist,
                        HoverTarget::Edge {
                            shape_idx: s_idx,
                            contour_idx: c_idx,
                            seg_idx,
                            t: if sub_mode == SelectSubMode::AddAnchor {
                                t_clamped
                            } else {
                                nearest.t
                            },
                            closest_pos: eval_pt,
                        },
                    ));
                }
            }
        }
    }
    if let Some((_, target)) = best_edge {
        return target;
    }
    if sub_mode == SelectSubMode::AddAnchor {
        return HoverTarget::None;
    }

    // 3. シェイプ内部の塗り領域判定
    if let Some(s_idx) = canvas.topmost_shape_at(mouse_pt) {
        return HoverTarget::Fill { shape_idx: s_idx };
    }

    HoverTarget::None
}

/// 頂点ドラッグ時に周囲の他の頂点へ吸着（スナップ）する座標を検索
pub(crate) fn find_snap_vertex(
    canvas: &CanvasState,
    drag_pt: Point,
    zoom: f32,
    ignore_shape: usize,
    ignore_contour: usize,
    ignore_seg: usize,
) -> Option<Point> {
    let snap_radius = 12.0 / (zoom as f64);
    let mut best_snap: Option<(f64, Point)> = None;

    for (s_idx, shape) in canvas.shapes.iter().enumerate() {
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !shape.is_raw_at_scope(&canvas.editing_group_path)
            || !bbox.inflate(snap_radius, snap_radius).contains(drag_pt)
        {
            continue;
        }
        for (c_idx, contour) in shape.contours().enumerate() {
            for (seg_idx, seg) in contour.segments.iter().enumerate() {
                if s_idx == ignore_shape && c_idx == ignore_contour && seg_idx == ignore_seg {
                    continue;
                }
                let dist = (drag_pt - seg.p0).hypot();
                if dist <= snap_radius && best_snap.as_ref().is_none_or(|(d, _)| dist < *d) {
                    best_snap = Some((dist, seg.p0));
                }
            }
        }
    }
    best_snap.map(|(_, pt)| pt)
}

pub(crate) fn draw_select_tool_cursor(
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match &app.canvas.drag {
        ActiveCanvasDrag::Select(SelectDragState::PullEdge {
            shape_idx,
            contour_idx,
            seg_idx,
            ..
        }) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            if app.ui.show_wireframe {
                if let Some(seg) =
                    app.canvas
                        .get_contour_segment(*shape_idx, *contour_idx, *seg_idx)
                {
                    draw_highlighted_quad_segment(app, painter, seg);
                }
            }
            draw_flash_curve_cursor_badge(painter, pos);
        }
        ActiveCanvasDrag::Select(SelectDragState::MoveVertex { snapped_to, .. }) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            if let Some(snap_pt) = snapped_to {
                let sp = app.viewport.world_to_screen(*snap_pt);
                painter.circle_stroke(sp, 9.0, Stroke::new(2.5, Color32::from_rgb(255, 50, 140)));
                painter.circle_filled(sp, 4.0, Color32::from_rgb(255, 50, 140));
            }
            draw_flash_corner_cursor_badge(painter, pos);
        }
        ActiveCanvasDrag::Select(SelectDragState::MoveShape { .. }) => {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        }
        ActiveCanvasDrag::Select(SelectDragState::MarqueeSelect { .. }) => {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
        }
        _ => draw_select_sub_mode_cursor(app, ctx, painter, pos),
    }
}

fn draw_select_sub_mode_cursor(
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match app.ui.select_sub_mode {
        SelectSubMode::AddAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, true);
            if let HoverTarget::Edge {
                shape_idx,
                contour_idx,
                seg_idx,
                closest_pos,
                ..
            } = app.canvas.hover_target
            {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                if let Some(seg) = app
                    .canvas
                    .get_contour_segment(shape_idx, contour_idx, seg_idx)
                {
                    draw_highlighted_quad_segment(app, painter, seg);
                }
                let cp = app.viewport.world_to_screen(closest_pos);
                painter.circle_filled(cp, 5.5, Color32::from_rgb(30, 200, 95));
                painter.circle_stroke(cp, 5.5, Stroke::new(2.0, Color32::WHITE));
            } else {
                ctx.set_cursor_icon(CursorIcon::Default);
            }
        }
        SelectSubMode::DeleteAnchor => {
            draw_anchor_edit_cursor_badge(painter, pos, false);
            if let HoverTarget::Vertex { pos: v_pos, .. } = app.canvas.hover_target {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
                let vp = app.viewport.world_to_screen(v_pos);
                painter.circle_filled(vp, 6.0, Color32::from_rgb(235, 55, 55));
                painter.circle_stroke(vp, 6.0, Stroke::new(2.0, Color32::WHITE));
            } else {
                ctx.set_cursor_icon(CursorIcon::Default);
            }
        }
        SelectSubMode::Reshape => draw_reshape_hover_cursor(app, ctx, painter, pos),
    }
}

fn draw_reshape_hover_cursor(
    app: &Is2dApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    pos: Pos2,
) {
    match app.canvas.hover_target {
        HoverTarget::Vertex {
            pos: v_pos,
            shape_idx,
            contour_idx,
            ..
        } => {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            if app.ui.show_wireframe {
                if let Some(contour) = app
                    .canvas
                    .shapes
                    .get(shape_idx)
                    .and_then(|s| s.get_contour(contour_idx))
                {
                    for seg in contour.segments.iter() {
                        let p = app.viewport.world_to_screen(seg.p0);
                        painter.circle_filled(p, 3.2, Color32::WHITE);
                        painter.circle_stroke(
                            p,
                            3.2,
                            Stroke::new(1.2, Color32::from_rgb(30, 130, 240)),
                        );
                    }
                }
                let vp = app.viewport.world_to_screen(v_pos);
                painter.circle_filled(vp, 5.5, Color32::from_rgb(255, 120, 40));
                painter.circle_stroke(vp, 5.5, Stroke::new(2.0, Color32::WHITE));
            }
            draw_flash_corner_cursor_badge(painter, pos);
        }
        HoverTarget::Edge {
            shape_idx,
            contour_idx,
            seg_idx,
            closest_pos,
            ..
        } => {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
            if app.ui.show_wireframe {
                if let Some(seg) = app
                    .canvas
                    .get_contour_segment(shape_idx, contour_idx, seg_idx)
                {
                    draw_highlighted_quad_segment(app, painter, seg);
                }
                painter.circle_filled(
                    app.viewport.world_to_screen(closest_pos),
                    4.0,
                    Color32::from_rgb(0, 175, 255),
                );
            }
            draw_flash_curve_cursor_badge(painter, pos);
        }
        HoverTarget::Fill { .. } => {
            ctx.set_cursor_icon(CursorIcon::Move);
        }
        HoverTarget::None => {
            ctx.set_cursor_icon(CursorIcon::Default);
        }
    }
}

/// ホバー・ドラッグ中の2次ベジェ曲線セグメントとその両端アンカーを強調描画（ハンドルは表示しない）
pub(crate) fn draw_highlighted_quad_segment(app: &Is2dApp, painter: &egui::Painter, seg: QuadBez) {
    let steps = 16;
    let mut prev = seg.p0;
    for k in 1..=steps {
        let curr = seg.eval(k as f64 / steps as f64);
        painter.line_segment(
            [
                app.viewport.world_to_screen(prev),
                app.viewport.world_to_screen(curr),
            ],
            Stroke::new(3.0, Color32::from_rgb(0, 175, 255)),
        );
        prev = curr;
    }
    for end_pt in [seg.p0, seg.p2] {
        let p = app.viewport.world_to_screen(end_pt);
        painter.circle_filled(p, 4.0, Color32::WHITE);
        painter.circle_stroke(p, 4.0, Stroke::new(1.8, Color32::from_rgb(0, 140, 230)));
    }
}

/// Flash の選択ツールでエッジに近づいた際の「曲線アーク」カーソルバッジを描画
fn draw_flash_curve_cursor_badge(painter: &egui::Painter, cursor: Pos2) {
    let base = cursor + Vec2::new(14.0, 14.0);
    let arc = QuadBez::new(
        Point::new(base.x as f64 - 6.0, base.y as f64 + 4.0),
        Point::new(base.x as f64, base.y as f64 - 8.0),
        Point::new(base.x as f64 + 8.0, base.y as f64 + 4.0),
    );
    let mut prev = arc.p0;
    for k in 1..=10 {
        let curr = arc.eval(k as f64 / 10.0);
        painter.line_segment(
            [to_pos2(prev), to_pos2(curr)],
            Stroke::new(2.2, Color32::from_rgb(20, 20, 25)),
        );
        prev = curr;
    }
}

/// Flash の選択ツールで頂点（アンカー）に近づいた際の「角（コーナー）」カーソルバッジを描画
fn draw_flash_corner_cursor_badge(painter: &egui::Painter, cursor: Pos2) {
    let base = cursor + Vec2::new(14.0, 14.0);
    let p0 = Pos2::new(base.x - 6.0, base.y + 5.0);
    let p1 = Pos2::new(base.x + 1.0, base.y - 5.0);
    let p2 = Pos2::new(base.x + 8.0, base.y + 5.0);
    let stroke = Stroke::new(2.2, Color32::from_rgb(20, 20, 25));
    painter.line_segment([p0, p1], stroke);
    painter.line_segment([p1, p2], stroke);
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
    painter.circle_stroke(base, 8.0, Stroke::new(1.5, Color32::WHITE));
    let stroke = Stroke::new(2.2, Color32::WHITE);
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
