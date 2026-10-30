use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    EditorDoc, MoveSelectionDrag, active_scope_layer_id, cancel_active_drag, clear_selection,
    insert_and_select_shapes_in_scope, stop_playback, sync_timeline_layer_from_canvas_selection,
};
use crate::select_ops::delete_selected_shapes;
use crate::symbol::{SymbolDef, SymbolEvalCtx, SymbolManager};
use crate::timeline::Timeline;
use crate::vector_engine::{FillShape, next_shape_id};

/// 相対グループパスを持つシェイプ群に対し、新しいグループID・シンボルインスタンスIDを採番して現在の編集スコープ用に変換する
pub(crate) fn instantiate_relative_shapes_in_scope(
    doc: &mut EditorDoc,
    rel_shapes: &[FillShape],
    offset: KVec2,
    preserve_original_layer: bool,
) -> (Vec<FillShape>, bool) {
    let active_lid = active_scope_layer_id(doc);
    let mut gid_map: Vec<(u64, u64)> = Vec::new();
    let mut seen_instance_gids: HashSet<u64> = HashSet::new();
    let mut instantiated = Vec::with_capacity(rel_shapes.len());
    let mut any_raw = false;

    for item in rel_shapes {
        let target_lid =
            if preserve_original_layer && doc.timeline.is_layer_editable_by_id(item.layer_id) {
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
            let mut new_parent_scope = doc.canvas.editing_group_path.clone();
            for &old_gid in orig_binding.parent_scope_from_path(&item.group_path) {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = doc.symbols.alloc_group_id();
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
                let alloc = doc.symbols.alloc_group_id();
                gid_map.push((orig_binding.instance_gid, alloc));
                alloc
            };

            let mut new_binding = orig_binding;
            new_binding.instance_gid = mapped_inst_gid;
            if offset != KVec2::ZERO {
                new_binding.xform = Affine::translate(offset) * new_binding.xform;
            }

            if let Some(sym) = doc.symbols.find_symbol(new_binding.symbol_id) {
                let current_frame = doc.timeline.current_frame;
                let kf_start_frame = doc
                    .timeline
                    .layer_index_by_id(target_lid)
                    .and_then(|l_idx| {
                        let layer = &doc.timeline.layers[l_idx];
                        layer
                            .keyframe_index_at(current_frame)
                            .map(|kf_idx| layer.keyframes[kf_idx].start_frame)
                    })
                    .unwrap_or(current_frame);
                let local_shapes = sym.evaluated_local_shapes_for_instance(
                    new_binding,
                    kf_start_frame,
                    current_frame,
                    SymbolEvalCtx::new(&doc.symbols.library),
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

        let mut new_path = doc.canvas.editing_group_path.clone();
        if item.group_path.is_empty() {
            any_raw = true;
        } else {
            for &old_gid in &item.group_path {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = doc.symbols.alloc_group_id();
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

/// 現在選択中のオブジェクト（生シェイプ・グループ・シンボル）をその場に複製し、
/// 複製側を選択状態に切り替えた上でドラッグ移動用の `undo_snapshot` を返す（`Alt + ドラッグ` 複製用）
pub(crate) fn duplicate_selected_shapes_for_alt_drag(doc: &mut EditorDoc) -> Vec<FillShape> {
    let rel_shapes = doc.canvas.extract_selected_relative_to_scope();
    if rel_shapes.is_empty() {
        return doc.canvas.snapshot_for_drag();
    }
    let saved_pivot = doc.canvas.transform_pivot;
    clear_selection(doc);
    let undo_snapshot = doc.canvas.snapshot_for_drag();
    let (duplicated, _) = instantiate_relative_shapes_in_scope(doc, &rel_shapes, KVec2::ZERO, true);
    insert_and_select_shapes_in_scope(doc, duplicated, false);
    doc.canvas.transform_pivot = saved_pivot;
    sync_timeline_layer_from_canvas_selection(doc);
    undo_snapshot
}

/// 選択オブジェクトの移動ドラッグ（`Alt` 複製移動を含む）の初期状態を構築する共通ヘルパー
pub(crate) fn begin_move_selection_drag(
    doc: &mut EditorDoc,
    pt: Point,
    alt_held: bool,
) -> MoveSelectionDrag {
    if alt_held {
        let orig_selected_ids = doc.canvas.selected_shape_ids();
        let undo_snapshot = duplicate_selected_shapes_for_alt_drag(doc);
        MoveSelectionDrag::new(pt, undo_snapshot).with_orig_selected_ids(orig_selected_ids)
    } else {
        let undo_snapshot = doc.canvas.snapshot_for_drag();
        MoveSelectionDrag::new(pt, undo_snapshot)
    }
}

fn collect_symbol_and_descendants(
    sym_id: u64,
    library: &[SymbolDef],
    reusable: &[Arc<SymbolDef>],
    visited: &mut Vec<u64>,
    out: &mut Vec<Arc<SymbolDef>>,
) {
    if visited.contains(&sym_id) {
        return;
    }
    visited.push(sym_id);
    let Some(sym) = library.iter().find(|s| s.id == sym_id) else {
        return;
    };
    for layer in &sym.timeline.layers {
        for kf in &layer.keyframes {
            for inst in kf.cel.instances.iter() {
                collect_symbol_and_descendants(
                    inst.binding.symbol_id,
                    library,
                    reusable,
                    visited,
                    out,
                );
            }
        }
    }
    if let Some(existing) = reusable.iter().find(|s| s.id == sym_id) {
        out.push(Arc::clone(existing));
    } else {
        let mut cloned = sym.clone();
        cloned.strip_transient_caches();
        out.push(Arc::new(cloned));
    }
}

/// シンボルの内部タイムライン内で使用されている最大のグループID（`group_path` / `instance_gid` / `parent_scope`）を返す
pub(crate) fn max_group_id_in_timeline(tl: &Timeline) -> u64 {
    let mut max_gid = 0u64;
    for layer in &tl.layers {
        for kf in &layer.keyframes {
            for sh in kf.cel.shapes.iter() {
                if let Some(&gid) = sh.group_path.iter().max() {
                    max_gid = max_gid.max(gid);
                }
            }
            for inst in kf.cel.instances.iter() {
                max_gid = max_gid.max(inst.binding.instance_gid);
                if let Some(&gid) = inst.parent_scope.iter().max() {
                    max_gid = max_gid.max(gid);
                }
            }
        }
    }
    max_gid
}

/// 別タブから移植されるシンボルの内部タイムライン内の全グループIDを貼り付け先タブの `SymbolManager` で新規採番したIDへ一括リマップする
fn remap_timeline_group_ids(tl: &mut Timeline, symbols: &mut SymbolManager) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    let mut map_gid = |old_gid: u64, symbols: &mut SymbolManager| -> u64 {
        *gid_map
            .entry(old_gid)
            .or_insert_with(|| symbols.alloc_group_id())
    };
    for layer in &mut tl.layers {
        for kf in &mut layer.keyframes {
            if kf
                .cel
                .shapes
                .iter()
                .any(|s| !s.group_path.is_empty() || s.symbol_instance.is_some())
            {
                for sh in kf.cel.shapes_mut() {
                    for gid in &mut sh.group_path {
                        *gid = map_gid(*gid, symbols);
                    }
                    if let Some(mut b) = sh.symbol_instance {
                        b.instance_gid = map_gid(b.instance_gid, symbols);
                        sh.symbol_instance = Some(b);
                    }
                }
            }
            if !kf.cel.instances.is_empty() {
                for inst in kf.cel.instances_mut() {
                    inst.binding.instance_gid = map_gid(inst.binding.instance_gid, symbols);
                    for gid in &mut inst.parent_scope {
                        *gid = map_gid(*gid, symbols);
                    }
                }
            }
        }
    }
}

/// `copy_selected_shapes`（`Cmd+C`）実行時に、クリップボード内の図形が参照しているシンボル定義（入れ子含む）を `Arc` 共有スナップショットとして退避する
pub(crate) fn capture_clipboard_symbols(doc: &mut EditorDoc) {
    let active_doc_id = doc.doc_id;
    if active_doc_id != doc.canvas.clipboard_source_doc_id {
        doc.canvas.clipboard_imported_map.clear();
    }
    let reusable = if active_doc_id == doc.canvas.clipboard_source_doc_id
        && doc.symbols.revision == doc.canvas.clipboard_source_symbols_revision
    {
        std::mem::take(&mut doc.canvas.clipboard_symbols)
    } else {
        doc.canvas.clipboard_symbols.clear();
        Vec::new()
    };

    doc.canvas.clipboard_source_doc_id = active_doc_id;
    doc.canvas.clipboard_source_symbol_id = doc.symbols.edit_stack.last().map(|s| s.symbol_id);
    doc.canvas.clipboard_source_symbols_revision = doc.symbols.revision;

    let mut visited = Vec::new();
    let mut collected = Vec::new();
    for shape in &doc.canvas.clipboard {
        if let Some(b) = shape.symbol_instance {
            collect_symbol_and_descendants(
                b.symbol_id,
                &doc.symbols.library,
                &reusable,
                &mut visited,
                &mut collected,
            );
        }
    }
    doc.canvas.clipboard_symbols = collected;
}

fn existing_reusable_target_symbol_id(doc: &EditorDoc, src_sym_id: u64) -> Option<u64> {
    let target_doc_id = doc.doc_id;
    let source_doc_id = doc.canvas.clipboard_source_doc_id;
    let candidate = (target_doc_id == source_doc_id)
        .then_some(src_sym_id)
        .filter(|&id| doc.symbols.find_symbol(id).is_some())
        .or_else(|| {
            doc.canvas
                .clipboard_imported_map
                .get(&(target_doc_id, src_sym_id))
                .copied()
                .filter(|&id| doc.symbols.find_symbol(id).is_some())
        })?;

    let sym_def = doc.symbols.find_symbol(candidate)?;
    let all_children_exist = sym_def.timeline.layers.iter().all(|l| {
        l.keyframes.iter().all(|kf| {
            kf.cel
                .instances
                .iter()
                .all(|inst| doc.symbols.find_symbol(inst.binding.symbol_id).is_some())
        })
    });
    all_children_exist.then_some(candidate)
}

fn can_paste_clipboard_symbol_inner(
    doc: &EditorDoc,
    src_sym_id: u64,
    visited: &mut Vec<u64>,
) -> bool {
    if doc.symbols.edit_stack.is_empty() {
        return true;
    }
    if visited.contains(&src_sym_id) {
        return true;
    }
    visited.push(src_sym_id);

    if let Some(live_id) = existing_reusable_target_symbol_id(doc, src_sym_id) {
        return doc.symbols.can_place_symbol_in_current_context(live_id);
    }

    if doc.doc_id == doc.canvas.clipboard_source_doc_id
        && doc
            .symbols
            .edit_stack
            .iter()
            .any(|sess| sess.symbol_id == src_sym_id)
    {
        return false;
    }

    let Some(src_sym) = doc
        .canvas
        .clipboard_symbols
        .iter()
        .find(|s| s.id == src_sym_id)
    else {
        return true;
    };

    for layer in &src_sym.timeline.layers {
        for kf in &layer.keyframes {
            for inst in kf.cel.instances.iter() {
                if !can_paste_clipboard_symbol_inner(doc, inst.binding.symbol_id, visited) {
                    return false;
                }
            }
        }
    }
    true
}

/// クリップボード内のシンボル `src_sym_id` が現在の編集コンテキストに対して循環参照を起こさずに配置可能かを判定する
pub(crate) fn can_paste_clipboard_symbol(doc: &EditorDoc, src_sym_id: u64) -> bool {
    let mut visited = Vec::new();
    can_paste_clipboard_symbol_inner(doc, src_sym_id, &mut visited)
}

/// `paste_clipboard`（`Cmd+V`）実行前に、現在のタブでクリップボード内の図形がすべて自己・親シンボルの循環参照により配置不可能かを判定する
pub(crate) fn would_clipboard_paste_all_cycle(doc: &EditorDoc) -> bool {
    if doc.canvas.clipboard.is_empty() || doc.symbols.edit_stack.is_empty() {
        return false;
    }
    doc.canvas.clipboard.iter().all(|shape| {
        shape
            .symbol_instance
            .is_some_and(|b| !can_paste_clipboard_symbol(doc, b.symbol_id))
    })
}

fn collect_needed_clipboard_imports(
    doc: &EditorDoc,
    src_sym_id: u64,
    local_sym_map: &mut HashMap<u64, u64>,
    needed_imports: &mut HashSet<u64>,
    visited: &mut HashSet<u64>,
) {
    if !visited.insert(src_sym_id) {
        return;
    }
    if let Some(valid_id) = existing_reusable_target_symbol_id(doc, src_sym_id) {
        local_sym_map.insert(src_sym_id, valid_id);
        return;
    }
    needed_imports.insert(src_sym_id);
    if let Some(src_sym) = doc
        .canvas
        .clipboard_symbols
        .iter()
        .find(|s| s.id == src_sym_id)
    {
        for layer in &src_sym.timeline.layers {
            for kf in &layer.keyframes {
                for inst in kf.cel.instances.iter() {
                    collect_needed_clipboard_imports(
                        doc,
                        inst.binding.symbol_id,
                        local_sym_map,
                        needed_imports,
                        visited,
                    );
                }
            }
        }
    }
}

/// `paste_clipboard`（`Cmd+V`）実行時に、クリップボードの不変スナップショットを破壊せず、
/// 現在のタブのライブラリに不足しているシンボル定義のみを自動移植してペースト用 `(Vec<FillShape>, had_cycle)` を返す
pub(crate) fn resolve_clipboard_for_paste(doc: &mut EditorDoc) -> (Vec<FillShape>, bool) {
    let mut had_cycle = false;
    let mut resolved_shapes = Vec::with_capacity(doc.canvas.clipboard.len());
    for shape in &doc.canvas.clipboard {
        if let Some(b) = shape.symbol_instance {
            if !can_paste_clipboard_symbol(doc, b.symbol_id) {
                had_cycle = true;
                continue;
            }
        }
        resolved_shapes.push(shape.clone());
    }

    if resolved_shapes.is_empty() || doc.canvas.clipboard_symbols.is_empty() {
        return (resolved_shapes, had_cycle);
    }

    let target_doc_id = doc.doc_id;
    let source_doc_id = doc.canvas.clipboard_source_doc_id;
    let mut local_sym_map: HashMap<u64, u64> = HashMap::new();
    let mut needed_imports: HashSet<u64> = HashSet::new();
    let mut visited: HashSet<u64> = HashSet::new();

    for shape in &resolved_shapes {
        if let Some(b) = shape.symbol_instance {
            collect_needed_clipboard_imports(
                doc,
                b.symbol_id,
                &mut local_sym_map,
                &mut needed_imports,
                &mut visited,
            );
        }
    }

    let mut any_imported = false;

    for idx in 0..doc.canvas.clipboard_symbols.len() {
        let src_sym_id = doc.canvas.clipboard_symbols[idx].id;
        if !needed_imports.contains(&src_sym_id) {
            continue;
        }
        let mut imported = (*doc.canvas.clipboard_symbols[idx]).clone();

        let new_id =
            if target_doc_id == source_doc_id && doc.symbols.find_symbol(imported.id).is_none() {
                doc.symbols.next_symbol_id = doc
                    .symbols
                    .next_symbol_id
                    .max(imported.id.saturating_add(1));
                imported.id
            } else {
                let id = doc.symbols.next_symbol_id;
                doc.symbols.next_symbol_id = doc.symbols.next_symbol_id.saturating_add(1);
                id
            };

        local_sym_map.insert(imported.id, new_id);
        doc.canvas
            .clipboard_imported_map
            .insert((target_doc_id, imported.id), new_id);

        imported.id = new_id;
        if doc.symbols.library.iter().any(|s| s.name == imported.name) {
            let base_name = imported.name.clone();
            let mut suffix = 2usize;
            loop {
                let candidate = format!("{base_name} ({suffix})");
                if !doc.symbols.library.iter().any(|s| s.name == candidate) {
                    imported.name = candidate;
                    break;
                }
                suffix = suffix.saturating_add(1);
            }
        }
        for layer in &mut imported.timeline.layers {
            for kf in &mut layer.keyframes {
                for inst in kf.cel.instances_mut() {
                    if let Some(&mapped_child_id) = local_sym_map.get(&inst.binding.symbol_id) {
                        inst.binding.symbol_id = mapped_child_id;
                    }
                }
            }
        }
        remap_timeline_group_ids(&mut imported.timeline, &mut doc.symbols);
        let max_internal_gid = max_group_id_in_timeline(&imported.timeline);
        doc.symbols.next_group_id = doc
            .symbols
            .next_group_id
            .max(max_internal_gid.saturating_add(1));

        doc.symbols.library.push(imported);
        if doc.symbols.selected_library_symbol.is_none() {
            doc.symbols.selected_library_symbol = Some(new_id);
        }
        any_imported = true;
    }

    if any_imported {
        doc.symbols.bump_revision();
    }

    for shape in &mut resolved_shapes {
        if let Some(mut b) = shape.symbol_instance {
            if let Some(&mapped_id) = local_sym_map.get(&b.symbol_id) {
                b.symbol_id = mapped_id;
                shape.symbol_instance = Some(b);
            }
        }
    }
    (resolved_shapes, had_cycle)
}

/// 選択中のオブジェクト（生シェイプ・部分スライス領域・グループ問わず）を内部クリップボードにコピーする（`Cmd+C`）
pub(crate) fn copy_selected_shapes(doc: &mut EditorDoc) -> bool {
    stop_playback(doc);
    cancel_active_drag(doc);
    let mut extracted = doc.canvas.extract_selected_relative_to_scope();
    if extracted.is_empty() {
        return false;
    }
    for s in &mut extracted {
        s.strip_transient_caches();
    }
    doc.canvas.clipboard = extracted;
    doc.canvas.paste_count = 0;
    doc.canvas.paste_offset = KVec2::ZERO;
    capture_clipboard_symbols(doc);
    true
}

/// 選択中のオブジェクトを内部クリップボードにコピーした上でキャンバスから削除する（`Cmd+X`）
pub(crate) fn cut_selected_shapes(doc: &mut EditorDoc) -> bool {
    if !copy_selected_shapes(doc) {
        return false;
    }
    delete_selected_shapes(doc)
}

/// 内部クリップボードのオブジェクトを現在の編集スコープへ貼り付ける。
pub(crate) fn paste_clipboard(doc: &mut EditorDoc, in_place: bool) -> bool {
    stop_playback(doc);
    if doc.canvas.clipboard.is_empty() {
        return false;
    }
    let target_lid = active_scope_layer_id(doc);
    if !doc.timeline.is_layer_editable_by_id(target_lid) {
        doc.notify_warning("現在のレイヤーまたはフレームは編集できないためペーストできません");
        return false;
    }
    if would_clipboard_paste_all_cycle(doc) {
        doc.notify_warning("編集中のシンボル自身や親シンボルは循環参照になるため配置できません");
        return false;
    }

    doc.mutate_canvas_with_undo(true, |doc| {
        let (resolved, had_cycle) = resolve_clipboard_for_paste(doc);
        if had_cycle {
            doc.notify_warning(
                "編集中のシンボル自身や親シンボルは循環参照になるため配置できません",
            );
        }
        if resolved.is_empty() {
            return;
        }

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
    });
    true
}
