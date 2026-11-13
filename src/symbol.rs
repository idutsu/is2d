use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, Mesh, Vec2};
use kurbo::{Affine, Point};

use crate::planar_map::LayerBase;
#[cfg(test)]
pub(crate) use crate::timeline::interpolate_affine_with_pivot;
use crate::timeline::{Cel, Layer, Timeline};
pub(crate) use crate::timeline::{ResolvedCelSig, SymbolInstance, TweenRotate, TweenState};
use crate::vector_engine::{
    dim_backdrop_color, next_shape_id, transform_mesh_to_screen, union_bounding_boxes, Contour,
    FillShape, SyncCache,
};
pub(crate) use crate::vector_engine::{ColorEffect, SymbolInstanceBinding, SymbolLoopMode};

// ============================================================================
// 1. シンボル定義（SymbolDef）・インスタンス紐付け・トゥイーン状態・編集セッション
// ============================================================================

impl SymbolInstanceBinding {
    /// このインスタンスの現在のワールド座標における変形中心点（ピボット）を返す
    #[inline]
    pub(crate) fn world_pivot(self) -> Point {
        self.xform * self.pivot
    }

    /// ローカル座標系のシェイプ列 `local_shapes` を、このインスタンスの変換行列 `self.xform`・`layer_id`・
    /// スコープパス `scope_prefix`（末尾に `self.instance_gid` を自動付与）でステージ座標へ展開する
    pub(crate) fn instantiate_shapes(
        self,
        local_shapes: &[FillShape],
        layer_id: u64,
        scope_prefix: &[u64],
        with_mesh: bool,
    ) -> Vec<FillShape> {
        let mut out = Vec::with_capacity(local_shapes.len());
        for s in local_shapes {
            if with_mesh {
                let _ = s.fill_mesh();
            }
            let mut inst_s = s.clone();
            inst_s.shape_id = next_shape_id();
            let mut full_path =
                Vec::with_capacity(scope_prefix.len() + 1 + inst_s.group_path.len());
            full_path.extend_from_slice(scope_prefix);
            full_path.push(self.instance_gid);
            full_path.extend(inst_s.group_path.iter().copied());
            inst_s.transform_with_cached_mesh(self.xform);
            inst_s.layer_id = layer_id;
            inst_s.group_path = full_path;
            inst_s.symbol_instance = Some(self);
            if self.color_effect != ColorEffect::default() {
                inst_s.set_color(self.color_effect.apply(inst_s.color));
            }
            out.push(inst_s);
        }
        out
    }

    pub(crate) fn expand(
        self,
        (kf_start_frame, frame): (usize, usize),
        layer_id: u64,
        parent_scope: &[u64],
        ctx: SymbolEvalCtx<'_>,
    ) -> Vec<FillShape> {
        let with_mesh = ctx.with_mesh;
        let local_shapes = (ctx.depth < MAX_SYMBOL_NEST_DEPTH)
            .then(|| ctx.library.iter().find(|s| s.id == self.symbol_id))
            .flatten()
            .map(|sym| {
                sym.evaluated_local_shapes_for_instance(self, kf_start_frame, frame, ctx.nested())
            })
            .unwrap_or_default();
        if !local_shapes.is_empty() {
            self.instantiate_shapes(&local_shapes, layer_id, parent_scope, with_mesh)
        } else if ctx.depth == 0 {
            vec![FillShape::new_empty_symbol_marker(
                self,
                layer_id,
                parent_scope,
            )]
        } else {
            Vec::new()
        }
    }
}

impl FillShape {
    /// 現在フレームで内部図形が空白（0シェイプ）のシンボルインスタンスを、
    /// `CanvasState::shapes` 内で他の図形と同一の一元配列上で管理するための非表示マーカー要素を生成する
    pub(crate) fn new_empty_symbol_marker(
        binding: SymbolInstanceBinding,
        layer_id: u64,
        parent_scope: &[u64],
    ) -> Self {
        let mut group_path = Vec::with_capacity(parent_scope.len() + 1);
        group_path.extend_from_slice(parent_scope);
        group_path.push(binding.instance_gid);
        let mut shape = Self::new(Color32::TRANSPARENT, Contour::new(Vec::new()), Vec::new())
            .with_scope(layer_id, &group_path);
        shape.symbol_instance = Some(binding);
        shape
    }
}

/// シンボル1つのプレビューの写し：鍵（シンボルの世代・ライブラリの世代・今のコマ）と、評価済みの外接矩形とシンボルの
/// 座標の結合メッシュ（中身が空なら無し）
#[derive(Clone, Debug)]
struct SymbolPreview {
    key: (u64, u64, usize),
    bounds_and_mesh: Option<(kurbo::Rect, egui::Mesh)>,
}

/// ライブラリのシンボルのミニプレビューの写し（シンボルの番号ごと）。画面の写しなので、文書の外（タブの表示状態）に持つ
/// （文書の型に置くと、Undo の控えとライブラリを書くときに写される：レビュー 6ee325a の N-2003）。画面座標への変換は
/// 描くたびに行う（`egui::Shape::Mesh` がメッシュを値で受け取るので、変換済みを持っても複製は減らない）
#[derive(Clone, Default)]
pub(crate) struct SymbolPreviews(SyncCache<HashMap<u64, SymbolPreview>>);

