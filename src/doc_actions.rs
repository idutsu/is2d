use std::collections::HashSet;

use eframe::egui::{Color32, Vec2};
use kurbo::{Affine, Point, QuadBez, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, cancel_active_drag, clear_selection, commit_floating, edit_layers,
    editable_layer_ids, focus_active_layer_to_shape_by_id, insert_and_select_shapes_in_scope,
    merge_floating_into_base_shapes, prepare_moved_selection, select_shapes_for_timeline_selection,
    set_tool, show_notice, toggle_select_sub_mode, Clipboard, CoalesceKey, Floating, NoticeKind,
    SelectSubMode, ShapeId, StatusToast, Tab, Tool, ToolSettings, UndoEntry,
};
use crate::clipboard_ops::{
    instantiate_relative_shapes_in_scope, prepare_copy_selected_shapes,
    resolve_clipboard_for_paste, would_clipboard_paste_all_cycle,
};
use crate::select_ops::{
    apply_marquee_selection, apply_move_vertex_to_shape, apply_pull_edge_to_shape,
    compute_hover_target, enter_group_at, exit_current_group, prepare_arrange_selected,
    prepare_clear_active_frame_shapes, prepare_delete_selected_shapes,
    prepare_group_selected_shapes, prepare_ungroup_selected_shapes,
    retain_selection_and_flush_deselected, select_single_shape_at, toggle_shape_selection,
    ArrangeMode,
};
use crate::symbol::{ColorEffect, SymbolInstanceBinding, SymbolLoopMode, TweenRotate};
use crate::symbol_ops::{
    enter_symbol_from_library, enter_symbol_timeline, exit_symbol_timeline,
    prepare_convert_selection_to_symbol, prepare_place_symbol_instance,
    selected_single_symbol_info,
};
use crate::timeline::{Timeline, TimelineCellSelection, MAX_LAYERS_PER_TIMELINE};
use crate::vector_boolean::{
    boolean_operation, erase_from_canvas, find_enclosed_region_at, merge_disjoint_fill_into_canvas,
    merge_shapes_into_canvas, resolve_deformed_shape, BoolOp,
};
use crate::vector_engine::{
    affine_rotate_around, affine_scale_around, rects_overlap, FillShape, MAX_VIEW_ZOOM,
    MIN_VIEW_ZOOM,
};
use crate::vector_stroke::{build_smoothed_stroke_shapes_at_zoom, generate_brush_outline_at_zoom};

// ============================================================================
// 1. ActionHost / Action の定義
// ============================================================================

