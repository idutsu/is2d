use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    EditorDoc, active_scope_layer_id, cancel_active_drag, clear_selection,
    flush_canvas_before_timeline_op, insert_and_select_shapes_in_scope, load_shapes_from_timeline,
    mutate_timeline_with_undo, reorder_selected_to_scope_front,
    select_shapes_for_timeline_selection, stop_playback, sync_shapes_to_timeline,
    topmost_layer_id_for_indices,
};
use crate::symbol::{
    SymbolEditSession, SymbolEvalCtx, SymbolInstanceBinding, SymbolLoopMode, TweenRotate,
    resolve_cel_with_library, transform_layers_with_local_cache,
};
use crate::timeline::Cel;
use crate::vector_engine::{FillShape, union_bounding_boxes};

/// 選択中のオブジェクト（生シェイプ・グループ問わず）を新しい「シンボル」に変換してライブラリに登録する（`F8`）
pub(crate) fn convert_selection_to_symbol(doc: &mut EditorDoc) -> Option<u64> {
    stop_playback(doc);
    cancel_active_drag(doc);
    if doc.canvas.selected_shapes.is_empty() {
        return None;
    }
    let snapshot = doc.canvas.take_or_clone_unsliced_snapshot();
    let valid_indices: Vec<usize> = doc
        .canvas
        .selected_shapes
        .iter()
        .copied()
        .filter(|&idx| {
            doc.canvas
                .shapes
                .get(idx)
                .is_some_and(|s| s.is_in_editing_scope(&doc.canvas.editing_group_path))
        })
        .collect();
    if valid_indices.is_empty() {
        doc.canvas.restore_unsliced_snapshot(snapshot);
        return None;
    }

    let mut bbox = doc.canvas.shapes[valid_indices[0]].bounding_box();
    for &idx in &valid_indices[1..] {
        bbox = bbox.union(doc.canvas.shapes[idx].bounding_box());
    }
    let center = Point::new((bbox.x0 + bbox.x1) * 0.5, (bbox.y0 + bbox.y1) * 0.5);

    let sym_id = doc.symbols.next_symbol_id;
    let inst_gid = doc.symbols.alloc_group_id();

    let depth = doc.canvas.editing_group_path.len();
    let to_local = KVec2::new(-center.x, -center.y);
    let mut local_shapes = Vec::with_capacity(valid_indices.len());
    for &idx in &valid_indices {
        let mut local_s = doc.canvas.shapes[idx].clone();
        local_s.layer_id = 1;
        local_s.group_path = local_s.group_path.get(depth..).unwrap_or_default().to_vec();
        if local_s
            .symbol_instance
            .is_some_and(|b| !local_s.group_path.contains(&b.instance_gid))
        {
            local_s.symbol_instance = None;
        }
        local_s.translate(to_local);
        local_shapes.push(local_s);
    }

    let target_lid = topmost_layer_id_for_indices(doc, &valid_indices);
    let binding = SymbolInstanceBinding::new(
        inst_gid,
        sym_id,
        Affine::translate(KVec2::new(center.x, center.y)),
    );
    for &idx in &valid_indices {
        if let Some(shape) = doc.canvas.shapes.get_mut(idx) {
            shape.layer_id = target_lid;
            shape.group_path.insert(depth, inst_gid);
            shape.symbol_instance = Some(binding);
        }
    }

    if let Some(l_idx) = doc.timeline.layer_index_by_id(target_lid) {
        doc.timeline.active_layer = l_idx;
    }
    let snapshot = doc.canvas.take_unsliced_snapshot().unwrap_or(snapshot);
    doc.record_undo_snapshot(snapshot);
    doc.symbols.register_new_symbol(local_shapes);
    reorder_selected_to_scope_front(doc);
    sync_shapes_to_timeline(doc);
    doc.canvas.selection_modified = false;
    doc.canvas.transform_pivot = None;
    doc.ui.inspect_timeline_frame = false;
    Some(sym_id)
}

