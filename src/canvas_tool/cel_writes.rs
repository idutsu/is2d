//! レイヤーの解決・ドラッグの取り消し・セル画を書く入口（`write_cel`・`write_scope_map`）と、浮いた片の確定（`plan_commit`・
//! `write_commit`・`commit_floating_except`・`clear_selection`）

use super::*;

/// 編集を断った理由（`Session::refusal`。`apply` が知らせる）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// 地図にできない形（格子の外 ±65,536px・有限でない座標・潰れるほど縮める変換：`PlanarMap::fits_grid`）を作る
    OffGrid,
    /// 浮いた片を置けない（置いた結果を地図に書けない・置くレイヤーを編集できない）。浮いた片は残す
    Unplaceable,
}

impl Refusal {
    /// 画面の下に出す知らせ
    pub(crate) fn message(self) -> &'static str {
        match self {
            Refusal::OffGrid => "地図の範囲（±65,536px）の外や、潰れるほど小さくには置けません",
            Refusal::Unplaceable => "選んだ図形をここに置けません（動かすか、取り消してください）",
        }
    }
}

/// 現在フレームで編集可能（表示中・非ロック・フレーム範囲内・非トゥイーン途中）な全レイヤーID一覧を背面→前面順で返す
pub(crate) fn editable_layer_ids(tab: &Tab) -> Vec<LayerId> {
    let tl = tab.active_timeline();
    let frame = tl.current_frame;
    tl.layers
        .iter()
        .filter(|l| l.is_editable_at(frame))
        .map(|l| l.id)
        .collect()
}

/// セル画を書いたあとの後始末。セル画を書く入口（`write_cel`・`write_scope_map`）のあとに、書いたレイヤーの一覧
/// `lids` で1回だけ呼ぶ：書いたレイヤーがあれば、編集中のシンボルの版と `Document::revision` を上げ、画面の土台のその
/// レイヤーを作り直す。編集中の階層が無くなっていれば、ある所まで戻る
pub(crate) fn finish_cel_writes(tab: &mut Tab, lids: &[LayerId]) {
    if !lids.is_empty() {
        if let Some(idx) = tab.active_symbol_index() {
            Arc::make_mut(&mut tab.doc.library[idx]).revision =
                crate::vector_engine::next_mesh_revision();
            tab.doc.bump_revision();
        }
        for &lid in lids {
            tab.session.base_cache.invalidate_layer(lid);
        }
    }
    tab.refresh_scene_base();
    tab.session.sanitize_editing_scope();
}

/// 地図の編集：今のコマ・今の階層の、レイヤー `layer_id` のセル画の地図（文書の座標）を `f` で編集する（`Cel::edit_map`。
/// 図形はその地図の面から作る）。`false`：編集できない（ロック・非表示・キーフレームが無い・地図にできない）か、変わらない
pub(crate) fn edit_layer_map(
    tab: &mut Tab,
    layer_id: LayerId,
    f: impl FnOnce(&crate::planar_map::LayerBase) -> Option<crate::planar_map::CheckedMap>,
) -> bool {
    let scope = tab.session.editing_group_path.clone();
    edit_scope_map(tab, layer_id, &scope, f)
}

/// `edit_layer_map` の、階層 `scope`（グループの中の階層など）を編集するもの
pub(crate) fn edit_scope_map(
    tab: &mut Tab,
    layer_id: LayerId,
    scope: &[u64],
    f: impl FnOnce(&crate::planar_map::LayerBase) -> Option<crate::planar_map::CheckedMap>,
) -> bool {
    let written: Vec<LayerId> = write_scope_map(tab, layer_id, scope, f)
        .then_some(layer_id)
        .into_iter()
        .collect();
    finish_cel_writes(tab, &written);
    !written.is_empty()
}

/// 今のコマの、レイヤー `layer_id` の階層 `scope` のセル画の地図を `f` で編集する（`Cel::edit_map`）。後始末はしない
/// ので、いくつかのレイヤー・階層を書いたあとに、書いたレイヤーで `finish_cel_writes` を1回呼ぶ。`false`：編集できない
/// （ロック・非表示・キーフレームが無い・地図にできない）か、変わらない
pub(crate) fn write_scope_map(
    tab: &mut Tab,
    layer_id: LayerId,
    scope: &[u64],
    f: impl FnOnce(&crate::planar_map::LayerBase) -> Option<crate::planar_map::CheckedMap>,
) -> bool {
    let mut held = None;
    let written = write_cel(tab, layer_id, |cel| match cel.edit_map(scope, f) {
        Some(w) => {
            held = w.0;
            true
        }
        None => false,
    });
    // 新しい地図の写し（面の図形は、変わらない面は前の図形）は、画面の土台が持つ
    if let Some(base) = held {
        tab.session.base_cache.hold(layer_id, base);
    }
    written
}