/// `apply` の対象となるホスト（`Is2dApp`）を抽象化するトレイト
pub(crate) trait ActionHost {
    fn tab(&self) -> &Tab;
    fn tab_mut(&mut self) -> &mut Tab;
    fn tool(&self) -> &ToolSettings;
    fn tool_mut(&mut self) -> &mut ToolSettings;
    #[allow(dead_code)]
    fn clipboard_mut(&mut self) -> &mut Clipboard;
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

/// ドキュメント状態または編集セッションを変更するすべての操作を表す列挙型
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
    #[cfg(test)]
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
        scene_idx: usize,
    },
    RetainSelection {
        keep_indices: Vec<usize>,
    },
    SelectMarquee {
        p0: Point,
        p1: Point,
        additive: bool,
    },
    #[cfg(test)]
    SelectAll,
    Deselect,
    #[cfg(test)]
    CommitFloating,
    #[cfg(test)]
    MoveSelected {
        delta: KVec2,
    },
    NudgeSelection {
        delta: KVec2,
    },
    FinishMoveSelectionDrag {
        total_delta: KVec2,
        alt_duplicate: bool,
    },
    DeleteSelection,
    ClearActiveFrame,
    Group,
    Ungroup,
    Arrange {
        mode: ArrangeMode,
    },
    EnterGroupAt {
        pt: Point,
    },
    ExitGroup,
    ExitGroupSteps {
        steps: usize,
    },

    // 輪郭変形・アンカー編集
    ReshapeEdge {
        shape_id: ShapeId,
        contour_idx: usize,
        seg_idx: usize,
        grab_t: f64,
        grab_pos: Point,
        target_pos: Point,
        orig_seg: QuadBez,
    },
    MoveVertex {
        shape_id: ShapeId,
        contour_idx: usize,
        seg_idx: usize,
        orig_vertex: Point,
        target_pos: Point,
        orig_ctrls: (Point, Point),
    },
    #[cfg(test)]
    PullEdgeFromTo {
        from: Point,
        to: Point,
    },
    #[cfg(test)]
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
    #[cfg(test)]
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
    #[cfg(test)]
    SetTransformPivot {
        world_pivot: Option<Point>,
    },
    ResetTransformPivot,
    CommitTransformDrag {
        affine: Affine,
        orig_pivot: Point,
        had_custom_pivot: bool,
        changed: bool,
    },
    CommitPivotDrag {
        new_pivot: Point,
        orig_pivot: Point,
        had_custom_pivot: bool,
        changed: bool,
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
    #[cfg(test)]
    ExitSymbolTimeline,
    #[cfg(test)]
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
    SetSymbolColorEffect {
        effect: ColorEffect,
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
    ClearTimelineCells,
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
    EndCoalesce {
        key: Option<CoalesceKey>,
    },
}

impl Action {
    /// 連続する同一種の変更を1つのUndoステップにまとめるための `CoalesceKey` を返す
    pub(crate) fn coalesce_key(&self, tab: &Tab) -> Option<CoalesceKey> {
        match self {
            Self::SetStageSize { .. } => Some(CoalesceKey::StageSize),
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
            Self::SetSymbolColorEffect { .. } => selected_single_symbol_info(tab)
                .map(|(b, _, _)| CoalesceKey::SymbolColorEffect(b.instance_gid)),
            Self::NudgeSelection { .. } => Some(CoalesceKey::NudgeSelection),
            _ => None,
        }
    }
}

// ============================================================================
// 2. Undo 記録・Dirty 更新（このモジュール内でのみ呼び出し可能）
// ============================================================================

fn push_undo_entry_from_doc(tab: &mut Tab, coalesce: Option<CoalesceKey>) {
    if tab.history.pending.is_some() {
        return;
    }
    let tl_snap = tab.doc.root.clone();
    let (library, symbols_revision) = tab.history.snapshot_symbols(&tab.doc);
    let floating = tab.history.snapshot_floating(tab.session.floating());
    let entry = UndoEntry {
        timeline: tl_snap,
        library,
        floating,
        edit_path: tab.session.edit_path.clone(),
        editing_group_path: tab.session.editing_group_path.clone(),
        stage_size: tab.doc.stage_size,
        fps: tab.doc.fps,
        symbols_revision,
        commit_id: tab.history.current_commit_id,
    };
    tab.history.pending = Some((entry, coalesce));
}

fn push_undo(tab: &mut Tab, coalesce: Option<CoalesceKey>) {
    if let Some(key) = coalesce {
        if tab.history.last_coalesce_key == Some(key) {
            return;
        }
    }
    push_undo_entry_from_doc(tab, coalesce);
}

fn reset_session_after_timeline_change(tab: &mut Tab) {
    cancel_active_drag(tab);
    tab.session.set_floating(None);
    tab.session.transform_pivot = None;
    tab.session.sanitize_editing_scope();
}

fn mutate_timeline_with_undo(tab: &mut Tab, mutate: impl FnOnce(&mut Timeline) -> bool) -> bool {
    commit_floating(tab);
    push_undo(tab, None);
    let changed = tab.with_active_timeline_mut(|tl, _| mutate(tl));
    if changed {
        reset_session_after_timeline_change(tab);
    }
    changed
}

// ============================================================================
// 3. 単一エントリポイント `apply`
// ============================================================================

/// すべての文書変更・編集操作の唯一の入口。
/// 再生停止・Undo記録・連続操作のCoalesce・タイムライン同期・Dirty更新を一元管理する。
pub(crate) fn apply(host: &mut impl ActionHost, action: Action) -> bool {
    debug_assert!(host.tab().history.pending.is_none());
    if !matches!(action, Action::SetFps { .. } | Action::EndCoalesce { .. }) {
        host.stop_playback();
    }
    let check_symbol_nest = matches!(
        action,
        Action::ConvertToSymbol | Action::Paste { .. } | Action::PlaceSymbol { .. }
    );
    let prev_ids = (host.tab().doc.next_symbol_id, host.tab().doc.next_group_id);
    let prev_imported_map =
        matches!(action, Action::Paste { .. }).then(|| host.parts_mut().2.imported_map.clone());
    let mut res = dispatch(host, action);
    let (tab, _, clipboard, toast) = host.parts_mut();
    if check_symbol_nest && res && tab.doc.symbol_nest_exceeded() {
        if let Some((prev, _)) = tab.history.pending.take() {
            tab.restore_state_snapshot(prev);
        }
        tab.ensure_valid_library_selection();
        show_notice(toast, NoticeKind::Warning, "シンボルの入れ子は8段までです");
        res = false;
    }
    if res {
        if let Some((entry, coalesce)) = tab.history.pending.take() {
            tab.history.push(entry);
            tab.history.last_coalesce_key = coalesce;
        }
    } else {
        tab.history.pending = None;
        (tab.doc.next_symbol_id, tab.doc.next_group_id) = prev_ids;
        if let Some(map) = prev_imported_map {
            clipboard.imported_map = map;
        }
    }
    res
}

fn dispatch(host: &mut impl ActionHost, action: Action) -> bool {
    let (tab, tool, clipboard, toast) = host.parts_mut();
    let coalesce = action.coalesce_key(tab);

    match action {
        Action::PaintStroke {
            points,
            radius,
            color,
        } => {
            if points.is_empty() {
                return false;
            }
            let z = (tab.view.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
            let raw = generate_brush_outline_at_zoom(&points, radius as f64, z);
            apply_raw_outline(tab, Tool::Brush, color, &raw)
        }
        Action::EraseStroke { points, radius } => {
            if points.is_empty() {
                return false;
            }
            let z = (tab.view.zoom as f64).clamp(MIN_VIEW_ZOOM as f64, MAX_VIEW_ZOOM as f64);
            let raw = generate_brush_outline_at_zoom(&points, radius as f64, z);
            apply_raw_outline(tab, Tool::Eraser, tool.brush_color, &raw)
        }
        #[cfg(test)]
        Action::CommitRawOutline { raw } => {
            apply_raw_outline(tab, tool.tool, tool.brush_color, &raw)
        }
        Action::BucketFill { pt, color } => apply_bucket_fill(tab, color, pt),

        Action::SelectSingleAt { pt } => select_single_shape_at(tab, tool.select_sub_mode, pt),
        Action::ToggleSelectionAt { scene_idx } => {
            toggle_shape_selection(tab, scene_idx);
            true
        }
        Action::RetainSelection { keep_indices } => {
            retain_selection_and_flush_deselected(tab, &keep_indices);
            true
        }
        Action::SelectMarquee { p0, p1, additive } => {
            apply_marquee_selection(tab, p0, p1, additive);
            !tab.session.selection.is_empty()
        }
        #[cfg(test)]
        Action::SelectAll => {
            clear_selection(tab);
            let editable_lids: std::collections::HashSet<u64> =
                editable_layer_ids(tab).into_iter().collect();
            tab.session.select_shapes_in_layers(&editable_lids);
            !tab.session.selection.is_empty()
        }
        Action::Deselect => {
            clear_selection(tab);
            true
        }
        #[cfg(test)]
        Action::CommitFloating => commit_floating(tab),
        #[cfg(test)]
        Action::MoveSelected { delta } => apply_move_selected(tab, delta, None),
        Action::NudgeSelection { delta } => apply_move_selected(tab, delta, coalesce),
        Action::FinishMoveSelectionDrag {
            total_delta,
            alt_duplicate,
        } => apply_finish_move_selection_drag(tab, total_delta, alt_duplicate),
        Action::DeleteSelection => prepare_delete_selected_shapes(tab, |tab| push_undo(tab, None)),
        Action::ClearActiveFrame => {
            prepare_clear_active_frame_shapes(tab, |tab| push_undo(tab, None))
        }
        Action::Group => prepare_group_selected_shapes(tab, |tab| push_undo(tab, None)),
        Action::Ungroup => prepare_ungroup_selected_shapes(tab, |tab| push_undo(tab, None)),
        Action::Arrange { mode } => prepare_arrange_selected(tab, mode, |tab| push_undo(tab, None)),
        Action::EnterGroupAt { pt } => enter_group_at(tab, pt),
        Action::ExitGroup => exit_current_group(tab),
        Action::ExitGroupSteps { steps } => {
            let mut exited = false;
            for _ in 0..steps {
                if !exit_current_group(tab) {
                    break;
                }
                exited = true;
            }
            exited
        }

        Action::ReshapeEdge {
            shape_id,
            contour_idx,
            seg_idx,
            grab_t,
            grab_pos,
            target_pos,
            orig_seg,
        } => {
            let drag_dist = (target_pos - grab_pos).hypot();
            apply_deform_shape(tab, shape_id, drag_dist, |deformed| {
                apply_pull_edge_to_shape(
                    deformed,
                    (contour_idx, seg_idx),
                    grab_t,
                    orig_seg,
                    target_pos,
                );
            })
        }
        Action::MoveVertex {
            shape_id,
            contour_idx,
            seg_idx,
            orig_vertex,
            target_pos,
            orig_ctrls,
        } => {
            let drag_dist = (target_pos - orig_vertex).hypot();
            apply_deform_shape(tab, shape_id, drag_dist, |deformed| {
                apply_move_vertex_to_shape(
                    deformed,
                    (contour_idx, seg_idx),
                    orig_vertex,
                    orig_ctrls,
                    target_pos,
                );
            })
        }
        #[cfg(test)]
        Action::PullEdgeFromTo { from, to } => apply_pull_edge_from_to(tab, from, to),
        #[cfg(test)]
        Action::MoveVertexFromTo { from, to } => apply_move_vertex_from_to(tab, from, to),
        Action::AddAnchor { pt } => apply_anchor_edit_at(tab, pt, true),
        Action::DeleteAnchor { pt } => apply_anchor_edit_at(tab, pt, false),

        #[cfg(test)]
        Action::ScaleSelection { sx, sy } => {
            let Some((_, pivot)) = tab.session.selection_bbox_and_pivot(tab.view.zoom) else {
                return false;
            };
            apply_affine_to_selected_inner(tab, affine_scale_around(pivot, sx, sy))
        }
        Action::FlipSelection { flip_x, flip_y } => {
            let Some((_, pivot)) = tab.session.selection_bbox_and_pivot(tab.view.zoom) else {
                return false;
            };
            let sx = if flip_x { -1.0 } else { 1.0 };
            let sy = if flip_y { -1.0 } else { 1.0 };
            apply_affine_to_selected_inner(tab, affine_scale_around(pivot, sx, sy))
        }
        Action::RotateSelection { angle_rad } => {
            let Some((_, pivot)) = tab.session.selection_bbox_and_pivot(tab.view.zoom) else {
                return false;
            };
            apply_affine_to_selected_inner(tab, affine_rotate_around(pivot, angle_rad))
        }
        #[cfg(test)]
        Action::SetTransformPivot { world_pivot } => {
            apply_set_selected_symbol_world_pivot(tab, world_pivot, true)
        }
        Action::ResetTransformPivot => {
            let mut changed = tab.session.transform_pivot.take().is_some();
            if apply_set_selected_symbol_world_pivot(tab, None, true) {
                changed = true;
            }
            changed
        }
        Action::CommitTransformDrag {
            affine,
            orig_pivot,
            had_custom_pivot,
            changed,
        } => {
            tab.session.clear_overlay();
            tab.session.transform_pivot = had_custom_pivot.then_some(orig_pivot);
            if changed && !tab.session.selection.is_empty() {
                let applied = apply_affine_to_selected_inner(tab, affine);
                if applied && had_custom_pivot && tab.session.transform_pivot.is_some() {
                    tab.session.transform_pivot = Some(affine * orig_pivot);
                }
                applied
            } else {
                false
            }
        }
        Action::CommitPivotDrag {
            new_pivot,
            orig_pivot,
            had_custom_pivot,
            changed,
        } => {
            tab.session.clear_overlay();
            if changed {
                if apply_set_selected_symbol_world_pivot(tab, Some(new_pivot), true) {
                    tab.session.transform_pivot = None;
                } else {
                    tab.session.transform_pivot = Some(new_pivot);
                }
                true
            } else {
                tab.session.transform_pivot = had_custom_pivot.then_some(orig_pivot);
                false
            }
        }

        Action::Copy => prepare_copy_selected_shapes(tab, clipboard),
        Action::Cut => {
            if !prepare_copy_selected_shapes(tab, clipboard) {
                return false;
            }
            prepare_delete_selected_shapes(tab, |tab| push_undo(tab, None))
        }
        Action::Paste { in_place } => apply_paste_clipboard(tab, tool, clipboard, toast, in_place),

        Action::ConvertToSymbol => {
            if let Some((sym_id, target_lid)) =
                prepare_convert_selection_to_symbol(tab, |tab| push_undo(tab, None))
            {
                tab.view.selected_library_symbol = Some(sym_id);
                if let Some(l_idx) = tab.active_timeline().layer_index_by_id(target_lid) {
                    tab.set_active_layer(l_idx);
                }
                tab.refresh_scene_base();
                tab.session.set_floating(None);
                tab.session.transform_pivot = None;
                tab.view.inspect_timeline_frame = false;
                true
            } else {
                false
            }
        }
        Action::PlaceSymbol { sym_id } => {
            if prepare_place_symbol_instance(tab, sym_id, |tab| push_undo(tab, None)) {
                if !matches!(tool.tool, Tool::Select | Tool::Transform) {
                    set_tool(tab, tool, Tool::Select);
                }
                true
            } else {
                false
            }
        }
        Action::EnterSymbolTimeline { binding } => enter_symbol_timeline(tab, binding),
        Action::EnterSymbolFromLibrary { sym_id } => enter_symbol_from_library(tab, sym_id),
        #[cfg(test)]
        Action::ExitSymbolTimeline => exit_symbol_timeline(tab),
        #[cfg(test)]
        Action::ExitSymbolToDepth { depth } => {
            let mut exited = false;
            while tab.session.edit_path.len() > depth {
                if !exit_symbol_timeline(tab) {
                    break;
                }
                exited = true;
            }
            exited
        }
        Action::RenameSymbol { sym_id, new_name } => {
            apply_rename_symbol(tab, sym_id, new_name, coalesce)
        }
        Action::DeleteSymbol { sym_id } => apply_delete_symbol(tab, sym_id),
        Action::SetSymbolPlayback {
            loop_mode,
            first_frame,
        } => apply_set_symbol_playback(tab, loop_mode, first_frame, coalesce),
        Action::SetSymbolColorEffect { effect } => {
            update_selected_symbol_binding(tab, coalesce, |b| b.color_effect = effect)
        }
        Action::ToggleTween => apply_timeline_toggle_tween(tab, toast),
        Action::SetTweenRotate { rotate } => apply_set_tween_rotate(tab, rotate),

        Action::SwitchLayerAndFrame { layer_idx, frame } => {
            apply_switch_to_layer_and_frame(tab, layer_idx, frame);
            true
        }
        Action::ScrubRulerToFrame { frame } => {
            apply_scrub_ruler_to_frame(tab, frame);
            true
        }
        Action::SelectTimelineCells { l0, f0, l1, f1 } => {
            apply_select_timeline_cells(tab, l0, f0, l1, f1);
            true
        }
        Action::ClearTimelineCells => {
            tab.set_active_cell_selection(None);
            true
        }
        Action::InsertKeyframe { copy_prev } => apply_timeline_insert_keyframe(tab, copy_prev),
        Action::DeleteKeyframe => apply_timeline_delete_keyframe(tab),
        Action::ExtendFrame => apply_timeline_extend_frame(tab),
        Action::RemoveFrame => apply_timeline_remove_frame(tab),
        Action::MoveOrCopyFrames {
            sel,
            layer_delta,
            frame_delta,
            copy_mode,
        } => apply_timeline_move_or_copy_frames(tab, sel, layer_delta, frame_delta, copy_mode),
        Action::AddLayer => apply_timeline_add_layer(tab),
        Action::DeleteActiveLayer => apply_timeline_delete_active_layer(tab),
        Action::MoveActiveLayer { up } => apply_timeline_move_active_layer(tab, up),
        Action::ReorderLayer { src_idx, dst_idx } => {
            apply_timeline_reorder_layer(tab, src_idx, dst_idx)
        }
        Action::SetLayerVisible { layer_idx, visible } => {
            apply_set_layer_visible(tab, layer_idx, visible)
        }
        Action::SetLayerLocked { layer_idx, locked } => {
            apply_set_layer_locked(tab, layer_idx, locked)
        }
        Action::SetFps { fps } => apply_set_fps(tab, fps, coalesce),
        Action::SetStageSize { size } => apply_set_stage_size(tab, size, coalesce),

        Action::SetTool { tool: target_tool } => {
            set_tool(tab, tool, target_tool);
            true
        }
        Action::ToggleSelectSubMode { mode } => {
            toggle_select_sub_mode(tab, tool, mode);
            true
        }
        Action::Undo => tab.undo(),
        Action::Redo => tab.redo(),
        Action::EndCoalesce { key } => {
            if key.is_none_or(|k| tab.history.last_coalesce_key == Some(k)) {
                tab.history.end_coalesce();
            }
            true
        }
    }
}

// ============================================================================
// 4. 各アクションの内部実装
// ============================================================================

fn apply_raw_outline(
    tab: &mut Tab,
    active_tool: Tool,
    brush_color: Color32,
    raw: &[Point],
) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab.active_timeline().is_layer_editable_by_id(target_lid) {
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
        push_undo(tab, None);
        edit_layers(tab, &[target_lid], |edit| {
            merge_shapes_into_canvas(&mut edit.shapes, stroke_shapes);
        });
        true
    } else {
        let has_bbox_overlap = tab.scene().iter().any(|s| {
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
        let mut next_shapes: Vec<FillShape> = tab
            .session
            .base_cache
            .shapes()
            .iter()
            .filter(|s| s.layer_id == target_lid)
            .cloned()
            .collect();
        let prev_revs: Vec<(u64, u64)> = next_shapes
            .iter()
            .map(|s| (s.shape_id, s.mesh_revision))
            .collect();
        erase_from_canvas(
            &mut next_shapes,
            &stroke_shapes,
            target_lid,
            &tab.session.editing_group_path,
        );
        let unchanged = next_shapes.len() == prev_revs.len()
            && next_shapes
                .iter()
                .zip(prev_revs.iter())
                .all(|(s, &(id, rev))| s.shape_id == id && s.mesh_revision == rev);
        if !unchanged {
            push_undo(tab, None);
            edit_layers(tab, &[target_lid], |edit| {
                edit.shapes = next_shapes;
            });
            true
        } else {
            false
        }
    }
}

fn collect_all_visible_shapes(tab: &Tab) -> Vec<FillShape> {
    let mut out = Vec::with_capacity(tab.scene().len());
    let tl = tab.active_timeline();
    let world_xform = tab.session.edit_path.world_xform();
    for (layer_idx, layer) in tl.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        if layer.locked {
            let mut eval =
                tl.evaluate_layer_shapes_at(layer_idx, tl.current_frame, &tab.doc.library);
            if world_xform != Affine::IDENTITY {
                for s in &mut eval {
                    s.transform(world_xform);
                }
            }
            out.extend(eval);
        } else {
            let lid = layer.id;
            out.extend(tab.scene().iter().filter(|s| s.layer_id == lid).cloned());
        }
    }
    out
}

fn apply_bucket_fill(tab: &mut Tab, fill_color: Color32, pt: Point) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab.active_timeline().is_layer_editable_by_id(target_lid) {
        return false;
    }

    // 0. 選択領域が存在する場合：選択中の「生シェイプ」のみにバケツで色を流し込む（グループは無効）
    if !tab.session.selection.is_empty() {
        let tl = tab.active_timeline();
        let clicked_selected_raw = tab.scene().iter().any(|s| {
            tab.session.selection.contains_shape(s)
                && tl.is_layer_editable_by_id(s.layer_id)
                && s.is_raw_at_scope(&tab.session.editing_group_path)
                && s.contains_point(pt)
        });
        if clicked_selected_raw {
            let any_color_diff = tab.scene().iter().any(|s| {
                tab.session.selection.contains_shape(s)
                    && tl.is_layer_editable_by_id(s.layer_id)
                    && s.is_raw_at_scope(&tab.session.editing_group_path)
                    && s.color != fill_color
            });
            if !any_color_diff {
                return false;
            }
            push_undo(tab, None);
            let editable_lids: HashSet<u64> = tab
                .active_timeline()
                .layers
                .iter()
                .filter(|l| l.visible && !l.locked)
                .map(|l| l.id)
                .collect();
            tab.session.lift_selected_raw_shapes_to_floating();
            let scope = tab.session.editing_group_path.clone();
            let sel = tab.session.selection.clone();
            tab.session.update_floating(|fl_opt| {
                if let Some(fl) = fl_opt {
                    for piece in &mut fl.pieces {
                        if sel.contains_shape(piece)
                            && editable_lids.contains(&piece.layer_id)
                            && piece.is_raw_at_scope(&scope)
                        {
                            piece.set_color(fill_color);
                        }
                    }
                    fl.modified = true;
                }
            });
            return true;
        }
        return false;
    }

    let all_visible_shapes = collect_all_visible_shapes(tab);
    let target_layer_ord = tab
        .active_timeline()
        .layer_index_by_id(target_lid)
        .unwrap_or(0);

    let scene = tab.scene();
    if let Some(hit_idx) = (0..scene.len()).rev().find(|&idx| {
        let Some(s) = scene.get(idx) else {
            return false;
        };
        s.layer_id == target_lid
            && s.is_in_editing_scope(&tab.session.editing_group_path)
            && s.contains_point(pt)
    }) {
        let tl = tab.active_timeline();
        let occluded_above = (target_layer_ord + 1..tl.layers.len()).any(|l_idx| {
            let layer = &tl.layers[l_idx];
            layer.visible
                && all_visible_shapes
                    .iter()
                    .any(|s| s.layer_id == layer.id && s.contains_point(pt))
        });

        let Some(hit_shape) = scene.get(hit_idx).cloned() else {
            return false;
        };
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
                    std::slice::from_ref(&hit_shape),
                    &overlapping_walls,
                    BoolOp::Difference,
                    fill_color,
                );
                if pieces.len() > 1 {
                    if let Some(split_piece) = pieces.into_iter().find(|p| p.contains_point(pt)) {
                        push_undo(tab, None);
                        let editing_group_path = tab.session.editing_group_path.clone();
                        edit_layers(tab, &[target_lid], |edit| {
                            merge_shapes_into_canvas(
                                &mut edit.shapes,
                                vec![split_piece.with_scope(target_lid, &editing_group_path)],
                            );
                        });
                        return true;
                    }
                }
            }

            focus_active_layer_to_shape_by_id(tab, hit_shape.shape_id);
            if !tab
                .session
                .base_cache
                .shapes()
                .iter()
                .any(|s| s.shape_id == hit_shape.shape_id)
            {
                return false;
            }
            push_undo(tab, None);
            let hit_id = hit_shape.shape_id;
            let hit_lid = hit_shape.layer_id;
            edit_layers(tab, &[hit_lid], |edit| {
                if let Some(pos) = edit.shapes.iter().position(|s| s.shape_id == hit_id) {
                    let mut target = edit.shapes.remove(pos);
                    target.set_color(fill_color);
                    merge_disjoint_fill_into_canvas(&mut edit.shapes, target);
                }
            });
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
            .scene()
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
        push_undo(tab, None);
        let editing_group_path = tab.session.editing_group_path.clone();
        edit_layers(tab, &[target_lid], |edit| {
            merge_disjoint_fill_into_canvas(
                &mut edit.shapes,
                enclosed_fill.with_scope(target_lid, &editing_group_path),
            );
        });
        return true;
    }

    false
}