impl SymbolPreviews {
    /// シンボル `sym` のプレビューの外接矩形とメッシュ（シンボルの座標）を `f` に渡す。鍵が変わっていれば作り直す。
    /// `None`：中身が空
    pub(crate) fn with<R>(
        &self,
        sym: &SymbolDef,
        library_revision: u64,
        library: &Library,
        f: impl FnOnce(kurbo::Rect, &egui::Mesh) -> R,
    ) -> Option<R> {
        let key = (sym.revision, library_revision, sym.timeline.current_frame);
        let mut all = self.0.lock();
        if all.get(&sym.id).is_none_or(|p| p.key != key) {
            let mut shapes = sym.evaluated_local_shapes_at(sym.timeline.current_frame, library);
            if shapes.is_empty() {
                shapes = sym.first_non_empty_local_shapes(library);
            }
            let bounds_and_mesh = (!shapes.is_empty()).then(|| {
                let mut batch = egui::Mesh::default();
                for s in &shapes {
                    s.append_fill_mesh_to(&mut batch);
                }
                (union_bounding_boxes(&shapes), batch)
            });
            all.insert(
                sym.id,
                SymbolPreview {
                    key,
                    bounds_and_mesh,
                },
            );
        }
        let (bounds, mesh) = all.get(&sym.id)?.bounds_and_mesh.as_ref()?;
        Some(f(*bounds, mesh))
    }
}

/// シンボルのライブラリ。塊ごとに Undo の控えと共有する列（`ChunkedList`。書くのは触った塊と、そのシンボルだけ）
pub(crate) type Library = crate::chunked_list::ChunkedList<SymbolDef>;

/// ライブラリに登録されたシンボル定義（独自の `Timeline` とローカル基準点 `(0, 0)` を持つ）
#[derive(Clone, Debug)]
pub(crate) struct SymbolDef {
    pub(crate) id: u64,
    pub(crate) name: String,
    /// シンボル内部の独自タイムライン（各シェイプは基準点 `(0, 0)` 中心のローカル座標系）
    pub(crate) timeline: Timeline,
    /// シンボル定義の変更世代番号（解決済みセルメッシュキャッシュの署名計算用）
    pub(crate) revision: u64,
}

pub(crate) const MAX_SYMBOL_NEST_DEPTH: usize = 8;

type SymbolEvalMemoKey = (u64, usize, usize, usize);

/// シンボルの中身を評価した結果（シンボルの座標の図形。画面に出すときはメッシュも持つ）のメモ。
/// `SymbolEvalCtx::new` のものは1回の評価の間だけ使う。セッションが持つもの（`Session::symbol_memo`）は、
/// 描くたびの評価（オニオンスキン・再生・ロック中のレイヤー）で使い回し、次の2つの決まりで捨てる
/// - ライブラリのどれかのシンボルが変わったら（ID と世代番号の並びが変わったら）全部捨てる
/// - 直前の1回の描画で使わなかったものは捨てる（持つのは、今の画面が使う分だけ）
#[derive(Default)]
pub(crate) struct SymbolEvalState {
    /// この回の描画で使ったもの
    memo: HashMap<SymbolEvalMemoKey, Arc<Vec<FillShape>>>,
    /// 前の回の描画で使ったもの（この回でも使うと `memo` へ移す）
    prev: HashMap<SymbolEvalMemoKey, Arc<Vec<FillShape>>>,
    library_sig: Vec<(u64, u64)>,
    /// この回に展開したセル画の地図の、計算に使う写し（写しを持ち続けるのは、受け取った所：画面の土台）
    held: Vec<Arc<LayerBase>>,
}

impl SymbolEvalState {
    /// 1回の描画の始めに呼ぶ
    pub(crate) fn start_pass(&mut self, library: &Library) {
        self.held.clear();
        let same_library = self.library_sig.len() == library.len()
            && self
                .library_sig
                .iter()
                .zip(library)
                .all(|(&(id, rev), s)| id == s.id && rev == s.revision);
        if same_library {
            self.prev = std::mem::take(&mut self.memo);
        } else {
            self.memo.clear();
            self.prev.clear();
            self.library_sig = library.iter().map(|s| (s.id, s.revision)).collect();
        }
    }

    /// この回に展開したセル画の地図の、計算に使う写し
    pub(crate) fn take_held(&mut self) -> Vec<Arc<LayerBase>> {
        std::mem::take(&mut self.held)
    }
}

/// シンボル再帰評価時の設定・メモ化コンテキスト
#[derive(Clone)]
pub(crate) struct SymbolEvalCtx<'a> {
    pub(crate) library: &'a Library,
    pub(crate) depth: usize,
    /// 展開した図形に画面表示用のメッシュを作るか（動画の書き出しでは作らない）
    with_mesh: bool,
    state: std::rc::Rc<std::cell::RefCell<SymbolEvalState>>,
}

impl<'a> SymbolEvalCtx<'a> {
    pub(crate) fn new(library: &'a Library) -> Self {
        Self::with_state(library, SymbolEvalState::default())
    }

    /// 前の評価のメモ `state` を引き継いで評価する（`SymbolEvalState::start_pass` を呼んでから渡す）
    pub(crate) fn with_state(library: &'a Library, state: SymbolEvalState) -> Self {
        Self {
            library,
            depth: 0,
            with_mesh: true,
            state: std::rc::Rc::new(std::cell::RefCell::new(state)),
        }
    }

    /// 評価を終えて、メモを返す。写し（`clone`・`nested`）はすべて評価の中で作って捨てるので、ここでは自分だけが持っている
    pub(crate) fn into_state(self) -> SymbolEvalState {
        std::rc::Rc::try_unwrap(self.state)
            .ok()
            .expect("評価の写しが残っている")
            .into_inner()
    }

