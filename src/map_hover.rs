//! カーソル合わせ：編集中の階層の生の図形の地図（セル画の地図・浮いた片の残りと片の地図）で、マウスに近い頂点・辺を探す
//! （輪郭の変形・アンカーの追加・削除）。場面（`Scene`）と作業状態（`Session`）を使うので、地図（`planar_map`）の上の層に置く

use std::collections::HashSet;
use std::sync::Arc;

use kurbo::{Affine, ParamCurve, Point, QuadBez, Vec2 as KVec2};

use crate::canvas_tool::{HoverTarget, MapOwner, Scene, Session};
use crate::planar_map::{is_map_shape, per_layer_parallel, LayerBase};
use crate::vector_engine::{quad_deriv, FillShape, LayerId, ShapeId};

/// 1つのレイヤーの、編集中の階層の生の図形から作った地図
pub(crate) struct LayerMap {
    pub(crate) layer_id: LayerId,
    pub(crate) group_path: Vec<u64>,
    /// 文書の座標の地図
    pub(crate) base: Arc<LayerBase>,
    /// 地図を置く座標の変換（シンボルの中ではステージへの変換。カーソル合わせの距離と表示は、置いた座標で測る）
    xform: Affine,
    /// 場面の並びで、このレイヤーの生の図形が最初に出てくる番号（これより後ろの、ほかの図形が前面）
    first_idx: usize,
    /// 地図の持ち主（編集するとき、どの地図か）
    pub(crate) owner: MapOwner,
}

impl LayerMap {
    /// 辺 `e` の、置いた座標の形
    fn placed_quad(&self, e: usize) -> QuadBez {
        self.xform * self.base.map.quad(e)
    }

    /// 頂点 `v` の、置いた座標の位置
    fn placed_node(&self, v: usize) -> Point {
        self.xform * self.base.map.nodes[v]
    }
}

/// カーソル合わせ用の地図（場面の生の図形が変わるまで使い回す）
#[derive(Default)]
pub(crate) struct ScopeMaps {
    key: u64,
    pub(crate) maps: Vec<LayerMap>,
}

/// 編集中の階層の生の図形（浮いた片を除く）の、レイヤーごとの地図：画面に出しているセル画の地図（`Cel::scope_base`）。
/// 浮いた片を持ち上げたレイヤーは、その残りの地図。場面の生の図形が前回と同じなら、前回の並びをそのまま返す
pub(crate) fn scope_maps(scene: Scene<'_>, session: &Session) -> Arc<ScopeMaps> {
    use std::hash::{Hash, Hasher};
    let scope = &session.editing_group_path;
    let xform = session.edit_path.world_xform();
    // 動かしていない浮いた片は、どの操作でも先に戻してから地図を編集する（`apply_anchor_edit_at`）ので、カーソル合わせも
    // 戻した後の地図（セル画の地図）で行う。動かした浮いた片は、残りの地図と片の地図（`floating_maps`）で行う
    let floating = session.floating().filter(|fl| fl.modified);
    let pieces: HashSet<ShapeId> = floating.map(|fl| fl.piece_ids()).unwrap_or_default();
    // 浮いた片（ほかの図形と重なることがある）は入れない
    let in_map = |s: &FillShape| is_map_shape(s, scope) && !pieces.contains(&s.shape_id);
    let mut by_layer: Vec<(LayerId, usize, Vec<&FillShape>)> = Vec::new();
    for (idx, s) in scene.iter().enumerate().filter(|(_, s)| in_map(s)) {
        match by_layer.iter_mut().find(|x| x.0 == s.layer_id) {
            Some(x) => x.2.push(s),
            None => by_layer.push((s.layer_id, idx, vec![s])),
        }
    }
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            scope,
            xform.as_coeffs().map(f64::to_bits),
            floating.is_some(),
        )
            .hash(&mut h);
        for (layer_id, first_idx, shapes) in &by_layer {
            (layer_id, first_idx).hash(&mut h);
            for s in shapes {
                (s.shape_id, s.mesh_revision).hash(&mut h);
            }
        }
        h.finish()
    };
    let mut cache = session.planar_maps.lock();
    if let Some(c) = cache.as_ref().filter(|c| c.key == key) {
        return c.clone();
    }
    // 地図にできないレイヤー（格子に置けない座標がある）は入れない（カーソル合わせも変形もしない）。作り直すレイヤーが
    // いくつあっても待つ時間が増えないよう、同時に作る
    let maps = per_layer_parallel(&by_layer, |(layer_id, first_idx, _)| {
        let rest = floating
            .and_then(|fl| fl.layers().iter().find(|l| l.layer_id == *layer_id))
            .and_then(|l| l.origin.as_ref())
            .map(|o| o.base());
        let base = match rest {
            Some(rest) => rest,
            None => session.base_cache.cel(*layer_id)?.scope_base(scope),
        };
        Some(LayerMap {
            layer_id: *layer_id,
            group_path: scope.clone(),
            base,
            xform,
            first_idx: *first_idx,
            owner: MapOwner::Layer(*layer_id),
        })
    })
    .into_iter()
    .flatten()
    .collect();
    let built = Arc::new(ScopeMaps { key, maps });
    *cache = Some(built.clone());
    built
}