fn has_editable_selection(tab: &Tab) -> bool {
    if tab.session.selection.is_empty() {
        return false;
    }
    let editable_lids = editable_layer_ids(tab);
    tab.scene().iter().any(|s| {
        tab.session.selection.contains_shape(s)
            && editable_lids.contains(&s.layer_id)
            && s.is_in_editing_scope(&tab.session.editing_group_path)
    })
}

fn transform_selected_shapes(tab: &mut Tab, affine: Affine) -> bool {
    if tab.session.selection.is_empty() {
        return false;
    }
    let _ = prepare_moved_selection(tab);
    let mut any_transformed = false;
    let selection = tab.session.selection.clone();
    tab.session.update_floating(|fl_opt| {
        if let Some(fl) = fl_opt {
            for piece in &mut fl.pieces {
                if selection.contains_shape(piece) {
                    piece.transform(affine);
                    any_transformed = true;
                }
            }
            if any_transformed {
                fl.modified = true;
            }
        }
    });
    let mut target_lids: Vec<u64> = Vec::new();
    for s in tab.session.base_cache.shapes() {
        let hidden = tab
            .session
            .floating()
            .is_some_and(|fl| fl.hidden.contains(&s.shape_id));
        if !hidden && tab.session.selection.contains_shape(s) && !target_lids.contains(&s.layer_id)
        {
            target_lids.push(s.layer_id);
        }
    }
    if !target_lids.is_empty() {
        let hidden = tab
            .session
            .floating()
            .map(|fl| fl.hidden.clone())
            .unwrap_or_default();
        edit_layers(tab, &target_lids, |edit| {
            for s in &mut edit.shapes {
                if !hidden.contains(&s.shape_id) && selection.contains_shape(s) {
                    s.transform(affine);
                    any_transformed = true;
                }
            }
        });
    }
    any_transformed
}

