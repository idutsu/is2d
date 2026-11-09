use std::collections::HashSet;

use kurbo::{ParamCurve, ParamCurveNearest, Point, QuadBez, Rect as KRect};

use crate::canvas_tool::{
    cancel_active_drag, clear_selection, commit_floating, commit_floating_except, edit_layers,
    editable_layer_ids, merge_floating_into_base_shapes, sync_timeline_layer_from_canvas_selection,
    ActiveCanvasDrag, Floating, HoverTarget, LayerId, Scene, SelectSubMode, Session, Tab,
};
use crate::doc_actions::{apply, Action, ActionHost};
use crate::symbol_ops::{enter_symbol_timeline, exit_symbol_timeline};
use crate::vector_engine::{
    make_marquee_rect_shape, next_shape_id, rects_overlap, ColorEffect, FillShape,
    SymbolInstanceBinding,
};

/// 現在の選択図形のうち `keep_indices` のみを選択に残し、選択から外れた変更済み生シェイプのみをキャンバスへ結合・切り抜き確定する
/// （選択に残った図形は結合せず `Floating` のまま維持する）
pub(crate) fn retain_selection_and_flush_deselected(tab: &mut Tab, keep_indices: &[usize]) {
    let (is_sel, any_valid) = tab.session.selected_shape_mask();
    if !any_valid {
        clear_selection(tab);
        return;
    }
    let any_keep = keep_indices
        .iter()
        .any(|&idx| idx < is_sel.len() && is_sel[idx]);
    if !any_keep {
        clear_selection(tab);
        return;
    }

    commit_floating_except(tab, keep_indices);
    tab.session.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(tab);
}

/// ダブルクリック位置 `pt` にシンボルがあればそのシンボルの独自タイムラインへ入り、グループがあればインプレイス編集モードへ移行する
pub(crate) fn enter_group_at(tab: &mut Tab, pt: Point) -> bool {
    let scene = tab.scene();
    let Some(hit_idx) = scene.topmost_shape_at(pt, &tab.session.editing_group_path) else {
        return false;
    };
    let Some(hit_shape) = scene.get(hit_idx) else {
        return false;
    };
    let Some(gid) = hit_shape.active_group_id_at_scope(&tab.session.editing_group_path) else {
        return false;
    };
    let binding_opt = hit_shape.symbol_instance.filter(|b| b.instance_gid == gid);
    if let Some(binding) = binding_opt {
        return enter_symbol_timeline(tab, binding);
    }
    clear_selection(tab);
    tab.session.editing_group_path.push(gid);
    true
}

/// 現在入っているグループまたはシンボルタイムライン編集階層から1段階上の親階層へ戻る
pub(crate) fn exit_current_group(tab: &mut Tab) -> bool {
    if !tab.session.editing_group_path.is_empty() {
        clear_selection(tab);
        let Some(exited_gid) = tab.session.editing_group_path.pop() else {
            return false;
        };
        let scene = tab.scene();
        let indices: Vec<usize> = (0..scene.len())
            .filter(|&i| {
                scene
                    .get(i)
                    .and_then(|s| s.active_group_id_at_scope(&tab.session.editing_group_path))
                    == Some(exited_gid)
            })
            .collect();
        tab.session.set_selected_indices(indices);
        tab.session.transform_pivot = None;
        return true;
    }
    exit_symbol_timeline(tab)
}

/// キャンバス上のダブルクリックによるグループ/シンボルへのインプレイス進入、または空白背景ダブルクリックによる親階層への復帰を処理する
pub(crate) fn handle_canvas_double_click(host: &mut impl ActionHost, pt: Point) {
    if !apply(host, Action::EnterGroupAt { pt })
        && host
            .tab()
            .scene()
            .topmost_shape_at(pt, &host.tab().session.editing_group_path)
            .is_none()
    {
        apply(host, Action::ExitGroupSteps { steps: 1 });
    }
}

/// クリックした点 `pt` の図形に応じて選択を切り替える（選択ツール・自由変形ツール共通）
pub(crate) fn apply_click_selection(
    host: &mut impl ActionHost,
    pt: Point,
    shift_held: bool,
    deselect_on_miss: bool,
) {
    let sub_mode = host.tool().select_sub_mode;
    let zoom = host.tab().view.zoom;
    let hit_shape =
        host.tab()
            .scene()
            .hit_shape_or_hover_at(&host.tab().session, pt, zoom, sub_mode);
    match hit_shape {
        Some(scene_idx) if shift_held => {
            apply(host, Action::ToggleSelectionAt { scene_idx });
        }
        Some(scene_idx) if host.tab().session.is_selected_at(scene_idx) => {
            let unit = host
                .tab()
                .scene()
                .expand_to_group_unit(scene_idx, &host.tab().session.editing_group_path);
            apply(host, Action::RetainSelection { keep_indices: unit });
        }
        Some(_) => {
            apply(host, Action::SelectSingleAt { pt });
        }
        None if !shift_held && deselect_on_miss => {
            apply(host, Action::Deselect);
        }
        None => {}
    }
}

