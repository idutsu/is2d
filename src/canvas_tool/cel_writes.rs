//! レイヤーの解決・ドラッグの取り消し・セル画を書く入口（`write_cel`）と、浮いた片の確定（`plan_commit`・
//! `write_commit`・`commit_floating_except`・`clear_selection`）

use super::*;

/// 編集を断った理由（`Session::refusal`。`apply` が知らせる）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// 地図にできない形（格子の外 ±65,536px・有限でない座標・潰れるほど縮める変換：`PlanarMap::fits_grid`）を作る
    OffGrid,
    /// 浮いた片を置けない（置いた結果を地図に書けない・置くレイヤーを編集できない）。浮いた片は残す
    Unplaceable,
    /// 地図の編集の結果が正しい地図にならない（書かない。文書は前のまま）
    Unwritable,
    /// 持ち上げた絵の出どころ（持ち上げた元のセル画の地図）に、確定のほかで書こうとした（`write_cel` の見張り。操作ごとの表
    /// `Action::policy` が正しければ起きない）
    UnderLifted,
    /// 開いたときに壊れていて、何も無い地図として見せている階層に書こうとした（`write_cel` の見張り：`Cel::keeps_blank_maps`。
    /// 書けば、ファイルから読んだ元の地図を黙って置き換える：審査 5616fcc の D-3）
    BlankPicture,
}

/// 1本の道が断った理由の、知らせ（読み替えはこれ1つ）
impl From<crate::planar_map::LocalRefusal> for Refusal {
    fn from(r: crate::planar_map::LocalRefusal) -> Self {
        match r {
            crate::planar_map::LocalRefusal::OffGrid => Refusal::OffGrid,
            crate::planar_map::LocalRefusal::Unwritable => Refusal::Unwritable,
        }
    }
}

impl Refusal {
    /// 画面の下に出す知らせ
    pub(crate) fn message(self) -> String {
        match self {
            Refusal::OffGrid => "地図の範囲（±65,536px）の外や、潰れるほど小さくには置けません".to_owned(),
            Refusal::Unplaceable => format!("選んだ図形をここに置けません。{}", undo_exit_text()),
            Refusal::Unwritable => "この編集の結果を地図にできないので、何も変えていません".to_owned(),
            Refusal::UnderLifted => "持ち上げている絵の下の絵には書けないので、何も変えていません".to_owned(),
            Refusal::BlankPicture => "壊れていて表示できない絵には書けないので、何も変えていません".to_owned(),
        }
    }
}

