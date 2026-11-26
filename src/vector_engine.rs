use eframe::egui::{self, epaint::Vertex, Color32, Mesh, Pos2, Vec2};
use kurbo::{
    Affine, BezPath, ParamCurve, Point, QuadBez, Rect as KRect, Shape as KurboShape, Vec2 as KVec2,
};
use lyon_tessellation::{
    math::point as lyon_point, path::Path as LyonPath, BuffersBuilder, FillOptions, FillRule,
    FillTessellator, FillVertex, VertexBuffers,
};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    cell::RefCell,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

pub(crate) const MIN_VIEW_ZOOM: f32 = 0.15;
pub(crate) const MAX_VIEW_ZOOM: f32 = 16.0;

/// シンボルインスタンスのフレーム再生モード（Adobe Animate のグラフィックシンボル相当）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub(crate) enum SymbolLoopMode {
    /// ループ再生：キーフレーム開始位置からシンボル内部フレームを繰り返し再生する
    #[default]
    Loop,
    /// 1回再生：キーフレーム開始位置からシンボル内部の最終フレームまで再生して停止する
    PlayOnce,
    /// 停止（コマ固定）：指定された `first_frame` の1コマだけを固定表示する
    SingleFrame,
}

/// シンボルインスタンスの色効果（アルファ 0〜1、明度 -1〜1）
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ColorEffect {
    pub(crate) alpha: f32,
    pub(crate) brightness: f32,
}

impl Default for ColorEffect {
    #[inline]
    fn default() -> Self {
        Self {
            alpha: 1.0,
            brightness: 0.0,
        }
    }
}

impl ColorEffect {
    /// アルファ `alpha` と明度 `brightness` を範囲（アルファ 0〜1、明度 -1〜1）に収めた色効果。有限かは見ない（有限かを決める
    /// のは、インスタンスを書く入口・ファイルを読む入口の判定 `SymbolInstanceBinding::is_finite` だけ）
    #[inline]
    pub(crate) fn new(alpha: f32, brightness: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.0, 1.0),
            brightness: brightness.clamp(-1.0, 1.0),
        }
    }

    #[inline]
    pub(crate) fn lerp(self, other: Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0) as f32;
        Self::new(
            self.alpha + (other.alpha - self.alpha) * t,
            self.brightness + (other.brightness - self.brightness) * t,
        )
    }

    pub(crate) fn apply(self, c: Color32) -> Color32 {
        let [r, g, b, a] = c.to_srgba_unmultiplied();
        let br = self.brightness;
        let apply_ch = |v: u8| -> u8 {
            let vf = v as f32;
            let out = if br >= 0.0 {
                vf + (255.0 - vf) * br
            } else {
                vf * (1.0 + br)
            };
            out.round().clamp(0.0, 255.0) as u8
        };
        let out_a = (a as f32 * self.alpha).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgba_unmultiplied(apply_ch(r), apply_ch(g), apply_ch(b), out_a)
    }
}

/// シンボルインスタンスに属するシェイプが持つ幾何変換・再生紐付け情報
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SymbolInstanceBinding {
    pub(crate) instance_gid: u64,
    pub(crate) symbol_id: u64,
    pub(crate) xform: Affine,
    /// シンボルローカル座標系での変形中心点（ピボット）。初期値は `Point::ZERO` `(0.0, 0.0)`
    pub(crate) pivot: Point,
    pub(crate) loop_mode: SymbolLoopMode,
    pub(crate) first_frame: usize,
    pub(crate) color_effect: ColorEffect,
}

impl SymbolInstanceBinding {
    #[inline]
    pub(crate) fn new(instance_gid: u64, symbol_id: u64, xform: Affine) -> Self {
        Self {
            instance_gid,
            symbol_id,
            xform,
            pivot: Point::ZERO,
            loop_mode: SymbolLoopMode::Loop,
            first_frame: 0,
            color_effect: ColorEffect::default(),
        }
    }

    /// 数がみな有限か（変換・中心点・色効果）。インスタンスの受け入れの判定はこれ1つ：インスタンスを書く入口
    /// （`Cel::update_instances`・`Cel::insert_content`）、置く準備（`Cel::transformed`）、ファイルを読む入口
    /// （`ProjectDocument::only_finite_numbers`）。文書のインスタンスはいつも有限なので、保存で直す所は無い
    pub(crate) fn is_finite(&self) -> bool {
        self.xform.is_finite()
            && self.pivot.is_finite()
            && self.color_effect.alpha.is_finite()
            && self.color_effect.brightness.is_finite()
    }

    /// 再生位置を `d` コマ進める：キーフレームの始めを `d` コマ後ろへずらしても、同じコマに同じ絵を出す（1コマだけの
    /// 表示は変えない）。再生位置をずらす式は、これと `rewind` だけ
    pub(crate) fn advance(&mut self, d: usize) {
        if self.loop_mode != SymbolLoopMode::SingleFrame {
            self.first_frame = self.first_frame.saturating_add(d);
        }
    }

    /// 再生位置を `d` コマ戻す（`advance` の逆：キーフレームの始めを `d` コマ前へずらす）。ループは、0 より前になるときだけ
    /// シンボルのコマ数 `frames` で回す。1回だけの再生は 0 で止める（その前の絵は出せない）
    pub(crate) fn rewind(&mut self, d: usize, frames: usize) {
        self.first_frame = match self.loop_mode {
            SymbolLoopMode::Loop if self.first_frame < d => {
                (self.first_frame as i128 - d as i128).rem_euclid(frames.max(1) as i128) as usize
            }
            SymbolLoopMode::Loop | SymbolLoopMode::PlayOnce => self.first_frame.saturating_sub(d),
            SymbolLoopMode::SingleFrame => self.first_frame,
        };
    }
}

/// 座標の変換で、逆にできるとみなす行列式の大きさの下限
const MIN_DET: f64 = 1e-12;

/// 計算の丸めの差だけなら同じとみなす差の大きさ（文書の px・変換の係数・向きの角度（ラジアン））。位置・中心点・変換・
/// トゥイーンの向きが同じかの判定はこれを使う
pub(crate) const SAME_TOL: f64 = 1e-6;

/// 割る前に、長さ 0 とみなす長さの上限（px）。向きを取る・長さで割る所はこれを使う
pub(crate) const TINY_LEN: f64 = 1e-9;

/// 座標の変換 `a` が使えるか：どの数も有限で、逆にできる（行列式が有限で、その大きさが `MIN_DET` より大きい。係数が
/// 有限でも行列式があふれれば使えない）。読み込み・図形の変換・中心点の写し・インスタンスの置き直し・地図を変換して
/// 置けるか（`PlanarMap::fits_grid`）のどこでも、変換が使えるかの判定はこれ1つ
#[inline]
pub(crate) fn invertible(a: Affine) -> bool {
    let det = a.determinant();
    a.as_coeffs().iter().all(|v| v.is_finite()) && det.is_finite() && det.abs() > MIN_DET
}