/// 地図 `lm` の頂点 `v` を通る面の図形と、その図形の輪郭のうち `v` を通るものの番号。カーソルを合わせた頂点の輪郭を
/// 見せるのに使う。面が無ければ番号 0
fn node_on_shape(scene: Scene<'_>, lm: &LayerMap, v: usize) -> (ShapeId, usize) {
    let map = &lm.base.map;
    let Some(id) = map
        .edges
        .iter()
        .filter(|e| e.a == v || e.b == v)
        .find_map(|e| e.left_src.or(e.right_src))
    else {
        return (0, 0);
    };
    let p = lm.placed_node(v);
    let at = scene.shape_by_id(id).and_then(|s| {
        s.contours()
            .position(|c| c.segments.iter().any(|q| q.p0 == p))
    });
    (id, at.unwrap_or(0))
}

/// 点 `p` が、地図 `lm` のレイヤーの生の図形より前面にある、ほかの図形（前面のレイヤー・同じレイヤーのグループ）に覆われているか
fn hidden_by_front(scene: Scene<'_>, session: &Session, lm: &LayerMap, p: Point) -> bool {
    (lm.first_idx + 1..scene.len()).any(|i| {
        scene.get(i).is_some_and(|s| {
            !(s.layer_id == lm.layer_id && s.is_raw_at_scope(&lm.group_path))
                && s.is_in_editing_scope(&session.editing_group_path)
                && s.contains_point(p)
        })
    })
}

/// アンカーの追加（`add`）・削除で、マウスにいちばん近い辺・頂点を探す（半径 `12 / zoom`。同じ地図の図形どうしに前後は無い）。
/// 動かして浮いている片（ほかの図形と重なることがある）は、レイヤーごとに片だけの地図で先に探し（`floating_maps`）、
/// 無ければレイヤーの地図で探す（`scope_maps`）
pub(crate) fn anchor_hover(
    scene: Scene<'_>,
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
    add: bool,
) -> HoverTarget {
    let r = 12.0 / (zoom as f64);
    let floating = floating_maps(scene, session);
    match anchor_hover_in(scene, session, &floating, mouse_pt, r, add) {
        HoverTarget::None => {
            let maps = scope_maps(scene, session);
            anchor_hover_in(scene, session, &maps.maps, mouse_pt, r, add)
        }
        found => found,
    }
}

/// 地図 `maps` で、マウスにいちばん近い辺・頂点を探す（`anchor_hover`）。追加は、足せる所（`with_anchor_added` が形を
/// 返す所）だけ。削除は、消せる頂点（`with_anchor_deleted` が形を返す頂点）だけ
fn anchor_hover_in(
    scene: Scene<'_>,
    session: &Session,
    maps: &[LayerMap],
    mouse_pt: Point,
    r: f64,
    add: bool,
) -> HoverTarget {
    if add {
        let mut best: Option<(f64, &LayerMap, usize, f64)> = None;
        for lm in maps {
            let Some((e, t)) = lm.base.map.hit_edge(mouse_pt, r, lm.xform) else {
                continue;
            };
            let p = lm.placed_quad(e).eval(t);
            let d = (p - mouse_pt).hypot();
            if best.as_ref().is_none_or(|b| d < b.0)
                && !hidden_by_front(scene, session, lm, p)
                && lm.base.map.with_anchor_added(e, t).is_some()
            {
                best = Some((d, lm, e, t));
            }
        }
        return best.map_or(HoverTarget::None, |(_, lm, e, t)| {
            let q = lm.placed_quad(e);
            let ed = &lm.base.map.edges[e];
            HoverTarget::Edge {
                shape_id: ed.left_src.or(ed.right_src).unwrap_or(0),
                t,
                closest_pos: q.eval(t),
                seg: q,
                map_edge: Some((lm.owner, lm.base.map.quad(e))),
            }
        });
    }
    let mut best: Option<(f64, &LayerMap, usize)> = None;
    for lm in maps {
        let Some(v) = lm.base.map.hit_node(mouse_pt, r, lm.xform) else {
            continue;
        };
        let p = lm.placed_node(v);
        let d = (p - mouse_pt).hypot();
        if best.as_ref().is_none_or(|b| d < b.0)
            && !hidden_by_front(scene, session, lm, p)
            && lm.base.map.with_anchor_deleted(v).is_some()
        {
            best = Some((d, lm, v));
        }
    }
    best.map_or(HoverTarget::None, |(_, lm, v)| {
        let (shape_id, contour_idx) = node_on_shape(scene, lm, v);
        HoverTarget::Vertex {
            shape_id,
            contour_idx,
            pos: lm.placed_node(v),
            map_node: Some((lm.owner, v, lm.base.map.nodes[v])),
        }
    })
}

