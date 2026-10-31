use eframe::egui::{Color32, Vec2};
use kurbo::{Affine, ParamCurve, Point, QuadBez, Vec2 as KVec2};

use crate::canvas_tool::{
    CoalesceKey, EditorDoc, SelectSubMode, Tool, UndoEntry, active_scope_layer_id,
    cancel_active_drag, clear_selection, flush_canvas_before_timeline_op,
    focus_active_layer_to_shape, insert_and_select_shapes_in_scope, load_shapes_from_timeline,
    prepare_moved_selection, select_shapes_for_timeline_selection, set_tool,
    sync_shapes_to_timeline, toggle_select_sub_mode,
};
use crate::clipboard_ops::{
    instantiate_relative_shapes_in_scope, prepare_copy_selected_shapes,
    resolve_clipboard_for_paste, would_clipboard_paste_all_cycle,
};
use crate::select_ops::{
    apply_marquee_selection, apply_move_vertex_to_shape, apply_pull_edge_to_shape,
    compute_hover_target, enter_group_at, exit_current_group, prepare_clear_active_frame_shapes,
    prepare_delete_selected_shapes, prepare_group_selected_shapes, prepare_ungroup_selected_shapes,
    select_single_shape_at, toggle_shape_selection,
};
use crate::symbol::{SymbolInstanceBinding, SymbolLoopMode, TweenRotate};
use crate::symbol_ops::{
    enter_symbol_from_library, enter_symbol_timeline, exit_symbol_timeline,
    prepare_convert_selection_to_symbol, prepare_place_symbol_instance, selected_single_symbol_info,
};
use crate::timeline::{MAX_LAYERS_PER_TIMELINE, Timeline, TimelineCellSelection};
use crate::vector_boolean::{
    BoolOp, boolean_operation, erase_from_canvas, find_enclosed_region_at,
    merge_disjoint_fill_into_canvas, merge_shapes_into_canvas, resolve_deformed_shape,
};
use crate::vector_engine::{
    FillShape, MAX_VIEW_ZOOM, MIN_VIEW_ZOOM, affine_rotate_around, affine_scale_around,
    rects_overlap,
};
use crate::vector_stroke::{build_smoothed_stroke_shapes_at_zoom, generate_brush_outline_at_zoom};

// ============================================================================
// 1. ActionHost / Action / ApplyResult の定義
// ============================================================================

/// `apply` の対象となるホスト（`EditorDoc` 単体または `Is2dApp`）を抽象化するトレイト
pub(crate) trait ActionHost {
    fn doc(&self) -> &EditorDoc;
    fn doc_mut(&mut self) -> &mut EditorDoc;
    fn stop_playback(&mut self);
}

impl ActionHost for EditorDoc {
    fn doc(&self) -> &EditorDoc {
        self
    }

    fn doc_mut(&mut self) -> &mut EditorDoc {
        self
    }

    fn stop_playback(&mut self) {}
}

/// `apply` の実行結果
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApplyResult {
    Applied(bool),
    CreatedId(Option<u64>),
}

impl ApplyResult {
    pub(crate) fn as_bool(self) -> bool {
        match self {
            Self::Applied(b) => b,
            Self::CreatedId(opt) => opt.is_some(),
        }
    }

    #[cfg(test)]
    pub(crate) fn id(self) -> Option<u64> {
        match self {
            Self::CreatedId(opt) => opt,
            Self::Applied(_) => None,
        }
    }
}

/// ドキュメント状態または編集セッションを変更するすべての操作を表す列挙型
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum Action {
    // 描画・塗り
    PaintStroke {
        points: Vec<Point>,
        radius: f32,
        color: Color32,
    },
    EraseStroke {
        points: Vec<Point>,
        radius: f32,
    },
    CommitRawOutline {
        raw: Vec<Point>,
    },
    BucketFill {
        pt: Point,
        color: Color32,
    },

    // 選択・移動・グループ階層
    SelectSingleAt {
        pt: Point,
    },
    ToggleSelectionAt {
        shape_idx: usize,
    },
    SelectMarquee {
        p0: Point,
        p1: Point,
        additive: bool,
    },
    SelectAll,
    Deselect,
    MoveSelected {
        delta: KVec2,
    },
    NudgeSelection {
        delta: KVec2,
    },
    FinishMoveSelectionDrag {
        total_delta: KVec2,
        undo_snapshot: Vec<FillShape>,
        orig_selected_ids: Vec<u64>,
    },
    CommitMovedSelection {
        undo_snapshot: Vec<FillShape>,
    },
    DeleteSelection,
    ClearActiveFrame,
    Group,
    Ungroup,
    EnterGroupAt {
        pt: Point,
    },
    ExitGroup,
    ExitGroupSteps {
        steps: usize,
    },

    // 輪郭変形・アンカー編集
    ReshapeEdge {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        grab_t: f64,
        grab_pos: Point,
        target_pos: Point,
        orig_seg: QuadBez,
        undo_snapshot: Vec<FillShape>,
    },
    MoveVertex {
        shape_idx: usize,
        contour_idx: usize,
        seg_idx: usize,
        orig_vertex: Point,
        target_pos: Point,
        orig_ctrls: (Point, Point),
        undo_snapshot: Vec<FillShape>,
    },
    PullEdgeFromTo {
        from: Point,
        to: Point,
    },
    MoveVertexFromTo {
        from: Point,
        to: Point,
    },
    AddAnchor {
        pt: Point,
    },
    DeleteAnchor {
        pt: Point,
    },

    // 自由変形・ピボット
    TransformSelection {
        affine: Affine,
    },
    ScaleSelection {
        sx: f64,
        sy: f64,
    },
    FlipSelection {
        flip_x: bool,
        flip_y: bool,
    },
    RotateSelection {
        angle_rad: f64,
    },
    SetTransformPivot {
        world_pivot: Option<Point>,
    },
    ResetTransformPivot,
    CommitTransformDrag {
        changed: bool,
        undo_snapshot: Vec<FillShape>,
    },
    CommitPivotDrag {
        changed: bool,
        undo_snapshot: Vec<FillShape>,
    },

    // クリップボード
    Copy,
    Cut,
    Paste {
        in_place: bool,
    },

    // シンボル・クラシックトゥイーン
    ConvertToSymbol,
    PlaceSymbol {
        sym_id: u64,
    },
    EnterSymbolTimeline {
        binding: SymbolInstanceBinding,
    },
    EnterSymbolFromLibrary {
        sym_id: u64,
    },
    ExitSymbolTimeline,
    ExitSymbolToDepth {
        depth: usize,
    },
    RenameSymbol {
        sym_id: u64,
        new_name: String,
    },
    DeleteSymbol {
        sym_id: u64,
    },
    SetSymbolPlayback {
        loop_mode: Option<SymbolLoopMode>,
        first_frame: Option<usize>,
    },
    ToggleTween,
    SetTweenRotate {
        rotate: TweenRotate,
    },

    // タイムライン・レイヤー・ステージ設定
    SwitchLayerAndFrame {
        layer_idx: usize,
        frame: usize,
    },
    ScrubRulerToFrame {
        frame: usize,
    },
    SelectTimelineCells {
        l0: usize,
        f0: usize,
        l1: usize,
        f1: usize,
    },
    InsertKeyframe {
        copy_prev: bool,
    },
    DeleteKeyframe,
    ExtendFrame,
    RemoveFrame,
    MoveOrCopyFrames {
        sel: TimelineCellSelection,
        layer_delta: isize,
        frame_delta: isize,
        copy_mode: bool,
    },
    AddLayer,
    DeleteActiveLayer,
    MoveActiveLayer {
        up: bool,
    },
    ReorderLayer {
        src_idx: usize,
        dst_idx: usize,
    },
    SetLayerVisible {
        layer_idx: usize,
        visible: bool,
    },
    SetLayerLocked {
        layer_idx: usize,
        locked: bool,
    },
    SetTotalFrames {
        total: usize,
    },
    SetFps {
        fps: u32,
    },
    SetStageSize {
        size: Vec2,
    },

    // ツール・サブモード・Undo/Redo・Coalesce区切り
    SetTool {
        tool: Tool,
    },
    ToggleSelectSubMode {
        mode: SelectSubMode,
    },
    Undo,
    Redo,
    EndCoalesce,
}