/// ライブラリ内の指定シンボル `symbol_id` の新しいインスタンスを現在のステージ中央へ配置する
pub(crate) fn place_symbol_instance_from_library(doc: &mut EditorDoc, symbol_id: u64) -> bool {
    stop_playback(doc);
    let target_lid = active_scope_layer_id(doc);
    if !doc.symbols.can_place_symbol_in_current_context(symbol_id)
        || !doc.timeline.is_layer_editable_by_id(target_lid)
    {
        return false;
    }
    let Some(first_shapes) = doc
        .symbols
        .find_symbol(symbol_id)
        .map(|s| s.first_non_empty_local_shapes(&doc.symbols.library))
    else {
        return false;
    };
    let first_bbox_center = if first_shapes.is_empty() {
        Point::ZERO
    } else {
        union_bounding_boxes(&first_shapes).center()
    };

    doc.mutate_canvas_with_undo(true, |doc| {
        let inst_gid = doc.symbols.alloc_group_id();
        let stage_center = doc.viewport.stage_center_world();
        let offset = stage_center - first_bbox_center.to_vec2();
        let binding = SymbolInstanceBinding::new(
            inst_gid,
            symbol_id,
            Affine::translate(KVec2::new(offset.x, offset.y)),
        );
        let target_lid = active_scope_layer_id(doc);
        let curr_frame = doc.timeline.current_frame;
        let kf_start = doc
            .timeline
            .layer_index_by_id(target_lid)
            .and_then(|l_idx| {
                doc.timeline.layers[l_idx]
                    .keyframe_index_at(curr_frame)
                    .map(|ki| doc.timeline.layers[l_idx].keyframes[ki].start_frame)
            })
            .unwrap_or(curr_frame);
        let sym_shapes = doc
            .symbols
            .find_symbol(symbol_id)
            .map(|s| {
                s.evaluated_local_shapes_for_instance(
                    binding,
                    kf_start,
                    curr_frame,
                    SymbolEvalCtx::new(&doc.symbols.library),
                )
            })
            .unwrap_or_default();

        let placed = if !sym_shapes.is_empty() {
            binding.instantiate_shapes(&sym_shapes, target_lid, &doc.canvas.editing_group_path)
        } else {
            vec![FillShape::new_empty_symbol_marker(
                binding,
                target_lid,
                &doc.canvas.editing_group_path,
            )]
        };
        insert_and_select_shapes_in_scope(doc, placed, false);
    });
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の名前を変更し、Undo履歴に記録する
pub(crate) fn rename_symbol_in_library(
    doc: &mut EditorDoc,
    symbol_id: u64,
    new_name: impl Into<String>,
) -> bool {
    stop_playback(doc);
    let new_name = new_name.into();
    if doc
        .symbols
        .find_symbol(symbol_id)
        .is_none_or(|s| s.name == new_name)
    {
        return false;
    }
    if doc.symbols.actively_renaming_symbol != Some(symbol_id) {
        doc.push_undo();
    }
    let renamed = doc.symbols.rename_symbol(symbol_id, new_name);
    if renamed {
        doc.symbols.actively_renaming_symbol = Some(symbol_id);
    }
    renamed
}

/// ライブラリから指定シンボル `symbol_id` を削除し、ステージ上および全フレームに配置されていた該当インスタンスは
/// 消去せず通常のグループ図形へ自動分解（デタッチ）して残す
pub(crate) fn delete_symbol_from_library(doc: &mut EditorDoc, symbol_id: u64) -> bool {
    stop_playback(doc);
    if doc.symbols.find_symbol(symbol_id).is_none() {
        return false;
    }
    while doc
        .symbols
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == symbol_id)
    {
        exit_symbol_timeline(doc);
    }
    clear_selection(doc);
    doc.push_undo();

    doc.symbols
        .remove_symbol_and_detach(symbol_id, &mut doc.timeline, &mut doc.canvas.shapes);

    doc.symbols.sync_active_edit(&doc.timeline);
    load_shapes_from_timeline(doc);
    true
}

