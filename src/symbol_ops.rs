use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    Session, Tab, active_scope_layer_id, cancel_active_drag, clear_selection, commit_floating,
    insert_and_select_shapes_in_scope, load_shapes_from_timeline, topmost_layer_id_for_indices,
};
use crate::symbol::{EditLevel, SymbolEvalCtx, SymbolInstanceBinding};
use crate::timeline::TimelineCellSelection;
use crate::vector_engine::{FillShape, union_bounding_boxes};

/// 選択中のオブジェクトを新しい「シンボル」に変換するためのローカル図形抽出とインスタンスバインディング付与を行い、
/// `(sym_id, target_lid, local_shapes)` を返す（Undo記録・ライブラリ登録は `doc_actions::apply` が行う）
pub(crate) fn prepare_convert_selection_to_symbol(
    tab: &mut Tab,
    before_mutate: impl FnOnce(&mut Tab),
) -> Option<(u64, u64, Vec<FillShape>)> {
    cancel_active_drag(tab);
    if tab.session.selection.is_empty() {
        return None;
    }
    let sel_scene_indices = tab.session.selected_indices();
    if sel_scene_indices.is_empty()
        || !tab
            .scene()
            .selected_has_any_in_scope(&tab.session.selection, &tab.session.editing_group_path)
    {
        return None;
    }

    let target_lid =
        topmost_layer_id_for_indices(&tab.session, tab.active_timeline(), &sel_scene_indices);

    before_mutate(tab);

    let layer_ids: Vec<u64> = tab.active_timeline().layers.iter().map(|l| l.id).collect();
    tab.session.layer_order_ids = layer_ids;
    tab.session.materialize_floating_without_merge();

    let valid_indices: Vec<usize> = tab
        .session
        .shapes
        .iter()
        .enumerate()
        .filter_map(|(idx, s)| {
            (tab.session.selection.contains_shape(s)
                && s.is_in_editing_scope(&tab.session.editing_group_path))
            .then_some(idx)
        })
        .collect();
    if valid_indices.is_empty() {
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

    let binding = SymbolInstanceBinding::new(
        inst_gid,
        sym_id,
        Affine::translate(KVec2::new(center.x, center.y)),
    );
    let insert_limit = tab.session.scope_insert_index();
    let mut unselected = Vec::with_capacity(tab.session.shapes.len());
    let mut converted_shapes = Vec::new();
    let mut trailing_outside = Vec::new();
    for (idx, mut shape) in tab.session.shapes.drain(..).enumerate() {
        if valid_indices.binary_search(&idx).is_ok() {
            shape.layer_id = target_lid;
            shape.group_path.insert(depth, inst_gid);
            shape.symbol_instance = Some(binding);
            converted_shapes.push(shape);
        } else if idx < insert_limit {
            unselected.push(shape);
        } else {
            trailing_outside.push(shape);
        }
    }
    unselected.extend(converted_shapes);
    unselected.extend(trailing_outside);
    Session::sort_vec_by_layer_order(&mut unselected, tab.active_timeline());
    tab.session.shapes = unselected;
    tab.session.floating = None;
    tab.session.rebuild_overlay_from_floating();
    tab.session.selection.clear();
    tab.session.selection.instances.insert(inst_gid);

    Some((sym_id, target_lid, local_shapes))
}

/// ライブラリ内の指定シンボル `symbol_id` の新しいインスタンスを現在のステージ中央へ配置する
pub(crate) fn prepare_place_symbol_instance(
    tab: &mut Tab,
    symbol_id: u64,
    before_mutate: impl FnOnce(&mut Tab),
) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab
        .doc
        .can_place_symbol_in_current_context(&tab.session.edit_path, symbol_id)
        || !tab.active_timeline().is_layer_editable_by_id(target_lid)
    {
        return false;
    }
    let Some(first_shapes) = tab
        .doc
        .find_symbol(symbol_id)
        .map(|s| s.first_non_empty_local_shapes(&tab.doc.library))
    else {
        return false;
    };
    let first_bbox_center = if first_shapes.is_empty() {
        Point::ZERO
    } else {
        union_bounding_boxes(&first_shapes).center()
    };

    clear_selection(tab);
    before_mutate(tab);
    let inst_gid = tab.doc.alloc_group_id();
    let stage_center = tab.stage_center_world();
    let offset = stage_center - first_bbox_center.to_vec2();
    let binding = SymbolInstanceBinding::new(
        inst_gid,
        symbol_id,
        Affine::translate(KVec2::new(offset.x, offset.y)),
    );
    let tl = tab.active_timeline();
    let target_lid = active_scope_layer_id(&tab.session, tl);
    let curr_frame = tl.current_frame;
    let kf_start = tl
        .layer_index_by_id(target_lid)
        .and_then(|l_idx| {
            tl.layers[l_idx]
                .keyframe_index_at(curr_frame)
                .map(|ki| tl.layers[l_idx].keyframes[ki].start_frame)
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
    insert_and_select_shapes_in_scope(tab, placed, None);
    true
}

/// 現在選択されている図形（またはアクティブセル上の単一シンボル）について、その `(SymbolInstanceBinding, シンボル名, シンボル総フレーム数)` を返す
pub(crate) fn selected_single_symbol_info(
    tab: &Tab,
) -> Option<(SymbolInstanceBinding, String, usize)> {
    let tl = tab.active_timeline();
    let binding = tab
        .scene()
        .selected_single_symbol_binding(&tab.session.selection)
        .or_else(|| {
            let layer = tl.layers.get(tl.active_layer)?;
            if !layer.visible || layer.locked {
                return None;
            }
            let mut b = layer
                .cel_at(tl.current_frame)?
                .single_symbol_instance()?;
            b.xform = tab.session.edit_path.world_xform() * b.xform;
            Some(b)
        })?;
    let sym = tab.doc.find_symbol(binding.symbol_id)?;
    Some((binding, sym.name.clone(), sym.timeline.total_frames.max(1)))
}

/// シンボルインスタンスをダブルクリックして、そのシンボル専用の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_timeline(tab: &mut Tab, binding: SymbolInstanceBinding) -> bool {
    if tab.doc.find_symbol(binding.symbol_id).is_none() {
        return false;
    }

    commit_floating(tab);
    tab.session.selection.clear();

    let (parent_frame, parent_active_layer, xform_in_parent) = {
        let tl = tab.active_timeline();
        let curr_f = tl.current_frame;
        let act_l = tl.active_layer;
        let prev_world = tab.session.edit_path.world_xform();
        let xform = if tab.session.edit_path.is_empty() {
            binding.xform
        } else {
            let local_inst_xform = (binding.instance_gid != 0).then(|| {
                tl.layers.iter().find_map(|layer| {
                    if !layer.visible {
                        return None;
                    }
                    if let Some((_, interp_b)) = layer.interpolated_tween_binding_at(curr_f) {
                        if interp_b.instance_gid == binding.instance_gid
                            && interp_b.symbol_id == binding.symbol_id
                        {
                            return Some(interp_b.xform);
                        }
                    }
                    layer.cel_at(curr_f)?.instances.iter().find_map(|inst| {
                        (inst.binding.instance_gid == binding.instance_gid
                            && inst.binding.symbol_id == binding.symbol_id)
                            .then_some(inst.binding.xform)
                    })
                })
            }).flatten();
            if let Some(local_xf) = local_inst_xform.filter(|&local_xf| {
                (prev_world * local_xf)
                    .as_coeffs()
                    .iter()
                    .zip(binding.xform.as_coeffs())
                    .all(|(a, b)| (a - b).abs() <= 1e-6)
            }) {
                local_xf
            } else if prev_world.determinant().abs() > 1e-9 {
                prev_world.inverse() * binding.xform
            } else {
                binding.xform
            }
        };
        (curr_f, act_l, xform)
    };

    tab.view.selected_library_symbol = Some(binding.symbol_id);
    let parent_group_path = std::mem::take(&mut tab.session.editing_group_path);
    tab.session.edit_path.levels.push(EditLevel {
        instance_gid: binding.instance_gid,
        symbol_id: binding.symbol_id,
        xform_in_parent,
        parent_frame,
        parent_active_layer,
        parent_group_path,
    });

    let fps = tab.doc.root.fps;
    if let Some(sym_def) = tab.doc.find_symbol_mut(binding.symbol_id) {
        let tl = &mut sym_def.timeline;
        tl.total_frames = tl.total_frames.max(1);
        let act_l = tl.active_layer.min(tl.layers.len().saturating_sub(1));
        let act_f = tl.current_frame.min(tl.total_frames.saturating_sub(1));
        tl.active_layer = act_l;
        tl.current_frame = act_f;
        tl.fps = fps;
        tl.next_layer_number = tl.next_layer_number.max(2);
        tl.next_layer_id = tl.next_layer_id.max(2);
        tl.cell_selection = Some(TimelineCellSelection::single(act_l, act_f));
    }

    load_shapes_from_timeline(tab);
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_from_library(tab: &mut Tab, symbol_id: u64) -> bool {
    if tab
        .session
        .edit_path
        .levels
        .iter()
        .any(|lvl| lvl.symbol_id == symbol_id)
    {
        return false;
    }
    let has_instance_in_current_timeline = tab
        .session
        .shapes
        .iter()
        .any(|s| s.symbol_instance.is_some_and(|b| b.symbol_id == symbol_id))
        || tab.active_timeline().layers.iter().any(|l| {
            l.keyframes.iter().any(|kf| {
                kf.cel
                    .instances
                    .iter()
                    .any(|inst| inst.binding.symbol_id == symbol_id)
            })
        });
    if !tab.session.edit_path.is_empty() && !has_instance_in_current_timeline {
        while !tab.session.edit_path.is_empty() {
            exit_symbol_timeline(tab);
        }
    }
    let tl = tab.active_timeline();
    let curr_f = tl.current_frame;
    let world_xform = tab.session.edit_path.world_xform();
    let binding = tab
        .session
        .shapes
        .iter()
        .find_map(|s| s.symbol_instance.filter(|b| b.symbol_id == symbol_id))
        .or_else(|| {
            tl.layers.iter().find_map(|l| {
                l.cel_at(curr_f)?.instances.iter().find_map(|inst| {
                    (inst.binding.symbol_id == symbol_id).then(|| {
                        let mut b = inst.binding;
                        b.xform = world_xform * b.xform;
                        b
                    })
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
    if tab.session.edit_path.is_empty() {
        return false;
    }
    commit_floating(tab);
    tab.session.selection.clear();

    let Some(lvl) = tab.session.edit_path.levels.pop() else {
        return false;
    };
    let fps = tab.doc.root.fps;
    tab.with_active_timeline_mut(|tl, _| {
        tl.fps = fps;
        let act_l = lvl.parent_active_layer.min(tl.layers.len().saturating_sub(1));
        let act_f = lvl.parent_frame.min(tl.total_frames.saturating_sub(1));
        tl.active_layer = act_l;
        tl.current_frame = act_f;
        tl.cell_selection = Some(TimelineCellSelection::single(act_l, act_f));
    });
    tab.session.editing_group_path = lvl.parent_group_path;

    load_shapes_from_timeline(tab);

    tab.session
        .select_symbol_instance_by_gid(lvl.instance_gid);
    true
}