    /// 画面表示用のメッシュを作らない（動画の書き出し用）
    pub(crate) fn without_mesh(mut self) -> Self {
        self.with_mesh = false;
        self
    }

    pub(crate) fn nested(&self) -> Self {
        Self {
            library: self.library,
            depth: self.depth + 1,
            with_mesh: self.with_mesh,
            state: std::rc::Rc::clone(&self.state),
        }
    }

    #[inline]
    fn get_memo(&self, key: &SymbolEvalMemoKey) -> Option<Arc<Vec<FillShape>>> {
        let mut state = self.state.borrow_mut();
        if let Some(hit) = state.memo.get(key) {
            return Some(Arc::clone(hit));
        }
        let hit = state.prev.remove(key)?;
        state.memo.insert(*key, Arc::clone(&hit));
        Some(hit)
    }

    #[inline]
    fn insert_memo(&self, key: SymbolEvalMemoKey, shapes: Arc<Vec<FillShape>>) {
        self.state.borrow_mut().memo.insert(key, shapes);
    }

    /// 展開したセル画の地図の、計算に使う写しを、この回の終わりまで持つ（`SymbolEvalState::take_held`）
    fn hold(&self, base: Arc<LayerBase>) {
        self.state.borrow_mut().held.push(base);
    }
}

/// 入れ子シンボル対応でセル画（階層の地図の面の図形と、シンボルインスタンス）を、背面→前面の重なりの順
/// （`Cel::parts`）に再帰展開して返す共通ヘルパー
pub(crate) fn resolve_cel_with_library(
    cel: &Cel,
    kf_start_frame: usize,
    frame: usize,
    ctx: SymbolEvalCtx<'_>,
) -> Vec<FillShape> {
    let mut out = Vec::new();
    for m in cel.maps.iter() {
        ctx.hold(m.base());
    }
    for part in cel.parts() {
        match part {
            crate::timeline::Part::Shape(s) => out.push((*s).clone()),
            crate::timeline::Part::Instance(inst) => {
                if ctx.depth < MAX_SYMBOL_NEST_DEPTH
                    && ctx.library.iter().any(|s| s.id == inst.binding.symbol_id)
                {
                    out.extend(inst.binding.expand(
                        (kf_start_frame, frame),
                        inst.layer_id,
                        &inst.parent_scope,
                        ctx.clone(),
                    ));
                }
            }
        }
    }
    out
}

impl SymbolDef {
    /// セル画 `cel`（シンボルの座標）を、1枚目のレイヤーの最初のキーフレームの中身にしたシンボル（シンボル化）
    pub(crate) fn from_cel(id: u64, name: String, mut cel: Cel) -> Self {
        if !cel.instances.is_empty() {
            for inst in cel.instances_mut() {
                inst.layer_id = 1;
            }
        }
        let mut timeline = Timeline::default();
        timeline.layers[0].keyframe_mut(0).cel = cel;
        Self::with_timeline(id, name, timeline)
    }

    /// テスト用：図形 `local_shapes`（シンボルの座標）を中身にしたシンボル（図形から地図を作る）。`Err`：地図にできない
    /// 形がある
    #[cfg(test)]
    pub(crate) fn new(
        id: u64,
        name: String,
        mut local_shapes: Vec<FillShape>,
    ) -> Result<Self, crate::timeline::Unmappable> {
        for s in &mut local_shapes {
            s.layer_id = 1;
            s.strip_transient_caches();
        }
        let mut timeline = Timeline::default();
        timeline.layers[0].keyframe_mut(0).cel = Cel::from_shapes(local_shapes)?;
        Ok(Self::with_timeline(id, name, timeline))
    }

    /// タイムライン `timeline` を中身にしたシンボル
    pub(crate) fn with_timeline(id: u64, name: String, timeline: Timeline) -> Self {
        Self {
            id,
            name,
            timeline,
            revision: crate::vector_engine::next_mesh_revision(),
        }
    }

    /// このシンボル自身、または内部の入れ子シンボルが複数フレーム（または先頭フレーム空白）を持つかを再帰判定する
    pub(crate) fn has_dynamic_animation(&self, library: &Library) -> bool {
        let mut known = Vec::new();
        self.has_dynamic_animation_inner(library, 0, &mut known)
    }

    /// 1回の判定の中で調べ終えた子のシンボル ID と結果 `known` を覚え、入れ子のインスタンスの数だけ同じ子を調べ直さないようにする
    fn has_dynamic_animation_inner(
        &self,
        library: &Library,
        depth: usize,
        known: &mut Vec<(u64, bool)>,
    ) -> bool {
        if depth >= MAX_SYMBOL_NEST_DEPTH {
            return false;
        }
        let frame_zero_empty = self
            .timeline
            .layers
            .iter()
            .all(|l| !l.visible || l.cel_at(0).is_none_or(|c| c.is_empty()));
        if self.timeline.total_frames > 1 || frame_zero_empty {
            return true;
        }
        self.timeline.instances().any(|inst| {
            let sid = inst.binding.symbol_id;
            if let Some(&(_, res)) = known.iter().find(|&&(id, _)| id == sid) {
                return res;
            }
            let res = library
                .iter()
                .find(|child| child.id == sid)
                .is_some_and(|child| child.has_dynamic_animation_inner(library, depth + 1, known));
            known.push((sid, res));
            res
        })
    }

