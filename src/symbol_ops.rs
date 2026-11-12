use std::collections::HashSet;

use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, cancel_active_drag, clear_selection, commit_floating, write_cels,
    editable_layer_ids, finish_cel_writes, reset_session_after_timeline_change, write_cel, Tab,
    View,
};
use crate::clipboard_ops::{
    content_xform, prepare_placement, put_placements, selected_content, Content,
};
use crate::symbol::{EditLevel, SymbolDef, SymbolInstance, SymbolInstanceBinding};
use crate::timeline::{Cel, TimelineCellSelection};
use crate::vector_engine::{invertible, union_bounding_boxes};

/// 選択中のオブジェクトを新しい「シンボル」に変換するためのローカル図形抽出・インスタンスバインディング付与・ライブラリ登録を行い、
/// `(sym_id, target_lid)` を返す
pub(crate) fn prepare_convert_selection_to_symbol(
    tab: &mut Tab,
    before_mutate: impl FnOnce(&mut Tab),
) -> Option<(u64, u64)> {
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

    let layer_ids = editable_layer_ids(tab);
    let scope = tab.session.editing_group_path.clone();
    let selection = tab.session.selection.clone();
    // シンボルの中身は、文書を変える前に、選んでいるものの中身（地図とインスタンス：`selected_content`。浮いた片は
    // 確定と同じ重ね方）から作る：編集できるレイヤーの中身を1枚に重ね（`Cel::stacked`）、外接矩形の中心を原点へ寄せる
    // （`Cel::transformed`）。地図にできなければ断る。インスタンスは、中身のいちばん上のレイヤーに置く
    let mut content = selected_content(tab)?;
    content.layers.retain(|(id, _)| layer_ids.contains(id));
    let target_lid = content.layers.last()?.0;
    let center = content_center(tab, &content);
    let cels: Vec<Cel> = content.layers.into_iter().map(|(_, c)| c).collect();
    let Some(cel) =
        Cel::stacked(&cels).and_then(|c| c.transformed(Affine::translate(-center.to_vec2())))
    else {
        tab.session.off_grid = true;
        return None;
    };
    let mut sym_def = SymbolDef::from_cel(0, String::new(), cel);

    before_mutate(tab);

    let sym_id = tab.doc.alloc_symbol_id();
    let inst_gid = tab.doc.alloc_group_id();
    // 浮いた片：選んだ片はシンボルの中身に入れたので捨て、残りと選んでいない片を確定する
    if let Some(mut fl) = tab.session.take_floating() {
        let keep: HashSet<u64> = fl
            .pieces()
            .iter()
            .filter(|p| selection.contains_shape(p))
            .map(|p| p.shape_id)
            .collect();
        let rest = fl.split_off(&keep, &scope);
        crate::canvas_tool::commit_floating_layers(tab, rest);
    }
    // 選んだもの（今の階層の）を外し、インスタンスを、置くレイヤーの今の階層のいちばん上に入れる（中心に置く）
    write_cels(tab, &layer_ids, |cel| {
        cel.remove(&scope, &selection.shapes, &selection.instances)
    });
    let mut inst = Cel::default();
    inst.instances_mut().push(SymbolInstance {
        binding: SymbolInstanceBinding::new(inst_gid, sym_id, Affine::translate(center.to_vec2())),
        layer_id: target_lid,
        parent_scope: Vec::new(),
        order: 0,
    });
    let written: Vec<u64> = write_cel(tab, target_lid, |cel| {
        cel.insert_content(&scope, &inst, None)
    })
    .then_some(target_lid)
    .into_iter()
    .collect();
    finish_cel_writes(tab, &written);

    sym_def.id = sym_id;
    tab.doc.register_symbol(sym_def);
    tab.session.selection.clear();
    tab.session.selection.instances.insert(inst_gid);

    Some((sym_id, target_lid))
}

