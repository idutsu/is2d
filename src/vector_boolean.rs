use eframe::egui::Color32;
use i_overlay::{
    core::{fill_rule::FillRule as OverlayFillRule, overlay_rule::OverlayRule, solver::Solver},
    float::{filter::ContourFilter, overlay::FloatOverlay},
};
use kurbo::{
    ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, Point, QuadBez,
    Rect as KRect,
};

use crate::{
    vector_engine::{
        dedup_closed_ring, fill_cumulative_arc_lengths, normalize_vec, rects_overlap,
        ring_area_and_perimeter_f64, signed_area_f64, union_bounding_boxes, Contour, FillShape,
    },
    vector_stroke::{
        estimate_brush_radius_from_ring, fit_quad_control_point_with_arc_lengths, rdp_recursive,
        smooth_ring_to_quad_contour,
    },
};

// ============================================================================
// アルゴリズム 3: 平面ブーリアン演算（同色 Union / 異色・消しゴム Difference）
// & De Casteljau 2次ベジェ曲線ハイブリッド再構築エンジン
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BoolOp {
    Union,
    Difference,
    Intersection,
}

/// `i_overlay` に入出力する1図形分の閉リング配列（先頭が外周、2番目以降が穴）
type OverlayShape = Vec<Vec<[f64; 2]>>;

/// 新しいブラシストロークまたは変形確定シェイプをキャンバスへマージする（Flashのマージ描画モードの核心挙動）。
/// 同一の `group_path` を持つ生シェイプ同士のみが結合（`Union`）・切り抜き（`Difference`）を行い、
/// 異なるグループに属するシェイプは保護される。
pub(crate) fn merge_shapes_into_canvas(canvas: &mut Vec<FillShape>, incoming: Vec<FillShape>) {
    merge_shapes_into_canvas_inner(canvas, incoming, true);
}

/// 他の異色シェイプと重ならない塗りシェイプ（バケツ塗り・リカラー）をキャンバスへマージし、
/// 同じ `group_path` の隣接する同色シェイプがあればシームレスに結合（Union）する。
pub(crate) fn merge_disjoint_fill_into_canvas(canvas: &mut Vec<FillShape>, fill_shape: FillShape) {
    merge_shapes_into_canvas_inner(canvas, vec![fill_shape], false);
}

fn merge_shapes_into_canvas_inner(
    canvas: &mut Vec<FillShape>,
    incoming: Vec<FillShape>,
    subtract_diff_color: bool,
) {
    let mut iter = incoming.into_iter().peekable();
    while let Some(first) = iter.next() {
        let lid = first.layer_id;
        let col = first.color;
        let gp = first.group_path.clone();
        let mut batch = vec![first];
        while iter
            .peek()
            .is_some_and(|next| next.layer_id == lid && next.color == col && next.group_path == gp)
        {
            if let Some(next) = iter.next() {
                batch.push(next);
            }
        }
        merge_homogeneous_batch_into_canvas(canvas, batch, subtract_diff_color);
    }
}