/// 現在のキーフレーム（または保持フレームが属する開始キーフレーム）に対してクラシックトゥイーンを ON / OFF トグルする（`T` キー）。
pub(crate) fn timeline_toggle_tween(doc: &mut EditorDoc) -> bool {
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

    doc.push_undo();
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

/// 現在のアクティブレイヤー・現在フレームが属するトゥイーンキーフレームの回転方向 `tween_rotate` を設定する
pub(crate) fn set_active_keyframe_tween_rotate(
    doc: &mut EditorDoc,
    rotate_mode: TweenRotate,
) -> bool {
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

    mutate_timeline_with_undo(doc, |tl| {
        if let Some(layer_mut) = tl.layers.get_mut(tl.active_layer) {
            if let Some(kf) = layer_mut.keyframes.get_mut(kf_idx) {
                kf.tween_rotate = rotate_mode;
            }
        }
    });
    select_shapes_for_timeline_selection(doc);
    doc.ui.inspect_timeline_frame = true;
    true
}

/// 現在選択されている図形（またはアクティブセル上の単一シンボル）について、その `(SymbolInstanceBinding, シンボル名, シンボル総フレーム数)` を返す
pub(crate) fn selected_single_symbol_info(
    doc: &EditorDoc,
) -> Option<(SymbolInstanceBinding, String, usize)> {
    let binding = doc.canvas.selected_single_symbol_binding().or_else(|| {
        let layer = doc.timeline.layers.get(doc.timeline.active_layer)?;
        if !layer.visible || layer.locked {
            return None;
        }
        layer
            .cel_at(doc.timeline.current_frame)?
            .single_symbol_instance()
    })?;
    let sym = doc.symbols.find_symbol(binding.symbol_id)?;
    Some((binding, sym.name.clone(), sym.timeline.total_frames.max(1)))
}

/// 現在選択されている単一シンボルインスタンスの再生モードおよび開始/指定フレーム番号（0始まり）を更新する
pub(crate) fn set_selected_symbol_playback(
    doc: &mut EditorDoc,
    loop_mode: SymbolLoopMode,
    first_frame: usize,
) -> bool {
    stop_playback(doc);
    let Some((binding, _, total_frames)) = selected_single_symbol_info(doc) else {
        return false;
    };
    let clamped_first = first_frame.min(total_frames.saturating_sub(1));
    if binding.loop_mode == loop_mode && binding.first_frame == clamped_first {
        return false;
    }

    let inst_gid = binding.instance_gid;
    let sym_id = binding.symbol_id;
    clear_selection(doc);
    sync_shapes_to_timeline(doc);
    if !doc.ui.editing_symbol_playback {
        doc.push_undo();
    }

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
                b.loop_mode = loop_mode;
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

/// シンボルインスタンスをダブルクリックして、そのシンボル専用の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_timeline(doc: &mut EditorDoc, binding: SymbolInstanceBinding) -> bool {
    let Some(sym_def) = doc.symbols.find_symbol(binding.symbol_id).cloned() else {
        return false;
    };

    flush_canvas_before_timeline_op(doc);

    let mut backdrop_shapes: Vec<FillShape> = doc
        .symbols
        .edit_stack
        .last()
        .map(|s| {
            resolve_cel_with_library(
                &s.backdrop_cel,
                s.parent_timeline.current_frame,
                s.parent_timeline.current_frame,
                SymbolEvalCtx::new(&doc.symbols.library),
            )
        })
        .unwrap_or_default();
    let curr_f = doc.timeline.current_frame;
    for layer_idx in 0..doc.timeline.layers.len() {
        let layer = &doc.timeline.layers[layer_idx];
        if !layer.visible {
            continue;
        }
        let kf_start = layer
            .keyframe_index_at(curr_f)
            .map(|ki| layer.keyframes[ki].start_frame)
            .unwrap_or(curr_f);
        let resolved =
            doc.timeline
                .evaluate_layer_shapes_at(layer_idx, curr_f, &doc.symbols.library);
        for mut s in resolved {
            if s.symbol_instance
                .is_some_and(|b| b.instance_gid == binding.instance_gid)
            {
                continue;
            }
            if let Some(mut b) = s.symbol_instance {
                if let Some(sym) = doc.symbols.find_symbol(b.symbol_id) {
                    b.first_frame = sym.resolve_internal_frame(b, kf_start, curr_f);
                    b.loop_mode = SymbolLoopMode::SingleFrame;
                    s.symbol_instance = Some(b);
                }
            }
            backdrop_shapes.push(s);
        }
    }

    let (sym_layers, stage_to_local_cache) =
        transform_layers_with_local_cache(&sym_def.timeline.layers, binding.xform);
    let sym_timeline = doc.timeline.new_for_symbol_edit(&sym_def, sym_layers);
    doc.symbols.selected_library_symbol = Some(sym_def.id);

    let parent_timeline = std::mem::replace(&mut doc.timeline, sym_timeline);
    let parent_editing_group_path = std::mem::take(&mut doc.canvas.editing_group_path);
    doc.symbols.edit_stack.push(SymbolEditSession {
        symbol_id: binding.symbol_id,
        instance_gid: binding.instance_gid,
        instance_xform: binding.xform,
        parent_timeline,
        parent_editing_group_path,
        backdrop_cel: Cel::new(backdrop_shapes),
        stage_to_local_cache,
    });
    doc.symbols.bump_revision();
    doc.timeline_switched = true;

    load_shapes_from_timeline(doc);
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_from_library(doc: &mut EditorDoc, symbol_id: u64) -> bool {
    stop_playback(doc);
    if doc
        .symbols
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == symbol_id)
    {
        return false;
    }
    let has_instance_in_current_timeline = doc
        .canvas
        .shapes
        .iter()
        .any(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == symbol_id))
        || doc.timeline.layers.iter().any(|l| {
            l.keyframes.iter().any(|kf| {
                kf.cel
                    .instances
                    .iter()
                    .any(|inst| inst.binding.symbol_id == symbol_id)
            })
        });
    if !doc.symbols.edit_stack.is_empty() && !has_instance_in_current_timeline {
        while !doc.symbols.edit_stack.is_empty() {
            exit_symbol_timeline(doc);
        }
    }
    let curr_f = doc.timeline.current_frame;
    let binding =
        doc.canvas
            .shapes
            .iter()
            .find_map(|s| s.symbol_instance.filter(|b| b.symbol_id == symbol_id))
            .or_else(|| {
                doc.timeline.layers.iter().find_map(|l| {
                    l.cel_at(curr_f)?.instances.iter().find_map(|inst| {
                        (inst.binding.symbol_id == symbol_id).then_some(inst.binding)
                    })
                })
            })
            .unwrap_or_else(|| {
                let stage_center = doc.viewport.stage_center_world();
                SymbolInstanceBinding::new(
                    0,
                    symbol_id,
                    Affine::translate(KVec2::new(stage_center.x, stage_center.y)),
                )
            });
    enter_symbol_timeline(doc, binding)
}