/// Shift クリック等で指定シェイプ（またはグループ塊全体）の選択状態をトグルし、変形ピボットをリセットする
pub(crate) fn toggle_shape_selection(tab: &mut Tab, scene_idx: usize) {
    tab.history.end_coalesce();
    let scene = tab.scene();
    let unit = scene.expand_to_group_unit(scene_idx, &tab.session.editing_group_path);
    let (sel_mask, _) = scene.selected_shape_mask(&tab.session.selection);
    if sel_mask.get(scene_idx).copied().unwrap_or(false) {
        let remaining: Vec<usize> = tab
            .session
            .selected_indices()
            .into_iter()
            .filter(|i| unit.binary_search(i).is_err())
            .collect();
        retain_selection_and_flush_deselected(tab, &remaining);
    } else if scene
        .get(scene_idx)
        .is_some_and(|s| tab.active_timeline().is_layer_editable_by_id(s.layer_id))
    {
        tab.session.select_shape_indices(unit);
        tab.session.transform_pivot = None;
        sync_timeline_layer_from_canvas_selection(tab);
    }
}

/// 既存の選択を確定解除した上で、指定座標 `pt` にある最前面シェイプ（塗り・境界線・頂点、またはグループ塊全体）を選択する
pub(crate) fn select_single_shape_at(tab: &mut Tab, sub_mode: SelectSubMode, pt: Point) -> bool {
    clear_selection(tab);
    let zoom = tab.view.zoom;
    let scene = tab.scene();
    if let Some(hit_idx) = scene
        .hit_shape_or_hover_at(&tab.session, pt, zoom, sub_mode)
        .filter(|&idx| {
            scene
                .get(idx)
                .is_some_and(|s| tab.active_timeline().is_layer_editable_by_id(s.layer_id))
        })
    {
        let unit = scene.expand_to_group_unit(hit_idx, &tab.session.editing_group_path);
        tab.session.set_selected_indices(unit);
        sync_timeline_layer_from_canvas_selection(tab);
        true
    } else {
        false
    }
}

/// 進行中の選択の移動・矩形選択のドラッグを `pt` へ進める（選択ツール・自由変形ツール共通）
pub(crate) fn update_selection_drag(tab: &mut Tab, pt: Point) -> bool {
    match &mut tab.session.drag {
        ActiveCanvasDrag::MoveSelection(drag) => {
            let delta = drag.step(pt);
            tab.session.translate_move_overlay(delta);
            true
        }
        ActiveCanvasDrag::Marquee(m) => {
            m.current_pos = pt;
            true
        }
        _ => false,
    }
}

/// 進行中の選択の移動・矩形選択のドラッグを確定する（離した位置 `mouse_pt` が無ければ最後のドラッグ位置）
pub(crate) fn finish_selection_drag(host: &mut impl ActionHost, mouse_pt: Option<Point>) -> bool {
    let action = match host.tab().session.drag {
        ActiveCanvasDrag::MoveSelection(ref drag) => Action::FinishMoveSelectionDrag {
            total_delta: drag.total_delta,
            alt_duplicate: drag.alt_duplicate,
        },
        ActiveCanvasDrag::Marquee(ref m) => Action::SelectMarquee {
            p0: m.start_pos,
            p1: mouse_pt.unwrap_or(m.current_pos),
            additive: m.additive,
        },
        _ => return false,
    };
    host.tab_mut().session.drag = ActiveCanvasDrag::None;
    apply(host, action);
    true
}

