use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, cancel_active_drag, finish_cel_writes, write_cel, Clipboard,
    MoveSelectionDrag, Tab,
};
use crate::planar_map::CheckedMap;
use crate::symbol::{Document, Library, SymbolDef};
use crate::timeline::{Cel, Timeline};
use crate::vector_engine::LayerId;

fn remap_gid(gid_map: &mut HashMap<u64, u64>, old_gid: u64, doc: &mut Document) -> u64 {
    *gid_map
        .entry(old_gid)
        .or_insert_with(|| doc.alloc_group_id())
}

// ============================================================================
// 新しい中身（コピー・カット・貼り付け・Alt 複製・シンボル化・シンボルの分解）
// 設計：claude/新しい中身も地図で運ぶ設計_クリップボード_4f4e953.md
// ============================================================================

/// 新しい中身：写したレイヤーごとのセル画（下から順。地図とインスタンス。階層の番号の並びは写した所からの相対、座標は
/// 写した所の文書の座標）と、写した所の文書の座標 → ステージの座標の変換。地図だけで作り（`selected_content`）、地図だけで
/// 置く（`prepare_placement`・`put_placements`）。図形から地図は作らない
#[derive(Clone, Debug)]
pub(crate) struct Content {
    pub(crate) layers: Vec<(LayerId, Cel)>,
    pub(crate) world: Affine,
}

impl Content {
    /// 中身が参照するシンボルの番号（重なりの順。同じ番号も並ぶ）
    fn symbol_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.layers
            .iter()
            .flat_map(|(_, c)| c.instances.iter().map(|i| i.binding.symbol_id))
    }
}

/// 選んでいるもの（今の階層の）の中身：レイヤーごとに、選んだ面だけを残した地図と、選んだグループ・インスタンス
/// （`Cel::picked`）。浮いた片のあるレイヤーの生の図形は、残りの選んだ所に片を重ねた地図（確定と同じ重ね方：
/// `FloatLayer::selected_map`）。トゥイーンの途中のインスタンスは、そのコマの結び付け。`None`：選んでいるものが無いか、
/// 浮いた片を地図にできない
pub(crate) fn selected_content(tab: &Tab) -> Option<Content> {
    let session = &tab.session;
    if session.selection.is_empty() {
        return None;
    }
    let scope = &session.editing_group_path;
    let tl = tab.active_timeline();
    let mut layers = Vec::new();
    for &layer_id in session.base_cache.layer_order_ids() {
        let Some(cel) = session.base_cache.cel(layer_id) else {
            continue;
        };
        let float = session
            .floating()
            .and_then(|fl| fl.layers().iter().find(|l| l.layer_id == layer_id));
        let raw = match float {
            Some(l) => Some(l.selected_map(session.stage(), &session.selection.shapes)?),
            None => None,
        };
        let tween = tl
            .layer_index_by_id(layer_id)
            .and_then(|i| tl.layers[i].interpolated_tween_binding_at(tl.current_frame));
        let picked = cel.picked(
            scope,
            &session.selection.shapes,
            &session.selection.instances,
            raw,
            tween,
        );
        if !picked.is_empty() {
            layers.push((layer_id, picked));
        }
    }
    (!layers.is_empty()).then(|| Content {
        layers,
        world: session.edit_path.world_xform(),
    })
}

/// 中身の座標（文書の座標 → ステージの変換が `from`）から、置く先の文書の座標（同じく `to`）への変換。ステージで
/// `offset` ずらす。同じ所（`from == to`）なら、ずらす量を置く先の座標に写した平行移動（`to⁻¹·T(offset)·to` と同じ値を、
/// 丸めを入れずに作る。ずらさなければ恒等の変換で、地図はそのまま写る）
pub(crate) fn content_xform(from: Affine, to: Affine, offset: KVec2) -> Affine {
    if from == to {
        let [a, b, c, d, _, _] = to.as_coeffs();
        let linear_inv = Affine::new([a, b, c, d, 0.0, 0.0]).inverse();
        Affine::translate((linear_inv * offset.to_point()).to_vec2())
    } else {
        to.inverse() * Affine::translate(offset) * from
    }
}