    /// シンボルの全フレーム（`0..total_frames`）を先頭から走査し、最初に図形が存在するフレームのローカル図形列を返す
    pub(crate) fn first_non_empty_local_shapes(&self, library: &Library) -> Vec<FillShape> {
        let has_any_content = self
            .timeline
            .layers
            .iter()
            .any(|l| l.visible && l.keyframes.iter().any(|kf| !kf.cel.is_empty()));
        if !has_any_content {
            return Vec::new();
        }
        let ctx = SymbolEvalCtx::new(library).nested();
        let binding = SymbolInstanceBinding::new(0, self.id, Affine::IDENTITY);
        for f in 0..self.timeline.total_frames.max(1) {
            let shapes = self.evaluated_local_shapes_for_instance(binding, 0, f, ctx.clone());
            if !shapes.is_empty() {
                return Arc::unwrap_or_clone(shapes);
            }
        }
        Vec::new()
    }

    fn collect_visible_local_shapes_at(
        &self,
        frame: usize,
        child_eval_frame: usize,
        ctx: SymbolEvalCtx<'_>,
    ) -> Vec<FillShape> {
        let mut out = Vec::new();
        for layer in &self.timeline.layers {
            if layer.visible {
                out.extend(layer.evaluate_shapes_at((frame, child_eval_frame), 1, ctx.clone()));
            }
        }
        out
    }

    /// インスタンスの再生設定（`binding.loop_mode`, `binding.first_frame`）とキーフレーム開始フレーム `kf_start_frame`、
    /// 現在の親フレーム `current_frame` から、`(内部フレーム番号, 子インスタンスを動かす時計)` を返す
    pub(crate) fn resolve_internal_frame(
        &self,
        binding: SymbolInstanceBinding,
        kf_start_frame: usize,
        current_frame: usize,
    ) -> (usize, usize) {
        let fc = self.timeline.total_frames.max(1);
        let elapsed = current_frame.saturating_sub(kf_start_frame);
        let internal_frame = match binding.loop_mode {
            SymbolLoopMode::Loop => (binding.first_frame % fc + elapsed % fc) % fc,
            SymbolLoopMode::PlayOnce => binding
                .first_frame
                .min(fc - 1)
                .saturating_add(elapsed)
                .min(fc - 1),
            SymbolLoopMode::SingleFrame => binding.first_frame.min(fc - 1),
        };
        let child_eval_frame = if self.timeline.total_frames <= 1
            && binding.loop_mode != SymbolLoopMode::SingleFrame
        {
            binding.first_frame.saturating_add(elapsed)
        } else {
            internal_frame
        };
        (internal_frame % fc, child_eval_frame)
    }

    /// シンボルインスタンス `parent_binding` を現在コマ `current_frame`（親キーフレーム開始コマ `parent_kf_start`）で分解した
    /// ときの中身：見えているレイヤーごと（下から順）の、そのコマのセル画（展開と同じ決まりでコマを選ぶ：
    /// `resolve_internal_frame`）。トゥイーンの途中のレイヤーは、そのコマの結び付けのインスタンスだけ。中のインスタンスの
    /// 再生位置は、分解したあとも同じ絵になるように、親のキーフレームの始めから数え直す
    pub(crate) fn detached_cels(
        &self,
        parent_binding: SymbolInstanceBinding,
        parent_kf_start: usize,
        current_frame: usize,
        library: &Library,
    ) -> Vec<Cel> {
        let (frame_idx, child_eval_frame) =
            self.resolve_internal_frame(parent_binding, parent_kf_start, current_frame);
        let stage_elapsed = current_frame.saturating_sub(parent_kf_start);
        let mut out = Vec::new();
        for layer in self.timeline.layers.iter().filter(|l| l.visible) {
            let Some(kf_idx) = layer.keyframe_index_at(frame_idx) else {
                continue;
            };
            let kf = &layer.keyframes[kf_idx];
            let child_elapsed = child_eval_frame.saturating_sub(kf.start_frame);
            let mut cel = match layer.interpolated_tween_binding_at(frame_idx) {
                Some(b) => {
                    let mut c = Cel::default();
                    c.instances_mut().push(SymbolInstance {
                        binding: b,
                        layer_id: layer.id,
                        parent_scope: Vec::new(),
                        order: 0,
                    });
                    c
                }
                None => kf.cel.clone(),
            };
            if !cel.instances.is_empty() {
                for inst in cel.instances_mut() {
                    let b = &mut inst.binding;
                    let Some(child_sym) = library.iter().find(|s| s.id == b.symbol_id) else {
                        continue;
                    };
                    b.advance(child_elapsed);
                    b.rewind(stage_elapsed, child_sym.timeline.total_frames);
                }
            }
            if !cel.is_empty() {
                out.push(cel);
            }
        }
        out
    }