/// Adobe Animate 風の矩形選択（Marquee Selection）
pub(crate) fn apply_marquee_selection(tab: &mut Tab, p0: Point, p1: Point, additive: bool) {
    tab.history.end_coalesce();
    let min_x = p0.x.min(p1.x);
    let max_x = p0.x.max(p1.x);
    let min_y = p0.y.min(p1.y);
    let max_y = p0.y.max(p1.y);

    let min_marquee = 2.0 / (tab.view.zoom as f64);
    if (max_x - min_x) < min_marquee || (max_y - min_y) < min_marquee {
        if !additive {
            clear_selection(tab);
        }
        return;
    }

    if !additive {
        clear_selection(tab);
    }

    let rect = KRect::new(min_x, min_y, max_x, max_y);
    let clip_rect_shape = make_marquee_rect_shape(rect);
    let hit_group_ids =
        collect_marquee_hit_groups(tab.scene(), &tab.session.editing_group_path, rect);

    let editable_lids = editable_layer_ids(tab);
    let base_shapes = tab.session.base_cache.shapes().to_vec();
    let editing_group_path = tab.session.editing_group_path.clone();
    let mut selection = std::mem::take(&mut tab.session.selection);
    tab.session.update_floating(|floating| {
        let mut lifted = Vec::new();
        if let Some(fl) = floating {
            // すでに浮いている片の残り（前の矩形選択の外側）を、レイヤーごとに地図で切り直す（残りは同じ地図から作ったもの）
            let mut chunks = fl.take_remainder_chunks();
            let eligible = |s: &FillShape| {
                s.is_in_editing_scope(&editing_group_path) && s.is_raw_at_scope(&editing_group_path)
            };
            let mut rem_lids: Vec<u64> = Vec::new();
            for s in chunks
                .iter()
                .flat_map(|(_, c)| c.iter())
                .filter(|s| eligible(s))
            {
                if !rem_lids.contains(&s.layer_id) {
                    rem_lids.push(s.layer_id);
                }
            }
            for lid in rem_lids {
                let own: Vec<FillShape> = chunks
                    .iter()
                    .flat_map(|(_, c)| c.iter())
                    .filter(|s| s.layer_id == lid && eligible(s))
                    .cloned()
                    .collect();
                let Some(sp) = crate::planar_map::marquee_split(
                    &own,
                    lid,
                    &editing_group_path,
                    |_| false,
                    &clip_rect_shape,
                ) else {
                    continue;
                };
                let mut cut_chunk = None;
                for (ci, (_, chunk)) in chunks.iter_mut().enumerate() {
                    let mut kept = Vec::with_capacity(chunk.len());
                    for mut rem in chunk.drain(..) {
                        if rem.layer_id == lid && eligible(&rem) {
                            if sp.whole.contains(&rem.shape_id) {
                                if fl.hidden.contains(&rem.shape_id) {
                                    rem.shape_id = next_shape_id();
                                }
                                selection.insert_shape(&rem);
                                lifted.push(rem);
                                continue;
                            }
                            if sp.hidden.contains(&rem.shape_id) {
                                cut_chunk.get_or_insert(ci);
                                continue;
                            }
                        }
                        kept.push(rem);
                    }
                    *chunk = kept;
                }
                if let Some(ci) = cut_chunk {
                    chunks[ci].1.extend(sp.remainder);
                }
                for piece in sp.pieces {
                    selection.insert_shape(&piece);
                    lifted.push(piece);
                }
            }
            chunks.retain(|(_, c)| !c.is_empty());
            fl.set_remainder_chunks(chunks);
        }

        // 地図で切る：レイヤーごとに、そのレイヤーの生の図形を1つの地図にして矩形で切る（矩形の中の片と外の残りの境目が、
        // 両側でビットまで同じになる）。地図で切れないレイヤーは、今までどおり図形ごとに切る
        let hidden_now: HashSet<u64> = floating
            .as_ref()
            .map(|fl| fl.hidden.clone())
            .unwrap_or_default();
        let mut split_lids: Vec<u64> = Vec::new();
        for shape in &base_shapes {
            if !editable_lids.contains(&shape.layer_id)
                || !shape.is_raw_at_scope(&editing_group_path)
                || selection.contains_shape(shape)
                || hidden_now.contains(&shape.shape_id)
                || !rects_overlap(shape.bounding_box(), rect)
                || split_lids.contains(&shape.layer_id)
            {
                continue;
            }
            split_lids.push(shape.layer_id);
        }
        // レイヤーごとの切り分けは、ほかのレイヤーを見ないので同時に計算する
        let split_results = crate::planar_map::per_layer_parallel(&split_lids, |&lid| {
            crate::planar_map::marquee_split(
                &base_shapes,
                lid,
                &editing_group_path,
                |s| selection.contains_shape(s) || hidden_now.contains(&s.shape_id),
                &clip_rect_shape,
            )
        });
        let splits: Vec<(u64, Option<crate::planar_map::MarqueeSplit>)> =
            split_lids.into_iter().zip(split_results).collect();
        // 切った片と残りを入れ終えたレイヤー
        let mut lifted_layers: Vec<u64> = Vec::new();
        for shape in &base_shapes {
            if selection.contains_shape(shape)
                || floating
                    .as_ref()
                    .is_some_and(|fl| fl.hidden.contains(&shape.shape_id))
            {
                continue;
            }
            if !editable_lids.contains(&shape.layer_id)
                || !shape.is_in_editing_scope(&editing_group_path)
            {
                continue;
            }
            if let Some(gid) = shape.active_group_id_at_scope(&editing_group_path) {
                if hit_group_ids.contains(&gid) {
                    selection.insert_shape(shape);
                }
                continue;
            }
            let map_split = splits
                .iter()
                .find(|(lid, _)| *lid == shape.layer_id)
                .and_then(|(_, sp)| sp.as_ref())
                .filter(|_| shape.is_raw_at_scope(&editing_group_path));
            if let Some(sp) = map_split {
                if sp.whole.contains(&shape.shape_id) {
                    selection.insert_shape(shape);
                } else if sp.hidden.contains(&shape.shape_id) {
                    let orig_id = shape.shape_id;
                    let fl = floating.get_or_insert_with(|| Floating {
                        pieces: Vec::new(),
                        remainder: Vec::new(),
                        hidden: HashSet::new(),
                        modified: false,
                    });
                    fl.hidden.insert(orig_id);
                    if !lifted_layers.contains(&shape.layer_id) {
                        lifted_layers.push(shape.layer_id);
                        // そのレイヤーの残りは全部、最初に隠した図形の塊に入れる（生の図形どうしは重ならないので、
                        // 並びの位置は見た目に関係しない）
                        let mut remainder = sp.remainder.clone();
                        if let Some(first) = remainder.first_mut() {
                            first.shape_id = orig_id;
                        }
                        fl.remainder.extend(remainder);
                        for piece in &sp.pieces {
                            selection.insert_shape(piece);
                            lifted.push(piece.clone());
                        }
                    }
                }
                continue;
            }
            // 地図にできないレイヤー（格子に置けない座標がある）の図形は、切らずに、丸ごと矩形の中にあるときだけ選ぶ
            let b = shape.bounding_box();
            if b.x0 >= rect.x0 && b.x1 <= rect.x1 && b.y0 >= rect.y0 && b.y1 <= rect.y1 {
                selection.insert_shape(shape);
            }
        }
        if let Some(fl) = floating {
            fl.pieces.splice(0..0, lifted);
        }
    });
    tab.session.selection = selection;
    tab.session.merge_touching_pieces();

    tab.session.drop_unmodified_slice_if_unselected();
    tab.session.transform_pivot = None;
    sync_timeline_layer_from_canvas_selection(tab);
}