/// 今のコマの、レイヤー `layer_id` のセル画を `f` で書き換える（`f` が `true` を返したら書いた）。後始末はしないので、
/// 書いたあとに、書いたレイヤーで `finish_cel_writes` を1回呼ぶ。`false`：編集できない（ロック・非表示・キーフレームが
/// 無い・トゥイーンの途中）か、`f` が書かなかった
pub(crate) fn write_cel(
    tab: &mut Tab,
    layer_id: LayerId,
    f: impl FnOnce(&mut crate::timeline::Cel) -> bool,
) -> bool {
    let tl = tab.active_timeline_raw_mut();
    let frame = tl.current_frame;
    let Some(layer) = tl
        .layers
        .iter_mut()
        .find(|l| l.id == layer_id && l.is_editable_at(frame))
    else {
        return false;
    };
    let Some(kf) = layer.keyframe_index_at(frame) else {
        return false;
    };
    let mut cel = layer.keyframes[kf].cel.clone();
    if !f(&mut cel) {
        return false;
    }
    Arc::make_mut(&mut layer.keyframes[kf]).cel = cel;
    true
}

/// 今のコマの、レイヤー `layer_ids` のセル画をそれぞれ `f(レイヤー, セル画)` で書き換え（`write_cel`）、書いたレイヤーで
/// 後始末をする（`finish_cel_writes`）。`false`：どのレイヤーも書かなかった
pub(crate) fn write_cels(
    tab: &mut Tab,
    layer_ids: &[LayerId],
    f: impl Fn(LayerId, &mut crate::timeline::Cel) -> bool,
) -> bool {
    let mut written = Vec::new();
    for &lid in layer_ids {
        if write_cel(tab, lid, |cel| f(lid, cel)) {
            written.push(lid);
        }
    }
    finish_cel_writes(tab, &written);
    !written.is_empty()
}

/// 今のコマの、レイヤー `layer_ids` のセル画の、番号が `pick` に合うインスタンスの結び付けを `f` で書き換え、後始末を
/// する（`Cel::update_instances`・`finish_cel_writes`）。`false`：どのレイヤーも書かなかった
pub(crate) fn update_instances(
    tab: &mut Tab,
    layer_ids: &[LayerId],
    pick: impl Fn(u64) -> bool,
    f: impl Fn(&mut crate::vector_engine::SymbolInstanceBinding),
) -> bool {
    write_cels(tab, layer_ids, |_, cel| cel.update_instances(&pick, &f))
}

/// 現在の編集スコープ（グループ内であればそのグループの所属レイヤー、そうでなければ現在のアクティブレイヤー）の `layer_id` を返す
pub(crate) fn active_scope_layer_id(session: &Session, timeline: &Timeline) -> u64 {
    if !session.editing_group_path.is_empty() {
        if let Some(s) = session
            .scene()
            .iter()
            .find(|s| s.is_in_editing_scope(&session.editing_group_path))
        {
            return s.layer_id;
        }
    }
    timeline.active_layer_id()
}

/// 指定した `shape_id` が属するレイヤーへ編集中タイムラインの `active_layer` およびタイムライン選択状態を追従させる
pub(crate) fn focus_active_layer_to_shape_by_id(tab: &mut Tab, shape_id: ShapeId) {
    tab.view.inspect_timeline_frame = false;
    if let Some(lid) = tab.scene().shape_by_id(shape_id).map(|s| s.layer_id) {
        focus_active_layer_by_id(tab, lid);
    }
}

/// レイヤー `lid` へ編集中タイムラインの `active_layer` およびタイムライン選択状態を追従させる
pub(crate) fn focus_active_layer_by_id(tab: &mut Tab, lid: LayerId) {
    tab.view.inspect_timeline_frame = false;
    let info = {
        let tl = tab.active_timeline();
        tl.layer_index_by_id(lid)
            .map(|l_idx| (l_idx, tl.current_frame))
    };
    if let Some((l_idx, cf)) = info {
        tab.set_active_layer(l_idx);
        tab.set_active_cell_selection(Some(TimelineCellSelection::single(l_idx, cf)));
    }
}