fn merge_homogeneous_batch_into_canvas(
    canvas: &mut Vec<FillShape>,
    incoming: Vec<FillShape>,
    subtract_diff_color: bool,
) {
    if incoming.is_empty() {
        return;
    }
    let target_color = incoming[0].color;
    let target_layer_id = incoming[0].layer_id;
    let target_group_path = incoming[0].group_path.clone();
    let test_bbox = union_bounding_boxes(&incoming).inflate(1.5, 1.5);

    let insert_limit = if target_group_path.is_empty() {
        canvas
            .iter()
            .rposition(|s| s.layer_id == target_layer_id)
            .map(|pos| pos + 1)
            .unwrap_or(canvas.len())
    } else {
        canvas
            .iter()
            .rposition(|s| {
                s.layer_id == target_layer_id && s.is_in_editing_scope(&target_group_path)
            })
            .or_else(|| canvas.iter().rposition(|s| s.layer_id == target_layer_id))
            .map(|pos| pos + 1)
            .unwrap_or(canvas.len())
    };
    let trailing_outside = canvas.split_off(insert_limit);

    let mut updated_canvas =
        Vec::with_capacity(canvas.len() + incoming.len() + trailing_outside.len());
    let mut overlapping_same_color = Vec::new();
    let mut prepared_clip: Option<PreparedClip> = None;

    for existing in canvas.drain(..) {
        if existing.layer_id != target_layer_id || existing.group_path != target_group_path {
            updated_canvas.push(existing);
            continue;
        }
        let intersects_bbox = rects_overlap(existing.bounding_box(), test_bbox);
        if existing.color == target_color {
            if intersects_bbox {
                overlapping_same_color.push(existing);
            } else {
                updated_canvas.push(existing);
            }
        } else if subtract_diff_color && intersects_bbox {
            let clip = prepared_clip.get_or_insert_with(|| PreparedClip::new(&incoming));
            updated_canvas.extend(
                clip.subtract_from(&existing)
                    .into_iter()
                    .map(|s| s.with_scope(target_layer_id, &target_group_path)),
            );
        } else {
            updated_canvas.push(existing);
        }
    }

    let incoming_self_overlaps = incoming.len() > 1
        && incoming.iter().enumerate().any(|(i, a)| {
            let a_box = a.bounding_box().inflate(1.5, 1.5);
            incoming[i + 1..]
                .iter()
                .any(|b| rects_overlap(a_box, b.bounding_box()))
        });

    if overlapping_same_color.is_empty() && !incoming_self_overlaps {
        updated_canvas.extend(incoming);
    } else if overlapping_same_color.is_empty() {
        updated_canvas.extend(
            boolean_operation(&incoming[..1], &incoming[1..], BoolOp::Union, target_color)
                .into_iter()
                .map(|s| s.with_scope(target_layer_id, &target_group_path)),
        );
    } else {
        updated_canvas.extend(
            boolean_operation(
                &overlapping_same_color,
                &incoming,
                BoolOp::Union,
                target_color,
            )
            .into_iter()
            .map(|s| s.with_scope(target_layer_id, &target_group_path)),
        );
    }

    updated_canvas.extend(trailing_outside);
    *canvas = updated_canvas;
}

/// 消しゴムの輪郭で、同じ編集スコープ（`group_path`）直下の生シェイプに対してのみ Boolean Difference を適用する
pub(crate) fn erase_from_canvas(canvas: &mut Vec<FillShape>, eraser_shapes: &[FillShape]) {
    if eraser_shapes.is_empty() {
        return;
    }
    let target_group_path = &eraser_shapes[0].group_path;
    let test_bbox = union_bounding_boxes(eraser_shapes).inflate(1.5, 1.5);
    let mut prepared_clip: Option<PreparedClip> = None;

    let mut updated_canvas = Vec::with_capacity(canvas.len());
    for existing in canvas.drain(..) {
        if existing.group_path == *target_group_path
            && rects_overlap(existing.bounding_box(), test_bbox)
        {
            let layer_id = existing.layer_id;
            let clip = prepared_clip.get_or_insert_with(|| PreparedClip::new(eraser_shapes));
            updated_canvas.extend(
                clip.subtract_from(&existing)
                    .into_iter()
                    .map(|s| s.with_scope(layer_id, target_group_path)),
            );
        } else {
            updated_canvas.push(existing);
        }
    }
    *canvas = updated_canvas;
}

/// 複数の対象シェイプに対して繰り返し Difference（減算）を行う際に、
/// クリップ側シェイプ群のフラットニング結果と `QuadRecord` を1回だけ事前構築して使い回すキャッシュ構造体
struct PreparedClip {
    clip_shapes: Vec<OverlayShape>,
    clip_quads: Vec<QuadRecord>,
    clip_min_dim: f64,
}

impl PreparedClip {
    fn new(clip: &[FillShape]) -> Self {
        let mut clip_quads = Vec::new();
        let clip_shapes = clip
            .iter()
            .map(|s| flatten_shape_for_overlay(s, &mut clip_quads))
            .filter(|r| !r.is_empty())
            .collect();
        let clip_min_dim = clip
            .iter()
            .map(|s| {
                let b = s.bounding_box();
                b.width().min(b.height())
            })
            .filter(|&d| d > 1e-6)
            .fold(f64::INFINITY, f64::min);
        Self {
            clip_shapes,
            clip_quads,
            clip_min_dim,
        }
    }