fn duplicate_selected_in_working_copy(tab: &mut Tab) {
    let extracted = tab.session.extract_selected_relative_to_scope();
    if extracted.is_empty() {
        return;
    }
    commit_floating(tab);
    let (duplicated, any_raw) =
        instantiate_relative_shapes_in_scope(tab, &extracted, KVec2::ZERO, true);
    insert_and_select_shapes_in_scope(tab, duplicated, any_raw);
}

fn apply_finish_move_selection_drag(
    tab: &mut Tab,
    total_delta: KVec2,
    alt_duplicate: bool,
) -> bool {
    tab.session.clear_overlay();
    let min_move = 1.0 / (tab.view.zoom as f64);
    if total_delta.hypot() <= min_move || !has_editable_selection(tab) {
        if let Some(piv) = &mut tab.session.transform_pivot {
            *piv -= total_delta;
        }
        return false;
    }
    push_undo(tab, None);
    if alt_duplicate {
        duplicate_selected_in_working_copy(tab);
    }
    transform_selected_shapes(tab, Affine::translate(total_delta))
}

fn apply_move_selected(tab: &mut Tab, delta: KVec2, coalesce: Option<CoalesceKey>) -> bool {
    cancel_active_drag(tab);
    if delta == KVec2::ZERO || !has_editable_selection(tab) {
        return false;
    }
    push_undo(tab, coalesce);
    let moved = transform_selected_shapes(tab, Affine::translate(delta));
    if let Some(piv) = &mut tab.session.transform_pivot {
        *piv += delta;
    }
    moved
}