/// 2次ベジェ曲線の3制御点 `(p0, p1, p2)` の凸包AABB（余白 `margin` 付き）に点 `pt` が含まれるかを高速判定する
#[inline]
pub(crate) fn quad_control_bbox_contains(seg: &QuadBez, pt: Point, margin: f64) -> bool {
    let min_x = seg.p0.x.min(seg.p1.x).min(seg.p2.x) - margin;
    let max_x = seg.p0.x.max(seg.p1.x).max(seg.p2.x) + margin;
    let min_y = seg.p0.y.min(seg.p1.y).min(seg.p2.y) - margin;
    let max_y = seg.p0.y.max(seg.p1.y).max(seg.p2.y) + margin;
    pt.x >= min_x && pt.x <= max_x && pt.y >= min_y && pt.y <= max_y
}

/// 指定座標 `pt` がシェイプの輪郭（外周・穴）から距離 `radius` 以内にあるかを判定する
fn point_within_shape_contours(shape: &FillShape, pt: Point, radius: f64, inv_z: f64) -> bool {
    let r_sq = radius * radius;
    for contour in shape.contours() {
        for seg in contour.segments.iter() {
            if !quad_control_bbox_contains(seg, pt, radius) {
                continue;
            }
            if seg.nearest(pt, 0.05 * inv_z).distance_sq <= r_sq {
                return true;
            }
        }
    }
    false
}

/// 図形 `shape` が矩形 `rect` に少しでもかかるか。輪郭が矩形の中を通る（端が中にあるか、矩形の辺と交わる）か、
/// 輪郭が矩形の中を通らないなら、矩形がまるごと図形の中にあるとき
fn shape_overlaps_rect(shape: &FillShape, rect: KRect) -> bool {
    use kurbo::ParamCurveExtrema;
    // 2次曲線 `q` の、軸 `axis`（0 = x、1 = y）の値が `v` になる t（0〜1）
    fn roots(q: &QuadBez, axis: usize, v: f64) -> impl Iterator<Item = f64> {
        let c = |p: Point| if axis == 0 { p.x } else { p.y };
        let (a, b, c0) = (
            c(q.p0) - 2.0 * c(q.p1) + c(q.p2),
            2.0 * (c(q.p1) - c(q.p0)),
            c(q.p0) - v,
        );
        let mut ts = [f64::NAN; 2];
        if a.abs() > f64::EPSILON * (b.abs() + c0.abs()).max(1.0) {
            let disc = b * b - 4.0 * a * c0;
            if disc >= 0.0 {
                let r = disc.sqrt();
                ts = [(-b - r) / (2.0 * a), (-b + r) / (2.0 * a)];
            }
        } else if b != 0.0 {
            ts[0] = -c0 / b;
        }
        ts.into_iter().filter(|t| (0.0..=1.0).contains(t))
    }
    for c in shape.contours() {
        for q in c.segments.iter() {
            if !rects_overlap(q.bounding_box(), rect) {
                continue;
            }
            if rect.contains(q.p0) || rect.contains(q.p2) {
                return true;
            }
            let crosses_x =
                |x: f64| roots(q, 0, x).any(|t| (rect.y0..=rect.y1).contains(&q.eval(t).y));
            let crosses_y =
                |y: f64| roots(q, 1, y).any(|t| (rect.x0..=rect.x1).contains(&q.eval(t).x));
            if crosses_x(rect.x0) || crosses_x(rect.x1) || crosses_y(rect.y0) || crosses_y(rect.y1)
            {
                return true;
            }
        }
    }
    shape.contains_point(rect.center())
}

/// 矩形 `rect` と交差する現在の編集スコープ直下の子グループID一覧を収集する
pub(crate) fn collect_marquee_hit_groups(
    scene: Scene<'_>,
    editing_group_path: &[u64],
    rect: KRect,
) -> HashSet<u64> {
    let mut hit_group_ids: HashSet<u64> = HashSet::new();
    for shape in scene.iter() {
        let Some(gid) = shape.active_group_id_at_scope(editing_group_path) else {
            continue;
        };
        if hit_group_ids.contains(&gid) {
            continue;
        };
        let Some(bbox) = shape.geometry_bounding_box() else {
            continue;
        };
        if !rects_overlap(bbox, rect) {
            continue;
        };
        let in_rect =
            bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1;
        if in_rect || shape_overlaps_rect(shape, rect) {
            hit_group_ids.insert(gid);
        }
    }
    hit_group_ids
}