    fn subtract_from(&self, subj: &FillShape) -> Vec<FillShape> {
        let mut quad_table = Vec::with_capacity(16 + self.clip_quads.len());
        let subj_poly = flatten_shape_for_overlay(subj, &mut quad_table);
        if subj_poly.is_empty() {
            return Vec::new();
        }
        quad_table.extend_from_slice(&self.clip_quads);
        let subj_b = subj.bounding_box();
        let subj_dim = subj_b.width().min(subj_b.height());
        let min_dim = if subj_dim > 1e-6 {
            self.clip_min_dim.min(subj_dim)
        } else {
            self.clip_min_dim
        };
        let min_area = if min_dim.is_finite() && min_dim < 2.0 {
            0.04_f64.min((min_dim * min_dim * 0.01).max(1e-5))
        } else {
            0.04
        };
        let overlay_result = run_float_overlay(
            std::slice::from_ref(&subj_poly),
            &self.clip_shapes,
            OverlayRule::Difference,
            OverlayFillRule::NonZero,
            min_area,
        );
        reconstruct_shapes_from_overlay(&overlay_result, &quad_table, subj.color, false)
    }
}

fn adaptive_overlay_min_area(shapes_a: &[FillShape], shapes_b: &[FillShape]) -> f64 {
    let min_dim = shapes_a
        .iter()
        .chain(shapes_b.iter())
        .map(|s| {
            let b = s.bounding_box();
            b.width().min(b.height())
        })
        .filter(|&d| d > 1e-6)
        .fold(f64::INFINITY, f64::min);
    if min_dim.is_finite() && min_dim < 2.0 {
        0.04_f64.min((min_dim * min_dim * 0.01).max(1e-5))
    } else {
        0.04
    }
}

/// `i_overlay` クレート（`FloatOverlay`）を用いたハイブリッド2次ベジェブーリアン演算＆再フィッティング
pub(crate) fn boolean_operation(
    subj: &[FillShape],
    clip: &[FillShape],
    op: BoolOp,
    out_color: Color32,
) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let subj_shapes: Vec<OverlayShape> = subj
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();
    let clip_shapes: Vec<OverlayShape> = clip
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();

    if (subj_shapes.is_empty() && op == BoolOp::Difference)
        || (op == BoolOp::Intersection && (subj_shapes.is_empty() || clip_shapes.is_empty()))
        || (subj_shapes.is_empty() && clip_shapes.is_empty())
    {
        return Vec::new();
    }

    let overlay_rule = match op {
        BoolOp::Union => OverlayRule::Union,
        BoolOp::Difference => OverlayRule::Difference,
        BoolOp::Intersection => OverlayRule::Intersect,
    };

    let min_area = adaptive_overlay_min_area(subj, clip);
    let overlay_result = run_float_overlay(
        &subj_shapes,
        &clip_shapes,
        overlay_rule,
        OverlayFillRule::NonZero,
        min_area,
    );
    reconstruct_shapes_from_overlay(&overlay_result, &quad_table, out_color, op == BoolOp::Union)
}

/// マーキー矩形 `clip_rect_shape` で単一シェイプ `shape` をスライスし、
/// 1回のフラット化＆`quad_table` 構築で `(矩形内 inside, 矩形外 outside)` を同時に返す
pub(crate) fn slice_shape_with_marquee_rect(
    shape: &FillShape,
    clip_rect_shape: &FillShape,
) -> (Vec<FillShape>, Vec<FillShape>) {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let subj = flatten_shape_for_overlay(shape, &mut quad_table);
    let clip = flatten_shape_for_overlay(clip_rect_shape, &mut quad_table);
    if subj.is_empty() || clip.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let min_area = adaptive_overlay_min_area(
        std::slice::from_ref(shape),
        std::slice::from_ref(clip_rect_shape),
    );
    let subj_slice = std::slice::from_ref(&subj);
    let clip_slice = std::slice::from_ref(&clip);
    let inside_rings = run_float_overlay(
        subj_slice,
        clip_slice,
        OverlayRule::Intersect,
        OverlayFillRule::NonZero,
        min_area,
    );
    let inside = reconstruct_shapes_from_overlay(&inside_rings, &quad_table, shape.color, false);
    if inside.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let outside_rings = run_float_overlay(
        subj_slice,
        clip_slice,
        OverlayRule::Difference,
        OverlayFillRule::NonZero,
        min_area,
    );
    let outside = reconstruct_shapes_from_overlay(&outside_rings, &quad_table, shape.color, false);
    (inside, outside)
}