impl Action {
    /// 連続する同一種の変更を1つのUndoステップにまとめるための `CoalesceKey` を返す
    pub(crate) fn coalesce_key(&self, doc: &EditorDoc) -> Option<CoalesceKey> {
        match self {
            Self::SetStageSize { .. } => Some(CoalesceKey::StageSize),
            Self::SetTotalFrames { .. } => Some(CoalesceKey::TotalFrames),
            Self::SetFps { .. } => Some(CoalesceKey::Fps),
            Self::RenameSymbol { sym_id, .. } => Some(CoalesceKey::RenameSymbol(*sym_id)),
            Self::SetSymbolPlayback {
                loop_mode,
                first_frame: Some(_),
            } => selected_single_symbol_info(doc).and_then(|(b, _, _)| {
                if loop_mode.is_none_or(|m| m == b.loop_mode) {
                    Some(CoalesceKey::SymbolPlayback(b.instance_gid))
                } else {
                    None
                }
            }),
            Self::NudgeSelection { .. } => Some(CoalesceKey::NudgeSelection),
            _ => None,
        }
    }
}

// ============================================================================
// 2. Undo 記録・Dirty 更新（このモジュール内でのみ呼び出し可能）
// ============================================================================

fn record_undo(
    doc: &mut EditorDoc,
    pre_canvas: Option<Vec<FillShape>>,
    coalesce: Option<CoalesceKey>,
) {
    if let Some(key) = coalesce {
        if doc.history.last_coalesce_key == Some(key) {
            return;
        }
    }
    let tl_snap = if let Some(fallback) = pre_canvas {
        let mut snapshot = doc.canvas.take_unsliced_snapshot().unwrap_or(fallback);
        let mut tl = doc.timeline.clone();
        tl.normalize_shape_layer_ids(&mut snapshot);
        tl.write_shapes_to_editable_cels(
            &snapshot,
            &doc.symbols.library,
            doc.canvas.loaded_signatures(),
        );
        tl
    } else {
        doc.canvas.restore_unsliced_preserving_selection();
        sync_shapes_to_timeline(doc);
        doc.timeline.clone()
    };
    let (library, symbol_edit_stack, symbols_revision) =
        doc.history.snapshot_symbols(&doc.symbols);
    let entry = UndoEntry {
        timeline: tl_snap,
        library,
        editing_group_path: doc.canvas.editing_group_path.clone(),
        symbol_edit_stack,
        stage_size: doc.viewport.stage_size,
        symbols_revision,
        commit_id: doc.history.current_commit_id,
    };
    doc.history.push(entry);
    doc.history.last_coalesce_key = coalesce;
}

fn push_undo(doc: &mut EditorDoc, coalesce: Option<CoalesceKey>) {
    record_undo(doc, None, coalesce);
}

fn commit_canvas_mutation_with_coalesce(
    doc: &mut EditorDoc,
    pre_mutation_canvas_snapshot: Vec<FillShape>,
    coalesce: Option<CoalesceKey>,
) {
    record_undo(doc, Some(pre_mutation_canvas_snapshot), coalesce);
    sync_shapes_to_timeline(doc);
}

fn commit_canvas_mutation(doc: &mut EditorDoc, pre_mutation_canvas_snapshot: Vec<FillShape>) {
    commit_canvas_mutation_with_coalesce(doc, pre_mutation_canvas_snapshot, None);
}

fn mutate_timeline_with_undo(doc: &mut EditorDoc, mutate: impl FnOnce(&mut Timeline)) {
    flush_canvas_before_timeline_op(doc);
    push_undo(doc, None);
    mutate(&mut doc.timeline);
    doc.symbols.sync_active_edit(&doc.timeline);
    load_shapes_from_timeline(doc);
}

// ============================================================================
// 3. 単一エントリポイント `apply`
// ============================================================================

