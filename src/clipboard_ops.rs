use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    Clipboard, MoveSelectionDrag, Tab, active_scope_layer_id, cancel_active_drag,
};
use crate::symbol::{Document, SymbolDef, SymbolEvalCtx};
use crate::timeline::Timeline;
use crate::vector_engine::{FillShape, next_shape_id};

/// 相対グループパスを持つシェイプ群に対し、新しいグループID・シンボルインスタンスIDを採番して現在の編集スコープ用に変換する
pub(crate) fn instantiate_relative_shapes_in_scope(
    tab: &mut Tab,
    rel_shapes: &[FillShape],
    offset: KVec2,
    preserve_original_layer: bool,
) -> (Vec<FillShape>, bool) {
    let active_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    let mut gid_map: Vec<(u64, u64)> = Vec::new();
    let mut seen_instance_gids: HashSet<u64> = HashSet::new();
    let mut instantiated = Vec::with_capacity(rel_shapes.len());
    let mut any_raw = false;

    for item in rel_shapes {
        let target_lid = if preserve_original_layer
            && tab.active_timeline().is_layer_editable_by_id(item.layer_id)
        {
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
            let mut new_parent_scope = tab.session.editing_group_path.clone();
            for &old_gid in orig_binding.parent_scope_from_path(&item.group_path) {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = tab.doc.alloc_group_id();
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
                let alloc = tab.doc.alloc_group_id();
                gid_map.push((orig_binding.instance_gid, alloc));
                alloc
            };

            let mut new_binding = orig_binding;
            new_binding.instance_gid = mapped_inst_gid;
            if offset != KVec2::ZERO {
                new_binding.xform = Affine::translate(offset) * new_binding.xform;
            }

            let tl = tab.active_timeline();
            let current_frame = tl.current_frame;
            let kf_start_frame = tl
                .layer_index_by_id(target_lid)
                .and_then(|l_idx| {
                    let layer = &tl.layers[l_idx];
                    layer
                        .keyframe_index_at(current_frame)
                        .map(|kf_idx| layer.keyframes[kf_idx].start_frame)
                })
                .unwrap_or(current_frame);
            if let Some(sym) = tab.doc.find_symbol(new_binding.symbol_id) {
                let local_shapes = sym.evaluated_local_shapes_for_instance(
                    new_binding,
                    kf_start_frame,
                    current_frame,
                    SymbolEvalCtx::new(&tab.doc.library),
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

        let mut new_path = tab.session.editing_group_path.clone();
        if item.group_path.is_empty() {
            any_raw = true;
        } else {
            for &old_gid in &item.group_path {
                let new_gid = if let Some(&(_, mapped)) =
                    gid_map.iter().find(|&&(orig, _)| orig == old_gid)
                {
                    mapped
                } else {
                    let alloc = tab.doc.alloc_group_id();
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

/// 選択オブジェクトの移動ドラッグ（`Alt` 複製移動を含む）の初期状態を構築する共通ヘルパー
pub(crate) fn begin_move_selection_drag(
    tab: &mut Tab,
    pt: Point,
    alt_held: bool,
) -> MoveSelectionDrag {
    tab.session.begin_move_overlay(alt_held);
    MoveSelectionDrag::new(pt, alt_held)
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

/// 別タブから移植されるシンボルの内部タイムライン内の全グループIDを貼り付け先タブの `Document` で新規採番したIDへ一括リマップする
fn remap_timeline_group_ids(tl: &mut Timeline, doc: &mut Document) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    let mut map_gid = |old_gid: u64, doc: &mut Document| -> u64 {
        *gid_map
            .entry(old_gid)
            .or_insert_with(|| doc.alloc_group_id())
    };
    for layer in &mut tl.layers {
        for i in 0..layer.keyframes.len() {
            if layer.keyframes[i]
                .cel
                .shapes
                .iter()
                .any(|s| !s.group_path.is_empty() || s.symbol_instance.is_some())
            {
                for sh in layer.keyframe_mut(i).cel.shapes_mut() {
                    for gid in &mut sh.group_path {
                        *gid = map_gid(*gid, doc);
                    }
                    if let Some(mut b) = sh.symbol_instance {
                        b.instance_gid = map_gid(b.instance_gid, doc);
                        sh.symbol_instance = Some(b);
                    }
                }
            }
            if !layer.keyframes[i].cel.instances.is_empty() {
                for inst in layer.keyframe_mut(i).cel.instances_mut() {
                    inst.binding.instance_gid = map_gid(inst.binding.instance_gid, doc);
                    for gid in &mut inst.parent_scope {
                        *gid = map_gid(*gid, doc);
                    }
                }
            }
        }
    }
}

/// `copy_selected_shapes`（`Cmd+C`）実行時に、クリップボード内の図形が参照しているシンボル定義（入れ子含む）を `Arc` 共有スナップショットとして退避する
pub(crate) fn capture_clipboard_symbols(tab: &Tab, clipboard: &mut Clipboard) {
    let active_doc_id = tab.id;
    if active_doc_id != clipboard.source_doc_id {
        clipboard.imported_map.clear();
    }
    let reusable = if active_doc_id == clipboard.source_doc_id
        && tab.doc.revision == clipboard.source_symbols_revision
    {
        std::mem::take(&mut clipboard.symbols)
    } else {
        clipboard.symbols.clear();
        Vec::new()
    };

    clipboard.source_doc_id = active_doc_id;
    clipboard.source_symbol_id = tab.session.edit_path.last().map(|s| s.symbol_id);
    clipboard.source_symbols_revision = tab.doc.revision;

    let mut visited = Vec::new();
    let mut collected = Vec::new();
    for shape in &clipboard.shapes {
        if let Some(b) = shape.symbol_instance {
            collect_symbol_and_descendants(
                b.symbol_id,
                &tab.doc.library,
                &reusable,
                &mut visited,
                &mut collected,
            );
        }
    }
    clipboard.symbols = collected;
}

fn existing_reusable_target_symbol_id(
    tab: &Tab,
    clipboard: &Clipboard,
    src_sym_id: u64,
) -> Option<u64> {
    let target_doc_id = tab.id;
    let source_doc_id = clipboard.source_doc_id;
    let candidate = (target_doc_id == source_doc_id)
        .then_some(src_sym_id)
        .filter(|&id| tab.doc.find_symbol(id).is_some())
        .or_else(|| {
            clipboard
                .imported_map
                .get(&(target_doc_id, src_sym_id))
                .copied()
                .filter(|&id| tab.doc.find_symbol(id).is_some())
        })?;

    let sym_def = tab.doc.find_symbol(candidate)?;
    let all_children_exist = sym_def.timeline.layers.iter().all(|l| {
        l.keyframes.iter().all(|kf| {
            kf.cel
                .instances
                .iter()
                .all(|inst| tab.doc.find_symbol(inst.binding.symbol_id).is_some())
        })
    });
    all_children_exist.then_some(candidate)
}

fn can_paste_clipboard_symbol_inner(
    tab: &Tab,
    clipboard: &Clipboard,
    src_sym_id: u64,
    visited: &mut Vec<u64>,
) -> bool {
    if tab.session.edit_path.is_empty() {
        return true;
    }
    if visited.contains(&src_sym_id) {
        return true;
    }
    visited.push(src_sym_id);

    if let Some(live_id) = existing_reusable_target_symbol_id(tab, clipboard, src_sym_id) {
        return tab
            .doc
            .can_place_symbol_in_current_context(&tab.session.edit_path, live_id);
    }

    if tab.id == clipboard.source_doc_id
        && tab
            .session
            .edit_path
            .levels
            .iter()
            .any(|lvl| lvl.symbol_id == src_sym_id)
    {
        return false;
    }

    let Some(src_sym) = clipboard.symbols.iter().find(|s| s.id == src_sym_id) else {
        return true;
    };

    for layer in &src_sym.timeline.layers {
        for kf in &layer.keyframes {
            for inst in kf.cel.instances.iter() {
                if !can_paste_clipboard_symbol_inner(
                    tab,
                    clipboard,
                    inst.binding.symbol_id,
                    visited,
                ) {
                    return false;
                }
            }
        }
    }
    true
}

/// クリップボード内のシンボル `src_sym_id` が現在の編集コンテキストに対して循環参照を起こさずに配置可能かを判定する
pub(crate) fn can_paste_clipboard_symbol(
    tab: &Tab,
    clipboard: &Clipboard,
    src_sym_id: u64,
) -> bool {
    let mut visited = Vec::new();
    can_paste_clipboard_symbol_inner(tab, clipboard, src_sym_id, &mut visited)
}

/// `paste_clipboard`（`Cmd+V`）実行前に、現在のタブでクリップボード内の図形がすべて自己・親シンボルの循環参照により配置不可能かを判定する
pub(crate) fn would_clipboard_paste_all_cycle(tab: &Tab, clipboard: &Clipboard) -> bool {
    if clipboard.shapes.is_empty() || tab.session.edit_path.is_empty() {
        return false;
    }
    clipboard.shapes.iter().all(|shape| {
        shape
            .symbol_instance
            .is_some_and(|b| !can_paste_clipboard_symbol(tab, clipboard, b.symbol_id))
    })
}

fn collect_needed_clipboard_imports(
    tab: &Tab,
    clipboard: &Clipboard,
    src_sym_id: u64,
    local_sym_map: &mut HashMap<u64, u64>,
    needed_imports: &mut HashSet<u64>,
    visited: &mut HashSet<u64>,
) {
    if !visited.insert(src_sym_id) {
        return;
    }
    if let Some(valid_id) = existing_reusable_target_symbol_id(tab, clipboard, src_sym_id) {
        local_sym_map.insert(src_sym_id, valid_id);
        return;
    }
    needed_imports.insert(src_sym_id);
    if let Some(src_sym) = clipboard.symbols.iter().find(|s| s.id == src_sym_id) {
        for layer in &src_sym.timeline.layers {
            for kf in &layer.keyframes {
                for inst in kf.cel.instances.iter() {
                    collect_needed_clipboard_imports(
                        tab,
                        clipboard,
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
pub(crate) fn resolve_clipboard_for_paste(
    tab: &mut Tab,
    clipboard: &mut Clipboard,
) -> (Vec<FillShape>, bool) {
    let mut had_cycle = false;
    let mut resolved_shapes = Vec::with_capacity(clipboard.shapes.len());
    for shape in &clipboard.shapes {
        if let Some(b) = shape.symbol_instance {
            if !can_paste_clipboard_symbol(tab, clipboard, b.symbol_id) {
                had_cycle = true;
                continue;
            }
        }
        resolved_shapes.push(shape.clone());
    }

    if resolved_shapes.is_empty() || clipboard.symbols.is_empty() {
        return (resolved_shapes, had_cycle);
    }

    let target_doc_id = tab.id;
    let source_doc_id = clipboard.source_doc_id;
    let mut local_sym_map: HashMap<u64, u64> = HashMap::new();
    let mut needed_imports: HashSet<u64> = HashSet::new();
    let mut visited: HashSet<u64> = HashSet::new();

    for shape in &resolved_shapes {
        if let Some(b) = shape.symbol_instance {
            collect_needed_clipboard_imports(
                tab,
                clipboard,
                b.symbol_id,
                &mut local_sym_map,
                &mut needed_imports,
                &mut visited,
            );
        }
    }

    let mut any_imported = false;

    for idx in 0..clipboard.symbols.len() {
        let src_sym_id = clipboard.symbols[idx].id;
        if !needed_imports.contains(&src_sym_id) {
            continue;
        }
        let mut imported = (*clipboard.symbols[idx]).clone();

        let new_id = if target_doc_id == source_doc_id && tab.doc.find_symbol(imported.id).is_none()
        {
            tab.doc.next_symbol_id = tab.doc.next_symbol_id.max(imported.id.saturating_add(1));
            imported.id
        } else {
            let id = tab.doc.next_symbol_id;
            tab.doc.next_symbol_id = tab.doc.next_symbol_id.saturating_add(1);
            id
        };

        local_sym_map.insert(imported.id, new_id);
        clipboard
            .imported_map
            .insert((target_doc_id, imported.id), new_id);

        imported.id = new_id;
        if tab.doc.library.iter().any(|s| s.name == imported.name) {
            let base_name = imported.name.clone();
            let mut suffix = 2usize;
            loop {
                let candidate = format!("{base_name} ({suffix})");
                if !tab.doc.library.iter().any(|s| s.name == candidate) {
                    imported.name = candidate;
                    break;
                }
                suffix = suffix.saturating_add(1);
            }
        }
        for layer in &mut imported.timeline.layers {
            for i in 0..layer.keyframes.len() {
                if !layer.keyframes[i].cel.instances.is_empty() {
                    for inst in layer.keyframe_mut(i).cel.instances_mut() {
                        if let Some(&mapped_child_id) = local_sym_map.get(&inst.binding.symbol_id) {
                            inst.binding.symbol_id = mapped_child_id;
                        }
                    }
                }
            }
        }
        remap_timeline_group_ids(&mut imported.timeline, &mut tab.doc);
        let max_internal_gid = max_group_id_in_timeline(&imported.timeline);
        tab.doc.next_group_id = tab
            .doc
            .next_group_id
            .max(max_internal_gid.saturating_add(1));

        tab.doc.library.push(imported);
        if tab.view.selected_library_symbol.is_none() {
            tab.view.selected_library_symbol = Some(new_id);
        }
        any_imported = true;
    }

    if any_imported {
        tab.doc.bump_revision();
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
pub(crate) fn prepare_copy_selected_shapes(tab: &mut Tab, clipboard: &mut Clipboard) -> bool {
    cancel_active_drag(tab);
    let mut extracted = tab.session.extract_selected_relative_to_scope();
    if extracted.is_empty() {
        return false;
    }
    for s in &mut extracted {
        s.strip_transient_caches();
    }
    clipboard.shapes = extracted;
    clipboard.paste_count = 0;
    clipboard.paste_offset = KVec2::ZERO;
    capture_clipboard_symbols(tab, clipboard);
    true
}