fn reconstruct_shapes_from_overlay(
    overlay_result: &[OverlayShape],
    quad_table: &[QuadRecord],
    color: Color32,
    filter_sliver_holes: bool,
) -> Vec<FillShape> {
    let grid = QuadSpatialGrid::new(quad_table);
    overlay_result
        .iter()
        .filter_map(|poly_shape| {
            reconstruct_fill_shape_from_rings(
                poly_shape,
                quad_table,
                &grid,
                color,
                filter_sliver_holes,
            )
        })
        .collect()
}

fn run_float_overlay(
    subj: &[OverlayShape],
    clip: &[OverlayShape],
    overlay_rule: OverlayRule,
    fill_rule: OverlayFillRule,
    min_area: f64,
) -> Vec<OverlayShape> {
    let filter = ContourFilter {
        min_area,
        simplify: false,
    };
    FloatOverlay::with_subj_and_clip(subj, clip).overlay_with_filter_and_solver(
        overlay_rule,
        fill_rule,
        filter,
        Solver::AUTO,
    )
}

fn reconstruct_fill_shape_from_rings(
    poly_shape: &[Vec<[f64; 2]>],
    quad_table: &[QuadRecord],
    grid: &QuadSpatialGrid,
    color: Color32,
    filter_sliver_holes: bool,
) -> Option<FillShape> {
    if poly_shape.is_empty() {
        return None;
    }
    let exterior = reconstruct_or_refit_quad_contour(&poly_shape[0], quad_table, grid, true)?;
    let (_, ext_perim) = ring_area_and_perimeter_f64(&poly_shape[0]);
    let hole_scale = if ext_perim < 16.0 {
        (ext_perim / 16.0).clamp(0.05, 1.0)
    } else {
        1.0
    };
    let min_hole_area = 1.5 * hole_scale * hole_scale;
    let min_hole_thickness = 0.15 * hole_scale;
    let holes = poly_shape[1..]
        .iter()
        .filter(|hole_ring| {
            if !filter_sliver_holes {
                return true;
            }
            let (a, p) = ring_area_and_perimeter_f64(hole_ring);
            a >= min_hole_area && (a / p.max(1e-5)) >= min_hole_thickness
        })
        .filter_map(|hole_ring| {
            reconstruct_or_refit_quad_contour(hole_ring, quad_table, grid, false)
        })
        .collect();
    Some(FillShape::new(color, exterior, holes))
}

