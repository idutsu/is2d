use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, cancel_active_drag, Clipboard, MoveSelectionDrag, Tab,
};
use crate::symbol::{Document, SymbolDef, SymbolEvalCtx};
use crate::timeline::Timeline;
use crate::vector_engine::{next_shape_id, FillShape};

fn remap_gid(gid_map: &mut HashMap<u64, u64>, old_gid: u64, doc: &mut Document) -> u64 {
    *gid_map
        .entry(old_gid)
        .or_insert_with(|| doc.alloc_group_id())
}

/// 相対グループパスを持つシェイプ群に対し、新しいグループID・シンボルインスタンスIDを採番して現在の編集スコープ用に変換する
pub(crate) fn instantiate_relative_shapes_in_scope(
    tab: &mut Tab,
    rel_shapes: &[FillShape],
    offset: KVec2,
    preserve_original_layer: bool,
) -> (Vec<FillShape>, bool) {
    let active_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
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
                new_parent_scope.push(remap_gid(&mut gid_map, old_gid, &mut tab.doc));
            }
            let mapped_inst_gid = remap_gid(&mut gid_map, orig_binding.instance_gid, &mut tab.doc);

            let mut new_binding = orig_binding;
            new_binding.instance_gid = mapped_inst_gid;
            if offset != KVec2::ZERO {
                new_binding.xform = Affine::translate(offset) * new_binding.xform;
            }

            if tab.doc.find_symbol(new_binding.symbol_id).is_some() {
                let tl = tab.active_timeline();
                let curr_f = tl.current_frame;
                let kf_start = tl.keyframe_start_frame_at(target_lid, curr_f);
                instantiated.extend(new_binding.expand(
                    (kf_start, curr_f),
                    target_lid,
                    &new_parent_scope,
                    SymbolEvalCtx::new(&tab.doc.library),
                ));
                continue;
            }
        }

        let mut new_path = tab.session.editing_group_path.clone();
        if item.group_path.is_empty() {
            any_raw = true;
        } else {
            for &old_gid in &item.group_path {
                new_path.push(remap_gid(&mut gid_map, old_gid, &mut tab.doc));
            }
        }
        let mut cloned = item
            .clone()
            .with_layer_id(target_lid)
            .with_group_path(new_path);
        cloned.shape_id = next_shape_id();
        cloned.symbol_instance = None;
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
    for inst in sym.timeline.instances() {
        collect_symbol_and_descendants(inst.binding.symbol_id, library, visited, out);
    }
    let mut cloned = sym.clone();
    cloned.clear_preview_cache();
    out.push(Arc::new(cloned));
}

/// 別タブから移植されるシンボルの内部タイムライン内の子シンボル参照ID（`sym_map` にあるもの）と全グループIDを、貼り付け先タブの `Document` のIDへ一括リマップする
fn remap_timeline_ids(tl: &mut Timeline, doc: &mut Document, sym_map: &HashMap<u64, u64>) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    for layer in &mut tl.layers {
        for i in 0..layer.keyframes.len() {
            if layer.keyframes[i]
                .cel
                .maps
                .iter()
                .any(|m| !m.group_path.is_empty())
            {
                layer
                    .keyframe_mut(i)
                    .cel
                    .remap_group_ids(|gid| remap_gid(&mut gid_map, gid, doc));
            }
            if !layer.keyframes[i].cel.instances.is_empty() {
                for inst in layer.keyframe_mut(i).cel.instances_mut() {
                    if let Some(&mapped_child_id) = sym_map.get(&inst.binding.symbol_id) {
                        inst.binding.symbol_id = mapped_child_id;
                    }
                    inst.binding.instance_gid =
                        remap_gid(&mut gid_map, inst.binding.instance_gid, doc);
                    for gid in &mut inst.parent_scope {
                        *gid = remap_gid(&mut gid_map, *gid, doc);
                    }
                }
            }
        }
    }
}

