use std::collections::HashSet;

use eframe::egui::{Color32, Vec2};
use kurbo::{Affine, ParamCurve, Point, QuadBez, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, cancel_active_drag, clear_selection, commit_floating, edit_layer_map,
    edit_layers, editable_layer_ids, finish_cel_writes, focus_active_layer_by_id,
    focus_active_layer_to_shape_by_id, reset_session_after_timeline_change,
    select_shapes_for_timeline_selection, set_tool, show_notice, toggle_select_sub_mode,
    write_scope_map, Clipboard, CoalesceKey, HoverTarget, LayerId, MapOwner, NoticeKind,
    SelectSubMode, StatusToast, Tab, Tool, ToolSettings, UndoEntry,
};
use crate::clipboard_ops::{
    content_xform, import_clipboard_symbols, pasteable_content, prepare_copy_selected_shapes,
    prepare_placement, put_placements, selected_content,
};
use crate::planar_map::{MapEdit, MapInput, MapRule};
use crate::select_ops::{
    apply_marquee_selection, compute_hover_target, enter_group_at, exit_current_group,
    prepare_arrange_selected, prepare_clear_active_frame_shapes, prepare_delete_selected_shapes,
    prepare_group_selected_shapes, prepare_ungroup_selected_shapes,
    retain_selection_and_flush_deselected, select_single_shape_at, toggle_shape_selection,
    ArrangeMode,
};
use crate::symbol::{ColorEffect, Document, SymbolInstanceBinding, SymbolLoopMode, TweenRotate};
use crate::symbol_ops::{
    enter_symbol_from_library, enter_symbol_timeline, exit_symbol_timeline,
    prepare_convert_selection_to_symbol, prepare_place_symbol_instance,
    selected_single_symbol_info,
};
use crate::timeline::{Layer, Timeline, TimelineCellSelection, MAX_LAYERS_PER_TIMELINE};
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
    ExitGroupSteps {
        steps: usize,
    },

    // 輪郭変形・アンカー編集
    /// 境目の線で持つ地図（レイヤー `layer_id` の、編集中の階層の生の図形）で、形が `orig_seg` の辺の `grab_t` の所を
    /// `target_pos` へ引っぱる（形と位置は文書の座標。シンボルの中ではシンボルの座標）
    ReshapeEdge {
        layer_id: LayerId,
        grab_t: f64,
        target_pos: Point,
        orig_seg: QuadBez,
    },
    /// 境目の線で持つ地図（レイヤー `layer_id` の、編集中の階層の生の図形）で、`orig_vertex` にある頂点を `target_pos` へ
    /// 動かす（位置は文書の座標）
    MoveVertex {
        layer_id: LayerId,
        orig_vertex: Point,
        target_pos: Point,
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

fn mutate_timeline_with_undo(
    tab: &mut Tab,
    mutate: impl FnOnce(&mut Timeline, &mut Document) -> bool,
) -> bool {
    commit_floating(tab);
    push_undo(tab, None);
    let changed = tab.with_active_timeline_mut(mutate);
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
    // 地図にできない形を作る編集は、文書を変える前に断ってある（どの入口も同じ決まり）。ここで知らせる
    if std::mem::take(&mut tab.session.off_grid) {
        show_notice(
            toast,
            NoticeKind::Warning,
            "地図の範囲（±65,536px）の外には置けません",
        );
    }
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
            layer_id,
            grab_t,
            target_pos,
            orig_seg,
        } => apply_map_reshape(
            tab,
            layer_id,
            (orig_seg.eval(grab_t), target_pos),
            MapEdit::PullEdge {
                orig_seg,
                grab_t,
                target: target_pos,
            },
        ),
        Action::MoveVertex {
            layer_id,
            orig_vertex,
            target_pos,
        } => apply_map_reshape(
            tab,
            layer_id,
            (orig_vertex, target_pos),
            MapEdit::MoveVertex {
                orig_vertex,
                target: target_pos,
            },
        ),
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
            apply_set_selected_symbol_world_pivot(tab, world_pivot)
        }
        Action::ResetTransformPivot => {
            let mut changed = tab.session.transform_pivot.take().is_some();
            if apply_set_selected_symbol_world_pivot(tab, None) {
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
                if apply_set_selected_symbol_world_pivot(tab, Some(new_pivot)) {
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
            push_undo(tab, coalesce);
            tab.doc.rename_symbol(sym_id, new_name)
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
        Action::ReorderLayer { src_idx, dst_idx } => {
            apply_timeline_reorder_layer(tab, src_idx, dst_idx)
        }
        Action::SetLayerVisible { layer_idx, visible } => {
            apply_set_layer_flag(tab, layer_idx, visible, |l| l.visible, |l| &mut l.visible)
        }
        Action::SetLayerLocked { layer_idx, locked } => {
            apply_set_layer_flag(tab, layer_idx, locked, |l| l.locked, |l| &mut l.locked)
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
    let is_brush = active_tool == Tool::Brush;
    // 消しゴムは、消す線がどの図形にもかからなければ何もしない
    if !is_brush
        && !tab.scene().iter().any(|s| {
            s.layer_id == target_lid
                && s.is_raw_at_scope(&tab.session.editing_group_path)
                && stroke_shapes
                    .iter()
                    .any(|er| rects_overlap(s.bounding_box(), er.bounding_box()))
        })
    {
        return false;
    }
    clear_selection(tab);
    // 絵が変わらなければ（同じ色で塗られた所の中をなぞった・何も消えないなど）edit_layers が false を返し、
    // apply が Undo の控えを捨てる
    push_undo(tab, None);
    let rule = MapRule::Paint(is_brush.then_some(brush_color));
    // 地図は文書の座標なので、線の形も文書の座標へ写す。格子に置けない所（±65,536px の外）の線は描かない
    let world = tab.session.edit_path.world_xform();
    let strokes: Vec<FillShape> = stroke_shapes
        .iter()
        .map(|s| s.to_doc(world).into_owned())
        .collect();
    edit_layer_map(tab, target_lid, |base| {
        base.map.overlay(MapInput::Shapes(&strokes), rule)
    })
}

/// 表示中の全レイヤーの図形を、下のレイヤーから順に、ステージ座標で集める（バケツとスポイト用）
pub(crate) fn collect_all_visible_shapes(tab: &Tab) -> Vec<FillShape> {
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
            if !tab.session.lift_selected_raw_shapes_to_floating() {
                return false;
            }
            let scope = tab.session.editing_group_path.clone();
            let ids = tab.session.selection.shapes.clone();
            tab.session.update_floating(|fl_opt| {
                if let Some(fl) = fl_opt {
                    fl.recolor(&ids, fill_color, &editable_lids, &scope);
                    fl.modified = true;
                }
            });
            // 同じ色でとなり合う片は1つになる（新しい番号）
            tab.session.select_floating_pieces();
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
        // 前面のレイヤーに覆われている所・グループやシンボルの上は塗らない
        if occluded_above || !hit_shape.is_raw_at_scope(&tab.session.editing_group_path) {
            return false;
        }
        if hit_shape.color == fill_color {
            return false;
        }
        focus_active_layer_to_shape_by_id(tab, hit_shape.shape_id);
    }
    fill_face_at(tab, target_lid, fill_color, pt)
}

/// バケツ：レイヤー `target_lid`・編集中の階層の生の図形を地図にして、点 `pt` を含む面（色のある面、または
/// 同じレイヤー・同じ階層の線で囲まれた何も無い所）を `fill_color` にする（Animate と同じく、ほかのレイヤーや
/// グループの図形は壁にしない）
fn fill_face_at(tab: &mut Tab, target_lid: u64, fill_color: Color32, pt: Point) -> bool {
    // 塗っても絵が変わらないとき（同じ色・囲まれていない所）と、格子に置けない所（±65,536px の外）では、edit_layers が
    // false を返し、apply が Undo の控えを捨てる
    push_undo(tab, None);
    // 地図は文書の座標なので、点も文書の座標へ写す
    let doc_pt = tab.session.edit_path.world_xform().inverse() * pt;
    edit_layer_map(tab, target_lid, |base| {
        base.map.with_face_filled(doc_pt, fill_color)
    })
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
    // 選んだ生の図形は片に持ち上げて、片の変換で動かす（持ち上げられなければ断る）
    if !tab.session.lift_selected_raw_shapes_to_floating() {
        return false;
    }
    let mut any_transformed = false;
    let selection = tab.session.selection.clone();
    // 地図は格子の内の形しか持てないので、生の図形（浮いた片と、選んだグループの中の生の図形）が格子の外（±65,536px の
    // 外）へ出る変形は、文書を変える前に断る（インスタンスは地図を持たないので、どこへでも動かせる）
    let xform = tab.session.edit_path.world_xform();
    let hidden = tab
        .session
        .floating()
        .map(|fl| fl.hidden().clone())
        .unwrap_or_default();
    let fits_moved = |s: &FillShape| {
        let mut moved = s.clone();
        moved.transform(affine);
        crate::planar_map::fits_grid(&moved, xform)
    };
    let fits = tab
        .session
        .floating()
        .is_none_or(|fl| fl.pieces().iter().all(fits_moved))
        && tab
            .session
            .base_cache
            .shapes()
            .iter()
            .filter(|s| {
                s.symbol_instance.is_none()
                    && !hidden.contains(&s.shape_id)
                    && selection.contains_shape(s)
            })
            .all(fits_moved);
    if !fits {
        tab.session.off_grid = true;
        return false;
    }
    // 浮いた片（いつもみな選ばれている）は、片の組の変換に、文書の座標の変換を重ねる（確定のときに地図にかける）
    tab.session.update_floating(|fl_opt| {
        if let Some(fl) = fl_opt {
            if !fl.pieces().is_empty() {
                fl.transform(xform.inverse() * affine * xform);
                any_transformed = true;
            }
        }
    });
    let mut target_lids: Vec<u64> = Vec::new();
    for s in tab.session.base_cache.shapes() {
        if !hidden.contains(&s.shape_id)
            && selection.contains_shape(s)
            && !target_lids.contains(&s.layer_id)
        {
            target_lids.push(s.layer_id);
        }
    }
    if !target_lids.is_empty() {
        // 選んだグループの中の生の図形は、階層ごとに、セル画の地図に文書の座標の変換をかけて、その座標の格子で正しい地図に
        // する（`realized`。図形はその地図の面で、番号はそのまま）
        let world = tab.session.edit_path.world_xform();
        let doc = world.inverse() * affine * world;
        let picked = |s: &FillShape| !hidden.contains(&s.shape_id) && selection.contains_shape(s);
        let mut scopes: Vec<(LayerId, Vec<u64>)> = Vec::new();
        for s in tab.session.base_cache.shapes() {
            if picked(s)
                && s.symbol_instance.is_none()
                && !scopes
                    .iter()
                    .any(|x| x.0 == s.layer_id && x.1 == s.group_path)
            {
                scopes.push((s.layer_id, s.group_path.clone()));
            }
        }
        let mut written: Vec<LayerId> = Vec::new();
        for (layer_id, group_path) in &scopes {
            if write_scope_map(tab, *layer_id, group_path, |base| {
                crate::planar_map::realized(&base.map, doc)
            }) && !written.contains(layer_id)
            {
                written.push(*layer_id);
            }
        }
        any_transformed |= !written.is_empty();
        finish_cel_writes(tab, &written);
        // インスタンスは、インスタンスの変換を動かす（生の図形は、片か、グループの階層の地図で動かした）
        edit_layers(tab, &target_lids, |edit| {
            for s in &mut edit.shapes {
                if picked(s) && s.symbol_instance.is_some() {
                    s.transform(affine);
                    any_transformed = true;
                }
            }
        });
    }
    any_transformed
}

/// `Alt` 複製：選んでいるものの中身（`selected_content`）を、同じ所にそのまま置く（変換は恒等なので、地図はそのまま写る。
/// 面・グループ・インスタンスには新しい番号）。置いたものを選ぶ（このあと、それを動かす）
fn duplicate_selected_in_working_copy(tab: &mut Tab) {
    let Some(content) = selected_content(tab) else {
        return;
    };
    let xform = content_xform(
        content.world,
        tab.session.edit_path.world_xform(),
        KVec2::ZERO,
    );
    let Some(placement) = prepare_placement(tab, &content, xform, true, None) else {
        tab.session.off_grid = true;
        return;
    };
    commit_floating(tab);
    tab.session.selection.clear();
    put_placements(tab, vec![placement], |sym| sym);
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

/// 輪郭の変形を確定する。図形のレイヤー・階層の生の図形を境目の線で持つ地図にして編集し、変わった図形だけを差し替える。
/// つかんだ所の、動かす前と後の位置（`from`・`to`。文書の座標）の距離は、ステージで測る
fn apply_map_reshape(
    tab: &mut Tab,
    layer_id: LayerId,
    (from, to): (Point, Point),
    map_edit: MapEdit,
) -> bool {
    tab.session.clear_overlay();
    let xform = tab.session.edit_path.world_xform();
    let min_deform = 0.5 / (tab.view.zoom as f64);
    if (xform * to - xform * from).hypot() <= min_deform {
        return false;
    }
    apply_map_edit(tab, layer_id, map_edit)
}

/// 地図での編集（輪郭の変形・アンカーの追加・削除）を確定する。レイヤー `layer_id` の、編集中の階層の生の図形を境目の線で持つ
/// 地図にして編集し、変わった図形だけを差し替える
fn apply_map_edit(tab: &mut Tab, layer_id: LayerId, map_edit: MapEdit) -> bool {
    if !editable_layer_ids(tab).contains(&layer_id) {
        return false;
    }
    // 輪郭の変形は、始めるときに選択を解除して `Floating` を確定している。`Floating` があるときは何もしない
    if tab.session.floating().is_some() {
        return false;
    }
    // 形が変わらないとき・編集できないとき（消せない頂点など）は、edit_layer_map が false を返し、apply が Undo の控えを捨てる
    push_undo(tab, None);
    let changed = edit_layer_map(tab, layer_id, |base| map_edit.apply(&base.map));
    if changed {
        focus_active_layer_by_id(tab, layer_id);
    }
    changed
}

#[cfg(test)]
fn apply_pull_edge_from_to(tab: &mut Tab, from: Point, to: Point) -> bool {
    clear_selection(tab);
    let target = compute_hover_target(
        tab.scene(),
        &tab.session,
        from,
        tab.view.zoom,
        SelectSubMode::Reshape,
    );
    let HoverTarget::Edge {
        t,
        map_edge: Some((MapOwner::Layer(layer_id), orig_seg)),
        ..
    } = target
    else {
        return false;
    };
    let grab_t = t.clamp(0.18, 0.82);
    let target = tab.session.edit_path.world_xform().inverse() * to;
    apply_map_reshape(
        tab,
        layer_id,
        (orig_seg.eval(grab_t), target),
        MapEdit::PullEdge {
            orig_seg,
            grab_t,
            target,
        },
    )
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
    let HoverTarget::Vertex {
        map_node: Some((MapOwner::Layer(layer_id), _, orig_vertex)),
        ..
    } = target
    else {
        return false;
    };
    let target = tab.session.edit_path.world_xform().inverse() * to;
    apply_map_reshape(
        tab,
        layer_id,
        (orig_vertex, target),
        MapEdit::MoveVertex {
            orig_vertex,
            target,
        },
    )
}

fn apply_anchor_edit_at(tab: &mut Tab, pt: Point, is_add: bool) -> bool {
    let zoom = tab.view.zoom;
    let sub_mode = if is_add {
        SelectSubMode::AddAnchor
    } else {
        SelectSubMode::DeleteAnchor
    };
    let mut target = compute_hover_target(tab.scene(), &tab.session, pt, zoom, sub_mode);
    let edit_of = |target: HoverTarget| match (is_add, target) {
        (
            true,
            HoverTarget::Edge {
                t,
                map_edge: Some((owner, orig_seg)),
                ..
            },
        ) => Some((owner, MapEdit::AddAnchor { orig_seg, t })),
        (
            false,
            HoverTarget::Vertex {
                map_node: Some((owner, _, orig_vertex)),
                ..
            },
        ) => Some((owner, MapEdit::DeleteAnchor { orig_vertex })),
        _ => None,
    };
    // 動かして浮いている片は、そのレイヤーの片の地図で編集する（境目なら両側の片が変わる）
    if let Some((MapOwner::Floating(layer_id, group), edit)) = edit_of(target) {
        return edit_floating_anchor(tab, layer_id, group, edit);
    }
    // ほかはレイヤーの地図で編集する（境目なら両側の図形が変わる）。選択・浮いている図形があれば、先に確定して探し直す
    if !tab.session.selection.is_empty() || tab.session.floating().is_some() {
        clear_selection(tab);
        target = compute_hover_target(tab.scene(), &tab.session, pt, zoom, sub_mode);
    }
    match edit_of(target) {
        Some((MapOwner::Layer(layer_id), edit)) => apply_map_edit(tab, layer_id, edit),
        _ => false,
    }
}

/// 動かして浮いている片のうち、レイヤー `layer_id` の組 `group` の片の地図（アンカーのカーソル合わせと同じ地図）を `edit` で
/// 編集する（境目なら両側の片が変わる。選択も入れ替える）
fn edit_floating_anchor(tab: &mut Tab, layer_id: LayerId, group: usize, edit: MapEdit) -> bool {
    let scope = tab.session.editing_group_path.clone();
    let Some(mut fl) = tab.session.floating().filter(|fl| fl.modified).cloned() else {
        return false;
    };
    if !fl.edit_group(layer_id, group, &scope, |m| edit.apply(m)) {
        return false;
    }
    push_undo(tab, None);
    tab.session.set_floating(Some(fl));
    tab.session.select_floating_pieces();
    true
}

fn apply_set_selected_symbol_world_pivot(tab: &mut Tab, world_pivot: Option<Point>) -> bool {
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
    // 変形の確定（apply_affine_to_selected_inner）から呼ぶときは、もう控えがあるので何もしない
    push_undo(tab, None);
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

fn apply_affine_to_selected_inner(tab: &mut Tab, affine: Affine) -> bool {
    cancel_active_drag(tab);
    if affine == Affine::IDENTITY || !has_editable_selection(tab) {
        return false;
    }
    push_undo(tab, None);
    if let Some(piv) = tab.session.transform_pivot {
        if apply_set_selected_symbol_world_pivot(tab, Some(piv)) {
            tab.session.transform_pivot = None;
        }
    }
    transform_selected_shapes(tab, affine)
}

fn apply_paste_clipboard(
    tab: &mut Tab,
    tool: &mut ToolSettings,
    clipboard: &mut Clipboard,
    toast: &mut Option<StatusToast>,
    in_place: bool,
) -> bool {
    if clipboard.content.is_none() {
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

    let (content, had_cycle) = pasteable_content(tab, clipboard);
    if had_cycle {
        show_notice(
            toast,
            NoticeKind::Warning,
            "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
        );
    }
    let Some(content) = content else {
        return false;
    };

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

    // 同じタイムラインの、いくつかのレイヤーから写した中身は、元のレイヤーへ置く（編集できなければ作業中のレイヤー）
    let same_timeline_context = clipboard.source_doc_id == tab.id
        && clipboard.source_symbol_id == tab.session.edit_path.last().map(|s| s.symbol_id)
        && tab.session.editing_group_path.is_empty();
    let multi_layer_copy = same_timeline_context
        && content
            .layers
            .first()
            .is_some_and(|first| content.layers.iter().any(|l| l.0 != first.0));
    // 置く準備（格子の外へ出るなら、文書を変えずに断る）をしてから、選択を確定し、シンボルを移植して置く
    let xform = content_xform(content.world, tab.session.edit_path.world_xform(), offset);
    let Some(placement) = prepare_placement(tab, &content, xform, multi_layer_copy, None) else {
        tab.session.off_grid = true;
        return false;
    };
    clear_selection(tab);
    push_undo(tab, None);
    let sym_map = import_clipboard_symbols(tab, clipboard, &content);
    put_placements(tab, vec![placement], |sym| {
        sym_map.get(&sym).copied().unwrap_or(sym)
    });
    if !matches!(tool.tool, Tool::Select | Tool::Transform) {
        set_tool(tab, tool, Tool::Select);
    }
    true
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

    let next_tween = !is_currently_tweened;
    let changed = mutate_timeline_with_undo(tab, |tl, _| {
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

    let changed = mutate_timeline_with_undo(tab, |tl, _| {
        if let Some(layer_mut) = tl.layers.get_mut(tl.active_layer) {
            if kf_idx < layer_mut.keyframes.len() {
                layer_mut.keyframe_mut(kf_idx).tween_rotate = rotate_mode;
                return true;
            }
        }
        false
    });
    if changed {
        select_shapes_for_timeline_selection(tab);
        tab.view.inspect_timeline_frame = true;
    }
    changed
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
    let changed = mutate_timeline_with_undo(tab, |tl, _| {
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
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.delete_keyframe_on_targets(&targets);
        true
    })
}

fn apply_timeline_extend_frame(tab: &mut Tab) -> bool {
    let targets = tab.active_timeline().target_layers_for_frame_op();
    if targets.is_empty() {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
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
    mutate_timeline_with_undo(tab, |tl, _| tl.remove_frame_on_targets(&targets))
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
    mutate_timeline_with_undo(tab, |tl, doc| {
        tl.move_or_copy_frames(sel, dst_sel, actual_dl, copy_mode, || doc.alloc_group_id());
        true
    });
    select_shapes_for_timeline_selection(tab);
    true
}

fn apply_timeline_add_layer(tab: &mut Tab) -> bool {
    if tab.active_timeline().layers.len() >= MAX_LAYERS_PER_TIMELINE {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.add_layer_above_active();
        true
    })
}

fn apply_timeline_delete_active_layer(tab: &mut Tab) -> bool {
    if tab.active_timeline().layers.len() <= 1 {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.delete_active_layer();
        true
    })
}

fn apply_timeline_reorder_layer(tab: &mut Tab, from_idx: usize, to_idx: usize) -> bool {
    let len = tab.active_timeline().layers.len();
    if from_idx >= len || to_idx >= len || from_idx == to_idx {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        tl.reorder_layer(from_idx, to_idx);
        true
    })
}

/// レイヤー `layer_idx` の表示・ロックの印（`get` で読み、`get_mut` で書く）を `value` にする
fn apply_set_layer_flag(
    tab: &mut Tab,
    layer_idx: usize,
    value: bool,
    get: fn(&Layer) -> bool,
    get_mut: fn(&mut Layer) -> &mut bool,
) -> bool {
    if tab
        .active_timeline()
        .layers
        .get(layer_idx)
        .is_none_or(|l| get(l) == value)
    {
        return false;
    }
    mutate_timeline_with_undo(tab, |tl, _| {
        if let Some(layer) = tl.layers.get_mut(layer_idx) {
            *get_mut(layer) = value;
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
) -> bool {
    apply(host, Action::SetTransformPivot { world_pivot })
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