/// キャンバス上で選択されたシェイプ群（`tab.session.selection`）が所属するレイヤーをタイムライン上で選択状態にする
pub(crate) fn sync_timeline_layer_from_canvas_selection(tab: &mut Tab) {
    tab.view.inspect_timeline_frame = false;
    let mut min_l: Option<usize> = None;
    let mut max_l: Option<usize> = None;
    let mut last_l: Option<usize> = None;
    let tl = tab.active_timeline();
    for shape in tab.scene().iter() {
        if tab.session.selection.contains_shape(shape) {
            if let Some(l_idx) = tl.layer_index_by_id(shape.layer_id) {
                min_l = Some(min_l.map_or(l_idx, |m| m.min(l_idx)));
                max_l = Some(max_l.map_or(l_idx, |m| m.max(l_idx)));
                last_l = Some(l_idx);
            }
        }
    }
    if let (Some(l0), Some(l1), Some(act)) = (min_l, max_l, last_l) {
        let cf = tab.active_timeline().current_frame;
        tab.set_active_layer(act);
        tab.set_active_cell_selection(Some(TimelineCellSelection::from_corners(l0, cf, l1, cf)));
    }
}

/// 進行中のすべてのキャンバスドラッグ操作（ブラシ/消しゴム・輪郭変形・移動・自由変形）を中断し、
/// プレビュー変形中だった図形をドラッグ開始前の状態へ安全に巻き戻す
pub(crate) fn cancel_active_drag(tab: &mut Tab) {
    tab.session.rollback_shape_drag();
    tab.session.drag = ActiveCanvasDrag::None;
    tab.session.hover_target = HoverTarget::None;
    tab.session.transform_hover = TransformHoverTarget::None;
}

pub(crate) fn reset_session_after_timeline_change(tab: &mut Tab) {
    cancel_active_drag(tab);
    tab.session.set_floating(None);
    tab.session.transform_pivot = None;
    tab.session.sanitize_editing_scope();
}

pub(crate) fn set_tool(tab: &mut Tab, tool_settings: &mut ToolSettings, tool: Tool) {
    cancel_active_drag(tab);
    tool_settings.tool = tool;
    tool_settings.select_sub_mode = SelectSubMode::Reshape;
}

pub(crate) fn toggle_select_sub_mode(
    tab: &mut Tab,
    tool_settings: &mut ToolSettings,
    target_mode: SelectSubMode,
) {
    if !clear_selection(tab) {
        return;
    }
    tool_settings.select_sub_mode = if tool_settings.select_sub_mode == target_mode {
        SelectSubMode::Reshape
    } else {
        target_mode
    };
}

/// タイムライン上で選択されたセル範囲（単一キーフレームまたは複数レイヤー範囲）に属するすべての図形をキャンバス上で選択状態にする
pub(crate) fn select_shapes_for_timeline_selection(tab: &mut Tab) {
    if !clear_selection(tab) {
        return;
    }
    let tl = tab.active_timeline();
    let Some(sel) = tl.cell_selection else {
        return;
    };
    let selected_lids: HashSet<u64> = (sel.min_layer..=sel.max_layer)
        .filter_map(|l_idx| tl.layers.get(l_idx))
        .filter(|l| l.is_editable_at(tl.current_frame))
        .map(|l| l.id)
        .collect();
    tab.session.select_shapes_in_layers(&selected_lids);
    tab.view.inspect_timeline_frame = true;
}

/// シンボル基準点（Registration Point `+`）をスクリーン座標 `center` に描画する共通ヘルパー
pub(crate) fn draw_registration_cross(
    painter: &egui::Painter,
    center: Pos2,
    arm: f32,
    with_white_halo: bool,
) {
    if with_white_halo {
        let halo = Stroke::new(3.0_f32, Color32::WHITE);
        painter.line_segment(
            [
                Pos2::new(center.x - arm, center.y),
                Pos2::new(center.x + arm, center.y),
            ],
            halo,
        );
        painter.line_segment(
            [
                Pos2::new(center.x, center.y - arm),
                Pos2::new(center.x, center.y + arm),
            ],
            halo,
        );
    }
    let fg = Stroke::new(1.4_f32, Color32::from_rgb(0, 125, 220));
    painter.line_segment(
        [
            Pos2::new(center.x - arm, center.y),
            Pos2::new(center.x + arm, center.y),
        ],
        fg,
    );
    painter.line_segment(
        [
            Pos2::new(center.x, center.y - arm),
            Pos2::new(center.x, center.y + arm),
        ],
        fg,
    );
}