/// 現在入っているシンボル内部タイムライン編集モードから1段階上の親シーン（または親シンボル）タイムラインへ戻る
pub(crate) fn exit_symbol_timeline(doc: &mut EditorDoc) -> bool {
    if doc.symbols.edit_stack.is_empty() {
        return false;
    }
    flush_canvas_before_timeline_op(doc);

    let Some(mut session) = doc.symbols.edit_stack.pop() else {
        return false;
    };
    session.parent_timeline.fps = doc.timeline.fps;
    let sym_timeline = std::mem::replace(&mut doc.timeline, session.parent_timeline);
    doc.canvas.editing_group_path = session.parent_editing_group_path;
    doc.timeline_switched = true;

    if session.instance_xform.determinant().abs() > 1e-9 {
        let inv_xform = session.instance_xform.inverse();
        if let Some(sym_def) = doc.symbols.find_symbol_mut(session.symbol_id) {
            sym_def.update_from_working_timeline(
                &sym_timeline,
                inv_xform,
                &mut session.stage_to_local_cache,
            );
        }
    }
    doc.symbols.bump_revision();

    doc.symbols.sync_active_edit(&doc.timeline);
    load_shapes_from_timeline(doc);

    doc.canvas
        .select_symbol_instance_by_gid(session.instance_gid);
    true
}