    /// インスタンスの再生設定（ループ再生 / 1回再生 / 任意のフレーム）に基づいて、
    /// 指定フレーム `current_frame`（開始キーフレーム `kf_start_frame`）でのローカル図形列を返す。
    /// `ctx` は、呼び出し元が1段 `nested()` したもの
    pub(crate) fn evaluated_local_shapes_for_instance(
        &self,
        binding: SymbolInstanceBinding,
        kf_start_frame: usize,
        current_frame: usize,
        ctx: SymbolEvalCtx<'_>,
    ) -> Arc<Vec<FillShape>> {
        let (frame_idx, child_eval_frame) =
            self.resolve_internal_frame(binding, kf_start_frame, current_frame);
        // `child_eval_frame`（子インスタンスを動かす時計）が結果を変えるのは、このフレームに子インスタンス
        // （トゥイーンを含む）があるときだけ。無いときは鍵に入れない（親のコマが進んでも同じ中身を使い回す）
        let uses_clock = self.timeline.layers.iter().any(|l| {
            l.visible
                && (l.interpolated_tween_binding_at(frame_idx).is_some()
                    || l.cel_at(frame_idx).is_some_and(|c| !c.instances.is_empty()))
        });
        let memo_key = (
            self.id,
            frame_idx,
            if uses_clock { child_eval_frame } else { 0 },
            ctx.depth,
        );
        if let Some(cached) = ctx.get_memo(&memo_key) {
            return cached;
        }
        let evaluated = Arc::new(self.collect_visible_local_shapes_at(
            frame_idx,
            child_eval_frame,
            ctx.clone(),
        ));
        ctx.insert_memo(memo_key, Arc::clone(&evaluated));
        evaluated
    }

    /// 指定されたフレーム番号 `offset_frame` に対応するシンボル内部フレーム（`offset_frame % total_frames` でループ評価）の
    /// 全可視レイヤーのローカル図形列（背面→前面順）を返す
    pub(crate) fn evaluated_local_shapes_at(
        &self,
        offset_frame: usize,
        library: &Library,
    ) -> Vec<FillShape> {
        let binding = SymbolInstanceBinding::new(0, self.id, Affine::IDENTITY);
        Arc::unwrap_or_clone(self.evaluated_local_shapes_for_instance(
            binding,
            0,
            offset_frame,
            SymbolEvalCtx::new(library).nested(),
        ))
    }
}

/// シンボルインプレイス編集の1段分の階層情報
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditLevel {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    /// 入った時点の、親の中での変換（トゥイーン中なら補間後の値）
    pub(crate) xform_in_parent: Affine,
    pub(crate) parent_frame: usize,
    pub(crate) parent_active_layer: usize,
    pub(crate) parent_group_path: Vec<u64>,
}

/// シンボルインプレイス編集の経路（空ならルート）
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EditPath {
    pub(crate) levels: Vec<EditLevel>,
}

impl EditPath {
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.levels.len()
    }

    #[inline]
    pub(crate) fn last(&self) -> Option<&EditLevel> {
        self.levels.last()
    }

    /// `levels` の変換を順に掛けたワールド（ステージ）変換行列を返す
    #[inline]
    pub(crate) fn world_xform(&self) -> Affine {
        self.levels
            .iter()
            .fold(Affine::IDENTITY, |acc, lvl| acc * lvl.xform_in_parent)
    }
}

/// 保存対象の文書状態（唯一の正解データ：ルートタイムライン・シンボルライブラリ・ステージサイズ・ID採番・リビジョン）
#[derive(Clone, Debug)]
pub(crate) struct Document {
    pub(crate) root: Timeline,
    /// Undo の控えと共有する（書く所だけ `Arc::make_mut` で写す。控えを取る・戻すは `Arc` の入れ替えだけ）
    pub(crate) library: Library,
    pub(crate) stage_size: Vec2,
    pub(crate) fps: u32,
    pub(crate) next_symbol_id: u64,
    pub(crate) next_group_id: u64,
    /// `library` の内容変更世代番号（シンボルのプレビューの一時キャッシュの鍵）
    pub(crate) revision: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            root: Timeline::default(),
            library: Library::default(),
            stage_size: Vec2::new(800.0, 450.0),
            fps: 24,
            next_symbol_id: 1,
            next_group_id: 1,
            revision: 0,
        }
    }
}

/// 入れ子をたどって、`start` から `is_target` に合うものに行き着くか（子は `children` が返す。同じものは1度だけ見る）。
/// シンボルの入れ子が輪になるかの判定はこれ1つ（文書のシンボル：`Document::would_create_symbol_cycle`、貼り付ける
/// クリップボードのシンボル：`clipboard_ops::can_paste_clipboard_symbol`）
pub(crate) fn reaches<N: Copy + Eq + std::hash::Hash>(
    start: N,
    is_target: impl Fn(N) -> bool,
    mut children: impl FnMut(N) -> Vec<N>,
) -> bool {
    let mut stack = vec![start];
    let mut seen: std::collections::HashSet<N> = std::collections::HashSet::from([start]);
    while let Some(n) = stack.pop() {
        if is_target(n) {
            return true;
        }
        for c in children(n) {
            if seen.insert(c) {
                stack.push(c);
            }
        }
    }
    false
}

impl Document {
    /// シンボルライブラリの世代番号 `revision` を一意な値へ更新する
    pub(crate) fn bump_revision(&mut self) {
        self.revision = crate::vector_engine::next_mesh_revision();
    }

    /// 新しいグループ / シンボルインスタンス用の一意な `group_id` を採番して返す
    pub(crate) fn alloc_group_id(&mut self) -> u64 {
        let id = self.next_group_id.max(1);
        self.next_group_id = if id < u64::MAX { id + 1 } else { 1 };
        id
    }

    /// 新しいシンボル定義用の一意な `symbol_id` を採番して返す
    pub(crate) fn alloc_symbol_id(&mut self) -> u64 {
        let id = self.next_symbol_id.max(1);
        self.next_symbol_id = if id < u64::MAX { id + 1 } else { 1 };
        id
    }

