use std::collections::HashSet;

use kurbo::{Affine, Point, Vec2 as KVec2};

use crate::canvas_tool::{
    active_scope_layer_id, clear_selection,
    editable_layer_ids, finish_cel_writes, reset_session_after_timeline_change, update_instances,
    write_cel, write_cels, Tab, View,
};
use crate::clipboard_ops::{
    content_xform, prepare_placement, put_placements, selected_content, Content,
};
use crate::symbol::{EditLevel, SymbolDef, SymbolInstance, SymbolInstanceBinding};
use crate::timeline::{Cel, TimelineCellSelection};
use crate::vector_engine::{invertible, union_bounding_boxes};

/// 選択中のオブジェクトを新しい「シンボル」に変換するためのローカル図形抽出・インスタンスバインディング付与・ライブラリ登録を行い、
/// `(sym_id, target_lid)` を返す
pub(in crate::canvas_tool) fn prepare_convert_selection_to_symbol(
    tab: &mut Tab,
) -> Option<(u64, u64)> {
    if tab.session.selection().is_empty() {
        return None;
    }
    let sel_scene_indices = tab.session.selected_indices();
    if sel_scene_indices.is_empty()
        || !tab
            .scene()
            .selected_has_any_in_scope(tab.session.selection(), tab.session.editing_group_path())
    {
        return None;
    }

    // シンボルにできるのは、1つのレイヤーの中で選んだものだけ（グループ化と同じ決まり：`selected_single_layer_id`）
    let target_lid = crate::select_ops::selected_single_layer_id(tab)?;
    if !editable_layer_ids(tab).contains(&target_lid) {
        return None;
    }
    let scope = tab.session.editing_group_path().to_vec();
    let selection = tab.session.selection().clone();
    // シンボルの中身は、文書を変える前に、選んでいるものの中身（地図とインスタンス：`selected_content`。浮いた片は
    // 確定と同じ重ね方）から作る：そのレイヤーの中身の、外接矩形の中心を原点へ寄せる（`Cel::transformed`）。地図に
    // できなければ断る。インスタンスは、そのレイヤーに置く
    let mut content = selected_content(tab)?;
    content.layers.retain(|(id, _)| *id == target_lid);
    let center = content_center(tab, &content);
    let (_, cel) = content.layers.pop()?;
    let Some(cel) = cel.transformed(Affine::translate(-center.to_vec2())) else {
        tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
        return None;
    };
    let mut sym_def = SymbolDef::from_cel(0, String::new(), cel);

    // 選んだ生の図形は先に片に持ち上げる（グループ化と同じ：`prepare_group_selected_shapes`）。選んだ生の図形は片にだけ
    // あり、残り（確定する分）には無いので、残りを書いても選んだ図形が今の階層に戻らない（選び方によらない。持ち上げずに
    // 進むと、浮いた片があるとき、残りの確定が選んだ図形を外す前のセル画から作った地図で今の階層を書き直し、図形が
    // インスタンスの下に二重に残っていた：レビュー 5b822a3 の N-4003）。持ち上げられなければ断る
    let before = tab.session.floating().cloned();
    if !tab.session.lift_selected_raw_shapes_to_floating(&tab.stage()) {
        tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
        return None;
    }
    // 浮いた片：選んだ片はシンボルの中身に入れたので捨て、残りと選んでいない片を確定する。書くものを先に決め（書けなければ
    // 断る）、インスタンスを置いてから書く（今の階層の中身をみな選んでいても、今の階層の地図がまだあるうちにインスタンス
    // を入れるので、今の階層の重なりの位置が変わらない）。断るのは文書を変える前（採番・登録より前）：`apply` は `false`
    // なら文書は変わっていないものとして控えを捨て、採番を戻す。断るときは、浮いた片を持ち上げる前に戻す
    let keep: HashSet<u64> = tab
        .session
        .floating_view()
        .pieces()
        .iter()
        .filter(|p| selection.contains_shape(p))
        .map(|p| p.shape_id)
        .collect();
    let rest_plan = match tab.session.take_floating() {
        Some(mut fl) => {
            // シンボルの中身に入れる片は、元の所から取り出す（元の所は切り抜いたまま書く：`Floating::take_out`）
            let Ok(rest) = fl.take_out(&keep, tab.session.editing_group_path()) else {
                tab.session.set_floating(before, &tab.stage());
                tab.session.refusal = Some(crate::canvas_tool::Refusal::Unplaceable);
                return None;
            };
            let Some(plan) = crate::canvas_tool::plan_commit(tab, &rest) else {
                tab.session.set_floating(before, &tab.stage());
                return None;
            };
            Some(plan)
        }
        None => None,
    };
    let sym_id = tab.doc.alloc_symbol_id();
    let inst_gid = tab.doc.alloc_group_id();
    // シンボルは、インスタンスを置く前に登録する（置いたインスタンスの図形が、今の階層の中身として画面に出る）
    sym_def.id = sym_id;
    tab.doc.register_symbol(sym_def);
    // インスタンスを、置くレイヤーの今の階層のいちばん上に入れ（中心に置く）、選んだもの（今の階層の）を外す
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
    // 持ち上げた片（`keep`）は外さない：今の階層の地図は、浮いた片の残りの確定（`rest_plan`）が書く（片を切り抜いた地図
    // から作ってある）。ここで外すと、計画を作ったときの地図と違う地図に書くことになり、書く入口が断る（`Cel::put_local`：
    // 約束の棚卸し 5616fcc の A の P-07）。外すのは、持ち上げない深い階層の図形（選んだグループの中）とインスタンス
    let deeper: HashSet<u64> = selection.shapes.difference(&keep).copied().collect();
    write_cels(tab, &[target_lid], |_, cel, held| {
        Ok(cel.remove(&scope, &deeper, &selection.instances, held)?)
    });
    // 書けなければ断る（それまでに書いたもの・登録したシンボルは、断った操作を `apply` が操作の前に戻す）
    if tab.session.refusal.is_some() {
        return None;
    }
    if let Some(plan) = rest_plan {
        crate::canvas_tool::write_commit(tab, plan)?;
    }

    let mut placed = crate::canvas_tool::Selection::default();
    placed.instances.insert(inst_gid);
    tab.session.replace_selection(placed);

    Some((sym_id, target_lid))
}