/// テスト用：片 `pieces`（座標の変換 `xform` で置いた図形）を、図形の並び `base_shapes`（文書の無い、ただの並び）の
/// レイヤー・階層ごとに、地図にして上に重ねる（浮いている片を置くのと同じ重ね方）
#[cfg(test)]
pub(crate) fn merge_pieces_layer_by_layer(
    base_shapes: &mut Vec<FillShape>,
    pieces: Vec<FillShape>,
    xform: Affine,
) {
    use crate::planar_map::{apply_map_change, layer_own, LayerBase, MapInput, MapRule};
    let mut scopes: Vec<(LayerId, Vec<u64>)> = Vec::new();
    for p in &pieces {
        if !scopes
            .iter()
            .any(|x| x.0 == p.layer_id && x.1 == p.group_path)
        {
            scopes.push((p.layer_id, p.group_path.clone()));
        }
    }
    for (layer_id, scope) in scopes {
        let mine: Vec<&FillShape> = pieces
            .iter()
            .filter(|p| p.layer_id == layer_id && p.group_path == scope)
            .collect();
        let own = layer_own(base_shapes, layer_id, &scope);
        let (Some(base), Some(piece)) = (LayerBase::of(&own, xform), LayerBase::of(&mine, xform))
        else {
            continue;
        };
        let change = base
            .map
            .overlay(MapInput::Map(&piece.map), MapRule::Overlay)
            .and_then(|m| m.written(&base))
            .and_then(|next| base.diff_placed(next, &own, layer_id, &scope, xform));
        if let Some(change) = change {
            apply_map_change(base_shapes, change, layer_id, &scope);
        }
    }
}

/// 浮いた片の確定で書くもの（`plan_commit`）：レイヤーごとの、書く地図と面の図形（変わるレイヤーだけ）
pub(crate) type CommitPlan = Vec<(LayerId, crate::planar_map::CheckedMap, Vec<Arc<FillShape>>)>;

/// 浮いた片 `fl` を確定するときに書くもの（文書を変えない）：レイヤーごとに、残りの地図（無ければセル画の地図）に片の組
/// を下から順に重ねた地図（`FloatLayer::committed`）を、今の階層の地図にするとき何を書くか（`Cel::outcome`）。重ねる
/// 計算から何を書くかまでをレイヤーごとに同時に作る（レイヤーをまたがないので、待ち時間はいちばん重いレイヤー1つ分）。
/// 全部か無しか：1つでも書けない（編集できないレイヤーを含む）なら `None` を返して知らせる（浮いた片を残すのは呼ぶ側。
/// 片はいつも文書か浮いた片のどちらかにあり、黙って消えない）。片の組の変換はかける前に確定と同じ式で確かめてある（変形
/// は `Floating::fits_transform`、新しい中身は `prepare_placement`）
pub(crate) fn plan_commit(tab: &mut Tab, fl: &Floating) -> Option<CommitPlan> {
    use crate::timeline::MapOutcome;
    tab.refresh_scene_base();
    let scope = tab.session.editing_group_path.clone();
    let editable = editable_layer_ids(tab);
    let outcomes: Vec<Option<MapOutcome>> = {
        let cache = &tab.session.base_cache;
        crate::planar_map::per_layer_parallel(fl.layers(), |l| {
            let cel = cache.cel(l.layer_id)?;
            let base = cel.scope_base(&scope);
            let next = l.committed(&base)?.written(&base)?;
            Some(cel.outcome(&scope, next))
        })
    };
    let mut plan = CommitPlan::new();
    for (l, outcome) in fl.layers().iter().zip(outcomes) {
        match outcome {
            Some(MapOutcome::Same) if editable.contains(&l.layer_id) => {}
            Some(MapOutcome::Changed(map, shapes)) if editable.contains(&l.layer_id) => {
                plan.push((l.layer_id, map, shapes));
            }
            _ => {
                tab.session.refusal = Some(Refusal::Unplaceable);
                return None;
            }
        }
    }
    Some(plan)
}

/// `plan_commit` で決めたものを、今の階層の地図に書く。戻り値は、セル画を書き換えたか
pub(crate) fn write_commit(tab: &mut Tab, plan: CommitPlan) -> bool {
    let scope = tab.session.editing_group_path.clone();
    let mut written: Vec<LayerId> = Vec::new();
    for (layer_id, map, shapes) in plan {
        let mut held = None;
        if write_cel(tab, layer_id, |cel| {
            held = cel.put_map(&scope, map, shapes);
            true
        }) {
            written.push(layer_id);
        }
        // 新しい地図の写し（面の図形は、変わらない面は前の図形）は、画面の土台が持つ
        if let Some(base) = held {
            tab.session.base_cache.hold(layer_id, base);
        }
    }
    finish_cel_writes(tab, &written);
    !written.is_empty()
}