/// すべての文書変更・編集操作の唯一の入口。
/// 再生停止・Undo記録・連続操作のCoalesce・タイムライン同期・Dirty更新を一元管理する。
pub(crate) fn apply(host: &mut impl ActionHost, action: Action) -> ApplyResult {
    if !matches!(action, Action::SetFps { .. }) {
        host.stop_playback();
    }
    let doc = host.doc_mut();
    let coalesce = action.coalesce_key(doc);

    match action {
        Action::PaintStroke {
            points,
            radius,
            color,
        } => {
            if points.is_empty() {
                return ApplyResult::Applied(false);
            }
            let prev_tool = doc.ui.tool;
            let prev_color = doc.ui.brush_color;
            doc.ui.tool = Tool::Brush;
            doc.ui.brush_color = color;
            let z = (doc.viewport.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
            let raw = generate_brush_outline_at_zoom(&points, radius as f64, z);
            let changed = apply_raw_outline(doc, &raw);
            doc.ui.tool = prev_tool;
            doc.ui.brush_color = prev_color;
            ApplyResult::Applied(changed)
        }
        Action::EraseStroke { points, radius } => {
            if points.is_empty() {
                return ApplyResult::Applied(false);
            }
            let prev_tool = doc.ui.tool;
            doc.ui.tool = Tool::Eraser;
            let z = (doc.viewport.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
            let raw = generate_brush_outline_at_zoom(&points, radius as f64, z);
            let changed = apply_raw_outline(doc, &raw);
            doc.ui.tool = prev_tool;
            ApplyResult::Applied(changed)
        }
        Action::CommitRawOutline { raw } => ApplyResult::Applied(apply_raw_outline(doc, &raw)),
        Action::BucketFill { pt, color } => {
            let prev_color = doc.ui.brush_color;
            doc.ui.brush_color = color;
            let changed = apply_bucket_fill(doc, pt);
            doc.ui.brush_color = prev_color;
            ApplyResult::Applied(changed)
        }

        Action::SelectSingleAt { pt } => ApplyResult::Applied(select_single_shape_at(doc, pt)),
        Action::ToggleSelectionAt { shape_idx } => {
            toggle_shape_selection(doc, shape_idx);
            ApplyResult::Applied(true)
        }
        Action::SelectMarquee { p0, p1, additive } => {
            apply_marquee_selection(doc, p0, p1, additive);
            ApplyResult::Applied(!doc.canvas.selected_shapes.is_empty())
        }
        Action::SelectAll => {
            clear_selection(doc);
            let editable_lids: std::collections::HashSet<u64> = doc
                .timeline
                .layers
                .iter()
                .filter(|l| l.visible && !l.locked)
                .map(|l| l.id)
                .collect();
            doc.canvas.select_shapes_in_layers(&editable_lids);
            ApplyResult::Applied(!doc.canvas.selected_shapes.is_empty())
        }
        Action::Deselect => {
            clear_selection(doc);
            ApplyResult::Applied(true)
        }
        Action::MoveSelected { delta } => {
            ApplyResult::Applied(apply_move_selected(doc, delta, None))
        }
        Action::NudgeSelection { delta } => {
            ApplyResult::Applied(apply_move_selected(doc, delta, coalesce))
        }
        Action::FinishMoveSelectionDrag {
            total_delta,
            undo_snapshot,
            orig_selected_ids,
        } => {
            let min_move = 1.0 / (doc.viewport.zoom as f64);
            let rolled_back =
                total_delta.hypot() <= min_move || doc.canvas.selected_shapes.is_empty();
            let applied = apply_finish_move_selection_drag(doc, total_delta, undo_snapshot);
            if rolled_back && !orig_selected_ids.is_empty() {
                doc.canvas.restore_selection_by_shape_ids(&orig_selected_ids);
            }
            ApplyResult::Applied(applied)
        }
        Action::CommitMovedSelection { undo_snapshot } => {
            ApplyResult::Applied(apply_commit_moved_selection(doc, undo_snapshot, None))
        }
        Action::DeleteSelection => {
            if let Some(snapshot) = prepare_delete_selected_shapes(doc) {
                commit_canvas_mutation(doc, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::ClearActiveFrame => {
            if let Some(snapshot) = prepare_clear_active_frame_shapes(doc) {
                commit_canvas_mutation(doc, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::Group => {
            if let Some((gid, snapshot)) = prepare_group_selected_shapes(doc) {
                commit_canvas_mutation(doc, snapshot);
                ApplyResult::CreatedId(Some(gid))
            } else {
                ApplyResult::CreatedId(None)
            }
        }
        Action::Ungroup => {
            if let Some(snapshot) = prepare_ungroup_selected_shapes(doc) {
                commit_canvas_mutation(doc, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::EnterGroupAt { pt } => ApplyResult::Applied(enter_group_at(doc, pt)),
        Action::ExitGroup => ApplyResult::Applied(exit_current_group(doc)),
        Action::ExitGroupSteps { steps } => {
            let mut exited = false;
            for _ in 0..steps {
                if !exit_current_group(doc) {
                    break;
                }
                exited = true;
            }
            ApplyResult::Applied(exited)
        }

        Action::ReshapeEdge {
            shape_idx,
            contour_idx,
            seg_idx,
            grab_t,
            grab_pos,
            target_pos,
            orig_seg,
            undo_snapshot,
        } => {
            let drag_dist = (target_pos - grab_pos).hypot();
            let changed = apply_deform_shape(doc, shape_idx, drag_dist, undo_snapshot, |deformed| {
                apply_pull_edge_to_shape(
                    deformed,
                    (contour_idx, seg_idx),
                    grab_t,
                    orig_seg,
                    target_pos,
                );
            });
            ApplyResult::Applied(changed)
        }
        Action::MoveVertex {
            shape_idx,
            contour_idx,
            seg_idx,
            orig_vertex,
            target_pos,
            orig_ctrls,
            undo_snapshot,
        } => {
            let drag_dist = (target_pos - orig_vertex).hypot();
            let changed = apply_deform_shape(doc, shape_idx, drag_dist, undo_snapshot, |deformed| {
                apply_move_vertex_to_shape(
                    deformed,
                    (contour_idx, seg_idx),
                    orig_vertex,
                    orig_ctrls,
                    target_pos,
                );
            });
            ApplyResult::Applied(changed)
        }
        Action::PullEdgeFromTo { from, to } => {
            ApplyResult::Applied(apply_pull_edge_from_to(doc, from, to))
        }
        Action::MoveVertexFromTo { from, to } => {
            ApplyResult::Applied(apply_move_vertex_from_to(doc, from, to))
        }
        Action::AddAnchor { pt } => ApplyResult::Applied(apply_anchor_edit_at(doc, pt, true)),
        Action::DeleteAnchor { pt } => ApplyResult::Applied(apply_anchor_edit_at(doc, pt, false)),

        Action::TransformSelection { affine } => {
            ApplyResult::Applied(apply_affine_to_selected_inner(doc, affine))
        }
        Action::ScaleSelection { sx, sy } => {
            let Some(bbox) = doc.canvas.selection_bounding_box(doc.viewport.zoom) else {
                return ApplyResult::Applied(false);
            };
            let pivot = doc.canvas.active_transform_pivot(bbox);
            ApplyResult::Applied(apply_affine_to_selected_inner(
                doc,
                affine_scale_around(pivot, sx, sy),
            ))
        }
        Action::FlipSelection { flip_x, flip_y } => {
            let Some(bbox) = doc.canvas.selection_bounding_box(doc.viewport.zoom) else {
                return ApplyResult::Applied(false);
            };
            let pivot = doc.canvas.active_transform_pivot(bbox);
            let sx = if flip_x { -1.0 } else { 1.0 };
            let sy = if flip_y { -1.0 } else { 1.0 };
            ApplyResult::Applied(apply_affine_to_selected_inner(
                doc,
                affine_scale_around(pivot, sx, sy),
            ))
        }
        Action::RotateSelection { angle_rad } => {
            let Some(bbox) = doc.canvas.selection_bounding_box(doc.viewport.zoom) else {
                return ApplyResult::Applied(false);
            };
            let pivot = doc.canvas.active_transform_pivot(bbox);
            ApplyResult::Applied(apply_affine_to_selected_inner(
                doc,
                affine_rotate_around(pivot, angle_rad),
            ))
        }
        Action::SetTransformPivot { world_pivot } => {
            ApplyResult::Applied(apply_set_selected_symbol_world_pivot(doc, world_pivot, true))
        }
        Action::ResetTransformPivot => {
            let mut changed = doc.canvas.transform_pivot.take().is_some();
            if apply_set_selected_symbol_world_pivot(doc, None, true) {
                changed = true;
            }
            ApplyResult::Applied(changed)
        }
        Action::CommitTransformDrag {
            changed,
            undo_snapshot,
        } => {
            if changed && !doc.canvas.selected_shapes.is_empty() {
                ApplyResult::Applied(apply_commit_moved_selection(doc, undo_snapshot, None))
            } else {
                doc.canvas.restore_drag_snapshot(undo_snapshot);
                ApplyResult::Applied(false)
            }
        }
        Action::CommitPivotDrag {
            changed,
            undo_snapshot,
        } => {
            if changed {
                if selected_single_symbol_info(doc).is_some() {
                    commit_canvas_mutation(doc, undo_snapshot);
                }
                ApplyResult::Applied(true)
            } else {
                doc.canvas.restore_drag_snapshot(undo_snapshot);
                ApplyResult::Applied(false)
            }
        }

        Action::Copy => ApplyResult::Applied(prepare_copy_selected_shapes(doc)),
        Action::Cut => {
            if !prepare_copy_selected_shapes(doc) {
                return ApplyResult::Applied(false);
            }
            if let Some(snapshot) = prepare_delete_selected_shapes(doc) {
                commit_canvas_mutation(doc, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::Paste { in_place } => ApplyResult::Applied(apply_paste_clipboard(doc, in_place)),

        Action::ConvertToSymbol => {
            if let Some((sym_id, target_lid, local_shapes, snapshot)) =
                prepare_convert_selection_to_symbol(doc)
            {
                commit_canvas_mutation(doc, snapshot);
                doc.symbols.register_new_symbol(local_shapes);
                if let Some(l_idx) = doc.timeline.layer_index_by_id(target_lid) {
                    doc.timeline.active_layer = l_idx;
                }
                doc.canvas.selection_modified = false;
                doc.canvas.transform_pivot = None;
                doc.ui.inspect_timeline_frame = false;
                ApplyResult::CreatedId(Some(sym_id))
            } else {
                ApplyResult::CreatedId(None)
            }
        }
        Action::PlaceSymbol { sym_id } => {
            if let Some(snapshot) = prepare_place_symbol_instance(doc, sym_id) {
                commit_canvas_mutation(doc, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::EnterSymbolTimeline { binding } => {
            ApplyResult::Applied(enter_symbol_timeline(doc, binding))
        }
        Action::EnterSymbolFromLibrary { sym_id } => {
            ApplyResult::Applied(enter_symbol_from_library(doc, sym_id))
        }
        Action::ExitSymbolTimeline => ApplyResult::Applied(exit_symbol_timeline(doc)),
        Action::ExitSymbolToDepth { depth } => {
            let mut exited = false;
            while doc.symbols.edit_stack.len() > depth {
                if !exit_symbol_timeline(doc) {
                    break;
                }
                exited = true;
            }
            ApplyResult::Applied(exited)
        }
        Action::RenameSymbol { sym_id, new_name } => {
            ApplyResult::Applied(apply_rename_symbol(doc, sym_id, new_name, coalesce))
        }
        Action::DeleteSymbol { sym_id } => {
            ApplyResult::Applied(apply_delete_symbol(doc, sym_id))
        }
        Action::SetSymbolPlayback {
            loop_mode,
            first_frame,
        } => ApplyResult::Applied(apply_set_symbol_playback(
            doc,
            loop_mode,
            first_frame,
            coalesce,
        )),
        Action::ToggleTween => ApplyResult::Applied(apply_timeline_toggle_tween(doc)),
        Action::SetTweenRotate { rotate } => {
            ApplyResult::Applied(apply_set_tween_rotate(doc, rotate))
        }

        Action::SwitchLayerAndFrame { layer_idx, frame } => {
            apply_switch_to_layer_and_frame(doc, layer_idx, frame);
            ApplyResult::Applied(true)
        }
        Action::ScrubRulerToFrame { frame } => {
            apply_scrub_ruler_to_frame(doc, frame);
            ApplyResult::Applied(true)
        }
        Action::SelectTimelineCells { l0, f0, l1, f1 } => {
            apply_select_timeline_cells(doc, l0, f0, l1, f1);
            ApplyResult::Applied(true)
        }
        Action::InsertKeyframe { copy_prev } => {
            ApplyResult::Applied(apply_timeline_insert_keyframe(doc, copy_prev))
        }
        Action::DeleteKeyframe => ApplyResult::Applied(apply_timeline_delete_keyframe(doc)),
        Action::ExtendFrame => ApplyResult::Applied(apply_timeline_extend_frame(doc)),
        Action::RemoveFrame => ApplyResult::Applied(apply_timeline_remove_frame(doc)),
        Action::MoveOrCopyFrames {
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        } => ApplyResult::Applied(apply_timeline_move_or_copy_frames(
            doc,
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        )),
        Action::AddLayer => {
            apply_timeline_add_layer(doc);
            ApplyResult::Applied(true)
        }
        Action::DeleteActiveLayer => {
            ApplyResult::Applied(apply_timeline_delete_active_layer(doc))
        }
        Action::MoveActiveLayer { up } => {
            ApplyResult::Applied(apply_timeline_move_active_layer(doc, up))
        }
        Action::ReorderLayer { src_idx, dst_idx } => {
            ApplyResult::Applied(apply_timeline_reorder_layer(doc, src_idx, dst_idx))
        }
        Action::SetLayerVisible { layer_idx, visible } => {
            apply_set_layer_visible(doc, layer_idx, visible);
            ApplyResult::Applied(true)
        }
        Action::SetLayerLocked { layer_idx, locked } => {
            apply_set_layer_locked(doc, layer_idx, locked);
            ApplyResult::Applied(true)
        }
        Action::SetTotalFrames { total } => {
            ApplyResult::Applied(apply_set_total_frames(doc, total, coalesce))
        }
        Action::SetFps { fps } => ApplyResult::Applied(apply_set_fps(doc, fps, coalesce)),
        Action::SetStageSize { size } => {
            ApplyResult::Applied(apply_set_stage_size(doc, size, coalesce))
        }

        Action::SetTool { tool } => {
            set_tool(doc, tool);
            ApplyResult::Applied(true)
        }
        Action::ToggleSelectSubMode { mode } => {
            toggle_select_sub_mode(doc, mode);
            ApplyResult::Applied(true)
        }
        Action::Undo => {
            doc.undo();
            ApplyResult::Applied(true)
        }
        Action::Redo => {
            doc.redo();
            ApplyResult::Applied(true)
        }
        Action::EndCoalesce => {
            doc.history.end_coalesce();
            ApplyResult::Applied(true)
        }
    }
}

// ============================================================================
// 4. 各アクションの内部実装
// ============================================================================

fn apply_raw_outline(doc: &mut EditorDoc, raw: &[Point]) -> bool {
    let target_lid = active_scope_layer_id(&doc.canvas, &doc.timeline);
    if !doc.timeline.is_layer_editable_by_id(target_lid) {
        return false;
    }
    let stroke_shapes: Vec<FillShape> =
        build_smoothed_stroke_shapes_at_zoom(raw, doc.ui.brush_color, doc.viewport.zoom as f64)
            .into_iter()
            .map(|s| s.with_scope(target_lid, &doc.canvas.editing_group_path))
            .collect();
    if stroke_shapes.is_empty() {
        return false;
    }
    if doc.ui.tool == Tool::Brush {
        clear_selection(doc);
        let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
        merge_shapes_into_canvas(&mut doc.canvas.shapes, stroke_shapes);
        doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
        commit_canvas_mutation(doc, snapshot);
        true
    } else {
        let has_bbox_overlap = doc.canvas.shapes.iter().any(|s| {
            s.layer_id == target_lid
                && s.is_raw_at_scope(&doc.canvas.editing_group_path)
                && stroke_shapes
                    .iter()
                    .any(|er| rects_overlap(s.bounding_box(), er.bounding_box()))
        });
        if !has_bbox_overlap {
            return false;
        }
        clear_selection(doc);
        let prev_revs: Vec<(u64, u64)> = doc
            .canvas
            .shapes
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
        erase_from_canvas(
            &mut doc.canvas.shapes,
            &stroke_shapes,
            target_lid,
            &doc.canvas.editing_group_path,
        );
        let unchanged = doc.canvas.shapes.len() == prev_revs.len()
            && doc
                .canvas
                .shapes
                .iter()
                .zip(prev_revs.iter())
                .all(|(s, &(id, rev))| s.shape_id == id && s.mesh_revision == rev);
        if !unchanged {
            commit_canvas_mutation(doc, snapshot);
            true
        } else {
            false
        }
    }
}

fn collect_all_visible_shapes(doc: &EditorDoc) -> Vec<FillShape> {
    let mut out = Vec::with_capacity(doc.canvas.shapes.len());
    for (layer_idx, layer) in doc.timeline.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        if layer.locked {
            out.extend(doc.timeline.evaluate_layer_shapes_at(
                layer_idx,
                doc.timeline.current_frame,
                &doc.symbols.library,
            ));
        } else {
            let lid = layer.id;
            out.extend(
                doc.canvas
                    .shapes
                    .iter()
                    .filter(|s| s.layer_id == lid)
                    .cloned(),
            );
        }
    }
    out
}

fn apply_bucket_fill(doc: &mut EditorDoc, pt: Point) -> bool {
    let fill_color = doc.ui.brush_color;
    let target_lid = active_scope_layer_id(&doc.canvas, &doc.timeline);
    if !doc.timeline.is_layer_editable_by_id(target_lid) {
        return false;
    }

    // 0. 選択領域が存在する場合：選択中の「生シェイプ」のみにバケツで色を流し込む（グループは無効）
    if !doc.canvas.selected_shapes.is_empty() {
        let clicked_selected_raw = doc.canvas.selected_shapes.iter().copied().any(|idx| {
            doc.canvas.shapes.get(idx).is_some_and(|s| {
                doc.timeline.is_layer_editable_by_id(s.layer_id)
                    && s.is_raw_at_scope(&doc.canvas.editing_group_path)
                    && s.contains_point(pt)
            })
        });
        if clicked_selected_raw {
            let any_color_diff = doc.canvas.selected_shapes.iter().copied().any(|idx| {
                doc.canvas.shapes.get(idx).is_some_and(|s| {
                    doc.timeline.is_layer_editable_by_id(s.layer_id)
                        && s.is_raw_at_scope(&doc.canvas.editing_group_path)
                        && s.color != fill_color
                })
            });
            if !any_color_diff {
                return false;
            }
            let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
            for &sel_idx in &doc.canvas.selected_shapes {
                if let Some(shape) = doc.canvas.shapes.get_mut(sel_idx) {
                    if doc.timeline.is_layer_editable_by_id(shape.layer_id)
                        && shape.is_raw_at_scope(&doc.canvas.editing_group_path)
                    {
                        shape.set_color(fill_color);
                    }
                }
            }
            doc.canvas.selection_modified = true;
            commit_canvas_mutation(doc, snapshot);
            return true;
        }
        return false;
    }

    let all_visible_shapes = collect_all_visible_shapes(doc);
    let target_layer_ord = doc.timeline.layer_index_by_id(target_lid).unwrap_or(0);

    if let Some(hit_idx) = (0..doc.canvas.shapes.len()).rev().find(|&idx| {
        let s = &doc.canvas.shapes[idx];
        s.layer_id == target_lid
            && s.is_in_editing_scope(&doc.canvas.editing_group_path)
            && s.contains_point(pt)
    }) {
        let occluded_above = (target_layer_ord + 1..doc.timeline.layers.len()).any(|l_idx| {
            let layer = &doc.timeline.layers[l_idx];
            layer.visible
                && all_visible_shapes
                    .iter()
                    .any(|s| s.layer_id == layer.id && s.contains_point(pt))
        });

        let hit_shape = &doc.canvas.shapes[hit_idx];
        if !occluded_above && hit_shape.is_raw_at_scope(&doc.canvas.editing_group_path) {
            if hit_shape.color == fill_color {
                return false;
            }
            let hit_bbox = hit_shape.bounding_box();
            let overlapping_walls: Vec<FillShape> = all_visible_shapes
                .iter()
                .filter(|s| {
                    (s.layer_id != target_lid
                        || !s.is_raw_at_scope(&doc.canvas.editing_group_path))
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
                        let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
                        merge_shapes_into_canvas(
                            &mut doc.canvas.shapes,
                            vec![split_piece
                                .with_scope(target_lid, &doc.canvas.editing_group_path)],
                        );
                        doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
                        commit_canvas_mutation(doc, snapshot);
                        return true;
                    }
                }
            }

            focus_active_layer_to_shape(doc, hit_idx);
            let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
            let mut target = doc.canvas.shapes.remove(hit_idx);
            target.set_color(fill_color);
            merge_disjoint_fill_into_canvas(&mut doc.canvas.shapes, target);
            doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
            commit_canvas_mutation(doc, snapshot);
            return true;
        }
        return false;
    }

    let wall_shapes: Vec<FillShape> = all_visible_shapes
        .iter()
        .filter(|s| !s.contains_point(pt))
        .cloned()
        .collect();
    let enclosed_opt = find_enclosed_region_at(&wall_shapes, pt, fill_color).or_else(|| {
        let active_layer_raw: Vec<FillShape> = doc
            .canvas
            .shapes
            .iter()
            .filter(|s| {
                s.layer_id == target_lid
                    && s.is_raw_at_scope(&doc.canvas.editing_group_path)
                    && !s.contains_point(pt)
            })
            .cloned()
            .collect();
        (active_layer_raw.len() < wall_shapes.len())
            .then(|| find_enclosed_region_at(&active_layer_raw, pt, fill_color))
            .flatten()
    });
    if let Some(enclosed_fill) = enclosed_opt {
        let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
        merge_disjoint_fill_into_canvas(
            &mut doc.canvas.shapes,
            enclosed_fill.with_scope(target_lid, &doc.canvas.editing_group_path),
        );
        doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
        commit_canvas_mutation(doc, snapshot);
        return true;
    }

    false
}

fn apply_commit_moved_selection(
    doc: &mut EditorDoc,
    undo_snapshot: Vec<FillShape>,
    coalesce: Option<CoalesceKey>,
) -> bool {
    if !prepare_moved_selection(doc, &undo_snapshot) {
        return false;
    }
    commit_canvas_mutation_with_coalesce(doc, undo_snapshot, coalesce);
    true
}

fn apply_finish_move_selection_drag(
    doc: &mut EditorDoc,
    total_delta: KVec2,
    undo_snapshot: Vec<FillShape>,
) -> bool {
    let min_move = 1.0 / (doc.viewport.zoom as f64);
    if total_delta.hypot() > min_move && !doc.canvas.selected_shapes.is_empty() {
        apply_commit_moved_selection(doc, undo_snapshot, None)
    } else {
        doc.canvas.restore_drag_snapshot(undo_snapshot);
        false
    }
}

fn apply_move_selected(
    doc: &mut EditorDoc,
    delta: KVec2,
    coalesce: Option<CoalesceKey>,
) -> bool {
    cancel_active_drag(doc);
    if delta == KVec2::ZERO || doc.canvas.selected_shapes.is_empty() {
        return false;
    }
    let undo_snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
    doc.canvas.translate_selected(delta);
    apply_commit_moved_selection(doc, undo_snapshot, coalesce)
}

fn apply_deform_shape(
    doc: &mut EditorDoc,
    shape_idx: usize,
    drag_dist: f64,
    undo_snapshot: Vec<FillShape>,
    apply_deform: impl FnOnce(&mut FillShape),
) -> bool {
    let min_deform = 0.5 / (doc.viewport.zoom as f64);
    if drag_dist > min_deform && shape_idx < doc.canvas.shapes.len() {
        focus_active_layer_to_shape(doc, shape_idx);
        let mut deformed = doc.canvas.shapes.remove(shape_idx);
        apply_deform(&mut deformed);
        let resolved = resolve_deformed_shape(&deformed);
        merge_shapes_into_canvas(&mut doc.canvas.shapes, resolved);
        doc.canvas.sort_shapes_by_layer_order(&doc.timeline);
        commit_canvas_mutation(doc, undo_snapshot);
        true
    } else {
        doc.canvas.restore_drag_snapshot(undo_snapshot);
        false
    }
}

fn apply_pull_edge_from_to(doc: &mut EditorDoc, from: Point, to: Point) -> bool {
    clear_selection(doc);
    let target = compute_hover_target(
        &doc.canvas,
        from,
        doc.viewport.zoom,
        SelectSubMode::Reshape,
    );
    let crate::canvas_tool::HoverTarget::Edge {
        shape_idx,
        contour_idx,
        seg_idx,
        t,
        ..
    } = target
    else {
        return false;
    };
    let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
    let Some(orig_seg) = doc
        .canvas
        .get_contour_segment(shape_idx, contour_idx, seg_idx)
    else {
        return false;
    };
    let grab_t = t.clamp(0.18, 0.82);
    let grab_pos = orig_seg.eval(grab_t);
    let drag_dist = (to - grab_pos).hypot();
    apply_deform_shape(doc, shape_idx, drag_dist, snapshot, |deformed| {
        apply_pull_edge_to_shape(deformed, (contour_idx, seg_idx), grab_t, orig_seg, to);
    })
}

fn apply_move_vertex_from_to(doc: &mut EditorDoc, from: Point, to: Point) -> bool {
    clear_selection(doc);
    let target = compute_hover_target(
        &doc.canvas,
        from,
        doc.viewport.zoom,
        SelectSubMode::Reshape,
    );
    let crate::canvas_tool::HoverTarget::Vertex {
        shape_idx,
        contour_idx,
        seg_idx,
        pos,
    } = target
    else {
        return false;
    };
    let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
    let Some((orig_prev_ctrl, orig_curr_ctrl)) = doc
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
    else {
        return false;
    };
    let drag_dist = (to - pos).hypot();
    apply_deform_shape(doc, shape_idx, drag_dist, snapshot, |deformed| {
        apply_move_vertex_to_shape(
            deformed,
            (contour_idx, seg_idx),
            pos,
            (orig_prev_ctrl, orig_curr_ctrl),
            to,
        );
    })
}

fn apply_anchor_edit_at(doc: &mut EditorDoc, pt: Point, is_add: bool) -> bool {
    let zoom = doc.viewport.zoom;
    let sub_mode = if is_add {
        SelectSubMode::AddAnchor
    } else {
        SelectSubMode::DeleteAnchor
    };
    let target = compute_hover_target(&doc.canvas, pt, zoom, sub_mode);
    let had_unsliced = doc.canvas.has_unsliced_snapshot();
    let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
    let modified_shape = match (is_add, target) {
        (
            true,
            crate::canvas_tool::HoverTarget::Edge {
                shape_idx,
                contour_idx,
                seg_idx,
                t,
                ..
            },
        ) => doc
            .canvas
            .shapes
            .get_mut(shape_idx)
            .and_then(|s| s.get_contour_mut(contour_idx))
            .is_some_and(|c| c.insert_anchor_at(seg_idx, t))
            .then_some(shape_idx),
        (
            false,
            crate::canvas_tool::HoverTarget::Vertex {
                shape_idx,
                contour_idx,
                seg_idx,
                ..
            },
        ) => doc
            .canvas
            .shapes
            .get_mut(shape_idx)
            .and_then(|s| s.get_contour_mut(contour_idx))
            .is_some_and(|c| c.delete_anchor_at(seg_idx))
            .then_some(shape_idx),
        _ => None,
    };
    if let Some(s_idx) = modified_shape {
        if let Some(shape) = doc.canvas.shapes.get_mut(s_idx) {
            shape.rebuild_cache();
        }
        commit_canvas_mutation(doc, snapshot);
        true
    } else {
        if had_unsliced {
            doc.canvas.restore_unsliced_snapshot(snapshot);
        }
        false
    }
}

pub(crate) fn update_selected_symbol_world_pivot_live(
    doc: &mut EditorDoc,
    world_pivot: Option<Point>,
) -> bool {
    apply_set_selected_symbol_world_pivot(doc, world_pivot, false)
}

fn apply_set_selected_symbol_world_pivot(
    doc: &mut EditorDoc,
    world_pivot: Option<Point>,
    commit_to_timeline: bool,
) -> bool {
    let Some(binding) = doc.canvas.selected_single_symbol_binding() else {
        return false;
    };
    let new_local_pivot = match world_pivot {
        Some(pt) => {
            if binding.xform.determinant().abs() <= 1e-9 {
                return false;
            }
            binding.xform.inverse() * pt
        }
        None => Point::ZERO,
    };
    if (new_local_pivot - binding.pivot).hypot() <= 1e-7 {
        return true;
    }
    if !doc.canvas.selection_modified {
        doc.canvas.restore_unsliced_preserving_selection();
    }
    let undo_snapshot =
        commit_to_timeline.then(|| doc.canvas.take_or_clone_unsliced_snapshot());
    doc.canvas
        .update_symbol_instance_binding(binding.instance_gid, |b| b.pivot = new_local_pivot);
    if let Some(snapshot) = undo_snapshot {
        commit_canvas_mutation(doc, snapshot);
    }
    true
}

pub(crate) fn flush_transform_pivot_to_symbol(doc: &mut EditorDoc) {
    if let Some(piv) = doc.canvas.transform_pivot {
        if update_selected_symbol_world_pivot_live(doc, Some(piv)) {
            doc.canvas.transform_pivot = None;
        }
    }
}

fn apply_affine_to_selected_inner(doc: &mut EditorDoc, affine: Affine) -> bool {
    cancel_active_drag(doc);
    if doc.canvas.selected_shapes.is_empty() {
        return false;
    }
    let undo_snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
    flush_transform_pivot_to_symbol(doc);
    let any_transformed = doc.canvas.transform_selected(affine);
    if any_transformed {
        apply_commit_moved_selection(doc, undo_snapshot, None);
    }
    any_transformed
}

fn apply_paste_clipboard(doc: &mut EditorDoc, in_place: bool) -> bool {
    if doc.canvas.clipboard.is_empty() {
        return false;
    }
    let target_lid = active_scope_layer_id(&doc.canvas, &doc.timeline);
    if !doc.timeline.is_layer_editable_by_id(target_lid) {
        doc.notify_warning("現在のレイヤーまたはフレームは編集できないためペーストできません");
        return false;
    }
    if would_clipboard_paste_all_cycle(doc) {
        doc.notify_warning("編集中のシンボル自身や親シンボルは循環参照になるため配置できません");
        return false;
    }

    clear_selection(doc);
    push_undo(doc, None);
    let (resolved, had_cycle) = resolve_clipboard_for_paste(doc);
    if had_cycle {
        doc.notify_warning(
            "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
        );
    }
    if !resolved.is_empty() {
        let offset = if in_place {
            KVec2::ZERO
        } else {
            if doc.canvas.paste_count == 0 {
                doc.canvas.paste_offset = KVec2::ZERO;
            }
            doc.canvas.paste_count += 1;
            let step = 16.0 / (doc.viewport.zoom as f64);
            doc.canvas.paste_offset += KVec2::new(step, step);
            doc.canvas.paste_offset
        };

        let same_timeline_context = doc.canvas.clipboard_source_doc_id == doc.doc_id
            && doc.canvas.clipboard_source_symbol_id
                == doc.symbols.edit_stack.last().map(|s| s.symbol_id)
            && doc.canvas.editing_group_path.is_empty();
        let multi_layer_copy = same_timeline_context
            && resolved
                .first()
                .is_some_and(|first| resolved.iter().any(|s| s.layer_id != first.layer_id));
        let (pasted_shapes, any_raw) =
            instantiate_relative_shapes_in_scope(doc, &resolved, offset, multi_layer_copy);

        insert_and_select_shapes_in_scope(doc, pasted_shapes, any_raw);
        sync_shapes_to_timeline(doc);
    }
    true
}

fn apply_rename_symbol(
    doc: &mut EditorDoc,
    sym_id: u64,
    new_name: String,
    coalesce: Option<CoalesceKey>,
) -> bool {
    if doc
        .symbols
        .find_symbol(sym_id)
        .is_none_or(|s| s.name == new_name)
    {
        return false;
    }
    push_undo(doc, coalesce);
    doc.symbols.rename_symbol(sym_id, new_name)
}

fn apply_delete_symbol(doc: &mut EditorDoc, sym_id: u64) -> bool {
    if doc.symbols.find_symbol(sym_id).is_none() {
        return false;
    }
    while doc
        .symbols
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == sym_id)
    {
        exit_symbol_timeline(doc);
    }
    clear_selection(doc);
    push_undo(doc, None);

    doc.symbols
        .remove_symbol_and_detach(sym_id, &mut doc.timeline, &mut doc.canvas.shapes);

    doc.symbols.sync_active_edit(&doc.timeline);
    load_shapes_from_timeline(doc);
    true
}

fn apply_timeline_toggle_tween(doc: &mut EditorDoc) -> bool {
    let targets = doc.timeline.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    let prev_selected_ids = doc.canvas.selected_shape_ids();
    flush_canvas_before_timeline_op(doc);

    let (can_toggle, is_currently_tweened) = doc.timeline.can_toggle_tween_at_current_frame();
    if !can_toggle {
        doc.canvas
            .restore_selection_by_shape_ids(&prev_selected_ids);
        doc.notify_warning(
            "トゥイーンを設定するには、キーフレームに単一のシンボルインスタンス（F8）のみを配置してください",
        );
        return false;
    }

    push_undo(doc, None);
    let curr_f = doc.timeline.current_frame;
    let next_tween = !is_currently_tweened;
    let mut changed = false;

    for &l_idx in &targets {
        let layer = &mut doc.timeline.layers[l_idx];
        if let Some(kf_idx) = layer.keyframe_index_at(curr_f) {
            let kf = &mut layer.keyframes[kf_idx];
            if next_tween {
                if kf.cel.single_symbol_instance().is_some() && !kf.tween {
                    kf.tween = true;
                    changed = true;
                }
            } else if kf.tween {
                kf.tween = false;
                changed = true;
            }
        }
    }

    if changed {
        load_shapes_from_timeline(doc);
        select_shapes_for_timeline_selection(doc);
        doc.ui.inspect_timeline_frame = true;
    }
    changed
}

fn apply_set_tween_rotate(doc: &mut EditorDoc, rotate_mode: TweenRotate) -> bool {
    let Some(layer) = doc.timeline.layers.get(doc.timeline.active_layer) else {
        return false;
    };
    if layer.locked || !layer.visible {
        return false;
    }
    let Some(kf_idx) = layer.keyframe_index_at(doc.timeline.current_frame) else {
        return false;
    };
    if !layer.keyframes[kf_idx].tween || layer.keyframes[kf_idx].tween_rotate == rotate_mode {
        return false;
    }

    flush_canvas_before_timeline_op(doc);
    push_undo(doc, None);
    if let Some(layer_mut) = doc.timeline.layers.get_mut(doc.timeline.active_layer) {
        if let Some(kf) = layer_mut.keyframes.get_mut(kf_idx) {
            kf.tween_rotate = rotate_mode;
        }
    }
    doc.symbols.sync_active_edit(&doc.timeline);
    load_shapes_from_timeline(doc);
    select_shapes_for_timeline_selection(doc);
    doc.ui.inspect_timeline_frame = true;
    true
}

fn apply_set_symbol_playback(
    doc: &mut EditorDoc,
    loop_mode: Option<SymbolLoopMode>,
    first_frame: Option<usize>,
    coalesce: Option<CoalesceKey>,
) -> bool {
    let Some((binding, _, total_frames)) = selected_single_symbol_info(doc) else {
        return false;
    };
    let next_mode = loop_mode.unwrap_or(binding.loop_mode);
    let clamped_first = first_frame
        .unwrap_or(binding.first_frame)
        .min(total_frames.saturating_sub(1));
    if binding.loop_mode == next_mode && binding.first_frame == clamped_first {
        return false;
    }

    let inst_gid = binding.instance_gid;
    let sym_id = binding.symbol_id;
    let prev_coalesce = doc.history.last_coalesce_key;
    clear_selection(doc);
    sync_shapes_to_timeline(doc);
    doc.history.last_coalesce_key = prev_coalesce;
    push_undo(doc, coalesce);

    if doc.symbols.find_symbol(sym_id).is_none() {
        return false;
    }

    let curr_f = doc.timeline.current_frame;
    let mut updated = false;
    for layer in &mut doc.timeline.layers {
        if !layer.visible || layer.locked {
            continue;
        }
        let Some(kf_idx) = layer.keyframe_index_at(curr_f) else {
            continue;
        };
        let kf = &mut layer.keyframes[kf_idx];
        if kf.cel.update_symbol_instances_with(|mut b| {
            (b.instance_gid == inst_gid).then(|| {
                b.loop_mode = next_mode;
                b.first_frame = clamped_first;
                b
            })
        }) {
            updated = true;
        }
    }

    if updated {
        doc.symbols.sync_active_edit(&doc.timeline);
        load_shapes_from_timeline(doc);
        doc.canvas.select_symbol_instance_by_gid(inst_gid);
    }
    updated
}

fn apply_switch_to_layer_and_frame(doc: &mut EditorDoc, layer_idx: usize, frame_idx: usize) {
    if doc.timeline.layers.is_empty() {
        return;
    }
    flush_canvas_before_timeline_op(doc);
    doc.timeline.select_single_cell(layer_idx, frame_idx);
    load_shapes_from_timeline(doc);
    doc.ui.inspect_timeline_frame = true;
}

fn apply_scrub_ruler_to_frame(doc: &mut EditorDoc, frame_idx: usize) {
    let clamped_frame = frame_idx.min(doc.timeline.total_frames.saturating_sub(1));
    if doc.timeline.current_frame == clamped_frame && doc.timeline.cell_selection.is_none() {
        return;
    }
    flush_canvas_before_timeline_op(doc);
    doc.timeline.current_frame = clamped_frame;
    doc.timeline.cell_selection = None;
    load_shapes_from_timeline(doc);
}

fn apply_select_timeline_cells(
    doc: &mut EditorDoc,
    start_layer: usize,
    start_frame: usize,
    end_layer: usize,
    end_frame: usize,
) {
    if doc.timeline.layers.is_empty() {
        return;
    }
    let max_l = doc.timeline.layers.len() - 1;
    let (l0, l1) = (start_layer.min(max_l), end_layer.min(max_l));
    let (f0, f1) = (start_frame, end_frame);

    apply_switch_to_layer_and_frame(doc, l1, f1);
    doc.timeline.cell_selection = Some(TimelineCellSelection::from_corners(l0, f0, l1, f1));
    select_shapes_for_timeline_selection(doc);
}

fn apply_timeline_insert_keyframe(doc: &mut EditorDoc, clone_prev: bool) -> bool {
    let targets = doc.timeline.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    mutate_timeline_with_undo(doc, |tl| {
        tl.insert_keyframe_on_targets(&targets, clone_prev);
    });
    if clone_prev {
        select_shapes_for_timeline_selection(doc);
    }
    true
}

fn apply_timeline_delete_keyframe(doc: &mut EditorDoc) -> bool {
    let targets = doc.timeline.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    flush_canvas_before_timeline_op(doc);
    if !doc.timeline.can_delete_keyframe_on_targets(&targets) {
        return false;
    }
    mutate_timeline_with_undo(doc, |tl| {
        tl.delete_keyframe_on_targets(&targets);
    });
    true
}

fn apply_timeline_extend_frame(doc: &mut EditorDoc) -> bool {
    let targets = doc.timeline.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    mutate_timeline_with_undo(doc, |tl| {
        tl.extend_frame_on_targets(&targets);
    });
    true
}

fn apply_timeline_remove_frame(doc: &mut EditorDoc) -> bool {
    let targets = doc.timeline.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    flush_canvas_before_timeline_op(doc);
    if !doc.timeline.can_remove_frame_on_targets(&targets) {
        return false;
    }
    let mut any_changed = false;
    mutate_timeline_with_undo(doc, |tl| {
        any_changed = tl.remove_frame_on_targets(&targets);
    });
    any_changed
}

fn apply_timeline_move_or_copy_frames(
    doc: &mut EditorDoc,
    sel: TimelineCellSelection,
    delta_layer: isize,
    delta_frame: isize,
    copy_mode: bool,
) -> bool {
    let Some((dst_sel, actual_dl)) = doc
        .timeline
        .resolve_frame_move_bounds(sel, delta_layer, delta_frame)
    else {
        return false;
    };
    mutate_timeline_with_undo(doc, |tl| {
        tl.move_or_copy_frames(sel, dst_sel, actual_dl, copy_mode);
    });
    doc.symbols.next_group_id = doc
        .symbols
        .next_group_id
        .max(doc.timeline.max_group_id().saturating_add(1));
    select_shapes_for_timeline_selection(doc);
    true
}

fn apply_timeline_add_layer(doc: &mut EditorDoc) {
    if doc.timeline.layers.len() >= MAX_LAYERS_PER_TIMELINE {
        return;
    }
    mutate_timeline_with_undo(doc, Timeline::add_layer_above_active);
}

fn apply_timeline_delete_active_layer(doc: &mut EditorDoc) -> bool {
    if doc.timeline.layers.len() <= 1 {
        return false;
    }
    mutate_timeline_with_undo(doc, Timeline::delete_active_layer);
    true
}

fn apply_timeline_reorder_layer(doc: &mut EditorDoc, from_idx: usize, to_idx: usize) -> bool {
    let len = doc.timeline.layers.len();
    if from_idx >= len || to_idx >= len || from_idx == to_idx {
        return false;
    }
    mutate_timeline_with_undo(doc, |tl| {
        tl.reorder_layer(from_idx, to_idx);
    });
    true
}

fn apply_timeline_move_active_layer(doc: &mut EditorDoc, up: bool) -> bool {
    let len = doc.timeline.layers.len();
    let curr = doc.timeline.active_layer;
    let target = if up {
        if curr + 1 >= len {
            return false;
        }
        curr + 1
    } else {
        if curr == 0 {
            return false;
        }
        curr - 1
    };
    apply_timeline_reorder_layer(doc, curr, target)
}

fn apply_set_layer_visible(doc: &mut EditorDoc, layer_idx: usize, visible: bool) {
    if doc
        .timeline
        .layers
        .get(layer_idx)
        .is_none_or(|l| l.visible == visible)
    {
        return;
    }
    mutate_timeline_with_undo(doc, |tl| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            layer.visible = visible;
        }
    });
}

fn apply_set_layer_locked(doc: &mut EditorDoc, layer_idx: usize, locked: bool) {
    if doc
        .timeline
        .layers
        .get(layer_idx)
        .is_none_or(|l| l.locked == locked)
    {
        return;
    }
    mutate_timeline_with_undo(doc, |tl| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            layer.locked = locked;
        }
    });
}

fn apply_set_stage_size(
    doc: &mut EditorDoc,
    next_size: Vec2,
    coalesce: Option<CoalesceKey>,
) -> bool {
    if (next_size.x - doc.viewport.stage_size.x).abs() <= f32::EPSILON
        && (next_size.y - doc.viewport.stage_size.y).abs() <= f32::EPSILON
    {
        return false;
    }
    push_undo(doc, coalesce);
    doc.viewport.stage_size = next_size;
    true
}

fn apply_set_total_frames(
    doc: &mut EditorDoc,
    next_total: usize,
    coalesce: Option<CoalesceKey>,
) -> bool {
    let next_total = next_total.max(1);
    if next_total != doc.timeline.total_frames {
        if doc.history.last_coalesce_key != coalesce || coalesce.is_none() {
            flush_canvas_before_timeline_op(doc);
        }
        push_undo(doc, coalesce);
        doc.timeline.resize_total_frames(next_total);
        doc.symbols.sync_active_edit(&doc.timeline);
        load_shapes_from_timeline(doc);
        return true;
    }
    false
}

fn apply_set_fps(doc: &mut EditorDoc, next_fps: u32, coalesce: Option<CoalesceKey>) -> bool {
    let next_fps = next_fps.clamp(1, 60);
    if next_fps != doc.timeline.fps {
        if (doc.history.last_coalesce_key != coalesce || coalesce.is_none())
            && doc.canvas.selected_shapes.is_empty()
            && !doc.canvas.has_unsliced_snapshot()
        {
            flush_canvas_before_timeline_op(doc);
        }
        push_undo(doc, coalesce);
        doc.timeline.fps = next_fps;
        for sess in &mut doc.symbols.edit_stack {
            sess.parent_timeline.fps = next_fps;
        }
        doc.symbols.sync_active_edit(&doc.timeline);
        return true;
    }
    false
}

// ============================================================================
// 5. テスト向けのラッパー関数（すべて `apply` を経由）
// ============================================================================

#[cfg(test)]
pub(crate) fn commit_raw_outline(doc: &mut EditorDoc, raw: &[Point]) {
    apply(doc, Action::CommitRawOutline { raw: raw.to_vec() });
}

#[cfg(test)]
pub(crate) fn bucket_fill_at(doc: &mut EditorDoc, pt: Point) -> bool {
    let color = doc.ui.brush_color;
    apply(doc, Action::BucketFill { pt, color }).as_bool()
}

#[cfg(test)]
pub(crate) fn commit_moved_selection(doc: &mut EditorDoc, undo_snapshot: Vec<FillShape>) {
    apply(doc, Action::CommitMovedSelection { undo_snapshot });
}

#[cfg(test)]
pub(crate) fn finish_move_selection_drag(
    doc: &mut EditorDoc,
    total_delta: KVec2,
    undo_snapshot: Vec<FillShape>,
) {
    apply(
        doc,
        Action::FinishMoveSelectionDrag {
            total_delta,
            undo_snapshot,
            orig_selected_ids: Vec::new(),
        },
    );
}

#[cfg(test)]
pub(crate) fn group_selected_shapes(doc: &mut EditorDoc) -> Option<u64> {
    apply(doc, Action::Group).id()
}

#[cfg(test)]
pub(crate) fn ungroup_selected_shapes(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::Ungroup).as_bool()
}

#[cfg(test)]
pub(crate) fn nudge_selected_shapes(doc: &mut EditorDoc, delta: KVec2) -> bool {
    apply(doc, Action::NudgeSelection { delta }).as_bool()
}

#[cfg(test)]
pub(crate) fn delete_selected_shapes(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::DeleteSelection).as_bool()
}

#[cfg(test)]
pub(crate) fn clear_active_frame_shapes(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::ClearActiveFrame).as_bool()
}

#[cfg(test)]
pub(crate) fn copy_selected_shapes(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::Copy).as_bool()
}

#[cfg(test)]
pub(crate) fn cut_selected_shapes(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::Cut).as_bool()
}

#[cfg(test)]
pub(crate) fn paste_clipboard(doc: &mut EditorDoc, in_place: bool) -> bool {
    apply(doc, Action::Paste { in_place }).as_bool()
}

#[cfg(test)]
pub(crate) fn set_selected_symbol_world_pivot(
    doc: &mut EditorDoc,
    world_pivot: Option<Point>,
    commit_to_timeline: bool,
) -> bool {
    if commit_to_timeline {
        apply(doc, Action::SetTransformPivot { world_pivot }).as_bool()
    } else {
        update_selected_symbol_world_pivot_live(doc, world_pivot)
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn apply_affine_to_selected(doc: &mut EditorDoc, affine: Affine) -> bool {
    apply(doc, Action::TransformSelection { affine }).as_bool()
}

#[cfg(test)]
pub(crate) fn scale_selected_shapes(doc: &mut EditorDoc, sx: f64, sy: f64) -> bool {
    apply(doc, Action::ScaleSelection { sx, sy }).as_bool()
}

#[cfg(test)]
pub(crate) fn flip_selected_shapes(doc: &mut EditorDoc, flip_x: bool, flip_y: bool) -> bool {
    apply(doc, Action::FlipSelection { flip_x, flip_y }).as_bool()
}

#[cfg(test)]
pub(crate) fn rotate_selected_shapes(doc: &mut EditorDoc, angle_rad: f64) -> bool {
    apply(doc, Action::RotateSelection { angle_rad }).as_bool()
}

#[cfg(test)]
pub(crate) fn convert_selection_to_symbol(doc: &mut EditorDoc) -> Option<u64> {
    apply(doc, Action::ConvertToSymbol).id()
}

#[cfg(test)]
pub(crate) fn place_symbol_instance_from_library(doc: &mut EditorDoc, sym_id: u64) -> bool {
    apply(doc, Action::PlaceSymbol { sym_id }).as_bool()
}

#[cfg(test)]
pub(crate) fn rename_symbol_in_library(
    doc: &mut EditorDoc,
    sym_id: u64,
    new_name: impl Into<String>,
) -> bool {
    apply(
        doc,
        Action::RenameSymbol {
            sym_id,
            new_name: new_name.into(),
        },
    )
    .as_bool()
}

#[cfg(test)]
pub(crate) fn delete_symbol_from_library(doc: &mut EditorDoc, sym_id: u64) -> bool {
    apply(doc, Action::DeleteSymbol { sym_id }).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_toggle_tween(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::ToggleTween).as_bool()
}

#[cfg(test)]
pub(crate) fn set_active_keyframe_tween_rotate(
    doc: &mut EditorDoc,
    rotate: TweenRotate,
) -> bool {
    apply(doc, Action::SetTweenRotate { rotate }).as_bool()
}

#[cfg(test)]
pub(crate) fn set_selected_symbol_playback(
    doc: &mut EditorDoc,
    loop_mode: SymbolLoopMode,
    first_frame: usize,
) -> bool {
    apply(
        doc,
        Action::SetSymbolPlayback {
            loop_mode: Some(loop_mode),
            first_frame: Some(first_frame),
        },
    )
    .as_bool()
}

#[cfg(test)]
pub(crate) fn switch_to_layer_and_frame(doc: &mut EditorDoc, layer_idx: usize, frame: usize) {
    apply(doc, Action::SwitchLayerAndFrame { layer_idx, frame });
}

#[cfg(test)]
pub(crate) fn scrub_ruler_to_frame(doc: &mut EditorDoc, frame: usize) {
    apply(doc, Action::ScrubRulerToFrame { frame });
}

#[cfg(test)]
pub(crate) fn select_timeline_cells(
    doc: &mut EditorDoc,
    l0: usize,
    f0: usize,
    l1: usize,
    f1: usize,
) {
    apply(doc, Action::SelectTimelineCells { l0, f0, l1, f1 });
}

#[cfg(test)]
pub(crate) fn timeline_insert_keyframe(doc: &mut EditorDoc, copy_prev: bool) -> bool {
    apply(doc, Action::InsertKeyframe { copy_prev }).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_delete_keyframe(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::DeleteKeyframe).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_extend_frame(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::ExtendFrame).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_remove_frame(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::RemoveFrame).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_move_or_copy_frames(
    doc: &mut EditorDoc,
    sel: TimelineCellSelection,
    layer_delta: isize,
    frame_delta: isize,
    copy_mode: bool,
) -> bool {
    apply(
        doc,
        Action::MoveOrCopyFrames {
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        },
    )
    .as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_add_layer(doc: &mut EditorDoc) {
    apply(doc, Action::AddLayer);
}

#[cfg(test)]
pub(crate) fn timeline_delete_active_layer(doc: &mut EditorDoc) -> bool {
    apply(doc, Action::DeleteActiveLayer).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_move_active_layer(doc: &mut EditorDoc, up: bool) -> bool {
    apply(doc, Action::MoveActiveLayer { up }).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_reorder_layer(doc: &mut EditorDoc, src_idx: usize, dst_idx: usize) -> bool {
    apply(doc, Action::ReorderLayer { src_idx, dst_idx }).as_bool()
}

#[cfg(test)]
pub(crate) fn set_layer_visible(doc: &mut EditorDoc, layer_idx: usize, visible: bool) {
    apply(doc, Action::SetLayerVisible { layer_idx, visible });
}

#[cfg(test)]
pub(crate) fn set_layer_locked(doc: &mut EditorDoc, layer_idx: usize, locked: bool) {
    apply(doc, Action::SetLayerLocked { layer_idx, locked });
}
