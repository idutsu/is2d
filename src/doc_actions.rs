use eframe::egui::{Color32, Vec2};
use kurbo::{Affine, ParamCurve, Point, QuadBez, Vec2 as KVec2};

use crate::canvas_tool::{
    Clipboard, CoalesceKey, NoticeKind, SelectSubMode, StatusToast, Tab, Tool, ToolSettings,
    UndoEntry, active_scope_layer_id, cancel_active_drag, clear_selection,
    flush_canvas_before_timeline_op, focus_active_layer_to_shape, insert_and_select_shapes_in_scope,
    load_shapes_from_timeline, prepare_moved_selection, select_shapes_for_timeline_selection,
    set_tool, show_notice, sync_shapes_to_timeline, toggle_select_sub_mode,
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

/// `apply` の対象となるホスト（`Is2dApp`）を抽象化するトレイト
pub(crate) trait ActionHost {
    fn tab(&self) -> &Tab;
    fn tab_mut(&mut self) -> &mut Tab;
    fn tool(&self) -> &ToolSettings;
    #[allow(dead_code)]
    fn tool_mut(&mut self) -> &mut ToolSettings;
    #[allow(dead_code)]
    fn clipboard(&self) -> &Clipboard;
    #[allow(dead_code)]
    fn clipboard_mut(&mut self) -> &mut Clipboard;
    #[allow(dead_code)]
    fn toast(&self) -> &Option<StatusToast>;
    fn toast_mut(&mut self) -> &mut Option<StatusToast>;
    fn parts_mut(
        &mut self,
    ) -> (
        &mut Tab,
        &mut ToolSettings,
        &mut Clipboard,
        &mut Option<StatusToast>,
    );
    fn stop_playback(&mut self);

    fn notify_error(&mut self, message: impl Into<String>) {
        show_notice(self.toast_mut(), NoticeKind::Error, message);
    }

    fn notify_info(&mut self, message: impl Into<String>) {
        show_notice(self.toast_mut(), NoticeKind::Info, message);
    }
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
    pub(crate) fn coalesce_key(&self, tab: &Tab) -> Option<CoalesceKey> {
        match self {
            Self::SetStageSize { .. } => Some(CoalesceKey::StageSize),
            Self::SetTotalFrames { .. } => Some(CoalesceKey::TotalFrames),
            Self::SetFps { .. } => Some(CoalesceKey::Fps),
            Self::RenameSymbol { sym_id, .. } => Some(CoalesceKey::RenameSymbol(*sym_id)),
            Self::SetSymbolPlayback {
                loop_mode,
                first_frame: Some(_),
            } => selected_single_symbol_info(tab).and_then(|(b, _, _)| {
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
    tab: &mut Tab,
    pre_canvas: Option<Vec<FillShape>>,
    coalesce: Option<CoalesceKey>,
) {
    if let Some(key) = coalesce {
        if tab.history.last_coalesce_key == Some(key) {
            return;
        }
    }
    let tl_snap = if let Some(fallback) = pre_canvas {
        let mut snapshot = tab.session.take_unsliced_snapshot().unwrap_or(fallback);
        let mut tl = tab.doc.root.clone();
        tl.normalize_shape_layer_ids(&mut snapshot);
        tl.write_shapes_to_editable_cels(
            &snapshot,
            &tab.doc.library,
            tab.session.loaded_signatures(),
        );
        tl
    } else {
        tab.session.restore_unsliced_preserving_selection();
        sync_shapes_to_timeline(tab);
        tab.doc.root.clone()
    };
    let (library, symbol_edit_stack, symbols_revision) = tab
        .history
        .snapshot_symbols(&tab.doc, &tab.session.edit_stack);
    let entry = UndoEntry {
        timeline: tl_snap,
        library,
        editing_group_path: tab.session.editing_group_path.clone(),
        symbol_edit_stack,
        stage_size: tab.doc.stage_size,
        symbols_revision,
        commit_id: tab.history.current_commit_id,
    };
    tab.history.push(entry);
    tab.history.last_coalesce_key = coalesce;
}

fn push_undo(tab: &mut Tab, coalesce: Option<CoalesceKey>) {
    record_undo(tab, None, coalesce);
}

fn commit_canvas_mutation_with_coalesce(
    tab: &mut Tab,
    pre_mutation_canvas_snapshot: Vec<FillShape>,
    coalesce: Option<CoalesceKey>,
) {
    record_undo(tab, Some(pre_mutation_canvas_snapshot), coalesce);
    sync_shapes_to_timeline(tab);
}

fn commit_canvas_mutation(tab: &mut Tab, pre_mutation_canvas_snapshot: Vec<FillShape>) {
    commit_canvas_mutation_with_coalesce(tab, pre_mutation_canvas_snapshot, None);
}

fn mutate_timeline_with_undo(tab: &mut Tab, mutate: impl FnOnce(&mut Timeline)) {
    flush_canvas_before_timeline_op(tab);
    push_undo(tab, None);
    mutate(&mut tab.doc.root);
    tab.doc.sync_active_edit(&mut tab.session.edit_stack);
    load_shapes_from_timeline(tab);
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
    let (tab, tool, clipboard, toast) = host.parts_mut();
    let coalesce = action.coalesce_key(tab);

    match action {
        Action::PaintStroke {
            points,
            radius,
            color,
        } => {
            if points.is_empty() {
                return ApplyResult::Applied(false);
            }
            let z = (tab.view.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
            let raw = generate_brush_outline_at_zoom(&points, radius as f64, z);
            let changed = apply_raw_outline(tab, Tool::Brush, color, &raw);
            ApplyResult::Applied(changed)
        }
        Action::EraseStroke { points, radius } => {
            if points.is_empty() {
                return ApplyResult::Applied(false);
            }
            let z = (tab.view.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
            let raw = generate_brush_outline_at_zoom(&points, radius as f64, z);
            let changed = apply_raw_outline(tab, Tool::Eraser, tool.brush_color, &raw);
            ApplyResult::Applied(changed)
        }
        Action::CommitRawOutline { raw } => {
            ApplyResult::Applied(apply_raw_outline(tab, tool.tool, tool.brush_color, &raw))
        }
        Action::BucketFill { pt, color } => {
            ApplyResult::Applied(apply_bucket_fill(tab, color, pt))
        }

        Action::SelectSingleAt { pt } => {
            ApplyResult::Applied(select_single_shape_at(tab, tool.select_sub_mode, pt))
        }
        Action::ToggleSelectionAt { shape_idx } => {
            toggle_shape_selection(tab, shape_idx);
            ApplyResult::Applied(true)
        }
        Action::SelectMarquee { p0, p1, additive } => {
            apply_marquee_selection(tab, p0, p1, additive);
            ApplyResult::Applied(!tab.session.selection.is_empty())
        }
        Action::SelectAll => {
            clear_selection(tab);
            let editable_lids: std::collections::HashSet<u64> = tab
                .doc
                .root
                .layers
                .iter()
                .filter(|l| l.visible && !l.locked)
                .map(|l| l.id)
                .collect();
            tab.session.select_shapes_in_layers(&editable_lids);
            ApplyResult::Applied(!tab.session.selection.is_empty())
        }
        Action::Deselect => {
            clear_selection(tab);
            ApplyResult::Applied(true)
        }
        Action::MoveSelected { delta } => {
            ApplyResult::Applied(apply_move_selected(tab, delta, None))
        }
        Action::NudgeSelection { delta } => {
            ApplyResult::Applied(apply_move_selected(tab, delta, coalesce))
        }
        Action::FinishMoveSelectionDrag {
            total_delta,
            undo_snapshot,
            orig_selected_ids,
        } => {
            let min_move = 1.0 / (tab.view.zoom as f64);
            let rolled_back =
                total_delta.hypot() <= min_move || tab.session.selection.is_empty();
            let applied = apply_finish_move_selection_drag(tab, total_delta, undo_snapshot);
            if rolled_back && !orig_selected_ids.is_empty() {
                tab.session
                    .restore_selection_by_shape_ids(&orig_selected_ids);
            }
            ApplyResult::Applied(applied)
        }
        Action::CommitMovedSelection { undo_snapshot } => {
            ApplyResult::Applied(apply_commit_moved_selection(tab, undo_snapshot, None))
        }
        Action::DeleteSelection => {
            if let Some(snapshot) = prepare_delete_selected_shapes(tab) {
                commit_canvas_mutation(tab, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::ClearActiveFrame => {
            if let Some(snapshot) = prepare_clear_active_frame_shapes(tab) {
                commit_canvas_mutation(tab, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::Group => {
            if let Some((gid, snapshot)) = prepare_group_selected_shapes(tab) {
                commit_canvas_mutation(tab, snapshot);
                ApplyResult::CreatedId(Some(gid))
            } else {
                ApplyResult::CreatedId(None)
            }
        }
        Action::Ungroup => {
            if let Some(snapshot) = prepare_ungroup_selected_shapes(tab) {
                commit_canvas_mutation(tab, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::EnterGroupAt { pt } => ApplyResult::Applied(enter_group_at(tab, pt)),
        Action::ExitGroup => ApplyResult::Applied(exit_current_group(tab)),
        Action::ExitGroupSteps { steps } => {
            let mut exited = false;
            for _ in 0..steps {
                if !exit_current_group(tab) {
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
            let changed = apply_deform_shape(tab, shape_idx, drag_dist, undo_snapshot, |deformed| {
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
            let changed = apply_deform_shape(tab, shape_idx, drag_dist, undo_snapshot, |deformed| {
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
            ApplyResult::Applied(apply_pull_edge_from_to(tab, from, to))
        }
        Action::MoveVertexFromTo { from, to } => {
            ApplyResult::Applied(apply_move_vertex_from_to(tab, from, to))
        }
        Action::AddAnchor { pt } => ApplyResult::Applied(apply_anchor_edit_at(tab, pt, true)),
        Action::DeleteAnchor { pt } => ApplyResult::Applied(apply_anchor_edit_at(tab, pt, false)),

        Action::TransformSelection { affine } => {
            ApplyResult::Applied(apply_affine_to_selected_inner(tab, affine))
        }
        Action::ScaleSelection { sx, sy } => {
            let Some(bbox) = tab.session.selection_bounding_box(tab.view.zoom) else {
                return ApplyResult::Applied(false);
            };
            let pivot = tab.session.active_transform_pivot(bbox);
            ApplyResult::Applied(apply_affine_to_selected_inner(
                tab,
                affine_scale_around(pivot, sx, sy),
            ))
        }
        Action::FlipSelection { flip_x, flip_y } => {
            let Some(bbox) = tab.session.selection_bounding_box(tab.view.zoom) else {
                return ApplyResult::Applied(false);
            };
            let pivot = tab.session.active_transform_pivot(bbox);
            let sx = if flip_x { -1.0 } else { 1.0 };
            let sy = if flip_y { -1.0 } else { 1.0 };
            ApplyResult::Applied(apply_affine_to_selected_inner(
                tab,
                affine_scale_around(pivot, sx, sy),
            ))
        }
        Action::RotateSelection { angle_rad } => {
            let Some(bbox) = tab.session.selection_bounding_box(tab.view.zoom) else {
                return ApplyResult::Applied(false);
            };
            let pivot = tab.session.active_transform_pivot(bbox);
            ApplyResult::Applied(apply_affine_to_selected_inner(
                tab,
                affine_rotate_around(pivot, angle_rad),
            ))
        }
        Action::SetTransformPivot { world_pivot } => {
            ApplyResult::Applied(apply_set_selected_symbol_world_pivot(tab, world_pivot, true))
        }
        Action::ResetTransformPivot => {
            let mut changed = tab.session.transform_pivot.take().is_some();
            if apply_set_selected_symbol_world_pivot(tab, None, true) {
                changed = true;
            }
            ApplyResult::Applied(changed)
        }
        Action::CommitTransformDrag {
            changed,
            undo_snapshot,
        } => {
            if changed && !tab.session.selection.is_empty() {
                ApplyResult::Applied(apply_commit_moved_selection(tab, undo_snapshot, None))
            } else {
                tab.session.restore_drag_snapshot(undo_snapshot);
                ApplyResult::Applied(false)
            }
        }
        Action::CommitPivotDrag {
            changed,
            undo_snapshot,
        } => {
            if changed {
                if selected_single_symbol_info(tab).is_some() {
                    commit_canvas_mutation(tab, undo_snapshot);
                }
                ApplyResult::Applied(true)
            } else {
                tab.session.restore_drag_snapshot(undo_snapshot);
                ApplyResult::Applied(false)
            }
        }

        Action::Copy => ApplyResult::Applied(prepare_copy_selected_shapes(tab, clipboard)),
        Action::Cut => {
            if !prepare_copy_selected_shapes(tab, clipboard) {
                return ApplyResult::Applied(false);
            }
            if let Some(snapshot) = prepare_delete_selected_shapes(tab) {
                commit_canvas_mutation(tab, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::Paste { in_place } => {
            ApplyResult::Applied(apply_paste_clipboard(tab, tool, clipboard, toast, in_place))
        }

        Action::ConvertToSymbol => {
            if let Some((sym_id, target_lid, local_shapes, snapshot)) =
                prepare_convert_selection_to_symbol(tab)
            {
                commit_canvas_mutation(tab, snapshot);
                let registered_id = tab.doc.register_new_symbol(local_shapes);
                tab.view.selected_library_symbol = Some(registered_id);
                if let Some(l_idx) = tab.doc.root.layer_index_by_id(target_lid) {
                    tab.doc.root.active_layer = l_idx;
                }
                tab.session.selection_modified = false;
                tab.session.transform_pivot = None;
                tab.view.inspect_timeline_frame = false;
                ApplyResult::CreatedId(Some(sym_id))
            } else {
                ApplyResult::CreatedId(None)
            }
        }
        Action::PlaceSymbol { sym_id } => {
            if let Some(snapshot) = prepare_place_symbol_instance(tab, sym_id) {
                if !matches!(tool.tool, Tool::Select | Tool::Transform) {
                    set_tool(tab, tool, Tool::Select);
                }
                commit_canvas_mutation(tab, snapshot);
                ApplyResult::Applied(true)
            } else {
                ApplyResult::Applied(false)
            }
        }
        Action::EnterSymbolTimeline { binding } => {
            ApplyResult::Applied(enter_symbol_timeline(tab, binding))
        }
        Action::EnterSymbolFromLibrary { sym_id } => {
            ApplyResult::Applied(enter_symbol_from_library(tab, sym_id))
        }
        Action::ExitSymbolTimeline => ApplyResult::Applied(exit_symbol_timeline(tab)),
        Action::ExitSymbolToDepth { depth } => {
            let mut exited = false;
            while tab.session.edit_stack.len() > depth {
                if !exit_symbol_timeline(tab) {
                    break;
                }
                exited = true;
            }
            ApplyResult::Applied(exited)
        }
        Action::RenameSymbol { sym_id, new_name } => {
            ApplyResult::Applied(apply_rename_symbol(tab, sym_id, new_name, coalesce))
        }
        Action::DeleteSymbol { sym_id } => {
            ApplyResult::Applied(apply_delete_symbol(tab, sym_id))
        }
        Action::SetSymbolPlayback {
            loop_mode,
            first_frame,
        } => ApplyResult::Applied(apply_set_symbol_playback(
            tab,
            loop_mode,
            first_frame,
            coalesce,
        )),
        Action::ToggleTween => ApplyResult::Applied(apply_timeline_toggle_tween(tab, toast)),
        Action::SetTweenRotate { rotate } => {
            ApplyResult::Applied(apply_set_tween_rotate(tab, rotate))
        }

        Action::SwitchLayerAndFrame { layer_idx, frame } => {
            apply_switch_to_layer_and_frame(tab, layer_idx, frame);
            ApplyResult::Applied(true)
        }
        Action::ScrubRulerToFrame { frame } => {
            apply_scrub_ruler_to_frame(tab, frame);
            ApplyResult::Applied(true)
        }
        Action::SelectTimelineCells { l0, f0, l1, f1 } => {
            apply_select_timeline_cells(tab, l0, f0, l1, f1);
            ApplyResult::Applied(true)
        }
        Action::InsertKeyframe { copy_prev } => {
            ApplyResult::Applied(apply_timeline_insert_keyframe(tab, copy_prev))
        }
        Action::DeleteKeyframe => ApplyResult::Applied(apply_timeline_delete_keyframe(tab)),
        Action::ExtendFrame => ApplyResult::Applied(apply_timeline_extend_frame(tab)),
        Action::RemoveFrame => ApplyResult::Applied(apply_timeline_remove_frame(tab)),
        Action::MoveOrCopyFrames {
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        } => ApplyResult::Applied(apply_timeline_move_or_copy_frames(
            tab,
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        )),
        Action::AddLayer => {
            apply_timeline_add_layer(tab);
            ApplyResult::Applied(true)
        }
        Action::DeleteActiveLayer => {
            ApplyResult::Applied(apply_timeline_delete_active_layer(tab))
        }
        Action::MoveActiveLayer { up } => {
            ApplyResult::Applied(apply_timeline_move_active_layer(tab, up))
        }
        Action::ReorderLayer { src_idx, dst_idx } => {
            ApplyResult::Applied(apply_timeline_reorder_layer(tab, src_idx, dst_idx))
        }
        Action::SetLayerVisible { layer_idx, visible } => {
            apply_set_layer_visible(tab, layer_idx, visible);
            ApplyResult::Applied(true)
        }
        Action::SetLayerLocked { layer_idx, locked } => {
            apply_set_layer_locked(tab, layer_idx, locked);
            ApplyResult::Applied(true)
        }
        Action::SetTotalFrames { total } => {
            ApplyResult::Applied(apply_set_total_frames(tab, total, coalesce))
        }
        Action::SetFps { fps } => ApplyResult::Applied(apply_set_fps(tab, fps, coalesce)),
        Action::SetStageSize { size } => {
            ApplyResult::Applied(apply_set_stage_size(tab, size, coalesce))
        }

        Action::SetTool { tool: target_tool } => {
            set_tool(tab, tool, target_tool);
            ApplyResult::Applied(true)
        }
        Action::ToggleSelectSubMode { mode } => {
            toggle_select_sub_mode(tab, tool, mode);
            ApplyResult::Applied(true)
        }
        Action::Undo => {
            tab.undo();
            ApplyResult::Applied(true)
        }
        Action::Redo => {
            tab.redo();
            ApplyResult::Applied(true)
        }
        Action::EndCoalesce => {
            tab.history.end_coalesce();
            ApplyResult::Applied(true)
        }
    }
}

// ============================================================================
// 4. 各アクションの内部実装
// ============================================================================

fn apply_raw_outline(tab: &mut Tab, active_tool: Tool, brush_color: Color32, raw: &[Point]) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, &tab.doc.root);
    if !tab.doc.root.is_layer_editable_by_id(target_lid) {
        return false;
    }
    let stroke_shapes: Vec<FillShape> =
        build_smoothed_stroke_shapes_at_zoom(raw, brush_color, tab.view.zoom as f64)
            .into_iter()
            .map(|s| s.with_scope(target_lid, &tab.session.editing_group_path))
            .collect();
    if stroke_shapes.is_empty() {
        return false;
    }
    if active_tool == Tool::Brush {
        clear_selection(tab);
        let snapshot = tab.session.take_or_clone_unsliced_snapshot();
        merge_shapes_into_canvas(&mut tab.session.shapes, stroke_shapes);
        tab.session.sort_shapes_by_layer_order(&tab.doc.root);
        commit_canvas_mutation(tab, snapshot);
        true
    } else {
        let has_bbox_overlap = tab.session.shapes.iter().any(|s| {
            s.layer_id == target_lid
                && s.is_raw_at_scope(&tab.session.editing_group_path)
                && stroke_shapes
                    .iter()
                    .any(|er| rects_overlap(s.bounding_box(), er.bounding_box()))
        });
        if !has_bbox_overlap {
            return false;
        }
        clear_selection(tab);
        let prev_revs: Vec<(u64, u64)> = tab
            .session
            .shapes
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        let snapshot = tab.session.take_or_clone_unsliced_snapshot();
        erase_from_canvas(
            &mut tab.session.shapes,
            &stroke_shapes,
            target_lid,
            &tab.session.editing_group_path,
        );
        let unchanged = tab.session.shapes.len() == prev_revs.len()
            && tab
                .session
                .shapes
                .iter()
                .zip(prev_revs.iter())
                .all(|(s, &(id, rev))| s.shape_id == id && s.mesh_revision == rev);
        if !unchanged {
            commit_canvas_mutation(tab, snapshot);
            true
        } else {
            false
        }
    }
}

fn collect_all_visible_shapes(tab: &Tab) -> Vec<FillShape> {
    let mut out = Vec::with_capacity(tab.session.shapes.len());
    for (layer_idx, layer) in tab.doc.root.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        if layer.locked {
            out.extend(tab.doc.root.evaluate_layer_shapes_at(
                layer_idx,
                tab.doc.root.current_frame,
                &tab.doc.library,
            ));
        } else {
            let lid = layer.id;
            out.extend(
                tab.session
                    .shapes
                    .iter()
                    .filter(|s| s.layer_id == lid)
                    .cloned(),
            );
        }
    }
    out
}

fn apply_bucket_fill(tab: &mut Tab, fill_color: Color32, pt: Point) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, &tab.doc.root);
    if !tab.doc.root.is_layer_editable_by_id(target_lid) {
        return false;
    }

    // 0. 選択領域が存在する場合：選択中の「生シェイプ」のみにバケツで色を流し込む（グループは無効）
    if !tab.session.selection.is_empty() {
        let clicked_selected_raw = tab.session.shapes.iter().any(|s| {
            tab.session.selection.contains_shape(s)
                && tab.doc.root.is_layer_editable_by_id(s.layer_id)
                && s.is_raw_at_scope(&tab.session.editing_group_path)
                && s.contains_point(pt)
        });
        if clicked_selected_raw {
            let any_color_diff = tab.session.shapes.iter().any(|s| {
                tab.session.selection.contains_shape(s)
                    && tab.doc.root.is_layer_editable_by_id(s.layer_id)
                    && s.is_raw_at_scope(&tab.session.editing_group_path)
                    && s.color != fill_color
            });
            if !any_color_diff {
                return false;
            }
            let snapshot = tab.session.take_or_clone_unsliced_snapshot();
            for shape in &mut tab.session.shapes {
                if tab.session.selection.contains_shape(shape)
                    && tab.doc.root.is_layer_editable_by_id(shape.layer_id)
                    && shape.is_raw_at_scope(&tab.session.editing_group_path)
                {
                    shape.set_color(fill_color);
                }
            }
            tab.session.selection_modified = true;
            commit_canvas_mutation(tab, snapshot);
            return true;
        }
        return false;
    }

    let all_visible_shapes = collect_all_visible_shapes(tab);
    let target_layer_ord = tab.doc.root.layer_index_by_id(target_lid).unwrap_or(0);

    if let Some(hit_idx) = (0..tab.session.shapes.len()).rev().find(|&idx| {
        let s = &tab.session.shapes[idx];
        s.layer_id == target_lid
            && s.is_in_editing_scope(&tab.session.editing_group_path)
            && s.contains_point(pt)
    }) {
        let occluded_above = (target_layer_ord + 1..tab.doc.root.layers.len()).any(|l_idx| {
            let layer = &tab.doc.root.layers[l_idx];
            layer.visible
                && all_visible_shapes
                    .iter()
                    .any(|s| s.layer_id == layer.id && s.contains_point(pt))
        });

        let hit_shape = &tab.session.shapes[hit_idx];
        if !occluded_above && hit_shape.is_raw_at_scope(&tab.session.editing_group_path) {
            if hit_shape.color == fill_color {
                return false;
            }
            let hit_bbox = hit_shape.bounding_box();
            let overlapping_walls: Vec<FillShape> = all_visible_shapes
                .iter()
                .filter(|s| {
                    (s.layer_id != target_lid
                        || !s.is_raw_at_scope(&tab.session.editing_group_path))
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
                        let snapshot = tab.session.take_or_clone_unsliced_snapshot();
                        merge_shapes_into_canvas(
                            &mut tab.session.shapes,
                            vec![split_piece
                                .with_scope(target_lid, &tab.session.editing_group_path)],
                        );
                        tab.session.sort_shapes_by_layer_order(&tab.doc.root);
                        commit_canvas_mutation(tab, snapshot);
                        return true;
                    }
                }
            }

            focus_active_layer_to_shape(tab, hit_idx);
            let snapshot = tab.session.take_or_clone_unsliced_snapshot();
            let mut target = tab.session.shapes.remove(hit_idx);
            target.set_color(fill_color);
            merge_disjoint_fill_into_canvas(&mut tab.session.shapes, target);
            tab.session.sort_shapes_by_layer_order(&tab.doc.root);
            commit_canvas_mutation(tab, snapshot);
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
        let active_layer_raw: Vec<FillShape> = tab
            .session
            .shapes
            .iter()
            .filter(|s| {
                s.layer_id == target_lid
                    && s.is_raw_at_scope(&tab.session.editing_group_path)
                    && !s.contains_point(pt)
            })
            .cloned()
            .collect();
        (active_layer_raw.len() < wall_shapes.len())
            .then(|| find_enclosed_region_at(&active_layer_raw, pt, fill_color))
            .flatten()
    });
    if let Some(enclosed_fill) = enclosed_opt {
        let snapshot = tab.session.take_or_clone_unsliced_snapshot();
        merge_disjoint_fill_into_canvas(
            &mut tab.session.shapes,
            enclosed_fill.with_scope(target_lid, &tab.session.editing_group_path),
        );
        tab.session.sort_shapes_by_layer_order(&tab.doc.root);
        commit_canvas_mutation(tab, snapshot);
        return true;
    }

    false
}

fn apply_commit_moved_selection(
    tab: &mut Tab,
    undo_snapshot: Vec<FillShape>,
    coalesce: Option<CoalesceKey>,
) -> bool {
    if !prepare_moved_selection(tab, &undo_snapshot) {
        return false;
    }
    commit_canvas_mutation_with_coalesce(tab, undo_snapshot, coalesce);
    true
}

fn apply_finish_move_selection_drag(
    tab: &mut Tab,
    total_delta: KVec2,
    undo_snapshot: Vec<FillShape>,
) -> bool {
    let min_move = 1.0 / (tab.view.zoom as f64);
    if total_delta.hypot() > min_move && !tab.session.selection.is_empty() {
        apply_commit_moved_selection(tab, undo_snapshot, None)
    } else {
        tab.session.restore_drag_snapshot(undo_snapshot);
        false
    }
}

fn apply_move_selected(
    tab: &mut Tab,
    delta: KVec2,
    coalesce: Option<CoalesceKey>,
) -> bool {
    cancel_active_drag(tab);
    if delta == KVec2::ZERO || tab.session.selection.is_empty() {
        return false;
    }
    let undo_snapshot = tab.session.take_or_clone_unsliced_snapshot();
    tab.session.translate_selected(delta);
    apply_commit_moved_selection(tab, undo_snapshot, coalesce)
}

fn apply_deform_shape(
    tab: &mut Tab,
    shape_idx: usize,
    drag_dist: f64,
    undo_snapshot: Vec<FillShape>,
    apply_deform: impl FnOnce(&mut FillShape),
) -> bool {
    let min_deform = 0.5 / (tab.view.zoom as f64);
    if drag_dist > min_deform && shape_idx < tab.session.shapes.len() {
        focus_active_layer_to_shape(tab, shape_idx);
        let mut deformed = tab.session.shapes.remove(shape_idx);
        apply_deform(&mut deformed);
        let resolved = resolve_deformed_shape(&deformed);
        merge_shapes_into_canvas(&mut tab.session.shapes, resolved);
        tab.session.sort_shapes_by_layer_order(&tab.doc.root);
        commit_canvas_mutation(tab, undo_snapshot);
        true
    } else {
        tab.session.restore_drag_snapshot(undo_snapshot);
        false
    }
}

fn apply_pull_edge_from_to(tab: &mut Tab, from: Point, to: Point) -> bool {
    clear_selection(tab);
    let target = compute_hover_target(
        &tab.session,
        from,
        tab.view.zoom,
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
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();
    let Some(orig_seg) = tab
        .session
        .get_contour_segment(shape_idx, contour_idx, seg_idx)
    else {
        return false;
    };
    let grab_t = t.clamp(0.18, 0.82);
    let grab_pos = orig_seg.eval(grab_t);
    let drag_dist = (to - grab_pos).hypot();
    apply_deform_shape(tab, shape_idx, drag_dist, snapshot, |deformed| {
        apply_pull_edge_to_shape(deformed, (contour_idx, seg_idx), grab_t, orig_seg, to);
    })
}

fn apply_move_vertex_from_to(tab: &mut Tab, from: Point, to: Point) -> bool {
    clear_selection(tab);
    let target = compute_hover_target(
        &tab.session,
        from,
        tab.view.zoom,
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
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();
    let Some((orig_prev_ctrl, orig_curr_ctrl)) = tab
        .session
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
    apply_deform_shape(tab, shape_idx, drag_dist, snapshot, |deformed| {
        apply_move_vertex_to_shape(
            deformed,
            (contour_idx, seg_idx),
            pos,
            (orig_prev_ctrl, orig_curr_ctrl),
            to,
        );
    })
}

fn apply_anchor_edit_at(tab: &mut Tab, pt: Point, is_add: bool) -> bool {
    let zoom = tab.view.zoom;
    let sub_mode = if is_add {
        SelectSubMode::AddAnchor
    } else {
        SelectSubMode::DeleteAnchor
    };
    let target = compute_hover_target(&tab.session, pt, zoom, sub_mode);
    let had_unsliced = tab.session.has_unsliced_snapshot();
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();
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
        ) => tab
            .session
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
        ) => tab
            .session
            .shapes
            .get_mut(shape_idx)
            .and_then(|s| s.get_contour_mut(contour_idx))
            .is_some_and(|c| c.delete_anchor_at(seg_idx))
            .then_some(shape_idx),
        _ => None,
    };
    if let Some(s_idx) = modified_shape {
        if let Some(shape) = tab.session.shapes.get_mut(s_idx) {
            shape.rebuild_cache();
        }
        commit_canvas_mutation(tab, snapshot);
        true
    } else {
        if had_unsliced {
            tab.session.restore_unsliced_snapshot(snapshot);
        }
        false
    }
}

pub(crate) fn update_selected_symbol_world_pivot_live(
    tab: &mut Tab,
    world_pivot: Option<Point>,
) -> bool {
    apply_set_selected_symbol_world_pivot(tab, world_pivot, false)
}

fn apply_set_selected_symbol_world_pivot(
    tab: &mut Tab,
    world_pivot: Option<Point>,
    commit_to_timeline: bool,
) -> bool {
    let Some(binding) = tab.session.selected_single_symbol_binding() else {
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
    if !tab.session.selection_modified {
        tab.session.restore_unsliced_preserving_selection();
    }
    let undo_snapshot =
        commit_to_timeline.then(|| tab.session.take_or_clone_unsliced_snapshot());
    tab.session
        .update_symbol_instance_binding(binding.instance_gid, |b| b.pivot = new_local_pivot);
    if let Some(snapshot) = undo_snapshot {
        commit_canvas_mutation(tab, snapshot);
    }
    true
}

pub(crate) fn flush_transform_pivot_to_symbol(tab: &mut Tab) {
    if let Some(piv) = tab.session.transform_pivot {
        if update_selected_symbol_world_pivot_live(tab, Some(piv)) {
            tab.session.transform_pivot = None;
        }
    }
}

fn apply_affine_to_selected_inner(tab: &mut Tab, affine: Affine) -> bool {
    cancel_active_drag(tab);
    if tab.session.selection.is_empty() {
        return false;
    }
    let undo_snapshot = tab.session.take_or_clone_unsliced_snapshot();
    flush_transform_pivot_to_symbol(tab);
    let any_transformed = tab.session.transform_selected(affine);
    if any_transformed {
        apply_commit_moved_selection(tab, undo_snapshot, None);
    }
    any_transformed
}

fn apply_paste_clipboard(
    tab: &mut Tab,
    tool: &mut ToolSettings,
    clipboard: &mut Clipboard,
    toast: &mut Option<StatusToast>,
    in_place: bool,
) -> bool {
    if clipboard.shapes.is_empty() {
        return false;
    }
    let target_lid = active_scope_layer_id(&tab.session, &tab.doc.root);
    if !tab.doc.root.is_layer_editable_by_id(target_lid) {
        show_notice(
            toast,
            NoticeKind::Warning,
            "現在のレイヤーまたはフレームは編集できないためペーストできません",
        );
        return false;
    }
    if would_clipboard_paste_all_cycle(tab, clipboard) {
        show_notice(
            toast,
            NoticeKind::Warning,
            "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
        );
        return false;
    }

    clear_selection(tab);
    push_undo(tab, None);
    let (resolved, had_cycle) = resolve_clipboard_for_paste(tab, clipboard);
    if had_cycle {
        show_notice(
            toast,
            NoticeKind::Warning,
            "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
        );
    }
    if !resolved.is_empty() {
        let offset = if in_place {
            KVec2::ZERO
        } else {
            if clipboard.paste_count == 0 {
                clipboard.paste_offset = KVec2::ZERO;
            }
            clipboard.paste_count += 1;
            let step = 16.0 / (tab.view.zoom as f64);
            clipboard.paste_offset += KVec2::new(step, step);
            clipboard.paste_offset
        };

        let same_timeline_context = clipboard.source_doc_id == tab.id
            && clipboard.source_symbol_id
                == tab.session.edit_stack.last().map(|s| s.symbol_id)
            && tab.session.editing_group_path.is_empty();
        let multi_layer_copy = same_timeline_context
            && resolved
                .first()
                .is_some_and(|first| resolved.iter().any(|s| s.layer_id != first.layer_id));
        let (pasted_shapes, any_raw) =
            instantiate_relative_shapes_in_scope(tab, &resolved, offset, multi_layer_copy);

        insert_and_select_shapes_in_scope(tab, pasted_shapes, any_raw);
        if !matches!(tool.tool, Tool::Select | Tool::Transform) {
            set_tool(tab, tool, Tool::Select);
        }
        sync_shapes_to_timeline(tab);
    }
    true
}

fn apply_rename_symbol(
    tab: &mut Tab,
    sym_id: u64,
    new_name: String,
    coalesce: Option<CoalesceKey>,
) -> bool {
    if tab
        .doc
        .find_symbol(sym_id)
        .is_none_or(|s| s.name == new_name)
    {
        return false;
    }
    push_undo(tab, coalesce);
    tab.doc.rename_symbol(sym_id, new_name)
}

fn apply_delete_symbol(tab: &mut Tab, sym_id: u64) -> bool {
    if tab.doc.find_symbol(sym_id).is_none() {
        return false;
    }
    while tab
        .session
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == sym_id)
    {
        exit_symbol_timeline(tab);
    }
    clear_selection(tab);
    push_undo(tab, None);

    tab.doc
        .remove_symbol_and_detach(sym_id, &mut tab.session.edit_stack, &mut tab.session.shapes);
    if tab.view.selected_library_symbol == Some(sym_id) {
        tab.view.selected_library_symbol = tab.doc.library.first().map(|s| s.id);
    }

    tab.doc.sync_active_edit(&mut tab.session.edit_stack);
    load_shapes_from_timeline(tab);
    true
}

fn apply_timeline_toggle_tween(tab: &mut Tab, toast: &mut Option<StatusToast>) -> bool {
    let targets = tab.doc.root.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    flush_canvas_before_timeline_op(tab);

    let (can_toggle, is_currently_tweened) = tab.doc.root.can_toggle_tween_at_current_frame();
    if !can_toggle {
        show_notice(
            toast,
            NoticeKind::Warning,
            "トゥイーンを設定するには、キーフレームに単一のシンボルインスタンス（F8）のみを配置してください",
        );
        return false;
    }

    push_undo(tab, None);
    let curr_f = tab.doc.root.current_frame;
    let next_tween = !is_currently_tweened;
    let mut changed = false;

    for &l_idx in &targets {
        let layer = &mut tab.doc.root.layers[l_idx];
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
        load_shapes_from_timeline(tab);
        select_shapes_for_timeline_selection(tab);
        tab.view.inspect_timeline_frame = true;
    }
    changed
}

fn apply_set_tween_rotate(tab: &mut Tab, rotate_mode: TweenRotate) -> bool {
    let Some(layer) = tab.doc.root.layers.get(tab.doc.root.active_layer) else {
        return false;
    };
    if layer.locked || !layer.visible {
        return false;
    }
    let Some(kf_idx) = layer.keyframe_index_at(tab.doc.root.current_frame) else {
        return false;
    };
    if !layer.keyframes[kf_idx].tween || layer.keyframes[kf_idx].tween_rotate == rotate_mode {
        return false;
    }

    flush_canvas_before_timeline_op(tab);
    push_undo(tab, None);
    if let Some(layer_mut) = tab.doc.root.layers.get_mut(tab.doc.root.active_layer) {
        if let Some(kf) = layer_mut.keyframes.get_mut(kf_idx) {
            kf.tween_rotate = rotate_mode;
        }
    }
    tab.doc.sync_active_edit(&mut tab.session.edit_stack);
    load_shapes_from_timeline(tab);
    select_shapes_for_timeline_selection(tab);
    tab.view.inspect_timeline_frame = true;
    true
}

fn apply_set_symbol_playback(
    tab: &mut Tab,
    loop_mode: Option<SymbolLoopMode>,
    first_frame: Option<usize>,
    coalesce: Option<CoalesceKey>,
) -> bool {
    let Some((binding, _, total_frames)) = selected_single_symbol_info(tab) else {
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
    let prev_coalesce = tab.history.last_coalesce_key;
    flush_canvas_before_timeline_op(tab);
    tab.history.last_coalesce_key = prev_coalesce;
    push_undo(tab, coalesce);

    if tab.doc.find_symbol(sym_id).is_none() {
        return false;
    }

    let curr_f = tab.doc.root.current_frame;
    let mut updated = false;
    for layer in &mut tab.doc.root.layers {
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
        tab.doc.sync_active_edit(&mut tab.session.edit_stack);
        load_shapes_from_timeline(tab);
    }
    updated
}

fn apply_switch_to_layer_and_frame(tab: &mut Tab, layer_idx: usize, frame_idx: usize) {
    if tab.doc.root.layers.is_empty() {
        return;
    }
    flush_canvas_before_timeline_op(tab);
    tab.doc.root.select_single_cell(layer_idx, frame_idx);
    load_shapes_from_timeline(tab);
    tab.view.inspect_timeline_frame = true;
}

fn apply_scrub_ruler_to_frame(tab: &mut Tab, frame_idx: usize) {
    let clamped_frame = frame_idx.min(tab.doc.root.total_frames.saturating_sub(1));
    if tab.doc.root.current_frame == clamped_frame && tab.doc.root.cell_selection.is_none() {
        return;
    }
    flush_canvas_before_timeline_op(tab);
    tab.doc.root.current_frame = clamped_frame;
    tab.doc.root.cell_selection = None;
    load_shapes_from_timeline(tab);
}

fn apply_select_timeline_cells(
    tab: &mut Tab,
    start_layer: usize,
    start_frame: usize,
    end_layer: usize,
    end_frame: usize,
) {
    if tab.doc.root.layers.is_empty() {
        return;
    }
    let max_l = tab.doc.root.layers.len() - 1;
    let (l0, l1) = (start_layer.min(max_l), end_layer.min(max_l));
    let (f0, f1) = (start_frame, end_frame);

    apply_switch_to_layer_and_frame(tab, l1, f1);
    tab.doc.root.cell_selection = Some(TimelineCellSelection::from_corners(l0, f0, l1, f1));
    select_shapes_for_timeline_selection(tab);
}

fn apply_timeline_insert_keyframe(tab: &mut Tab, clone_prev: bool) -> bool {
    let targets = tab.doc.root.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.insert_keyframe_on_targets(&targets, clone_prev);
    });
    if clone_prev {
        select_shapes_for_timeline_selection(tab);
    }
    true
}

fn apply_timeline_delete_keyframe(tab: &mut Tab) -> bool {
    let targets = tab.doc.root.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    flush_canvas_before_timeline_op(tab);
    if !tab.doc.root.can_delete_keyframe_on_targets(&targets) {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.delete_keyframe_on_targets(&targets);
    });
    true
}

fn apply_timeline_extend_frame(tab: &mut Tab) -> bool {
    let targets = tab.doc.root.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.extend_frame_on_targets(&targets);
    });
    true
}

fn apply_timeline_remove_frame(tab: &mut Tab) -> bool {
    let targets = tab.doc.root.target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    flush_canvas_before_timeline_op(tab);
    if !tab.doc.root.can_remove_frame_on_targets(&targets) {
        return false;
    }
    let mut any_changed = false;
    mutate_timeline_with_undo(tab, |tl| {
        any_changed = tl.remove_frame_on_targets(&targets);
    });
    any_changed
}

fn apply_timeline_move_or_copy_frames(
    tab: &mut Tab,
    sel: TimelineCellSelection,
    delta_layer: isize,
    delta_frame: isize,
    copy_mode: bool,
) -> bool {
    let Some((dst_sel, actual_dl)) = tab
        .doc
        .root
        .resolve_frame_move_bounds(sel, delta_layer, delta_frame)
    else {
        return false;
    };
    mutate_timeline_with_undo(tab, |tl| {
        tl.move_or_copy_frames(sel, dst_sel, actual_dl, copy_mode);
    });
    tab.doc.next_group_id = tab
        .doc
        .next_group_id
        .max(tab.doc.root.max_group_id().saturating_add(1));
    select_shapes_for_timeline_selection(tab);
    true
}

fn apply_timeline_add_layer(tab: &mut Tab) {
    if tab.doc.root.layers.len() >= MAX_LAYERS_PER_TIMELINE {
        return;
    }
    mutate_timeline_with_undo(tab, Timeline::add_layer_above_active);
}

fn apply_timeline_delete_active_layer(tab: &mut Tab) -> bool {
    if tab.doc.root.layers.len() <= 1 {
        return false;
    }
    mutate_timeline_with_undo(tab, Timeline::delete_active_layer);
    true
}

fn apply_timeline_reorder_layer(tab: &mut Tab, from_idx: usize, to_idx: usize) -> bool {
    let len = tab.doc.root.layers.len();
    if from_idx >= len || to_idx >= len || from_idx == to_idx {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.reorder_layer(from_idx, to_idx);
    });
    true
}

fn apply_timeline_move_active_layer(tab: &mut Tab, up: bool) -> bool {
    let len = tab.doc.root.layers.len();
    let curr = tab.doc.root.active_layer;
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
    apply_timeline_reorder_layer(tab, curr, target)
}

fn apply_set_layer_visible(tab: &mut Tab, layer_idx: usize, visible: bool) {
    if tab
        .doc
        .root
        .layers
        .get(layer_idx)
        .is_none_or(|l| l.visible == visible)
    {
        return;
    }
    mutate_timeline_with_undo(tab, |tl| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            layer.visible = visible;
        }
    });
}

fn apply_set_layer_locked(tab: &mut Tab, layer_idx: usize, locked: bool) {
    if tab
        .doc
        .root
        .layers
        .get(layer_idx)
        .is_none_or(|l| l.locked == locked)
    {
        return;
    }
    mutate_timeline_with_undo(tab, |tl| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            layer.locked = locked;
        }
    });
}

fn apply_set_stage_size(
    tab: &mut Tab,
    next_size: Vec2,
    coalesce: Option<CoalesceKey>,
) -> bool {
    if (next_size.x - tab.doc.stage_size.x).abs() <= f32::EPSILON
        && (next_size.y - tab.doc.stage_size.y).abs() <= f32::EPSILON
    {
        return false;
    }
    push_undo(tab, coalesce);
    tab.doc.stage_size = next_size;
    true
}

fn apply_set_total_frames(
    tab: &mut Tab,
    next_total: usize,
    coalesce: Option<CoalesceKey>,
) -> bool {
    let next_total = next_total.max(1);
    if next_total != tab.doc.root.total_frames {
        if tab.history.last_coalesce_key != coalesce || coalesce.is_none() {
            flush_canvas_before_timeline_op(tab);
        }
        push_undo(tab, coalesce);
        tab.doc.root.resize_total_frames(next_total);
        tab.doc.sync_active_edit(&mut tab.session.edit_stack);
        load_shapes_from_timeline(tab);
        return true;
    }
    false
}

fn apply_set_fps(tab: &mut Tab, next_fps: u32, coalesce: Option<CoalesceKey>) -> bool {
    let next_fps = next_fps.clamp(1, 60);
    if next_fps != tab.doc.root.fps {
        if (tab.history.last_coalesce_key != coalesce || coalesce.is_none())
            && tab.session.selection.is_empty()
            && !tab.session.has_unsliced_snapshot()
        {
            flush_canvas_before_timeline_op(tab);
        }
        push_undo(tab, coalesce);
        tab.doc.root.fps = next_fps;
        for sess in &mut tab.session.edit_stack {
            sess.parent_timeline.fps = next_fps;
        }
        tab.doc.sync_active_edit(&mut tab.session.edit_stack);
        return true;
    }
    false
}

// ============================================================================
// 5. テスト向けのラッパー関数（すべて `apply` を経由）
// ============================================================================

#[cfg(test)]
pub(crate) fn commit_raw_outline(host: &mut impl ActionHost, raw: &[Point]) {
    apply(host, Action::CommitRawOutline { raw: raw.to_vec() });
}

#[cfg(test)]
pub(crate) fn bucket_fill_at(host: &mut impl ActionHost, pt: Point) -> bool {
    let color = host.tool().brush_color;
    apply(host, Action::BucketFill { pt, color }).as_bool()
}

#[cfg(test)]
pub(crate) fn commit_moved_selection(host: &mut impl ActionHost, undo_snapshot: Vec<FillShape>) {
    apply(host, Action::CommitMovedSelection { undo_snapshot });
}

#[cfg(test)]
pub(crate) fn finish_move_selection_drag(
    host: &mut impl ActionHost,
    total_delta: KVec2,
    undo_snapshot: Vec<FillShape>,
) {
    apply(
        host,
        Action::FinishMoveSelectionDrag {
            total_delta,
            undo_snapshot,
            orig_selected_ids: Vec::new(),
        },
    );
}

#[cfg(test)]
pub(crate) fn group_selected_shapes(host: &mut impl ActionHost) -> Option<u64> {
    apply(host, Action::Group).id()
}

#[cfg(test)]
pub(crate) fn ungroup_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Ungroup).as_bool()
}

#[cfg(test)]
pub(crate) fn nudge_selected_shapes(host: &mut impl ActionHost, delta: KVec2) -> bool {
    apply(host, Action::NudgeSelection { delta }).as_bool()
}

#[cfg(test)]
pub(crate) fn delete_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteSelection).as_bool()
}

#[cfg(test)]
pub(crate) fn clear_active_frame_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ClearActiveFrame).as_bool()
}

#[cfg(test)]
pub(crate) fn copy_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Copy).as_bool()
}

#[cfg(test)]
pub(crate) fn cut_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Cut).as_bool()
}