fn apply_deform_shape(
    tab: &mut Tab,
    shape_id: ShapeId,
    drag_dist: f64,
    apply_deform: impl FnOnce(&mut FillShape),
) -> bool {
    tab.session.clear_overlay();
    let min_deform = 0.5 / (tab.view.zoom as f64);
    if drag_dist <= min_deform {
        return false;
    }
    let layer_ids = editable_layer_ids(tab);
    let in_remainder = tab.session.floating().is_some_and(|fl| {
        fl.remainder
            .iter()
            .any(|r| r.shape_id == shape_id && layer_ids.contains(&r.layer_id))
    });
    let in_base = tab
        .session
        .base_cache
        .shapes()
        .iter()
        .any(|s| s.shape_id == shape_id && layer_ids.contains(&s.layer_id));
    if !in_remainder && !in_base {
        return false;
    }
    push_undo(tab, None);
    focus_active_layer_to_shape_by_id(tab, shape_id);
    let rem_to_commit = if in_remainder {
        tab.session.update_floating(|fl_opt| {
            fl_opt.as_mut().map(|fl| {
                fl.modified = true;
                Floating {
                    pieces: Vec::new(),
                    remainder: std::mem::take(&mut fl.remainder),
                    hidden: std::mem::take(&mut fl.hidden),
                    modified: true,
                }
            })
        })
    } else {
        None
    };
    let mut deformed_ok = false;
    edit_layers(tab, &layer_ids, |edit| {
        if let Some(rem_fl) = rem_to_commit {
            merge_floating_into_base_shapes(&mut edit.shapes, rem_fl, &edit.layer_order_ids);
        }
        if let Some(pos) = edit.shapes.iter().position(|s| s.shape_id == shape_id) {
            let mut deformed = edit.shapes.remove(pos);
            apply_deform(&mut deformed);
            let resolved = resolve_deformed_shape(&deformed);
            merge_shapes_into_canvas(&mut edit.shapes, resolved);
            deformed_ok = true;
        }
    });
    deformed_ok
}

#[cfg(test)]
fn apply_pull_edge_from_to(tab: &mut Tab, from: Point, to: Point) -> bool {
    use kurbo::ParamCurve;
    clear_selection(tab);
    let target = compute_hover_target(
        tab.scene(),
        &tab.session,
        from,
        tab.view.zoom,
        SelectSubMode::Reshape,
    );
    let crate::canvas_tool::HoverTarget::Edge {
        shape_id,
        contour_idx,
        seg_idx,
        t,
        ..
    } = target
    else {
        return false;
    };
    let Some(orig_seg) = tab
        .scene()
        .get_contour_segment(shape_id, contour_idx, seg_idx)
    else {
        return false;
    };
    let grab_t = t.clamp(0.18, 0.82);
    let grab_pos = orig_seg.eval(grab_t);
    let drag_dist = (to - grab_pos).hypot();
    apply_deform_shape(tab, shape_id, drag_dist, |deformed| {
        apply_pull_edge_to_shape(deformed, (contour_idx, seg_idx), grab_t, orig_seg, to);
    })
}