    /// 新しいシンボル定義 `sym_def`（`SymbolDef::from_cel`）をライブラリへ登録する。名前はここで付ける
    pub(crate) fn register_symbol(&mut self, mut sym_def: SymbolDef) {
        let sym_id = sym_def.id;
        let mut name_num = sym_id;
        while self
            .library
            .iter()
            .any(|s| s.name == format!("Symbol {name_num}"))
        {
            name_num = if name_num < u64::MAX { name_num + 1 } else { 1 };
        }
        sym_def.name = format!("Symbol {name_num}");
        self.library.push(Arc::new(sym_def));
        self.bump_revision();
    }

    /// 指定した `symbol_id` のシンボル定義を参照する
    pub(crate) fn find_symbol(&self, symbol_id: u64) -> Option<&SymbolDef> {
        self.library
            .iter()
            .find(|s| s.id == symbol_id)
            .map(|s| &**s)
    }

    /// 指定した `symbol_id` のシンボル定義を可変参照する（Undo の控えと共有していれば、そのシンボルと、それが入っている
    /// ライブラリの塊だけを写す）
    pub(crate) fn find_symbol_mut(&mut self, symbol_id: u64) -> Option<&mut SymbolDef> {
        let i = self.library.iter().position(|s| s.id == symbol_id)?;
        Some(Arc::make_mut(&mut self.library[i]))
    }

    #[cfg(test)]
    pub(crate) fn has_any_cached_fill_mesh(&self) -> bool {
        let tl_has_mesh = |tl: &Timeline| {
            tl.layers.iter().any(|l| {
                l.keyframes
                    .iter()
                    .any(|kf| kf.cel.shapes().iter().any(|s| s.has_cached_fill_mesh()))
            })
        };
        tl_has_mesh(&self.root) || self.library.iter().any(|sym| tl_has_mesh(&sym.timeline))
    }

    /// 高さが `MAX_SYMBOL_NEST_DEPTH`（8）を超えるシンボルが1つでもあれば `true` を返す
    pub(crate) fn symbol_nest_exceeded(&self) -> bool {
        fn height(
            sym_id: u64,
            depth: usize,
            by_id: &HashMap<u64, &SymbolDef>,
            memo: &mut HashMap<u64, usize>,
        ) -> usize {
            if let Some(&h) = memo.get(&sym_id) {
                return h;
            }
            let Some(sym) = by_id.get(&sym_id) else {
                return 0;
            };
            if depth > MAX_SYMBOL_NEST_DEPTH {
                return MAX_SYMBOL_NEST_DEPTH + 1;
            }
            let mut max_child = 0usize;
            for inst in sym.timeline.instances() {
                let ch = height(inst.binding.symbol_id, depth + 1, by_id, memo);
                if ch >= MAX_SYMBOL_NEST_DEPTH {
                    return MAX_SYMBOL_NEST_DEPTH + 1;
                }
                max_child = max_child.max(ch);
            }
            let h = 1 + max_child;
            memo.insert(sym_id, h);
            h
        }
        let by_id: HashMap<u64, &SymbolDef> = self.library.iter().map(|s| (s.id, &**s)).collect();
        let mut memo = HashMap::with_capacity(self.library.len());
        self.library
            .iter()
            .any(|s| height(s.id, 1, &by_id, &mut memo) > MAX_SYMBOL_NEST_DEPTH)
    }

    /// `parent_symbol_id` の内部に `candidate_child_id` を配置した場合に、
    /// 直接または間接的な循環参照（`A -> B -> A` 等）が発生するかを DFS で判定する
    pub(crate) fn would_create_symbol_cycle(
        &self,
        parent_symbol_id: u64,
        candidate_child_id: u64,
    ) -> bool {
        reaches(
            candidate_child_id,
            |id| id == parent_symbol_id,
            |id| self.child_symbol_ids(id),
        )
    }

    /// シンボル `symbol_id` の中のインスタンスが参照するシンボルの番号（シンボルが無ければ空）
    pub(crate) fn child_symbol_ids(&self, symbol_id: u64) -> Vec<u64> {
        self.find_symbol(symbol_id).map_or_else(Vec::new, |sym| {
            sym.timeline
                .instances()
                .map(|inst| inst.binding.symbol_id)
                .collect()
        })
    }

    /// 現在の編集コンテキスト（ルートシーンまたはシンボルインプレース編集経路 `edit_path`）に
    /// `candidate_child_id` のシンボルインスタンスを安全に配置可能か（循環参照を起こさないか）を返す
    pub(crate) fn can_place_symbol_in_current_context(
        &self,
        edit_path: &EditPath,
        candidate_child_id: u64,
    ) -> bool {
        self.find_symbol(candidate_child_id).is_some()
            && edit_path
                .levels
                .iter()
                .all(|lvl| !self.would_create_symbol_cycle(lvl.symbol_id, candidate_child_id))
    }

    /// 指定した `symbol_id` のシンボル名を変更する（変更があった場合のみ `true` を返す）
    pub(crate) fn rename_symbol(&mut self, symbol_id: u64, new_name: String) -> bool {
        let Some(sym) = self.find_symbol_mut(symbol_id) else {
            return false;
        };
        if sym.name == new_name {
            return false;
        }
        sym.name = new_name;
        sym.revision = crate::vector_engine::next_mesh_revision();
        self.bump_revision();
        true
    }

