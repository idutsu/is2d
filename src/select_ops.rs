use std::collections::HashSet;

use kurbo::{ParamCurve, ParamCurveNearest, Point, QuadBez, Rect as KRect};

use crate::canvas_tool::{
    cancel_active_drag, clear_selection, commit_floating, commit_floating_except, edit_layers,
    editable_layer_ids, merge_floating_into_base_shapes, sync_timeline_layer_from_canvas_selection,
    Floating, HoverTarget, LayerId, MarqueeDrag, Scene, SelectSubMode, Session, Tab,
};
use crate::doc_actions::{apply, Action, ActionHost};
use crate::symbol_ops::{enter_symbol_timeline, exit_symbol_timeline};
use crate::vector_boolean::{boolean_operation, slice_shape_with_marquee_rect, BoolOp};
use crate::vector_engine::{
    make_marquee_rect_shape, next_shape_id, normalize_vec, rects_overlap, ColorEffect, FillShape,
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

/// 矩形選択のドラッグ `m` を確定する（離した位置 `mouse_pt` が無ければ最後のドラッグ位置で確定する）
pub(crate) fn finish_marquee_drag(
    host: &mut impl ActionHost,
    m: MarqueeDrag,
    mouse_pt: Option<Point>,
) {
    apply(
        host,
        Action::SelectMarquee {
            p0: m.start_pos,
            p1: mouse_pt.unwrap_or(m.current_pos),
            additive: m.additive,
        },
    );
}

/// 矩形選択と図形の関係
enum MarqueeHit {
    /// 矩形に触れない
    Outside,
    /// 図形全体が矩形の中にある
    Inside,
    /// 矩形で切った中と外の片（どちらも元の図形のレイヤー・階層に置いてある）
    Split {
        inside: Vec<FillShape>,
        outside: Vec<FillShape>,
    },
}

/// 図形 `shape` と矩形 `rect`（その形が `clip_rect_shape`）の関係を調べ、かかっていれば切り分ける
fn marquee_hit(shape: &FillShape, rect: KRect, clip_rect_shape: &FillShape) -> MarqueeHit {
    let bbox = shape.bounding_box();
    if !rects_overlap(bbox, rect) {
        return MarqueeHit::Outside;
    }
    if bbox.x0 >= rect.x0 && bbox.x1 <= rect.x1 && bbox.y0 >= rect.y0 && bbox.y1 <= rect.y1 {
        return MarqueeHit::Inside;
    }
    let (inside, outside) = slice_shape_with_marquee_rect(shape, clip_rect_shape);
    if inside.is_empty() {
        return MarqueeHit::Outside;
    }
    if outside.is_empty() {
        return MarqueeHit::Inside;
    }
    let in_scope = |pieces: Vec<FillShape>| -> Vec<FillShape> {
        pieces
            .into_iter()
            .map(|s| s.with_scope(shape.layer_id, &shape.group_path))
            .collect()
    };
    MarqueeHit::Split {
        inside: in_scope(inside),
        outside: in_scope(outside),
    }
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
    let hit_group_ids = collect_marquee_hit_groups(
        tab.scene(),
        &tab.session.editing_group_path,
        rect,
        &clip_rect_shape,
    );

    let editable_lids = editable_layer_ids(tab);
    let base_shapes = tab.session.base_cache.shapes().to_vec();
    let editing_group_path = tab.session.editing_group_path.clone();
    let mut selection = std::mem::take(&mut tab.session.selection);
    tab.session.update_floating(|floating| {
        let mut lifted = Vec::new();
        if let Some(fl) = floating {
            let mut new_chunks = Vec::new();
            for (orig_id, chunk) in fl.take_remainder_chunks() {
                let mut chunk_rem: Vec<FillShape> = Vec::new();
                for mut rem in chunk {
                    if !rem.is_in_editing_scope(&editing_group_path)
                        || !rem.is_raw_at_scope(&editing_group_path)
                    {
                        chunk_rem.push(rem);
                        continue;
                    }
                    match marquee_hit(&rem, rect, &clip_rect_shape) {
                        MarqueeHit::Outside => chunk_rem.push(rem),
                        MarqueeHit::Inside => {
                            if fl.hidden.contains(&rem.shape_id) {
                                rem.shape_id = next_shape_id();
                            }
                            selection.insert_shape(&rem);
                            lifted.push(rem);
                        }
                        MarqueeHit::Split { inside, outside } => {
                            for mut out_s in outside {
                                if fl.hidden.contains(&out_s.shape_id) {
                                    out_s.shape_id = next_shape_id();
                                }
                                chunk_rem.push(out_s);
                            }
                            for mut piece in inside {
                                if fl.hidden.contains(&piece.shape_id) {
                                    piece.shape_id = next_shape_id();
                                }
                                selection.insert_shape(&piece);
                                lifted.push(piece);
                            }
                        }
                    }
                }
                if !chunk_rem.is_empty() {
                    new_chunks.push((orig_id, chunk_rem));
                }
            }
            fl.set_remainder_chunks(new_chunks);
        }

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
            match marquee_hit(shape, rect, &clip_rect_shape) {
                MarqueeHit::Outside => {}
                MarqueeHit::Inside => selection.insert_shape(shape),
                MarqueeHit::Split {
                    inside,
                    mut outside,
                } => {
                    let orig_id = shape.shape_id;
                    let fl = floating.get_or_insert_with(|| Floating {
                        pieces: Vec::new(),
                        remainder: Vec::new(),
                        hidden: HashSet::new(),
                        modified: false,
                    });
                    fl.hidden.insert(orig_id);
                    for s in &mut outside {
                        if s.shape_id == orig_id {
                            s.shape_id = next_shape_id();
                        }
                    }
                    if let Some(first_out) = outside.first_mut() {
                        first_out.shape_id = orig_id;
                    }
                    fl.remainder.extend(outside);
                    for mut piece in inside {
                        if piece.shape_id == orig_id {
                            piece.shape_id = next_shape_id();
                        }
                        selection.insert_shape(&piece);
                        lifted.push(piece);
                    }
                }
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

/// 矩形 `rect` と交差する現在の編集スコープ直下の子グループID一覧を収集する
pub(crate) fn collect_marquee_hit_groups(
    scene: Scene<'_>,
    editing_group_path: &[u64],
    rect: KRect,
    clip_rect_shape: &FillShape,
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
        if in_rect
            || !boolean_operation(
                std::slice::from_ref(shape),
                std::slice::from_ref(clip_rect_shape),
                BoolOp::Intersection,
                shape.color,
            )
            .is_empty()
        {
            hit_group_ids.insert(gid);
        }
    }
    hit_group_ids
}

/// アルゴリズム 4: マウス位置に対する頂点・輪郭エッジ・塗りのヒットテスト（`Scene` を読み取る）
pub(crate) fn compute_hover_target(
    scene: Scene<'_>,
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
    sub_mode: SelectSubMode,
) -> HoverTarget {
    let inv_z = 1.0 / (zoom as f64);
    let vertex_hit_radius = 9.0 * inv_z;
    let edge_hit_outer_radius = 9.5 * inv_z;
    let edge_hit_inner_radius = 5.5 * inv_z;
    let anchor_edit_hit_radius = 12.0 * inv_z;
    let selected_grab_radius = 6.0 * inv_z;

    let scene_len = scene.len();
    let (is_sel, has_sel) = scene.selected_shape_mask(&session.selection);

    if sub_mode == SelectSubMode::Reshape && has_sel {
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

    let topmost_fill_idx = scene.topmost_shape_at(mouse_pt, &session.editing_group_path);
    let is_sel = is_sel.as_slice();
    // 頂点・辺の当たり判定の候補になる図形を前から順に返す
    // （最前面の塗りより後ろの図形と、外接矩形を `radius` だけ広げてもマウスが入らない図形は除く）
    let candidate_shapes = |radius: f64| {
        (0..scene_len).rev().filter_map(move |s_idx| {
            if topmost_fill_idx.is_some_and(|top| s_idx < top) {
                return None;
            }
            let shape = scene.get(s_idx)?;
            let bbox = shape.geometry_bounding_box()?;
            if (sub_mode == SelectSubMode::Reshape && is_sel[s_idx])
                || !shape.is_raw_at_scope(&session.editing_group_path)
                || !bbox.inflate(radius, radius).contains(mouse_pt)
            {
                return None;
            }
            Some((s_idx, shape))
        })
    };
    // `s_idx` の図形より前面にある編集スコープ内の図形が、点 `pt` を覆っているか
    let hidden_by_front = |s_idx: usize, pt: Point| {
        (s_idx + 1..scene_len).any(|front_idx| {
            scene.get(front_idx).is_some_and(|front| {
                front.is_in_editing_scope(&session.editing_group_path) && front.contains_point(pt)
            })
        })
    };

    if sub_mode != SelectSubMode::AddAnchor {
        let v_radius = if sub_mode == SelectSubMode::DeleteAnchor {
            anchor_edit_hit_radius
        } else {
            vertex_hit_radius
        };
        let mut best_vertex: Option<(f64, HoverTarget)> = None;
        for (s_idx, shape) in candidate_shapes(v_radius) {
            for (c_idx, contour) in shape.contours().enumerate() {
                if sub_mode == SelectSubMode::DeleteAnchor && contour.segments.len() <= 2 {
                    continue;
                }
                for (seg_idx, seg) in contour.segments.iter().enumerate() {
                    let dist = (mouse_pt - seg.p0).hypot();
                    if dist <= v_radius && best_vertex.as_ref().is_none_or(|(d, _)| dist < *d) {
                        if hidden_by_front(s_idx, seg.p0) {
                            continue;
                        }
                        best_vertex = Some((
                            dist,
                            HoverTarget::Vertex {
                                shape_id: shape.shape_id,
                                contour_idx: c_idx,
                                seg_idx,
                                pos: seg.p0,
                            },
                        ));
                    }
                }
            }
        }
        if let Some((_, target)) = best_vertex {
            return target;
        }
        if sub_mode == SelectSubMode::DeleteAnchor {
            return HoverTarget::None;
        }
    }

    let mut best_edge: Option<(f64, HoverTarget)> = None;
    for (s_idx, shape) in candidate_shapes(anchor_edit_hit_radius) {
        let max_dist = if sub_mode == SelectSubMode::AddAnchor {
            anchor_edit_hit_radius
        } else if shape.contains_point(mouse_pt) {
            edge_hit_inner_radius
        } else {
            edge_hit_outer_radius
        };

        for (c_idx, contour) in shape.contours().enumerate() {
            for (seg_idx, seg) in contour.segments.iter().enumerate() {
                if !quad_control_bbox_contains(seg, mouse_pt, max_dist) {
                    continue;
                }
                let nearest = seg.nearest(mouse_pt, 0.05 * inv_z);
                let t_clamped = nearest.t.clamp(0.08, 0.92);
                let eval_pt = if sub_mode == SelectSubMode::AddAnchor {
                    seg.eval(t_clamped)
                } else {
                    seg.eval(nearest.t)
                };
                let dist = nearest.distance_sq.sqrt();
                if dist <= max_dist && best_edge.as_ref().is_none_or(|(d, _)| dist < *d) {
                    if hidden_by_front(s_idx, eval_pt) {
                        continue;
                    }
                    best_edge = Some((
                        dist,
                        HoverTarget::Edge {
                            shape_id: shape.shape_id,
                            contour_idx: c_idx,
                            seg_idx,
                            t: if sub_mode == SelectSubMode::AddAnchor {
                                t_clamped
                            } else {
                                nearest.t
                            },
                            closest_pos: eval_pt,
                        },
                    ));
                }
            }
        }
    }
    if let Some((_, target)) = best_edge {
        return target;
    }
    if sub_mode == SelectSubMode::AddAnchor {
        return HoverTarget::None;
    }

    if let Some(s_idx) = topmost_fill_idx {
        if let Some(shape) = scene.get(s_idx) {
            return HoverTarget::Fill {
                shape_id: shape.shape_id,
            };
        }
    }

    HoverTarget::None
}

/// 向こう側の輪郭エッジ付近でドロップされた際に確実に貫通して分割されるよう僅かに押し出す。
pub(crate) fn punch_through_opposite_edge(
    shape: &FillShape,
    contour_idx: usize,
    ignore_segs: &[usize],
    (anchor_a, anchor_b): (Point, Point),
    (start_pt, drag_pt): (Point, Point),
) -> Point {
    let bbox = shape.bounding_box();
    let min_dim = bbox.width().min(bbox.height());
    let chord_len = (anchor_b - anchor_a).hypot();
    let scale = (min_dim / 16.0).min(chord_len / 24.0).clamp(0.04, 1.0);
    let min_drag = 6.0 * scale;
    let min_anchor_dist = 10.0 * scale;
    let near_thresh = 6.0 * scale;
    let min_near_anchor = 8.0 * scale;
    let punch_push = 4.5 * scale;

    let drag_vec = drag_pt - start_pt;
    if drag_vec.hypot() <= min_drag
        || (drag_pt - anchor_a).hypot() <= min_anchor_dist
        || (drag_pt - anchor_b).hypot() <= min_anchor_dist
    {
        return drag_pt;
    }

    let drag_dir = normalize_vec(drag_vec);
    let mut best_near: Option<(f64, Point)> = None;
    for (c_idx, c) in shape.contours().enumerate() {
        for (s_idx, seg) in c.segments.iter().enumerate() {
            if (c_idx == contour_idx && ignore_segs.contains(&s_idx))
                || !quad_control_bbox_contains(seg, drag_pt, near_thresh)
            {
                continue;
            }
            let near = seg.nearest(drag_pt, 0.05 * scale);
            let d = near.distance_sq.sqrt();
            if d <= near_thresh {
                let near_pt = seg.eval(near.t);
                if (near_pt - anchor_a).hypot() > min_near_anchor
                    && (near_pt - anchor_b).hypot() > min_near_anchor
                    && best_near.is_none_or(|(bd, _)| d < bd)
                {
                    best_near = Some((d, near_pt));
                }
            }
        }
    }
    if let Some((_, near_pt)) = best_near {
        let punched = near_pt + drag_dir * punch_push;
        if (punched - start_pt).dot(drag_dir) > (drag_pt - start_pt).dot(drag_dir) {
            return punched;
        }
    }
    drag_pt
}

/// エッジつまみ変形（PullEdge）の制御点を計算して反映する（リアルタイムプレビュー用）。
pub(crate) fn set_pulled_edge(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    grab_t: f64,
    orig_seg: QuadBez,
    target_pt: Point,
) {
    let grab_pos = orig_seg.eval(grab_t);
    let delta = target_pt - grab_pos;
    let denom = 2.0 * grab_t * (1.0 - grab_t);
    let new_p1 = orig_seg.p1 + delta / denom;
    shape.edit_contour(contour_idx, |contour| {
        if seg_idx < contour.segments.len() {
            contour.segments_mut()[seg_idx].p1 = new_p1;
            true
        } else {
            false
        }
    });
}

/// エッジつまみ変形（PullEdge）を対向エッジ貫通補正込みで確定適用する。
pub(crate) fn apply_pull_edge_to_shape(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    grab_t: f64,
    orig_seg: QuadBez,
    drag_pt: Point,
) {
    let grab_pos = orig_seg.eval(grab_t);
    let effective_pt = punch_through_opposite_edge(
        shape,
        contour_idx,
        &[seg_idx],
        (orig_seg.p0, orig_seg.p2),
        (grab_pos, drag_pt),
    );
    set_pulled_edge(
        shape,
        (contour_idx, seg_idx),
        grab_t,
        orig_seg,
        effective_pt,
    );
}

/// 頂点ドラッグ変形（MoveVertex）の頂点と前後制御点を計算して反映する（リアルタイムプレビュー用）。
pub(crate) fn set_moved_vertex(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    orig_vertex: Point,
    orig_ctrls: (Point, Point),
    target_pt: Point,
) {
    let delta = target_pt - orig_vertex;
    shape.edit_contour(contour_idx, |contour| {
        let Some(prev_idx) = contour.prev_seg_idx(seg_idx) else {
            return false;
        };
        let segs = contour.segments_mut();
        segs[prev_idx].p2 = target_pt;
        segs[seg_idx].p0 = target_pt;
        segs[prev_idx].p1 = orig_ctrls.0 + delta * 0.5;
        segs[seg_idx].p1 = orig_ctrls.1 + delta * 0.5;
        true
    });
}

/// 頂点ドラッグ変形（MoveVertex）を対向エッジ貫通補正込みで確定適用する。
pub(crate) fn apply_move_vertex_to_shape(
    shape: &mut FillShape,
    (contour_idx, seg_idx): (usize, usize),
    orig_vertex: Point,
    orig_ctrls: (Point, Point),
    drag_pt: Point,
) {
    let Some((prev_idx, prev_p0, curr_p2)) = shape.get_contour(contour_idx).and_then(|c| {
        let prev_idx = c.prev_seg_idx(seg_idx)?;
        Some((prev_idx, c.segments[prev_idx].p0, c.segments[seg_idx].p2))
    }) else {
        return;
    };

    let effective_pt = punch_through_opposite_edge(
        shape,
        contour_idx,
        &[prev_idx, seg_idx],
        (prev_p0, curr_p2),
        (orig_vertex, drag_pt),
    );
    set_moved_vertex(
        shape,
        (contour_idx, seg_idx),
        orig_vertex,
        orig_ctrls,
        effective_pt,
    );
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
