use std::collections::HashSet;

use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, cancel_active_drag, clear_selection, commit_floating, edit_layers,
    editable_layer_ids, insert_and_select_shapes_in_scope, reset_session_after_timeline_change,
    topmost_layer_id_for_indices, Tab, View,
};
use crate::symbol::{EditLevel, SymbolDef, SymbolEvalCtx, SymbolInstanceBinding};
use crate::timeline::TimelineCellSelection;
use crate::vector_engine::{union_bounding_boxes, FillShape};

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

    let target_lid =
        topmost_layer_id_for_indices(&tab.session, tab.active_timeline(), &sel_scene_indices);

    let depth = tab.session.editing_group_path.len();
    let layer_ids = editable_layer_ids(tab);
    let world = tab.session.edit_path.world_xform();
    let selection = tab.session.selection.clone();
    // シンボルの中身は、文書を変える前に作る（地図にできなければ断る）。選んでいるもの：画面の土台の図形（浮いた片が
    // 隠している元の図形を除く）と、浮いた片（いつも選ばれている）。並びは、書き戻す図形の並び（土台のあとに片）と同じ
    let (pieces, hidden): (Vec<FillShape>, HashSet<u64>) = tab
        .session
        .floating()
        .map(|fl| (fl.pieces().to_vec(), fl.hidden().clone()))
        .unwrap_or_default();
    let scope = tab.session.editing_group_path.clone();
    let picked: Vec<&FillShape> = tab
        .session
        .base_cache
        .shapes()
        .iter()
        .filter(|s| layer_ids.contains(&s.layer_id) && !hidden.contains(&s.shape_id))
        .chain(pieces.iter())
        .filter(|s| selection.contains_shape(s) && s.is_in_editing_scope(&scope))
        .collect();
    let (local_shapes, center) = symbol_content(&picked, depth, world)?;
    let Ok(sym_def) = SymbolDef::new(0, String::new(), local_shapes) else {
        tab.session.off_grid = true;
        return None;
    };

    before_mutate(tab);

    let sym_id = tab.doc.alloc_symbol_id();
    let inst_gid = tab.doc.alloc_group_id();
    // 浮いた片の残りは、そのままセル画の地図にする。片（ステージの写し）は、シンボルのインスタンスの中身の印にするので
    // 図形のまま入れる（選択は、残りを書く前のもの：片も選んでいる）
    if let Some(mut rest) = tab.session.take_floating() {
        rest.drop_pieces();
        crate::canvas_tool::commit_floating_layers(tab, rest);
    }

    edit_layers(tab, &layer_ids, |edit| {
        if !pieces.is_empty() {
            let at = edit.scope_insert_index();
            edit.shapes.splice(at..at, pieces);
        }
        let valid_indices: Vec<usize> = edit
            .shapes
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| {
                (selection.contains_shape(s) && s.is_in_editing_scope(&edit.editing_group_path))
                    .then_some(idx)
            })
            .collect();
        if valid_indices.is_empty() {
            return;
        }
        // インスタンスはステージ座標で置き、書き戻しで今いるシンボルの座標（`translate(center)`）になる
        let binding = SymbolInstanceBinding::new(
            inst_gid,
            sym_id,
            world * Affine::translate(center.to_vec2()),
        );
        let insert_limit = edit.scope_insert_index();
        let mut unselected = Vec::with_capacity(edit.shapes.len());
        let mut converted_shapes = Vec::new();
        let mut trailing_outside = Vec::new();
        for (idx, mut shape) in edit.shapes.drain(..).enumerate() {
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
        edit.shapes = unselected;
    });

    let mut sym_def = sym_def;
    sym_def.id = sym_id;
    tab.doc.register_symbol(sym_def);
    tab.session.selection.clear();
    tab.session.selection.instances.insert(inst_gid);

    Some((sym_id, target_lid))
}

/// 選んでいる図形 `picked`（ステージ座標、下から順）を、新しいシンボルの中身にする：今いるシンボルの座標（`world` の逆）に
/// 戻し、その外接矩形の中心が原点になるように置いたもの（中に入ったインスタンスは逆行列を持つ：`enter_symbol_timeline`）
/// と、その中心（今いるシンボルの座標）。`depth` は今いる階層の深さ。`None`：選んでいるものが無い
fn symbol_content(
    picked: &[&FillShape],
    depth: usize,
    world: Affine,
) -> Option<(Vec<FillShape>, Point)> {
    let to_parent = world.inverse();
    let parent_shapes: Vec<FillShape> = picked
        .iter()
        .map(|s| {
            let mut local_s = (*s).clone();
            local_s.layer_id = 1;
            local_s.group_path = local_s.group_path.get(depth..).unwrap_or_default().to_vec();
            if local_s
                .symbol_instance
                .is_some_and(|b| !local_s.group_path.contains(&b.instance_gid))
            {
                local_s.symbol_instance = None;
            }
            if world != Affine::IDENTITY {
                local_s.transform(to_parent);
            }
            local_s
        })
        .collect();
    let first = parent_shapes.first()?.bounding_box();
    let center = parent_shapes[1..]
        .iter()
        .fold(first, |acc, s| acc.union(s.bounding_box()))
        .center();
    let local = parent_shapes
        .into_iter()
        .map(|mut s| {
            s.translate(-center.to_vec2());
            s
        })
        .collect();
    Some((local, center))
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
    let stage_center = View::DEFAULT_STAGE_CENTER;
    let offset = stage_center - first_bbox_center.to_vec2();
    let binding = SymbolInstanceBinding::new(
        inst_gid,
        symbol_id,
        Affine::translate(KVec2::new(offset.x, offset.y)),
    );
    let tl = tab.active_timeline();
    let target_lid = active_scope_layer_id(&tab.session, tl);
    let curr_frame = tl.current_frame;
    let kf_start = tl.keyframe_start_frame_at(target_lid, curr_frame);
    let placed = binding.expand(
        (kf_start, curr_frame),
        target_lid,
        &tab.session.editing_group_path,
        SymbolEvalCtx::new(&tab.doc.library),
    );
    insert_and_select_shapes_in_scope(tab, placed, false);
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
            } else if prev_world.determinant().abs() > 1e-9 {
                prev_world.inverse() * binding.xform
            } else {
                binding.xform
            }
        };
        (curr_f, act_l, xform)
    };

    let next_world = tab.session.edit_path.world_xform() * xform_in_parent;
    if !xform_in_parent.as_coeffs().iter().all(|v| v.is_finite())
        || xform_in_parent.determinant().abs() <= 1e-9
        || !next_world.as_coeffs().iter().all(|v| v.is_finite())
        || next_world.determinant().abs() <= 1e-9
    {
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