/// 置く準備のできた中身（文書を変えない）：置く先のレイヤーごとの、浮いた片にする地図（下から順。中身の座標。面の番号は
/// 新しい）と、セル画に入れるグループとインスタンス（置く先の座標。面の番号は新しく、グループ・インスタンス・シンボルの
/// 番号は中身のまま）。浮いた片には変換 `xform` をかける
pub(crate) struct Placement {
    layers: Vec<PlacedLayer>,
    xform: Affine,
}

struct PlacedLayer {
    layer_id: LayerId,
    pieces: Vec<CheckedMap>,
    nested: Cel,
    /// 中身と入れ替えるインスタンス（シンボルの分解）
    replacing: Option<u64>,
}

impl Placement {
    /// 浮いた片にする地図があるか
    pub(crate) fn has_pieces(&self) -> bool {
        self.layers.iter().any(|l| !l.pieces.is_empty())
    }
}

/// 中身 `content` を、変換 `xform`（中身の座標 → 今の階層の文書の座標）で置く準備をする（文書を変えない）。置く先は、
/// `keep_layers` なら元のレイヤー（編集できなければ作業中のレイヤー）、そうでなければ作業中のレイヤー。生の図形の地図は
/// 浮いた片にする（変換は確定のときにかける）。グループの地図は、その座標の格子で正しい地図にする（`Cel::transformed`）。
/// `replacing`：（レイヤー, インスタンス）があれば、そのレイヤーでは中身をそのインスタンスと入れ替える。`None`：格子に
/// 置けない座標がある（呼ぶ側は文書を変えずに断る）
pub(crate) fn prepare_placement(
    tab: &Tab,
    content: &Content,
    xform: Affine,
    keep_layers: bool,
    replacing: Option<(LayerId, u64)>,
) -> Option<Placement> {
    let tl = tab.active_timeline();
    let active = active_scope_layer_id(&tab.session, tl);
    // 入れ替えるインスタンスのレイヤーは、中身が空でも置く（インスタンスを外す）
    let mut by_layer: Vec<(LayerId, Vec<CheckedMap>, Vec<Cel>)> = replacing
        .map(|(layer_id, _)| (layer_id, Vec::new(), Vec::new()))
        .into_iter()
        .collect();
    for (layer_id, cel) in &content.layers {
        let target =
            tl.editable_layer_or(if keep_layers && tl.is_layer_editable_by_id(*layer_id) {
                *layer_id
            } else {
                active
            });
        let (raw, nested) = cel.split_raw();
        let nested = nested.transformed(xform)?;
        let raw = match raw {
            Some(m) if !m.fits_grid(xform) => return None,
            Some(m) => Some(m.checked()?.freshly_labeled()),
            None => None,
        };
        let i = match by_layer.iter().position(|l| l.0 == target) {
            Some(i) => i,
            None => {
                by_layer.push((target, Vec::new(), Vec::new()));
                by_layer.len() - 1
            }
        };
        by_layer[i].1.extend(raw);
        by_layer[i].2.push(nested);
    }
    let layers = by_layer
        .into_iter()
        .map(|(layer_id, pieces, nested)| {
            Some(PlacedLayer {
                layer_id,
                pieces,
                nested: Cel::stacked(&nested)?,
                replacing: replacing.filter(|r| r.0 == layer_id).map(|r| r.1),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Placement { layers, xform })
}

/// 準備した中身 `placements` を、今の階層に置く（文書を変える）。グループとインスタンスには新しい番号を振り（中身ごとに。
/// シンボルの番号は `sym` で付け替える）、セル画に入れる（`Cel::insert_content`）。生の図形の地図は、浮いた片の組にする。
/// 置いたものを選択に足す（呼ぶ側は、要るなら先に選択を外す）
pub(crate) fn put_placements(tab: &mut Tab, placements: Vec<Placement>, sym: impl Fn(u64) -> u64) {
    let scope = tab.session.editing_group_path.clone();
    let mut written: Vec<LayerId> = Vec::new();
    let mut floats = Vec::new();
    for p in placements {
        let mut gid_map: HashMap<u64, u64> = HashMap::new();
        for l in p.layers {
            let mut nested = l.nested;
            if !nested.is_empty() || l.replacing.is_some() {
                nested.remap_ids(
                    |gid| remap_gid(&mut gid_map, gid, &mut tab.doc),
                    &sym,
                    Some(l.layer_id),
                );
                if write_cel(tab, l.layer_id, |cel| {
                    cel.insert_content(&scope, &nested, l.replacing)
                }) {
                    if !written.contains(&l.layer_id) {
                        written.push(l.layer_id);
                    }
                    let sel = &mut tab.session.selection;
                    sel.shapes.extend(nested.face_ids());
                    sel.instances
                        .extend(nested.instances.iter().map(|i| i.binding.instance_gid));
                }
            }
            floats.extend(l.pieces.into_iter().map(|m| (l.layer_id, m, p.xform)));
        }
    }
    finish_cel_writes(tab, &written);
    tab.session.add_floating_maps(floats);
    tab.session.retain_existing_selection();
    tab.session.transform_pivot = None;
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
    library: &Library,
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
    out.push(Arc::clone(sym));
}

/// 別タブから移植されるシンボルの内部タイムライン内の子シンボル参照ID（`sym_map` にあるもの）と全グループIDを、貼り付け先タブの `Document` のIDへ一括リマップする
fn remap_timeline_ids(tl: &mut Timeline, doc: &mut Document, sym_map: &HashMap<u64, u64>) {
    let mut gid_map: HashMap<u64, u64> = HashMap::new();
    for layer in &mut tl.layers {
        for i in 0..layer.keyframes.len() {
            let cel = &layer.keyframes[i].cel;
            if cel.maps.iter().all(|m| m.group_path.is_empty()) && cel.instances.is_empty() {
                continue;
            }
            layer.keyframe_mut(i).cel.remap_ids(
                |gid| remap_gid(&mut gid_map, gid, doc),
                |sym| sym_map.get(&sym).copied().unwrap_or(sym),
                None,
            );
        }
    }
}

/// `copy_selected_shapes`（`Cmd+C`）実行時に、クリップボード内の図形が参照しているシンボル定義（入れ子含む）を `Arc` 共有スナップショットとして退避する
pub(crate) fn capture_clipboard_symbols(tab: &Tab, clipboard: &mut Clipboard) {
    clipboard.source_doc_id = tab.id;
    clipboard.source_symbol_id = tab.session.edit_path.last().map(|s| s.symbol_id);
    clipboard.symbols.clear();

    let mut visited = Vec::new();
    let ids: Vec<u64> = clipboard
        .content
        .iter()
        .flat_map(Content::symbol_ids)
        .collect();
    for id in ids {
        collect_symbol_and_descendants(id, &tab.doc.library, &mut visited, &mut clipboard.symbols);
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

/// クリップボード内のシンボル `src_sym_id` が現在の編集コンテキストに対して循環参照を起こさずに配置可能かを判定する：
/// 中身を（貼り先のシンボルを使えるものは、そのシンボルの中身を）たどって、編集中のシンボルに行き着かないか
/// （`symbol::reaches`。同じタブのクリップボードのシンボルは、同じ番号の文書のシンボルそのもの）
pub(crate) fn can_paste_clipboard_symbol(
    tab: &Tab,
    clipboard: &Clipboard,
    src_sym_id: u64,
) -> bool {
    /// たどる相手：クリップボードのシンボルか、貼り先の文書のシンボル
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum Node {
        Clip(u64),
        Doc(u64),
    }
    let levels = &tab.session.edit_path.levels;
    if levels.is_empty() {
        return true;
    }
    let same_doc = tab.id == clipboard.source_doc_id;
    let node = |id: u64| {
        existing_reusable_target_symbol_id(tab, clipboard, id).map_or(Node::Clip(id), Node::Doc)
    };
    let editing = |id: u64| levels.iter().any(|lvl| lvl.symbol_id == id);
    !crate::symbol::reaches(
        node(src_sym_id),
        |n| match n {
            Node::Doc(id) => editing(id),
            Node::Clip(id) => same_doc && editing(id),
        },
        |n| match n {
            Node::Doc(id) => tab
                .doc
                .child_symbol_ids(id)
                .into_iter()
                .map(Node::Doc)
                .collect(),
            Node::Clip(id) => {
                clipboard
                    .symbols
                    .iter()
                    .find(|s| s.id == id)
                    .map_or_else(Vec::new, |s| {
                        s.timeline
                            .instances()
                            .map(|inst| node(inst.binding.symbol_id))
                            .collect()
                    })
            }
        },
    )
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

/// クリップボードの中身のうち、今の編集の所に置けるもの（循環参照になるシンボルのインスタンスを除いたもの。文書は変えない）
/// と、除いたものがあったか。置けるものが無ければ `None`
pub(crate) fn pasteable_content(tab: &Tab, clipboard: &Clipboard) -> (Option<Content>, bool) {
    let Some(content) = &clipboard.content else {
        return (None, false);
    };
    let mut had_cycle = false;
    let mut content = content.clone();
    for (_, cel) in &mut content.layers {
        if cel
            .instances
            .iter()
            .all(|i| can_paste_clipboard_symbol(tab, clipboard, i.binding.symbol_id))
        {
            continue;
        }
        had_cycle = true;
        cel.instances_mut()
            .retain(|i| can_paste_clipboard_symbol(tab, clipboard, i.binding.symbol_id));
    }
    content.layers.retain(|(_, c)| !c.is_empty());
    ((!content.layers.is_empty()).then_some(content), had_cycle)
}

/// 中身 `content` が参照するシンボルのうち、今のタブのライブラリに無いものを、クリップボードの写し（壊さない）から移植する。
/// 戻り値は、クリップボードのシンボルの番号 → 今のタブのシンボルの番号（変わるものだけ）
pub(crate) fn import_clipboard_symbols(
    tab: &mut Tab,
    clipboard: &mut Clipboard,
    content: &Content,
) -> HashMap<u64, u64> {
    let mut local_sym_map: HashMap<u64, u64> = HashMap::new();
    if clipboard.symbols.is_empty() {
        return local_sym_map;
    }
    let target_doc_id = tab.id;
    let source_doc_id = clipboard.source_doc_id;
    let mut needed_imports: HashSet<u64> = HashSet::new();
    let mut visited: HashSet<u64> = HashSet::new();
    for id in content.symbol_ids() {
        collect_needed_clipboard_imports(
            tab,
            clipboard,
            id,
            &mut local_sym_map,
            &mut needed_imports,
            &mut visited,
        );
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

        tab.doc.library.push(Arc::new(imported));
        if tab.view.selected_library_symbol.is_none() {
            tab.view.selected_library_symbol = Some(new_id);
        }
        any_imported = true;
    }

    if any_imported {
        tab.doc.bump_revision();
    }
    local_sym_map
}

/// 選択中のオブジェクト（生シェイプ・部分スライス領域・グループ問わず）を内部クリップボードにコピーする（`Cmd+C`）
pub(crate) fn prepare_copy_selected_shapes(tab: &mut Tab, clipboard: &mut Clipboard) -> bool {
    cancel_active_drag(tab);
    let Some(content) = selected_content(tab) else {
        return false;
    };
    clipboard.content = Some(content);
    clipboard.paste_count = 0;
    clipboard.paste_offset = KVec2::ZERO;
    capture_clipboard_symbols(tab, clipboard);
    true
}