/// 中身 `content` の外接矩形の中心（今いる階層の文書の座標）：地図の辺の曲線と、インスタンスの図形（画面の図形を文書の
/// 座標へ戻したもの）の外接矩形を合わせたもの。形が何も無ければ原点
fn content_center(tab: &Tab, content: &Content) -> Point {
    let world = content.world;
    let instances: HashSet<u64> = content
        .layers
        .iter()
        .flat_map(|(_, c)| c.instances().iter().map(|i| i.binding.instance_gid))
        .collect();
    let maps = content.layers.iter().filter_map(|(_, c)| c.map_bounds());
    let insts = tab
        .session
        .base_cache()
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

/// ライブラリの「入替」で、シンボル `symbol_id` に入れ替えるインスタンスの番号：キャンバスで選んでいるインスタンスの
/// うち、編集できるレイヤーにあって、まだ別のシンボルのもの（一緒に選んだ図形・グループは入れない。何も選んでいなければ
/// 空）。今の階層に `symbol_id` を置くと入れ子が回るとき（「配置」と同じ決まり）も空
pub(crate) fn swap_targets(tab: &Tab, symbol_id: u64) -> HashSet<u64> {
    let selected = &tab.session.selection().instances;
    if selected.is_empty()
        || !tab
            .doc
            .can_place_symbol_in_current_context(tab.session.edit_path(), symbol_id)
    {
        return HashSet::new();
    }
    let layer_ids = editable_layer_ids(tab);
    tab.scene()
        .iter()
        .filter(|s| layer_ids.contains(&s.layer_id))
        .filter_map(|s| s.symbol_instance)
        .filter(|b| b.symbol_id != symbol_id && selected.contains(&b.instance_gid))
        .map(|b| b.instance_gid)
        .collect()
}

/// キャンバスで選んでいるインスタンス（`swap_targets`）を、シンボル `symbol_id` に入れ替える（Adobe Animate の「入れ替え」）。
/// 位置・大きさ・回転・中心点・色効果・再生設定はそのままで、シンボルだけを変える。選択はそのまま
pub(in crate::canvas_tool) fn prepare_swap_selected_instances(
    tab: &mut Tab,
    symbol_id: u64,
) -> bool {
    // 浮いた片は入口が先に貼り戻してある（操作ごとの表 `Action::policy`）
    let targets = swap_targets(tab, symbol_id);
    if targets.is_empty() {
        return false;
    }
    let layer_ids = editable_layer_ids(tab);
    let updated = update_instances(
        tab,
        &layer_ids,
        |gid| targets.contains(&gid),
        |b| b.symbol_id = symbol_id,
    );
    updated && reset_session_after_timeline_change(tab)
}

/// ライブラリ内の指定シンボル `symbol_id` の新しいインスタンスを現在のステージ中央へ配置する
pub(in crate::canvas_tool) fn prepare_place_symbol_instance(
    tab: &mut Tab,
    symbol_id: u64,
) -> bool {
    let target_lid = active_scope_layer_id(&tab.session, tab.active_timeline());
    if !tab
        .doc
        .can_place_symbol_in_current_context(tab.session.edit_path(), symbol_id)
        || !tab.active_timeline().is_layer_editable_by_id(target_lid)
    {
        return false;
    }
    let Some(first_shapes) = tab
        .doc
        .find_symbol(symbol_id)
        .map(|s| s.first_non_empty_local_shapes(tab.doc.library()))
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
        tab.session.edit_path().world_xform(),
        KVec2::ZERO,
    );
    let Some(placement) = prepare_placement(tab, &content, xform, false, None) else {
        tab.session.refusal = Some(crate::canvas_tool::Refusal::OffGrid);
        return false;
    };
    if !clear_selection(tab) {
        return false;
    }
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
        .selected_single_symbol_binding(tab.session.selection())
        .or_else(|| {
            let layer = tl.layers.get(tl.active_layer)?;
            if !layer.is_editable_at(tl.current_frame) {
                return None;
            }
            let mut b = layer.cel_at(tl.current_frame)?.single_symbol_instance()?;
            b.xform = tab.session.edit_path().world_xform() * b.xform;
            Some(b)
        })?;
    let sym = tab.doc.find_symbol(binding.symbol_id)?;
    Some((binding, sym.name.clone(), sym.timeline.total_frames.max(1)))
}