/// 浮いた片 `fl` を確定する（`plan_commit`・`write_commit`）。`None`：書けない（何も書かず、知らせを記す）。`Some`：
/// セル画を書き換えたか
pub(crate) fn commit_floating_layers(tab: &mut Tab, fl: &Floating) -> Option<bool> {
    let plan = plan_commit(tab, fl)?;
    Some(write_commit(tab, plan))
}

/// `tab.session.floating` のうち `keep_indices`（`Scene` インデックス）に含まれない選択図形を確定
/// （未変形スライスなら外した片を残りに戻すだけで文書は変更せず、変更済みなら外した片と残りを確定して文書へ書き込む）。
/// `false`：確定を断った（`commit_floating_layers`。浮いた片と選択は前のまま。知らせは `Session::refusal`）
#[must_use]
pub(crate) fn commit_floating_except(tab: &mut Tab, keep_indices: &[usize]) -> bool {
    let (is_sel, any_valid) = tab.session.selected_shape_mask();
    let mut is_keep = vec![false; is_sel.len()];
    let mut any_keep = false;
    if any_valid {
        for &idx in keep_indices {
            if idx < is_keep.len() && is_sel[idx] {
                is_keep[idx] = true;
                any_keep = true;
            }
        }
    }

    if !any_keep {
        let Some(fl) = tab.session.take_floating() else {
            return true;
        };
        if fl.modified && commit_floating_layers(tab, &fl).is_none() {
            tab.session.set_floating(Some(fl));
            return false;
        }
        tab.session.retain_existing_selection();
        return true;
    }

    let before = tab.session.selection.clone();
    tab.session
        .set_selected_indices((0..is_keep.len()).filter(|&i| is_keep[i]));
    let Some(mut fl) = tab.session.take_floating() else {
        return true;
    };
    let keep: HashSet<ShapeId> = fl
        .pieces()
        .iter()
        .filter(|p| tab.session.selection.contains_shape(p))
        .map(|p| p.shape_id)
        .collect();
    let all_kept = keep.len() == fl.pieces().len();

    if !fl.modified {
        if keep.is_empty() {
            tab.session.retain_existing_selection();
            return true;
        }
        // 外した片は残りに戻す（残す片の所だけで、セル画の地図を切り直す。同じ色でつながる所は1つになる）。戻せなければ、
        // 浮いた片と選択を前のままにして断る（浮いた片はいつもみな選ばれている）
        if !all_kept {
            let whole = fl.clone();
            if !fl.return_pieces(tab.session.stage(), &keep) {
                tab.session.set_floating(Some(whole));
                tab.session.selection = before;
                tab.session.refusal = Some(Refusal::Unplaceable);
                return false;
            }
        }
        tab.session.set_floating(Some(fl));
        tab.session.retain_existing_selection();
        return true;
    }

    let had_rest = fl.layers().iter().any(|l| l.origin.is_some());
    let merged_any = !all_kept || (keep.is_empty() && had_rest);
    // 確定を断ったら、分ける前の浮いた片と選択に戻す
    let whole = merged_any.then(|| fl.clone());
    let to_commit = merged_any.then(|| fl.split_off(&keep));
    if !keep.is_empty() {
        tab.session.set_floating(Some(fl));
    }
    if let Some(to_commit) = to_commit {
        if commit_floating_layers(tab, &to_commit).is_none() {
            tab.session.set_floating(whole);
            tab.session.selection = before;
            return false;
        }
    }
    tab.session.retain_existing_selection();
    true
}

/// 進行中のドラッグを中断し、`tab.session.floating` をすべて確定する。`false`：確定を断った（浮いた片と選択は前のまま。
/// 呼ぶ側は、浮いた片が無いことを前提にする操作をせずに断る）
#[must_use]
pub(crate) fn commit_floating(tab: &mut Tab) -> bool {
    cancel_active_drag(tab);
    commit_floating_except(tab, &[])
}

/// 選択を外す（浮いた片は確定する）。`false`：確定を断った（浮いた片と選択は前のまま。呼ぶ側は断る）
#[must_use]
pub(crate) fn clear_selection(tab: &mut Tab) -> bool {
    tab.history.end_coalesce();
    tab.session.rollback_shape_drag();
    if !commit_floating_except(tab, &[]) {
        return false;
    }
    tab.session.selection.clear();
    tab.session.transform_pivot = None;
    tab.view.inspect_timeline_frame = false;
    true
}