#[cfg(test)]
fn apply_move_vertex_from_to(tab: &mut Tab, from: Point, to: Point) -> bool {
    clear_selection(tab);
    let target = compute_hover_target(
        tab.scene(),
        &tab.session,
        from,
        tab.view.zoom,
        SelectSubMode::Reshape,
    );
    let crate::canvas_tool::HoverTarget::Vertex {
        shape_id,
        contour_idx,
        seg_idx,
        pos,
    } = target
    else {
        return false;
    };
    let Some((orig_prev_ctrl, orig_curr_ctrl)) = tab
        .scene()
        .shape_by_id(shape_id)
        .and_then(|s| s.get_contour(contour_idx))
        .and_then(|contour| {
            let prev_idx = contour.prev_seg_idx(seg_idx)?;
            Some((contour.segments[prev_idx].p1, contour.segments[seg_idx].p1))
        })
    else {
        return false;
    };
    let drag_dist = (to - pos).hypot();
    apply_deform_shape(tab, shape_id, drag_dist, |deformed| {
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
    let target = compute_hover_target(tab.scene(), &tab.session, pt, zoom, sub_mode);
    let (shape_id, contour_idx, seg_idx, t_opt) = match (is_add, target) {
        (
            true,
            crate::canvas_tool::HoverTarget::Edge {
                shape_id,
                contour_idx,
                seg_idx,
                t,
                ..
            },
        ) => (shape_id, contour_idx, seg_idx, Some(t)),
        (
            false,
            crate::canvas_tool::HoverTarget::Vertex {
                shape_id,
                contour_idx,
                seg_idx,
                ..
            },
        ) => (shape_id, contour_idx, seg_idx, None),
        _ => return false,
    };

    let apply_to_shape = |s: &mut FillShape| -> bool {
        s.edit_contour(contour_idx, |c| {
            if let Some(t) = t_opt {
                c.insert_anchor_at(seg_idx, t)
            } else {
                c.delete_anchor_at(seg_idx)
            }
        })
    };

    if let Some(fl) = tab.session.floating() {
        if let Some(p_idx) = fl.pieces.iter().position(|p| p.shape_id == shape_id) {
            let mut test_piece = fl.pieces[p_idx].clone();
            if apply_to_shape(&mut test_piece) {
                push_undo(tab, None);
                tab.session.update_floating(|fl_opt| {
                    if let Some(fl_mut) = fl_opt {
                        fl_mut.pieces[p_idx] = test_piece;
                        fl_mut.modified = true;
                    }
                });
                return true;
            }
            return false;
        }
        if let Some(r_idx) = fl.remainder.iter().position(|r| r.shape_id == shape_id) {
            let mut test_rem = fl.remainder[r_idx].clone();
            if apply_to_shape(&mut test_rem) {
                push_undo(tab, None);
                tab.session.update_floating(|fl_opt| {
                    if let Some(fl_mut) = fl_opt {
                        fl_mut.remainder[r_idx] = test_rem;
                        fl_mut.modified = true;
                    }
                });
                return true;
            }
            return false;
        }
    }

    let Some(base_shape) = tab
        .session
        .base_cache
        .shapes()
        .iter()
        .find(|s| s.shape_id == shape_id)
        .cloned()
    else {
        return false;
    };
    let target_lid = base_shape.layer_id;
    let mut test_shape = base_shape;
    if apply_to_shape(&mut test_shape) {
        push_undo(tab, None);
        edit_layers(tab, &[target_lid], |edit| {
            if let Some(pos) = edit.shapes.iter().position(|s| s.shape_id == shape_id) {
                edit.shapes[pos] = test_shape;
            }
        });
        true
    } else {
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
    let Some(binding) = tab
        .scene()
        .selected_single_symbol_binding(&tab.session.selection)
    else {
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
    if commit_to_timeline {
        push_undo(tab, None);
    }
    let inst_gid = binding.instance_gid;
    let layer_ids = editable_layer_ids(tab);
    let mut updated = false;
    edit_layers(tab, &layer_ids, |edit| {
        for s in &mut edit.shapes {
            if let Some(b) = &mut s.symbol_instance {
                if b.instance_gid == inst_gid {
                    b.pivot = new_local_pivot;
                    updated = true;
                }
            }
        }
    });
    updated
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
    if affine == Affine::IDENTITY || !has_editable_selection(tab) {
        return false;
    }
    push_undo(tab, None);
    flush_transform_pivot_to_symbol(tab);
    transform_selected_shapes(tab, affine)
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
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab.active_timeline().is_layer_editable_by_id(target_lid) {
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
            && clipboard.source_symbol_id == tab.session.edit_path.last().map(|s| s.symbol_id)
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
        .edit_path
        .levels
        .iter()
        .any(|lvl| lvl.symbol_id == sym_id)
    {
        exit_symbol_timeline(tab);
    }
    clear_selection(tab);
    push_undo(tab, None);

    tab.doc.remove_symbol_and_instances(sym_id);
    if tab.view.selected_library_symbol == Some(sym_id) {
        tab.view.selected_library_symbol = tab.doc.library.first().map(|s| s.id);
    }

    tab.refresh_scene_base();
    reset_session_after_timeline_change(tab);
    true
}

fn apply_timeline_toggle_tween(tab: &mut Tab, toast: &mut Option<StatusToast>) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    commit_floating(tab);

    let (can_toggle, is_currently_tweened) =
        tab.active_timeline().can_toggle_tween_at_current_frame();
    if !can_toggle {
        show_notice(
            toast,
            NoticeKind::Warning,
            "トゥイーンを設定するには、キーフレームに単一のシンボルインスタンス（F8）のみを配置してください",
        );
        return false;
    }

    push_undo(tab, None);
    let next_tween = !is_currently_tweened;
    let changed = tab.with_active_timeline_mut(|tl, _| {
        let curr_f = tl.current_frame;
        let mut changed = false;
        for &l_idx in &targets {
            let layer = &mut tl.layers[l_idx];
            if let Some(kf_idx) = layer.keyframe_index_at(curr_f) {
                let kf = &layer.keyframes[kf_idx];
                if next_tween {
                    if kf.cel.single_symbol_instance().is_some() && !kf.tween {
                        layer.keyframe_mut(kf_idx).tween = true;
                        changed = true;
                    }
                } else if kf.tween {
                    layer.keyframe_mut(kf_idx).tween = false;
                    changed = true;
                }
            }
        }
        changed
    });

    if changed {
        reset_session_after_timeline_change(tab);
        select_shapes_for_timeline_selection(tab);
        tab.view.inspect_timeline_frame = true;
    }
    changed
}

fn apply_set_tween_rotate(tab: &mut Tab, rotate_mode: TweenRotate) -> bool {
    let tl = tab.active_timeline();
    let Some(layer) = tl.layers.get(tl.active_layer) else {
        return false;
    };
    if layer.locked || !layer.visible {
        return false;
    }
    let Some(kf_idx) = layer.keyframe_index_at(tl.current_frame) else {
        return false;
    };
    if !layer.keyframes[kf_idx].tween || layer.keyframes[kf_idx].tween_rotate == rotate_mode {
        return false;
    }

    commit_floating(tab);
    push_undo(tab, None);
    tab.with_active_timeline_mut(|tl, _| {
        if let Some(layer_mut) = tl.layers.get_mut(tl.active_layer) {
            if kf_idx < layer_mut.keyframes.len() {
                layer_mut.keyframe_mut(kf_idx).tween_rotate = rotate_mode;
            }
        }
    });
    reset_session_after_timeline_change(tab);
    select_shapes_for_timeline_selection(tab);
    tab.view.inspect_timeline_frame = true;
    true
}

/// 選ばれている1つのシンボルインスタンスの設定を `update` で書き換える（取り消し1回分）
fn update_selected_symbol_binding(
    tab: &mut Tab,
    coalesce: Option<CoalesceKey>,
    update: impl Fn(&mut SymbolInstanceBinding),
) -> bool {
    let Some((binding, _, _)) = selected_single_symbol_info(tab) else {
        return false;
    };
    let mut next_binding = binding;
    update(&mut next_binding);
    if next_binding == binding {
        return false;
    }

    let inst_gid = binding.instance_gid;
    let sym_id = binding.symbol_id;
    let layer_ids = editable_layer_ids(tab);
    if tab.doc.find_symbol(sym_id).is_none()
        || !tab.scene().iter().any(|s| {
            s.symbol_instance
                .is_some_and(|b| b.instance_gid == inst_gid)
                && layer_ids.contains(&s.layer_id)
        })
    {
        return false;
    }

    commit_floating(tab);
    push_undo(tab, coalesce);

    let mut updated = false;
    edit_layers(tab, &layer_ids, |edit| {
        for s in &mut edit.shapes {
            if let Some(b) = &mut s.symbol_instance {
                if b.instance_gid == inst_gid {
                    update(b);
                    updated = true;
                }
            }
        }
    });
    if updated {
        reset_session_after_timeline_change(tab);
    }
    updated
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
    update_selected_symbol_binding(tab, coalesce, |b| {
        b.loop_mode = next_mode;
        b.first_frame = clamped_first;
    })
}

/// フレーム移動（`prev_frame` -> `next_frame`）時に、同じキーフレームのスパン内に留まるレイヤー上の選択のみを残し、
/// 別のキーフレームへ跨いだレイヤー上の選択を解除する
fn retain_selection_on_same_keyframe(tab: &mut Tab, prev_frame: usize, next_frame: usize) {
    if prev_frame == next_frame || tab.session.selection.is_empty() {
        return;
    }
    let same_kf_layer_ids: HashSet<u64> = tab
        .active_timeline()
        .layers
        .iter()
        .filter(|l| {
            l.is_editable_at(next_frame)
                && l.keyframe_index_at(prev_frame).is_some()
                && l.keyframe_index_at(prev_frame) == l.keyframe_index_at(next_frame)
        })
        .map(|l| l.id)
        .collect();
    tab.session.retain_selection_in_layers(&same_kf_layer_ids);
}

fn apply_switch_to_layer_and_frame(tab: &mut Tab, layer_idx: usize, frame_idx: usize) {
    if tab.active_timeline().layers.is_empty() {
        return;
    }
    let prev_frame = tab.active_timeline().current_frame;
    commit_floating(tab);
    let clamped_layer = layer_idx.min(tab.active_timeline().layers.len().saturating_sub(1));
    tab.set_active_layer(clamped_layer);
    tab.set_active_frame(frame_idx);
    tab.set_active_cell_selection(Some(TimelineCellSelection::single(
        clamped_layer,
        frame_idx,
    )));
    let next_frame = tab.active_timeline().current_frame;
    retain_selection_on_same_keyframe(tab, prev_frame, next_frame);
    reset_session_after_timeline_change(tab);
    tab.view.inspect_timeline_frame = true;
}

fn apply_scrub_ruler_to_frame(tab: &mut Tab, frame_idx: usize) {
    let tl = tab.active_timeline();
    let clamped_frame = frame_idx.min(tl.total_frames.saturating_sub(1));
    if tl.current_frame == clamped_frame && tl.cell_selection.is_none() {
        return;
    }
    let prev_frame = tl.current_frame;
    commit_floating(tab);
    tab.set_active_frame(clamped_frame);
    tab.set_active_cell_selection(None);
    retain_selection_on_same_keyframe(tab, prev_frame, clamped_frame);
    reset_session_after_timeline_change(tab);
}

fn apply_select_timeline_cells(
    tab: &mut Tab,
    start_layer: usize,
    start_frame: usize,
    end_layer: usize,
    end_frame: usize,
) {
    let num_layers = tab.active_timeline().layers.len();
    if num_layers == 0 {
        return;
    }
    let max_l = num_layers - 1;
    let (l0, l1) = (start_layer.min(max_l), end_layer.min(max_l));
    let (f0, f1) = (start_frame, end_frame);

    apply_switch_to_layer_and_frame(tab, l1, f1);
    tab.set_active_cell_selection(Some(TimelineCellSelection::from_corners(l0, f0, l1, f1)));
    select_shapes_for_timeline_selection(tab);
}

fn apply_timeline_insert_keyframe(tab: &mut Tab, clone_prev: bool) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    let changed = mutate_timeline_with_undo(tab, |tl| {
        tl.insert_keyframe_on_targets(&targets, clone_prev)
    });
    if changed && clone_prev {
        select_shapes_for_timeline_selection(tab);
    }
    changed
}

fn apply_timeline_delete_keyframe(tab: &mut Tab) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    commit_floating(tab);
    if !tab
        .active_timeline()
        .can_delete_keyframe_on_targets(&targets)
    {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.delete_keyframe_on_targets(&targets);
        true
    })
}

fn apply_timeline_extend_frame(tab: &mut Tab) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.extend_frame_on_targets(&targets);
        true
    })
}

