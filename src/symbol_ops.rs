use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    Tab, active_scope_layer_id, cancel_active_drag, clear_selection,
    flush_canvas_before_timeline_op, insert_and_select_shapes_in_scope, load_shapes_from_timeline,
    reorder_selected_to_scope_front, topmost_layer_id_for_indices,
};
use crate::symbol::{
    SymbolEditSession, SymbolEvalCtx, SymbolInstanceBinding, SymbolLoopMode,
    resolve_cel_with_library, transform_layers_with_local_cache,
};
use crate::timeline::Cel;
use crate::vector_engine::{FillShape, union_bounding_boxes};

/// 選択中のオブジェクトを新しい「シンボル」に変換するためのローカル図形抽出とインスタンスバインディング付与を行い、
/// `(sym_id, target_lid, local_shapes, undo_snapshot)` を返す（Undo記録・ライブラリ登録は `doc_actions::apply` が行う）
pub(crate) fn prepare_convert_selection_to_symbol(
    tab: &mut Tab,
) -> Option<(u64, u64, Vec<FillShape>, Vec<FillShape>)> {
    cancel_active_drag(tab);
    if tab.session.selection.is_empty() {
        return None;
    }
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();
    let valid_indices: Vec<usize> = tab
        .session
        .selected_indices()
        .into_iter()
        .filter(|&idx| {
            tab.session
                .shapes
                .get(idx)
                .is_some_and(|s| s.is_in_editing_scope(&tab.session.editing_group_path))
        })
        .collect();
    if valid_indices.is_empty() {
        tab.session.restore_unsliced_snapshot(snapshot);
        return None;
    }

    let mut bbox = tab.session.shapes[valid_indices[0]].bounding_box();
    for &idx in &valid_indices[1..] {
        bbox = bbox.union(tab.session.shapes[idx].bounding_box());
    }
    let center = Point::new((bbox.x0 + bbox.x1) * 0.5, (bbox.y0 + bbox.y1) * 0.5);

    let sym_id = tab.doc.next_symbol_id;
    let inst_gid = tab.doc.alloc_group_id();

    let depth = tab.session.editing_group_path.len();
    let to_local = KVec2::new(-center.x, -center.y);
    let mut local_shapes = Vec::with_capacity(valid_indices.len());
    for &idx in &valid_indices {
        let mut local_s = tab.session.shapes[idx].clone();
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

    let target_lid = topmost_layer_id_for_indices(&tab.session, &tab.doc.root, &valid_indices);
    let binding = SymbolInstanceBinding::new(
        inst_gid,
        sym_id,
        Affine::translate(KVec2::new(center.x, center.y)),
    );
    for &idx in &valid_indices {
        if let Some(shape) = tab.session.shapes.get_mut(idx) {
            shape.layer_id = target_lid;
            shape.group_path.insert(depth, inst_gid);
            shape.symbol_instance = Some(binding);
        }
    }
    tab.session.selection.clear();
    tab.session.selection.instances.insert(inst_gid);

    reorder_selected_to_scope_front(tab);
    Some((sym_id, target_lid, local_shapes, snapshot))
}

/// ライブラリ内の指定シンボル `symbol_id` の新しいインスタンスを現在のステージ中央へ配置し、変更前スナップショットを返す
pub(crate) fn prepare_place_symbol_instance(
    tab: &mut Tab,
    symbol_id: u64,
) -> Option<Vec<FillShape>> {
    let target_lid = active_scope_layer_id(&tab.session, &tab.doc.root);
    if !tab
        .doc
        .can_place_symbol_in_current_context(&tab.session.edit_stack, symbol_id)
        || !tab.doc.root.is_layer_editable_by_id(target_lid)
    {
        return None;
    }
    let first_shapes = tab
        .doc
        .find_symbol(symbol_id)
        .map(|s| s.first_non_empty_local_shapes(&tab.doc.library))?;
    let first_bbox_center = if first_shapes.is_empty() {
        Point::ZERO
    } else {
        union_bounding_boxes(&first_shapes).center()
    };

    clear_selection(tab);
    let snapshot = tab.session.take_or_clone_unsliced_snapshot();
    let inst_gid = tab.doc.alloc_group_id();
    let stage_center = tab.stage_center_world();
    let offset = stage_center - first_bbox_center.to_vec2();
    let binding = SymbolInstanceBinding::new(
        inst_gid,
        symbol_id,
        Affine::translate(KVec2::new(offset.x, offset.y)),
    );
    let target_lid = active_scope_layer_id(&tab.session, &tab.doc.root);
    let curr_frame = tab.doc.root.current_frame;
    let kf_start = tab
        .doc
        .root
        .layer_index_by_id(target_lid)
        .and_then(|l_idx| {
            tab.doc.root.layers[l_idx]
                .keyframe_index_at(curr_frame)
                .map(|ki| tab.doc.root.layers[l_idx].keyframes[ki].start_frame)
        })
        .unwrap_or(curr_frame);
    let sym_shapes = tab
        .doc
        .find_symbol(symbol_id)
        .map(|s| {
            s.evaluated_local_shapes_for_instance(
                binding,
                kf_start,
                curr_frame,
                SymbolEvalCtx::new(&tab.doc.library),
            )
        })
        .unwrap_or_default();

    let placed = if !sym_shapes.is_empty() {
        binding.instantiate_shapes(&sym_shapes, target_lid, &tab.session.editing_group_path)
    } else {
        vec![FillShape::new_empty_symbol_marker(
            binding,
            target_lid,
            &tab.session.editing_group_path,
        )]
    };
    insert_and_select_shapes_in_scope(tab, placed, false);
    Some(snapshot)
}

/// 現在選択されている図形（またはアクティブセル上の単一シンボル）について、その `(SymbolInstanceBinding, シンボル名, シンボル総フレーム数)` を返す
pub(crate) fn selected_single_symbol_info(
    tab: &Tab,
) -> Option<(SymbolInstanceBinding, String, usize)> {
    let binding = tab
        .scene
        .selected_single_symbol_binding(&tab.session.selection)
        .or_else(|| {
            let layer = tab.doc.root.layers.get(tab.doc.root.active_layer)?;
            if !layer.visible || layer.locked {
                return None;
            }
            layer
                .cel_at(tab.doc.root.current_frame)?
                .single_symbol_instance()
        })?;
    let sym = tab.doc.find_symbol(binding.symbol_id)?;
    Some((binding, sym.name.clone(), sym.timeline.total_frames.max(1)))
}

/// シンボルインスタンスをダブルクリックして、そのシンボル専用の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_timeline(tab: &mut Tab, binding: SymbolInstanceBinding) -> bool {
    let Some(sym_def) = tab.doc.find_symbol(binding.symbol_id).cloned() else {
        return false;
    };

    flush_canvas_before_timeline_op(tab);
    tab.session.selection.clear();

    let mut backdrop_shapes: Vec<FillShape> = tab
        .session
        .edit_stack
        .last()
        .map(|s| {
            resolve_cel_with_library(
                &s.backdrop_cel,
                s.parent_timeline.current_frame,
                s.parent_timeline.current_frame,
                SymbolEvalCtx::new(&tab.doc.library),
            )
        })
        .unwrap_or_default();
    let curr_f = tab.doc.root.current_frame;
    for layer_idx in 0..tab.doc.root.layers.len() {
        let layer = &tab.doc.root.layers[layer_idx];
        if !layer.visible {
            continue;
        }
        let kf_start = layer
            .keyframe_index_at(curr_f)
            .map(|ki| layer.keyframes[ki].start_frame)
            .unwrap_or(curr_f);
        let resolved =
            tab.doc
                .root
                .evaluate_layer_shapes_at(layer_idx, curr_f, &tab.doc.library);
        for mut s in resolved {
            if s.symbol_instance
                .is_some_and(|b| b.instance_gid == binding.instance_gid)
            {
                continue;
            }
            if let Some(mut b) = s.symbol_instance {
                if let Some(sym) = tab.doc.find_symbol(b.symbol_id) {
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
    let sym_timeline = tab.doc.root.new_for_symbol_edit(&sym_def, sym_layers);
    tab.view.selected_library_symbol = Some(sym_def.id);

    let parent_timeline = std::mem::replace(&mut tab.doc.root, sym_timeline);
    let parent_editing_group_path = std::mem::take(&mut tab.session.editing_group_path);
    tab.session.edit_stack.push(SymbolEditSession {
        symbol_id: binding.symbol_id,
        instance_gid: binding.instance_gid,
        instance_xform: binding.xform,
        parent_timeline,
        parent_editing_group_path,
        backdrop_cel: Cel::new(backdrop_shapes),
        stage_to_local_cache,
    });
    tab.doc.bump_revision();

    load_shapes_from_timeline(tab);
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_from_library(tab: &mut Tab, symbol_id: u64) -> bool {
    if tab
        .session
        .edit_stack
        .iter()
        .any(|sess| sess.symbol_id == symbol_id)
    {
        return false;
    }
    let has_instance_in_current_timeline = tab
        .session
        .shapes
        .iter()
        .any(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == symbol_id))
        || tab.doc.root.layers.iter().any(|l| {
            l.keyframes.iter().any(|kf| {
                kf.cel
                    .instances
                    .iter()
                    .any(|inst| inst.binding.symbol_id == symbol_id)
            })
        });
    if !tab.session.edit_stack.is_empty() && !has_instance_in_current_timeline {
        while !tab.session.edit_stack.is_empty() {
            exit_symbol_timeline(tab);
        }
    }
    let curr_f = tab.doc.root.current_frame;
    let binding = tab
        .session
        .shapes
        .iter()
        .find_map(|s| s.symbol_instance.filter(|b| b.symbol_id == symbol_id))
        .or_else(|| {
            tab.doc.root.layers.iter().find_map(|l| {
                l.cel_at(curr_f)?.instances.iter().find_map(|inst| {
                    (inst.binding.symbol_id == symbol_id).then_some(inst.binding)
                })
            })
        })
        .unwrap_or_else(|| {
            let stage_center = tab.stage_center_world();
            SymbolInstanceBinding::new(
                0,
                symbol_id,
                Affine::translate(KVec2::new(stage_center.x, stage_center.y)),
            )
        });
    enter_symbol_timeline(tab, binding)
}

/// 現在入っているシンボル内部タイムライン編集モードから1段階上の親シーン（または親シンボル）タイムラインへ戻る
pub(crate) fn exit_symbol_timeline(tab: &mut Tab) -> bool {
    if tab.session.edit_stack.is_empty() {
        return false;
    }
    flush_canvas_before_timeline_op(tab);
    tab.session.selection.clear();

    let Some(mut session) = tab.session.edit_stack.pop() else {
        return false;
    };
    session.parent_timeline.fps = tab.doc.root.fps;
    let sym_timeline = std::mem::replace(&mut tab.doc.root, session.parent_timeline);
    tab.session.editing_group_path = session.parent_editing_group_path;

    if session.instance_xform.determinant().abs() > 1e-9 {
        let inv_xform = session.instance_xform.inverse();
        if let Some(sym_def) = tab.doc.find_symbol_mut(session.symbol_id) {
            sym_def.update_from_working_timeline(
                &sym_timeline,
                inv_xform,
                &mut session.stage_to_local_cache,
            );
        }
    }
    tab.doc.bump_revision();

    tab.doc.sync_active_edit(&mut tab.session.edit_stack);
    load_shapes_from_timeline(tab);

    tab.session
        .select_symbol_instance_by_gid(session.instance_gid);
    true
}