    /// 指定した `symbol_id` をライブラリから削除し、ルートおよび全シンボルのタイムラインからそのインスタンスを取り除く
    pub(crate) fn remove_symbol_and_instances(&mut self, symbol_id: u64) -> bool {
        if !self.library.iter().any(|s| s.id == symbol_id) {
            return false;
        }
        let remove_from_tl = |tl: &mut Timeline| {
            for layer in &mut tl.layers {
                for i in 0..layer.keyframes.len() {
                    if layer.keyframes[i]
                        .cel
                        .instances
                        .iter()
                        .any(|inst| inst.binding.symbol_id == symbol_id)
                    {
                        let kf = layer.keyframe_mut(i);
                        kf.cel
                            .instances_mut()
                            .retain(|inst| inst.binding.symbol_id != symbol_id);
                    }
                }
            }
        };
        remove_from_tl(&mut self.root);
        self.library.retain(|s| s.id != symbol_id);
        // インスタンスを含むシンボルだけを書く（ほかのシンボルと塊は、Undo の控えと共有したまま）
        for i in 0..self.library.len() {
            if self.library[i]
                .timeline
                .instances()
                .any(|inst| inst.binding.symbol_id == symbol_id)
            {
                let sym_def = Arc::make_mut(&mut self.library[i]);
                remove_from_tl(&mut sym_def.timeline);
                sym_def.revision = crate::vector_engine::next_mesh_revision();
            }
        }
        self.bump_revision();
        true
    }

    /// Undo / Redo 後に `edit_path` を検証し、途中のシンボルまたはインスタンスが消えていたら到達できる段まで戻る
    pub(crate) fn validate_edit_path(
        &mut self,
        edit_path: &mut EditPath,
        editing_group_path: &mut Vec<u64>,
    ) {
        let mut reachable_len = 0usize;
        for i in 0..edit_path.levels.len() {
            let lvl = &edit_path.levels[i];
            if self.find_symbol(lvl.symbol_id).is_none() {
                break;
            }
            if lvl.instance_gid != 0 {
                let parent_tl = if i == 0 {
                    &self.root
                } else {
                    let prev_sym_id = edit_path.levels[i - 1].symbol_id;
                    let Some(sym) = self.find_symbol(prev_sym_id) else {
                        break;
                    };
                    &sym.timeline
                };
                let has_inst = parent_tl.instances().any(|inst| {
                    inst.binding.instance_gid == lvl.instance_gid
                        && inst.binding.symbol_id == lvl.symbol_id
                });
                if !has_inst {
                    break;
                }
            }
            reachable_len = i + 1;
        }
        if reachable_len < edit_path.levels.len() {
            let popped = edit_path.levels[reachable_len].clone();
            edit_path.levels.truncate(reachable_len);
            *editing_group_path = popped.parent_group_path;
            if let Some(last) = edit_path.last() {
                if let Some(s) = self.find_symbol_mut(last.symbol_id) {
                    s.timeline.active_layer = popped
                        .parent_active_layer
                        .min(s.timeline.layers.len().saturating_sub(1));
                    s.timeline.current_frame = popped
                        .parent_frame
                        .min(s.timeline.total_frames.saturating_sub(1));
                }
            } else {
                self.root.active_layer = popped
                    .parent_active_layer
                    .min(self.root.layers.len().saturating_sub(1));
                self.root.current_frame = popped
                    .parent_frame
                    .min(self.root.total_frames.saturating_sub(1));
            }
        }
    }
}

// ============================================================================
// 2. セル署名計算（衝突ゼロ署名 `ResolvedCelSig`：土台キャッシュ・背景キャッシュ用）
// ============================================================================

impl ResolvedCelSig {
    #[inline]
    fn are_referenced_symbols_current(&self, library: &Library) -> bool {
        self.referenced_symbols.iter().all(|&(sym_id, rev)| {
            if rev == u64::MAX {
                !library.iter().any(|s| s.id == sym_id)
            } else {
                library.iter().any(|s| s.id == sym_id && s.revision == rev)
            }
        })
    }

    /// ヒープアロケーションや再帰走査を一切行わずに、既存の署名 `self` が指定キーフレーム・ライブラリに対して有効かを O(参照シンボル数) で判定する
    pub(crate) fn matches_layer_keyframe(
        &self,
        layer: &Layer,
        kf_idx: usize,
        library: &Library,
    ) -> bool {
        let kf = &layer.keyframes[kf_idx];
        let next_kf = if kf.tween {
            layer.keyframes.get(kf_idx + 1).and_then(|nkf| {
                nkf.cel
                    .single_symbol_instance()
                    .map(|nb| (nkf.start_frame, nb))
            })
        } else {
            None
        };
        self.cel_revision == kf.cel.revision
            && self.start_frame == kf.start_frame
            && self.tween == kf.tween
            && self.tween_rotate == kf.tween_rotate
            && self.next_kf == next_kf
            && self.are_referenced_symbols_current(library)
    }
}

/// 指定された `sym_id` およびその内部で入れ子参照されている全子シンボルの `(id, revision)` を再帰的に収集する
fn collect_referenced_symbol_revisions(
    sym_id: u64,
    library: &Library,
    visited: &mut Vec<u64>,
    depth: usize,
    out: &mut Vec<(u64, u64)>,
) {
    if depth >= MAX_SYMBOL_NEST_DEPTH || visited.contains(&sym_id) {
        return;
    }
    visited.push(sym_id);
    if let Some(sym) = library.iter().find(|s| s.id == sym_id) {
        out.push((sym.id, sym.revision));
        for inst in sym.timeline.instances() {
            collect_referenced_symbol_revisions(
                inst.binding.symbol_id,
                library,
                visited,
                depth + 1,
                out,
            );
        }
    } else {
        out.push((sym_id, u64::MAX));
    }
}