/// インスタンスの中心点 `a` と `b`（シンボルのローカル座標）が同じ所か（中心点を比べる判定はこれ1つ）
#[inline]
pub(crate) fn same_pivot(a: Point, b: Point) -> bool {
    (a - b).hypot() <= SAME_TOL
}

/// インスタンスの中心点 `pivot` を、初めの位置（原点）から動かしてあるか（`same_pivot`）
#[inline]
pub(crate) fn is_custom_pivot(pivot: Point) -> bool {
    !same_pivot(pivot, Point::ZERO)
}

static NEXT_MESH_REVISION: AtomicU64 = AtomicU64::new(1);
static NEXT_SHAPE_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_mesh_revision() -> u64 {
    NEXT_MESH_REVISION.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn next_shape_id() -> u64 {
    new_shape_ids(1)
}

/// 続けて使える新しい図形の番号を `n` 個取り、その最初の番号を返す（図形の番号を振るのはこれ1つ。ファイルから読んだ
/// 地図の印も、これで取った番号に替える：`TileReader::new`）
pub(crate) fn new_shape_ids(n: u64) -> u64 {
    NEXT_SHAPE_ID.fetch_add(n, Ordering::Relaxed)
}

/// `Arc` と共存可能なスレッドセーフ（`Send + Sync`）かつ `Clone` 対応の内部可変キャッシュラッパー
#[derive(Debug, Default)]
pub(crate) struct SyncCache<T>(Mutex<T>);

impl<T> SyncCache<T> {
    #[inline]
    pub(crate) const fn new(val: T) -> Self {
        Self(Mutex::new(val))
    }

    #[inline]
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<T: Clone> Clone for SyncCache<T> {
    fn clone(&self) -> Self {
        Self::new(self.lock().clone())
    }
}

thread_local! {
    static TESSELLATOR: RefCell<FillTessellator> = RefCell::new(FillTessellator::new());
}

// ============================================================================
// コアデータ構造 (2次ベジェ輪郭 & マージ描画シェイプ)
// ============================================================================

/// 閉じた2次ベジェ曲線（QuadBez）のループ輪郭（Copy-on-Write `Arc` 共有）
/// 隣接セグメント間で `segments[i].p2 == segments[(i + 1) % len].p0` を常に満たす。
#[derive(Clone, Debug)]
pub(crate) struct Contour {
    pub(crate) segments: Arc<Vec<QuadBez>>,
}

impl Contour {
    pub(crate) fn new(mut segments: Vec<QuadBez>) -> Self {
        segments.shrink_to_fit();
        Self {
            segments: Arc::new(segments),
        }
    }

    pub(crate) fn segments_mut(&mut self) -> &mut Vec<QuadBez> {
        Arc::make_mut(&mut self.segments)
    }

    pub(crate) fn translate(&mut self, delta: KVec2) {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return;
        }
        for seg in self.segments_mut() {
            seg.p0 += delta;
            seg.p1 += delta;
            seg.p2 += delta;
        }
    }

    /// 座標の変換 `affine` で写す。潰れた変換（逆にできない）でも、数が有限なら写す（面積 0 の輪郭になり、見えない。
    /// 潰れた変換で置いたインスタンスを描かないのは `SymbolInstanceBinding::instantiate_shapes` の1か所）。数が有限でない
    /// 変換は写せないのでかけない（図形の変換はどれも同じ）
    pub(crate) fn transform(&mut self, affine: Affine) {
        if !affine.is_finite() {
            return;
        }
        for seg in self.segments_mut() {
            seg.p0 = affine * seg.p0;
            seg.p1 = affine * seg.p1;
            seg.p2 = affine * seg.p2;
        }
        if affine.determinant() < 0.0 {
            self.reverse();
        }
        self.enforce_continuity();
    }

    /// この輪郭のバウンディングボックスを2次ベジェ曲線群から直接計算する
    pub(crate) fn bounding_box(&self) -> KRect {
        let mut iter = self.segments.iter().map(KurboShape::bounding_box);
        let Some(first) = iter.next() else {
            return KRect::ZERO;
        };
        iter.fold(first, |acc, r| acc.union(r))
    }

    pub(crate) fn append_to_bez_path(&self, path: &mut BezPath) {
        if let Some(first) = self.segments.first() {
            path.move_to(first.p0);
            for seg in self.segments.iter() {
                path.quad_to(seg.p1, seg.p2);
            }
            path.close_path();
        }
    }

    /// ポリゴン近似した際の符号付き面積（Shoelace公式）
    pub(crate) fn signed_area(&self) -> f64 {
        const SUBDIVS: usize = 8;
        signed_area_f64(self.segments.iter().flat_map(|seg| {
            (0..SUBDIVS).map(move |k| {
                let p = seg.eval(k as f64 / SUBDIVS as f64);
                [p.x, p.y]
            })
        }))
    }

    pub(crate) fn reverse(&mut self) {
        let segs = self.segments_mut();
        segs.reverse();
        for seg in segs {
            std::mem::swap(&mut seg.p0, &mut seg.p2);
        }
    }

    pub(crate) fn enforce_continuity(&mut self) {
        let n = self.segments.len();
        if n == 0 {
            return;
        }
        let segs = self.segments_mut();
        for i in 0..n {
            let next = (i + 1) % n;
            let mid = segs[i].p2.lerp(segs[next].p0, 0.5);
            segs[i].p2 = mid;
            segs[next].p0 = mid;
        }
    }
}

/// 図形の番号（`FillShape::shape_id`。地図では面の印）
pub(crate) type ShapeId = u64;
/// レイヤーの番号（`FillShape::layer_id`）
pub(crate) type LayerId = u64;

/// キャンバス上の1つの塗りシェイプ（色 + 外周2次ベジェ輪郭 + 穴の2次ベジェ輪郭リスト + 所属レイヤーID + グループ階層 + シンボル紐付け + 描画・判定キャッシュ）
#[derive(Clone, Debug)]
pub(crate) struct FillShape {
    /// 各シェイプインスタンスを順序変更・ソート後も安定して識別する固有ID
    pub(crate) shape_id: ShapeId,
    /// 色・外周・穴（外からは読むだけ：`color`・`exterior`・`holes`。変えるのは `FillShape` の関数だけで、そこで版を上げ、
    /// メッシュ・当たり判定・外枠の写しを捨てる：約束の棚卸し 5616fcc の B の C9）
    color: Color32,
    exterior: Contour,
    holes: Vec<Contour>,
    /// このシェイプが所属するタイムラインレイヤーの固有ID（デフォルト `1`）
    pub(crate) layer_id: LayerId,
    /// 外側のグループIDから内側のグループIDへの階層パス（空 `[]` ならルート階層の生シェイプ）
    pub(crate) group_path: Vec<u64>,
    /// このシェイプがシンボルインスタンスに属する場合の紐付け情報
    pub(crate) symbol_instance: Option<SymbolInstanceBinding>,
    /// メッシュ内容が更新されるたびにインクリメントされるリビジョン番号（土台キャッシュや書き戻しでの差分検知用）
    pub(crate) mesh_revision: u64,
    /// 線の図形なら、その線（`LayerBase::lines`：線を乗せた辺1本を太らせた形。番号は線の番号 `line_id`）。面の図形は `None`
    pub(crate) line: Option<crate::planar_map::EdgeStroke>,
    /// 線の図形なら、線の中心を通る曲線（地図の辺1本）を、線の形を作った座標で持ったもの（今の座標の曲線は
    /// `center_curve`：`line_xform` で写す）。輪郭の表示とアンカーは、太らせた形の輪郭でなく、これで描く。閉じた輪郭
    /// （`Contour`）でなく曲線1本で持つので、写しても端と端をつながない（つなぐと両端が真ん中に寄って潰れる）
    center: Option<QuadBez>,
    /// 線の図形なら、線の始めと終わりの端の形（`planar_map::line_ends`。線の形を作った座標の向き）
    line_ends: [crate::vector_stroke::LineEnd; 2],
    /// 線の形を作った座標から今の座標への変換（線の形を作ったあとにかけた変換を掛け合わせたもの。シンボルの中の線は、
    /// 太さも一緒にこれで写る）。線の形を作り直すとき（`zoom_mesh`）は、作った座標で帯を作ってこれで写す
    line_xform: Affine,
    bbox: KRect,
    fill_mesh: OnceLock<Arc<Mesh>>,
    /// 拡大して描くときの、`fill_mesh` より細かいメッシュ（段と一緒に1つだけ：`view_mesh`）
    zoom_mesh: ZoomMesh,
    /// 線の図形の選択の色を塗る、両端を丸くした帯のメッシュ（段と一緒に1つだけ：`selection_mesh`）
    round_mesh: ZoomMesh,
    /// `fill_mesh` を作ったあとに、その頂点を写した変換の最大の拡大率を掛け合わせたもの（メッシュのずれは、作る許容の
    /// この倍まで：`keeps_mesh`）。メッシュが無いときは 1
    mesh_stretch: f64,
    hit_bez_path: OnceLock<Arc<BezPath>>,
}

impl FillShape {
    /// 色
    pub(crate) fn color(&self) -> Color32 {
        self.color
    }

    /// 外周の輪郭
    pub(crate) fn exterior(&self) -> &Contour {
        &self.exterior
    }

    /// 穴の輪郭
    pub(crate) fn holes(&self) -> &[Contour] {
        &self.holes
    }

    pub(crate) fn new(color: Color32, exterior: Contour, holes: Vec<Contour>) -> Self {
        let mut shape = Self {
            shape_id: next_shape_id(),
            color,
            exterior,
            holes,
            layer_id: 1,
            group_path: Vec::new(),
            symbol_instance: None,
            mesh_revision: 0,
            line: None,
            center: None,
            line_ends: [crate::vector_stroke::LineEnd::Cap; 2],
            line_xform: Affine::IDENTITY,
            bbox: KRect::ZERO,
            fill_mesh: OnceLock::new(),
            zoom_mesh: ZoomMesh::default(),
            round_mesh: ZoomMesh::default(),
            mesh_stretch: 1.0,
            hit_bez_path: OnceLock::new(),
        };
        shape.rebuild_cache();
        shape
    }

    /// この要素が空白フレームのシンボルインスタンスを表す非表示マーカーであるか
    #[inline]
    pub(crate) fn is_empty_symbol_marker(&self) -> bool {
        self.exterior.segments.is_empty() && self.symbol_instance.is_some()
    }

    /// 描画用の塗り `egui::Mesh` を遅延テッセレーションして返す（キャッシュ済みの場合は O(1) で参照を返す）
    pub(crate) fn fill_mesh(&self) -> &Arc<Mesh> {
        self.fill_mesh.get_or_init(|| {
            Arc::new(tessellate_contours_to_mesh(
                &self.exterior,
                &self.holes,
                self.color,
                self.fill_rule(),
                MESH_TOL,
            ))
        })
    }

    /// キャッシュ済みの `fill_mesh` があればそれを、無ければその場でテッセレーションしたメッシュを `dst` に追記する（自身の `fill_mesh` には保存しない）
    pub(crate) fn append_fill_mesh_to(&self, dst: &mut Mesh) {
        if let Some(m) = self.fill_mesh.get() {
            append_mesh(dst, m);
        } else {
            let m = tessellate_contours_to_mesh(&self.exterior, &self.holes, self.color, self.fill_rule(), MESH_TOL);
            append_mesh(dst, &m);
        }
    }

    /// 倍率 `zoom` で画面に描くメッシュ（文書の座標）：段（`mesh_level`）が 0 なら `fill_mesh`、それより上なら、その段の
    /// 許容で作ったもの（1つだけ持ち、段が変われば作り直す）。画面に描く図形のメッシュはみなこれ（`screen_mesh`・
    /// `selection_screen_mesh`・セル画の写しの拡大した表示 `CelMeshCache::screen_mesh`）
    pub(crate) fn view_mesh(&self, zoom: f32) -> Arc<Mesh> {
        let level = mesh_level(zoom);
        if level == 0 {
            return Arc::clone(self.fill_mesh());
        }
        self.zoom_mesh.get_or_make(level, || match self.line {
            Some(_) => self.band_mesh(level_tol(level), self.line_ends),
            None => tessellate_contours_to_mesh(
                &self.exterior,
                &self.holes,
                self.color,
                self.fill_rule(),
                level_tol(level),
            ),
        })
    }

    /// 選択の色を塗るメッシュ（文書の座標。倍率 `zoom` の段の細かさ）：線の図形は、両端を丸くした帯（同じ線が続くつなぎ目
    /// でも、選んだ区間の端が丸く見える。利用者が決めた。選んだ線どうしで丸い端が重なっても、選択の色は不透明なので濃く
    /// ならない：`selection_color`）、面の図形は描くメッシュ（`view_mesh`）。両端がもともと丸い線は、描くメッシュを使う
    fn selection_mesh(&self, zoom: f32) -> Arc<Mesh> {
        let round = [crate::vector_stroke::LineEnd::Cap; 2];
        if self.line.is_none() || self.line_ends == round {
            return self.view_mesh(zoom);
        }
        let level = mesh_level(zoom);
        self.round_mesh.get_or_make(level, || self.band_mesh(level_tol(level), round))
    }

    /// 線の図形の帯のメッシュ（文書の座標）：線の形を作った座標で、中心の曲線と太さと端の形 `ends` から、許容 `tol` を
    /// 変換の拡大率で割った許容で帯を作り（`quad_band`）、今の座標へ写す。線の図形の輪郭は帯を `MESH_TOL` で折れ線に
    /// したものなので、細かくするときは輪郭からでなく、ここで作り直す
    fn band_mesh(&self, tol: f64, ends: [crate::vector_stroke::LineEnd; 2]) -> Mesh {
        let (Some(stroke), Some(c)) = (self.line, self.center) else {
            return Mesh::default();
        };
        let s = max_scale(self.line_xform);
        let local_tol = if s > 0.0 { tol / s } else { tol };
        let (_, mut mesh) =
            crate::vector_stroke::quad_band(c, stroke.width as f64 / 2.0, local_tol, self.color, ends);
        transform_mesh_vertices_in_place(&mut mesh.vertices, self.line_xform.as_coeffs());
        mesh
    }

    /// 形・色・置き場所を変えたときに、倍率に合わせて作ったメッシュ（`zoom_mesh`・`round_mesh`）を捨てる
    fn drop_view_meshes(&mut self) {
        self.zoom_mesh = ZoomMesh::default();
        self.round_mesh = ZoomMesh::default();
    }

    /// 指定された `(zoom, pan, dim_backdrop)` に対応するスクリーン座標変換済み `Mesh` を返す（倍率に合わせた細かさ：`view_mesh`）
    pub(crate) fn screen_mesh(&self, zoom: f32, pan: Vec2, dim_backdrop: bool) -> Mesh {
        transform_mesh_to_screen(
            (*self.view_mesh(zoom)).clone(),
            zoom,
            pan,
            if dim_backdrop {
                Some(&dim_backdrop_color)
            } else {
                None
            },
        )
    }

    /// Undo 履歴保存時やバックアップ時に、幾何データから随時再生成可能な描画メッシュキャッシュを破棄してメモリを解放する
    pub(crate) fn strip_transient_caches(&mut self) {
        self.hit_bez_path = OnceLock::new();
        self.fill_mesh = OnceLock::new();
        self.drop_view_meshes();
        self.mesh_stretch = 1.0;
    }

    /// 座標の変換 `xform` で置いた写しにする（シンボルの中の表示の図形）。番号と `mesh_revision` は文書の図形のまま
    /// （書き戻しで、変わっていない図形を見分けるため。変えた図形は版が変わる）
    pub(crate) fn place(&mut self, xform: Affine) {
        if xform == Affine::IDENTITY {
            return;
        }
        let orig_rev = self.mesh_revision;
        self.transform(xform);
        self.mesh_revision = orig_rev;
    }

    /// この図形（座標の変換 `xform` で置いた図形：ブラシの形・矩形・貼り付けた図形など）の、文書の座標の形（`xform` の
    /// 逆で写す。2次ベジェはアフィン変換で正確に写る）
    pub(crate) fn to_doc(&self, xform: Affine) -> Cow<'_, FillShape> {
        if xform == Affine::IDENTITY {
            return Cow::Borrowed(self);
        }
        let mut d = self.clone();
        d.transform(xform.inverse());
        Cow::Owned(d)
    }

    /// 同じ `shape_id`・`mesh_revision` の図形 `donor` から、色が同じなら描画・当たり判定キャッシュを O(1) で引き継ぐ
    pub(crate) fn reuse_caches_from(&mut self, donor: &FillShape) {
        if self.color != donor.color {
            return;
        }
        if self.hit_bez_path.get().is_none() {
            if let Some(p) = donor.hit_bez_path.get() {
                let _ = self.hit_bez_path.set(Arc::clone(p));
            }
        }
        if self.fill_mesh.get().is_none() {
            if let Some(m) = donor.fill_mesh.get() {
                let _ = self.fill_mesh.set(Arc::clone(m));
                self.mesh_stretch = donor.mesh_stretch;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn has_cached_fill_mesh(&self) -> bool {
        self.fill_mesh.get().is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_cached_hit_bez_path(&self) -> bool {
        self.hit_bez_path.get().is_some()
    }

    pub(crate) fn with_layer_id(mut self, layer_id: u64) -> Self {
        self.layer_id = layer_id;
        self
    }

    pub(crate) fn with_scope(mut self, layer_id: u64, group_path: &[u64]) -> Self {
        self.layer_id = layer_id;
        self.group_path = group_path.to_vec();
        self
    }

    /// 実体のある外周および全内周穴の `Contour` を順に走査するイテレータを返す
    /// （空白フレームのシンボルマーカーの場合は 0 件の空イテレータを返すため、呼び出し側での個別ガードが不要）
    pub(crate) fn contours(&self) -> impl Iterator<Item = &Contour> {
        (!self.exterior.segments.is_empty())
            .then_some(&self.exterior)
            .into_iter()
            .chain(self.holes.iter())
    }

    /// 指定されたグループ編集スコープ `scope` の内部（直下の生シェイプ、または子孫グループ）に属するか
    pub(crate) fn is_in_editing_scope(&self, scope: &[u64]) -> bool {
        self.group_path.starts_with(scope)
    }

    /// 指定されたグループ編集スコープ `scope` 直下の「生のシェイプ（未グループ）」であるか
    pub(crate) fn is_raw_at_scope(&self, scope: &[u64]) -> bool {
        self.group_path.as_slice() == scope
    }

    /// 指定されたグループ編集スコープ `scope` において、このシェイプが属する直下の子グループIDを返す
    pub(crate) fn active_group_id_at_scope(&self, scope: &[u64]) -> Option<u64> {
        if self.is_in_editing_scope(scope) {
            self.group_path.get(scope.len()).copied()
        } else {
            None
        }
    }

    fn compute_bbox(&self) -> KRect {
        if self.exterior.segments.is_empty() {
            if let Some(b) = &self.symbol_instance {
                let p = b.xform * Point::ZERO;
                return KRect::from_points(p, p);
            }
        }
        let mut bbox = self.exterior.bounding_box();
        for hole in &self.holes {
            bbox = bbox.union(hole.bounding_box());
        }
        bbox
    }

    pub(crate) fn rebuild_cache(&mut self) {
        self.bbox = self.compute_bbox();
        self.strip_transient_caches();
        self.mesh_revision = next_mesh_revision();
    }

    /// 線の図形にする：線 `stroke` と、線の中心を通る曲線 `center` と、その端の形 `ends`（`LayerBase::lines`）
    pub(crate) fn with_line(
        mut self,
        stroke: crate::planar_map::EdgeStroke,
        center: QuadBez,
        ends: [crate::vector_stroke::LineEnd; 2],
    ) -> Self {
        self.line = Some(stroke);
        self.center = Some(center);
        self.line_ends = ends;
        self.line_xform = Affine::IDENTITY;
        self
    }

    /// 線の図形なら、線の中心を通る曲線（今の座標。地図の辺1本。輪郭の表示とアンカー・線の辺を探す所）
    pub(crate) fn center_curve(&self) -> Option<QuadBez> {
        self.center.map(|c| self.line_xform * c)
    }

    /// 線の図形を、線の太さはそのままに、中心の曲線に変換 `affine`（ステージの座標）をかけた所へ作り直す。生の絵とグループの
    /// 線は、拡大しても太さが変わらない（確定すると地図の辺に変換をかけ、同じ太さの線で作り直す：その形と同じ。シンボルの線は
    /// 図形ごと写す `transform` で、一緒に太くなる）。図形はステージの座標で、線の太さが決まる編集中の階層の座標からステージ
    /// への変換が `world`。線の図形でなければ（作り直せなければ）`transform` と同じ
    pub(crate) fn transform_keeping_line_width(&mut self, affine: Affine, world: Affine) {
        let (Some(stroke), Some(q)) = (self.line, self.center_curve()) else {
            self.transform(affine);
            return;
        };
        if !affine.is_finite() || !invertible(world) {
            self.transform(affine);
            return;
        }
        let inv = world.inverse();
        let doc = inv * affine * world;
        let at = |p: Point| doc * (inv * p);
        let qd = QuadBez::new(at(q.p0), at(q.p1), at(q.p2));
        let ends = self.line_ends.map(|e| e.transformed(doc * self.line_xform));
        let Some((exterior, holes, mesh)) = crate::planar_map::line_band(qd, stroke, ends) else {
            self.transform(affine);
            return;
        };
        let mut s = FillShape {
            exterior,
            holes,
            center: Some(qd),
            line_ends: ends,
            line_xform: Affine::IDENTITY,
            fill_mesh: OnceLock::new(),
            mesh_stretch: 1.0,
            hit_bez_path: OnceLock::new(),
            ..self.clone()
        };
        s.bbox = s.compute_bbox();
        let mut s = s.with_mesh(mesh);
        s.mesh_revision = next_mesh_revision();
        s.place(world);
        *self = s;
    }

    /// 輪郭と同じ形のメッシュ `mesh` を、先に持たせる（線の図形：帯で直接作ったメッシュ `quad_band_mesh`。変換で許すずれを
    /// 超えて捨てたあとは、輪郭から作り直す）
    pub(crate) fn with_mesh(self, mesh: Mesh) -> Self {
        let _ = self.fill_mesh.set(Arc::new(mesh));
        self
    }

    /// 塗りの決まりが NonZero か（線の図形：輪郭がブラシと同じ作り方で、自分と重なることがある。面の図形は偶奇）。メッシュ・
    /// 当たり判定・動画の書き出しが、この決まりで塗る
    pub(crate) fn fills_nonzero(&self) -> bool {
        self.line.is_some()
    }

    /// 色を `f` で替える（色効果）。再テッセレーションを行わず、キャッシュ済みの頂点の色も `f` で替える
    pub(crate) fn recolor(&mut self, f: impl Fn(Color32) -> Color32) {
        self.color = f(self.color);
        self.drop_view_meshes();
        if let Some(arc_mesh) = self.fill_mesh.get_mut() {
            for v in &mut Arc::make_mut(arc_mesh).vertices {
                v.color = f(v.color);
            }
        }
        self.mesh_revision = next_mesh_revision();
    }

    pub(crate) fn bounding_box(&self) -> KRect {
        self.bbox
    }

    /// 実体のあるベクター輪郭を持つ場合のみバウンディングボックスを返す（空白フレームのシンボルマーカーの場合は `None`）
    #[inline]
    pub(crate) fn geometry_bounding_box(&self) -> Option<KRect> {
        (!self.is_empty_symbol_marker()).then_some(self.bbox)
    }

    pub(crate) fn contains_point(&self, pt: Point) -> bool {
        if !pt.x.is_finite() || !pt.y.is_finite() || !self.bbox.inflate(2.0, 2.0).contains(pt) {
            return false;
        }
        let path = self.hit_bez_path.get_or_init(|| {
            let mut p = BezPath::new();
            for c in self.contours() {
                c.append_to_bez_path(&mut p);
            }
            Arc::new(p)
        });
        let w = path.winding(pt);
        if self.fills_nonzero() {
            w != 0
        } else {
            (w & 1) != 0
        }
    }

    /// メッシュにするときの塗りの決まり（`fills_nonzero`）
    fn fill_rule(&self) -> FillRule {
        if self.fills_nonzero() {
            FillRule::NonZero
        } else {
            FillRule::EvenOdd
        }
    }

    pub(crate) fn translate(&mut self, delta: KVec2) {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return;
        }
        self.exterior.translate(delta);
        for hole in &mut self.holes {
            hole.translate(delta);
        }
        self.line_xform = Affine::translate(delta) * self.line_xform;
        self.drop_view_meshes();
        if let Some(binding) = &mut self.symbol_instance {
            binding.xform = Affine::translate(delta) * binding.xform;
        }
        self.bbox = self.bbox + delta;
        self.hit_bez_path = OnceLock::new();
        let d = egui::vec2(delta.x as f32, delta.y as f32);
        if let Some(arc_mesh) = self.fill_mesh.get_mut() {
            for v in &mut Arc::make_mut(arc_mesh).vertices {
                v.pos += d;
            }
        }
        self.mesh_revision = next_mesh_revision();
    }

    /// 変換をかける。持っているメッシュは、変換したあとも許すずれに収まるなら頂点を写して使い回し（`keeps_mesh`。
    /// メッシュを作ってから写した変換の拡大率を掛け合わせて見るので、小さな拡大を重ねても許すずれを超えない。再生・
    /// オニオンスキンで、トゥイーンのコマごとにシンボルの中身をテッセレーションし直さない）、収まらなければ捨てる（次に
    /// 要るとき、変換した曲線から作る）
    pub(crate) fn transform(&mut self, affine: Affine) {
        // 数が有限でない変換はかけない（潰れた変換はかける：`Contour::transform`）
        if !affine.is_finite() {
            return;
        }
        let coeffs = affine.as_coeffs();
        self.exterior.transform(affine);
        for hole in &mut self.holes {
            hole.transform(affine);
        }
        self.line_xform = affine * self.line_xform;
        self.drop_view_meshes();
        if let Some(binding) = &mut self.symbol_instance {
            binding.xform = affine * binding.xform;
        }
        let stretch = self.mesh_stretch * max_scale(affine);
        if !keeps_mesh(stretch) && !self.exterior.segments.is_empty() {
            self.rebuild_cache();
        } else {
            self.bbox = self.compute_bbox();
            self.hit_bez_path = OnceLock::new();
            if let Some(arc_mesh) = self.fill_mesh.get_mut() {
                transform_mesh_vertices_in_place(&mut Arc::make_mut(arc_mesh).vertices, coeffs);
                self.mesh_stretch = stretch;
            }
            self.mesh_revision = next_mesh_revision();
        }
    }

    /// 自由変形ドラッグ中、ドラッグ開始時の `orig` から既存の単一所有 `fill_mesh` 頂点バッファを再利用して `affine` を適用する
    /// （`clone_from` による `Arc` 参照カウント増加と毎フレームの頂点バッファ再確保を回避する）
    pub(crate) fn transform_from_origin(&mut self, orig: &FillShape, affine: Affine) {
        // 数が有限でない変換はかけない（潰れた変換はかける：`Contour::transform`）
        if !affine.is_finite() {
            self.clone_from(orig);
            return;
        }
        let coeffs = affine.as_coeffs();
        self.exterior = orig.exterior.clone();
        self.exterior.transform(affine);
        self.holes.clone_from(&orig.holes);
        for hole in &mut self.holes {
            hole.transform(affine);
        }
        self.center = orig.center;
        self.line_xform = affine * orig.line_xform;
        self.drop_view_meshes();
        self.symbol_instance = orig.symbol_instance.map(|mut binding| {
            binding.xform = affine * binding.xform;
            binding
        });
        self.bbox = self.compute_bbox();
        self.hit_bez_path = OnceLock::new();
        if let Some(src_mesh) = orig.fill_mesh.get() {
            let reused = if let Some(dst_arc) = self.fill_mesh.get_mut() {
                if dst_arc.vertices.len() == src_mesh.vertices.len()
                    && dst_arc.indices.len() == src_mesh.indices.len()
                {
                    let dst_mesh = Arc::make_mut(dst_arc);
                    dst_mesh.vertices.clone_from(&src_mesh.vertices);
                    transform_mesh_vertices_in_place(&mut dst_mesh.vertices, coeffs);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !reused {
                let mut cloned = (**src_mesh).clone();
                transform_mesh_vertices_in_place(&mut cloned.vertices, coeffs);
                self.fill_mesh = OnceLock::from(Arc::new(cloned));
            }
            self.mesh_stretch = orig.mesh_stretch * max_scale(affine);
        } else {
            self.fill_mesh = OnceLock::new();
            self.mesh_stretch = 1.0;
        }
        self.mesh_revision = next_mesh_revision();
    }

    /// 選択表示：選択の色を塗るメッシュ（`selection_mesh`。線の図形は両端を丸くした帯）を画面座標に写し、頂点の色を選択の
    /// 色（`selection_color`：図形の色に `SELECTION_TINT` を重ねた色）にしたもの。同じ段の図形の上に描く。どの倍率でも、
    /// 図形と同じ形のまま青みがかって見える
    pub(crate) fn selection_screen_mesh(&self, zoom: f32, pan: Vec2) -> Mesh {
        let mut mesh = (*self.selection_mesh(zoom)).clone();
        for v in &mut mesh.vertices {
            v.pos = to_screen(v.pos.x, v.pos.y, zoom, pan);
            v.color = selection_color(v.color);
        }
        mesh
    }

    pub(crate) fn get_contour(&self, idx: usize) -> Option<&Contour> {
        if self.exterior.segments.is_empty() {
            return None;
        }
        if idx == 0 {
            Some(&self.exterior)
        } else {
            self.holes.get(idx - 1)
        }
    }
}

// ============================================================================
// アフィン変換・幾何ユーティリティ関数
// ============================================================================

#[inline]
fn transform_mesh_vertices_in_place(vertices: &mut [Vertex], [a, b, c, d, tx, ty]: [f64; 6]) {
    for v in vertices {
        let x = v.pos.x as f64;
        let y = v.pos.y as f64;
        v.pos = Pos2::new((a * x + c * y + tx) as f32, (b * x + d * y + ty) as f32);
    }
}

/// 指定基準点 `anchor` を中心とした非一様スケーリング（および反転）のアフィン変換行列を生成する
pub(crate) fn affine_scale_around(anchor: Point, sx: f64, sy: f64) -> Affine {
    Affine::translate(anchor.to_vec2())
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate(-anchor.to_vec2())
}

/// 指定基準点 `pivot` を中心とした回転のアフィン変換行列を生成する
pub(crate) fn affine_rotate_around(pivot: Point, angle_rad: f64) -> Affine {
    Affine::translate(pivot.to_vec2())
        * Affine::rotate(angle_rad)
        * Affine::translate(-pivot.to_vec2())
}

/// 矩形選択用の `KRect` から4辺の直線 `QuadBez` を持つ `FillShape` を生成する
pub(crate) fn make_marquee_rect_shape(rect: KRect) -> FillShape {
    let p00 = Point::new(rect.x0, rect.y0);
    let p10 = Point::new(rect.x1, rect.y0);
    let p11 = Point::new(rect.x1, rect.y1);
    let p01 = Point::new(rect.x0, rect.y1);
    let segs = vec![
        QuadBez::new(p00, p00.lerp(p10, 0.5), p10),
        QuadBez::new(p10, p10.lerp(p11, 0.5), p11),
        QuadBez::new(p11, p11.lerp(p01, 0.5), p01),
        QuadBez::new(p01, p01.lerp(p00, 0.5), p00),
    ];
    FillShape::new(Color32::WHITE, Contour::new(segs), Vec::new())
}

#[inline]
pub(crate) fn normalize_vec(v: KVec2) -> KVec2 {
    let len = v.hypot();
    if len > TINY_LEN {
        v / len
    } else {
        KVec2::new(1.0, 0.0)
    }
}

#[inline]
pub(crate) fn left_normal(v: KVec2) -> KVec2 {
    KVec2::new(-v.y, v.x)
}

/// 曲線 `q` の上で点 `p` にいちばん近い所の（パラメータ, 距離）。曲線の上のいちばん近い点を求めるのはこれ1つ
/// - どの所か：距離の2乗の導関数（3次式）の [0,1] の根と両端を全部比べて決める（kurbo の `ParamCurveNearest::nearest`。
///   2次ベジェでは精度の引数を使わない）ので、折り返した曲線でも大域のいちばん近い所になる
/// - パラメータの細かさ：3次式の根の公式は丸めで少しずれる（曲線の上の点でも 1e-9px ほど離れる）。同じ線を見分ける所
///   （`overlap`）では、曲線の上の頂点のパラメータがずれると折れ線がそろわないので、その根からニュートン法で、距離が
///   もう縮まなくなるまで詰める（根の所の谷の中を進むので、ほかの所へは移らない）
pub(crate) fn nearest_on_quad(q: &QuadBez, p: Point) -> (f64, f64) {
    let n = kurbo::ParamCurveNearest::nearest(q, p, 0.0);
    let (mut t, mut d) = (n.t, n.distance_sq);
    let second = (q.p0.to_vec2() - q.p1.to_vec2() * 2.0 + q.p2.to_vec2()) * 2.0;
    loop {
        let b = q.eval(t) - p;
        let first = quad_deriv(q, t);
        let next = (t - b.dot(first) / (first.dot(first) + b.dot(second))).clamp(0.0, 1.0);
        let dn = (q.eval(next) - p).hypot2();
        if dn.total_cmp(&d).is_ge() {
            break;
        }
        (t, d) = (next, dn);
    }
    (t, d.sqrt())
}

/// 曲線 `q` の t での向き（微分）
pub(crate) fn quad_deriv(q: &QuadBez, t: f64) -> KVec2 {
    (q.p1 - q.p0) * (2.0 * (1.0 - t)) + (q.p2 - q.p1) * (2.0 * t)
}

/// 閉じた折れ線 `pts`（最後の点から最初の点へ閉じる）の符号付き面積（面積の式。小数の面積の式はこれ1つ。格子の点は
/// `planar_map` の `area2_of`）。点が3つより少なければ 0
pub(crate) fn signed_area_f64(pts: impl IntoIterator<Item = [f64; 2]>) -> f64 {
    let mut it = pts.into_iter();
    let Some(first) = it.next() else {
        return 0.0;
    };
    let (mut prev, mut sum, mut n) = (first, 0.0, 1usize);
    for p in it {
        sum += prev[0] * p[1] - p[0] * prev[1];
        prev = p;
        n += 1;
    }
    if n < 3 {
        0.0
    } else {
        0.5 * (sum + prev[0] * first[1] - first[0] * prev[1])
    }
}

pub(crate) fn point_to_segment_dist(pt: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len_sq = ab.hypot2();
    if len_sq < TINY_LEN * TINY_LEN {
        return (pt - a).hypot();
    }
    let t = ((pt - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (pt - (a + ab * t)).hypot()
}

pub(crate) fn rects_overlap(a: KRect, b: KRect) -> bool {
    a.x0 <= b.x1 && a.x1 >= b.x0 && a.y0 <= b.y1 && a.y1 >= b.y0
}

pub(crate) fn union_bounding_boxes(shapes: &[FillShape]) -> KRect {
    let Some(first) = shapes.first() else {
        return KRect::ZERO;
    };
    shapes[1..]
        .iter()
        .fold(first.bounding_box(), |acc, s| acc.union(s.bounding_box()))
}

// ============================================================================
// Lyon テッセレーション描画エンジン (2次ベジェパス -> GPU Mesh)
// ============================================================================

fn tessellate_contours_to_mesh(
    exterior: &Contour,
    holes: &[Contour],
    color: Color32,
    fill_rule: FillRule,
    tol: f64,
) -> Mesh {
    let mut builder = LyonPath::builder();
    for contour in std::iter::once(exterior).chain(holes.iter()) {
        if let Some(first) = contour.segments.first() {
            let p0 = first.p0;
            builder.begin(lyon_point(p0.x as f32, p0.y as f32));
            for seg in contour.segments.iter() {
                let p1 = seg.p1;
                let p2 = seg.p2;
                builder.quadratic_bezier_to(
                    lyon_point(p1.x as f32, p1.y as f32),
                    lyon_point(p2.x as f32, p2.y as f32),
                );
            }
            builder.close();
        }
    }
    let path = builder.build();
    TESSELLATOR.with(|t| tessellate_lyon_path(&mut t.borrow_mut(), &path, color, fill_rule, tol))
}

/// 曲線をメッシュにするときの許容（文書の座標）。図形が持つメッシュ（`fill_mesh`）と、セル画の結合メッシュはこれで作る
/// （倍率が高いときに画面に描くメッシュは、段ごとにこの半分ずつ：`mesh_level`）
pub(crate) const MESH_TOL: f64 = 0.04;

/// 倍率 `zoom` で画面に描くメッシュの段 k（許容 `MESH_TOL / 2^k`）：画面でのずれ（許容 × 倍率）が見分けられないずれ
/// `MESH_MAX_ERROR`（画面の画素の 1/4）を超えない、いちばん小さい k。6.25 倍までは 0（`fill_mesh`）。段にするのは、
/// 倍率を少し変えるたびに作り直さないため（作り直すのは、倍率が 2 倍の境をまたいだときだけ）。倍率は表示の倍率
/// （`MIN_VIEW_ZOOM..=MAX_VIEW_ZOOM`）
pub(crate) fn mesh_level(zoom: f32) -> u32 {
    let over = MESH_TOL * f64::from(zoom) / MESH_MAX_ERROR;
    if over > 1.0 {
        over.log2().ceil() as u32
    } else {
        0
    }
}

/// 拡大して描くときの細かいメッシュの入れ物（`FillShape::view_mesh`）：（段, メッシュ）を1つ。図形を写す（複製する）
/// ときは持ち越さない（形を変える前の複製から作られた図形が、古い形のメッシュを持たない）
#[derive(Default, Debug)]
struct ZoomMesh(Mutex<Option<(u32, Arc<Mesh>)>>);

impl Clone for ZoomMesh {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl ZoomMesh {
    /// 鍵 `key`（段など）のメッシュ（持っていればそれ、無ければ `make` で作って持つ。鍵が変われば置き換える）
    fn get_or_make(&self, key: u32, make: impl FnOnce() -> Mesh) -> Arc<Mesh> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, m)) = &*slot {
            if *k == key {
                return Arc::clone(m);
            }
        }
        let m = Arc::new(make());
        *slot = Some((key, Arc::clone(&m)));
        m
    }
}

/// 段 `level` の許容（`MESH_TOL / 2^level`：`mesh_level`）
fn level_tol(level: u32) -> f64 {
    MESH_TOL * 0.5f64.powi(level as i32)
}
/// 持っているメッシュの頂点を写して使い回すときに許すずれ（文書の座標）：見分けられないずれ（画面の画素の 1/4。形を
/// 曲線に当てはめるときの許容 `FIT_TOL_PX` と同じ決まり）を、100% 表示の文書の座標にしたもの。形そのものが当てはめで
/// これだけずれうるので、メッシュのずれがこの中なら、作り直しても見た目は変わらない（作る許容の 6.25 倍、つまり 6.25 倍
/// までの拡大なら使い回す。再生でトゥイーンのコマごとにテッセレーションし直さない）
const MESH_MAX_ERROR: f64 = crate::vector_stroke::FIT_TOL_PX;

/// 持っているメッシュを、作ってから頂点を写した変換の最大の拡大率の積 `stretch` のまま使い回してよいか：メッシュのずれ
/// （作る許容 × `stretch`）が許すずれを超えない。メッシュを写すかどうかは、どの変換でもこれ1つで決める
fn keeps_mesh(stretch: f64) -> bool {
    MESH_TOL * stretch <= MESH_MAX_ERROR
}

/// 変換 `affine` の最大の拡大率（AᵀA の大きいほうの固有値の平方根。列の長さの2乗 p・q と内積 r から）
pub(crate) fn max_scale(affine: Affine) -> f64 {
    let [a, b, c, d, _, _] = affine.as_coeffs();
    let (p, q, r) = (a * a + b * b, c * c + d * d, a * c + b * d);
    ((p + q) / 2.0 + (((p - q) / 2.0).powi(2) + r * r).sqrt()).sqrt()
}

/// 選択表示で、選んだ図形の色に重ねる半透明の色（明るい青）
const SELECTION_TINT: Color32 = Color32::from_rgba_premultiplied(0, 46, 100, 100);

/// 選んだ図形を塗る色：図形の色 `c` に `SELECTION_TINT` を重ねた色（どちらも前もって不透明度を掛けた色で、重ねる式は
/// `t + c × (1 − t の不透明度)`）。不透明な図形なら不透明なので、選んだ図形どうしがどう重なっても（線の丸い端・曲がり角の
/// 内側・塗りと輪郭の線）、2回塗られて濃くならない。同じ段（同じレイヤー・同じ階層の塗りどうし・線どうし）の図形を描き
/// 終えた所で塗るので、選んだ図形は、同じ段の中ではいちばん上に見える（利用者が決めた）。不透明度を下げた図形は、図形の色
/// が2回重なる分、選んでいる間は少し濃く見える
pub(crate) fn selection_color(c: Color32) -> Color32 {
    let t = SELECTION_TINT;
    let keep = 255 - u16::from(t.a());
    let over = |tc: u8, cc: u8| (u16::from(tc) + (u16::from(cc) * keep + 127) / 255).min(255) as u8;
    Color32::from_rgba_premultiplied(over(t.r(), c.r()), over(t.g(), c.g()), over(t.b(), c.b()), over(t.a(), c.a()))
}

pub(crate) fn tessellate_raw_polygon(pts: &[Point], color: Color32) -> Mesh {
    if pts.len() < 3 {
        return Mesh::default();
    }
    let mut builder = LyonPath::builder();
    builder.begin(lyon_point(pts[0].x as f32, pts[0].y as f32));
    for p in &pts[1..] {
        builder.line_to(lyon_point(p.x as f32, p.y as f32));
    }
    builder.close();
    let path = builder.build();
    TESSELLATOR.with(|t| tessellate_lyon_path(&mut t.borrow_mut(), &path, color, FillRule::NonZero, MESH_TOL))
}

fn tessellate_lyon_path(
    tessellator: &mut FillTessellator,
    path: &LyonPath,
    color: Color32,
    fill_rule: FillRule,
    tol: f64,
) -> Mesh {
    let mut buffers: VertexBuffers<Vertex, u32> = VertexBuffers::new();
    let options = FillOptions::tolerance(tol as f32).with_fill_rule(fill_rule);

    if tessellator
        .tessellate_path(
            path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| {
                let p = vertex.position();
                Vertex {
                    pos: Pos2::new(p.x, p.y),
                    uv: egui::epaint::WHITE_UV,
                    color,
                }
            }),
        )
        .is_err()
    {
        return Mesh::default();
    }

    buffers.vertices.shrink_to_fit();
    buffers.indices.shrink_to_fit();
    Mesh {
        vertices: buffers.vertices,
        indices: buffers.indices,
        ..Default::default()
    }
}

/// シンボルインプレイス編集時の親シーン背景表示用に、頂点色を薄くブレンドする
#[inline]
pub(crate) fn dim_backdrop_color(c: Color32) -> Color32 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(
        ((r as u16 * 30 + 250 * 70) / 100) as u8,
        ((g as u16 * 30 + 250 * 70) / 100) as u8,
        ((b as u16 * 30 + 252 * 70) / 100) as u8,
        a,
    )
}

/// オニオンスキン表示用に、頂点色を指定されたティント色・不透明度へブレンドする
#[inline]
pub(crate) fn onion_tint_color(c: Color32, tint_rgb: [u8; 3], alpha: u8) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(
        ((r as u16 * 35 + tint_rgb[0] as u16 * 65) / 100) as u8,
        ((g as u16 * 35 + tint_rgb[1] as u16 * 65) / 100) as u8,
        ((b as u16 * 35 + tint_rgb[2] as u16 * 65) / 100) as u8,
        alpha,
    )
}

/// セル画を1枚の結合メッシュで描くときの、色の変え方
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CelTint {
    /// そのまま
    Plain,
    /// グループ・シンボルの編集中の、ほかの所の薄い表示（`dim_backdrop_color`）
    Dim,
    /// オニオンスキン（色と不透明度：`onion_tint_color`）
    Onion([u8; 3], u8),
}

/// ワールド座標の結合メッシュ `world` を、ズーム `zoom`・パン `pan` の画面座標に写し、色を `tint` で変えたもの
pub(crate) fn tinted_screen_mesh(world: &Mesh, zoom: f32, pan: Vec2, tint: CelTint) -> Mesh {
    let mesh = world.clone();
    match tint {
        CelTint::Plain => transform_mesh_to_screen(mesh, zoom, pan, None),
        CelTint::Dim => transform_mesh_to_screen(mesh, zoom, pan, Some(&dim_backdrop_color)),
        CelTint::Onion(rgb, alpha) => {
            transform_mesh_to_screen(mesh, zoom, pan, Some(&|c| onion_tint_color(c, rgb, alpha)))
        }
    }
}

/// 座標 (`x`, `y`) を、ズーム `zoom`・パン `pan` の画面の座標にする（画面に写す式はこれ1つ）
#[inline]
pub(crate) fn to_screen(x: f32, y: f32, zoom: f32, pan: Vec2) -> Pos2 {
    Pos2::new(x * zoom + pan.x, y * zoom + pan.y)
}

/// ワールド座標で構築された `Mesh` を複製せずその場でスクリーン座標へ変換し、任意の頂点色変換 `map_color` を適用して返す
pub(crate) fn transform_mesh_to_screen(
    mut mesh: Mesh,
    zoom: f32,
    pan: Vec2,
    map_color: Option<&dyn Fn(Color32) -> Color32>,
) -> Mesh {
    if zoom == 1.0 && pan == Vec2::ZERO && map_color.is_none() {
        return mesh;
    }
    let mut last_color: Option<(Color32, Color32)> = None;
    for v in &mut mesh.vertices {
        v.pos = to_screen(v.pos.x, v.pos.y, zoom, pan);
        if let Some(f) = map_color {
            v.color = match last_color {
                Some((src, dst)) if src == v.color => dst,
                _ => {
                    let dst = f(v.color);
                    last_color = Some((v.color, dst));
                    dst
                }
            };
        }
    }
    mesh
}

/// 複数の `Mesh` を1つのバッチ `Mesh` に結合する（セル単位の一括キャッシュ構築用）
pub(crate) fn append_mesh(dst: &mut Mesh, src: &Mesh) {
    if src.is_empty() {
        return;
    }
    let base = dst.vertices.len() as u32;
    dst.vertices.extend_from_slice(&src.vertices);
    dst.indices
        .extend(src.indices.iter().map(|&idx| base + idx));
}