/// 中身 `content` の外接矩形の中心（今いる階層の文書の座標）：地図の辺の曲線と、インスタンスの図形（画面の図形を文書の
/// 座標へ戻したもの）の外接矩形を合わせたもの。形が何も無ければ原点
fn content_center(tab: &Tab, content: &Content) -> Point {
    let world = content.world;
    let instances: HashSet<u64> = content
        .layers
        .iter()
        .flat_map(|(_, c)| c.instances.iter().map(|i| i.binding.instance_gid))
        .collect();
    let maps = content.layers.iter().filter_map(|(_, c)| c.map_bounds());
    let insts = tab
        .session
        .base_cache
        .shapes()
        .iter()
        .filter(|s| {
            s.symbol_instance
                .is_some_and(|b| instances.contains(&b.instance_gid))
        })
        .filter_map(|s| s.to_doc(world).geometry_bounding_box());
    maps.chain(insts)
        .reduce(|a, b| a.union(b))
        .map_or(Point::ZERO, |r| r.center())
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

    // インスタンス1つの中身（ステージの座標で、ステージの真ん中に置く）を、今の階層に置く
    let offset = View::DEFAULT_STAGE_CENTER - first_bbox_center.to_vec2();
    let mut cel = Cel::default();
    cel.instances_mut().push(SymbolInstance {
        binding: SymbolInstanceBinding::new(0, symbol_id, Affine::translate(offset.to_vec2())),
        layer_id: target_lid,
        parent_scope: Vec::new(),
        order: 0,
    });
    let content = Content {
        layers: vec![(target_lid, cel)],
        world: Affine::IDENTITY,
    };
    let xform = content_xform(
        content.world,
        tab.session.edit_path.world_xform(),
        KVec2::ZERO,
    );
    let Some(placement) = prepare_placement(tab, &content, xform, false, None) else {
        return false;
    };
    clear_selection(tab);
    before_mutate(tab);
    put_placements(tab, vec![placement], |sym| sym);
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
            if !layer.is_editable_at(tl.current_frame) {
                return None;
            }
            let mut b = layer.cel_at(tl.current_frame)?.single_symbol_instance()?;
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

    let (parent_frame, parent_active_layer, xform_in_parent) = {
        let tl = tab.active_timeline();
        let curr_f = tl.current_frame;
        let act_l = tl.active_layer;
        let prev_world = tab.session.edit_path.world_xform();
        let xform = if tab.session.edit_path.is_empty() {
            binding.xform
        } else {
            let local_inst_xform = (binding.instance_gid != 0)
                .then(|| {
                    tl.layers.iter().find_map(|layer| {
                        if !layer.visible {
                            return None;
                        }
                        if let Some(interp_b) = layer.interpolated_tween_binding_at(curr_f) {
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
                })
                .flatten();
            if let Some(local_xf) = local_inst_xform.filter(|&local_xf| {
                (prev_world * local_xf)
                    .as_coeffs()
                    .iter()
                    .zip(binding.xform.as_coeffs())
                    .all(|(a, b)| (a - b).abs() <= 1e-6)
            }) {
                local_xf
            } else if invertible(prev_world) {
                prev_world.inverse() * binding.xform
            } else {
                binding.xform
            }
        };
        (curr_f, act_l, xform)
    };

    let next_world = tab.session.edit_path.world_xform() * xform_in_parent;
    if !invertible(xform_in_parent) || !invertible(next_world) {
        return false;
    }

    commit_floating(tab);
    tab.session.selection.clear();

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

    let (act_l, act_f) = {
        let tl = tab.active_timeline();
        (
            tl.active_layer.min(tl.layers.len().saturating_sub(1)),
            tl.current_frame.min(tl.total_frames.saturating_sub(1)),
        )
    };
    tab.set_active_layer(act_l);
    tab.set_active_frame(act_f);
    tab.set_active_cell_selection(Some(TimelineCellSelection::single(act_l, act_f)));

    reset_session_after_timeline_change(tab);
    true
}

/// ライブラリ内の指定シンボル `symbol_id` の内部タイムライン編集モードへ入る
pub(crate) fn enter_symbol_from_library(tab: &mut Tab, symbol_id: u64) -> bool {
    if tab.doc.find_symbol(symbol_id).is_none()
        || tab
            .session
            .edit_path
            .levels
            .iter()
            .any(|lvl| lvl.symbol_id == symbol_id)
    {
        return false;
    }
    let has_instance_in_current_timeline = tab
        .scene()
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
        .scene()
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
            let stage_center = View::DEFAULT_STAGE_CENTER;
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
    let (act_l, act_f) = {
        let tl = tab.active_timeline();
        (
            lvl.parent_active_layer
                .min(tl.layers.len().saturating_sub(1)),
            lvl.parent_frame.min(tl.total_frames.saturating_sub(1)),
        )
    };
    tab.set_active_layer(act_l);
    tab.set_active_frame(act_f);
    tab.set_active_cell_selection(Some(TimelineCellSelection::single(act_l, act_f)));
    tab.session.editing_group_path = lvl.parent_group_path;

    reset_session_after_timeline_change(tab);

    tab.session.select_symbol_instance_by_gid(lvl.instance_gid);
    true
}