/// 指定座標 `pt` を囲み得る連結AABBクラスタを計算し、無関係な遠方のシェイプを事前にAABBカリングする
fn cull_enclosed_region_candidates(
    canvas: &[FillShape],
    pt: Point,
) -> Option<(KRect, Vec<FillShape>)> {
    let mut clusters: Vec<KRect> = canvas
        .iter()
        .filter(|s| !s.exterior.segments.is_empty())
        .map(|s| s.bounding_box().inflate(1.0, 1.0))
        .collect();
    if clusters.is_empty() {
        return None;
    }

    let mut merged = true;
    while merged {
        merged = false;
        let mut i = 0;
        while i < clusters.len() {
            let mut j = i + 1;
            while j < clusters.len() {
                if rects_overlap(clusters[i], clusters[j]) {
                    let b = clusters.swap_remove(j);
                    clusters[i] = clusters[i].union(b);
                    merged = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }

    let mut active_bounds: Option<KRect> = None;
    for c in clusters {
        if c.contains(pt) {
            active_bounds = Some(match active_bounds {
                Some(acc) => acc.union(c),
                None => c,
            });
        }
    }
    let bounds = active_bounds?;
    let candidates: Vec<FillShape> = canvas
        .iter()
        .filter(|s| !s.exterior.segments.is_empty() && rects_overlap(s.bounding_box(), bounds))
        .cloned()
        .collect();
    if candidates.is_empty() {
        None
    } else {
        Some((bounds, candidates))
    }
}

/// キャンバス上のシェイプ群によって完全に囲まれた空白領域（単一シェイプの穴、または複数シェイプで囲まれた閉領域）のうち、
/// 指定座標 `pt` を含むものを検出し、周囲の2次ベジェ曲線をそのまま引き継いだ `FillShape` を生成する。
pub(crate) fn find_enclosed_region_at(
    canvas: &[FillShape],
    pt: Point,
    fill_color: Color32,
) -> Option<FillShape> {
    let (active_bbox, candidates) = cull_enclosed_region_candidates(canvas, pt)?;

    let bounds = active_bbox.inflate(32.0, 32.0);
    let mut bounds_ring = vec![
        [bounds.x0, bounds.y0],
        [bounds.x1, bounds.y0],
        [bounds.x1, bounds.y1],
        [bounds.x0, bounds.y1],
    ];
    if signed_area_f64(&bounds_ring) > 0.0 {
        bounds_ring.reverse();
    }

    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let clip_shapes: Vec<OverlayShape> = candidates
        .iter()
        .map(|s| flatten_shape_for_overlay(s, &mut quad_table))
        .filter(|r| !r.is_empty())
        .collect();
    if clip_shapes.is_empty() {
        return None;
    }

    let min_area = adaptive_overlay_min_area(&candidates, &[]);
    let overlay_result = run_float_overlay(
        &[vec![bounds_ring]],
        &clip_shapes,
        OverlayRule::Difference,
        OverlayFillRule::NonZero,
        min_area,
    );

    for poly_shape in overlay_result {
        if poly_shape.is_empty() {
            continue;
        }
        let ext_ring = &poly_shape[0];
        let touches_bounds = ext_ring.iter().any(|&[x, y]| {
            (x - bounds.x0).abs() < 4.0
                || (x - bounds.x1).abs() < 4.0
                || (y - bounds.y0).abs() < 4.0
                || (y - bounds.y1).abs() < 4.0
        });
        if touches_bounds {
            continue;
        }

        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for &[x, y] in ext_ring {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        if pt.x < min_x - 1.0 || pt.x > max_x + 1.0 || pt.y < min_y - 1.0 || pt.y > max_y + 1.0 {
            continue;
        }
        if !poly_shape_contains_point(&poly_shape, pt) {
            continue;
        }

        let grid = QuadSpatialGrid::new(&quad_table);
        let Some(candidate) =
            reconstruct_fill_shape_from_rings(&poly_shape, &quad_table, &grid, fill_color, false)
        else {
            continue;
        };
        if candidate.contains_point(pt) {
            return Some(candidate);
        }
    }

    None
}

/// ポリゴンリング群（外周＋穴）が点 `pt` を内包するかを偶奇レイキャスト法で高速判定する（ベジェ再構築・メッシュ化前のフィルタ用）
fn poly_shape_contains_point(poly_shape: &[Vec<[f64; 2]>], pt: Point) -> bool {
    let mut inside = false;
    for ring in poly_shape {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let [xi, yi] = ring[i];
            let [xj, yj] = ring[j];
            if ((yi > pt.y) != (yj > pt.y))
                && (pt.x < (xj - xi) * (pt.y - yi) / (yj - yi) + xi)
            {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

#[derive(Clone, Copy, Debug)]
struct QuadRecord {
    quad: QuadBez,
    bbox: KRect,
}

/// `quad_table` 内の2次ベジェ曲線セグメント群を 2D 均一グリッドに空間インデックス化し、
/// ブーリアン演算後のポリゴン辺と元の `QuadBez` の照合を O(1) セル検索に高速化する構造体
struct QuadSpatialGrid {
    min_x: f64,
    min_y: f64,
    inv_cell_w: f64,
    inv_cell_h: f64,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<usize>>,
}

impl QuadSpatialGrid {
    fn new(quad_table: &[QuadRecord]) -> Self {
        if quad_table.is_empty() {
            return Self {
                min_x: 0.0,
                min_y: 0.0,
                inv_cell_w: 1.0,
                inv_cell_h: 1.0,
                cols: 1,
                rows: 1,
                cells: vec![Vec::new()],
            };
        }
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for rec in quad_table {
            min_x = min_x.min(rec.bbox.x0);
            min_y = min_y.min(rec.bbox.y0);
            max_x = max_x.max(rec.bbox.x1);
            max_y = max_y.max(rec.bbox.y1);
        }
        let side = (quad_table.len() as f64).sqrt().ceil() as usize;
        let cols = side.clamp(1, 32);
        let rows = side.clamp(1, 32);
        let span_x = (max_x - min_x).max(1.0);
        let span_y = (max_y - min_y).max(1.0);
        let inv_cell_w = (cols as f64) / span_x;
        let inv_cell_h = (rows as f64) / span_y;
        let mut cells = vec![Vec::new(); cols * rows];

        for (qid, rec) in quad_table.iter().enumerate() {
            let cx0 = (((rec.bbox.x0 - min_x) * inv_cell_w).floor() as isize)
                .clamp(0, cols as isize - 1) as usize;
            let cx1 = (((rec.bbox.x1 - min_x) * inv_cell_w).floor() as isize)
                .clamp(0, cols as isize - 1) as usize;
            let cy0 = (((rec.bbox.y0 - min_y) * inv_cell_h).floor() as isize)
                .clamp(0, rows as isize - 1) as usize;
            let cy1 = (((rec.bbox.y1 - min_y) * inv_cell_h).floor() as isize)
                .clamp(0, rows as isize - 1) as usize;
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    cells[cy * cols + cx].push(qid);
                }
            }
        }

        Self {
            min_x,
            min_y,
            inv_cell_w,
            inv_cell_h,
            cols,
            rows,
            cells,
        }
    }

    #[inline]
    fn candidates_at(&self, pt: Point) -> &[usize] {
        let cx = ((pt.x - self.min_x) * self.inv_cell_w).floor() as isize;
        let cy = ((pt.y - self.min_y) * self.inv_cell_h).floor() as isize;
        if cx < 0 || cx >= self.cols as isize || cy < 0 || cy >= self.rows as isize {
            return &[];
        }
        &self.cells[(cy as usize) * self.cols + (cx as usize)]
    }
}

#[derive(Clone, Copy, Debug)]
struct TaggedEdge {
    from: Point,
    to: Point,
    quad_match: Option<(usize, f64)>,
}

fn flatten_contour_for_overlay(
    contour: &Contour,
    is_outer: bool,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table)?;
    let area = signed_area_f64(&ring);
    if area.abs() < 1e-6 {
        return None;
    }
    if (is_outer && area > 0.0) || (!is_outer && area < 0.0) {
        ring.reverse();
    }
    Some(ring)
}

fn flatten_contour_preserving_direction(
    contour: &Contour,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    let mut ring = sample_contour_ring_for_overlay(contour, quad_table)?;
    ring.reverse();
    Some(ring)
}

fn flatten_shape_for_overlay(
    shape: &FillShape,
    quad_table: &mut Vec<QuadRecord>,
) -> OverlayShape {
    let Some(ext_ring) = flatten_contour_for_overlay(&shape.exterior, true, quad_table) else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) = flatten_contour_for_overlay(hole, false, quad_table) {
            rings.push(hole_ring);
        }
    }
    rings
}

/// 輪郭変形（エッジつまみ変形・頂点移動）後のシェイプの自己交差・反対側輪郭の突き抜けを解消し、
/// 正のワインディング領域（`OverlayFillRule::Positive`）のみを残して1つまたは複数の `FillShape` に分割・再構築する。
pub(crate) fn resolve_deformed_shape(shape: &FillShape) -> Vec<FillShape> {
    let mut quad_table: Vec<QuadRecord> = Vec::new();
    let Some(ext_ring) = flatten_contour_preserving_direction(&shape.exterior, &mut quad_table)
    else {
        return Vec::new();
    };
    let mut rings = Vec::with_capacity(1 + shape.holes.len());
    rings.push(ext_ring);
    for hole in &shape.holes {
        if let Some(hole_ring) = flatten_contour_preserving_direction(hole, &mut quad_table) {
            rings.push(hole_ring);
        }
    }
    let min_area = adaptive_overlay_min_area(std::slice::from_ref(shape), &[]);
    let overlay_result = run_float_overlay(
        &[rings],
        &[],
        OverlayRule::Subject,
        OverlayFillRule::Positive,
        min_area,
    );
    reconstruct_shapes_from_overlay(&overlay_result, &quad_table, shape.color, false)
        .into_iter()
        .map(|s| s.with_scope(shape.layer_id, &shape.group_path))
        .collect()
}

fn quad_subdivision_steps(seg: QuadBez) -> usize {
    let chord_len = (seg.p2 - seg.p0).hypot();
    let mid_dev = (seg.p1 - seg.p0.lerp(seg.p2, 0.5)).hypot();
    let flat_tol = 0.04_f64.min((chord_len * 0.015).max(1e-4));
    if mid_dev <= flat_tol {
        2
    } else {
        let arc = seg.arclen(0.5);
        let step_len = 1.2_f64.min((arc * 0.125).max(0.02));
        ((arc / step_len).ceil() as usize).clamp(4, 256)
    }
}

fn sample_contour_ring_for_overlay(
    contour: &Contour,
    quad_table: &mut Vec<QuadRecord>,
) -> Option<Vec<[f64; 2]>> {
    if contour.segments.is_empty() {
        return None;
    }

    let mut raw_pts = Vec::new();
    for &seg in contour.segments.iter() {
        if !seg.p0.x.is_finite()
            || !seg.p0.y.is_finite()
            || !seg.p1.x.is_finite()
            || !seg.p1.y.is_finite()
            || !seg.p2.x.is_finite()
            || !seg.p2.y.is_finite()
        {
            return None;
        }
        let min_x = seg.p0.x.min(seg.p1.x).min(seg.p2.x) - 0.6;
        let max_x = seg.p0.x.max(seg.p1.x).max(seg.p2.x) + 0.6;
        let min_y = seg.p0.y.min(seg.p1.y).min(seg.p2.y) - 0.6;
        let max_y = seg.p0.y.max(seg.p1.y).max(seg.p2.y) + 0.6;
        let bbox = KRect::new(min_x, min_y, max_x, max_y);
        quad_table.push(QuadRecord { quad: seg, bbox });

        let steps = quad_subdivision_steps(seg);
        for k in 0..steps {
            raw_pts.push(seg.eval(k as f64 / steps as f64));
        }
    }

    let deduped = dedup_closed_ring(raw_pts, 1e-4);
    if deduped.len() < 3 {
        return None;
    }

    Some(deduped.into_iter().map(|p| [p.x, p.y]).collect())
}

fn reconstruct_or_refit_quad_contour(
    ring_f64: &[[f64; 2]],
    quad_table: &[QuadRecord],
    grid: &QuadSpatialGrid,
    is_outer: bool,
) -> Option<Contour> {
    let pts = dedup_closed_ring(ring_f64.iter().map(|&[x, y]| Point::new(x, y)), 1e-4);
    let n = pts.len();
    if n < 3 {
        return None;
    }

    let tagged_edges: Vec<TaggedEdge> = (0..n)
        .map(|i| {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            TaggedEdge {
                from: a,
                to: b,
                quad_match: match_edge_to_quad(a, b, quad_table, grid),
            }
        })
        .collect();

    if tagged_edges.iter().all(|e| e.quad_match.is_none()) {
        let r_est = estimate_brush_radius_from_ring(&pts);
        return smooth_ring_to_quad_contour(&pts, r_est, is_outer);
    }

    let shift = (0..n)
        .find(|&i| {
            tagged_edges[i].quad_match.map(|(id, _)| id)
                != tagged_edges[(i + n - 1) % n].quad_match.map(|(id, _)| id)
        })
        .unwrap_or(0);

    let rotated: Vec<TaggedEdge> = (0..n).map(|i| tagged_edges[(i + shift) % n]).collect();
    let mut segments: Vec<QuadBez> = Vec::new();
    let mut i = 0;

    let (_, ring_perim) = ring_area_and_perimeter_f64(ring_f64);
    let scale = if ring_perim < 4.0 {
        (ring_perim / 4.0).clamp(0.02, 1.0)
    } else {
        1.0
    };
    let refit_eps = 0.35 * scale;
    let min_seg_len = 0.02 * scale;
    let min_area = 0.04 * scale * scale;

    while i < n {
        if let Some((qid, mut prev_t)) = rotated[i].quad_match {
            let orig_quad = quad_table[qid].quad;
            let run_start_pt = rotated[i].from;
            let mut run_end_pt = rotated[i].to;
            let mut run_len = 1usize;

            while i + run_len < n {
                let Some((next_qid, next_t)) = rotated[i + run_len].quad_match else {
                    break;
                };
                if next_qid != qid || (next_t - prev_t).abs() > 0.65 {
                    break;
                }
                run_end_pt = rotated[i + run_len].to;
                prev_t = next_t;
                run_len += 1;
            }

            if shift == 0 && i == 0 && run_len == n {
                let mid_pt = rotated[n / 2].from;
                segments.push(extract_sub_quad(orig_quad, run_start_pt, mid_pt));
                segments.push(extract_sub_quad(orig_quad, mid_pt, run_start_pt));
                break;
            }

            segments.push(extract_sub_quad(orig_quad, run_start_pt, run_end_pt));
            i += run_len;
        } else {
            let mut chain_pts = vec![rotated[i].from, rotated[i].to];
            let mut run_len = 1usize;
            while i + run_len < n && rotated[i + run_len].quad_match.is_none() {
                chain_pts.push(rotated[i + run_len].to);
                run_len += 1;
            }
            segments.extend(refit_open_polyline_to_quads(&chain_pts, refit_eps));
            i += run_len;
        }
    }

    if segments.len() == 1 {
        let single = segments[0];
        segments = vec![single.subsegment(0.0..0.5), single.subsegment(0.5..1.0)];
    }

    if segments.len() < 2 {
        let r_est = estimate_brush_radius_from_ring(&pts);
        return smooth_ring_to_quad_contour(&pts, r_est, is_outer);
    }

    Contour::from_segments_oriented(segments, min_seg_len, min_area, is_outer)
}

fn match_edge_to_quad(
    a: Point,
    b: Point,
    quad_table: &[QuadRecord],
    grid: &QuadSpatialGrid,
) -> Option<(usize, f64)> {
    let chord = b - a;
    let chord_len = chord.hypot();
    if chord_len < 1e-5 {
        return None;
    }
    let chord_dir = chord / chord_len;
    let mid = a.lerp(b, 0.5);

    let dist_tol = 0.45_f64.min((chord_len * 0.35).max(1e-4));
    let dist_tol_sq = dist_tol * dist_tol;
    let nearest_acc = (dist_tol_sq * 0.01).clamp(1e-7, 1e-3);
    let mut best_match = None;
    let mut best_score = f64::INFINITY;

    for &qid in grid.candidates_at(mid) {
        let rec = &quad_table[qid];
        if !rec.bbox.contains(mid) {
            continue;
        }
        let n_mid = rec.quad.nearest(mid, nearest_acc);
        if n_mid.distance_sq > dist_tol_sq {
            continue;
        }
        let n_a = rec.quad.nearest(a, nearest_acc);
        if n_a.distance_sq > dist_tol_sq {
            continue;
        }
        let n_b = rec.quad.nearest(b, nearest_acc);
        if n_b.distance_sq > dist_tol_sq {
            continue;
        }

        let deriv = rec.quad.deriv().eval(n_mid.t).to_vec2();
        let deriv_len = deriv.hypot();
        if deriv_len > 1e-6 && chord_dir.dot(deriv / deriv_len).abs() < 0.90 {
            continue;
        }

        let score = n_mid.distance_sq + 0.5 * (n_a.distance_sq + n_b.distance_sq);
        if score < best_score {
            best_score = score;
            best_match = Some((qid, n_b.t));
        }
    }

    best_match
}

fn refit_open_polyline_to_quads(pts: &[Point], epsilon: f64) -> Vec<QuadBez> {
    let n = pts.len();
    if n < 2 {
        return Vec::new();
    }
    if n == 2 {
        return vec![QuadBez::new(pts[0], pts[0].lerp(pts[1], 0.5), pts[1])];
    }

    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;

    for i in 1..(n - 1) {
        let d_in = normalize_vec(pts[i] - pts[i - 1]);
        let d_out = normalize_vec(pts[i + 1] - pts[i]);
        if d_in.dot(d_out) < 0.57 {
            keep[i] = true;
        }
    }

    let mut seg_start = 0;
    for i in 1..n {
        if keep[i] {
            rdp_recursive(pts, seg_start, i, epsilon, &mut keep);
            seg_start = i;
        }
    }

    let anchors: Vec<usize> = (0..n).filter(|&i| keep[i]).collect();
    let mut cum = Vec::new();
    anchors
        .windows(2)
        .map(|w| {
            let span = &pts[w[0]..=w[1]];
            let p0 = pts[w[0]];
            let p2 = pts[w[1]];
            fill_cumulative_arc_lengths(span, &mut cum);
            let p1 = fit_quad_control_point_with_arc_lengths(span, &cum, p0, p2);
            QuadBez::new(p0, p1, p2)
        })
        .collect()
}

fn extract_sub_quad(orig: QuadBez, p_start: Point, p_end: Point) -> QuadBez {
    let t0 = orig.nearest(p_start, 1e-6).t.clamp(0.0, 1.0);
    let t1 = orig.nearest(p_end, 1e-6).t.clamp(0.0, 1.0);

    if (t1 - t0).abs() < 1e-4 {
        return QuadBez::new(p_start, p_start.lerp(p_end, 0.5), p_end);
    }

    let mut sub = orig.subsegment(t0..t1);
    sub.p0 = p_start;
    sub.p2 = p_end;
    sub
}