/// 持ち上げた絵を置けないときの出口の案内（確定を断ったとき・保存や書き出しを断るときの知らせに付ける）。出口は、もと
/// からある取り消し：取り消しは確定を断られたときも効き、動かす前に戻る（新しい操作は作らない：持ち主が決めた
/// 2026-11-27。審査 5616fcc の D-2）。キーは表から出す
pub(crate) fn undo_exit_text() -> String {
    let keys = crate::menu::keys_text(|a| {
        matches!(a, crate::panels::UiAction::Doc(crate::doc_actions::Action::Undo))
    });
    format!("取り消し（{keys}）で動かす前に戻せます")
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

/// セル画を書いたあとの後始末。セル画を書く入口（`write_cel`）のあとに、書いたレイヤーの一覧
/// `lids` で1回だけ呼ぶ：画面の土台の、書いたレイヤーを作り直す（編集中のシンボルの版は、書いた所で上がっている：
/// `Tab::write_active_timeline`）。編集中の階層が無くなっていれば、ある所まで戻る
pub(crate) fn finish_cel_writes(tab: &mut Tab, lids: &[LayerId]) {
    for &lid in lids {
        tab.session.base_cache.invalidate_layer(lid);
    }
    tab.refresh_scene_base();
    tab.session.sanitize_editing_scope();
}

/// 今のコマの、レイヤー `layer_id` のセル画を `f` で書き換える（`f` が `true` を返したら書いた）。後始末はしないので、
/// 書いたあとに、書いたレイヤーで `finish_cel_writes` を1回呼ぶ。`false`：編集できない（ロック・非表示・キーフレームが
/// 無い・トゥイーンの途中）か、`f` が書かなかったか、持ち上げた絵の出どころを書き換えようとした（書かずに断りを記す：
/// `Refusal::UnderLifted`）。セル画を書く道はみなここを通るので、持ち上げた絵の出どころ（そのレイヤー・編集中の階層の、
/// 持ち上げた元の地図）を、確定のほかで書き換えることは無い（確定は浮いた片をセッションから取り出してから書く。設計
/// claude/約束を入口で守る設計_5616fcc.md の 2.。前は操作ごとに先に確定する約束で、インクボトルが書き忘れた：審査
/// 5616fcc の D-1）
pub(crate) fn write_cel(
    tab: &mut Tab,
    layer_id: LayerId,
    f: impl FnOnce(&mut crate::timeline::Cel) -> bool,
) -> bool {
    let tl = tab.active_timeline();
    let frame = tl.current_frame;
    let Some(li) = tl
        .layers
        .iter()
        .position(|l| l.id == layer_id && l.is_editable_at(frame))
    else {
        return false;
    };
    let Some(kf) = tl.layers[li].keyframe_index_at(frame) else {
        return false;
    };
    let mut cel = tl.layers[li].keyframes[kf].cel.clone();
    let scope = &tab.session.editing_group_path;
    let lifted_from = tab
        .session
        .floating()
        .is_some_and(|fl| fl.layers().iter().any(|l| l.layer_id == layer_id && l.origin.is_some()))
        .then(|| cel.scope_stored(scope).map(crate::stored_map::StoredMap::id));
    if !f(&mut cel) {
        return false;
    }
    if let Some(from) = lifted_from {
        if cel.scope_stored(&tab.session.editing_group_path).map(crate::stored_map::StoredMap::id) != from {
            tab.session.refusal = Some(Refusal::UnderLifted);
            return false;
        }
    }
    if !cel.keeps_blank_maps(&tab.active_timeline().layers[li].keyframes[kf].cel) {
        tab.session.refusal = Some(Refusal::BlankPicture);
        return false;
    }
    tab.write_active_timeline(|tl, _| tl.layers[li].keyframe_mut(kf).cel = cel);
    true
}

/// 今のコマの、レイヤー `layer_ids` のセル画をそれぞれ `f(レイヤー, セル画, 写しの入れ物)` で書き換え（`write_cel`）、
/// 書いたレイヤーで後始末をする（`finish_cel_writes`）。`f` が入れ物に入れた新しい地図の計算に使う写しは、画面の土台が
/// 持つ（作り直すときに引き継ぐ。後始末の前に渡す）。`f` の `Ok(false)`：そのレイヤーは変わらない。`f` が断ったら
/// （`Err`）、そこで止めて知らせを記し（`Session::refusal`）、`false` を返す（それまでに書いたレイヤーは、操作が断った
/// ので `apply` が操作の前に戻す：`restore_state_before_action`）。`false`：どのレイヤーも書かなかったか、断った
pub(crate) fn write_cels(
    tab: &mut Tab,
    layer_ids: &[LayerId],
    f: impl Fn(
        LayerId,
        &mut crate::timeline::Cel,
        &mut Vec<Arc<crate::planar_map::LayerBase>>,
    ) -> Result<bool, Refusal>,
) -> bool {
    let mut written = Vec::new();
    let mut refusal = None;
    for &lid in layer_ids {
        let mut held = Vec::new();
        if write_cel(tab, lid, |cel| match f(lid, cel, &mut held) {
            Ok(changed) => changed,
            Err(r) => {
                refusal = Some(r);
                false
            }
        }) {
            written.push(lid);
        }
        for base in held {
            tab.session.base_cache.hold(lid, base);
        }
        if refusal.is_some() || tab.session.refusal.is_some() {
            break;
        }
    }
    finish_cel_writes(tab, &written);
    if refusal.is_some() || tab.session.refusal.is_some() {
        tab.session.refusal = refusal.or(tab.session.refusal);
        return false;
    }
    !written.is_empty()
}

/// 今のコマの、レイヤー `layer_ids` のセル画の、番号が `pick` に合うインスタンスの結び付けを `f` で書き換え、後始末を
/// する（`Cel::update_instances`・`write_cels`）。数が有限でない結び付けになるレイヤーがあれば断る（`Refusal::OffGrid`。
/// 全部か無しかは、断った操作を `apply` が操作の前に戻すことで守る：`restore_state_before_action`。前は先に全レイヤーを
/// 写して試してから書いたので、同じ判定を2回していた）。`false`：どのレイヤーも書かなかったか、断った
pub(crate) fn update_instances(
    tab: &mut Tab,
    layer_ids: &[LayerId],
    pick: impl Fn(u64) -> bool,
    f: impl Fn(&mut crate::vector_engine::SymbolInstanceBinding),
) -> bool {
    write_cels(tab, layer_ids, |_, cel, _| {
        cel.update_instances(&pick, &f).map_err(|_| Refusal::OffGrid)
    })
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
    if let Some(lid) = tab.scene().shape_by_id(shape_id).map(|s| s.layer_id) {
        focus_active_layer_by_id(tab, lid);
    }
}

/// レイヤー `lid` へ編集中タイムラインの `active_layer` およびタイムライン選択状態を追従させる
pub(crate) fn focus_active_layer_by_id(tab: &mut Tab, lid: LayerId) {
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

/// タイムラインを変えたあとの後片付け（選択・浮いた片・中心点はコマごとのもの）。浮いた片は捨てずに貼り戻し（前は捨てた
/// ので、呼ぶ前に貼り戻すのを忘れると、動かした絵が黙って消えた：約束の棚卸し 5616fcc の B の B4）、断られたら `false`
/// （浮いた片はそのまま。入口 `apply` は、表のとおりに操作の前に貼り戻してあるので、ふつうは貼り戻すものが無い）
#[must_use]
pub(crate) fn reset_session_after_timeline_change(tab: &mut Tab) -> bool {
    if !commit_floating(tab) {
        return false;
    }
    tab.session.transform_pivot = None;
    tab.session.sanitize_editing_scope();
    true
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

/// 浮いた片の確定で書くもの（`plan_commit`）：レイヤーごとの、1本の道の書くもの（`LocalWrite`）と、それを作った前の
/// 地図の写し（升目の索引を、変わった所だけ作り直すため：`Cel::put_local`）。変わるレイヤーだけ
pub(crate) type CommitPlan = Vec<(
    LayerId,
    crate::planar_map::LocalWrite,
    Arc<crate::planar_map::LayerBase>,
)>;

/// 浮いた片 `fl` を確定するときに書くもの（文書を変えない）：レイヤーごとに、残りの地図（無ければセル画の地図）に片の組
/// を下から順に重ねた地図を、今の階層の地図にするとき何を書くか（確定の計算の1つ `FloatLayer::commit_edit`：1本の道の答え `LayerBase::replace_local`）。重ねる
/// 計算から何を書くかまでをレイヤーごとに同時に作る（レイヤーをまたがないので、待ち時間はいちばん重いレイヤー1つ分）。
/// 全部か無しか：1つでも書けない（編集できないレイヤー・持ち上げた絵と違うセル画（`FloatLayer::still_on`）を含む）なら
/// `None` を返して知らせる（浮いた片を残すのは呼ぶ側。
/// 片はいつも文書か浮いた片のどちらかにあり、黙って消えない）。片の組の変換はかける前に確定と同じ式で確かめてある（変形
/// は `TransformFit::fits`、新しい中身は `prepare_placement`）
pub(crate) fn plan_commit(tab: &mut Tab, fl: &Floating) -> Option<CommitPlan> {
    use crate::planar_map::LocalEdit;
    // 画面の土台はいつも最新（`apply` の入口で作り直し、文書を変えた所は後始末で作り直す。Undo・保存の写しは確定の前に
    // 作り直す）ので、ここでは作り直さない
    #[cfg(test)]
    assert!(
        tab.scene_base_is_fresh(),
        "浮いた片を確定するとき、画面の土台が古い"
    );
    let editable = editable_layer_ids(tab);
    // 複製のドラッグを始めるときに同じ計算をしてあれば、それを使い回す（`Session::take_ahead_commit`。同じ浮いた片・
    // 選択・階層・セル画の地図のときだけ）
    if fl.layers().iter().all(|l| editable.contains(&l.layer_id)) {
        if let Some(plan) = tab.session.take_ahead_commit(fl) {
            return Some(plan);
        }
    }
    // 1本の道（設計 段2b の 11. の F2）：片を窓で重ねたしまう形（`committed_local`）とセル画のしまう形の違いを差分にして、
    // 差分から先（`apply_diff`）に渡す（`Floating::commit_edits`。持ち上げた絵（出どころ）でないセル画には書かない：
    // `FloatLayer::still_on`、設計審査 de92ceb の D-3）
    let outcomes = fl.commit_edits(tab.session.stage());
    let mut plan = CommitPlan::new();
    for (l, outcome) in fl.layers().iter().zip(outcomes) {
        match outcome {
            Some((LocalEdit::Same, _)) if editable.contains(&l.layer_id) => {}
            Some((LocalEdit::Changed(w), base)) if editable.contains(&l.layer_id) => {
                plan.push((l.layer_id, w, base));
            }
            _ => {
                tab.session.refusal = Some(Refusal::Unplaceable);
                return None;
            }
        }
    }
    Some(plan)
}

/// `plan_commit` で決めたものを、今の階層の地図に書く（`write_maps`）。戻り値は、セル画を書き換えたか（`None`：断った）
#[must_use]
pub(crate) fn write_commit(tab: &mut Tab, plan: CommitPlan) -> Option<bool> {
    let scope = tab.session.editing_group_path.clone();
    write_maps(tab, &scope, plan)
}

/// 先に決めた地図と面の図形 `plan`（レイヤーごと）を、階層 `scope` の地図に書く。戻り値は、セル画を書き換えたか。計画が
/// 今の地図から作ったものでなければ（`Cel::put_local` が断る）、そこで止めて知らせを記し（`Session::refusal`）、`None` を
/// 返す（それまでに書いたレイヤーは、断った操作を `apply` が操作の前に戻す）
#[must_use]
pub(crate) fn write_maps(tab: &mut Tab, scope: &[u64], plan: CommitPlan) -> Option<bool> {
    let mut written: Vec<LayerId> = Vec::new();
    for (layer_id, w, prev) in plan {
        let mut held = None;
        let mut refused = None;
        if write_cel(tab, layer_id, |cel| match cel.put_local(scope, w, &prev) {
            Ok(base) => {
                held = base;
                true
            }
            Err(r) => {
                refused = Some(r);
                false
            }
        }) {
            written.push(layer_id);
        }
        if let Some(r) = refused {
            tab.session.refusal = Some(r.into());
            finish_cel_writes(tab, &written);
            return None;
        }
        // 新しい地図の写し（面の図形は、変わらない面は前の図形）は、画面の土台が持つ
        if let Some(base) = held {
            tab.session.base_cache.hold(layer_id, base);
        }
    }
    finish_cel_writes(tab, &written);
    Some(!written.is_empty())
}

/// 浮いた片 `fl` を確定する（`plan_commit`・`write_commit`）。`None`：書けない（何も書かず、知らせを記す）。`Some`：
/// セル画を書き換えたか
pub(crate) fn commit_floating_layers(tab: &mut Tab, fl: &Floating) -> Option<bool> {
    let plan = plan_commit(tab, fl)?;
    write_commit(tab, plan)
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
        if fl.modified() && commit_floating_layers(tab, &fl).is_none() {
            tab.session.set_floating(Some(fl));
            return false;
        }
        tab.session.retain_existing_selection();
        return true;
    }

    let before = tab.session.selection.clone();
    tab.session
        .set_selected_indices((0..is_keep.len()).filter(|&i| is_keep[i]));
    let pieces = tab.session.floating_view().pieces();
    let keep: HashSet<ShapeId> = pieces
        .iter()
        .filter(|p| tab.session.selection.contains_shape(p))
        .map(|p| p.shape_id)
        .collect();
    let all_kept = keep.len() == pieces.len();
    let Some(mut fl) = tab.session.take_floating() else {
        return true;
    };

    if !fl.modified() {
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
                tab.session.restore_selection(before);
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
    let to_commit = match merged_any
        .then(|| fl.split_off(&keep, &tab.session.editing_group_path))
        .transpose()
    {
        Ok(to_commit) => to_commit,
        Err(_) => {
            tab.session.set_floating(whole);
            tab.session.restore_selection(before);
            tab.session.refusal = Some(Refusal::Unplaceable);
            return false;
        }
    };
    if let Some(to_commit) = &to_commit {
        if commit_floating_layers(tab, to_commit).is_none() {
            tab.session.set_floating(whole);
            tab.session.restore_selection(before);
            return false;
        }
        // 持ったまま残る置かない組の残りを、書いたあとのセル画に合わせて作り直す
        if !keep.is_empty() && !fl.rebase_after_commit(tab.session.stage(), to_commit) {
            tab.session.set_floating(whole);
            tab.session.restore_selection(before);
            tab.session.refusal = Some(Refusal::Unplaceable);
            return false;
        }
    }
    if !keep.is_empty() {
        tab.session.set_floating(Some(fl));
    }
    // 残す片から覆われた所を消すと、分かれた片に新しい番号が付くことがある（`Floating::split_off`。選び直しは
    // `Session::sync_floating`）
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
    tab.session.rollback_shape_drag();
    if !commit_floating_except(tab, &[]) {
        return false;
    }
    tab.session.replace_selection(Selection::default());
    true
}