fn apply_timeline_remove_frame(tab: &mut Tab) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    commit_floating(tab);
    if !tab.active_timeline().can_remove_frame_on_targets(&targets) {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| tl.remove_frame_on_targets(&targets))
}

fn apply_timeline_move_or_copy_frames(
    tab: &mut Tab,
    sel: TimelineCellSelection,
    delta_layer: isize,
    delta_frame: isize,
    copy_mode: bool,
) -> bool {
    let Some((dst_sel, actual_dl)) =
        tab.active_timeline()
            .resolve_frame_move_bounds(sel, delta_layer, delta_frame)
    else {
        return false;
    };
    commit_floating(tab);
    push_undo(tab, None);
    tab.with_active_timeline_mut(|tl, doc| {
        tl.move_or_copy_frames(sel, dst_sel, actual_dl, copy_mode, || doc.alloc_group_id());
    });
    reset_session_after_timeline_change(tab);
    select_shapes_for_timeline_selection(tab);
    true
}

fn apply_timeline_add_layer(tab: &mut Tab) -> bool {
    if tab.active_timeline().layers.len() >= MAX_LAYERS_PER_TIMELINE {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.add_layer_above_active();
        true
    })
}

fn apply_timeline_delete_active_layer(tab: &mut Tab) -> bool {
    if tab.active_timeline().layers.len() <= 1 {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.delete_active_layer();
        true
    })
}

fn apply_timeline_reorder_layer(tab: &mut Tab, from_idx: usize, to_idx: usize) -> bool {
    let len = tab.active_timeline().layers.len();
    if from_idx >= len || to_idx >= len || from_idx == to_idx {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        tl.reorder_layer(from_idx, to_idx);
        true
    })
}