/// 今のコマの、編集中の階層のインスタンス `instance_gid` の結び付け（親の座標。トゥイーンの途中は補間したもの）。見えて
/// いるレイヤーから探す
fn instance_binding_now(tab: &Tab, instance_gid: u64) -> Option<SymbolInstanceBinding> {
    let tl = tab.active_timeline();
    let f = tl.current_frame;
    tl.layers.iter().filter(|l| l.visible).find_map(|layer| {
        if let Some(b) = layer
            .interpolated_tween_binding_at(f)
            .filter(|b| b.instance_gid == instance_gid)
        {
            return Some(b);
        }
        layer
            .cel_at(f)?
            .instances()
            .iter()
            .find(|i| i.binding.instance_gid == instance_gid)
            .map(|i| i.binding)
    })
}

/// シンボルインスタンス `instance_gid` をダブルクリックして、そのシンボル専用の内部タイムライン編集モードへ入る。変換は
/// タイムラインの結び付け（親の座標）をそのまま使う（画面の座標から取り戻さない：レビュー f01d518 の N-4006）。今のコマに
/// 無ければ断る
pub(in crate::canvas_tool) fn enter_symbol_timeline(tab: &mut Tab, instance_gid: u64) -> bool {
    match instance_binding_now(tab, instance_gid) {
        Some(b) => enter_symbol_level(tab, instance_gid, b.symbol_id, b.xform),
        None => false,
    }
}