/// アルゴリズム 4: マウス位置に対する頂点・輪郭エッジ・塗りのヒットテスト（`Scene` を読み取る）。
/// 変形モードは選択中の塗り → 地図の頂点・辺 → 最前面の塗り、アンカー削除モードは図形の頂点、アンカー追加モードは図形の辺を探す
pub(crate) fn compute_hover_target(
    scene: Scene<'_>,
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
    sub_mode: SelectSubMode,
) -> HoverTarget {
    let inv_z = 1.0 / (zoom as f64);
    let anchor_edit_hit_radius = 12.0 * inv_z;
    let scene_len = scene.len();

    if sub_mode == SelectSubMode::Reshape {
        let selected_grab_radius = 6.0 * inv_z;
        let (is_sel, has_sel) = scene.selected_shape_mask(&session.selection);
        if has_sel {
            for s_idx in (0..scene_len).rev() {
                if !is_sel[s_idx] {
                    continue;
                }
                let Some(shape) = scene.get(s_idx) else {
                    continue;
                };
                if !shape.is_in_editing_scope(&session.editing_group_path)
                    || !shape
                        .bounding_box()
                        .inflate(selected_grab_radius, selected_grab_radius)
                        .contains(mouse_pt)
                {
                    continue;
                }
                if shape.contains_point(mouse_pt)
                    || point_within_shape_contours(shape, mouse_pt, selected_grab_radius, inv_z)
                {
                    return HoverTarget::Fill {
                        shape_id: shape.shape_id,
                    };
                }
            }
        }
        // 輪郭の変形は、境目の線で持つ地図で頂点・辺を探す（同じレイヤーの生の図形どうしに前後は無い）
        let target = crate::planar_map::reshape_hover(scene, session, mouse_pt, zoom);
        if !matches!(target, HoverTarget::None) {
            return target;
        }
        return scene
            .topmost_shape_at(mouse_pt, &session.editing_group_path)
            .and_then(|s_idx| scene.get(s_idx))
            .map_or(HoverTarget::None, |shape| HoverTarget::Fill {
                shape_id: shape.shape_id,
            });
    }

    // アンカーの追加・削除は地図で探す（前後は無い）。動かして浮いている片（ほかの図形と重なることがある）だけは、
    // 先に図形ごとに探す。残りと動かしていない片は、地図から切り出したもので境目を隣と共有するので地図で探す
    let Some(fl) = session.floating().filter(|fl| fl.modified) else {
        return crate::planar_map::anchor_hover(
            scene,
            session,
            mouse_pt,
            zoom,
            sub_mode == SelectSubMode::AddAnchor,
        );
    };
    let floating_ids: HashSet<u64> = fl.pieces.iter().map(|s| s.shape_id).collect();

    // 浮いている図形の頂点・辺の当たり判定の候補になる図形を前から順に返す
    // （最前面の塗りより後ろの図形と、外接矩形を半径だけ広げてもマウスが入らない図形は除く）
    let topmost_fill_idx = scene.topmost_shape_at(mouse_pt, &session.editing_group_path);
    let candidate_shapes = (0..scene_len).rev().filter_map(|s_idx| {
        if topmost_fill_idx.is_some_and(|top| s_idx < top) {
            return None;
        }
        let shape = scene.get(s_idx)?;
        let bbox = shape.geometry_bounding_box()?;
        if !floating_ids.contains(&shape.shape_id)
            || !shape.is_raw_at_scope(&session.editing_group_path)
            || !bbox
                .inflate(anchor_edit_hit_radius, anchor_edit_hit_radius)
                .contains(mouse_pt)
        {
            return None;
        }
        Some((s_idx, shape))
    });
    // `s_idx` の図形より前面にある編集スコープ内の図形が、点 `pt` を覆っているか
    let hidden_by_front = |s_idx: usize, pt: Point| {
        (s_idx + 1..scene_len).any(|front_idx| {
            scene.get(front_idx).is_some_and(|front| {
                front.is_in_editing_scope(&session.editing_group_path) && front.contains_point(pt)
            })
        })
    };

    let mut best: Option<(f64, HoverTarget)> = None;
    for (s_idx, shape) in candidate_shapes {
        for (c_idx, contour) in shape.contours().enumerate() {
            if sub_mode == SelectSubMode::DeleteAnchor && contour.segments.len() <= 2 {
                continue;
            }
            for (seg_idx, seg) in contour.segments.iter().enumerate() {
                let found = if sub_mode == SelectSubMode::DeleteAnchor {
                    let dist = (mouse_pt - seg.p0).hypot();
                    (dist <= anchor_edit_hit_radius).then_some((
                        dist,
                        seg.p0,
                        HoverTarget::Vertex {
                            shape_id: shape.shape_id,
                            contour_idx: c_idx,
                            seg_idx,
                            pos: seg.p0,
                            map_node: None,
                        },
                    ))
                } else {
                    if !quad_control_bbox_contains(seg, mouse_pt, anchor_edit_hit_radius) {
                        continue;
                    }
                    let nearest = seg.nearest(mouse_pt, 0.05 * inv_z);
                    let t = nearest.t.clamp(0.08, 0.92);
                    let dist = nearest.distance_sq.sqrt();
                    (dist <= anchor_edit_hit_radius).then(|| {
                        (
                            dist,
                            seg.eval(t),
                            HoverTarget::Edge {
                                shape_id: shape.shape_id,
                                contour_idx: c_idx,
                                seg_idx,
                                t,
                                closest_pos: seg.eval(t),
                                seg: *seg,
                                map_edge: None,
                            },
                        )
                    })
                };
                if let Some((dist, pos, target)) = found {
                    if best.as_ref().is_none_or(|(d, _)| dist < *d) && !hidden_by_front(s_idx, pos)
                    {
                        best = Some((dist, target));
                    }
                }
            }
        }
    }
    match best {
        Some((_, target)) => target,
        None => crate::planar_map::anchor_hover(
            scene,
            session,
            mouse_pt,
            zoom,
            sub_mode == SelectSubMode::AddAnchor,
        ),
    }
}