/// 動かして浮いている片の、片の組ごとの地図（片の地図を「ステージへの変換 ∘ 片の変換」で置く）
fn floating_maps(scene: Scene<'_>, session: &Session) -> Vec<LayerMap> {
    let Some(fl) = session.floating().filter(|fl| fl.modified) else {
        return Vec::new();
    };
    let world = session.edit_path.world_xform();
    let mut out = Vec::new();
    for l in fl.layers() {
        for (gi, g) in l.groups.iter().enumerate() {
            let Some(first_idx) = g.first_id().and_then(|id| scene.index_of_shape_id(id)) else {
                continue;
            };
            out.push(LayerMap {
                layer_id: l.layer_id,
                group_path: session.editing_group_path.clone(),
                base: g.base(),
                xform: world * g.xform,
                first_idx,
                owner: MapOwner::Floating(l.layer_id, gi),
            });
        }
    }
    out
}

/// 選択ツールの輪郭の変形で、マウスにいちばん近い頂点か辺を探す。
/// 同じレイヤーの生の図形どうしに前後は無い（境目の線は1本）。前面のレイヤーやグループに隠れた所は除く
pub(crate) fn reshape_hover(
    scene: Scene<'_>,
    session: &Session,
    mouse_pt: Point,
    zoom: f32,
) -> HoverTarget {
    let inv_z = 1.0 / (zoom as f64);
    let (vertex_r, inner_r, outer_r) = (9.0 * inv_z, 5.5 * inv_z, 9.5 * inv_z);
    let maps = scope_maps(scene, session);
    let hidden = |lm: &LayerMap, p: Point| hidden_by_front(scene, session, lm, p);
    let mut best_v: Option<(f64, &LayerMap, usize)> = None;
    for lm in &maps.maps {
        if let Some(v) = lm.base.map.hit_node(mouse_pt, vertex_r, lm.xform) {
            let p = lm.placed_node(v);
            let d = (p - mouse_pt).hypot();
            if best_v.as_ref().is_none_or(|b| d < b.0) && !hidden(lm, p) {
                best_v = Some((d, lm, v));
            }
        }
    }
    if let Some((_, lm, v)) = best_v {
        let (shape_id, contour_idx) = node_on_shape(scene, lm, v);
        return HoverTarget::Vertex {
            shape_id,
            contour_idx,
            pos: lm.placed_node(v),
            map_node: Some((lm.owner, v, lm.base.map.nodes[v])),
        };
    }
    let mut best_e: Option<(f64, &LayerMap, usize, f64, ShapeId)> = None;
    for lm in &maps.maps {
        let Some((e, t)) = lm.base.map.hit_edge(mouse_pt, outer_r, lm.xform) else {
            continue;
        };
        let q = lm.placed_quad(e);
        let p = q.eval(t);
        let d = (p - mouse_pt).hypot();
        let tan = quad_deriv(&q, t);
        // 辺の左右は地図の座標で決まる（裏返して置いた地図では、置いた座標の左右と逆になる）
        let on_left = ((mouse_pt - p).dot(KVec2::new(-tan.y, tan.x)) >= 0.0)
            == (lm.xform.determinant() >= 0.0);
        let ed = &lm.base.map.edges[e];
        let (side, side_src) = if on_left {
            (ed.left, ed.left_src)
        } else {
            (ed.right, ed.right_src)
        };
        let max = if side.is_some() { inner_r } else { outer_r };
        let src = side_src.or(ed.left_src).or(ed.right_src).unwrap_or(0);
        if d <= max && best_e.as_ref().is_none_or(|b| d < b.0) && !hidden(lm, p) {
            best_e = Some((d, lm, e, t, src));
        }
    }
    if let Some((_, lm, e, t, shape_id)) = best_e {
        let q = lm.placed_quad(e);
        return HoverTarget::Edge {
            shape_id,
            t,
            closest_pos: q.eval(t),
            seg: q,
            map_edge: Some((lm.owner, lm.base.map.quad(e))),
        };
    }
    HoverTarget::None
}