fn apply_timeline_move_active_layer(tab: &mut Tab, up: bool) -> bool {
    let tl = tab.active_timeline();
    let len = tl.layers.len();
    let curr = tl.active_layer;
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

fn apply_set_layer_visible(tab: &mut Tab, layer_idx: usize, visible: bool) -> bool {
    if tab
        .active_timeline()
        .layers
        .get(layer_idx)
        .is_none_or(|l| l.visible == visible)
    {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            layer.visible = visible;
        }
        true
    })
}

fn apply_set_layer_locked(tab: &mut Tab, layer_idx: usize, locked: bool) -> bool {
    if tab
        .active_timeline()
        .layers
        .get(layer_idx)
        .is_none_or(|l| l.locked == locked)
    {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            layer.locked = locked;
        }
        true
    })
}

fn apply_set_stage_size(tab: &mut Tab, next_size: Vec2, coalesce: Option<CoalesceKey>) -> bool {
    if (next_size.x - tab.doc.stage_size.x).abs() <= f32::EPSILON
        && (next_size.y - tab.doc.stage_size.y).abs() <= f32::EPSILON
    {
        return false;
    }
    push_undo(tab, coalesce);
    tab.doc.stage_size = next_size;
    true
}

fn apply_set_fps(tab: &mut Tab, next_fps: u32, coalesce: Option<CoalesceKey>) -> bool {
    let next_fps = next_fps.clamp(1, 60);
    if next_fps != tab.doc.fps {
        push_undo(tab, coalesce);
        tab.doc.fps = next_fps;
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
    apply(host, Action::BucketFill { pt, color })
}

#[cfg(test)]
pub(crate) fn finish_move_selection_drag(
    host: &mut impl ActionHost,
    total_delta: KVec2,
    alt_duplicate: bool,
) {
    apply(
        host,
        Action::FinishMoveSelectionDrag {
            total_delta,
            alt_duplicate,
        },
    );
}

#[cfg(test)]
pub(crate) fn group_selected_shapes(host: &mut impl ActionHost) -> Option<u64> {
    if !apply(host, Action::Group) {
        return None;
    }
    let tab = host.tab();
    tab.session
        .selected_indices()
        .first()
        .and_then(|&idx| tab.scene().get(idx))
        .and_then(|s| s.active_group_id_at_scope(&tab.session.editing_group_path))
}

#[cfg(test)]
pub(crate) fn ungroup_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Ungroup)
}

#[cfg(test)]
pub(crate) fn nudge_selected_shapes(host: &mut impl ActionHost, delta: KVec2) -> bool {
    apply(host, Action::NudgeSelection { delta })
}

#[cfg(test)]
pub(crate) fn delete_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteSelection)
}

#[cfg(test)]
pub(crate) fn clear_active_frame_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ClearActiveFrame)
}

#[cfg(test)]
pub(crate) fn copy_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Copy)
}

#[cfg(test)]
pub(crate) fn cut_selected_shapes(host: &mut impl ActionHost) -> bool {
    apply(host, Action::Cut)
}

#[cfg(test)]
pub(crate) fn paste_clipboard(host: &mut impl ActionHost, in_place: bool) -> bool {
    apply(host, Action::Paste { in_place })
}

#[cfg(test)]
pub(crate) fn set_selected_symbol_world_pivot(
    host: &mut impl ActionHost,
    world_pivot: Option<Point>,
    commit_to_timeline: bool,
) -> bool {
    if commit_to_timeline {
        apply(host, Action::SetTransformPivot { world_pivot })
    } else {
        update_selected_symbol_world_pivot_live(host.tab_mut(), world_pivot)
    }
}

#[cfg(test)]
pub(crate) fn scale_selected_shapes(host: &mut impl ActionHost, sx: f64, sy: f64) -> bool {
    apply(host, Action::ScaleSelection { sx, sy })
}

#[cfg(test)]
pub(crate) fn flip_selected_shapes(host: &mut impl ActionHost, flip_x: bool, flip_y: bool) -> bool {
    apply(host, Action::FlipSelection { flip_x, flip_y })
}

#[cfg(test)]
pub(crate) fn rotate_selected_shapes(host: &mut impl ActionHost, angle_rad: f64) -> bool {
    apply(host, Action::RotateSelection { angle_rad })
}

#[cfg(test)]
pub(crate) fn convert_selection_to_symbol(host: &mut impl ActionHost) -> Option<u64> {
    apply(host, Action::ConvertToSymbol).then(|| host.tab().doc.library.last().map(|s| s.id))?
}

#[cfg(test)]
pub(crate) fn place_symbol_instance_from_library(host: &mut impl ActionHost, sym_id: u64) -> bool {
    apply(host, Action::PlaceSymbol { sym_id })
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
}

#[cfg(test)]
pub(crate) fn delete_symbol_from_library(host: &mut impl ActionHost, sym_id: u64) -> bool {
    apply(host, Action::DeleteSymbol { sym_id })
}

#[cfg(test)]
pub(crate) fn timeline_toggle_tween(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ToggleTween)
}

#[cfg(test)]
pub(crate) fn set_active_keyframe_tween_rotate(
    host: &mut impl ActionHost,
    rotate: TweenRotate,
) -> bool {
    apply(host, Action::SetTweenRotate { rotate })
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
    apply(host, Action::InsertKeyframe { copy_prev })
}

#[cfg(test)]
pub(crate) fn timeline_delete_keyframe(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteKeyframe)
}

#[cfg(test)]
pub(crate) fn timeline_extend_frame(host: &mut impl ActionHost) -> bool {
    apply(host, Action::ExtendFrame)
}

#[cfg(test)]
pub(crate) fn timeline_remove_frame(host: &mut impl ActionHost) -> bool {
    apply(host, Action::RemoveFrame)
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
}

#[cfg(test)]
pub(crate) fn timeline_add_layer(host: &mut impl ActionHost) {
    apply(host, Action::AddLayer);
}

#[cfg(test)]
pub(crate) fn timeline_delete_active_layer(host: &mut impl ActionHost) -> bool {
    apply(host, Action::DeleteActiveLayer)
}

#[cfg(test)]
pub(crate) fn timeline_move_active_layer(host: &mut impl ActionHost, up: bool) -> bool {
    apply(host, Action::MoveActiveLayer { up })
}

#[cfg(test)]
pub(crate) fn timeline_reorder_layer(
    host: &mut impl ActionHost,
    src_idx: usize,
    dst_idx: usize,
) -> bool {
    apply(host, Action::ReorderLayer { src_idx, dst_idx })
}

#[cfg(test)]
pub(crate) fn set_layer_visible(host: &mut impl ActionHost, layer_idx: usize, visible: bool) {
    apply(host, Action::SetLayerVisible { layer_idx, visible });
}

#[cfg(test)]
pub(crate) fn set_layer_locked(host: &mut impl ActionHost, layer_idx: usize, locked: bool) {
    apply(host, Action::SetLayerLocked { layer_idx, locked });
}