/// 選択中の図形がすべて同じレイヤーにあればその `LayerId` を返す（未選択・複数レイヤーにまたがるときは `None`）
pub(crate) fn selected_single_layer_id(tab: &Tab) -> Option<LayerId> {
    let scene = tab.scene();
    let mut lids = tab
        .session
        .selected_indices()
        .into_iter()
        .filter_map(|idx| scene.get(idx).map(|s| s.layer_id));
    let first = lids.next()?;
    lids.all(|lid| lid == first).then_some(first)
}

/// 選択中のオブジェクト（生シェイプ・既存グループ問わず）を新しい1つのグループにまとめる
pub(crate) fn prepare_group_selected_shapes(
    tab: &mut Tab,
    before_mutate: impl FnOnce(&mut Tab),
) -> bool {
    cancel_active_drag(tab);
    let Some(target_lid) = selected_single_layer_id(tab) else {
        return false;
    };
    if !tab
        .scene()
        .selected_has_any_in_scope(&tab.session.selection, &tab.session.editing_group_path)
    {
        return false;
    }

    before_mutate(tab);
    let new_gid = tab.doc.alloc_group_id();

    let fl_opt = tab.session.take_floating();
    let selection = tab.session.selection.clone();
    let depth = tab.session.editing_group_path.len();
    let layer_ids = editable_layer_ids(tab);
    edit_layers(tab, &layer_ids, |edit| {
        if let Some(fl) = fl_opt {
            edit.materialize_floating_without_merge(fl);
        }
        let insert_limit = edit.scope_insert_index();
        let mut unselected = Vec::with_capacity(edit.shapes.len());
        let mut grouped_shapes = Vec::new();
        let mut trailing_outside = Vec::new();
        for (idx, mut shape) in edit.shapes.drain(..).enumerate() {
            if selection.contains_shape(&shape)
                && shape.is_in_editing_scope(&edit.editing_group_path)
            {
                shape.group_path.insert(depth, new_gid);
                grouped_shapes.push(shape);
            } else if idx < insert_limit {
                unselected.push(shape);
            } else {
                trailing_outside.push(shape);
            }
        }
        unselected.extend(grouped_shapes);
        unselected.extend(trailing_outside);
        edit.shapes = unselected;
    });

    if let Some(l_idx) = tab.active_timeline().layer_index_by_id(target_lid) {
        tab.set_active_layer(l_idx);
    }
    tab.session.transform_pivot = None;
    true
}