/// シンボル `symbol_id` の段に入る（インスタンス `instance_gid`。ライブラリから入るときは 0）。`xform_in_parent` は親の
/// 座標での変換。逆にできない変換・浮いた片の確定が断られたときは断る
fn enter_symbol_level(
    tab: &mut Tab,
    instance_gid: u64,
    symbol_id: u64,
    xform_in_parent: Affine,
) -> bool {
    if tab.doc.find_symbol(symbol_id).is_none() {
        return false;
    }
    let (parent_frame, parent_active_layer) = {
        let tl = tab.active_timeline();
        (tl.current_frame, tl.active_layer)
    };

    let next_world = tab.session.edit_path().world_xform() * xform_in_parent;
    if !invertible(xform_in_parent) || !invertible(next_world) {
        return false;
    }

    // シンボルの段に入る（見る場所を変える入口：先に持ち上げた絵を貼り戻す）
    if !tab.move_view(|edit_path, group_path| {
        edit_path.levels.push(EditLevel {
            instance_gid,
            symbol_id,
            xform_in_parent,
            parent_frame,
            parent_active_layer,
            parent_group_path: std::mem::take(group_path),
        });
    }) {
        return false;
    }
    tab.session.replace_selection(crate::canvas_tool::Selection::default());
    tab.view.select_library_symbol(symbol_id);

    let (act_l, act_f) = {
        let tl = tab.active_timeline();
        (
            tl.active_layer.min(tl.layers.len().saturating_sub(1)),
            tl.current_frame.min(tl.total_frames.saturating_sub(1)),
        )
    };
    tab.set_active_layer(act_l);
    // 持ち上げた絵は段に入るときに貼り戻してあるので、コマを変える入口は断らない
    let moved = tab.set_active_frame(act_f);
    debug_assert!(moved);
    tab.set_active_cell_selection(Some(TimelineCellSelection::single(act_l, act_f)));
    reset_session_after_timeline_change(tab)
}

/// ライブラリのシンボル `symbol_id` を、どのインスタンスにも結び付けずに編集する（Adobe Animate の「シンボル編集」）。
/// いったんシーンまで出てから入り、周りの絵もステージも見せない（`EditPath::shows_stage`・`first_level_showing_parent`）。
/// 基準点はステージの真ん中に置く。いくつ置いてあっても、どれか1つを選んでその場で開くことはしない
pub(in crate::canvas_tool) fn enter_symbol_from_library(tab: &mut Tab, symbol_id: u64) -> bool {
    if tab.doc.find_symbol(symbol_id).is_none() {
        return false;
    }
    while !tab.session.edit_path().is_empty() {
        if !exit_symbol_timeline(tab) {
            return false;
        }
    }
    let center = View::DEFAULT_STAGE_CENTER;
    enter_symbol_level(
        tab,
        0,
        symbol_id,
        Affine::translate(KVec2::new(center.x, center.y)),
    )
}

/// 現在入っているシンボル内部タイムライン編集モードから1段階上の親シーン（または親シンボル）タイムラインへ戻る
pub(in crate::canvas_tool) fn exit_symbol_timeline(tab: &mut Tab) -> bool {
    if tab.session.edit_path().is_empty() {
        return false;
    }
    // シンボルの段から出る（見る場所を変える入口：先に持ち上げた絵を貼り戻す）。親のグループの階層は、コマを親に戻して
    // から入れる
    let mut popped = None;
    if !tab.move_view(|edit_path, _| popped = edit_path.levels.pop()) {
        return false;
    }
    tab.session.replace_selection(crate::canvas_tool::Selection::default());
    let Some(lvl) = popped else {
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
    let moved = tab.set_active_frame(act_f)
        && tab.move_view(|_, group_path| *group_path = lvl.parent_group_path);
    debug_assert!(moved);
    tab.set_active_cell_selection(Some(TimelineCellSelection::single(act_l, act_f)));
    if !reset_session_after_timeline_change(tab) {
        return false;
    }

    tab.session.select_symbol_instance_by_gid(lvl.instance_gid);
    true
}