pub(crate) fn compute_resolved_cel_signature(
    layer: &Layer,
    kf_idx: usize,
    excluded_instance_gid: Option<u64>,
    library: &Library,
) -> ResolvedCelSig {
    let kf = &layer.keyframes[kf_idx];
    let mut visited_syms: Vec<u64> = Vec::new();
    let mut referenced_symbols: Vec<(u64, u64)> = Vec::new();
    for inst in kf.cel.instances.iter() {
        if excluded_instance_gid == Some(inst.binding.instance_gid) {
            continue;
        }
        collect_referenced_symbol_revisions(
            inst.binding.symbol_id,
            library,
            &mut visited_syms,
            0,
            &mut referenced_symbols,
        );
    }
    let mut next_kf = None;
    if kf.tween {
        if let Some(nkf) = layer.keyframes.get(kf_idx + 1) {
            if let Some(nb) = nkf.cel.single_symbol_instance() {
                next_kf = Some((nkf.start_frame, nb));
                if kf
                    .cel
                    .single_symbol_instance()
                    .is_none_or(|sb| excluded_instance_gid != Some(sb.instance_gid))
                {
                    collect_referenced_symbol_revisions(
                        nb.symbol_id,
                        library,
                        &mut visited_syms,
                        0,
                        &mut referenced_symbols,
                    );
                }
            }
        }
    }
    ResolvedCelSig {
        cel_revision: kf.cel.revision,
        start_frame: kf.start_frame,
        tween: kf.tween,
        tween_rotate: kf.tween_rotate,
        next_kf,
        referenced_symbols,
    }
}

// ============================================================================
// 3. Layer / Timeline のシンボル展開・解決済みメッシュ評価メソッド
// ============================================================================

impl Layer {
    /// このレイヤーの指定フレーム `frame` における図形列を、クラシックトゥイーン補間およびシンボル内タイムライン評価込みで返す
    pub(crate) fn evaluate_shapes_at(
        &self,
        (frame, child_eval_frame): (usize, usize),
        out_layer_id: u64,
        ctx: SymbolEvalCtx<'_>,
    ) -> Vec<FillShape> {
        let Some(kf_idx) = self.keyframe_index_at(frame) else {
            return Vec::new();
        };
        let kf = &self.keyframes[kf_idx];
        if let Some(interp_b) = self.interpolated_tween_binding_at(frame) {
            return interp_b.expand((kf.start_frame, child_eval_frame), out_layer_id, &[], ctx);
        }

        resolve_cel_with_library(&kf.cel, kf.start_frame, child_eval_frame, ctx)
            .into_iter()
            .map(|s| s.with_layer_id(out_layer_id))
            .collect()
    }
}

impl Timeline {
    /// 指定レイヤー `layer_idx` の指定フレーム `frame` における図形列を、クラシックトゥイーン補間およびシンボル内タイムライン評価込みで返す
    pub(crate) fn evaluate_layer_shapes_at(
        &self,
        layer_idx: usize,
        frame: usize,
        library: &Library,
    ) -> Vec<FillShape> {
        self.evaluate_layer_shapes_with(layer_idx, frame, SymbolEvalCtx::new(library))
    }

    /// `evaluate_layer_shapes_at` と同じ。シンボルの評価に `ctx` のメモを使う
    pub(crate) fn evaluate_layer_shapes_with(
        &self,
        layer_idx: usize,
        frame: usize,
        ctx: SymbolEvalCtx<'_>,
    ) -> Vec<FillShape> {
        let Some(layer) = self.layers.get(layer_idx) else {
            return Vec::new();
        };
        layer.evaluate_shapes_at((frame, frame), layer.id, ctx)
    }

    pub(crate) fn build_cel_world_batch_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        world_xform: Affine,
        ctx: SymbolEvalCtx<'_>,
    ) -> Option<Mesh> {
        let layer = self.layers.get(layer_idx)?;
        let kf_idx = layer.keyframe_index_at(frame)?;
        let kf = &layer.keyframes[kf_idx];
        if kf.cel.is_empty() {
            return None;
        }
        let has_tween = layer.interpolated_tween_binding_at(frame).is_some();
        if !has_tween && kf.cel.instances.is_empty() && world_xform == Affine::IDENTITY {
            let mut batch = Mesh::default();
            for s in kf.cel.shapes() {
                s.append_fill_mesh_to(&mut batch);
            }
            return (!batch.is_empty()).then_some(batch);
        }
        let mut resolved = self.evaluate_layer_shapes_with(layer_idx, frame, ctx);
        if world_xform != Affine::IDENTITY {
            for s in &mut resolved {
                s.transform_with_cached_mesh(world_xform);
            }
        }
        let mut batch = Mesh::default();
        for s in &resolved {
            s.append_fill_mesh_to(&mut batch);
        }
        (!batch.is_empty()).then_some(batch)
    }

    /// 指定レイヤー・フレームの描画用スクリーン `Arc<Mesh>` をその場で構築して返す
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolved_cel_screen_mesh(
        &self,
        layer_idx: usize,
        frame: usize,
        zoom: f32,
        pan: Vec2,
        dim_backdrop: bool,
        world_xform: Affine,
        ctx: SymbolEvalCtx<'_>,
    ) -> Option<Arc<Mesh>> {
        let world_batch = self.build_cel_world_batch_mesh(layer_idx, frame, world_xform, ctx)?;
        Some(Arc::new(transform_mesh_to_screen(
            world_batch,
            zoom,
            pan,
            if dim_backdrop {
                Some(&dim_backdrop_color)
            } else {
                None
            },
        )))
    }
}