/// 選択中のグループまたはシンボルインスタンスを1段階だけ解除（分離）する
pub(crate) fn prepare_ungroup_selected_shapes(
    tab: &mut Tab,
    before_mutate: impl FnOnce(&mut Tab),
) -> bool {
    cancel_active_drag(tab);
    if !tab
        .scene()
        .selection_has_group(&tab.session.selection, &tab.session.editing_group_path)
    {
        return false;
    }
    before_mutate(tab);

    let depth = tab.session.editing_group_path.len();
    let layer_ids = editable_layer_ids(tab);
    let sel_snapshot = tab.session.selection.clone();
    edit_layers(tab, &layer_ids, |edit| {
        for shape in &mut edit.shapes {
            if sel_snapshot.contains_shape(shape)
                && shape.is_in_editing_scope(&edit.editing_group_path)
                && shape.group_path.len() > depth
            {
                let gid = shape.group_path[depth];
                if let Some(b) = shape
                    .symbol_instance
                    .as_mut()
                    .filter(|b| b.instance_gid == gid)
                {
                    b.color_effect = ColorEffect::default();
                }
            }
        }
    });

    let mut child_map: Vec<((u64, u64), Vec<u64>, SymbolInstanceBinding)> = Vec::new();
    let tl = tab.active_timeline();
    let current_frame = tl.current_frame;
    for layer in &tl.layers {
        let Some(kf_idx) = layer.keyframe_index_at(current_frame) else {
            continue;
        };
        let kf = &layer.keyframes[kf_idx];
        for inst in kf.cel.instances.iter() {
            if inst.parent_scope == tab.session.editing_group_path
                && tab
                    .session
                    .selection
                    .instances
                    .contains(&inst.binding.instance_gid)
            {
                if let Some(sym_a) = tab
                    .doc
                    .library
                    .iter()
                    .find(|s| s.id == inst.binding.symbol_id)
                {
                    for (scope, cb) in sym_a.detached_child_bindings(
                        inst.binding,
                        kf.start_frame,
                        current_frame,
                        &tab.doc.library,
                    ) {
                        child_map.push(((inst.binding.instance_gid, cb.instance_gid), scope, cb));
                    }
                }
            }
        }
    }

    let mut any_became_raw = false;
    let mut selection = std::mem::take(&mut tab.session.selection);
    edit_layers(tab, &layer_ids, |edit| {
        let mut detached_inner_gid_map: Vec<((u64, u64), u64)> = Vec::new();
        let mut detached_instances: Vec<(u64, usize, u64, SymbolInstanceBinding)> = Vec::new();
        let editing_group_path = &edit.editing_group_path;
        let next_group_id = &mut edit.next_group_id;
        let map_gid =
            |map: &mut Vec<((u64, u64), u64)>, next_id: &mut u64, key: (u64, u64)| -> u64 {
                if let Some(&(_, m)) = map.iter().find(|&&(k, _)| k == key) {
                    m
                } else {
                    let id = (*next_id).max(1);
                    *next_id = if id < u64::MAX { id + 1 } else { 1 };
                    map.push((key, id));
                    id
                }
            };
        for (idx, shape) in edit.shapes.iter_mut().enumerate() {
            if !selection.contains_shape(shape) {
                continue;
            }
            if shape.is_in_editing_scope(editing_group_path) && shape.group_path.len() > depth {
                let removed_gid = shape.group_path.remove(depth);
                if let Some(a_binding) = shape
                    .symbol_instance
                    .filter(|b| b.instance_gid == removed_gid)
                {
                    shape.symbol_instance = None;
                    if let Some(entry) = detached_instances
                        .iter_mut()
                        .find(|(gid, ..)| *gid == removed_gid)
                    {
                        entry.1 = idx;
                    } else {
                        detached_instances.push((removed_gid, idx, shape.layer_id, a_binding));
                    }
                    for inner_gid in &mut shape.group_path[depth..] {
                        let key = (removed_gid, *inner_gid);
                        let mapped = map_gid(&mut detached_inner_gid_map, next_group_id, key);
                        *inner_gid = mapped;
                        if shape.symbol_instance.is_none() {
                            if let Some(&(_, _, mut cb)) =
                                child_map.iter().find(|&&(k, _, _)| k == key)
                            {
                                cb.instance_gid = mapped;
                                cb.xform = a_binding.xform * cb.xform;
                                shape.symbol_instance = Some(cb);
                                selection.instances.insert(mapped);
                            }
                        }
                    }
                    if shape.symbol_instance.is_none() && !shape.exterior.segments.is_empty() {
                        selection.shapes.insert(shape.shape_id);
                    }
                }
                if shape.group_path.len() == depth && !shape.exterior.segments.is_empty() {
                    any_became_raw = true;
                }
            }
        }
        let mut offset = 0usize;
        for &(a_gid, last_idx, layer_id, a_binding) in &detached_instances {
            for &(key @ (p_gid, _), ref child_scope, mut cb) in &child_map {
                if p_gid == a_gid && !detached_inner_gid_map.iter().any(|&(k, _)| k == key) {
                    let mut scope = editing_group_path.clone();
                    for &g in child_scope {
                        scope.push(map_gid(
                            &mut detached_inner_gid_map,
                            next_group_id,
                            (a_gid, g),
                        ));
                    }
                    let id = map_gid(&mut detached_inner_gid_map, next_group_id, key);
                    cb.instance_gid = id;
                    cb.xform = a_binding.xform * cb.xform;
                    let insert_at = (last_idx + 1 + offset).min(edit.shapes.len());
                    edit.shapes.insert(
                        insert_at,
                        FillShape::new_empty_symbol_marker(cb, layer_id, &scope),
                    );
                    offset += 1;
                    selection.instances.insert(id);
                }
            }
        }
        selection
            .instances
            .retain(|gid| !detached_instances.iter().any(|(d, ..)| d == gid));
        edit.shapes
            .retain(|s| !s.exterior.segments.is_empty() || s.symbol_instance.is_some());
    });
    tab.session.selection = selection;
    if any_became_raw {
        tab.session.lift_selected_raw_shapes_to_floating();
    }
    tab.session.retain_existing_selection();
    tab.session.transform_pivot = None;
    true
}

/// グループ・シンボルインスタンスの重ね順（前後関係）の変更モード
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ArrangeMode {
    ToFront,
    Forward,
    Backward,
    ToBack,
}