#[cfg(test)]
pub(crate) fn paste_clipboard(host: &mut impl ActionHost, in_place: bool) -> bool {
    apply(host, Action::Paste { in_place }).as_bool()
}

#[cfg(test)]
pub(crate) fn set_selected_symbol_world_pivot(
    host: &mut impl ActionHost,
    world_pivot: Option<Point>,
    commit_to_timeline: bool,
) -> bool {
    if commit_to_timeline {
        apply(host, Action::SetTransformPivot { world_pivot }).as_bool()
    } else {
        update_selected_symbol_world_pivot_live(host.tab_mut(), world_pivot)
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn apply_affine_to_selected(host: &mut impl ActionHost, affine: Affine) -> bool {
    apply(host, Action::TransformSelection { affine }).as_bool()
}

#[cfg(test)]
pub(crate) fn scale_selected_shapes(host: &mut impl ActionHost, sx: f64, sy: f64) -> bool {
    apply(host, Action::ScaleSelection { sx, sy }).as_bool()
}

#[cfg(test)]
pub(crate) fn flip_selected_shapes(host: &mut impl ActionHost, flip_x: bool, flip_y: bool) -> bool {
    apply(host, Action::FlipSelection { flip_x, flip_y }).as_bool()
}

#[cfg(test)]
pub(crate) fn rotate_selected_shapes(host: &mut impl ActionHost, angle_rad: f64) -> bool {
    apply(host, Action::RotateSelection { angle_rad }).as_bool()
}

#[cfg(test)]
pub(crate) fn convert_selection_to_symbol(host: &mut impl ActionHost) -> Option<u64> {
    apply(host, Action::ConvertToSymbol).id()
}

#[cfg(test)]
pub(crate) fn place_symbol_instance_from_library(host: &mut impl ActionHost, sym_id: u64) -> bool {
    apply(host, Action::PlaceSymbol { sym_id }).as_bool()
}

#[cfg(test)]
pub(crate) fn rename_symbol_in_library(
    host: &mut impl ActionHost,
    sym_id: u64,
    new_name: impl Into<String>,
) -> bool {
    apply(
        host,
        Action::RenameSymbol {
            sym_id,
            new_name: new_name.into(),
        },
    )
    .as_bool()
}

#[cfg(test)]
pub(crate) fn delete_symbol_from_library(host: &mut impl ActionHost, sym_id: u64) -> bool {
    apply(host, Action::DeleteSymbol { sym_id }).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_toggle_tween(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ToggleTween).as_bool()
}

#[cfg(test)]
pub(crate) fn set_active_keyframe_tween_rotate(
    host: &mut impl ActionHost,
    rotate: TweenRotate,
) -> bool {
    apply(host, Action::SetTweenRotate { rotate }).as_bool()
}

#[cfg(test)]
pub(crate) fn set_selected_symbol_playback(
    host: &mut impl ActionHost,
    loop_mode: SymbolLoopMode,
    first_frame: usize,
) -> bool {
    apply(
        host,
        Action::SetSymbolPlayback {
            loop_mode: Some(loop_mode),
            first_frame: Some(first_frame),
        },
    )
    .as_bool()
}

#[cfg(test)]
pub(crate) fn switch_to_layer_and_frame(
    host: &mut impl ActionHost,
    layer_idx: usize,
    frame: usize,
) {
    apply(host, Action::SwitchLayerAndFrame { layer_idx, frame });
}

#[cfg(test)]
pub(crate) fn scrub_ruler_to_frame(host: &mut impl ActionHost, frame: usize) {
    apply(host, Action::ScrubRulerToFrame { frame });
}

#[cfg(test)]
pub(crate) fn select_timeline_cells(
    host: &mut impl ActionHost,
    l0: usize,
    f0: usize,
    l1: usize,
    f1: usize,
) {
    apply(host, Action::SelectTimelineCells { l0, f0, l1, f1 });
}

#[cfg(test)]
pub(crate) fn timeline_insert_keyframe(host: &mut impl ActionHost, copy_prev: bool) -> bool {
    apply(host, Action::InsertKeyframe { copy_prev }).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_delete_keyframe(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteKeyframe).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_extend_frame(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ExtendFrame).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_remove_frame(host: &mut impl ActionHost) -> bool {
    apply(host, Action::RemoveFrame).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_move_or_copy_frames(
    host: &mut impl ActionHost,
    sel: TimelineCellSelection,
    layer_delta: isize,
    frame_delta: isize,
    copy_mode: bool,
) -> bool {
    apply(
        host,
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
pub(crate) fn timeline_add_layer(host: &mut impl ActionHost) {
    apply(host, Action::AddLayer);
}

#[cfg(test)]
pub(crate) fn timeline_delete_active_layer(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteActiveLayer).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_move_active_layer(host: &mut impl ActionHost, up: bool) -> bool {
    apply(host, Action::MoveActiveLayer { up }).as_bool()
}

#[cfg(test)]
pub(crate) fn timeline_reorder_layer(
    host: &mut impl ActionHost,
    src_idx: usize,
    dst_idx: usize,
) -> bool {
    apply(host, Action::ReorderLayer { src_idx, dst_idx }).as_bool()
}

#[cfg(test)]
pub(crate) fn set_layer_visible(host: &mut impl ActionHost, layer_idx: usize, visible: bool) {
    apply(host, Action::SetLayerVisible { layer_idx, visible });
}

#[cfg(test)]
pub(crate) fn set_layer_locked(host: &mut impl ActionHost, layer_idx: usize, locked: bool) {
    apply(host, Action::SetLayerLocked { layer_idx, locked });
}