/// `copy_selected_shapes`（`Cmd+C`）実行時に、クリップボード内の図形が参照しているシンボル定義（入れ子含む）を `Arc` 共有スナップショットとして退避する
pub(crate) fn capture_clipboard_symbols(tab: &Tab, clipboard: &mut Clipboard) {
    clipboard.source_doc_id = tab.id;
    clipboard.source_symbol_id = tab.session.edit_path.last().map(|s| s.symbol_id);
    clipboard.symbols.clear();

    let mut visited = Vec::new();
    for shape in &clipboard.shapes {
        if let Some(b) = shape.symbol_instance {
            collect_symbol_and_descendants(
                b.symbol_id,
                &tab.doc.library,
                &mut visited,
                &mut clipboard.symbols,
            );
        }
    }
}

/// クリップボードのシンボル `src_sym_id` の代わりに使える、貼り先のタブのシンボルの番号を返す。
/// 同じタブなら同じ番号のシンボルを使う。別のタブから前に移植した写しは、中身（`revision`）が同じで、
/// 子もすべてそのまま使えるものを指しているときだけ使う
fn existing_reusable_target_symbol_id(
    tab: &Tab,
    clipboard: &Clipboard,
    src_sym_id: u64,
) -> Option<u64> {
    let target_doc_id = tab.id;
    let source_doc_id = clipboard.source_doc_id;
    if target_doc_id == source_doc_id {
        if let Some(sym_def) = tab.doc.find_symbol(src_sym_id) {
            return sym_def
                .timeline
                .instances()
                .all(|inst| tab.doc.find_symbol(inst.binding.symbol_id).is_some())
                .then_some(src_sym_id);
        }
    }
    let src = clipboard.symbols.iter().find(|s| s.id == src_sym_id)?;
    let candidate = *clipboard
        .imported_map
        .get(&(source_doc_id, src_sym_id, target_doc_id))?;
    let target = tab
        .doc
        .find_symbol(candidate)
        .filter(|t| t.revision == src.revision)?;
    let target_children: HashSet<u64> = target
        .timeline
        .instances()
        .map(|inst| inst.binding.symbol_id)
        .collect();
    let src_children: HashSet<u64> = src
        .timeline
        .instances()
        .map(|inst| inst.binding.symbol_id)
        .collect();
    src_children
        .into_iter()
        .all(|child| {
            existing_reusable_target_symbol_id(tab, clipboard, child)
                .is_some_and(|id| target_children.contains(&id))
        })
        .then_some(candidate)
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

    for inst in src_sym.timeline.instances() {
        if !can_paste_clipboard_symbol_inner(tab, clipboard, inst.binding.symbol_id, visited) {
            return false;
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
        for inst in src_sym.timeline.instances() {
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

/// `paste_clipboard`（`Cmd+V`）実行時に、クリップボードの不変スナップショットを破壊せず、
/// 現在のタブのライブラリに不足しているシンボル定義のみを自動移植してペースト用 `(Vec<FillShape>, had_cycle)` を返す。
/// 循環参照で貼れる図形が1つも無いときは文書に触れずに空の列を返し、貼れる図形があるときは文書を変える前に `before_mutate` を呼ぶ
pub(crate) fn resolve_clipboard_for_paste(
    tab: &mut Tab,
    clipboard: &mut Clipboard,
    before_mutate: impl FnOnce(&mut Tab),
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

    if resolved_shapes.is_empty() {
        return (resolved_shapes, had_cycle);
    }
    before_mutate(tab);
    if clipboard.symbols.is_empty() {
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
            tab.doc.alloc_symbol_id()
        };

        local_sym_map.insert(imported.id, new_id);
        clipboard
            .imported_map
            .insert((source_doc_id, imported.id, target_doc_id), new_id);
        clipboard
            .imported_map
            .insert((target_doc_id, new_id, source_doc_id), src_sym_id);

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
        remap_timeline_ids(&mut imported.timeline, &mut tab.doc, &local_sym_map);

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