/// 選択中のグループ・シンボルインスタンスの重ね順を、各レイヤー・現在の編集スコープ内で変更する
pub(crate) fn prepare_arrange_selected(
    tab: &mut Tab,
    mode: ArrangeMode,
    before_mutate: impl FnOnce(&mut Tab),
) -> bool {
    commit_floating(tab);
    if tab.session.selection.is_empty() {
        return false;
    }
    let scope = &tab.session.editing_group_path;
    let selection = &tab.session.selection;
    let mut layer_orders: Vec<(u64, Vec<u64>)> = Vec::new();
    for lid in editable_layer_ids(tab) {
        let mut chunks: Vec<(u64, bool)> = Vec::new();
        for s in tab.scene().iter() {
            if s.layer_id == lid {
                if let Some(gid) = s.active_group_id_at_scope(scope) {
                    let sel = selection.contains_shape(s);
                    if let Some(c) = chunks.iter_mut().find(|(id, _)| *id == gid) {
                        c.1 |= sel;
                    } else {
                        chunks.push((gid, sel));
                    }
                }
            }
        }
        if chunks.len() < 2 || !chunks.iter().any(|(_, sel)| *sel) {
            continue;
        }
        let orig: Vec<u64> = chunks.iter().map(|(gid, _)| *gid).collect();
        match mode {
            ArrangeMode::ToFront => chunks.sort_by_key(|(_, sel)| *sel),
            ArrangeMode::ToBack => chunks.sort_by_key(|(_, sel)| !*sel),
            ArrangeMode::Forward => {
                for i in (0..chunks.len() - 1).rev() {
                    if chunks[i].1 && !chunks[i + 1].1 {
                        chunks.swap(i, i + 1);
                    }
                }
            }
            ArrangeMode::Backward => {
                for i in 1..chunks.len() {
                    if !chunks[i - 1].1 && chunks[i].1 {
                        chunks.swap(i - 1, i);
                    }
                }
            }
        }
        let next: Vec<u64> = chunks.into_iter().map(|(gid, _)| gid).collect();
        if next != orig {
            layer_orders.push((lid, next));
        }
    }
    if layer_orders.is_empty() {
        return false;
    }

    before_mutate(tab);
    let target_lids: Vec<u64> = layer_orders.iter().map(|(lid, _)| *lid).collect();
    edit_layers(tab, &target_lids, |edit| {
        for (lid, order) in &layer_orders {
            let slots: Vec<&mut FillShape> = edit
                .shapes
                .iter_mut()
                .filter(|s| {
                    s.layer_id == *lid
                        && s.active_group_id_at_scope(&edit.editing_group_path)
                            .is_some()
                })
                .collect();
            let mut sorted: Vec<FillShape> = slots.iter().map(|s| (*s).clone()).collect();
            sorted.sort_by_key(|s| {
                let gid = s.active_group_id_at_scope(&edit.editing_group_path);
                order.iter().position(|&id| Some(id) == gid)
            });
            for (slot, shape) in slots.into_iter().zip(sorted) {
                *slot = shape;
            }
        }
    });
    true
}

/// 選択中のオブジェクトをキャンバスから削除する
pub(crate) fn prepare_delete_selected_shapes(
    tab: &mut Tab,
    before_mutate: impl FnOnce(&mut Tab),
) -> bool {
    cancel_active_drag(tab);
    if tab.session.selection.is_empty() {
        return false;
    }
    let layer_ids = editable_layer_ids(tab);
    let any_valid = tab
        .scene()
        .iter()
        .any(|s| layer_ids.contains(&s.layer_id) && tab.session.selection.contains_shape(s));
    if !any_valid {
        tab.session.selection.clear();
        tab.session.set_floating(None);
        tab.session.transform_pivot = None;
        return false;
    }

    before_mutate(tab);

    let fl_opt = tab.session.take_floating();
    let selection = std::mem::take(&mut tab.session.selection);
    edit_layers(tab, &layer_ids, |edit| {
        if let Some(mut fl) = fl_opt {
            fl.pieces.clear();
            merge_floating_into_base_shapes(&mut edit.shapes, fl, &edit.layer_order_ids);
        }
        edit.shapes.retain(|s| !selection.contains_shape(s));
    });
    tab.session.transform_pivot = None;
    true
}

/// 現在の編集スコープの図形をすべて消去する
pub(crate) fn prepare_clear_active_frame_shapes(
    tab: &mut Tab,
    before_mutate: impl FnOnce(&mut Tab),
) -> bool {
    let layer_ids = editable_layer_ids(tab);
    let has_any_in_scope = tab.scene().iter().any(|s| {
        layer_ids.contains(&s.layer_id) && s.is_in_editing_scope(&tab.session.editing_group_path)
    });
    if !has_any_in_scope {
        return false;
    }
    clear_selection(tab);
    before_mutate(tab);
    edit_layers(tab, &layer_ids, |edit| {
        edit.shapes
            .retain(|s| !s.is_in_editing_scope(&edit.editing_group_path));
    });
    true
}
